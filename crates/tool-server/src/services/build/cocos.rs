use std::path::{Path, PathBuf};

use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, watch},
};
use tracing::info;

use crate::{
    error::AppError,
    models::Engine,
    state::{AppState, RuntimeFlushMode},
};

use super::logging::write_log_line;

pub async fn execute_cocos_build(
    state: &AppState,
    task_id: &str,
    task_name: &str,
    engine: &Engine,
    project_path: &Path,
    config_path: &Path,
    log_file: &mut File,
) -> Result<(), AppError> {
    let executable = resolve_creator_executable(&engine.path)?;
    write_log_line(
        log_file,
        &format!(
            "开始调用 Cocos Creator: {} --project {} --build configPath={}",
            executable.display(),
            project_path.display(),
            config_path.display()
        ),
    )
    .await?;
    info!(
        task_id,
        executable = %executable.display(),
        project_path = %project_path.display(),
        config_path = %config_path.display(),
        "开始执行 Cocos Creator 构建"
    );
    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |task| {
            task.step_label = "Cocos Creator 构建中".to_owned();
        })
        .await?;

    let mut command = Command::new(&executable);
    command
        .arg("--project")
        .arg(project_path)
        .arg("--build")
        .arg(format!("configPath={}", config_path.display()))
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|error| AppError::internal(format!("启动 Cocos Creator 失败: {error}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::internal("无法读取构建标准输出"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::internal("无法读取构建标准错误"))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let stdout_handle = tokio::spawn(read_stream(stdout, tx.clone()));
    let stderr_handle = tokio::spawn(read_stream(stderr, tx.clone()));
    drop(tx);

    let mut max_progress = 40u8;
    let mut cancellation = state.cancellation_receiver().await;
    loop {
        let line = tokio::select! {
            _ = wait_for_cancellation(&mut cancellation) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = stdout_handle.await;
                let _ = stderr_handle.await;
                write_log_line(log_file, "已终止 Cocos Creator 子进程").await?;
                return Err(AppError::canceled(format!("任务 {task_name} 已取消")));
            }
            line = rx.recv() => line,
        };
        let Some(line) = line else {
            break;
        };
        write_log_line(log_file, &line).await?;
        if let Some(progress) = extract_cocos_progress(&line) {
            let mapped = 40 + ((progress / 100.0) * 55.0).round() as u8;
            if mapped > max_progress {
                max_progress = mapped.min(95);
                state
                    .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |task| {
                        task.progress = max_progress;
                        task.step_label = format!("Cocos Creator 构建中（{}%）", progress.round());
                    })
                    .await?;
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|error| AppError::internal(format!("等待 Cocos Creator 进程结束失败: {error}")))?;
    let _ = stdout_handle.await;
    let _ = stderr_handle.await;

    let exit_code = status.code().unwrap_or(-1);
    if !status.success() && exit_code != 32 && exit_code != 36 {
        return Err(AppError::internal(format!(
            "任务 {task_name} 构建失败，退出码: {exit_code}"
        )));
    }

    if exit_code == 36 {
        write_log_line(log_file, "Cocos Creator 返回退出码 36，按配置忽略并继续").await?;
    }
    if exit_code == 32 {
        write_log_line(log_file, "Cocos Creator 返回退出码 32，按配置忽略并继续").await?;
    }

    state
        .update_task_runtime(task_id, RuntimeFlushMode::Deferred, |task| {
            if task.progress < 95 {
                task.progress = 95;
            }
            task.step_label = "Cocos Creator 构建完成".to_owned();
        })
        .await?;
    info!(task_id, exit_code, "Cocos Creator 构建结束");

    Ok(())
}

async fn wait_for_cancellation(receiver: &mut Option<watch::Receiver<bool>>) {
    match receiver.as_mut() {
        Some(receiver) => {
            if *receiver.borrow() {
                return;
            }
            let _ = receiver.changed().await;
        }
        None => std::future::pending::<()>().await,
    }
}

async fn read_stream(
    stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    tx: mpsc::UnboundedSender<String>,
) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let _ = tx.send(line);
    }
}

fn resolve_creator_executable(engine_path: &str) -> Result<PathBuf, AppError> {
    let path = PathBuf::from(engine_path);
    let executable =
        if cfg!(target_os = "macos") && path.extension().is_some_and(|ext| ext == "app") {
            path.join("Contents").join("MacOS").join("CocosCreator")
        } else if cfg!(target_os = "windows") && path.is_dir() {
            path.join("CocosCreator.exe")
        } else {
            path
        };

    if !executable.exists() {
        return Err(AppError::validation(format!(
            "未找到 Cocos Creator 可执行文件: {}",
            executable.display()
        )));
    }

    Ok(executable)
}

fn extract_cocos_progress(line: &str) -> Option<f32> {
    if let Some(value) = parse_percent_after(line, "progress:") {
        return Some(value);
    }
    if let Some(value) = parse_percent_after(line, "building...") {
        return Some(value);
    }
    None
}

fn parse_percent_after(line: &str, needle: &str) -> Option<f32> {
    let rest = line.split_once(needle)?.1.trim();
    let number = rest.split('%').next()?.trim().parse::<f32>().ok()?;
    Some(number.clamp(0.0, 100.0))
}
