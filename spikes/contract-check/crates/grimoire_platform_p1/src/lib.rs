//! P1 addition to `grimoire_platform` (contract §5, `FileSystem::read_limited`). The trait is
//! mirrored: P0 methods verbatim plus the provided method.

use std::io::{self, Read};
use std::path::Path;

use grimoire_platform::{MemoryFileSystem, StdFileSystem};

pub trait FileSystem: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;

    fn read_limited(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
        let bytes = self.read(path)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_len {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        Ok(bytes)
    }
}

impl FileSystem for StdFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        grimoire_platform::FileSystem::read(self, path)
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        grimoire_platform::FileSystem::write_atomic(self, path, bytes)
    }

    fn exists(&self, path: &Path) -> bool {
        grimoire_platform::FileSystem::exists(self, path)
    }

    /// Contract text: `take(max_len.saturating_add(1))`.
    fn read_limited(&self, path: &Path, max_len: u64) -> io::Result<Vec<u8>> {
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(max_len.saturating_add(1)).read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_len {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        Ok(bytes)
    }
}

impl FileSystem for MemoryFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        grimoire_platform::FileSystem::read(self, path)
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        grimoire_platform::FileSystem::write_atomic(self, path, bytes)
    }

    fn exists(&self, path: &Path) -> bool {
        grimoire_platform::FileSystem::exists(self, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_limited_with_u64_max_reads_the_whole_file() {
        let path = std::env::temp_dir().join("grimoire_contract_check_read_limited.bin");
        std::fs::write(&path, b"abc").expect("write");
        let bytes = StdFileSystem.read_limited(&path, u64::MAX).expect("read");
        let _ = std::fs::remove_file(&path);
        assert_eq!(bytes, b"abc");
    }
}
