use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::Regex;
use walkdir::WalkDir;

use super::io::{read_lines, unique_sorted, write_lines};

pub fn collect_exclude_keywords(assets_dir: &Path) -> Result<Vec<String>> {
    let files: Vec<_> = WalkDir::new(assets_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();

    let mut base_words = Vec::new();
    for entry in WalkDir::new(assets_dir).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_dir()
            && let Some(name) = entry.path().file_name().and_then(|x| x.to_str())
        {
            base_words.push(name.to_string());
        }
    }

    for path in &files {
        if path.extension().and_then(|x| x.to_str()) == Some("meta") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|x| x.to_str()) {
            base_words.push(stem.to_string());
        }
    }

    let regex_proto = Regex::new(r"(\w+?)\??:").context("compile proto regex")?;
    let regex_json = Regex::new(r#"['\"]?(\w+?)['\"]?:"#).context("compile json regex")?;
    let regex_str = Regex::new(r#"["']([a-zA-Z_]\w*?)["']"#).context("compile string regex")?;
    let regex_name = Regex::new(r"\.name = `(\w+)\$").context("compile name regex")?;
    let regex_platform = Regex::new(r"(?:\.platform\?\.|\.platform\.|\._platform\?\.|\._platform\.|\bwx\.|\bqq\.|\btt\.|\bqg\.)(\w+)")
        .context("compile platform regex")?;

    let extracted: Vec<String> = files
        .par_iter()
        .flat_map_iter(|path| {
            let ext = path
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or_default();
            if !matches!(ext, "json" | "ts" | "js" | "prefab" | "scene") {
                return Vec::new().into_iter();
            }
            let Ok(content) = fs::read_to_string(path) else {
                return Vec::new().into_iter();
            };

            let file_name = path
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or_default();
            let mut local = Vec::new();
            if file_name == "ProtoInterface.d" {
                local.extend(
                    regex_proto
                        .captures_iter(&content)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
                );
            } else if file_name == "JsonInterface.d" {
                local.extend(
                    regex_json
                        .captures_iter(&content)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
                );
            } else {
                local.extend(
                    regex_str
                        .captures_iter(&content)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
                );
                local.extend(
                    regex_name
                        .captures_iter(&content)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
                );
                local.extend(
                    regex_platform
                        .captures_iter(&content)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
                );
            }
            local.into_iter()
        })
        .collect();

    base_words.extend(extracted);
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for item in base_words {
        if seen.insert(item.clone()) {
            output.push(item);
        }
    }
    Ok(unique_sorted(output))
}

pub fn ensure_exclude_keywords(
    dict_dir: &Path,
    assets_dir: &Path,
    refresh: bool,
) -> Result<Vec<String>> {
    let exclude_path = dict_dir.join("exclude.txt");
    if refresh || !exclude_path.exists() {
        refresh_exclude(dict_dir, assets_dir)?;
    }

    let mut exclude_keywords: Vec<String> = read_lines(&exclude_path)?
        .into_iter()
        .filter(|s| s.len() >= 6)
        .collect();

    let js_keywords = read_lines(&dict_dir.join("js_keywords.txt"))?;
    for item in js_keywords {
        if item.len() >= 6 {
            exclude_keywords.push(item);
        }
    }
    let exclude_keywords = unique_sorted(exclude_keywords);
    write_lines(&dict_dir.join("exclude_keywords.txt"), &exclude_keywords)?;
    Ok(exclude_keywords)
}

pub fn refresh_exclude(dict_dir: &Path, assets_dir: &Path) -> Result<()> {
    let output = collect_exclude_keywords(assets_dir)?;
    write_lines(&dict_dir.join("exclude.txt"), &output)?;
    Ok(())
}
