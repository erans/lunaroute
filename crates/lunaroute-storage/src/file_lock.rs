//! File locking for concurrent write protection

use crate::traits::{StorageError, StorageResult};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// File lock for concurrent access control
pub struct FileLock {
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl FileLock {
    /// Acquire an exclusive lock on a file
    ///
    /// This creates a lock file at `{path}.lock` and acquires an exclusive lock.
    /// The lock is automatically released when the FileLock is dropped.
    pub fn acquire<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let lock_path = Self::lock_path(path.as_ref());

        // Create parent directory if needed
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open or create the lock file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)?;

        // Acquire exclusive lock (platform-specific)
        Self::lock_file(&file)?;

        Ok(Self {
            file,
            path: lock_path,
        })
    }

    /// Try to acquire an exclusive lock without blocking
    ///
    /// Returns Ok(Some(lock)) if lock acquired successfully
    /// Returns Ok(None) if lock is already held
    /// Returns Err if an error occurred
    pub fn try_acquire<P: AsRef<Path>>(path: P) -> StorageResult<Option<Self>> {
        let lock_path = Self::lock_path(path.as_ref());

        // Create parent directory if needed
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open or create the lock file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)?;

        // Try to acquire exclusive lock
        match Self::try_lock_file(&file) {
            Ok(true) => Ok(Some(Self {
                file,
                path: lock_path,
            })),
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Generate lock file path
    fn lock_path(path: &Path) -> PathBuf {
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        PathBuf::from(lock_path)
    }

    /// Platform-specific file locking (blocking)
    #[cfg(unix)]
    fn lock_file(file: &File) -> StorageResult<()> {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_raw_fd();

        // LOCK_EX: exclusive lock
        let result = unsafe { libc::flock(fd, libc::LOCK_EX) };

        if result == 0 {
            Ok(())
        } else {
            Err(StorageError::Io(std::io::Error::last_os_error()))
        }
    }

    /// Platform-specific file locking (blocking) - Windows
    #[cfg(windows)]
    fn lock_file(file: &File) -> StorageResult<()> {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::fileapi::LockFileEx;
        use winapi::um::minwinbase::LOCKFILE_EXCLUSIVE_LOCK;

        let handle = file.as_raw_handle();

        let result = unsafe {
            LockFileEx(
                handle as _,
                LOCKFILE_EXCLUSIVE_LOCK,
                0,
                !0,
                !0,
                std::ptr::null_mut(),
            )
        };

        if result != 0 {
            Ok(())
        } else {
            Err(StorageError::Io(std::io::Error::last_os_error()))
        }
    }

    /// Platform-specific non-blocking file locking
    #[cfg(unix)]
    fn try_lock_file(file: &File) -> StorageResult<bool> {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_raw_fd();

        // LOCK_EX | LOCK_NB: exclusive non-blocking lock
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if result == 0 {
            Ok(true)
        } else {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                Ok(false)
            } else {
                Err(StorageError::Io(err))
            }
        }
    }

    /// Platform-specific non-blocking file locking - Windows
    #[cfg(windows)]
    fn try_lock_file(file: &File) -> StorageResult<bool> {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::fileapi::LockFileEx;
        use winapi::um::minwinbase::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};

        let handle = file.as_raw_handle();

        let result = unsafe {
            LockFileEx(
                handle as _,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                !0,
                !0,
                std::ptr::null_mut(),
            )
        };

        if result != 0 {
            Ok(true)
        } else {
            let err = std::io::Error::last_os_error();
            // Windows returns ERROR_LOCK_VIOLATION when lock is held
            if err.raw_os_error() == Some(33) {
                Ok(false)
            } else {
                Err(StorageError::Io(err))
            }
        }
    }

    /// Unlock (happens automatically on drop, but can be called explicitly)
    #[cfg(unix)]
    fn unlock(&self) -> StorageResult<()> {
        use std::os::unix::io::AsRawFd;

        let fd = self.file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_UN) };

        if result == 0 {
            Ok(())
        } else {
            Err(StorageError::Io(std::io::Error::last_os_error()))
        }
    }

    #[cfg(windows)]
    fn unlock(&self) -> StorageResult<()> {
        use std::os::windows::io::AsRawHandle;
        use winapi::um::fileapi::UnlockFileEx;

        let handle = self.file.as_raw_handle();

        let result = unsafe { UnlockFileEx(handle as _, 0, !0, !0, std::ptr::null_mut()) };

        if result != 0 {
            Ok(())
        } else {
            Err(StorageError::Io(std::io::Error::last_os_error()))
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Unlock is automatic on Unix (file descriptor close unlocks)
        // On Windows, we need to explicitly unlock
        let _ = self.unlock();

        // Try to remove lock file (ignore errors as file may be in use)
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_lock_acquire_and_release() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        {
            let _lock = FileLock::acquire(&file_path).unwrap();
            // Lock is held here
        }
        // Lock is released when dropped
    }

    #[test]
    fn test_file_lock_concurrent_access() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let lock1 = FileLock::acquire(&file_path).unwrap();

        // Try to acquire second lock (should fail)
        let lock2 = FileLock::try_acquire(&file_path).unwrap();
        assert!(lock2.is_none(), "Second lock should fail to acquire");

        drop(lock1);

        // Now we should be able to acquire
        let lock3 = FileLock::try_acquire(&file_path).unwrap();
        assert!(
            lock3.is_some(),
            "Should acquire lock after first is released"
        );
    }

    #[test]
    fn test_file_lock_creates_parent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("subdir/test.txt");

        let _lock = FileLock::acquire(&file_path).unwrap();
        // Should create subdir/.lock file
    }

    #[test]
    fn test_file_lock_reacquire_after_drop() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        {
            let _lock1 = FileLock::acquire(&file_path).unwrap();
        }

        // Should be able to reacquire after drop
        let _lock2 = FileLock::acquire(&file_path).unwrap();
    }
}
