mod cocos;
mod logging;
mod prep_executor;
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
