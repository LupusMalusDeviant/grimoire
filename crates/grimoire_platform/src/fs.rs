//! File system access behind a trait, so persistence code is testable in memory.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Minimal file system contract used by engine and game persistence.
pub trait FileSystem: Send + Sync {
    /// Reads a whole file.
    ///
    /// # Errors
    /// Returns the underlying I/O error, `NotFound` if the file does not exist.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Writes `bytes` so that a concurrent or later reader sees either the complete old or the
    /// complete new content — never a partial write (PRD-0015 FR-02). Creates parent directories.
    ///
    /// # Errors
    /// Returns the underlying I/O error; the previous content stays intact on failure.
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;

    /// Whether a file exists at `path`.
    fn exists(&self, path: &Path) -> bool;
}

/// File system of the operating system.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdFileSystem;

impl FileSystem for StdFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let _ = path;
        todo!("P0 stream platform")
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let _ = (path, bytes);
        todo!("P0 stream platform")
    }

    fn exists(&self, path: &Path) -> bool {
        let _ = path;
        todo!("P0 stream platform")
    }
}

/// In-memory file system for tests.
#[derive(Debug, Default)]
pub struct MemoryFileSystem {
    files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
}

impl MemoryFileSystem {
    /// Creates an empty in-memory file system.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl FileSystem for MemoryFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let _ = (path, &self.files);
        todo!("P0 stream platform")
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let _ = (path, bytes);
        todo!("P0 stream platform")
    }

    fn exists(&self, path: &Path) -> bool {
        let _ = path;
        todo!("P0 stream platform")
    }
}
