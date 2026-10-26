//! Save-root locking, mirroring vanilla's `DirectoryLock` (`session.lock`).
//!
//! Vanilla creates `session.lock` in the level directory, writes a snowman,
//! takes an exclusive OS file lock, and refuses to start when another instance
//! already holds it. Steel keeps every world under the `worlds.toml` save root,
//! so the root itself is locked instead of each world directory; the file name,
//! payload, and failure message stay vanilla's.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Name of the lock file vanilla places in every level directory.
pub const LOCK_FILE: &str = "session.lock";

/// Vanilla's dummy payload, so a plain-text look at the file is recognizable.
const DUMMY: &str = "\u{2603}";

/// An exclusive advisory lock on the save root, held until it is dropped.
///
/// Dropping closes the file, which releases the OS lock; the standalone server
/// drops it only after the final save.
pub struct SessionLock {
    file: File,
    path: PathBuf,
}

impl SessionLock {
    /// Creates `dir` if needed and takes an exclusive lock on its `session.lock`.
    ///
    /// # Errors
    /// Returns an error when the directory or file cannot be created, or when
    /// another process already holds the lock (vanilla's
    /// `DirectoryLock.LockException`).
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOCK_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        file.write_all(DUMMY.as_bytes())?;
        file.sync_all()?;
        file.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "{}: already locked (possibly by another Steel instance?)",
                    path.display()
                ),
            ),
            TryLockError::Error(error) => error,
        })?;
        Ok(Self { file, path })
    }

    /// Path of the locked file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Releases the OS lock, mirroring `DirectoryLock#close`.
    ///
    /// # Errors
    /// Returns an error when the platform refuses to release the lock.
    pub fn release(self) -> io::Result<()> {
        self.file.unlock()
    }
}

#[cfg(test)]
mod tests {
    use super::SessionLock;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock should be after the epoch")
            .as_nanos();
        env::temp_dir().join(format!("steelmc-session-lock-{name}-{suffix}"))
    }

    #[test]
    fn second_lock_on_the_same_root_is_rejected() {
        let dir = temp_dir("exclusive");
        let first = SessionLock::acquire(&dir).expect("first lock should succeed");
        let second = SessionLock::acquire(&dir);
        assert!(
            second.is_err(),
            "a second lock on the same save root must fail"
        );
        first.release().expect("lock should release");
        let third = SessionLock::acquire(&dir).expect("lock should be reacquirable");
        third.release().expect("lock should release");
        fs::remove_dir_all(&dir).expect("temp dir should be removable");
    }
}
