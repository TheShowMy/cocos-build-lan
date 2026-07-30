use std::{collections::BTreeSet, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::{fs, process::Command};

use crate::{
    error::AppError,
    models::{GitConfig, RepoSyncResult},
};

#[derive(Debug, Clone)]
pub struct BranchPrepareResult {
    pub previous_branch: String,
    pub branch: String,
    pub source_branch: String,
    pub remote_exists: bool,
    pub created: bool,
    pub pushed: bool,
}

#[derive(Debug, Clone)]
pub struct CommitPushResult {
    pub branch: String,
    pub had_changes: bool,
    pub discarded_changes: bool,
    pub commit_sha: Option<String>,
    pub had_unpushed_commits: bool,
    pub pushed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct UnstagedCleanupResult {
    pub had_staged_changes: bool,
    pub had_unstaged_changes: bool,
    pub had_untracked_files: bool,
}

pub async fn list_remote_branches(
    git_url: &str,
    git_config: &GitConfig,
) -> Result<Vec<String>, AppError> {
    let output =
        run_git_with_auth(git_url, git_config, None, ["ls-remote", "--heads", git_url]).await?;
    let mut branches = BTreeSet::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((_, reference)) = line.split_once('\t')
            && let Some(branch) = reference.strip_prefix("refs/heads/")
        {
            branches.insert(branch.to_string());
        }
    }

    Ok(branches.into_iter().collect())
}

pub async fn ensure_repo_synced(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
    requested_branch: Option<&str>,
) -> Result<RepoSyncResult, AppError> {
    if repo_dir.exists() && !repo_dir.join(".git").exists() {
        return Err(AppError::validation(format!(
            "目录 {} 已存在，但不是 Git 仓库",
            repo_dir.display()
        )));
    }

    if !repo_dir.exists() {
        if let Some(parent) = repo_dir.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| AppError::internal(format!("创建仓库目录失败: {error}")))?;
        }
        clone_repo(repo_dir, git_url, git_config).await?;
    } else {
        ensure_clean_worktree(repo_dir).await?;
        run_git_in_repo(repo_dir, ["remote", "set-url", "origin", git_url]).await?;
    }

    fetch_repo(repo_dir, git_url, git_config).await?;

    let branch = match requested_branch {
        Some(branch) => normalize_branch_name(branch),
        None => resolve_default_branch(repo_dir).await?,
    };

    checkout_branch(repo_dir, &branch).await?;
    let commit = current_commit(repo_dir).await?;

    Ok(RepoSyncResult {
        path: repo_dir.to_path_buf(),
        branch,
        commit,
    })
}

pub async fn current_branch(repo_dir: &Path) -> Result<String, AppError> {
    let output = run_git_in_repo(repo_dir, ["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub async fn has_uncommitted_changes(repo_dir: &Path) -> Result<bool, AppError> {
    Ok(!uncommitted_change_entries(repo_dir).await?.is_empty())
}

pub async fn uncommitted_change_entries(repo_dir: &Path) -> Result<Vec<String>, AppError> {
    let output = run_git_in_repo(repo_dir, ["status", "--porcelain=v1", "-uall"]).await?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

pub async fn cleanup_all_changes(repo_dir: &Path) -> Result<UnstagedCleanupResult, AppError> {
    let result = collect_worktree_status(repo_dir).await?;

    if result.had_staged_changes || result.had_unstaged_changes {
        run_git_in_repo(repo_dir, ["reset", "--hard", "HEAD"]).await?;
    }

    if result.had_untracked_files {
        run_git_in_repo(repo_dir, ["clean", "-fd"]).await?;
    }

    Ok(result)
}

async fn collect_worktree_status(repo_dir: &Path) -> Result<UnstagedCleanupResult, AppError> {
    let output = run_git_in_repo(repo_dir, ["status", "--porcelain"]).await?;
    let status_text = String::from_utf8_lossy(&output.stdout);
    let mut result = UnstagedCleanupResult::default();

    for line in status_text.lines() {
        let bytes = line.as_bytes();
        if bytes.len() < 2 {
            continue;
        }
        let staged = bytes[0] as char;
        let unstaged = bytes[1] as char;
        if staged != ' ' && staged != '?' {
            result.had_staged_changes = true;
        }
        if unstaged != ' ' {
            result.had_unstaged_changes = true;
        }
        if staged == '?' && unstaged == '?' {
            result.had_untracked_files = true;
        }
    }

    Ok(result)
}

pub async fn prepare_branch_from_current(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
    target_branch: &str,
) -> Result<BranchPrepareResult, AppError> {
    let current = current_branch(repo_dir).await?;
    prepare_branch(
        repo_dir,
        git_url,
        git_config,
        target_branch,
        &current,
        false,
    )
    .await
}

pub async fn prepare_branch_from_base(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
    target_branch: &str,
    base_branch: &str,
) -> Result<BranchPrepareResult, AppError> {
    prepare_branch(
        repo_dir,
        git_url,
        git_config,
        target_branch,
        base_branch,
        true,
    )
    .await
}

pub async fn finalize_repo_changes(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
    commit_message: &str,
    discard_changes: bool,
) -> Result<CommitPushResult, AppError> {
    let branch = current_branch(repo_dir).await?;
    let had_changes = has_uncommitted_changes(repo_dir).await?;
    let mut commit_sha = None;

    if discard_changes {
        if had_changes {
            discard_all_changes(repo_dir).await?;
        }
    } else if had_changes {
        commit_sha = Some(commit_all_changes(repo_dir, git_config, commit_message).await?);
    }

    let had_unpushed_commits = has_unpushed_commits(repo_dir).await?;
    if had_unpushed_commits {
        push_current_branch(repo_dir, git_url, git_config).await?;
    }

    Ok(CommitPushResult {
        branch,
        had_changes,
        discarded_changes: discard_changes && had_changes,
        commit_sha,
        had_unpushed_commits,
        pushed: had_unpushed_commits,
    })
}

async fn clone_repo(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
) -> Result<(), AppError> {
    let repo_dir_string = repo_dir.to_string_lossy().to_string();
    run_git_with_auth(
        git_url,
        git_config,
        None,
        ["clone", git_url, &repo_dir_string],
    )
    .await?;
    run_git_in_repo(repo_dir, ["remote", "set-url", "origin", git_url])
        .await
        .map(|_| ())
}

async fn fetch_repo(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
) -> Result<(), AppError> {
    run_git_with_auth(
        git_url,
        git_config,
        Some(repo_dir),
        ["fetch", "--all", "--prune"],
    )
    .await
    .map(|_| ())
}

async fn ensure_clean_worktree(repo_dir: &Path) -> Result<(), AppError> {
    let entries = uncommitted_change_entries(repo_dir).await?;
    if !entries.is_empty() {
        return Err(AppError::validation(format!(
            "托管工作区 {} 存在未提交改动，可能是构建或准备步骤生成了未提交文件。请提交这些文件或加入 .gitignore 后再打包。\n未提交改动列表:\n{}",
            repo_dir.display(),
            format_status_entries(&entries, 50)
        )));
    }
    Ok(())
}

fn format_status_entries(entries: &[String], limit: usize) -> String {
    let mut lines = entries
        .iter()
        .take(limit)
        .map(|entry| format!("  - {entry}"))
        .collect::<Vec<_>>();
    if entries.len() > limit {
        lines.push(format!("  - ... 还有 {} 项未显示", entries.len() - limit));
    }
    lines.join("\n")
}

async fn resolve_default_branch(repo_dir: &Path) -> Result<String, AppError> {
    let symbolic = run_git_in_repo(repo_dir, ["symbolic-ref", "refs/remotes/origin/HEAD"]).await;
    if let Ok(output) = symbolic {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(normalized) = branch.strip_prefix("refs/remotes/origin/") {
            return Ok(normalized.to_string());
        }
    }

    let rev_parse = run_git_in_repo(repo_dir, ["rev-parse", "--abbrev-ref", "origin/HEAD"]).await;
    if let Ok(output) = rev_parse {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(normalized) = branch.strip_prefix("origin/") {
            return Ok(normalized.to_string());
        }
    }

    let refs = run_git_in_repo(
        repo_dir,
        [
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin",
        ],
    )
    .await?;
    for line in String::from_utf8_lossy(&refs.stdout).lines() {
        let branch = line.trim();
        if branch.is_empty() || branch == "origin/HEAD" {
            continue;
        }
        if let Some(normalized) = branch.strip_prefix("origin/") {
            return Ok(normalized.to_string());
        }
    }

    Err(AppError::internal("无法解析远端默认分支"))
}

async fn checkout_branch(repo_dir: &Path, branch: &str) -> Result<(), AppError> {
    let normalized = normalize_branch_name(branch);
    let remote_branch = format!("origin/{normalized}");
    run_git_in_repo(repo_dir, ["checkout", "-B", &normalized, &remote_branch]).await?;
    run_git_in_repo(repo_dir, ["reset", "--hard", &remote_branch]).await?;
    Ok(())
}

async fn checkout_local_branch(repo_dir: &Path, branch: &str) -> Result<(), AppError> {
    let normalized = normalize_branch_name(branch);
    run_git_in_repo(repo_dir, ["checkout", &normalized]).await?;
    Ok(())
}

async fn create_branch_from_current(repo_dir: &Path, branch: &str) -> Result<(), AppError> {
    let normalized = normalize_branch_name(branch);
    run_git_in_repo(repo_dir, ["checkout", "-b", &normalized]).await?;
    Ok(())
}

pub async fn current_commit(repo_dir: &Path) -> Result<String, AppError> {
    let output = run_git_in_repo(repo_dir, ["rev-parse", "HEAD"]).await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn has_local_branch(repo_dir: &Path, branch: &str) -> Result<bool, AppError> {
    let normalized = normalize_branch_name(branch);
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{normalized}"),
        ])
        .output()
        .await
        .map_err(|error| AppError::internal(format!("执行 git 命令失败: {error}")))?;

    Ok(output.status.success())
}

async fn has_remote_branch(repo_dir: &Path, branch: &str) -> Result<bool, AppError> {
    let normalized = normalize_branch_name(branch);
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{normalized}"),
        ])
        .output()
        .await
        .map_err(|error| AppError::internal(format!("执行 git 命令失败: {error}")))?;

    Ok(output.status.success())
}

async fn prepare_branch(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
    target_branch: &str,
    base_branch: &str,
    force_base_from_remote: bool,
) -> Result<BranchPrepareResult, AppError> {
    let target_branch = normalize_branch_name(target_branch);
    let previous_branch = current_branch(repo_dir).await?;
    let remote_exists = has_remote_branch(repo_dir, &target_branch).await?;

    if remote_exists {
        checkout_branch(repo_dir, &target_branch).await?;
        return Ok(BranchPrepareResult {
            previous_branch,
            branch: target_branch.clone(),
            source_branch: target_branch,
            remote_exists: true,
            created: false,
            pushed: false,
        });
    }

    if has_local_branch(repo_dir, &target_branch).await? {
        checkout_local_branch(repo_dir, &target_branch).await?;
        push_current_branch(repo_dir, git_url, git_config).await?;
        return Ok(BranchPrepareResult {
            previous_branch,
            branch: target_branch.clone(),
            source_branch: target_branch,
            remote_exists: false,
            created: false,
            pushed: true,
        });
    }

    let source_branch = normalize_branch_name(base_branch);
    if force_base_from_remote {
        if !has_remote_branch(repo_dir, &source_branch).await? {
            return Err(AppError::not_found(format!(
                "远端不存在基线分支 {}",
                source_branch
            )));
        }
        checkout_branch(repo_dir, &source_branch).await?;
    }

    create_branch_from_current(repo_dir, &target_branch).await?;
    push_current_branch(repo_dir, git_url, git_config).await?;

    Ok(BranchPrepareResult {
        previous_branch,
        branch: target_branch,
        source_branch,
        remote_exists: false,
        created: true,
        pushed: true,
    })
}

async fn discard_all_changes(repo_dir: &Path) -> Result<(), AppError> {
    run_git_in_repo(repo_dir, ["reset", "--hard", "HEAD"]).await?;
    run_git_in_repo(repo_dir, ["clean", "-fd"]).await?;
    Ok(())
}

async fn commit_all_changes(
    repo_dir: &Path,
    git_config: &GitConfig,
    commit_message: &str,
) -> Result<String, AppError> {
    ensure_commit_identity(repo_dir, git_config).await?;
    run_git_in_repo(repo_dir, ["add", "-A"]).await?;
    run_git_in_repo(repo_dir, ["commit", "-m", commit_message]).await?;
    current_commit(repo_dir).await
}

async fn ensure_commit_identity(repo_dir: &Path, git_config: &GitConfig) -> Result<(), AppError> {
    let has_name = run_git_in_repo(repo_dir, ["config", "--get", "user.name"])
        .await
        .is_ok();
    let has_email = run_git_in_repo(repo_dir, ["config", "--get", "user.email"])
        .await
        .is_ok();
    if has_name && has_email {
        return Ok(());
    }

    let username = git_config.username.trim();
    let default_name = if let Some((name, _)) = username.split_once('@') {
        name
    } else if !username.is_empty() {
        username
    } else {
        "cocos-build"
    };
    let default_email = if username.contains('@') {
        username.to_string()
    } else if !username.is_empty() {
        format!("{username}@local")
    } else {
        "cocos-build@local".to_string()
    };

    run_git_in_repo(repo_dir, ["config", "user.name", default_name]).await?;
    run_git_in_repo(repo_dir, ["config", "user.email", &default_email]).await?;
    Ok(())
}

async fn has_upstream(repo_dir: &Path) -> bool {
    run_git_in_repo(
        repo_dir,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .await
    .is_ok()
}

async fn has_unpushed_commits(repo_dir: &Path) -> Result<bool, AppError> {
    if !has_upstream(repo_dir).await {
        return Ok(true);
    }

    let output = run_git_in_repo(
        repo_dir,
        ["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )
    .await?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let _behind = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let ahead = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    Ok(ahead > 0)
}

async fn push_current_branch(
    repo_dir: &Path,
    git_url: &str,
    git_config: &GitConfig,
) -> Result<(), AppError> {
    let branch = current_branch(repo_dir).await?;
    let args = if has_upstream(repo_dir).await {
        vec!["push".to_string(), "origin".to_string(), branch.clone()]
    } else {
        vec![
            "push".to_string(),
            "-u".to_string(),
            "origin".to_string(),
            branch.clone(),
        ]
    };

    run_git_with_auth(git_url, git_config, Some(repo_dir), args).await?;
    Ok(())
}

async fn run_git_in_repo(
    repo_dir: &Path,
    args: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<std::process::Output, AppError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo_dir);
    for arg in args {
        command.arg(arg.as_ref());
    }

    let output = command
        .output()
        .await
        .map_err(|error| AppError::internal(format!("执行 git 命令失败: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(AppError::internal(if !stderr.is_empty() {
            stderr
        } else {
            stdout
        }));
    }

    Ok(output)
}

async fn run_git_with_auth(
    git_url: &str,
    git_config: &GitConfig,
    repo_dir: Option<&Path>,
    args: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<std::process::Output, AppError> {
    let mut command = Command::new("git");

    if let Some(header) = build_auth_header(git_url, git_config) {
        command.arg("-c").arg(format!("http.extraHeader={header}"));
    }

    if let Some(repo_dir) = repo_dir {
        command.arg("-C").arg(repo_dir);
    }

    for arg in args {
        command.arg(arg.as_ref());
    }

    let output = command
        .output()
        .await
        .map_err(|error| AppError::internal(format!("执行 git 命令失败: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(AppError::internal(if !stderr.is_empty() {
            stderr
        } else {
            stdout
        }));
    }

    Ok(output)
}

fn build_auth_header(git_url: &str, git_config: &GitConfig) -> Option<String> {
    if git_config.username.is_empty() {
        return None;
    }

    if !(git_url.starts_with("http://") || git_url.starts_with("https://")) {
        return None;
    }

    let raw = format!("{}:{}", git_config.username, git_config.password);
    let encoded = STANDARD.encode(raw);
    Some(format!("AUTHORIZATION: Basic {encoded}"))
}

fn normalize_branch_name(branch: &str) -> String {
    branch.trim().trim_start_matches("origin/").to_string()
}

impl From<AppError> for String {
    fn from(value: AppError) -> Self {
        value.message().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_branch_name_should_trim_origin_prefix() {
        assert_eq!(normalize_branch_name(" origin/main "), "main");
        assert_eq!(normalize_branch_name("feature/demo"), "feature/demo");
    }

    #[test]
    fn build_auth_header_should_only_support_http_urls() {
        let git_config = GitConfig {
            username: "user@example.com".to_string(),
            password: "token".to_string(),
        };

        assert_eq!(
            build_auth_header("https://example.com/repo.git", &git_config),
            Some(format!(
                "AUTHORIZATION: Basic {}",
                STANDARD.encode("user@example.com:token")
            ))
        );
        assert_eq!(
            build_auth_header("git@example.com:repo.git", &git_config),
            None
        );
    }
}
