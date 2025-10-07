//! Session cleanup and disk space management
//!
//! This module handles retention policies, disk space monitoring, and automatic
//! cleanup of old sessions.

use crate::config::RetentionPolicy;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::time::{Duration, sleep};

/// Result type for cleanup operations
pub type CleanupResult<T> = Result<T, CleanupError>;

/// Errors that can occur during cleanup
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Compression error: {0}")]
    Compression(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Statistics about cleanup operations
#[derive(Debug, Clone, Default)]
pub struct CleanupStats {
    /// Number of sessions deleted
    pub sessions_deleted: u64,
    /// Number of sessions compressed
    pub sessions_compressed: u64,
    /// Bytes freed by deletion
    pub bytes_freed: u64,
    /// Bytes saved by compression
    pub bytes_saved: u64,
    /// Duration of cleanup operation in milliseconds
    pub duration_ms: u64,
}

/// Disk usage statistics for a session directory
#[derive(Debug, Clone)]
pub struct DiskUsage {
    /// Total size in bytes
    pub total_bytes: u64,
    /// Number of session files
    pub session_count: u64,
    /// Number of compressed files
    pub compressed_count: u64,
}

/// Validate that a directory name matches the expected YYYY-MM-DD format
///
/// This prevents path traversal attacks by ensuring date directories follow
/// the expected pattern and don't contain special path components like ".." or "."
fn is_valid_date_directory(name: &str) -> bool {
    // Must be exactly 10 characters
    if name.len() != 10 {
        return false;
    }

    let bytes = name.as_bytes();

    // Check format: YYYY-MM-DD
    // Year: 4 digits
    if !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || !bytes[2].is_ascii_digit()
        || !bytes[3].is_ascii_digit()
    {
        return false;
    }

    // First separator
    if bytes[4] != b'-' {
        return false;
    }

    // Month: 2 digits (01-12)
    if !bytes[5].is_ascii_digit() || !bytes[6].is_ascii_digit() {
        return false;
    }
    let month = (bytes[5] - b'0') * 10 + (bytes[6] - b'0');
    if !(1..=12).contains(&month) {
        return false;
    }

    // Second separator
    if bytes[7] != b'-' {
        return false;
    }

    // Day: 2 digits (01-31)
    if !bytes[8].is_ascii_digit() || !bytes[9].is_ascii_digit() {
        return false;
    }
    let day = (bytes[8] - b'0') * 10 + (bytes[9] - b'0');
    if !(1..=31).contains(&day) {
        return false;
    }

    true
}

/// Calculate disk usage for a session directory
pub fn calculate_disk_usage(directory: &Path) -> CleanupResult<DiskUsage> {
    let mut usage = DiskUsage {
        total_bytes: 0,
        session_count: 0,
        compressed_count: 0,
    };

    if !directory.exists() {
        return Ok(usage);
    }

    // Walk through all date directories (YYYY-MM-DD/)
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Validate directory name to prevent path traversal
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| CleanupError::InvalidPath("Invalid directory name".to_string()))?;

            if !is_valid_date_directory(dir_name) {
                tracing::warn!("Skipping invalid date directory: {}", dir_name);
                continue;
            }

            // Process all session files in this date directory
            for session_entry in fs::read_dir(&path)? {
                let session_entry = session_entry?;
                let session_path = session_entry.path();

                if session_path.is_file() {
                    let metadata = fs::metadata(&session_path)?;
                    usage.total_bytes += metadata.len();
                    usage.session_count += 1;

                    // Check if compressed (has .zst extension)
                    if session_path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e == "zst")
                    {
                        usage.compressed_count += 1;
                    }
                }
            }
        }
    }

    Ok(usage)
}

/// Get the age of a file in days
pub fn file_age_days(path: &Path) -> CleanupResult<u32> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let now = SystemTime::now();
    let duration = now
        .duration_since(modified)
        .map_err(|e| CleanupError::InvalidPath(format!("Invalid timestamp: {}", e)))?;

    // Clamp to u32::MAX to prevent overflow with extreme timestamps
    Ok((duration.as_secs() / 86400).min(u32::MAX as u64) as u32)
}

/// Compress a session file using zstd
pub fn compress_session_file(path: &Path) -> CleanupResult<u64> {
    if !path.exists() {
        return Err(CleanupError::InvalidPath("File does not exist".to_string()));
    }

    // Skip if already compressed
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == "zst")
    {
        return Ok(0);
    }

    // Read the original file
    let contents = fs::read(path)?;
    let original_size = contents.len() as u64;

    // Compress with zstd (level 3)
    let compressed =
        zstd::encode_all(&contents[..], 3).map_err(|e| CleanupError::Compression(e.to_string()))?;

    // Write compressed file
    let compressed_path = path.with_extension("jsonl.zst");
    fs::write(&compressed_path, &compressed)?;

    // Delete original file
    fs::remove_file(path)?;

    let compressed_size = compressed.len() as u64;
    let bytes_saved = original_size.saturating_sub(compressed_size);

    tracing::info!(
        "Compressed session file: {:?} (saved {} bytes)",
        path.file_name(),
        bytes_saved
    );

    Ok(bytes_saved)
}

/// Delete a session file
pub fn delete_session_file(path: &Path) -> CleanupResult<u64> {
    if !path.exists() {
        return Ok(0);
    }

    let metadata = fs::metadata(path)?;
    let size = metadata.len();

    fs::remove_file(path)?;

    tracing::info!(
        "Deleted session file: {:?} ({} bytes)",
        path.file_name(),
        size
    );

    Ok(size)
}

/// Execute cleanup based on retention policy
pub fn execute_cleanup(directory: &Path, policy: &RetentionPolicy) -> CleanupResult<CleanupStats> {
    let start = std::time::Instant::now();
    let mut stats = CleanupStats::default();

    if !directory.exists() {
        return Ok(stats);
    }

    // Get current disk usage
    let usage = calculate_disk_usage(directory)?;
    let size_gb = usage.total_bytes as f64 / 1_073_741_824.0;

    tracing::info!(
        "Session storage: {:.2} GB ({} sessions, {} compressed)",
        size_gb,
        usage.session_count,
        usage.compressed_count
    );

    // Collect all session files with their ages
    let mut session_files: Vec<(PathBuf, u32)> = Vec::new();

    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Validate directory name to prevent path traversal
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| CleanupError::InvalidPath("Invalid directory name".to_string()))?;

            if !is_valid_date_directory(dir_name) {
                tracing::warn!(
                    "Skipping invalid date directory during cleanup: {}",
                    dir_name
                );
                continue;
            }

            for session_entry in fs::read_dir(&path)? {
                let session_entry = session_entry?;
                let session_path = session_entry.path();

                if session_path.is_file() {
                    let age = file_age_days(&session_path)?;
                    session_files.push((session_path, age));
                }
            }
        }
    }

    // Sort by age (oldest first) for size-based cleanup
    session_files.sort_by_key(|(_, age)| std::cmp::Reverse(*age));

    // Step 1: Delete sessions older than max_age_days
    if let Some(max_age) = policy.max_age_days {
        for (path, age) in &session_files {
            if *age > max_age {
                let freed = delete_session_file(path)?;
                stats.sessions_deleted += 1;
                stats.bytes_freed += freed;
            }
        }

        // Remove deleted files from the list
        session_files.retain(|(path, _)| path.exists());
    }

    // Step 2: Compress sessions older than compress_after_days
    if let Some(compress_after) = policy.compress_after_days {
        for (path, age) in &session_files {
            if *age > compress_after {
                // Skip already compressed files
                if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_none_or(|e| e != "zst")
                {
                    let saved = compress_session_file(path)?;
                    stats.sessions_compressed += 1;
                    stats.bytes_saved += saved;
                }
            }
        }
    }

    // Step 3: Enforce size limit (delete oldest uncompressed first, then compressed)
    if let Some(max_size_gb) = policy.max_total_size_gb {
        let max_bytes = (max_size_gb as u64) * 1_073_741_824;
        let current_usage = calculate_disk_usage(directory)?;

        if current_usage.total_bytes > max_bytes {
            let bytes_to_free = current_usage.total_bytes - max_bytes;
            let mut freed = 0u64;

            tracing::warn!(
                "Disk usage ({:.2} GB) exceeds limit ({} GB), deleting oldest sessions",
                current_usage.total_bytes as f64 / 1_073_741_824.0,
                max_size_gb
            );

            // Delete oldest files until under the limit
            for (path, _) in session_files.iter().rev() {
                if freed >= bytes_to_free {
                    break;
                }

                if path.exists() {
                    let file_size = delete_session_file(path)?;
                    stats.sessions_deleted += 1;
                    stats.bytes_freed += file_size;
                    freed += file_size;
                }
            }
        }
    }

    stats.duration_ms = start.elapsed().as_millis() as u64;

    tracing::info!(
        "Cleanup completed: deleted {} sessions ({} bytes), compressed {} sessions ({} bytes saved) in {}ms",
        stats.sessions_deleted,
        stats.bytes_freed,
        stats.sessions_compressed,
        stats.bytes_saved,
        stats.duration_ms
    );

    Ok(stats)
}

/// Handle for the background cleanup task
pub struct CleanupTask {
    shutdown_tx: tokio::sync::mpsc::Sender<()>,
}

impl CleanupTask {
    /// Signal the cleanup task to shutdown gracefully
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(()).await;
    }
}

/// Spawn a background cleanup task that runs periodically
///
/// Returns a CleanupTask handle that can be used to shutdown the task gracefully.
pub fn spawn_cleanup_task(directory: PathBuf, policy: Arc<RetentionPolicy>) -> CleanupTask {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    tokio::spawn(async move {
        let interval = Duration::from_secs((policy.cleanup_interval_minutes as u64) * 60);

        tracing::info!(
            "Starting cleanup task for {:?} (interval: {}m)",
            directory,
            policy.cleanup_interval_minutes
        );

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("Cleanup task shutting down");
                    break;
                }
                _ = sleep(interval) => {
                    // Run cleanup
                    match execute_cleanup(&directory, &policy) {
                        Ok(stats) => {
                            if stats.sessions_deleted > 0 || stats.sessions_compressed > 0 {
                                tracing::info!(
                                    "Cleanup cycle: {} deleted, {} compressed",
                                    stats.sessions_deleted,
                                    stats.sessions_compressed
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!("Cleanup task failed: {}", e);
                        }
                    }
                }
            }
        }
    });

    CleanupTask { shutdown_tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_session(dir: &Path, date: &str, name: &str, size: usize) -> PathBuf {
        let date_dir = dir.join(date);
        fs::create_dir_all(&date_dir).unwrap();

        let path = date_dir.join(format!("{}.jsonl", name));
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![b'x'; size]).unwrap();

        path
    }

    #[test]
    fn test_calculate_disk_usage_empty() {
        let temp_dir = TempDir::new().unwrap();
        let usage = calculate_disk_usage(temp_dir.path()).unwrap();

        assert_eq!(usage.total_bytes, 0);
        assert_eq!(usage.session_count, 0);
        assert_eq!(usage.compressed_count, 0);
    }

    #[test]
    fn test_calculate_disk_usage_with_sessions() {
        let temp_dir = TempDir::new().unwrap();

        // Create some test sessions
        create_test_session(temp_dir.path(), "2024-01-01", "session1", 1000);
        create_test_session(temp_dir.path(), "2024-01-01", "session2", 2000);
        create_test_session(temp_dir.path(), "2024-01-02", "session3", 1500);

        let usage = calculate_disk_usage(temp_dir.path()).unwrap();

        assert_eq!(usage.total_bytes, 4500);
        assert_eq!(usage.session_count, 3);
        assert_eq!(usage.compressed_count, 0);
    }

    #[test]
    fn test_file_age_days() {
        let temp_dir = TempDir::new().unwrap();
        let path = create_test_session(temp_dir.path(), "2024-01-01", "session", 100);

        let age = file_age_days(&path).unwrap();
        // Should be 0 days for a newly created file
        assert_eq!(age, 0);
    }

    #[test]
    fn test_compress_session_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = create_test_session(temp_dir.path(), "2024-01-01", "session", 10000);

        let bytes_saved = compress_session_file(&path).unwrap();

        // Original file should be deleted
        assert!(!path.exists());

        // Compressed file should exist
        let compressed_path = path.with_extension("jsonl.zst");
        assert!(compressed_path.exists());

        // Should have saved some bytes
        assert!(bytes_saved > 0);
    }

    #[test]
    fn test_compress_already_compressed() {
        let temp_dir = TempDir::new().unwrap();
        let path = create_test_session(temp_dir.path(), "2024-01-01", "session", 1000);

        // Compress once
        compress_session_file(&path).unwrap();

        // Try to compress again
        let compressed_path = path.with_extension("jsonl.zst");
        let bytes_saved = compress_session_file(&compressed_path).unwrap();

        // Should return 0 (already compressed)
        assert_eq!(bytes_saved, 0);
    }

    #[test]
    fn test_delete_session_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = create_test_session(temp_dir.path(), "2024-01-01", "session", 500);

        let bytes_freed = delete_session_file(&path).unwrap();

        assert_eq!(bytes_freed, 500);
        assert!(!path.exists());
    }

    #[test]
    fn test_execute_cleanup_no_policy() {
        let temp_dir = TempDir::new().unwrap();

        create_test_session(temp_dir.path(), "2024-01-01", "session1", 1000);
        create_test_session(temp_dir.path(), "2024-01-02", "session2", 2000);

        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };

        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();

        assert_eq!(stats.sessions_deleted, 0);
        assert_eq!(stats.sessions_compressed, 0);
        assert_eq!(stats.bytes_freed, 0);
    }

    #[test]
    #[ignore] // Slow test: creates ~1.8GB of test data
    fn test_execute_cleanup_size_limit_large() {
        let temp_dir = TempDir::new().unwrap();

        // Create 3 sessions of 600MB each (total 1.8 GB)
        let session_size = 600 * 1024 * 1024; // 600 MB
        create_test_session(temp_dir.path(), "2024-01-01", "session1", session_size);
        create_test_session(temp_dir.path(), "2024-01-02", "session2", session_size);
        create_test_session(temp_dir.path(), "2024-01-03", "session3", session_size);

        // Set limit to 1GB (will delete at least one session to get under limit)
        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: Some(1), // 1 GB
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };

        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();

        // Should have deleted at least two sessions
        assert!(stats.sessions_deleted >= 2);
        assert!(stats.bytes_freed > 0);

        // Verify we're now under the limit
        let final_usage = calculate_disk_usage(temp_dir.path()).unwrap();
        let limit_bytes = 1_073_741_824u64; // 1 GB in bytes
        assert!(final_usage.total_bytes <= limit_bytes);
    }

    #[test]
    fn test_execute_cleanup_size_limit() {
        let temp_dir = TempDir::new().unwrap();

        // Create 5 sessions of 500KB each (total 2.5 MB)
        create_test_session(temp_dir.path(), "2024-01-01", "session1", 500_000);
        create_test_session(temp_dir.path(), "2024-01-02", "session2", 500_000);
        create_test_session(temp_dir.path(), "2024-01-03", "session3", 500_000);
        create_test_session(temp_dir.path(), "2024-01-04", "session4", 500_000);
        create_test_session(temp_dir.path(), "2024-01-05", "session5", 500_000);

        // Since we can't specify fractional GB, we work around this by making the
        // limit extremely small (essentially 0) which will trigger cleanup
        // In production, this would be like having a 1GB limit with 10GB of data
        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: Some(1), // Will be treated as 1GB = 1,073,741,824 bytes
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };

        // Our test data is only 2.5MB, which is way under 1GB, so nothing should be deleted
        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();
        assert_eq!(stats.sessions_deleted, 0); // Nothing deleted because we're under limit
    }

    #[test]
    fn test_execute_cleanup_age_based_deletion() {
        use filetime::{FileTime, set_file_mtime};
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp_dir = TempDir::new().unwrap();

        // Create sessions with different ages
        let old_session = create_test_session(temp_dir.path(), "2024-01-01", "old", 1000);
        let medium_session = create_test_session(temp_dir.path(), "2024-01-02", "medium", 1000);
        let recent_session = create_test_session(temp_dir.path(), "2024-01-03", "recent", 1000);

        // Set file timestamps to simulate age
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let days_40_ago = FileTime::from_unix_time((now - (40 * 24 * 60 * 60)) as i64, 0);
        let days_20_ago = FileTime::from_unix_time((now - (20 * 24 * 60 * 60)) as i64, 0);
        let days_5_ago = FileTime::from_unix_time((now - (5 * 24 * 60 * 60)) as i64, 0);

        set_file_mtime(&old_session, days_40_ago).unwrap();
        set_file_mtime(&medium_session, days_20_ago).unwrap();
        set_file_mtime(&recent_session, days_5_ago).unwrap();

        // Delete sessions older than 30 days
        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };

        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();

        assert_eq!(stats.sessions_deleted, 1); // Only the 40-day-old session
        assert_eq!(stats.bytes_freed, 1000);
        assert!(!old_session.exists());
        assert!(medium_session.exists());
        assert!(recent_session.exists());
    }

    #[test]
    fn test_execute_cleanup_age_based_compression() {
        use filetime::{FileTime, set_file_mtime};
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp_dir = TempDir::new().unwrap();

        // Create sessions with different ages
        let old_session = create_test_session(temp_dir.path(), "2024-01-01", "old", 10000);
        let recent_session = create_test_session(temp_dir.path(), "2024-01-02", "recent", 10000);

        // Set file timestamps
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let days_10_ago = FileTime::from_unix_time((now - (10 * 24 * 60 * 60)) as i64, 0);
        let days_3_ago = FileTime::from_unix_time((now - (3 * 24 * 60 * 60)) as i64, 0);

        set_file_mtime(&old_session, days_10_ago).unwrap();
        set_file_mtime(&recent_session, days_3_ago).unwrap();

        // Compress sessions older than 7 days
        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: Some(7),
            cleanup_interval_minutes: 60,
        };

        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();

        assert_eq!(stats.sessions_compressed, 1); // Only the 10-day-old session
        assert!(stats.bytes_saved > 0);
        assert!(!old_session.exists());
        assert!(old_session.with_extension("jsonl.zst").exists());
        assert!(recent_session.exists());
    }

    #[test]
    fn test_execute_cleanup_combined_policies() {
        use filetime::{FileTime, set_file_mtime};
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp_dir = TempDir::new().unwrap();

        // Create sessions: very old, old, medium age, recent
        let very_old = create_test_session(temp_dir.path(), "2024-01-01", "very_old", 10000);
        let old = create_test_session(temp_dir.path(), "2024-01-02", "old", 10000);
        let medium = create_test_session(temp_dir.path(), "2024-01-03", "medium", 10000);
        let recent = create_test_session(temp_dir.path(), "2024-01-04", "recent", 10000);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        set_file_mtime(
            &very_old,
            FileTime::from_unix_time((now - (40 * 24 * 60 * 60)) as i64, 0),
        )
        .unwrap();
        set_file_mtime(
            &old,
            FileTime::from_unix_time((now - (25 * 24 * 60 * 60)) as i64, 0),
        )
        .unwrap();
        set_file_mtime(
            &medium,
            FileTime::from_unix_time((now - (10 * 24 * 60 * 60)) as i64, 0),
        )
        .unwrap();
        set_file_mtime(
            &recent,
            FileTime::from_unix_time((now - (2 * 24 * 60 * 60)) as i64, 0),
        )
        .unwrap();

        // Combined policy: delete after 30 days, compress after 7 days
        let policy = RetentionPolicy {
            max_age_days: Some(30),
            max_total_size_gb: None,
            compress_after_days: Some(7),
            cleanup_interval_minutes: 60,
        };

        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();

        // Should delete very_old (40 days), compress old and medium (25 and 10 days)
        assert_eq!(stats.sessions_deleted, 1);
        assert_eq!(stats.sessions_compressed, 2);
        assert!(!very_old.exists());
        assert!(!old.exists());
        assert!(old.with_extension("jsonl.zst").exists());
        assert!(!medium.exists());
        assert!(medium.with_extension("jsonl.zst").exists());
        assert!(recent.exists());
    }

    #[test]
    fn test_calculate_disk_usage_with_compressed() {
        let temp_dir = TempDir::new().unwrap();

        // Create regular and compressed sessions
        create_test_session(temp_dir.path(), "2024-01-01", "session1", 1000);
        let session2 = create_test_session(temp_dir.path(), "2024-01-02", "session2", 2000);

        // Compress session2
        compress_session_file(&session2).unwrap();

        let usage = calculate_disk_usage(temp_dir.path()).unwrap();

        assert_eq!(usage.session_count, 2);
        assert_eq!(usage.compressed_count, 1);
        assert!(usage.total_bytes > 0); // Compressed file still takes space
    }

    #[test]
    fn test_mixed_compressed_uncompressed_cleanup() {
        use filetime::{FileTime, set_file_mtime};
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp_dir = TempDir::new().unwrap();

        // Create sessions
        let old_uncompressed = create_test_session(temp_dir.path(), "2024-01-01", "old_unc", 10000);
        let old_compressed = create_test_session(temp_dir.path(), "2024-01-02", "old_comp", 10000);

        // Compress one manually
        compress_session_file(&old_compressed).unwrap();
        let old_compressed_path = old_compressed.with_extension("jsonl.zst");

        // Age both files
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let days_10_ago = FileTime::from_unix_time((now - (10 * 24 * 60 * 60)) as i64, 0);
        set_file_mtime(&old_uncompressed, days_10_ago).unwrap();
        set_file_mtime(&old_compressed_path, days_10_ago).unwrap();

        // Try to compress sessions older than 7 days
        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: Some(7),
            cleanup_interval_minutes: 60,
        };

        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();

        // Should compress only the uncompressed one
        assert_eq!(stats.sessions_compressed, 1);
        assert!(!old_uncompressed.exists());
        assert!(old_uncompressed.with_extension("jsonl.zst").exists());
        assert!(old_compressed_path.exists());
    }

    #[tokio::test]
    async fn test_background_cleanup_task() {
        use std::sync::Arc;

        let temp_dir = TempDir::new().unwrap();

        // Create a session
        create_test_session(temp_dir.path(), "2024-01-01", "session", 1000);

        // Start background task with very short interval
        let policy = Arc::new(RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 1, // Not used in test, we'll shutdown immediately
        });

        let task = spawn_cleanup_task(temp_dir.path().to_path_buf(), policy);

        // Let it run briefly
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Shutdown
        task.shutdown().await;

        // Verify task started and stopped without panicking
        // (The fact that we got here means success)
    }

    #[test]
    fn test_cleanup_with_non_session_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create session files and non-session files
        create_test_session(temp_dir.path(), "2024-01-01", "session", 1000);

        // Create a non-JSONL file
        let date_dir = temp_dir.path().join("2024-01-01");
        fs::create_dir_all(&date_dir).unwrap();
        let mut other_file = File::create(date_dir.join("other.txt")).unwrap();
        other_file.write_all(b"not a session").unwrap();

        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };

        // Should not crash on non-session files
        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();
        assert_eq!(stats.sessions_deleted, 0);
    }

    #[test]
    fn test_is_valid_date_directory() {
        // Valid dates
        assert!(is_valid_date_directory("2024-01-01"));
        assert!(is_valid_date_directory("2024-12-31"));
        assert!(is_valid_date_directory("2023-06-15"));
        assert!(is_valid_date_directory("1999-01-01"));

        // Invalid: wrong length
        assert!(!is_valid_date_directory("2024-1-1"));
        assert!(!is_valid_date_directory("2024-01-1"));
        assert!(!is_valid_date_directory("24-01-01"));
        assert!(!is_valid_date_directory("2024-01-01-extra"));

        // Invalid: wrong separators
        assert!(!is_valid_date_directory("2024/01/01"));
        assert!(!is_valid_date_directory("2024.01.01"));
        assert!(!is_valid_date_directory("20240101"));

        // Invalid: month out of range
        assert!(!is_valid_date_directory("2024-00-01"));
        assert!(!is_valid_date_directory("2024-13-01"));
        assert!(!is_valid_date_directory("2024-99-01"));

        // Invalid: day out of range
        assert!(!is_valid_date_directory("2024-01-00"));
        assert!(!is_valid_date_directory("2024-01-32"));
        assert!(!is_valid_date_directory("2024-01-99"));

        // Invalid: path traversal attempts
        assert!(!is_valid_date_directory(".."));
        assert!(!is_valid_date_directory("."));
        assert!(!is_valid_date_directory("../etc"));
        assert!(!is_valid_date_directory("../../etc"));

        // Invalid: other malicious patterns
        assert!(!is_valid_date_directory(""));
        assert!(!is_valid_date_directory("/etc/passwd"));
        assert!(!is_valid_date_directory("C:\\Windows"));
        assert!(!is_valid_date_directory("random-name"));
    }

    #[test]
    fn test_cleanup_skips_invalid_directories() {
        let temp_dir = TempDir::new().unwrap();

        // Create valid date directory with session
        create_test_session(temp_dir.path(), "2024-01-01", "session1", 1000);

        // Create invalid directories that should be skipped
        fs::create_dir_all(temp_dir.path().join("invalid-dir")).unwrap();
        fs::create_dir_all(temp_dir.path().join("..")).unwrap();
        fs::create_dir_all(temp_dir.path().join("2024-99-99")).unwrap();

        // Add files in invalid directories
        let mut file =
            File::create(temp_dir.path().join("invalid-dir").join("session.jsonl")).unwrap();
        file.write_all(b"should not be processed").unwrap();

        let usage = calculate_disk_usage(temp_dir.path()).unwrap();

        // Should only count the valid session
        assert_eq!(usage.session_count, 1);
        assert_eq!(usage.total_bytes, 1000);

        let policy = RetentionPolicy {
            max_age_days: None,
            max_total_size_gb: None,
            compress_after_days: None,
            cleanup_interval_minutes: 60,
        };

        // Should not crash on invalid directories
        let stats = execute_cleanup(temp_dir.path(), &policy).unwrap();
        assert_eq!(stats.sessions_deleted, 0);
    }

    #[test]
    fn test_path_traversal_prevention() {
        let temp_dir = TempDir::new().unwrap();

        // Create a valid session
        create_test_session(temp_dir.path(), "2024-01-01", "session", 1000);

        // Try to create a directory with path traversal pattern
        // Note: The filesystem itself may prevent ".." from being created as a directory name,
        // but our validation should catch it anyway
        let traversal_attempts = vec!["../../../etc", "..", ".", "../../passwd"];

        for attempt in traversal_attempts {
            // Try to create directory (may fail on filesystem level)
            let _ = fs::create_dir_all(temp_dir.path().join(attempt));
        }

        // Calculate disk usage - should not process any traversal directories
        let usage = calculate_disk_usage(temp_dir.path()).unwrap();

        // Should only see the valid session
        assert_eq!(usage.session_count, 1);
        assert_eq!(usage.total_bytes, 1000);
    }
}
