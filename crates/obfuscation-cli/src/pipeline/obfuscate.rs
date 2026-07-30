use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

use super::io::{default_mapping_path, ensure_parent};
use super::namegen::NameGenerator;

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub input_dir: Option<PathBuf>,
    pub input_file: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub output_file: Option<PathBuf>,
    pub dict_dir: PathBuf,
    pub dry_run: bool,
    pub fail_fast: bool,
    pub seed: Option<u64>,
    pub min_len: usize,
    pub max_len: usize,
    pub no_mapping: bool,
    pub mapping_out: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MappingMeta {
    pub seed: Option<u64>,
    pub thread_count: usize,
    pub input_mode: String,
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct MappingItem {
    pub file: String,
    pub original: String,
    pub obfuscated: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MappingDocument {
    pub meta: MappingMeta,
    pub items: Vec<MappingItem>,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub file_count: usize,
    pub replaced_word_count: usize,
    pub output_files: Vec<PathBuf>,
    pub mapping_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct FileKeywords {
    path: PathBuf,
    candidates: BTreeSet<String>,
}

struct StageProgress {
    total: usize,
    current: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
    ticker: Option<thread::JoinHandle<()>>,
    enabled: bool,
}

impl StageProgress {
    fn new(label: &'static str, total: usize) -> Self {
        let enabled = io::stderr().is_terminal();
        let current = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let ticker = if enabled {
            let current_ref = Arc::clone(&current);
            let done_ref = Arc::clone(&done);
            Some(thread::spawn(move || {
                loop {
                    let cur = current_ref.load(Ordering::Relaxed);
                    render_progress_line(label, cur, total);
                    if done_ref.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(120));
                }
                eprintln!();
            }))
        } else {
            None
        };

        Self {
            total,
            current,
            done,
            ticker,
            enabled,
        }
    }

    fn counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.current)
    }

    fn finish(mut self) {
        self.current.store(self.total, Ordering::Relaxed);
        self.stop();
    }

    fn stop(&mut self) {
        if !self.enabled {
            return;
        }
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.ticker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for StageProgress {
    fn drop(&mut self) {
        self.stop();
    }
}

fn render_progress_line(label: &str, current: usize, total: usize) {
    const BAR_WIDTH: usize = 28;
    let safe_total = total.max(1);
    let clamped = current.min(total);
    let filled = clamped * BAR_WIDTH / safe_total;
    let mut bar = String::with_capacity(BAR_WIDTH);
    bar.push_str(&"=".repeat(filled));
    bar.push_str(&" ".repeat(BAR_WIDTH - filled));
    let percent = clamped * 100 / safe_total;
    eprint!("\r[{label}] [{bar}] {clamped}/{total} ({percent:>3}%)");
    let _ = io::stderr().flush();
}

pub fn run(
    config: RunConfig,
    engine_keywords: &[String],
    exclude_keywords: &[String],
) -> Result<RunSummary> {
    let input_stage = StageProgress::new("收集输入文件", 1);
    let input_files_res = resolve_input_files(&config);
    input_stage.finish();
    let input_files = input_files_res?;
    if input_files.is_empty() {
        bail!("no input js files found");
    }

    let engine: HashSet<String> = engine_keywords.iter().cloned().collect();
    let exclude: HashSet<String> = exclude_keywords.iter().cloned().collect();

    let extract_stage = StageProgress::new("提取候选符号", input_files.len());
    let extract_counter = extract_stage.counter();
    let extracted_res: Result<Vec<FileKeywords>> = input_files
        .par_iter()
        .map(|path| {
            let out = extract_file_keywords(path, &engine, &exclude);
            extract_counter.fetch_add(1, Ordering::Relaxed);
            out
        })
        .collect::<Result<Vec<_>>>();
    extract_stage.finish();
    let extracted = extracted_res?;

    // 先做全局候选集合，保证跨文件同名标识符映射到同一混淆名。
    let mut global_candidates = BTreeSet::new();
    for item in &extracted {
        global_candidates.extend(item.candidates.iter().cloned());
    }

    let mut blocked: HashSet<String> = engine.union(&exclude).cloned().collect();
    blocked.extend(global_candidates.iter().cloned());

    let map_stage = StageProgress::new("生成全局映射", global_candidates.len());
    let mut name_generator =
        NameGenerator::new(config.min_len, config.max_len, config.seed, blocked)?;
    let mut global_map = HashMap::new();
    for original in &global_candidates {
        global_map.insert(original.clone(), name_generator.next_name()?);
        map_stage.current.fetch_add(1, Ordering::Relaxed);
    }
    map_stage.finish();

    // fail_fast=true 时直接短路返回；否则收集全部错误并汇总输出。
    let process_stage = StageProgress::new("替换并写出文件", extracted.len());
    let process_counter = process_stage.counter();
    let mapped_files_res = if config.fail_fast {
        extracted
            .par_iter()
            .map(|f| {
                let out = process_file(f, &global_map, &config);
                process_counter.fetch_add(1, Ordering::Relaxed);
                out
            })
            .collect::<Result<Vec<_>>>()
    } else {
        let out: Vec<Result<ProcessedFile>> = extracted
            .par_iter()
            .map(|f| {
                let out = process_file(f, &global_map, &config);
                process_counter.fetch_add(1, Ordering::Relaxed);
                out
            })
            .collect();
        let mut ok = Vec::new();
        let mut errors = Vec::new();
        for item in out {
            match item {
                Ok(v) => ok.push(v),
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            let msg = errors
                .into_iter()
                .map(|e| format!("- {e:#}"))
                .collect::<Vec<_>>()
                .join("\n");
            bail!("file processing failed:\n{msg}");
        }
        Ok(ok)
    };
    process_stage.finish();
    let mapped_files = mapped_files_res?;

    let mut all_items = Vec::new();
    let mut output_files = Vec::new();
    let mut replaced_word_count = 0usize;
    for item in mapped_files {
        replaced_word_count += item.mapping_items.len();
        output_files.push(item.output_path.clone());
        all_items.extend(item.mapping_items);
    }
    all_items.sort();

    let mapping_path = if config.no_mapping || config.dry_run {
        None
    } else {
        let path = config
            .mapping_out
            .clone()
            .unwrap_or_else(default_mapping_path);
        let doc = MappingDocument {
            meta: MappingMeta {
                seed: config.seed,
                thread_count: rayon::current_num_threads(),
                input_mode: if config.input_dir.is_some() {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
            },
            items: all_items,
        };
        write_mapping(&path, &doc)?;
        Some(path)
    };

    Ok(RunSummary {
        file_count: output_files.len(),
        replaced_word_count,
        output_files,
        mapping_path,
    })
}

fn write_mapping(path: &Path, doc: &MappingDocument) -> Result<()> {
    ensure_parent(path)?;
    let content = serde_json::to_string_pretty(doc).context("serialize mapping json")?;
    fs::write(path, content).with_context(|| format!("write mapping {}", path.display()))?;
    Ok(())
}

fn resolve_input_files(config: &RunConfig) -> Result<Vec<PathBuf>> {
    let mut files = if let Some(file) = &config.input_file {
        vec![file.clone()]
    } else if let Some(dir) = &config.input_dir {
        WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("js"))
            .map(|e| e.into_path())
            .collect()
    } else {
        bail!("either --input-dir or --input-file is required");
    };
    files.sort();
    Ok(files)
}

fn extract_file_keywords(
    path: &Path,
    engine: &HashSet<String>,
    exclude: &HashSet<String>,
) -> Result<FileKeywords> {
    let content =
        fs::read_to_string(path).with_context(|| format!("read input {}", path.display()))?;
    let prototype_re =
        Regex::new(r"\.prototype\.(\w+?)=function\(").context("compile prototype regex")?;
    let method_re = Regex::new(r"\.(\w+?)=function\(").context("compile method regex")?;

    let prototypes: HashSet<String> = prototype_re
        .captures_iter(&content)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();

    let mut candidates = BTreeSet::new();
    for cap in method_re.captures_iter(&content) {
        let Some(m) = cap.get(1) else {
            continue;
        };
        let name = m.as_str();
        if name.len() < 6 {
            continue;
        }
        if prototypes.contains(name) {
            continue;
        }
        if engine.contains(name) || exclude.contains(name) {
            continue;
        }
        candidates.insert(name.to_string());
    }

    Ok(FileKeywords {
        path: path.to_path_buf(),
        candidates,
    })
}

#[derive(Debug, Clone)]
struct ProcessedFile {
    output_path: PathBuf,
    mapping_items: Vec<MappingItem>,
}

fn process_file(
    file: &FileKeywords,
    global_map: &HashMap<String, String>,
    config: &RunConfig,
) -> Result<ProcessedFile> {
    let input_content = fs::read_to_string(&file.path)
        .with_context(|| format!("read input {}", file.path.display()))?;
    let word_re = Regex::new(r"\b\w+\b").context("compile replacement regex")?;
    // 基于词边界做替换，避免把长标识符的一部分误替换掉。
    let replaced = word_re.replace_all(&input_content, |caps: &regex::Captures<'_>| {
        let token = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
        match global_map.get(token) {
            Some(v) => Cow::Owned(v.clone()),
            None => Cow::Owned(token.to_string()),
        }
    });

    let output_path = resolve_output_path(config, &file.path)?;
    if !config.dry_run {
        ensure_parent(&output_path)?;
        fs::write(&output_path, replaced.as_ref())
            .with_context(|| format!("write output {}", output_path.display()))?;
    }

    let mut mapping_items: Vec<MappingItem> = file
        .candidates
        .iter()
        .filter_map(|original| {
            global_map.get(original).map(|obfuscated| MappingItem {
                file: file.path.display().to_string(),
                original: original.clone(),
                obfuscated: obfuscated.clone(),
            })
        })
        .collect();
    mapping_items.sort();

    Ok(ProcessedFile {
        output_path,
        mapping_items,
    })
}

fn resolve_output_path(config: &RunConfig, input_file: &Path) -> Result<PathBuf> {
    if let Some(output_file) = &config.output_file {
        return Ok(output_file.clone());
    }

    let stem = input_file
        .file_stem()
        .and_then(|x| x.to_str())
        .context("invalid input file name")?;
    let ext = input_file
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("js");
    let output_name = format!("{stem}_obfuscation.{ext}");

    if let Some(input_dir) = &config.input_dir {
        let output_dir = config
            .output_dir
            .as_ref()
            .context("--output-dir is required with --input-dir")?;
        let relative = input_file.strip_prefix(input_dir).with_context(|| {
            format!(
                "strip prefix {} from {}",
                input_dir.display(),
                input_file.display()
            )
        })?;
        let mut out = output_dir.join(relative);
        out.set_file_name(output_name);
        Ok(out)
    } else {
        Ok(input_file.with_file_name(output_name))
    }
}
