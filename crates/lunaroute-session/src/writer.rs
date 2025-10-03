//! Session writer trait and multi-writer recorder implementation

use crate::events::SessionEvent;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Error, Debug)]
pub enum WriterError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Channel send error")]
    ChannelSend,

    #[error("Writer not initialized")]
    NotInitialized,

    #[error("Invalid data: {0}")]
    InvalidData(String),
}

pub type WriterResult<T> = Result<T, WriterError>;

/// Trait for session event writers
#[async_trait]
pub trait SessionWriter: Send + Sync {
    /// Write a single event
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()>;

    /// Write a batch of events (for efficiency)
    async fn write_batch(&self, events: &[SessionEvent]) -> WriterResult<()> {
        for event in events {
            self.write_event(event).await?;
        }
        Ok(())
    }

    /// Flush any pending writes
    async fn flush(&self) -> WriterResult<()> {
        Ok(())
    }

    /// Check if this writer supports batching
    fn supports_batching(&self) -> bool {
        false
    }
}

/// Multi-writer recorder that dispatches events to multiple writers asynchronously
pub struct MultiWriterRecorder {
    tx: mpsc::Sender<SessionEvent>,
    worker_handle: Option<JoinHandle<()>>,
}

impl MultiWriterRecorder {
    /// Create a new multi-writer recorder with the given writers
    pub fn new(writers: Vec<Arc<dyn SessionWriter>>) -> Self {
        Self::with_config(writers, RecorderConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(writers: Vec<Arc<dyn SessionWriter>>, config: RecorderConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.channel_buffer_size);

        let worker_handle = tokio::spawn(async move {
            Self::worker_loop(rx, writers, config).await;
        });

        Self {
            tx,
            worker_handle: Some(worker_handle),
        }
    }

    /// Record an event (non-blocking, fire-and-forget)
    /// Returns false if the event was dropped due to channel being full
    pub fn record_event(&self, event: SessionEvent) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Session recording buffer full, dropping event");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("Session recorder channel closed");
                false
            }
        }
    }

    /// Background worker loop
    async fn worker_loop(
        mut rx: mpsc::Receiver<SessionEvent>,
        writers: Vec<Arc<dyn SessionWriter>>,
        config: RecorderConfig,
    ) {
        let mut buffer = Vec::with_capacity(config.batch_size);
        let mut interval = tokio::time::interval(Duration::from_millis(config.batch_timeout_ms));

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    buffer.push(event);

                    // Flush when buffer is full
                    if buffer.len() >= config.batch_size {
                        Self::flush_buffer(&writers, &mut buffer).await;
                    }
                }
                _ = interval.tick() => {
                    // Periodic flush for low-traffic periods
                    if !buffer.is_empty() {
                        Self::flush_buffer(&writers, &mut buffer).await;
                    }
                }
                else => {
                    // Channel closed, flush remaining and exit
                    if !buffer.is_empty() {
                        Self::flush_buffer(&writers, &mut buffer).await;
                    }
                    break;
                }
            }
        }

        tracing::debug!("Session recorder worker loop exited");
    }

    /// Flush buffer to all writers
    async fn flush_buffer(writers: &[Arc<dyn SessionWriter>], buffer: &mut Vec<SessionEvent>) {
        if buffer.is_empty() {
            return;
        }

        let events = std::mem::take(buffer);
        let event_count = events.len();

        // Write to all destinations in parallel
        let futures: Vec<_> = writers
            .iter()
            .map(|writer| {
                let events = events.clone();
                let writer = Arc::clone(writer);
                async move {
                    if writer.supports_batching() {
                        writer.write_batch(&events).await
                    } else {
                        for event in &events {
                            writer.write_event(event).await?;
                        }
                        Ok(())
                    }
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        // Log any write failures but don't propagate
        for (i, result) in results.iter().enumerate() {
            if let Err(e) = result {
                tracing::error!(
                    writer = i,
                    error = %e,
                    event_count = event_count,
                    "Failed to write session events"
                );
            }
        }

        // Flush all writers
        let flush_futures: Vec<_> = writers.iter().map(|w| w.flush()).collect();
        let _ = futures::future::join_all(flush_futures).await;
    }

    /// Gracefully shutdown and flush all pending events
    /// This consumes the recorder and waits for the worker to finish
    pub async fn shutdown(mut self) -> WriterResult<()> {
        // tx will be automatically dropped when self is consumed, signaling shutdown to the worker

        // Wait for worker to finish flushing and exit
        if let Some(handle) = self.worker_handle.take() {
            handle
                .await
                .map_err(|_| WriterError::Database("Worker task panicked".to_string()))?;
        }

        tracing::info!("Session recorder shutdown complete");
        Ok(())
    }
}

impl Drop for MultiWriterRecorder {
    fn drop(&mut self) {
        // Worker will automatically shut down when tx is dropped
        // But log a warning if shutdown() wasn't called explicitly
        if self.worker_handle.is_some() {
            tracing::warn!(
                "MultiWriterRecorder dropped without calling shutdown(). \
                 Worker will exit but pending events may not be fully flushed."
            );
        }
    }
}

/// Configuration for the multi-writer recorder
#[derive(Debug, Clone)]
pub struct RecorderConfig {
    /// Maximum events to buffer before flushing
    pub batch_size: usize,
    /// Maximum time to wait before flushing (milliseconds)
    pub batch_timeout_ms: u64,
    /// Size of the channel buffer (prevents OOM under high load)
    pub channel_buffer_size: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            batch_timeout_ms: 100,
            // Allow up to 10,000 events in the channel before backpressure kicks in
            channel_buffer_size: 10_000,
        }
    }
}

/// Builder for MultiWriterRecorder
pub struct RecorderBuilder {
    writers: Vec<Arc<dyn SessionWriter>>,
    config: RecorderConfig,
}

impl RecorderBuilder {
    pub fn new() -> Self {
        Self {
            writers: Vec::new(),
            config: RecorderConfig::default(),
        }
    }

    pub fn add_writer(mut self, writer: Arc<dyn SessionWriter>) -> Self {
        self.writers.push(writer);
        self
    }

    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    pub fn batch_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.config.batch_timeout_ms = timeout_ms;
        self
    }

    pub fn build(self) -> MultiWriterRecorder {
        MultiWriterRecorder::with_config(self.writers, self.config)
    }
}

impl Default for RecorderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a MultiWriterRecorder from configuration
#[cfg(feature = "sqlite-writer")]
pub async fn build_from_config(
    config: &crate::config::SessionRecordingConfig,
) -> WriterResult<Option<MultiWriterRecorder>> {
    use crate::jsonl_writer::JsonlWriter;
    use crate::sqlite_writer::SqliteWriter;

    if !config.has_writers() {
        return Ok(None);
    }

    let mut builder = RecorderBuilder::new()
        .batch_size(config.worker.batch_size)
        .batch_timeout_ms(config.worker.batch_timeout_ms);

    // Add JSONL writer if enabled
    if config.is_jsonl_enabled()
        && let Some(jsonl_config) = &config.jsonl {
            let writer = JsonlWriter::new(jsonl_config.directory.clone());
            builder = builder.add_writer(Arc::new(writer));
        }

    // Add SQLite writer if enabled
    if config.is_sqlite_enabled()
        && let Some(sqlite_config) = &config.sqlite {
            let writer = SqliteWriter::new(&sqlite_config.path).await?;
            builder = builder.add_writer(Arc::new(writer));
        }

    Ok(Some(builder.build()))
}

/// Build a MultiWriterRecorder from configuration (without SQLite feature)
#[cfg(not(feature = "sqlite-writer"))]
pub async fn build_from_config(
    config: &crate::config::SessionRecordingConfig,
) -> WriterResult<Option<MultiWriterRecorder>> {
    use crate::jsonl_writer::JsonlWriter;

    if !config.is_jsonl_enabled() {
        return Ok(None);
    }

    let mut builder = RecorderBuilder::new()
        .batch_size(config.worker.batch_size)
        .batch_timeout_ms(config.worker.batch_timeout_ms);

    // Add JSONL writer if enabled
    if let Some(jsonl_config) = &config.jsonl {
        let writer = JsonlWriter::new(jsonl_config.directory.clone());
        builder = builder.add_writer(Arc::new(writer));
    }

    Ok(Some(builder.build()))
}
