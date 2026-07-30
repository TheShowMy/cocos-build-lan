use std::{
    fs,
    path::{Path, PathBuf},
};

use obfuscation_cli::pipeline::{
    obfuscate::{RunConfig, run as run_obfuscation},
    whitelist::{WhitelistDocument, ensure_whitelist},
};
use tokio::task;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct ObfuscationResult {
    pub target_input_path: PathBuf,
    pub target_output_path: PathBuf,
    pub work_dir: PathBuf,
    pub whitelist_path: PathBuf,
    pub whitelist_keyword_count: usize,
    pub mapping_path: Option<PathBuf>,
    pub file_count: usize,
    pub replaced_word_count: usize,
    pub copied_back: bool,
}

fn copy_if_exists(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }

    fs::copy(source, target).map_err(|error| {
        format!(
            "复制混淆文件失败: source={}, target={}, error={error}",
            source.display(),
            target.display(),
        )
    })?;
    Ok(())
}

pub(crate) fn resolve_engine_build_dir_from_game_js(
    target_input_path: &Path,
) -> Result<PathBuf, String> {
    let has_engine_dirs = |dir: &Path| {
        ["cocos-js", "libs", "src"]
            .iter()
            .any(|name| dir.join(name).exists())
    };

    for ancestor in target_input_path.ancestors().skip(1) {
        if has_engine_dirs(ancestor) {
            return Ok(ancestor.to_path_buf());
        }
    }

    Err(format!(
        "未找到代码包构建目录，目标 game.js 附近不存在 cocos-js/libs/src: {}",
        target_input_path.display()
    ))
}

pub(crate) fn prepare_task_obfuscation_dict(
    main_repo_path: &Path,
    build_dir: &Path,
    obfuscation_dir: &Path,
) -> Result<(WhitelistDocument, PathBuf), String> {
    let source_dict_dir = main_repo_path.join("python").join("obfuscation");
    let assets_dir = main_repo_path.join("assets");
    let task_dict_dir = obfuscation_dir.to_path_buf();

    if !source_dict_dir.exists() {
        return Err(format!("未找到混淆词典目录: {}", source_dict_dir.display()));
    }
    if !assets_dir.exists() {
        return Err(format!(
            "未找到主工程 assets 目录: {}",
            assets_dir.display()
        ));
    }

    fs::create_dir_all(&task_dict_dir).map_err(|error| {
        format!(
            "创建任务混淆目录失败: dir={}, error={error}",
            task_dict_dir.display()
        )
    })?;

    for file_name in [
        "js_keywords.txt",
        "lexicon_origin.txt",
        "lexicon_origin1.txt",
        "lexicon.txt",
    ] {
        copy_if_exists(
            &source_dict_dir.join(file_name),
            &task_dict_dir.join(file_name),
        )?;
    }

    let whitelist = ensure_whitelist(&task_dict_dir, &assets_dir, Some(build_dir), true, true)
        .map_err(|error| format!("准备混淆白名单失败: {error:#}"))?;

    Ok((whitelist, task_dict_dir))
}

pub async fn run_code_package_obfuscation(
    main_repo_path: &Path,
    code_repo_path: &Path,
    task_dir: &Path,
    seed: Option<u64>,
) -> Result<ObfuscationResult, String> {
    let main_repo_path = main_repo_path.to_path_buf();
    let code_repo_path = code_repo_path.to_path_buf();
    let task_dir = task_dir.to_path_buf();

    task::spawn_blocking(move || {
        run_code_package_obfuscation_blocking(&main_repo_path, &code_repo_path, &task_dir, seed)
    })
    .await
    .map_err(|error| format!("执行混淆任务失败: {error}"))?
}

fn run_code_package_obfuscation_blocking(
    main_repo_path: &Path,
    code_repo_path: &Path,
    task_dir: &Path,
    seed: Option<u64>,
) -> Result<ObfuscationResult, String> {
    let target_input_path = find_unique_game_js(code_repo_path)?;
    let build_dir = resolve_engine_build_dir_from_game_js(&target_input_path)?;
    let obfuscation_dir = task_dir.join("obfuscation");
    let target_output_path = obfuscation_dir.join("game_obfuscation.js");
    let mapping_path = obfuscation_dir.join("obfuscation_mapping.json");
    fs::create_dir_all(&obfuscation_dir)
        .map_err(|error| format!("创建混淆输出目录失败: {error}"))?;
    let (whitelist, task_dict_dir) =
        prepare_task_obfuscation_dict(main_repo_path, &build_dir, &obfuscation_dir)?;

    let summary = run_obfuscation(
        RunConfig {
            input_dir: None,
            input_file: Some(target_input_path.clone()),
            output_dir: None,
            output_file: Some(target_output_path.clone()),
            dict_dir: task_dict_dir.clone(),
            dry_run: false,
            fail_fast: true,
            seed,
            min_len: 8,
            max_len: 14,
            no_mapping: false,
            mapping_out: Some(mapping_path.clone()),
        },
        &whitelist.engine_keywords,
        &whitelist.merged_keywords,
    )
    .map_err(|error| format!("执行代码包混淆失败: {error:#}"))?;

    if !target_output_path.exists() {
        return Err(format!(
            "未生成目标混淆文件: {}",
            target_output_path.display()
        ));
    }

    let original_content =
        fs::read(&target_input_path).map_err(|error| format!("读取目标 game.js 失败: {error}"))?;
    let obfuscated_content = fs::read(&target_output_path)
        .map_err(|error| format!("读取混淆后的 game.js 失败: {error}"))?;
    let copied_back = original_content != obfuscated_content;
    if copied_back {
        fs::write(&target_input_path, obfuscated_content)
            .map_err(|error| format!("回写混淆后的 game.js 失败: {error}"))?;
    }

    Ok(ObfuscationResult {
        target_input_path,
        target_output_path,
        work_dir: obfuscation_dir,
        whitelist_path: task_dict_dir.join("whitelist.json"),
        whitelist_keyword_count: whitelist.merged_keywords.len(),
        mapping_path: summary.mapping_path,
        file_count: summary.file_count,
        replaced_word_count: summary.replaced_word_count,
        copied_back,
    })
}

fn find_unique_game_js(code_repo_path: &Path) -> Result<PathBuf, String> {
    let mut matches = Vec::new();

    for entry in WalkDir::new(code_repo_path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.into_path();
        if is_target_game_js(&path) {
            matches.push(path);
        }
    }

    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(format!(
            "代码包中未找到唯一匹配的 **/subpackages/main/game.js: {}",
            code_repo_path.display()
        )),
        _ => Err(format!(
            "代码包中找到多个 **/subpackages/main/game.js，请手动确认目录结构: {}",
            code_repo_path.display()
        )),
    }
}

fn is_target_game_js(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|item| item.to_str()) else {
        return false;
    };
    if file_name != "game.js" {
        return false;
    }

    let Some(parent_name) = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|item| item.to_str())
    else {
        return false;
    };
    if parent_name != "main" {
        return false;
    }

    let Some(grand_parent_name) = path
        .parent()
        .and_then(|parent| parent.parent())
        .and_then(|parent| parent.file_name())
        .and_then(|item| item.to_str())
    else {
        return false;
    };

    grand_parent_name == "subpackages"
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_obfuscation_test_{name}_{unique}"))
    }

    #[test]
    fn resolve_engine_build_dir_from_game_js_should_find_package_root() {
        let temp_dir = temp_test_dir("engine_build_dir");
        let build_dir = temp_dir.join("pkg_a");
        std::fs::create_dir_all(build_dir.join("subpackages/main")).expect("create game dir");
        std::fs::create_dir_all(build_dir.join("cocos-js")).expect("create cocos-js dir");
        let target_input_path = build_dir.join("subpackages/main/game.js");

        let resolved = resolve_engine_build_dir_from_game_js(&target_input_path)
            .expect("resolve engine build dir");

        assert_eq!(resolved, build_dir);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn resolve_engine_build_dir_from_game_js_should_fail_when_engine_dirs_missing() {
        let target_input_path = Path::new("/tmp/code/pkg_a/subpackages/main/game.js");

        let error = resolve_engine_build_dir_from_game_js(target_input_path)
            .expect_err("missing engine dirs should fail");

        assert!(error.contains("未找到代码包构建目录"));
    }
}
