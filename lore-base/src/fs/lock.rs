// SPDX-FileCopyrightText: 2026 Epic Games, Inc.
// SPDX-License-Identifier: MIT
use std::fs::OpenOptions;
#[cfg(target_family = "unix")]
use std::os::fd::AsRawFd;
#[cfg(target_family = "windows")]
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::Duration;

#[cfg(target_family = "windows")]
use windows_sys::Win32::Storage::FileSystem;

pub struct FSLock {
    file: std::fs::File,
}

impl FSLock {
    /// Bounded acquisition for credential updates. Call from a blocking worker;
    /// never leave an abandoned worker waiting forever on an OS file lock.
    pub fn acquire_file_lock_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> std::io::Result<Self> {
        let path = path.as_ref();
        let mut name = path
            .file_name()
            .ok_or_else(|| std::io::Error::other("Missing lock filename"))?
            .to_os_string();
        name.push(".lock");
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("Missing lock directory"))?
            .canonicalize()?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(parent.join(name))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Credential store lock timed out",
                    ));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
        }
    }

    pub fn acquire_file_lock(path: impl AsRef<Path>) -> std::io::Result<FSLock> {
        let mut path = path.as_ref().to_path_buf();
        let mut file_name = path
            .file_name()
            .ok_or(std::io::Error::other(
                "Acquiring file lock on path with no file",
            ))?
            .to_owned();
        path.pop();
        let mut path = path.canonicalize()?;
        file_name.push(".lock");
        path.push(file_name);
        Self::acquire_exact_path(&path).map_err(|_err| {
            std::io::Error::other(format!("Failed to acquire lock file \"{path:?}\""))
        })
    }

    pub fn acquire_directory_lock(path: impl AsRef<Path>) -> std::io::Result<FSLock> {
        let path = path.as_ref().canonicalize()?.join("lock");
        Self::acquire_exact_path(&path)
    }

    fn acquire_exact_path(path: impl AsRef<Path> + Copy) -> std::io::Result<FSLock> {
        let mut retry = 2;
        let file = loop {
            let file = OpenOptions::new()
                .create(false)
                .truncate(false)
                .write(false)
                .read(true)
                .open(path);
            if let Ok(file) = file {
                break file;
            }

            let file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .read(true)
                .open(path);
            if let Ok(file) = file {
                break file;
            }

            retry -= 1;
            if retry == 0 {
                return Err(file.unwrap_err());
            }
            std::thread::sleep(Duration::from_millis(10));
        };

        #[cfg(target_family = "windows")]
        {
            // Safety: Calling OS functions
            let ret = unsafe {
                let mut overlapped = std::mem::zeroed();
                FileSystem::LockFileEx(
                    file.as_raw_handle(),
                    FileSystem::LOCKFILE_EXCLUSIVE_LOCK,
                    0,
                    !0,
                    !0,
                    &mut overlapped,
                )
            };
            if ret == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(Self { file })
            }
        }

        #[cfg(not(target_family = "windows"))]
        {
            // Safety: Calling OS functions
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(Self { file })
            }
        }
    }
}

impl Drop for FSLock {
    fn drop(&mut self) {
        #[cfg(target_family = "windows")]
        {
            // Safety: Calling OS functions
            unsafe { FileSystem::UnlockFile(self.file.as_raw_handle(), 0, 0, !0, !0) };
        }

        #[cfg(not(target_family = "windows"))]
        {
            // Safety: Calling OS functions
            unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[test]
    fn bounded_lock_times_out_and_releases() {
        let path = std::env::temp_dir().join(format!(
            "portals-refresh-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = FSLock::acquire_file_lock_timeout(&path, Duration::from_secs(1)).unwrap();
        let failure = FSLock::acquire_file_lock_timeout(&path, Duration::from_millis(30));
        assert!(matches!(failure, Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut));
        let child = |expect_free: bool| {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "fs::lock::bounded_tests::bounded_lock_child",
                    "--test-threads=1",
                ])
                .env("PORTALS_TEST_LOCK_PATH", &path)
                .env(
                    "PORTALS_TEST_LOCK_EXPECT_FREE",
                    if expect_free { "yes" } else { "no" },
                )
                .status()
                .unwrap();
            assert!(status.success());
        };
        child(false);
        drop(first);
        child(true);
        drop(FSLock::acquire_file_lock_timeout(&path, Duration::from_secs(1)).unwrap());
        std::fs::remove_file(path.with_file_name(format!(
            "{}.lock",
            path.file_name().unwrap().to_string_lossy()
        )))
        .unwrap();
    }

    #[test]
    fn bounded_lock_child() {
        let Some(path) = std::env::var_os("PORTALS_TEST_LOCK_PATH") else {
            return;
        };
        let result = FSLock::acquire_file_lock_timeout(path, Duration::from_millis(50));
        if std::env::var("PORTALS_TEST_LOCK_EXPECT_FREE").unwrap() == "yes" {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut));
        }
    }
}
