use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::engine::collect_engine_keywords;
use super::exclude::collect_exclude_keywords;
use super::io::{ensure_parent, read_lines, unique_sorted};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhitelistDocument {
    pub engine_keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    pub js_keywords: Vec<String>,
    pub merged_keywords: Vec<String>,
}

pub fn whitelist_path(dict_dir: &Path) -> PathBuf {
    dict_dir.join("whitelist.json")
}

pub fn ensure_whitelist(
    dict_dir: &Path,
    assets_dir: &Path,
    build_dir: Option<&Path>,
    refresh_engine: bool,
    refresh_exclude: bool,
) -> Result<WhitelistDocument> {
    let path = whitelist_path(dict_dir);
    let existing = read_whitelist(&path).unwrap_or_default();
    let legacy_engine = read_optional_lines(&dict_dir.join("engine_keywords.txt"));
    let legacy_exclude = read_optional_lines(&dict_dir.join("exclude.txt"));

    let engine_keywords = if refresh_engine {
        let build = build_dir.context("使用 --refresh-engine 时必须提供 --build-dir")?;
        collect_engine_keywords(build)?
    } else if !existing.engine_keywords.is_empty() {
        existing.engine_keywords
    } else if let Some(words) = legacy_engine {
        words
    } else if let Some(build) = build_dir {
        collect_engine_keywords(build)?
    } else {
        bail!("缺少引擎白名单，请提供 --build-dir 或启用 --refresh-engine");
    };

    let exclude_keywords = if refresh_exclude {
        collect_exclude_keywords(assets_dir)?
    } else if !existing.exclude_keywords.is_empty() {
        existing.exclude_keywords
    } else if let Some(words) = legacy_exclude {
        words
    } else {
        collect_exclude_keywords(assets_dir)?
    };

    let js_keywords = load_js_keywords(dict_dir, Some(assets_dir), &existing.js_keywords)?;

    let doc = build_whitelist(engine_keywords, exclude_keywords, js_keywords);
    write_whitelist(&path, &doc)?;
    Ok(doc)
}

pub fn refresh_whitelist_exclude(dict_dir: &Path, assets_dir: &Path) -> Result<WhitelistDocument> {
    let path = whitelist_path(dict_dir);
    let existing = read_whitelist(&path).unwrap_or_default();

    let exclude_keywords = collect_exclude_keywords(assets_dir)?;
    let js_keywords = load_js_keywords(dict_dir, Some(assets_dir), &existing.js_keywords)?;
    let engine_keywords = if !existing.engine_keywords.is_empty() {
        existing.engine_keywords
    } else {
        read_optional_lines(&dict_dir.join("engine_keywords.txt")).unwrap_or_default()
    };

    let doc = build_whitelist(engine_keywords, exclude_keywords, js_keywords);
    write_whitelist(&path, &doc)?;
    Ok(doc)
}

pub fn refresh_whitelist_engine(dict_dir: &Path, build_dir: &Path) -> Result<WhitelistDocument> {
    let path = whitelist_path(dict_dir);
    let existing = read_whitelist(&path).unwrap_or_default();

    let engine_keywords = collect_engine_keywords(build_dir)?;
    let js_keywords = load_js_keywords(dict_dir, None, &existing.js_keywords)?;
    let exclude_keywords = if !existing.exclude_keywords.is_empty() {
        existing.exclude_keywords
    } else {
        read_optional_lines(&dict_dir.join("exclude.txt")).unwrap_or_default()
    };

    let doc = build_whitelist(engine_keywords, exclude_keywords, js_keywords);
    write_whitelist(&path, &doc)?;
    Ok(doc)
}

fn read_whitelist(path: &Path) -> Result<WhitelistDocument> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))
}

fn write_whitelist(path: &Path, doc: &WhitelistDocument) -> Result<()> {
    ensure_parent(path)?;
    let content = serde_json::to_string_pretty(doc).context("serialize whitelist json")?;
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn read_optional_lines(path: &Path) -> Option<Vec<String>> {
    if !path.exists() {
        return None;
    }
    read_lines(path).ok()
}

fn load_js_keywords(
    dict_dir: &Path,
    assets_dir: Option<&Path>,
    existing_js_keywords: &[String],
) -> Result<Vec<String>> {
    let local = dict_dir.join("js_keywords.txt");
    if local.exists() {
        return read_lines(&local).with_context(|| format!("read {}", local.display()));
    }

    if !existing_js_keywords.is_empty() {
        return Ok(existing_js_keywords.to_vec());
    }

    if let Some(assets_dir) = assets_dir
        && let Some(project_root) = assets_dir.parent()
    {
        let fallback = project_root.join("python/obfuscation/js_keywords.txt");
        if fallback.exists() {
            return read_lines(&fallback).with_context(|| format!("read {}", fallback.display()));
        }
    }

    bail!(
        "缺少 js_keywords.txt：未在 {} 找到，且无法从 assets 推断项目词典路径",
        local.display()
    )
}

fn build_whitelist(
    engine_keywords: Vec<String>,
    exclude_keywords: Vec<String>,
    js_keywords: Vec<String>,
) -> WhitelistDocument {
    let mut merged_keywords = Vec::new();
    merged_keywords.extend(engine_keywords.iter().cloned());
    merged_keywords.extend(exclude_keywords.iter().filter(|s| s.len() >= 6).cloned());
    merged_keywords.extend(js_keywords.iter().filter(|s| s.len() >= 6).cloned());
    let merged_keywords = unique_sorted(merged_keywords);

    WhitelistDocument {
        engine_keywords,
        exclude_keywords,
        js_keywords,
        merged_keywords,
    }
}
