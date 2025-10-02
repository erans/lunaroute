# Session Recording Implementation Plan

## Phase 1: Core Infrastructure (Week 1)

### 1.1 Create New Crate: `lunaroute-session-v2`

```toml
# Cargo.toml
[dependencies]
tokio = { version = "1.40", features = ["full"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
async-trait = "0.1"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.10", features = ["v4"] }
tracing = "0.1"
futures = "0.3"
dashmap = "6.0"  # Concurrent HashMap
parking_lot = "0.12"  # Fast RwLock
zstd = "0.13"  # Compression
bytes = "1.5"
thiserror = "1.0"
```

### 1.2 Module Structure

```
crates/lunaroute-session-v2/
├── src/
│   ├── lib.rs              # Public API
│   ├── recorder.rs         # Core trait and MultiWriterRecorder
│   ├── events.rs           # SessionEvent enum and helpers
│   ├── writers/
│   │   ├── mod.rs
│   │   ├── jsonl.rs       # JSONL file writer
│   │   ├── sqlite.rs      # SQLite writer
│   │   └── metrics.rs     # Metrics writer (Prometheus)
│   ├── extractors.rs       # Text extraction utilities
│   ├── config.rs          # Configuration types
│   └── migrations/
│       ├── 001_initial.sql
│       └── 002_indexes.sql
```

## Phase 2: Key Implementation Files

### 2.1 Core Types (`src/events.rs`)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Started {
        session_id: String,
        timestamp: DateTime<Utc>,
        model_requested: String,
        provider: String,
        listener: String,
        client_ip: Option<String>,
        user_agent: Option<String>,
    },

    RequestRecorded {
        session_id: String,
        timestamp: DateTime<Utc>,
        request_text: String,
        request_json: Value,
        estimated_input_tokens: u32,
    },

    ResponseRecorded {
        session_id: String,
        timestamp: DateTime<Utc>,
        response_text: String,
        response_json: Value,
        input_tokens: u32,
        output_tokens: u32,
        thinking_tokens: Option<u32>,
        model_used: String,
        latency_ms: u64,
    },

    ToolCallRecorded {
        session_id: String,
        timestamp: DateTime<Utc>,
        tool_name: String,
        tool_input: Option<Value>,
        tool_output: Option<Value>,
    },

    StreamDelta {
        session_id: String,
        timestamp: DateTime<Utc>,
        delta_index: u32,
        content: String,
        delta_type: StreamDeltaType,
    },

    Error {
        session_id: String,
        timestamp: DateTime<Utc>,
        error_type: String,
        error_message: String,
        error_details: Option<Value>,
    },

    Completed {
        session_id: String,
        timestamp: DateTime<Utc>,
        success: bool,
        finish_reason: Option<String>,
        total_duration_ms: u64,
        total_input_tokens: u32,
        total_output_tokens: u32,
        total_thinking_tokens: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamDeltaType {
    Text,
    ToolCall,
    ToolResult,
    Metadata,
}

impl SessionEvent {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Started { session_id, .. } |
            Self::RequestRecorded { session_id, .. } |
            Self::ResponseRecorded { session_id, .. } |
            Self::ToolCallRecorded { session_id, .. } |
            Self::StreamDelta { session_id, .. } |
            Self::Error { session_id, .. } |
            Self::Completed { session_id, .. } => session_id,
        }
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::Started { timestamp, .. } |
            Self::RequestRecorded { timestamp, .. } |
            Self::ResponseRecorded { timestamp, .. } |
            Self::ToolCallRecorded { timestamp, .. } |
            Self::StreamDelta { timestamp, .. } |
            Self::Error { timestamp, .. } |
            Self::Completed { timestamp, .. } => *timestamp,
        }
    }
}
```

### 2.2 Text Extraction (`src/extractors.rs`)

```rust
use serde_json::Value;

/// Extract user message from Anthropic request
pub fn extract_anthropic_user_text(req: &Value) -> String {
    req.get("messages")
        .and_then(|msgs| msgs.as_array())
        .and_then(|msgs| msgs.iter().rev().find(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("user")
        }))
        .and_then(|msg| msg.get("content"))
        .and_then(|content| {
            if let Some(text) = content.as_str() {
                Some(text.to_string())
            } else if let Some(parts) = content.as_array() {
                let texts: Vec<String> = parts.iter()
                    .filter_map(|part| {
                        if part.get("type") == Some(&Value::String("text".to_string())) {
                            part.get("text").and_then(|t| t.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect();
                Some(texts.join("\n"))
            } else {
                None
            }
        })
        .unwrap_or_default()
}

/// Extract assistant message from Anthropic response
pub fn extract_anthropic_assistant_text(resp: &Value) -> String {
    resp.get("content")
        .and_then(|content| content.as_array())
        .map(|content_blocks| {
            content_blocks.iter()
                .filter_map(|block| {
                    if block.get("type") == Some(&Value::String("text".to_string())) {
                        block.get("text").and_then(|t| t.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Extract tool calls from response
pub fn extract_tool_calls(resp: &Value) -> Vec<(String, Value)> {
    let mut tool_calls = Vec::new();

    if let Some(content) = resp.get("content").and_then(|c| c.as_array()) {
        for block in content {
            if block.get("type") == Some(&Value::String("tool_use".to_string())) {
                if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    tool_calls.push((name.to_string(), input));
                }
            }
        }
    }

    tool_calls
}

/// Extract token counts from usage object
pub fn extract_token_count(usage: Option<&Value>, field: &str) -> u32 {
    usage
        .and_then(|u| u.get(field))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(0)
}
```

### 2.3 Main Recorder (`src/recorder.rs`)

```rust
use crate::{events::SessionEvent, writers::SessionWriter};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, warn, info, debug};

#[async_trait]
pub trait SessionRecorder: Send + Sync {
    /// Record an event (non-blocking)
    fn record(&self, event: SessionEvent);

    /// Generate a new session ID
    fn generate_session_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// Flush all pending events
    async fn flush(&self) -> Result<(), RecorderError>;

    /// Shutdown the recorder gracefully
    async fn shutdown(self) -> Result<(), RecorderError>;
}

pub struct MultiWriterRecorder {
    tx: mpsc::UnboundedSender<SessionEvent>,
    worker_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    metrics: Arc<RecorderMetrics>,
}

impl MultiWriterRecorder {
    pub fn new(
        writers: Vec<Box<dyn SessionWriter>>,
        config: RecorderConfig,
    ) -> Result<Self, RecorderError> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let metrics = Arc::new(RecorderMetrics::new());
        let metrics_clone = metrics.clone();

        let worker_handle = tokio::spawn(async move {
            let mut buffer = Vec::with_capacity(config.batch_size);
            let mut interval = tokio::time::interval(Duration::from_millis(config.flush_interval_ms));
            let writers = Arc::new(writers);

            info!("Session recorder worker started with {} writers", writers.len());

            loop {
                tokio::select! {
                    Some(event) = rx.recv() => {
                        metrics_clone.events_received.inc();
                        buffer.push(event);

                        // Flush when buffer is full
                        if buffer.len() >= config.batch_size {
                            debug!("Buffer full ({}), flushing", buffer.len());
                            Self::flush_buffer(&writers, &mut buffer, &metrics_clone).await;
                        }
                    }

                    _ = interval.tick() => {
                        // Periodic flush for low-traffic periods
                        if !buffer.is_empty() {
                            debug!("Periodic flush ({} events)", buffer.len());
                            Self::flush_buffer(&writers, &mut buffer, &metrics_clone).await;
                        }
                    }

                    else => {
                        // Channel closed, flush remaining and exit
                        if !buffer.is_empty() {
                            info!("Shutdown flush ({} events)", buffer.len());
                            Self::flush_buffer(&writers, &mut buffer, &metrics_clone).await;
                        }
                        break;
                    }
                }

                // Update metrics
                metrics_clone.queue_depth.set(buffer.len() as f64);
            }

            info!("Session recorder worker stopped");
        });

        Ok(Self {
            tx,
            worker_handle: Arc::new(RwLock::new(Some(worker_handle))),
            metrics,
        })
    }

    async fn flush_buffer(
        writers: &Arc<Vec<Box<dyn SessionWriter>>>,
        buffer: &mut Vec<SessionEvent>,
        metrics: &RecorderMetrics,
    ) {
        if buffer.is_empty() {
            return;
        }

        let start = tokio::time::Instant::now();
        let events = std::mem::take(buffer);
        let event_count = events.len();

        // Write to all destinations in parallel
        let futures: Vec<_> = writers
            .iter()
            .map(|writer| {
                let events = events.clone();
                async move {
                    writer.write_batch(&events).await
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        // Track results
        let mut any_success = false;
        for (i, result) in results.iter().enumerate() {
            match result {
                Ok(_) => {
                    any_success = true;
                    metrics.writes_succeeded.inc_by(event_count as f64);
                }
                Err(e) => {
                    error!(writer = i, error = %e, "Failed to write session events");
                    metrics.writes_failed.inc_by(event_count as f64);
                }
            }
        }

        if !any_success {
            warn!("All writers failed for {} events", event_count);
        }

        let duration = start.elapsed();
        metrics.write_duration.observe(duration.as_secs_f64());

        debug!(
            "Flushed {} events in {:?} ({} succeeded)",
            event_count,
            duration,
            if any_success { "some" } else { "none" }
        );
    }
}

impl SessionRecorder for MultiWriterRecorder {
    fn record(&self, event: SessionEvent) {
        // Non-blocking send
        if let Err(e) = self.tx.send(event) {
            error!("Failed to queue session event: {}", e);
            self.metrics.events_dropped.inc();
        }
    }

    async fn flush(&self) -> Result<(), RecorderError> {
        // Send a flush signal through a special event or use a separate channel
        // For now, just wait a bit for the periodic flush
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    async fn shutdown(self) -> Result<(), RecorderError> {
        info!("Shutting down session recorder");

        // Close the channel
        drop(self.tx);

        // Wait for worker to finish
        if let Some(handle) = self.worker_handle.write().take() {
            handle.await.map_err(|e| RecorderError::Shutdown(e.to_string()))?;
        }

        info!("Session recorder shutdown complete");
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub channel_buffer_size: usize,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            flush_interval_ms: 100,
            channel_buffer_size: 10000,
        }
    }
}

struct RecorderMetrics {
    events_received: prometheus::Counter,
    events_dropped: prometheus::Counter,
    writes_succeeded: prometheus::Counter,
    writes_failed: prometheus::Counter,
    write_duration: prometheus::Histogram,
    queue_depth: prometheus::Gauge,
}

impl RecorderMetrics {
    fn new() -> Self {
        // Initialize Prometheus metrics
        todo!("Initialize metrics")
    }
}
```

## Phase 3: Integration Points

### 3.1 Passthrough Handler Update

```rust
// In crates/lunaroute-ingress/src/anthropic.rs

pub async fn messages_passthrough(
    State(state): State<Arc<PassthroughState>>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Result<Response, IngressError> {
    let start_time = Instant::now();
    let session_id = state.session_recorder
        .as_ref()
        .map(|r| r.generate_session_id())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Extract metadata
    let model_requested = req.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let client_ip = headers.get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let user_agent = headers.get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Start recording
    if let Some(recorder) = &state.session_recorder {
        recorder.record(SessionEvent::Started {
            session_id: session_id.clone(),
            timestamp: Utc::now(),
            model_requested: model_requested.clone(),
            provider: "anthropic".to_string(),
            listener: "anthropic".to_string(),
            client_ip,
            user_agent,
        });

        let request_text = extract_anthropic_user_text(&req);
        recorder.record(SessionEvent::RequestRecorded {
            session_id: session_id.clone(),
            timestamp: Utc::now(),
            request_text,
            request_json: req.clone(),
            estimated_input_tokens: estimate_tokens(&req),
        });
    }

    // Send to provider
    let provider_start = Instant::now();
    let response = match state.connector.send_passthrough(req, passthrough_headers).await {
        Ok(resp) => resp,
        Err(e) => {
            // Record error
            if let Some(recorder) = &state.session_recorder {
                recorder.record(SessionEvent::Error {
                    session_id: session_id.clone(),
                    timestamp: Utc::now(),
                    error_type: "provider_error".to_string(),
                    error_message: e.to_string(),
                    error_details: None,
                });

                recorder.record(SessionEvent::Completed {
                    session_id,
                    timestamp: Utc::now(),
                    success: false,
                    finish_reason: None,
                    total_duration_ms: start_time.elapsed().as_millis() as u64,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_thinking_tokens: 0,
                });
            }
            return Err(IngressError::ProviderError(e.to_string()));
        }
    };

    let provider_latency = provider_start.elapsed();

    // Extract response data
    let response_text = extract_anthropic_assistant_text(&response);
    let usage = response.get("usage");
    let input_tokens = extract_token_count(usage, "input_tokens");
    let output_tokens = extract_token_count(usage, "output_tokens");
    let thinking_tokens = extract_token_count(usage, "thinking_tokens");
    let model_used = response.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or(&model_requested)
        .to_string();

    let finish_reason = response.get("stop_reason")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Record response
    if let Some(recorder) = &state.session_recorder {
        recorder.record(SessionEvent::ResponseRecorded {
            session_id: session_id.clone(),
            timestamp: Utc::now(),
            response_text,
            response_json: response.clone(),
            input_tokens,
            output_tokens,
            thinking_tokens: if thinking_tokens > 0 { Some(thinking_tokens) } else { None },
            model_used,
            latency_ms: provider_latency.as_millis() as u64,
        });

        // Record tool calls
        for (tool_name, tool_input) in extract_tool_calls(&response) {
            recorder.record(SessionEvent::ToolCallRecorded {
                session_id: session_id.clone(),
                timestamp: Utc::now(),
                tool_name,
                tool_input: Some(tool_input),
                tool_output: None,
            });
        }

        // Complete session
        recorder.record(SessionEvent::Completed {
            session_id,
            timestamp: Utc::now(),
            success: true,
            finish_reason,
            total_duration_ms: start_time.elapsed().as_millis() as u64,
            total_input_tokens: input_tokens,
            total_output_tokens: output_tokens,
            total_thinking_tokens: thinking_tokens,
        });
    }

    Ok(Json(response).into_response())
}
```

## Phase 4: SQLite Migrations

### 4.1 Initial Schema (`migrations/001_initial.sql`)

```sql
-- Enable foreign keys
PRAGMA foreign_keys = ON;

-- Main sessions table
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    started_at TIMESTAMP NOT NULL,
    completed_at TIMESTAMP,

    -- Request details
    provider TEXT NOT NULL,
    listener TEXT NOT NULL,
    model_requested TEXT NOT NULL,
    model_used TEXT,

    -- Status
    success BOOLEAN,
    error_message TEXT,
    finish_reason TEXT,

    -- Timing
    total_duration_ms INTEGER,
    provider_latency_ms INTEGER,

    -- Tokens
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    thinking_tokens INTEGER DEFAULT 0,

    -- Content (first 1000 chars for quick preview)
    request_text TEXT,
    response_text TEXT,

    -- Client info
    client_ip TEXT,
    user_agent TEXT,

    -- Metadata
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Tool calls table
CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    call_count INTEGER DEFAULT 1,
    first_called_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_id, tool_name)
);

-- Stream events (for streaming sessions)
CREATE TABLE IF NOT EXISTS stream_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    event_index INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    content TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Session errors
CREATE TABLE IF NOT EXISTS session_errors (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
    error_type TEXT NOT NULL,
    error_message TEXT NOT NULL,
    error_details TEXT,
    occurred_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexes for performance
CREATE INDEX idx_sessions_started_at ON sessions(started_at DESC);
CREATE INDEX idx_sessions_provider ON sessions(provider, started_at DESC);
CREATE INDEX idx_sessions_model_used ON sessions(model_used, started_at DESC);
CREATE INDEX idx_sessions_success ON sessions(success, started_at DESC);
CREATE INDEX idx_sessions_client_ip ON sessions(client_ip);
CREATE INDEX idx_tool_calls_session ON tool_calls(session_id);
CREATE INDEX idx_tool_calls_name ON tool_calls(tool_name);
CREATE INDEX idx_stream_events_session ON stream_events(session_id, event_index);

-- Trigger to update updated_at
CREATE TRIGGER update_sessions_updated_at
AFTER UPDATE ON sessions
BEGIN
    UPDATE sessions SET updated_at = CURRENT_TIMESTAMP WHERE session_id = NEW.session_id;
END;
```

### 4.2 Daily Statistics (`migrations/002_daily_stats.sql`)

```sql
-- Daily aggregated statistics
CREATE TABLE IF NOT EXISTS daily_stats (
    date DATE PRIMARY KEY,

    -- Request counts
    total_requests INTEGER DEFAULT 0,
    successful_requests INTEGER DEFAULT 0,
    failed_requests INTEGER DEFAULT 0,

    -- Token usage
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_thinking_tokens INTEGER DEFAULT 0,

    -- Performance
    avg_latency_ms REAL,
    p50_latency_ms REAL,
    p95_latency_ms REAL,
    p99_latency_ms REAL,

    -- Models and providers
    unique_models TEXT,  -- JSON array
    unique_providers TEXT,  -- JSON array

    -- Top tools
    top_tools TEXT,  -- JSON object with tool counts

    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- View for hourly statistics
CREATE VIEW hourly_stats AS
SELECT
    strftime('%Y-%m-%d %H:00:00', started_at) as hour,
    provider,
    model_used,
    COUNT(*) as request_count,
    SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END) as success_count,
    AVG(provider_latency_ms) as avg_latency_ms,
    SUM(input_tokens) as total_input_tokens,
    SUM(output_tokens) as total_output_tokens,
    SUM(thinking_tokens) as total_thinking_tokens
FROM sessions
WHERE started_at > datetime('now', '-24 hours')
GROUP BY hour, provider, model_used
ORDER BY hour DESC;

-- Materialized view for model usage (refresh periodically)
CREATE TABLE model_usage_stats AS
SELECT
    model_used,
    COUNT(*) as total_requests,
    AVG(provider_latency_ms) as avg_latency_ms,
    SUM(input_tokens) as total_input_tokens,
    SUM(output_tokens) as total_output_tokens,
    SUM(thinking_tokens) as total_thinking_tokens,
    MIN(started_at) as first_seen,
    MAX(started_at) as last_seen
FROM sessions
WHERE model_used IS NOT NULL
GROUP BY model_used;

CREATE INDEX idx_model_usage_requests ON model_usage_stats(total_requests DESC);
CREATE INDEX idx_model_usage_last_seen ON model_usage_stats(last_seen DESC);
```

## Testing Strategy

### Unit Tests
- Event serialization/deserialization
- Text extraction from various formats
- Buffer management and batching
- Writer interface compliance

### Integration Tests
- Full recording flow with mock providers
- SQLite database operations
- JSONL file creation and rotation
- Concurrent write stress testing

### Performance Tests
- Measure overhead with recording enabled/disabled
- Queue depth under various loads
- Write throughput benchmarking
- Memory usage profiling

## Deployment Plan

1. **Week 1**: Core infrastructure (traits, events, multi-writer)
2. **Week 2**: JSONL writer with compression and rotation
3. **Week 3**: SQLite writer with migrations and queries
4. **Week 4**: Integration and testing
5. **Week 5**: Performance optimization and monitoring
6. **Week 6**: Documentation and rollout

## Benefits Summary

- **Zero request blocking** through channel-based async design
- **Flexible storage** with JSONL for full data and SQLite for queries
- **Production ready** with metrics, error handling, and graceful shutdown
- **Maintainable** with clear separation of concerns and trait-based design
- **Scalable** with batching, buffering, and connection pooling
- **Observable** with comprehensive metrics and logging