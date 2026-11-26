//! Only task subprocesses enter jobs; opening a browser never does.
use std::{
    ffi::OsStr,
    io,
    ops::{Deref, DerefMut},
    process::{Output, Stdio},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

static JOBS: OnceLock<Mutex<Vec<Weak<Job>>>> = OnceLock::new();
static STOPPING: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
tokio::task_local! { static PRIVATE_REPOS: Job; }

/// A named job survives the last handle until its last process exits. A new
/// operation therefore cannot take over a canceled operation's private repos.
pub async fn with_private_repos<T>(
    dir: &std::path::Path,
    work: impl Future<Output = Result<T, crate::error::AppError>>,
) -> Result<T, crate::error::AppError> {
    std::fs::create_dir_all(dir)?;
    let busy = || {
        crate::error::AppError::conflict(format!(
            "任务私有仓库正被占用：{}；请结束占用后重新启动此任务",
            dir.display()
        ))
    };
    #[cfg(windows)]
    {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut key = DefaultHasher::new();
        dir.canonicalize()?
            .to_string_lossy()
            .to_lowercase()
            .hash(&mut key);
        let name: Vec<u16> = format!("Global\\cocos-build-lan-repos-{:016x}\0", key.finish())
            .encode_utf16()
            .collect();
        for attempt in 0..=5 {
            match Job::create(name.as_ptr()) {
                Ok(job) => return PRIVATE_REPOS.scope(job, work).await,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 5 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(busy()),
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!()
    }
    #[cfg(not(windows))]
    {
        let lease = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(".private-repos.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lease).map_err(|_| busy())?;
        work.await
    }
}

pub fn stop_all() {
    STOPPING.store(true, Ordering::SeqCst);
    if let Some(jobs) = JOBS.get() {
        for job in jobs.lock().unwrap().iter().filter_map(Weak::upgrade) {
            job.terminate();
        }
    }
}

pub struct Command(tokio::process::Command);
impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self(tokio::process::Command::new(program))
    }
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.0.arg(arg);
        self
    }
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.0.args(args);
        self
    }
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.0.env(key, value);
        self
    }
    pub fn current_dir(&mut self, path: impl AsRef<OsStr>) -> &mut Self {
        self.0.current_dir(path.as_ref());
        self
    }
    pub fn stdout(&mut self, value: Stdio) -> &mut Self {
        self.0.stdout(value);
        self
    }
    pub fn stderr(&mut self, value: Stdio) -> &mut Self {
        self.0.stderr(value);
        self
    }
    pub fn spawn(&mut self) -> io::Result<Child> {
        let mut jobs = JOBS.get_or_init(Default::default).lock().unwrap();
        if STOPPING.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "服务正在退出"));
        }
        self.0.kill_on_drop(true).stdin(Stdio::null());
        #[cfg(windows)]
        self.0.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_SUSPENDED
                | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
        );
        let mut child = self.0.spawn()?;
        let job = match Job::attach(&child) {
            Ok(job) => Arc::new(job),
            Err(error) => {
                let _ = child.start_kill();
                return Err(error);
            }
        };
        jobs.retain(|job| job.strong_count() > 0);
        jobs.push(Arc::downgrade(&job));
        Ok(Child { child, job })
    }
    pub async fn output(&mut self) -> io::Result<Output> {
        self.stdout(Stdio::piped()).stderr(Stdio::piped());
        let Child { child, job } = self.spawn()?;
        let result = child.wait_with_output().await;
        drop(job);
        result
    }
}

pub struct Child {
    child: tokio::process::Child,
    job: Arc<Job>,
}
impl Deref for Child {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}
impl DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}
impl Child {
    pub fn terminate(&self) {
        self.job.terminate();
    }
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}
#[cfg(windows)]
impl Job {
    fn create(name: *const u16) -> io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
            System::JobObjects::*,
        };
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), name);
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let exists = !name.is_null() && GetLastError() == ERROR_ALREADY_EXISTS;
            let job = Self(handle);
            if exists {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }
    fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::HANDLE, System::JobObjects::AssignProcessToJobObject,
        };
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtResumeProcess(process: HANDLE) -> i32;
        }
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("子进程句柄不可用"))?;
        let job = Self::create(std::ptr::null())?;
        unsafe {
            if let Ok(result) =
                PRIVATE_REPOS.try_with(|outer| AssignProcessToJobObject(outer.0, process))
                && result == 0
            {
                return Err(io::Error::last_os_error());
            }
            if AssignProcessToJobObject(job.0, process) == 0 {
                return Err(io::Error::last_os_error());
            }
            if NtResumeProcess(process) < 0 {
                return Err(io::Error::other("无法恢复受控子进程"));
            }
            Ok(job)
        }
    }
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct Job;
#[cfg(not(windows))]
impl Job {
    fn attach(_: &tokio::process::Child) -> io::Result<Self> {
        Ok(Self)
    }
    fn terminate(&self) {}
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn managed_output_preserves_unicode_and_exit_status() {
        let output = Command::new("cmd")
            .args(["/d", "/c", "echo managed-job"])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("managed-job"));
    }
    #[tokio::test]
    async fn job_termination_reaps_a_silent_process() {
        let mut child = Command::new("cmd")
            .args(["/d", "/c", "ping -n 60 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        child.terminate();
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!result.success());
    }
}
