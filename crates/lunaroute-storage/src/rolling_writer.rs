//! Rolling file writer for stream events

use crate::traits::StorageResult;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Rolling file writer that creates new files when size threshold is reached
pub struct RollingWriter {
    base_path: PathBuf,
    max_file_size: u64,
    current_index: usize,
    current_file: Option<File>,
    current_size: u64,
}

impl RollingWriter {
    /// Create a new rolling writer
    ///
    /// # Arguments
    /// * `base_path` - Base path for files (will append .0, .1, etc.)
    /// * `max_file_size` - Maximum size per file in bytes
    pub fn new<P: AsRef<Path>>(base_path: P, max_file_size: u64) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            max_file_size,
            current_index: 0,
            current_file: None,
            current_size: 0,
        }
    }

    /// Write data to the rolling file, rotating if necessary
    pub fn write(&mut self, data: &[u8]) -> StorageResult<()> {
        // Check if we need to rotate
        if self.current_file.is_none() || self.current_size + data.len() as u64 > self.max_file_size
        {
            self.rotate()?;
        }

        // Write to current file
        if let Some(ref mut file) = self.current_file {
            file.write_all(data)?;
            file.sync_all()?;
            self.current_size += data.len() as u64;
        }

        Ok(())
    }

    /// Rotate to a new file
    fn rotate(&mut self) -> StorageResult<()> {
        // Close current file if open
        if let Some(file) = self.current_file.take() {
            file.sync_all()?;
        }

        // Create parent directory if needed
        if let Some(parent) = self.base_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Open new file
        let file_path = self.get_file_path(self.current_index);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        // Get current file size
        let metadata = file.metadata()?;
        self.current_size = metadata.len();

        // If this file is already at max size, move to next
        if self.current_size >= self.max_file_size {
            self.current_index += 1;
            return self.rotate();
        }

        self.current_file = Some(file);
        Ok(())
    }

    /// Get the path for a specific file index
    fn get_file_path(&self, index: usize) -> PathBuf {
        let mut path = self.base_path.as_os_str().to_owned();
        path.push(format!(".{}", index));
        PathBuf::from(path)
    }

    /// Read all data from all rolling files
    pub fn read_all<P: AsRef<Path>>(base_path: P) -> StorageResult<Vec<Vec<u8>>> {
        let base_path = base_path.as_ref();
        let mut all_lines = Vec::new();
        let mut index = 0;

        loop {
            let file_path = Self::get_file_path_static(base_path, index);
            if !file_path.exists() {
                break;
            }

            let content = fs::read_to_string(&file_path)?;
            for line in content.lines() {
                if !line.is_empty() {
                    all_lines.push(line.as_bytes().to_vec());
                }
            }

            index += 1;
        }

        Ok(all_lines)
    }

    /// Static version of get_file_path for reading
    fn get_file_path_static(base_path: &Path, index: usize) -> PathBuf {
        let mut path = base_path.as_os_str().to_owned();
        path.push(format!(".{}", index));
        PathBuf::from(path)
    }

    /// Get the current file index
    pub fn current_index(&self) -> usize {
        self.current_index
    }

    /// Flush and sync the current file
    pub fn flush(&mut self) -> StorageResult<()> {
        if let Some(ref mut file) = self.current_file {
            file.flush()?;
            file.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_rolling_writer_basic() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().join("stream.ndjson");

        let mut writer = RollingWriter::new(&base_path, 100);
        writer.write(b"line1\n").unwrap();
        writer.write(b"line2\n").unwrap();
        writer.flush().unwrap();

        // Read back
        let lines = RollingWriter::read_all(&base_path).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], b"line1");
        assert_eq!(lines[1], b"line2");
    }

    #[test]
    fn test_rolling_writer_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().join("stream.ndjson");

        let mut writer = RollingWriter::new(&base_path, 20); // Small size to force rotation

        // Write enough data to force rotation
        writer.write(b"line1_long_data\n").unwrap();
        writer.write(b"line2_long_data\n").unwrap();
        writer.write(b"line3_long_data\n").unwrap();
        writer.flush().unwrap();

        // Should have created multiple files
        assert!(writer.current_index() >= 1);

        // Read all data
        let lines = RollingWriter::read_all(&base_path).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], b"line1_long_data");
        assert_eq!(lines[1], b"line2_long_data");
        assert_eq!(lines[2], b"line3_long_data");
    }

    #[test]
    fn test_rolling_writer_empty_lines_filtered() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().join("stream.ndjson");

        let mut writer = RollingWriter::new(&base_path, 100);
        writer.write(b"line1\n").unwrap();
        writer.write(b"\n").unwrap();
        writer.write(b"line2\n").unwrap();
        writer.flush().unwrap();

        let lines = RollingWriter::read_all(&base_path).unwrap();
        assert_eq!(lines.len(), 2); // Empty line filtered
    }

    #[test]
    fn test_rolling_writer_creates_parent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().join("subdir/stream.ndjson");

        let mut writer = RollingWriter::new(&base_path, 100);
        writer.write(b"test\n").unwrap();
        writer.flush().unwrap();

        assert!(base_path.parent().unwrap().exists());
    }

    #[test]
    fn test_rolling_writer_multiple_rotations() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().join("stream.ndjson");

        let mut writer = RollingWriter::new(&base_path, 15);

        for i in 0..10 {
            writer.write(format!("line{}\n", i).as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        let lines = RollingWriter::read_all(&base_path).unwrap();
        assert_eq!(lines.len(), 10);
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line, format!("line{}", i).as_bytes());
        }
    }
}
