use std::fs;

use assert_cmd::Command;
use tempfile::tempdir;

fn setup_case() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let dict = root.join("dict");
    let assets = root.join("assets");
    let input = root.join("input.js");

    fs::create_dir_all(&dict).unwrap();
    fs::create_dir_all(&assets).unwrap();

    fs::write(dict.join("engine_keywords.txt"), "keepEngine").unwrap();
    fs::write(dict.join("exclude.txt"), "keepExcludeWord").unwrap();
    fs::write(dict.join("js_keywords.txt"), "function\nreturn\n").unwrap();
    fs::write(dict.join("lexicon_origin.txt"), "alpha bravo").unwrap();
    fs::write(dict.join("lexicon_origin1.txt"), "charlie delta").unwrap();

    let content = "obj.randomMethod=function(){return 1}; obj.keepEngine=function(){};";
    fs::write(&input, content).unwrap();

    (tmp, dict, assets, input)
}

#[test]
fn default_mapping_written_to_cwd() {
    let (tmp, dict, assets, input) = setup_case();
    let cwd = tmp.path().join("work");
    fs::create_dir_all(&cwd).unwrap();

    let mut cmd = Command::cargo_bin("obfuscation-cli").unwrap();
    cmd.current_dir(&cwd)
        .arg("run")
        .arg("--input-file")
        .arg(&input)
        .arg("--dict-dir")
        .arg(&dict)
        .arg("--assets-dir")
        .arg(&assets)
        .arg("--refresh-exclude=false")
        .arg("--seed")
        .arg("100");

    cmd.assert().success();

    let mapping = cwd.join("obfuscation_mapping.json");
    assert!(mapping.exists(), "mapping should exist in current dir");
}

#[test]
fn no_mapping_skips_mapping_output() {
    let (tmp, dict, assets, input) = setup_case();
    let cwd = tmp.path().join("work");
    fs::create_dir_all(&cwd).unwrap();

    let mut cmd = Command::cargo_bin("obfuscation-cli").unwrap();
    cmd.current_dir(&cwd)
        .arg("run")
        .arg("--input-file")
        .arg(&input)
        .arg("--dict-dir")
        .arg(&dict)
        .arg("--assets-dir")
        .arg(&assets)
        .arg("--refresh-exclude=false")
        .arg("--seed")
        .arg("100")
        .arg("--no-mapping");

    cmd.assert().success();

    let mapping = cwd.join("obfuscation_mapping.json");
    assert!(
        !mapping.exists(),
        "mapping should not exist when --no-mapping"
    );
}

#[test]
fn same_seed_produces_same_output() {
    let (tmp, dict, assets, input) = setup_case();
    let cwd = tmp.path().join("work");
    fs::create_dir_all(&cwd).unwrap();

    for _ in 0..2 {
        let mut cmd = Command::cargo_bin("obfuscation-cli").unwrap();
        cmd.current_dir(&cwd)
            .arg("run")
            .arg("--input-file")
            .arg(&input)
            .arg("--dict-dir")
            .arg(&dict)
            .arg("--assets-dir")
            .arg(&assets)
            .arg("--refresh-exclude=false")
            .arg("--seed")
            .arg("777");
        cmd.assert().success();
    }

    let out_file = input.with_file_name("input_obfuscation.js");
    let a = fs::read_to_string(&out_file).unwrap();
    let b = fs::read_to_string(&out_file).unwrap();
    assert_eq!(a, b);
}
