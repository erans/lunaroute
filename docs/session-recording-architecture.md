# Session Recording Architecture

## Overview

A high-performance, non-blocking session recording system that writes to multiple destinations asynchronously without impacting request latency.

## Core Design Principles

1. **Zero Request Blocking**: All recording operations happen in background tasks
2. **Multiple Write Destinations**: Extensible system supporting JSONL files and SQLite
3. **Eventual Consistency**: Prioritize request performance over immediate persistence
4. **Graceful Degradation**: Recording failures don't affect core proxy functionality
5. **Efficient Resource Usage**: Batched writes, connection pooling, and buffering

## Architecture

```
┌─────────────────┐
│  Request Flow   │
│  (Passthrough)  │
└────────┬────────┘
         │
         ▼
┌─────────────────┐      ┌──────────────────┐
│  Record Event   │─────►│  Channel (MPSC)  │
│  (Non-blocking) │      └────────┬─────────┘
└─────────────────┘               │
                                  ▼
                      ┌───────────────────────┐
                      │  Background Worker    │
                      │  (Dedicated Thread)   │
                      └───────────┬───────────┘
                                  │
                    ┌─────────────┴─────────────┐
                    ▼                           ▼
         ┌──────────────────┐       ┌──────────────────┐
         │   JSONL Writer   │       │  SQLite Writer   │
         │  (Full Details)  │       │   (Metadata)     │
         └──────────────────┘       └──────────────────┘
```

## Components

### 1. Session Recorder Trait

```rust
#[async_trait]
pub trait SessionRecorder: Send + Sync {
    /// Fire-and-forget recording - returns immediately
    fn record_event(&self, event: SessionEvent);

    /// Graceful shutdown - waits for pending writes
    async fn flush(&self) -> Result<()>;

    /// Get session by ID (queries all backends)
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;
}
```

### 2. Session Events with Statistics

```rust
pub enum SessionEvent {
    Started {
        session_id: String,
        request_id: String,     // Request ID for correlation
        timestamp: DateTime<Utc>,
        model_requested: String,
        provider: String,
        listener: String,
        is_streaming: bool,     // NEW: Indicates streaming request
        metadata: SessionMetadata,
    },

    StreamStarted {              // NEW: Streaming-specific event
        session_id: String,
        request_id: String,
        timestamp: DateTime<Utc>,
        time_to_first_token_ms: u64,  // TTFT - critical UX metric
    },

    RequestRecorded {
        session_id: String,
        request_id: String,     // Request ID for correlation
        timestamp: DateTime<Utc>,
        request_text: String,  // Extracted user message
        request_json: Value,   // Full request
        estimated_tokens: u32,
        // Timing stats
        stats: RequestStats,
    },

    ResponseRecorded {
        session_id: String,
        request_id: String,     // Request ID for correlation
        timestamp: DateTime<Utc>,
        response_text: String,  // Extracted assistant message
        response_json: Value,   // Full response
        model_used: String,     // Actual model from response
        // Detailed stats
        stats: ResponseStats,
    },

    StatsSnapshot {
        session_id: String,
        request_id: String,     // Request ID for correlation
        timestamp: DateTime<Utc>,
        // Periodic stats update during long requests
        stats: SessionStats,
    },

    Completed {
        session_id: String,
        request_id: String,     // Request ID for correlation
        timestamp: DateTime<Utc>,
        success: bool,
        error: Option<String>,
        finish_reason: Option<String>,
        // Final comprehensive stats
        final_stats: FinalSessionStats,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub api_version: Option<String>,
    pub request_headers: HashMap<String, String>,  // Selected headers
    pub session_tags: Vec<String>,  // Custom tags for filtering
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestStats {
    pub pre_processing_ms: f64,  // Time before provider call
    pub request_size_bytes: usize,
    pub message_count: usize,
    pub has_system_prompt: bool,
    pub has_tools: bool,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseStats {
    pub provider_latency_ms: u64,  // Provider response time
    pub post_processing_ms: f64,   // Time after provider response
    pub total_proxy_overhead_ms: f64,  // pre + post

    // Token breakdown
    pub tokens: TokenStats,

    // Tool usage
    pub tool_calls: Vec<ToolCallStats>,

    // Response characteristics
    pub response_size_bytes: usize,
    pub content_blocks: usize,
    pub has_refusal: bool,

    // Streaming metrics (NEW)
    pub is_streaming: bool,
    pub chunk_count: Option<u32>,
    pub streaming_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStats {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub thinking_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,

    // Calculated fields
    pub total_tokens: u32,
    pub thinking_percentage: Option<f32>,
    pub tokens_per_second: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallStats {
    pub tool_name: String,
    pub tool_call_id: Option<String>,
    pub execution_time_ms: Option<u64>,
    pub input_size_bytes: usize,
    pub output_size_bytes: Option<usize>,
    pub success: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub request_count: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_thinking_tokens: u64,
    pub total_tool_calls: u32,
    pub unique_tools: HashSet<String>,
    pub cumulative_latency_ms: u64,
    pub cumulative_proxy_overhead_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalSessionStats {
    pub total_duration_ms: u64,
    pub provider_time_ms: u64,      // Time spent waiting for provider
    pub proxy_overhead_ms: f64,     // Total overhead added by proxy

    // Token totals
    pub total_tokens: TokenTotals,

    // Tool usage summary
    pub tool_summary: ToolUsageSummary,

    // Performance metrics
    pub performance: PerformanceMetrics,

    // Streaming statistics (NEW - only present for streaming requests)
    pub streaming_stats: Option<StreamingStats>,

    // Cost estimation (optional)
    pub estimated_cost: Option<CostEstimate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub thinking: u64,
    pub cached: u64,
    pub total: u64,
    pub by_model: HashMap<String, TokenStats>,  // If multiple models used
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsageSummary {
    pub total_calls: u32,
    pub unique_tools: u32,
    pub by_tool: HashMap<String, ToolStats>,
    pub total_tool_time_ms: u64,
    pub tool_error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStats {
    pub call_count: u32,
    pub total_execution_time_ms: u64,
    pub avg_execution_time_ms: u64,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub avg_provider_latency_ms: f64,
    pub p50_latency_ms: Option<u64>,
    pub p95_latency_ms: Option<u64>,
    pub p99_latency_ms: Option<u64>,
    pub max_latency_ms: u64,
    pub min_latency_ms: u64,

    // Proxy overhead analysis
    pub avg_pre_processing_ms: f64,
    pub avg_post_processing_ms: f64,
    pub proxy_overhead_percentage: f32,  // Overhead as % of total time
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub provider: String,
    pub model: String,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub thinking_cost_usd: Option<f64>,
    pub total_cost_usd: f64,
    pub cost_per_1k_tokens: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingStats {
    pub time_to_first_token_ms: u64,     // TTFT - critical UX metric
    pub total_chunks: u32,                // Total number of SSE chunks
    pub streaming_duration_ms: u64,       // Total time from first to last chunk
    pub avg_chunk_latency_ms: f64,        // Average time between chunks
    pub p50_chunk_latency_ms: Option<u64>, // Median chunk latency
    pub p95_chunk_latency_ms: Option<u64>, // 95th percentile
    pub p99_chunk_latency_ms: Option<u64>, // 99th percentile
    pub max_chunk_latency_ms: u64,        // Maximum chunk latency
    pub min_chunk_latency_ms: u64,        // Minimum chunk latency
}
```

### 3. Multi-Writer Implementation

```rust
pub struct MultiWriterRecorder {
    tx: mpsc::Sender<SessionEvent>,
    worker_handle: Option<JoinHandle<()>>,
}

impl MultiWriterRecorder {
    pub fn new(writers: Vec<Box<dyn SessionWriter>>) -> Self {
        let (tx, rx) = mpsc::channel(10_000);  // Bounded channel prevents OOM

        let worker_handle = tokio::spawn(async move {
            let mut rx = rx;
            let mut buffer = Vec::new();
            let mut interval = tokio::time::interval(Duration::from_millis(100));

            loop {
                tokio::select! {
                    Some(event) = rx.recv() => {
                        buffer.push(event);

                        // Batch writes when buffer is large
                        if buffer.len() >= 100 {
                            Self::flush_buffer(&writers, &mut buffer).await;
                        }
                    }
                    _ = interval.tick() => {
                        // Periodic flush for low-traffic periods
                        if !buffer.is_empty() {
                            Self::flush_buffer(&writers, &mut buffer).await;
                        }
                    }
                    else => break,
                }
            }
        });

        Self { tx, worker_handle }
    }

    async fn flush_buffer(
        writers: &[Box<dyn SessionWriter>],
        buffer: &mut Vec<SessionEvent>,
    ) {
        if buffer.is_empty() {
            return;
        }

        let events = std::mem::take(buffer);

        // Write to all destinations in parallel
        let futures = writers
            .iter()
            .map(|writer| writer.write_batch(&events));

        let results = futures::future::join_all(futures).await;

        // Log any write failures but don't propagate
        for (i, result) in results.iter().enumerate() {
            if let Err(e) = result {
                tracing::error!(
                    writer = i,
                    error = %e,
                    "Failed to write session events"
                );
            }
        }
    }
}
```

### 4. JSONL Writer

```rust
pub struct JsonlWriter {
    sessions_dir: PathBuf,
    file_cache: Arc<RwLock<HashMap<String, File>>>,
    session_index: Arc<RwLock<HashMap<String, PathBuf>>>,
}

impl JsonlWriter {
    fn get_session_file(&self, session_id: &str) -> PathBuf {
        // Organize by date for easier management
        let today = Utc::now().format("%Y-%m-%d");
        self.sessions_dir
            .join(today.to_string())
            .join(format!("{}.jsonl", session_id))
    }

    async fn ensure_file(&self, session_id: &str) -> Result<File> {
        let path = self.get_session_file(session_id);

        // Create directory if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Open with append mode
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        // Update index
        self.session_index.write().await
            .insert(session_id.to_string(), path);

        Ok(file)
    }
}

#[async_trait]
impl SessionWriter for JsonlWriter {
    async fn write_batch(&self, events: &[SessionEvent]) -> Result<()> {
        // Group events by session for efficient file operations
        let mut by_session: HashMap<String, Vec<&SessionEvent>> = HashMap::new();

        for event in events {
            let session_id = event.session_id();
            by_session.entry(session_id.to_string())
                .or_default()
                .push(event);
        }

        // Write each session's events
        for (session_id, session_events) in by_session {
            let mut file = self.ensure_file(&session_id).await?;

            for event in session_events {
                let json = serde_json::to_string(event)?;
                file.write_all(json.as_bytes()).await?;
                file.write_all(b"\n").await?;
            }

            // Flush to ensure data is written
            file.flush().await?;
        }

        Ok(())
    }
}
```

### 5. SQLite Schema

```sql
-- Schema version tracking (MUST be first table)
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
);

-- Initialize with version 1
INSERT INTO schema_version (version) VALUES (1)
ON CONFLICT(version) DO NOTHING;

-- Core session metadata
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    request_id TEXT,            -- Request ID for correlation
    started_at TIMESTAMP NOT NULL,
    completed_at TIMESTAMP,
    provider TEXT NOT NULL,
    listener TEXT NOT NULL,
    model_requested TEXT NOT NULL,
    model_used TEXT,            -- Actual model from response
    success BOOLEAN,
    error_message TEXT,
    finish_reason TEXT,

    -- Timing
    total_duration_ms INTEGER,
    provider_latency_ms INTEGER,

    -- Token usage
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    thinking_tokens INTEGER DEFAULT 0,
    total_tokens INTEGER GENERATED ALWAYS AS (
        input_tokens + output_tokens + COALESCE(thinking_tokens, 0)
    ) STORED,

    -- Content summary
    request_text TEXT,      -- User's message text
    response_text TEXT,     -- Assistant's response text

    -- Metadata
    client_ip TEXT,
    user_agent TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,

    -- Indexes for common queries
    INDEX idx_sessions_created (created_at DESC),
    INDEX idx_sessions_provider (provider, created_at DESC),
    INDEX idx_sessions_model (model_used, created_at DESC),
    INDEX idx_sessions_success (success, created_at DESC),
    INDEX idx_sessions_request_id (request_id)
);

-- Tool usage tracking (with model_name for stats queries)
CREATE TABLE tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    request_id TEXT,            -- Request ID for correlation
    model_name TEXT,            -- Model used for this tool call
    tool_name TEXT NOT NULL,
    call_count INTEGER DEFAULT 1,
    avg_execution_time_ms INTEGER,
    error_count INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
    UNIQUE(session_id, tool_name),
    INDEX idx_tool_calls_model (model_name, created_at DESC)
);

-- Request/response pairs for streaming sessions
CREATE TABLE stream_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    request_id TEXT,            -- Request ID for correlation
    model_name TEXT,            -- Model used for this stream
    event_type TEXT NOT NULL,   -- 'delta', 'error', 'completion'
    event_index INTEGER NOT NULL,
    content TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
    INDEX idx_stream_events_session (session_id, event_index),
    INDEX idx_stream_events_model (model_name, created_at DESC)
);

-- Session statistics (comprehensive stats per session with model tracking)
CREATE TABLE session_stats (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    request_id TEXT,            -- Request ID for correlation
    model_name TEXT NOT NULL,   -- Model used (required for stats queries)

    -- Timing stats
    pre_processing_ms REAL,
    post_processing_ms REAL,
    proxy_overhead_ms REAL,

    -- Token stats
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    thinking_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,

    -- Performance stats
    tokens_per_second REAL,
    thinking_percentage REAL,

    -- Request/Response characteristics
    request_size_bytes INTEGER,
    response_size_bytes INTEGER,
    message_count INTEGER,
    content_blocks INTEGER,
    has_tools BOOLEAN DEFAULT 0,
    has_refusal BOOLEAN DEFAULT 0,

    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
    INDEX idx_session_stats_session (session_id),
    INDEX idx_session_stats_model (model_name, created_at DESC)
);

-- Aggregated statistics (updated periodically)
CREATE TABLE daily_stats (
    date DATE PRIMARY KEY,
    total_requests INTEGER DEFAULT 0,
    successful_requests INTEGER DEFAULT 0,
    failed_requests INTEGER DEFAULT 0,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_thinking_tokens INTEGER DEFAULT 0,
    avg_latency_ms REAL,
    unique_models TEXT,  -- JSON array of models used
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### 6. SQLite Writer

```rust
pub struct SqliteWriter {
    pool: SqlitePool,
}

impl SqliteWriter {
    pub async fn new(db_path: &Path) -> Result<Self> {
        // Create database if needed
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(db_path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)  // Better concurrency
                    .synchronous(SqliteSynchronous::Normal)
            )
            .await?;

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await?;

        // Verify schema version
        let version: i32 = sqlx::query_scalar("SELECT version FROM schema_version")
            .fetch_one(&pool)
            .await?;

        if version != 1 {
            return Err(anyhow::anyhow!("Unsupported schema version: {}", version));
        }

        Ok(Self { pool })
    }
}

#[async_trait]
impl SessionWriter for SqliteWriter {
    async fn write_batch(&self, events: &[SessionEvent]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for event in events {
            match event {
                SessionEvent::Started { session_id, timestamp, model_requested, provider, listener } => {
                    sqlx::query!(
                        "INSERT INTO sessions (session_id, started_at, model_requested, provider, listener)
                         VALUES (?, ?, ?, ?, ?)
                         ON CONFLICT(session_id) DO NOTHING",
                        session_id, timestamp, model_requested, provider, listener
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                SessionEvent::RequestRecorded { session_id, request_text, input_tokens, .. } => {
                    sqlx::query!(
                        "UPDATE sessions
                         SET request_text = ?, input_tokens = ?
                         WHERE session_id = ?",
                        request_text, input_tokens, session_id
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                SessionEvent::ResponseRecorded {
                    session_id,
                    response_text,
                    output_tokens,
                    thinking_tokens,
                    model_used,
                    latency_ms,
                    ..
                } => {
                    sqlx::query!(
                        "UPDATE sessions
                         SET response_text = ?,
                             output_tokens = ?,
                             thinking_tokens = ?,
                             model_used = ?,
                             provider_latency_ms = ?
                         WHERE session_id = ?",
                        response_text, output_tokens, thinking_tokens, model_used, latency_ms, session_id
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                SessionEvent::Completed { session_id, success, error, finish_reason, total_duration_ms } => {
                    sqlx::query!(
                        "UPDATE sessions
                         SET completed_at = CURRENT_TIMESTAMP,
                             success = ?,
                             error_message = ?,
                             finish_reason = ?,
                             total_duration_ms = ?
                         WHERE session_id = ?",
                        success, error, finish_reason, total_duration_ms, session_id
                    )
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        tx.commit().await?;
        Ok(())
    }
}
```

## Usage in Passthrough Handler

```rust
pub async fn messages_passthrough(
    State(state): State<Arc<PassthroughState>>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Result<Response, IngressError> {
    let start_time = Instant::now();
    let session_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();  // For request correlation

    // Extract key information before moving request
    let model_requested = req.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let request_text = extract_user_message(&req);

    let is_streaming = req.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);

    // Fire and forget - non-blocking
    if let Some(recorder) = &state.session_recorder {
        recorder.record_event(SessionEvent::Started {
            session_id: session_id.clone(),
            request_id: request_id.clone(),
            timestamp: Utc::now(),
            model_requested: model_requested.clone(),
            provider: "anthropic".to_string(),
            listener: "anthropic".to_string(),
            is_streaming,
            metadata: SessionMetadata::default(),
        });

        recorder.record_event(SessionEvent::RequestRecorded {
            session_id: session_id.clone(),
            request_id: request_id.clone(),
            request_text,
            request_json: req.clone(),
            input_tokens: 0, // Will be updated from response
        });
    }

    // Send to provider (unchanged)
    let response = state.connector
        .send_passthrough(req, passthrough_headers)
        .await?;

    // Extract response information
    let response_text = extract_assistant_message(&response);
    let usage = response.get("usage");
    let model_used = response.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or(&model_requested)
        .to_string();

    // Record response - non-blocking
    if let Some(recorder) = &state.session_recorder {
        recorder.record_event(SessionEvent::ResponseRecorded {
            session_id: session_id.clone(),
            request_id: request_id.clone(),
            response_text,
            response_json: response.clone(),
            output_tokens: extract_tokens(usage, "output_tokens"),
            thinking_tokens: extract_tokens(usage, "thinking_tokens").into(),
            model_used: model_used.clone(),
            latency_ms: start_time.elapsed().as_millis() as u64,
        });

        recorder.record_event(SessionEvent::Completed {
            session_id,
            request_id,
            success: true,
            error: None,
            finish_reason: extract_finish_reason(&response),
            total_duration_ms: start_time.elapsed().as_millis() as u64,
        });
    }

    Ok(Json(response).into_response())
}
```

### Streaming Passthrough Example

For streaming requests, record TTFT and comprehensive streaming statistics:

```rust
pub async fn messages_passthrough_streaming(
    State(state): State<Arc<PassthroughState>>,
    Json(req): Json<Value>,
) -> Result<Response, IngressError> {
    let start_time = Instant::now();
    let session_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

    // Record session start with is_streaming: true
    if let Some(recorder) = &state.session_recorder {
        recorder.record_event(SessionEvent::Started {
            session_id: session_id.clone(),
            request_id: request_id.clone(),
            timestamp: Utc::now(),
            model_requested: model.clone(),
            provider: "anthropic".to_string(),
            listener: "anthropic".to_string(),
            is_streaming: true,
            metadata: SessionMetadata::default(),
        });
    }

    // Streaming metrics tracking
    let ttft = Arc::new(Mutex::new(None));
    let chunk_latencies = Arc::new(Mutex::new(Vec::new()));
    let accumulated_text = Arc::new(Mutex::new(String::new()));

    // Stream SSE events with metrics extraction
    let stream = stream_with_metrics(
        connector.stream_passthrough(req).await?,
        ttft.clone(),
        chunk_latencies.clone(),
        accumulated_text.clone(),
    );

    // Add completion handler to record stats after stream ends
    let stream_with_completion = stream.chain(futures::stream::once(async move {
        if let (Some(recorder), Some(ttft_ms), Ok(latencies), Ok(text)) = (
            &state.session_recorder,
            *ttft.lock().unwrap(),
            chunk_latencies.lock().map(|l| l.clone()),
            accumulated_text.lock().map(|t| t.clone()),
        ) {
            // Calculate streaming statistics
            let streaming_stats = calculate_streaming_stats(ttft_ms, &latencies);

            // Record StreamStarted event
            recorder.record_event(SessionEvent::StreamStarted {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                timestamp: Utc::now(),
                time_to_first_token_ms: ttft_ms,
            });

            // Record Completed event with streaming_stats
            recorder.record_event(SessionEvent::Completed {
                session_id,
                request_id,
                timestamp: Utc::now(),
                success: true,
                error: None,
                finish_reason: Some("end_turn".to_string()),
                final_stats: FinalSessionStats {
                    total_duration_ms: start_time.elapsed().as_millis() as u64,
                    provider_time_ms: start_time.elapsed().as_millis() as u64,
                    proxy_overhead_ms: 0.0,
                    total_tokens: extract_token_totals(&text),
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats: Some(streaming_stats),
                    estimated_cost: None,
                },
            });
        }
        Ok(Bytes::new()) // Completion marker (filtered out before sending)
    }))
    .filter_map(|result| async move {
        match result {
            Ok(bytes) if bytes.is_empty() => None, // Filter out completion marker
            other => Some(other),
        }
    });

    Ok(Sse::new(stream_with_completion).into_response())
}

fn calculate_streaming_stats(ttft_ms: u64, latencies: &[u64]) -> StreamingStats {
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();

    let len = sorted.len();
    let p50 = if len > 0 { Some(sorted[(len.saturating_sub(1) * 50 / 100).min(len - 1)]) } else { None };
    let p95 = if len > 0 { Some(sorted[(len.saturating_sub(1) * 95 / 100).min(len - 1)]) } else { None };
    let p99 = if len > 0 { Some(sorted[(len.saturating_sub(1) * 99 / 100).min(len - 1)]) } else { None };

    StreamingStats {
        time_to_first_token_ms: ttft_ms,
        total_chunks: latencies.len() as u32,
        streaming_duration_ms: latencies.iter().sum(),
        avg_chunk_latency_ms: if !latencies.is_empty() {
            latencies.iter().sum::<u64>() as f64 / latencies.len() as f64
        } else { 0.0 },
        p50_chunk_latency_ms: p50,
        p95_chunk_latency_ms: p95,
        p99_chunk_latency_ms: p99,
        max_chunk_latency_ms: sorted.last().copied().unwrap_or(0),
        min_chunk_latency_ms: sorted.first().copied().unwrap_or(0),
    }
}
```

This example shows:
1. Recording `is_streaming: true` in Started event
2. Tracking TTFT and chunk latencies during streaming
3. Recording StreamStarted event with TTFT
4. Recording Completed event with comprehensive StreamingStats
5. Safe percentile calculation with bounds checking
6. Zero-copy passthrough while extracting metrics

### Streaming Production Safety Features

The streaming implementation includes multiple layers of protection against memory exhaustion and parsing errors:

**Memory Bounds**
```rust
// Constants defined in openai.rs and anthropic.rs
const MAX_CHUNK_LATENCIES: usize = 10_000;        // Cap latency array
const MAX_ACCUMULATED_TEXT_BYTES: usize = 1_000_000;  // Cap text buffer (1MB)

// Latency tracking with warning
if latencies.len() < MAX_CHUNK_LATENCIES {
    latencies.push(latency);
} else if latencies.len() == MAX_CHUNK_LATENCIES {
    tracing::warn!(
        "Chunk latency array reached maximum size ({} entries), dropping further measurements",
        MAX_CHUNK_LATENCIES
    );
}

// Text accumulation with warning
if accumulated.len() + content.len() <= MAX_ACCUMULATED_TEXT_BYTES {
    accumulated.push_str(content);
} else if accumulated.len() < MAX_ACCUMULATED_TEXT_BYTES {
    tracing::warn!(
        "Accumulated text reached maximum size ({} bytes), dropping further content",
        MAX_ACCUMULATED_TEXT_BYTES
    );
}
```

**SSE Parsing Optimization**
- Single parse per event (not double parsing)
- Graceful fallback if JSON parsing fails
- Raw data forwarded if parse errors occur
- Warnings logged without failing the stream

**Mutex Poisoning Handling**
```rust
// All mutex operations use graceful error handling
if let Ok(mut latencies) = latencies_clone.lock() {
    latencies.push(latency);
} else {
    tracing::error!("Streaming metrics: latency tracking mutex poisoned");
    // Stream continues despite metrics failure
}
```

**Benefits:**
- Prevents OOM from extremely long streaming sessions
- Early warning when sessions exceed expected bounds
- Non-blocking error handling preserves stream integrity
- Graceful degradation of metrics collection
- No panics in production code paths

## Configuration

```yaml
session_recording:
  enabled: true

  # JSONL file storage
  jsonl:
    enabled: true
    directory: "~/.lunaroute/sessions"
    compression: "zstd"  # none, gzip, zstd
    retention_days: 30

  # SQLite metadata storage
  sqlite:
    enabled: true
    path: "~/.lunaroute/sessions.db"

    # Connection pool settings
    max_connections: 5
    connection_timeout: 5s

    # Write batching
    batch_size: 100
    batch_timeout_ms: 100

  # Background worker settings
  worker:
    channel_buffer: 10000
    flush_interval_ms: 100
    shutdown_timeout: 30s
```

## Security & Resource Management

### Path Traversal Protection
Session IDs are sanitized before file operations:
```rust
fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(255)
        .collect()
}
```

Prevents attacks like:
- `"../../../etc/passwd"` → `"etcpasswd"`
- `"/absolute/path"` → `"absolutepath"`

### SQL Injection Prevention
All SQLite queries use parameterized bindings via SQLx:
```rust
sqlx::query("INSERT INTO sessions (session_id, model_used) VALUES (?, ?)")
    .bind(session_id)
    .bind(model_used)
    .execute(&pool)
    .await?;
```

### Bounded Resources
- **Channel buffer**: Limited to 10,000 events (prevents OOM)
- **Backpressure**: Events dropped with warning when buffer full
- **Database connections**: Pooled (default 5 connections)
- **No file handle leaks**: Files opened per-write, immediately closed

### Graceful Shutdown
```rust
impl MultiWriterRecorder {
    pub async fn shutdown(mut self) -> WriterResult<()> {
        // Worker will flush remaining events when tx is dropped
        if let Some(handle) = self.worker_handle.take() {
            handle.await?;
        }
        Ok(())
    }
}

impl Drop for MultiWriterRecorder {
    fn drop(&mut self) {
        if self.worker_handle.is_some() {
            tracing::warn!("Recorder dropped without shutdown()");
        }
    }
}
```

## Benefits

1. **Zero Request Latency Impact**: Recording happens entirely in background
2. **Dual Storage**: Full details in JSONL, queryable metadata in SQLite
3. **Efficient Queries**: SQL queries for analytics without parsing JSONL files
4. **Scalable**: Batched writes, connection pooling, and bounded buffers
5. **Resilient**: Recording failures don't affect proxy operation
6. **Secure**: Path traversal protection, SQL injection prevention, resource limits
7. **Observable**: Metrics for queue depth, write latency, and failures
8. **Maintainable**: Clear separation of concerns with trait-based design

## Query Examples

```sql
-- Sessions with high thinking token usage
SELECT session_id, model_used, thinking_tokens, response_text
FROM sessions
WHERE thinking_tokens > 10000
ORDER BY thinking_tokens DESC;

-- Daily usage by model
SELECT
    DATE(started_at) as date,
    model_used,
    COUNT(*) as requests,
    SUM(input_tokens) as total_input,
    SUM(output_tokens) as total_output,
    AVG(provider_latency_ms) as avg_latency_ms
FROM sessions
WHERE started_at > datetime('now', '-7 days')
GROUP BY DATE(started_at), model_used
ORDER BY date DESC, requests DESC;

-- Tool usage patterns
SELECT
    t.tool_name,
    COUNT(DISTINCT t.session_id) as unique_sessions,
    SUM(t.call_count) as total_calls
FROM tool_calls t
JOIN sessions s ON t.session_id = s.session_id
WHERE s.started_at > datetime('now', '-1 day')
GROUP BY t.tool_name
ORDER BY total_calls DESC;
```

## Migration Path

1. Deploy with SQLite writer disabled initially
2. Backfill SQLite from existing JSONL files
3. Enable SQLite writer for new sessions
4. Gradually migrate queries from JSONL to SQLite
5. Archive old JSONL files to cold storage