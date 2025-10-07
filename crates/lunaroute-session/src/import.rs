//! Import JSONL session logs into SQLite database

use crate::events::SessionEvent;
#[cfg(feature = "sqlite-writer")]
use crate::sqlite_writer::SqliteWriter;
#[cfg(feature = "sqlite-writer")]
use crate::writer::SessionWriter;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Metadata about a session file
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub path: PathBuf,
    pub session_id: String,
    pub started_at: DateTime<Utc>,
}

/// Result of importing a single session
#[derive(Debug)]
pub enum ImportResult {
    Success { session_id: String },
    Skipped { session_id: String, reason: String },
    Failed { session_id: String, error: String },
}

/// Configuration for session import
#[derive(Debug, Clone)]
pub struct ImportConfig {
    /// Path to JSONL sessions directory
    pub sessions_dir: PathBuf,
    /// Path to SQLite database
    pub db_path: PathBuf,
    /// Number of sessions to process in one batch
    pub batch_size: usize,
    /// Skip sessions that already exist in database
    pub skip_existing: bool,
    /// Continue importing even if some sessions fail
    pub continue_on_error: bool,
    /// Show what would be imported without writing to DB
    pub dry_run: bool,
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self {
            sessions_dir: PathBuf::from("~/.lunaroute/sessions"),
            db_path: PathBuf::from("~/.lunaroute/sessions.db"),
            batch_size: 10,
            skip_existing: true,
            continue_on_error: true,
            dry_run: false,
        }
    }
}

/// Scan sessions directory recursively and return all JSONL files sorted by creation time
pub async fn scan_sessions(dir: &Path) -> Result<Vec<SessionFile>> {
    let mut session_files = Vec::new();
    scan_directory_recursive(dir, &mut session_files).await?;

    // Sort by started_at timestamp
    session_files.sort_by_key(|f| f.started_at);

    Ok(session_files)
}

/// Recursively scan a directory for JSONL files
fn scan_directory_recursive<'a>(
    dir: &'a Path,
    session_files: &'a mut Vec<SessionFile>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        let mut entries = fs::read_dir(dir)
            .await
            .with_context(|| format!("Failed to read directory: {}", dir.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .context("Failed to read directory entry")?
        {
            let path = entry.path();
            let metadata = entry.metadata().await.context("Failed to read metadata")?;

            if metadata.is_dir() {
                // Recursively scan subdirectories
                if let Err(e) = scan_directory_recursive(&path, session_files).await {
                    eprintln!("Warning: Failed to scan directory {}: {}", path.display(), e);
                }
            } else if path.extension().map_or(false, |ext| ext == "jsonl") {
                // Process .jsonl files
                match extract_session_metadata(&path).await {
                    Ok(metadata) => session_files.push(metadata),
                    Err(e) => {
                        eprintln!("Warning: Failed to read {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(())
    })
}

/// Extract session_id and started_at from first event in JSONL file
async fn extract_session_metadata(path: &Path) -> Result<SessionFile> {
    let file = fs::File::open(path)
        .await
        .context("Failed to open file")?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Read first line
    let first_line = lines
        .next_line()
        .await
        .context("Failed to read first line")?
        .context("File is empty")?;

    // Parse as SessionEvent
    let event: SessionEvent = serde_json::from_str(&first_line)
        .context("Failed to parse first event")?;

    // Extract session_id and started_at
    let (session_id, started_at) = match event {
        SessionEvent::Started {
            session_id,
            timestamp,
            ..
        } => (session_id, timestamp),
        _ => anyhow::bail!("First event is not SessionEvent::Started"),
    };

    Ok(SessionFile {
        path: path.to_path_buf(),
        session_id,
        started_at,
    })
}

/// Read all events from a JSONL session file
async fn read_session_events(path: &Path) -> Result<Vec<SessionEvent>> {
    let file = fs::File::open(path)
        .await
        .context("Failed to open file")?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let mut events = Vec::new();
    let mut line_num = 0;

    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                line_num += 1;
                let event: SessionEvent = serde_json::from_str(&line)
                    .with_context(|| format!("Failed to parse event at line {}", line_num))?;
                events.push(event);
            }
            Ok(None) => break,
            Err(e) => return Err(e).context("Failed to read line"),
        }
    }

    Ok(events)
}

/// Check if a session exists in the database
#[cfg(feature = "sqlite-writer")]
async fn session_exists(db_path: &Path, session_id: &str) -> Result<bool> {
    use sqlx::SqlitePool;

    let pool = SqlitePool::connect(&format!("sqlite://{}", db_path.display()))
        .await
        .context("Failed to connect to database")?;

    let exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sessions WHERE session_id = ?"
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .context("Failed to check if session exists")?;

    pool.close().await;

    Ok(exists)
}

/// Import a single session file
#[cfg(feature = "sqlite-writer")]
async fn import_session(
    writer: &SqliteWriter,
    session_file: &SessionFile,
    config: &ImportConfig,
) -> Result<ImportResult> {
    // Check if session already exists
    if config.skip_existing {
        if session_exists(&config.db_path, &session_file.session_id).await? {
            return Ok(ImportResult::Skipped {
                session_id: session_file.session_id.clone(),
                reason: "already exists".to_string(),
            });
        }
    }

    // Read all events from file
    let events = read_session_events(&session_file.path).await?;

    if config.dry_run {
        return Ok(ImportResult::Success {
            session_id: session_file.session_id.clone(),
        });
    }

    // Write events to database
    // Note: SqliteWriter supports batching, so we can use write_event multiple times
    for event in &events {
        writer
            .write_event(event)
            .await
            .context("Failed to write event to database")?;
    }

    Ok(ImportResult::Success {
        session_id: session_file.session_id.clone(),
    })
}

/// Import all sessions from JSONL files into SQLite database
#[cfg(feature = "sqlite-writer")]
pub async fn import_sessions(config: ImportConfig) -> Result<Vec<ImportResult>> {
    println!("Scanning sessions directory: {}", config.sessions_dir.display());

    // Scan and sort session files
    let session_files = scan_sessions(&config.sessions_dir).await?;
    let total = session_files.len();

    println!("Found {} session files to import\n", total);

    if config.dry_run {
        println!("DRY RUN MODE - No data will be written\n");
    }

    // Create SQLite writer
    let writer = SqliteWriter::new(&config.db_path).await?;

    let mut results = Vec::new();
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = 0;

    // Import sessions one by one with progress display
    for (idx, session_file) in session_files.iter().enumerate() {
        let progress = idx + 1;

        // Import session
        let result = match import_session(&writer, session_file, &config).await {
            Ok(result) => result,
            Err(e) => {
                if config.continue_on_error {
                    ImportResult::Failed {
                        session_id: session_file.session_id.clone(),
                        error: e.to_string(),
                    }
                } else {
                    return Err(e).with_context(|| {
                        format!("Failed to import session {}", session_file.session_id)
                    });
                }
            }
        };

        // Display progress
        match &result {
            ImportResult::Success { session_id } => {
                println!(
                    "[{}/{}] {} ({}) ✓",
                    progress, total, session_id, session_file.started_at
                );
                imported += 1;
            }
            ImportResult::Skipped { session_id, reason } => {
                println!(
                    "[{}/{}] {} ({}) ⊘ skipped ({})",
                    progress, total, session_id, session_file.started_at, reason
                );
                skipped += 1;
            }
            ImportResult::Failed { session_id, error } => {
                println!(
                    "[{}/{}] {} ({}) ✗ failed",
                    progress, total, session_id, session_file.started_at
                );
                eprintln!("  Error: {}", error);
                failed += 1;
            }
        }

        results.push(result);
    }

    // Print summary
    println!("\n{}", "=".repeat(60));
    println!("Summary:");
    println!("  Total sessions: {}", total);
    println!("  Imported: {}", imported);
    println!("  Skipped: {}", skipped);
    println!("  Failed: {}", failed);

    if failed > 0 {
        println!("\nFailed sessions:");
        for result in &results {
            if let ImportResult::Failed { session_id, error } = result {
                println!("  - {}: {}", session_id, error);
            }
        }
    }

    println!("{}", "=".repeat(60));

    Ok(results)
}
