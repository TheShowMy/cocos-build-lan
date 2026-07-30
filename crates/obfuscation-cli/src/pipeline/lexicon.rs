use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use regex::Regex;

use super::io::{read_lines, unique_sorted, write_lines};

pub fn ensure_lexicon(dict_dir: &Path) -> Result<Vec<String>> {
    let lexicon_path = dict_dir.join("lexicon.txt");
    if !lexicon_path.exists() {
        let origin0 = dict_dir.join("lexicon_origin.txt");
        let origin1 = dict_dir.join("lexicon_origin1.txt");
        generate_lexicon(&origin0, &origin1, &lexicon_path)?;
    }
    let lexicon = read_lines(&lexicon_path)?;
    if lexicon.is_empty() {
        bail!("lexicon is empty: {}", lexicon_path.display());
    }
    Ok(lexicon)
}

pub fn generate_lexicon(origin0: &Path, origin1: &Path, out_path: &Path) -> Result<()> {
    let word_re = Regex::new(r"\b[a-zA-Z]+\b").context("compile lexicon regex")?;
    let mut words = Vec::new();
    for path in [origin0, origin1] {
        let content =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for cap in word_re.captures_iter(&content) {
            let w = cap.get(0).map(|m| m.as_str()).unwrap_or_default();
            if w.len() >= 4 {
                words.push(w.to_string());
            }
        }
    }
    let words = unique_sorted(words);
    write_lines(out_path, &words)?;
    Ok(())
}
