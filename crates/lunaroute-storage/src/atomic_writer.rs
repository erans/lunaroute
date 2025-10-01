//! Atomic file writer to ensure safe file operations

use crate::traits::StorageResult;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Atomic file writer that writes to a temporary file and renames on success
pub struct AtomicWriter {
    temp_path: PathBuf,
    final_path: PathBuf,
    file: File,
}

impl AtomicWriter {
    /// Create a new atomic writer for the given path
    pub fn new<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let final_path = path.as_ref().to_path_buf();

        // Create parent directory if it doesn't exist
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Create temporary file path
        let temp_path = Self::temp_path(&final_path);

        // Create the temporary file
        let file = File::create(&temp_path)?;

        Ok(Self {
            temp_path,
            final_path,
            file,
        })
    }

    /// Write data to the temporary file
    pub fn write(&mut self, data: &[u8]) -> StorageResult<()> {
        self.file.write_all(data)?;
        Ok(())
    }

    /// Commit the write by renaming the temp file to the final path
    pub fn commit(mut self) -> StorageResult<()> {
        // Sync and flush to ensure all data is written
        self.file.sync_all()?;
        self.file.flush()?;

        // Get paths before consuming self
        let temp_path = self.temp_path.clone();
        let final_path = self.final_path.clone();

        // Prevent Drop from running (which would delete the temp file)
        std::mem::forget(self);

        // Atomic rename
        fs::rename(&temp_path, &final_path)?;

        Ok(())
    }

    /// Generate a temporary file path
    fn temp_path(final_path: &Path) -> PathBuf {
        let mut temp = final_path.as_os_str().to_owned();
        temp.push(".tmp");
        PathBuf::from(temp)
    }
}

impl Drop for AtomicWriter {
    fn drop(&mut self) {
        // Clean up temporary file if it still exists
        let _ = fs::remove_file(&self.temp_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_atomic_write_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let mut writer = AtomicWriter::new(&file_path).unwrap();
        writer.write(b"Hello, world!").unwrap();
        writer.commit().unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[test]
    fn test_atomic_write_creates_parent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("subdir/test.txt");

        let mut writer = AtomicWriter::new(&file_path).unwrap();
        writer.write(b"Test").unwrap();
        writer.commit().unwrap();

        assert!(file_path.exists());
    }

    #[test]
    fn test_atomic_write_rollback_on_drop() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        {
            let mut writer = AtomicWriter::new(&file_path).unwrap();
            writer.write(b"Should not be committed").unwrap();
            // Drop without commit
        }

        // File should not exist since we didn't commit
        assert!(!file_path.exists());
    }

    #[test]
    fn test_atomic_write_multiple_writes() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let mut writer = AtomicWriter::new(&file_path).unwrap();
        writer.write(b"Hello, ").unwrap();
        writer.write(b"world!").unwrap();
        writer.commit().unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[test]
    fn test_atomic_write_overwrites_existing() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        // Write initial content
        fs::write(&file_path, b"Old content").unwrap();

        // Overwrite with atomic writer
        let mut writer = AtomicWriter::new(&file_path).unwrap();
        writer.write(b"New content").unwrap();
        writer.commit().unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "New content");
    }
}
