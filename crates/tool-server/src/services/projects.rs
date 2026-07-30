use crate::{
    error::AppError,
    git,
    models::{ProjectWorkspaceStatus, ProjectWorktreeCleanupResponse, RepoSyncResult},
    state::AppState,
};

pub async fn list_project_workspace_statuses(
    state: &AppState,
) -> Result<Vec<ProjectWorkspaceStatus>, AppError> {
    let settings = state.get_settings().await;
    let mut statuses = Vec::with_capacity(settings.projects.len());

    for project in settings.projects {
        statuses.push(build_workspace_status(state, &project).await);
    }

    Ok(statuses)
}

pub async fn initialize_project_workspace(
    state: &AppState,
    project_id: &str,
) -> Result<ProjectWorkspaceStatus, AppError> {
    let project = state.find_project(project_id).await?;
    let settings = state.get_settings().await;

    let repo_dir = state.workspace_main_repo_dir(&project);
    let sync_result =
        git::ensure_repo_synced(&repo_dir, &project.git_url, &settings.git_config, None).await?;

    Ok(workspace_status_from_sync_result(&project, sync_result))
}

pub async fn cleanup_project_worktree(
    state: &AppState,
    project_id: &str,
) -> Result<ProjectWorktreeCleanupResponse, AppError> {
    let project = state.find_project(project_id).await?;
    let repo_dir = state.workspace_main_repo_dir(&project);

    if !state.is_project_initialized(&project) || !repo_dir.exists() {
        return Err(AppError::validation(format!(
            "项目 {} 尚未初始化，请先到引擎与项目页初始化",
            project.name
        )));
    }

    let result = git::cleanup_all_changes(&repo_dir).await?;

    Ok(ProjectWorktreeCleanupResponse {
        project_id: project.id,
        project_name: project.name,
        project_path: repo_dir.to_string_lossy().to_string(),
        had_staged_changes: result.had_staged_changes,
        had_unstaged_changes: result.had_unstaged_changes,
        had_untracked_files: result.had_untracked_files,
    })
}

async fn build_workspace_status(
    state: &AppState,
    project: &crate::models::Project,
) -> ProjectWorkspaceStatus {
    let repo_dir = state.workspace_main_repo_dir(project);
    let initialized = state.is_project_initialized(project);

    let (branch, commit) = if initialized {
        (
            git::current_branch(&repo_dir).await.ok(),
            git::current_commit(&repo_dir).await.ok(),
        )
    } else {
        (None, None)
    };

    ProjectWorkspaceStatus {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        workspace_dir_key: project.workspace_dir_key.clone(),
        initialized,
        project_path: repo_dir.to_string_lossy().to_string(),
        branch,
        commit,
    }
}

fn workspace_status_from_sync_result(
    project: &crate::models::Project,
    sync_result: RepoSyncResult,
) -> ProjectWorkspaceStatus {
    ProjectWorkspaceStatus {
        project_id: project.id.clone(),
        project_name: project.name.clone(),
        workspace_dir_key: project.workspace_dir_key.clone(),
        initialized: true,
        project_path: sync_result.path.to_string_lossy().to_string(),
        branch: Some(sync_result.branch),
        commit: Some(sync_result.commit),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use tokio::{fs, process::Command};

    use super::*;
    use crate::models::{AppSettings, Engine, Project};

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_projects_test_{name}_{unique}"))
    }

    async fn create_test_state(name: &str) -> (AppState, PathBuf) {
        let data_dir = temp_test_dir(name);
        fs::create_dir_all(&data_dir)
            .await
            .expect("create temp data dir");
        let state = AppState::load(data_dir.clone()).await;
        (state, data_dir)
    }

    async fn create_project_state(name: &str) -> (AppState, Project, PathBuf) {
        let (state, data_dir) = create_test_state(name).await;
        let project = Project {
            id: "project_1".to_string(),
            name: "演示项目".to_string(),
            workspace_dir_key: "workspace_1".to_string(),
            git_url: "https://example.com/demo.git".to_string(),
            engine_name: "cocos".to_string(),
            ..Project::default()
        };
        state
            .save_settings(AppSettings {
                engines: vec![Engine {
                    name: "cocos".to_string(),
                    path: "/engine".to_string(),
                }],
                projects: vec![project.clone()],
                ..AppSettings::default()
            })
            .await
            .expect("save settings");

        (state, project, data_dir)
    }

    async fn git(repo_dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .args(args)
            .status()
            .await
            .expect("run git command");
        assert!(status.success(), "git command failed: {:?}", args);
    }

    async fn git_with_input(repo_dir: &std::path::Path, script: &str) -> std::process::Output {
        Command::new("/bin/zsh")
            .arg("-lc")
            .arg(script)
            .current_dir(repo_dir)
            .output()
            .await
            .expect("run shell command")
    }

    async fn init_repo(repo_dir: &std::path::Path) {
        fs::create_dir_all(repo_dir).await.expect("create repo dir");
        git(repo_dir, &["init"]).await;
        git(repo_dir, &["config", "user.name", "tester"]).await;
        git(repo_dir, &["config", "user.email", "tester@example.com"]).await;
        fs::write(repo_dir.join("tracked.txt"), "base\n")
            .await
            .expect("write tracked file");
        git(repo_dir, &["add", "tracked.txt"]).await;
        git(repo_dir, &["commit", "-m", "init"]).await;
    }

    #[tokio::test]
    async fn cleanup_project_worktree_should_clear_all_change_types() {
        let (state, project, data_dir) = create_project_state("cleanup_success").await;
        let repo_dir = state.workspace_main_repo_dir(&project);
        init_repo(&repo_dir).await;

        fs::write(repo_dir.join("tracked.txt"), "changed\n")
            .await
            .expect("modify tracked file");
        git(&repo_dir, &["add", "tracked.txt"]).await;
        fs::write(repo_dir.join("tracked.txt"), "changed again\n")
            .await
            .expect("modify tracked file again");
        fs::write(repo_dir.join("new.txt"), "untracked\n")
            .await
            .expect("write untracked file");

        let response = cleanup_project_worktree(&state, &project.id)
            .await
            .expect("cleanup project worktree");

        assert_eq!(response.project_id, project.id);
        assert_eq!(response.project_name, project.name);
        assert_eq!(response.project_path, repo_dir.to_string_lossy());
        assert!(response.had_staged_changes);
        assert!(response.had_unstaged_changes);
        assert!(response.had_untracked_files);

        let status_output = git_with_input(&repo_dir, "git status --porcelain").await;
        assert!(status_output.status.success());
        assert!(
            String::from_utf8_lossy(&status_output.stdout)
                .trim()
                .is_empty()
        );
        assert!(!repo_dir.join("new.txt").exists());
        assert_eq!(
            fs::read_to_string(repo_dir.join("tracked.txt"))
                .await
                .expect("read tracked file"),
            "base\n"
        );

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn cleanup_project_worktree_should_reject_uninitialized_project() {
        let (state, project, data_dir) = create_project_state("cleanup_uninitialized").await;

        let error = cleanup_project_worktree(&state, &project.id)
            .await
            .expect_err("should reject uninitialized project");

        assert_eq!(
            error.message(),
            "项目 演示项目 尚未初始化，请先到引擎与项目页初始化"
        );

        let _ = fs::remove_dir_all(data_dir).await;
    }

    #[tokio::test]
    async fn cleanup_project_worktree_should_reject_missing_project() {
        let (state, data_dir) = create_test_state("cleanup_missing").await;

        let error = cleanup_project_worktree(&state, "missing")
            .await
            .expect_err("should reject missing project");

        assert_eq!(error.message(), "未找到项目 missing");

        let _ = fs::remove_dir_all(data_dir).await;
    }
}
