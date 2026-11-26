use std::{
    fs, io,
    path::{Component, Path},
    time::Duration,
};

use crate::error::AppError;

/// Called only while the build holds the task's private-repository lease.
pub(super) async fn recover_index_lock(workspace: &Path, repo: &Path) -> Result<bool, AppError> {
    let lock = repo.join(".git/index.lock");
    let failure = |reason: &dyn std::fmt::Display| {
        AppError::conflict(format!(
            "无法安全恢复私有仓库 {} 的 .git/index.lock：{reason}；请结束占用后重新启动此任务",
            repo.display()
        ))
    };
    match fs::symlink_metadata(&lock) {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(false);
        }
        Err(e) => return Err(failure(&e)),
        Ok(_) => (),
    }
    let parts: Vec<_> = repo
        .strip_prefix(workspace)
        .map_err(|e| failure(&e))?
        .components()
        .collect();
    if !matches!(parts.as_slice(), [Component::Normal(_), Component::Normal(tasks), Component::Normal(_), Component::Normal(name)] if *tasks == "tasks" && (*name == "code-repo" || *name == "asset-repo"))
    {
        return Err(failure(&"不属于任务私有仓库"));
    }
    let mut path = workspace.to_path_buf();
    for part in parts
        .into_iter()
        .chain([Component::Normal(".git".as_ref())])
    {
        path.push(part);
        let meta = fs::symlink_metadata(&path).map_err(|e| failure(&e))?;
        if !meta.is_dir() || is_link(&meta) {
            return Err(failure(&"仓库路径不是普通目录"));
        }
    }
    for attempt in 0..=5 {
        match remove_exclusive(&lock) {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) && attempt < 5 => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            Err(e) => return Err(failure(&e)),
        }
    }
    unreachable!()
}

#[cfg(windows)]
fn is_link(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(windows)]
fn remove_exclusive(path: &Path) -> io::Result<()> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FileDispositionInfo, SetFileInformationByHandle,
    };
    let file = fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || is_link(&meta) {
        return Err(io::Error::other("锁不是普通文件"));
    }
    // Mark this exact, exclusively opened file for deletion; never reopen by path.
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            size_of_val(&disposition) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn is_link(meta: &fs::Metadata) -> bool {
    meta.is_symlink()
}

#[cfg(not(windows))]
fn remove_exclusive(_: &Path) -> io::Result<()> {
    Err(io::Error::other("当前平台无法确认锁文件未被占用"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::process::{Command, with_private_repos};
    use std::os::windows::fs::OpenOptionsExt;

    #[tokio::test]
    async fn recovery_preserves_index_and_worktree_and_skips_shared_repo() {
        let root = tempfile::tempdir().unwrap();
        for name in ["code-repo", "asset-repo"] {
            let repo = root.path().join("project/tasks/t").join(name);
            fs::create_dir_all(&repo).unwrap();
            let git = |args: &[&str]| {
                let status = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    status.status.success(),
                    "{}",
                    String::from_utf8_lossy(&status.stderr)
                );
            };
            git(&["init"]);
            fs::write(repo.join("asset"), b"staged").unwrap();
            git(&["add", "asset"]);
            fs::write(repo.join("asset"), b"local edit").unwrap();
            let index = fs::read(repo.join(".git/index")).unwrap();
            fs::write(repo.join(".git/index.lock"), b"interrupted index").unwrap();
            assert!(recover_index_lock(root.path(), &repo).await.unwrap());
            assert!(!recover_index_lock(root.path(), &repo).await.unwrap());
            assert_eq!(index, fs::read(repo.join(".git/index")).unwrap());
            assert_eq!(b"local edit"[..], fs::read(repo.join("asset")).unwrap());
        }
        let shared = root.path().join("project/main-repo");
        fs::create_dir_all(shared.join(".git")).unwrap();
        let lock = shared.join(".git/index.lock");
        fs::write(&lock, b"shared").unwrap();
        assert!(recover_index_lock(root.path(), &shared).await.is_err());
        assert_eq!(fs::read(lock).unwrap(), b"shared");
    }

    #[tokio::test]
    async fn active_lock_is_preserved_or_disappears_while_waiting() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("p/tasks/t/asset-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let path = repo.join(".git/index.lock");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let error = recover_index_lock(root.path(), &repo).await.unwrap_err();
        assert!(error.message().contains("结束占用后重新启动此任务"));
        assert!(path.exists());
        let (result, ()) = tokio::join!(recover_index_lock(root.path(), &repo), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(file);
            fs::remove_file(&path).unwrap();
        });
        assert!(!result.unwrap());
        fs::create_dir(&path).unwrap();
        assert!(recover_index_lock(root.path(), &repo).await.is_err());
        assert!(path.is_dir());
    }

    #[tokio::test]
    async fn canceled_owner_must_exit_before_retry_can_recover() {
        let root = tempfile::tempdir().unwrap();
        let task = root.path().join("p/tasks/t");
        let repo = task.join("asset-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let previous = tokio::spawn({
            let task = task.clone();
            let repo = repo.clone();
            async move {
                with_private_repos(&task, async {
                    let mut child = Command::new("cmd")
                        .current_dir(&repo)
                        .args([
                            "/d",
                            "/c",
                            "(echo busy & ping -n 60 127.0.0.1) > .git\\index.lock",
                        ])
                        .spawn()?;
                    ready.send(()).unwrap();
                    child.wait().await?;
                    Ok(())
                })
                .await
            }
        });
        started.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !repo.join(".git/index.lock").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            with_private_repos(&task, async { Ok(()) }).await,
            Err(AppError::Conflict(_))
        ));
        previous.abort();
        assert!(previous.await.unwrap_err().is_cancelled());
        assert!(
            with_private_repos(&task, recover_index_lock(root.path(), &repo))
                .await
                .unwrap()
        );
    }
}
