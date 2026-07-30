//! 稳定启动器：在控制端退出后应用完整版本包，并重新打开控制端。

use std::{env, fs, path::PathBuf, process::Stdio, time::Duration};

use cocos_build_lan_core::{PendingUpdate, ToolLaunchSpec, ToolSupervisor};
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        launch_active().await;
        return;
    };
    if command != "--apply-staged" {
        eprintln!("用法：tool-launcher [--apply-staged <staging-dir> --wait-pid <pid>]");
        std::process::exit(2);
    }
    let staging = args.next().map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("缺少 staging 目录");
        std::process::exit(2);
    });
    let mut wait_pid = None;
    while let Some(flag) = args.next() {
        if flag == "--wait-pid" {
            wait_pid = args
                .next()
                .and_then(|value| value.to_string_lossy().parse().ok());
        }
    }
    if let Some(pid) = wait_pid
        && let Err(error) = wait_for_exit(pid).await
    {
        eprintln!("更新交接已取消：{error}");
        std::process::exit(1);
    }
    if let Err(error) = apply_staged(staging).await {
        eprintln!("应用更新失败：{error}");
        let _ = launch_active().await;
    }
}

fn names() -> (String, String) {
    let executable = env::current_exe().expect("读取启动器路径");
    let stem = executable
        .file_stem()
        .expect("启动器文件名")
        .to_string_lossy();
    let project = stem.trim_end_matches("-launcher");
    (format!("{project}-server"), format!("{project}-control"))
}

fn spec() -> ToolLaunchSpec {
    let (server, control) = names();
    ToolLaunchSpec::discover(server, control).expect("读取 tool.json")
}

async fn apply_staged(staging: PathBuf) -> Result<(), String> {
    let spec = spec();
    let manifest = serde_json::from_slice(
        &fs::read(staging.join("manifest.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let pending = PendingUpdate::from_staging(manifest, staging);
    let supervisor = ToolSupervisor::new(spec.clone());
    let before = spec
        .update_store()
        .map_err(|error| error.to_string())?
        .active_release()
        .map_err(|error| error.to_string())?;
    match supervisor.apply_pending(&pending).await {
        Ok(_) => launch_control(&spec),
        Err(error) => {
            spec.update_store()
                .map_err(|error| error.to_string())?
                .restore_active(&before)
                .map_err(|error| error.to_string())?;
            let _ = supervisor.start().await;
            Err(error.to_string())
        }
    }
}

async fn launch_active() {
    let spec = spec();
    let supervisor = ToolSupervisor::new(spec.clone());
    let _ = supervisor.start().await;
    let _ = launch_control(&spec);
}

fn launch_control(spec: &ToolLaunchSpec) -> Result<(), String> {
    let control = spec
        .active_control_path()
        .map_err(|error| error.to_string())?;
    std::process::Command::new(control)
        .current_dir(&spec.project_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn wait_for_exit(pid: u32) -> Result<(), String> {
    wait_for_exit_with(Duration::from_secs(15), || process_exists(pid)).await
}

async fn wait_for_exit_with<F>(timeout: Duration, mut exists: F) -> Result<(), String>
where
    F: FnMut() -> Result<bool, String>,
{
    let started = tokio::time::Instant::now();
    loop {
        if !exists()? {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err("旧控制端未在 15 秒内退出，未切换 active.json".to_owned());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> Result<bool, String> {
    Ok(std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map_err(|error| format!("无法检查旧控制端进程：{error}"))?
        .success())
}

#[cfg(windows)]
fn process_exists(pid: u32) -> Result<bool, String> {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map_err(|error| format!("无法检查旧控制端进程：{error}"))?;
    if !output.status.success() {
        return Err("tasklist 无法检查旧控制端进程".to_owned());
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    Ok(!listing.contains("No tasks are running") && listing.contains(&pid.to_string()))
}

#[cfg(not(any(unix, windows)))]
fn process_exists(_pid: u32) -> Result<bool, String> {
    Err("当前平台不支持安全确认旧控制端是否退出".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waits_for_exit_and_times_out_without_switching() {
        assert!(
            wait_for_exit_with(Duration::from_millis(1), || Ok(true))
                .await
                .is_err()
        );
        assert!(
            wait_for_exit_with(Duration::from_millis(1), || Ok(false))
                .await
                .is_ok()
        );
    }
}
