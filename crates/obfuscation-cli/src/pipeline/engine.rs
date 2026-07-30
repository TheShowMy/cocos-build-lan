use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::Regex;
use walkdir::WalkDir;

use super::io::{read_lines, unique_sorted, write_lines};

pub fn collect_engine_keywords(build_dir: &Path) -> Result<Vec<String>> {
    let check_paths = ["cocos-js", "libs", "src"];
    let files: Vec<_> = check_paths
        .iter()
        .map(|sub| build_dir.join(sub))
        .filter(|p| p.exists())
        .flat_map(|base| {
            WalkDir::new(base)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path())
                .collect::<Vec<_>>()
        })
        .collect();

    let re0 = Regex::new(r"\.(\w+?)=function\(").context("compile engine regex #0")?;
    let re1 = Regex::new(r"(\b\w+?):function\(").context("compile engine regex #1")?;
    let re2 = Regex::new(r"\.(\w+?)\(").context("compile engine regex #2")?;

    let words: Vec<String> = files
        .par_iter()
        .flat_map_iter(|path| {
            let Ok(content) = fs::read_to_string(path) else {
                return Vec::new().into_iter();
            };
            let mut local = Vec::new();
            local.extend(
                re0.captures_iter(&content)
                    .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
            );
            local.extend(
                re1.captures_iter(&content)
                    .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
            );
            local.extend(
                re2.captures_iter(&content)
                    .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
            );
            local.into_iter()
        })
        .collect();

    Ok(unique_sorted(words))
}

pub fn ensure_engine_keywords(
    dict_dir: &Path,
    build_dir: Option<&Path>,
    refresh: bool,
) -> Result<Vec<String>> {
    let out_path = dict_dir.join("engine_keywords.txt");
    if refresh || !out_path.exists() {
        let build_dir =
            build_dir.context("build dir is required when engine keywords need refresh")?;
        extract_engine_keywords(build_dir, &out_path)?;
    }
    read_lines(&out_path)
}

pub fn extract_engine_keywords(build_dir: &Path, out_path: &Path) -> Result<()> {
    let words = collect_engine_keywords(build_dir)?;
    write_lines(out_path, &words)?;
    Ok(())
}
