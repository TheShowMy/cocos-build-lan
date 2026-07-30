use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use tokio::process::Command;
use walkdir::WalkDir;

use super::obfuscation::{prepare_task_obfuscation_dict, resolve_engine_build_dir_from_game_js};

#[derive(Debug, Clone)]
pub struct AstObfuscationResult {
    pub target_input_path: PathBuf,
    pub target_output_path: PathBuf,
    pub work_dir: PathBuf,
    pub whitelist_path: PathBuf,
    pub whitelist_keyword_count: usize,
    pub renamed_binding_count: usize,
    pub rewritten_expression_count: usize,
    pub rewritten_literal_count: usize,
    pub dead_code_target_count: usize,
    pub dead_code_actual_count: usize,
    pub dead_code_block_count: usize,
    pub candidate_function_count: usize,
    pub dead_code_shortage_reason: Option<String>,
    pub copied_back: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AstScriptSummary {
    renamed_binding_count: usize,
    rewritten_expression_count: usize,
    rewritten_literal_count: usize,
    dead_code_target_count: usize,
    dead_code_actual_count: usize,
    dead_code_block_count: usize,
    candidate_function_count: usize,
    dead_code_shortage_reason: Option<String>,
}

fn resolve_runtime_paths_from_exe(current_exe_path: &Path) -> Result<PathBuf, String> {
    let backend_dir = current_exe_path.parent().ok_or_else(|| {
        format!(
            "无法解析后端可执行文件所在目录: {}",
            current_exe_path.display()
        )
    })?;
    let script_path = backend_dir.join("scripts").join("ast_obfuscate.cjs");
    Ok(script_path)
}

fn resolve_runtime_paths() -> Result<PathBuf, String> {
    let current_exe_path =
        std::env::current_exe().map_err(|error| format!("获取当前可执行文件路径失败: {error}"))?;
    resolve_runtime_paths_from_exe(&current_exe_path)
}

pub async fn run_code_package_ast_obfuscation(
    main_repo_path: &Path,
    code_repo_path: &Path,
    task_dir: &Path,
    seed: Option<u64>,
    enable_dead_code_injection: bool,
    dead_code_injection_count: u32,
) -> Result<AstObfuscationResult, String> {
    let target_input_path = find_unique_game_js(code_repo_path)?;
    let build_dir = resolve_engine_build_dir_from_game_js(&target_input_path)?;
    let obfuscation_dir = task_dir.join("obfuscation");
    let target_output_path = obfuscation_dir.join("game_ast_obfuscation.js");

    fs::create_dir_all(&obfuscation_dir)
        .map_err(|error| format!("创建 AST 混淆输出目录失败: {error}"))?;
    let (whitelist, task_dict_dir) =
        prepare_task_obfuscation_dict(main_repo_path, &build_dir, &obfuscation_dir)?;
    let whitelist_path = task_dict_dir.join("whitelist.json");

    let script_path = resolve_runtime_paths()?;

    if !script_path.exists() {
        return Err(format!("未找到 AST 混淆脚本: {}", script_path.display()));
    }

    let mut command = Command::new("node");
    command
        .arg(&script_path)
        .arg("--input")
        .arg(&target_input_path)
        .arg("--output")
        .arg(&target_output_path)
        .arg("--whitelist")
        .arg(&whitelist_path);

    if let Some(seed) = seed {
        command.arg("--seed").arg(seed.to_string());
    }
    if enable_dead_code_injection {
        command
            .arg("--dead-code")
            .arg("--dead-code-count")
            .arg(dead_code_injection_count.to_string());
    }

    let output = command
        .output()
        .await
        .map_err(|error| format!("执行 AST 混淆脚本失败: {error}"))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "AST 混淆脚本执行失败: exit_code={:?}, stdout={}, stderr={}",
            output.status.code(),
            if stdout.is_empty() { "无" } else { &stdout },
            if stderr.is_empty() { "无" } else { &stderr },
        ));
    }

    let summary: AstScriptSummary = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("解析 AST 混淆结果失败: {error}"))?;

    if !target_output_path.exists() {
        return Err(format!(
            "未生成 AST 混淆输出文件: {}",
            target_output_path.display()
        ));
    }

    let original_content =
        fs::read(&target_input_path).map_err(|error| format!("读取目标 game.js 失败: {error}"))?;
    let obfuscated_content = fs::read(&target_output_path)
        .map_err(|error| format!("读取 AST 混淆后的 game.js 失败: {error}"))?;
    let copied_back = original_content != obfuscated_content;
    if copied_back {
        fs::write(&target_input_path, obfuscated_content)
            .map_err(|error| format!("回写 AST 混淆后的 game.js 失败: {error}"))?;
    }

    Ok(AstObfuscationResult {
        target_input_path,
        target_output_path,
        work_dir: obfuscation_dir,
        whitelist_path,
        whitelist_keyword_count: whitelist.merged_keywords.len(),
        renamed_binding_count: summary.renamed_binding_count,
        rewritten_expression_count: summary.rewritten_expression_count,
        rewritten_literal_count: summary.rewritten_literal_count,
        dead_code_target_count: summary.dead_code_target_count,
        dead_code_actual_count: summary.dead_code_actual_count,
        dead_code_block_count: summary.dead_code_block_count,
        candidate_function_count: summary.candidate_function_count,
        dead_code_shortage_reason: summary.dead_code_shortage_reason,
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
    use super::*;

    #[test]
    fn resolve_runtime_paths_from_exe_should_use_backend_exe_dir() {
        let exe_path = Path::new("/opt/cocos-build/backend/cocos-build-backend");

        let script_path = resolve_runtime_paths_from_exe(exe_path).expect("resolve runtime paths");

        assert_eq!(
            script_path,
            PathBuf::from("/opt/cocos-build/backend/scripts/ast_obfuscate.cjs")
        );
    }

    #[test]
    fn resolve_runtime_paths_from_exe_should_fail_without_parent() {
        let error = resolve_runtime_paths_from_exe(Path::new("/"))
            .expect_err("path without parent should fail");

        assert!(error.contains("无法解析后端可执行文件所在目录"));
    }
}
