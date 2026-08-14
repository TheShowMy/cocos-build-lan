use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::sleep,
};
use tracing::info;

use crate::{
    error::AppError,
    models::Engine,
    state::{AppState, RuntimeFlushMode},
};

use super::logging::write_log_line;

/// 提高 Cocos Creator（Electron/Node）构建进程的 V8 堆上限，
/// 避免大图集打包时因默认堆上限触发"内存不足"而失败。
const COCOS_BUILD_MAX_OLD_SPACE_MB: u32 = 8192;
/// 看门狗检查节奏。
const COCOS_BUILD_WATCHDOG_TICK: Duration = Duration::from_secs(30);
/// 超过该时长无新输出先写告警日志。
const COCOS_BUILD_STALL_WARN_AFTER: Duration = Duration::from_secs(10 * 60);
/// 超过该时长无新输出判定卡死并终止（保留原样拼写以匹配 Cocos 输出）。
const COCOS_BUILD_STALL_KILL_AFTER: Duration = Duration::from_secs(20 * 60);
/// 输出中出现这些关键词即认为构建进程已崩溃，立即终止。
const COCOS_BUILD_CRASH_KEYWORDS: [&str; 6] = [
    "构建进程奔溃",
    "Builder Process Creashed",
    "Creashed when build platform",
    "render-process-gone",
    "heap out of memory",
    "FATAL ERROR",
];
/// 失败原因中携带的崩溃前输出行数。
const CRASH_CONTEXT_LINES: usize = 40;
/// 检测到崩溃关键词后继续读取输出的宽限期，用于捕获 V8 堆溢出的 FATAL ERROR 堆栈。
const CRASH_GRACE: Duration = Duration::from_secs(5);
/// 默认附加的 Chromium/Electron 参数：渲染进程崩溃（隐藏窗口，如自动图集）的常见修复项。
const DEFAULT_COCOS_EXTRA_FLAGS: [&str; 2] = ["--no-sandbox", "--disable-gpu"];

pub async fn execute_cocos_build(
    state: &AppState,
    task_id: &str,
    task_name: &str,
    engine: &Engine,
    project_path: &Path,
    config_path: &Path,
    log_file: &mut File,
) -> Result<(), AppError> {
    execute_cocos_build_with_watchdog(
        state,
        task_id,
        task_name,
        engine,
        project_path,
        config_path,
        log_file,
        COCOS_BUILD_STALL_KILL_AFTER,
        COCOS_BUILD_WATCHDOG_TICK,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_cocos_build_with_watchdog(
    state: &AppState,
    task_id: &str,
    task_name: &str,
    engine: &Engine,
    project_path: &Path,
    config_path: &Path,
    log_file: &mut File,
    stall_kill_after: Duration,
    watchdog_tick: Duration,
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
    write_log_line(
        log_file,
        &format!(
            "本机内存状态：{}；Cocos Creator 架构：{}；构建进程 V8 堆上限已设为 {}MB",
            memory_status_text(),
            executable_arch_text(&executable),
            COCOS_BUILD_MAX_OLD_SPACE_MB
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

    let memory_flags = format!("--max-old-space-size={COCOS_BUILD_MAX_OLD_SPACE_MB}");
    let extra_flags = cocos_extra_flags();
    let mut command = Command::new(&executable);
    command
        .arg(&memory_flags)
        .arg(format!("--js-flags={memory_flags}"))
        .args(&extra_flags)
        .arg("--project")
        .arg(project_path)
        .arg("--build")
        .arg(format!("configPath={}", config_path.display()))
        .env("NODE_OPTIONS", &memory_flags)
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    write_log_line(
        log_file,
        &format!(
            "额外 Chromium 参数：{}（可用环境变量 COCOS_BUILD_EXTRA_FLAGS 覆盖）",
            extra_flags.join(" ")
        ),
    )
    .await?;

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
    let mut last_output = Instant::now();
    let mut stall_warned = false;
    let mut abort_reason: Option<String> = None;
    let mut recent = VecDeque::new();
    loop {
        tokio::select! {
            _ = wait_for_cancellation(&mut cancellation) => {
                terminate_build_child(&mut child, stdout_handle, stderr_handle).await;
                write_log_line(log_file, "已终止 Cocos Creator 子进程").await?;
                return Err(AppError::canceled(format!("任务 {task_name} 已取消")));
            }
            line = rx.recv() => {
                match line {
                    Some(line) => {
                        write_log_line(log_file, &line).await?;
                        push_context_line(&mut recent, line.clone());
                        last_output = Instant::now();
                        stall_warned = false;
                        if let Some(keyword) = COCOS_BUILD_CRASH_KEYWORDS
                            .iter()
                            .find(|keyword| line.contains(**keyword))
                        {
                            abort_reason = Some(format!(
                                "检测到 Cocos 构建进程崩溃信息（{keyword}），构建终止。Cocos 的「内存不足」提示为通用崩溃文案，真实原因见崩溃前输出。本机内存状态：{}",
                                memory_status_text()
                            ));
                            drain_crash_grace(log_file, &mut rx, &mut recent).await?;
                            break;
                        }
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
                    None => break,
                }
            }
            _ = sleep(watchdog_tick) => {
                let idle = last_output.elapsed();
                if idle >= stall_kill_after {
                    abort_reason = Some(format!(
                        "Cocos 构建超过 {} 秒无新输出，判定进程卡死，已强制终止。可能原因：图集构建内存不足弹出对话框等待确认、构建进程崩溃。本机内存状态：{}",
                        stall_kill_after.as_secs(),
                        memory_status_text()
                    ));
                    break;
                } else if idle >= COCOS_BUILD_STALL_WARN_AFTER && !stall_warned {
                    stall_warned = true;
                    write_log_line(
                        log_file,
                        &format!(
                            "Cocos 构建已超过 {} 秒无新输出，可能卡死或崩溃，将再等待 {} 秒后终止",
                            idle.as_secs(),
                            (stall_kill_after - idle).as_secs()
                        ),
                    )
                    .await?;
                }
            }
        }
    }

    if let Some(reason) = abort_reason {
        let message = abort_message(&reason, &recent);
        write_log_line(log_file, &message).await?;
        terminate_build_child(&mut child, stdout_handle, stderr_handle).await;
        return Err(AppError::internal(message));
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
            "任务 {task_name} 构建失败，退出码: {exit_code}。本机内存状态：{}",
            memory_status_text()
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

/// 终止构建子进程：Windows 下连进程树一起杀（防止孙进程持有输出管道导致排空卡死），
/// 非 Windows 回退为直接终止；所有等待都带超时，保证取消/失败路径能尽快返回。
async fn terminate_build_child(
    child: &mut tokio::process::Child,
    stdout_handle: JoinHandle<()>,
    stderr_handle: JoinHandle<()>,
) {
    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status()
                .await;
        } else {
            let _ = child.start_kill();
        }
    }
    #[cfg(not(windows))]
    {
        let _ = child.start_kill();
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), stdout_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), stderr_handle).await;
}

fn parse_extra_flags(value: Option<&str>) -> Vec<String> {
    match value {
        Some(value) if !value.trim().is_empty() => {
            value.split_whitespace().map(ToOwned::to_owned).collect()
        }
        _ => DEFAULT_COCOS_EXTRA_FLAGS
            .iter()
            .map(|flag| flag.to_string())
            .collect(),
    }
}

fn cocos_extra_flags() -> Vec<String> {
    parse_extra_flags(std::env::var("COCOS_BUILD_EXTRA_FLAGS").ok().as_deref())
}

fn memory_status_text() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        unsafe {
            let mut status = MEMORYSTATUSEX {
                dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
                dwMemoryLoad: 0,
                ullTotalPhys: 0,
                ullAvailPhys: 0,
                ullTotalPageFile: 0,
                ullAvailPageFile: 0,
                ullTotalVirtual: 0,
                ullAvailVirtual: 0,
                ullAvailExtendedVirtual: 0,
            };
            if GlobalMemoryStatusEx(&mut status) != 0 {
                let gib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                format!(
                    "{:.1} GB 可用 / {:.1} GB 总计",
                    gib(status.ullAvailPhys),
                    gib(status.ullTotalPhys)
                )
            } else {
                "查询失败".to_owned()
            }
        }
    }
    #[cfg(not(windows))]
    {
        "未知（非 Windows 平台）".to_owned()
    }
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

fn push_context_line(recent: &mut VecDeque<String>, line: String) {
    if recent.len() >= CRASH_CONTEXT_LINES {
        recent.pop_front();
    }
    recent.push_back(line);
}

fn abort_message(reason: &str, recent: &VecDeque<String>) -> String {
    let mut message = reason.to_owned();
    if !recent.is_empty() {
        message.push_str("\n── 崩溃前最近输出 ──");
        for line in recent {
            message.push('\n');
            message.push_str(line);
        }
    }
    message
}

/// 检测到崩溃关键词后继续读取一段宽限期，捕获 V8 堆溢出的 FATAL ERROR 堆栈等后续输出。
async fn drain_crash_grace(
    log_file: &mut File,
    rx: &mut mpsc::UnboundedReceiver<String>,
    recent: &mut VecDeque<String>,
) -> Result<(), AppError> {
    write_log_line(
        log_file,
        &format!(
            "检测到崩溃信息，继续捕获 {} 秒输出以定位真实原因…",
            CRASH_GRACE.as_secs()
        ),
    )
    .await?;
    let deadline = Instant::now() + CRASH_GRACE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(line)) => {
                write_log_line(log_file, &line).await?;
                push_context_line(recent, line);
            }
            Ok(None) | Err(_) => break,
        }
    }
    Ok(())
}

fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(bytes[0x3c..0x40].try_into().ok()?) as usize;
    if bytes.len() < e_lfanew + 24 || &bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes(
        bytes[e_lfanew + 4..e_lfanew + 6].try_into().ok()?,
    ))
}

fn executable_arch_text(path: &Path) -> String {
    #[cfg(windows)]
    {
        let Ok(bytes) = std::fs::read(path) else {
            return "未知".to_owned();
        };
        match pe_machine(&bytes) {
            Some(0x8664) => "x64".to_owned(),
            Some(0x014c) => "x86（32 位进程约 2GB 地址空间上限）".to_owned(),
            Some(0xaa64) => "ARM64".to_owned(),
            Some(other) => format!("0x{other:04x}").to_owned(),
            None => "未知（非 PE 文件）".to_owned(),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        "未知（非 Windows 平台）".to_owned()
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use crate::models::{AppSettings, PackageTask};

    use super::*;

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cocos_build_watchdog_{name}_{unique}"))
    }

    #[test]
    fn memory_status_text_should_render_gib_format() {
        let text = memory_status_text();
        #[cfg(windows)]
        assert!(text.contains("GB 可用"), "unexpected: {text}");
        #[cfg(not(windows))]
        assert!(text.contains("未知"), "unexpected: {text}");
    }

    #[test]
    fn crash_keyword_list_should_include_heap_oom_markers() {
        assert!(COCOS_BUILD_CRASH_KEYWORDS.contains(&"FATAL ERROR"));
        assert!(COCOS_BUILD_CRASH_KEYWORDS.contains(&"heap out of memory"));
    }

    #[test]
    fn context_buffer_should_keep_most_recent_lines() {
        let mut recent = VecDeque::new();
        for index in 0..(CRASH_CONTEXT_LINES + 10) {
            push_context_line(&mut recent, format!("line {index}"));
        }
        assert_eq!(recent.len(), CRASH_CONTEXT_LINES);
        assert_eq!(recent.front().map(String::as_str), Some("line 10"));
        assert_eq!(recent.back().map(String::as_str), Some("line 49"));
    }

    #[test]
    fn abort_message_should_include_recent_output_when_present() {
        let mut recent = VecDeque::new();
        push_context_line(&mut recent, "first".to_owned());
        push_context_line(&mut recent, "second".to_owned());

        let message = abort_message("构建终止", &recent);

        assert!(message.contains("构建终止"));
        assert!(message.contains("崩溃前最近输出"));
        assert!(message.contains("first"));
        assert!(message.contains("second"));

        let without_context = abort_message("构建终止", &VecDeque::new());
        assert!(!without_context.contains("崩溃前最近输出"));
    }

    #[test]
    fn extra_flags_should_default_and_allow_env_override() {
        assert_eq!(
            parse_extra_flags(None),
            vec!["--no-sandbox".to_owned(), "--disable-gpu".to_owned()]
        );
        assert_eq!(
            parse_extra_flags(Some("")),
            vec!["--no-sandbox", "--disable-gpu"]
        );
        assert_eq!(
            parse_extra_flags(Some("   ")),
            vec!["--no-sandbox", "--disable-gpu"]
        );
        assert_eq!(
            parse_extra_flags(Some("--no-sandbox --disable-gpu-compositing")),
            vec![
                "--no-sandbox".to_owned(),
                "--disable-gpu-compositing".to_owned()
            ]
        );
        assert_eq!(
            parse_extra_flags(Some("--js-flags=--max-old-space-size=16384")),
            vec!["--js-flags=--max-old-space-size=16384".to_owned()]
        );
    }

    #[test]
    fn pe_machine_should_parse_x64_and_x86_headers() {
        fn fake_pe(machine: u16) -> Vec<u8> {
            let mut bytes = vec![0u8; 0x100];
            bytes[0..2].copy_from_slice(b"MZ");
            let e_lfanew: u32 = 0x40;
            bytes[0x3c..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
            bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
            bytes[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
            bytes
        }
        assert_eq!(pe_machine(&fake_pe(0x8664)), Some(0x8664));
        assert_eq!(pe_machine(&fake_pe(0x014c)), Some(0x014c));
        assert_eq!(pe_machine(&fake_pe(0x014c)[..0x3c]), None);
        assert_eq!(pe_machine(b"not a pe file"), None);
    }

    #[cfg(windows)]
    mod windows_process {
        use super::*;

        fn write_fake_engine(dir: &Path, body: &str) -> PathBuf {
            std::fs::create_dir_all(dir).expect("create engine dir");
            let path = dir.join("fake-creator.cmd");
            std::fs::write(&path, format!("@echo off\r\n{body}")).expect("write fake engine");
            path
        }

        async fn run_fake(
            engine_script: &str,
            stall_kill_after: Duration,
            tick: Duration,
        ) -> Result<(), AppError> {
            let dir = temp_test_dir("fake");
            std::fs::create_dir_all(&dir).expect("create temp dir");
            let engine_path = write_fake_engine(&dir, engine_script);
            let project_path = dir.join("project");
            let config_path = dir.join("config.json");
            std::fs::create_dir_all(&project_path).expect("create project dir");
            std::fs::write(&config_path, "{}").expect("write config");

            let state = AppState::load(dir.join("data")).await;
            state
                .save_settings(AppSettings {
                    package_tasks: vec![PackageTask {
                        id: "task_test".to_owned(),
                        name: "测试任务".to_owned(),
                        build_args_json: "{}".to_owned(),
                        ..PackageTask::default()
                    }],
                    ..AppSettings::default()
                })
                .await
                .expect("save settings");

            let mut log_file = File::create(dir.join("task.log"))
                .await
                .expect("create log file");
            let result = execute_cocos_build_with_watchdog(
                &state,
                "task_test",
                "测试任务",
                &Engine {
                    name: "fake".to_owned(),
                    path: engine_path.to_string_lossy().to_string(),
                },
                &project_path,
                &config_path,
                &mut log_file,
                stall_kill_after,
                tick,
            )
            .await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            result
        }

        #[tokio::test]
        async fn silent_build_is_terminated_by_stall_watchdog() {
            let started = std::time::Instant::now();
            let error = run_fake(
                "ping -n 300 127.0.0.1 >nul",
                Duration::from_secs(2),
                Duration::from_millis(300),
            )
            .await
            .expect_err("stall watchdog should fail the build");
            assert!(
                error.to_string().contains("无新输出"),
                "unexpected: {error}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "watchdog took too long: {:?}",
                started.elapsed()
            );
        }

        #[tokio::test]
        async fn crash_keyword_output_aborts_build_immediately() {
            let started = std::time::Instant::now();
            let error = run_fake(
                "echo FATAL ERROR: Reached heap limit\nping -n 300 127.0.0.1 >nul",
                Duration::from_secs(60),
                Duration::from_secs(1),
            )
            .await
            .expect_err("crash keyword should abort the build");
            let message = error.to_string();
            assert!(message.contains("FATAL ERROR"), "unexpected: {message}");
            assert!(
                message.contains("崩溃前最近输出"),
                "crash context should be included: {message}"
            );
            assert!(
                message.contains("FATAL ERROR: Reached heap limit"),
                "crash line should appear in context: {message}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "crash abort took too long: {:?}",
                started.elapsed()
            );
        }

        #[tokio::test]
        async fn stall_abort_includes_recent_output_context() {
            let started = std::time::Instant::now();
            let error = run_fake(
                "echo atlas build start\nping -n 300 127.0.0.1 >nul",
                Duration::from_secs(2),
                Duration::from_millis(300),
            )
            .await
            .expect_err("stall watchdog should fail the build");
            let message = error.to_string();
            assert!(message.contains("无新输出"), "unexpected: {message}");
            assert!(
                message.contains("atlas build start"),
                "recent output should appear in stall message: {message}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "watchdog took too long: {:?}",
                started.elapsed()
            );
        }

        #[tokio::test]
        async fn cancel_terminates_process_tree_and_returns_canceled() {
            let dir = temp_test_dir("cancel");
            std::fs::create_dir_all(&dir).expect("create temp dir");
            let engine_path = write_fake_engine(
                &dir,
                "start /b cmd /c ping -n 300 127.0.0.1\nping -n 300 127.0.0.1 >nul",
            );
            let project_path = dir.join("project");
            let config_path = dir.join("config.json");
            std::fs::create_dir_all(&project_path).expect("create project dir");
            std::fs::write(&config_path, "{}").expect("write config");

            let state = AppState::load(dir.join("data")).await;
            state
                .save_settings(AppSettings {
                    package_tasks: vec![PackageTask {
                        id: "task_cancel".to_owned(),
                        name: "取消任务".to_owned(),
                        build_args_json: "{}".to_owned(),
                        ..PackageTask::default()
                    }],
                    ..AppSettings::default()
                })
                .await
                .expect("save settings");
            state.try_start_build().await.expect("start build");
            state.set_active_task(Some("task_cancel".to_owned())).await;

            let state_for_build = state.clone();
            let dir_for_build = dir.clone();
            let build = tokio::spawn(async move {
                let mut log_file = File::create(dir_for_build.join("task.log"))
                    .await
                    .expect("create log file");
                execute_cocos_build_with_watchdog(
                    &state_for_build,
                    "task_cancel",
                    "取消任务",
                    &Engine {
                        name: "fake".to_owned(),
                        path: engine_path.to_string_lossy().to_string(),
                    },
                    &project_path,
                    &config_path,
                    &mut log_file,
                    Duration::from_secs(60),
                    Duration::from_secs(1),
                )
                .await
            });

            tokio::time::sleep(Duration::from_millis(1200)).await;
            state
                .cancel_active_build("task_cancel")
                .await
                .expect("cancel build");

            let started = std::time::Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(20), build)
                .await
                .expect("cancel should finish in time")
                .expect("build task should not panic");
            assert!(
                matches!(result, Err(AppError::Canceled(_))),
                "unexpected: {result:?}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "cancel took too long: {:?}",
                started.elapsed()
            );
            state.finish_build().await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
    }
}
