pub(crate) mod cocos;
mod logging;
mod prep_executor;
mod private_repo;
mod queue;
mod task;

use crate::{models::BuildStatusResponse, state::AppState};

pub(crate) use logging::{append_task_log, now_string, write_log_line};
pub use queue::start_build;
pub use task::validate_package_tasks;
pub(crate) use task::{ObfuscationExecutionContext, run_task_obfuscation};

pub async fn get_build_status(state: &AppState) -> BuildStatusResponse {
    state.get_build_status().await
}

/// Dropping the task future also drops its managed commands and closes their jobs.
/// This covers preparation, Git and obfuscation as well as the Cocos phase.
pub(crate) async fn cancel_on_request<T>(
    state: &AppState,
    work: impl std::future::Future<Output = Result<T, crate::error::AppError>>,
) -> Result<T, crate::error::AppError> {
    let mut cancellation = state.cancellation_receiver().await;
    tokio::select! {
        biased;
        _ = async {
            if let Some(receiver) = cancellation.as_mut() {
                loop {
                    if *receiver.borrow_and_update() { return; }
                    if receiver.changed().await.is_err() { break; }
                }
            }
            std::future::pending::<()>().await;
        } => Err(crate::error::AppError::Canceled("构建队列已取消".into())),
        result = work => result,
    }
}
