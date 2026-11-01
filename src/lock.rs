use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use crate::error::{EncoreError, Result};

/// Advisory, process-wide lock. Held for the lifetime of an `App`, so two
/// `encore run` invocations on the same database cannot overlap.
pub struct LockGuard {
    _file: File,
    path: PathBuf,
}

impl LockGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;

        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: file,
                path: path.to_path_buf(),
            }),
            Err(TryLockError::WouldBlock) => Err(EncoreError::Locked(path.to_path_buf())),
            Err(TryLockError::Error(e)) => Err(e.into()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
