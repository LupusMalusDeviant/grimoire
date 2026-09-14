//! File system access behind a trait, so persistence code is testable in memory.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

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
///
/// [`FileSystem::write_atomic`] writes a uniquely named temporary file next to the target,
/// flushes it with `sync_all` and renames it over the target, which replaces an existing file on
/// all supported platforms.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdFileSystem;

/// Upper bound for temp-name collisions (e.g. stale files from a crashed process).
const MAX_TEMP_ATTEMPTS: u32 = 64;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn split_target(path: &Path) -> io::Result<(&Path, &std::ffi::OsStr)> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok((parent, file_name))
}

fn create_temp_file(dir: &Path, file_name: &std::ffi::OsStr) -> io::Result<(PathBuf, File)> {
    let mut last_error = None;
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_name = format!(
            ".{}.{}-{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            counter
        );
        let temp_path = dir.join(temp_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AlreadyExists, "no free temporary file name")
    }))
}

fn write_and_replace(
    temp_path: &Path,
    mut file: File,
    target: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()?;
    // Windows refuses to rename a file that still has an open handle.
    drop(file);
    fs::rename(temp_path, target)
}

impl FileSystem for StdFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let (parent, file_name) = split_target(path)?;
        fs::create_dir_all(parent)?;
        let (temp_path, file) = create_temp_file(parent, file_name)?;
        if let Err(error) = write_and_replace(&temp_path, file, path, bytes) {
            // The original error matters more than a failed cleanup.
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        // Persist the directory entry of the rename; directories cannot be opened as files on
        // Windows, and NTFS journals the rename itself.
        #[cfg(unix)]
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }
}

/// In-memory file system for tests.
///
/// Mirrors the observable behaviour of [`StdFileSystem`]: directories exist implicitly as
/// ancestors of stored files, a file cannot be written below another file or over a directory,
/// and reading a missing file fails with `NotFound`. Paths are compared as given, without
/// normalisation.
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

    fn lock(&self) -> MutexGuard<'_, BTreeMap<PathBuf, Vec<u8>>> {
        // Every mutation is a single map operation, so a poisoned map is still consistent.
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn is_directory(files: &BTreeMap<PathBuf, Vec<u8>>, path: &Path) -> bool {
    files
        .keys()
        .any(|file| file.as_path() != path && file.starts_with(path))
}

impl FileSystem for MemoryFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let files = self.lock();
        if let Some(bytes) = files.get(path) {
            return Ok(bytes.clone());
        }
        if is_directory(&files, path) {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!("is a directory: {}", path.display()),
            ));
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("file not found: {}", path.display()),
        ))
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        split_target(path)?;
        let mut files = self.lock();
        if let Some(ancestor) = path.ancestors().skip(1).find(|a| files.contains_key(*a)) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("parent is a file: {}", ancestor.display()),
            ));
        }
        if is_directory(&files, path) {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!("is a directory: {}", path.display()),
            ));
        }
        files.insert(path.to_path_buf(), bytes.to_vec());
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.lock().contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique directory under the OS temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "grimoire_platform_fs_{label}_{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp test dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn check_round_trip(fs: &dyn FileSystem, root: &Path) {
        let path = root.join("save.bin");
        assert!(!fs.exists(&path));
        fs.write_atomic(&path, b"hello grimoire").unwrap();
        assert!(fs.exists(&path));
        assert_eq!(fs.read(&path).unwrap(), b"hello grimoire");

        let empty = root.join("empty.bin");
        fs.write_atomic(&empty, &[]).unwrap();
        assert_eq!(fs.read(&empty).unwrap(), Vec::<u8>::new());
    }

    fn check_overwrite(fs: &dyn FileSystem, root: &Path) {
        let path = root.join("settings.ron");
        fs.write_atomic(&path, b"a much longer first version")
            .unwrap();
        fs.write_atomic(&path, b"short").unwrap();
        assert_eq!(fs.read(&path).unwrap(), b"short");
    }

    fn check_creates_parents(fs: &dyn FileSystem, root: &Path) {
        let path = root.join("profiles").join("slot_1").join("run.sav");
        fs.write_atomic(&path, &[1, 2, 3]).unwrap();
        assert!(fs.exists(&path));
        assert_eq!(fs.read(&path).unwrap(), vec![1, 2, 3]);
    }

    fn check_missing_file(fs: &dyn FileSystem, root: &Path) {
        let path = root.join("does_not_exist.bin");
        assert!(!fs.exists(&path));
        let error = fs.read(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    fn check_directory_is_not_a_file(fs: &dyn FileSystem, root: &Path) {
        let path = root.join("dir").join("inner.bin");
        fs.write_atomic(&path, b"x").unwrap();
        assert!(!fs.exists(&root.join("dir")));
        assert!(fs.write_atomic(&root.join("dir"), b"y").is_err());
        assert_eq!(fs.read(&path).unwrap(), b"x");
    }

    fn check_no_file_name(fs: &dyn FileSystem) {
        let error = fs.write_atomic(Path::new(".."), b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn std_round_trip() {
        let dir = TempDir::new("round_trip");
        check_round_trip(&StdFileSystem, dir.path());
    }

    #[test]
    fn std_overwrite() {
        let dir = TempDir::new("overwrite");
        check_overwrite(&StdFileSystem, dir.path());
    }

    #[test]
    fn std_creates_parent_directories() {
        let dir = TempDir::new("parents");
        check_creates_parents(&StdFileSystem, dir.path());
    }

    #[test]
    fn std_missing_file_is_not_found() {
        let dir = TempDir::new("missing");
        check_missing_file(&StdFileSystem, dir.path());
    }

    #[test]
    fn std_write_over_directory_fails() {
        let dir = TempDir::new("over_dir");
        check_directory_is_not_a_file(&StdFileSystem, dir.path());
    }

    #[test]
    fn std_rejects_path_without_file_name() {
        check_no_file_name(&StdFileSystem);
    }

    #[test]
    fn std_leaves_no_temp_files() {
        let dir = TempDir::new("no_temp");
        let fs = StdFileSystem;
        let path = dir.path().join("data.bin");
        for round in 0..5u8 {
            fs.write_atomic(&path, &[round; 32]).unwrap();
        }
        // A failing rename (target is a directory) must clean up its temp file as well.
        fs::create_dir(dir.path().join("blocked")).unwrap();
        assert!(fs.write_atomic(&dir.path().join("blocked"), b"x").is_err());

        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["blocked", "data.bin"]);
        assert_eq!(fs.read(&path).unwrap(), [4u8; 32]);
    }

    #[test]
    fn memory_round_trip() {
        check_round_trip(&MemoryFileSystem::new(), Path::new("mem"));
    }

    #[test]
    fn memory_overwrite() {
        check_overwrite(&MemoryFileSystem::new(), Path::new("mem"));
    }

    #[test]
    fn memory_creates_parent_directories() {
        check_creates_parents(&MemoryFileSystem::new(), Path::new("mem"));
    }

    #[test]
    fn memory_missing_file_is_not_found() {
        check_missing_file(&MemoryFileSystem::new(), Path::new("mem"));
    }

    #[test]
    fn memory_write_over_directory_fails() {
        check_directory_is_not_a_file(&MemoryFileSystem::new(), Path::new("mem"));
    }

    #[test]
    fn memory_rejects_path_without_file_name() {
        check_no_file_name(&MemoryFileSystem::new());
    }

    #[test]
    fn memory_rejects_file_below_file() {
        let fs = MemoryFileSystem::new();
        fs.write_atomic(Path::new("a/b"), b"file").unwrap();
        let error = fs.write_atomic(Path::new("a/b/c"), b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
        assert!(!fs.exists(Path::new("a/b/c")));
    }

    #[test]
    fn memory_is_shareable_across_threads() {
        let fs = std::sync::Arc::new(MemoryFileSystem::new());
        let handles: Vec<_> = (0..4u8)
            .map(|i| {
                let fs = std::sync::Arc::clone(&fs);
                std::thread::spawn(move || {
                    fs.write_atomic(&PathBuf::from(format!("t/{i}")), &[i])
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        for i in 0..4u8 {
            assert_eq!(fs.read(&PathBuf::from(format!("t/{i}"))).unwrap(), [i]);
        }
    }
}
