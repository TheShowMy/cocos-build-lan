use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", path.display()))?;
    }
    Ok(())
}

pub fn read_lines(path: &Path) -> Result<Vec<String>> {
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn write_lines(path: &Path, lines: &[String]) -> Result<()> {
    ensure_parent(path)?;
    let content = lines.join("\n");
    fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn unique_sorted(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let set: BTreeSet<String> = items.into_iter().collect();
    set.into_iter().collect()
}

pub fn default_mapping_path() -> PathBuf {
    PathBuf::from("obfuscation_mapping.json")
}
