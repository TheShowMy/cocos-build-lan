use chrono::Local;
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
};

use crate::{error::AppError, state::AppState};

pub async fn write_log_line(log_file: &mut File, line: &str) -> Result<(), AppError> {
    log_file.write_all(format!("{line}\n").as_bytes()).await?;
    log_file.flush().await?;
    Ok(())
}

pub async fn append_task_log(
    state: &AppState,
    task_id: &str,
    message: &str,
) -> Result<(), AppError> {
    let task_runtime = state.get_task_runtime(task_id).await?;
    let Some(log_path) = task_runtime.last_log_path else {
        return Ok(());
    };

    let mut file = OpenOptions::new().append(true).open(&log_path).await?;
    write_log_line(&mut file, message).await
}

pub async fn append_task_logs(
    state: &AppState,
    task_id: &str,
    messages: &[String],
) -> Result<(), AppError> {
    for message in messages {
        append_task_log(state, task_id, message).await?;
    }
    Ok(())
}

pub fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
