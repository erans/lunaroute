# lunaroute-session

**Session recording and management for LLM applications with PII protection**

Records, stores, searches, and replays LLM interactions with automatic PII redaction, compression, and retention policies.

## Features

### Session Recording

- **JSONL-based storage** - One event per line for easy streaming and parsing
- **SQLite metadata** - Fast session lookup and querying
- **Request/Response capture** - Complete conversation history
- **Streaming support** - Record real-time streaming responses
- **Error tracking** - Capture and store error states

### Production-Ready Performance

- **File Handle Caching** - LRU cache (default: 100 handles) for efficient file reuse
- **Buffered Writes** - Configurable write buffers (default: 64KB) for reduced syscalls
- **Batch Writing** - Single write operation per session for batch operations
- **Race-Free Concurrency** - Pending operation tracking prevents file handle conflicts
- **10-100x faster** for high-volume streaming sessions

### Custom Storage Backends

- **SessionWriter trait** - Implement your own storage backends (S3, CloudWatch, GCS, etc.)
- **Multi-writer support** - Write to multiple backends simultaneously
- **Pluggable architecture** - Add custom writers without modifying core code
- **Public API** - Full access to SessionWriter, RecorderConfig, and SessionEvent types

### Production-Ready Security

- **AES-256-GCM Encryption at Rest** - Optional encryption for stored sessions
- **Argon2id Key Derivation** - Secure password-based key generation
- **Crypto-Secure Session IDs** - 128-bit entropy from OsRng (not UUID v4)
- **Persistent Salt Management** - Salt stored in `.encryption_salt` for consistency
- **Multi-Layer Session ID Validation** - Defense-in-depth validation at ingress, storage, and database layers
- **Secure Path Expansion** - Cross-platform tilde expansion with canonicalization and boundary checks

### Production-Ready Observability

- **Storage Metrics** - Track events written, bytes, cache performance, errors
- **Health Checks** - Verify storage accessibility and writability
- **Atomic Counters** - Thread-safe metric updates with minimal overhead

### PII Protection

Integrated with `lunaroute-pii` for automatic sensitive data protection:

- **Automatic redaction** before storage
- **Request-time redaction** - PII removed before writing
- **Response-time redaction** - AI responses sanitized
- **Stream-aware** - Redacts streaming chunks in real-time
- **JSON structure preservation** - Maintains valid JSON in tool calls
- **Configurable detection** - Enable/disable PII types per deployment

#### Supported PII Types

- Email addresses
- Phone numbers
- Social Security Numbers (SSN)
- Credit card numbers
- IP addresses
- Custom patterns (API keys, tokens, etc.)

#### Redaction Modes

1. **Mask** - Replace with `[EMAIL]`, `[PHONE]`, etc.
2. **Remove** - Delete PII completely
3. **Tokenize** - HMAC-based deterministic tokens
4. **Partial** - Show last N characters

### Storage Management

- **Automatic compression** - Gzip after configurable age
- **Retention policies** - Age-based and size-based cleanup
- **Disk usage monitoring** - Track storage consumption
- **Background cleanup** - Non-blocking maintenance tasks
- **Path traversal protection** - Secure file operations

### Session Search & Filter

- **Full-text search** across session content
- **Time range filtering** with timezone support
- **Status filtering** (success, error, pending)
- **Model/Provider filtering**
- **Token range filtering**
- **Pagination** with configurable page sizes
- **Complexity-based limits** - Progressive page size constraints

## Configuration

### Basic Configuration

```yaml
session_recording:
  enabled: true

  worker:
    batch_size: 100
    flush_interval_secs: 5
    channel_capacity: 10000

  jsonl:
    base_path: "./sessions"
    compression: true
    # Performance tuning
    cache_size: 100        # LRU cache size (file handles)
    buffer_size: 65536     # Write buffer size in bytes (64KB)
```

### Encryption Configuration

```yaml
session_recording:
  jsonl:
    # Enable encryption at rest
    encryption_password: "your-secure-password"
    # Optional: provide explicit salt (otherwise auto-generated and persisted)
    # encryption_salt: "base64-encoded-16-bytes"
```

**Important Security Notes:**
- Salt is automatically persisted in `{base_path}/.encryption_salt`
- Changing the password or salt makes existing sessions unreadable
- Use a strong password (recommended: 32+ random characters)
- Consider using environment variables for passwords in production

### PII Configuration

```yaml
session_recording:
  pii:
    enabled: true
    detect_email: true
    detect_phone: true
    detect_ssn: true
    detect_credit_card: true
    detect_ip_address: true
    min_confidence: 0.7
    redaction_mode: "mask"  # or "remove", "tokenize", "partial"
    partial_show_chars: 4
    hmac_secret: "your-secret-key"  # Required for "tokenize" mode

    custom_patterns:
      - name: "api_key"
        pattern: "sk-[a-zA-Z0-9]{32}"
        confidence: 0.95
        redaction_mode: "mask"
        placeholder: "[API_KEY]"
```

### Retention Policies

```yaml
session_recording:
  retention:
    max_age_days: 90              # Delete sessions older than 90 days
    max_size_mb: 10240            # Keep total storage under 10GB
    compress_after_days: 7        # Compress sessions after 7 days
    cleanup_interval_hours: 24    # Run cleanup daily
```

## Usage

### Basic Session Recording

```rust
use lunaroute_session::{SessionRecorder, SessionMetadata};
use lunaroute_core::normalized::*;

// Create recorder
let config = SessionRecordingConfig::default();
let recorder = SessionRecorder::new(config).await?;

// Create session
let metadata = SessionMetadata::builder()
    .session_id("session-123".to_string())
    .model("gpt-4".to_string())
    .provider("openai".to_string())
    .build();

recorder.create_session(&metadata).await?;

// Record request
let request = NormalizedRequest {
    messages: vec![/* ... */],
    model: "gpt-4".to_string(),
    // ...
};

recorder.record_request("session-123", &request).await?;

// Record response
let response = NormalizedResponse {
    id: "resp-123".to_string(),
    // ...
};

recorder.record_response("session-123", &response).await?;

// Complete session
recorder.complete_session("session-123").await?;
```

### PII-Protected Recording

```rust
use lunaroute_session::{SessionPIIRedactor, PIIConfig};

// Configure PII redaction
let pii_config = PIIConfig {
    enabled: true,
    detect_email: true,
    detect_phone: true,
    detect_ssn: true,
    detect_credit_card: true,
    detect_ip_address: true,
    min_confidence: 0.7,
    redaction_mode: "tokenize".to_string(),
    hmac_secret: Some("my-secret-key".to_string()),
    partial_show_chars: 4,
    custom_patterns: vec![],
};

let redactor = SessionPIIRedactor::from_config(&pii_config)?;

// Redact request before recording
let mut request = NormalizedRequest {
    messages: vec![
        Message {
            role: Role::User,
            content: MessageContent::Text(
                "My email is john@example.com".to_string()
            ),
            // ...
        }
    ],
    // ...
};

redactor.redact_request(&mut request);

// PII is now redacted: "My email is [EM:3kF9sL2p1Q7vN8h]"
recorder.record_request("session-123", &request).await?;
```

### JSON Structure Preservation

The PII redactor preserves JSON structure in tool call arguments:

```rust
// Original tool call arguments
let args = r#"{"email":"user@example.com","phone":"555-123-4567"}"#;

// After redaction - still valid JSON
let redacted = r#"{"email":"[EMAIL]","phone":"[PHONE]"}"#;

// Parse redacted JSON without errors
let parsed: serde_json::Value = serde_json::from_str(&redacted)?;
```

### Session Search

```rust
use lunaroute_session::{SessionFilter, SessionQuery};

let filter = SessionFilter::builder()
    .time_range_start(start_time)
    .time_range_end(end_time)
    .text_search("error".to_string())
    .models(vec!["gpt-4".to_string()])
    .min_tokens(100)
    .max_tokens(5000)
    .page_size(50)
    .build()?;

let query = SessionQuery::new(filter);
let results = recorder.search_sessions(&query).await?;

for session in results.sessions {
    println!("Session {}: {} tokens", session.session_id, session.total_tokens);
}
```

### Background Cleanup

```rust
use lunaroute_session::cleanup::run_background_cleanup;

// Start background cleanup task
let cleanup_handle = tokio::spawn(async move {
    run_background_cleanup(config.retention, jsonl_writer).await;
});
```

### Custom Storage Writers

Implement your own storage backend by implementing the `SessionWriter` trait:

```rust
use lunaroute_session_sqlite::{
    SqliteSessionStore, SessionWriter, SessionEvent,
    RecorderConfig, WriterResult
};
use lunaroute_session::sqlite_writer::SqliteWriter;
use async_trait::async_trait;
use std::sync::Arc;
use std::path::Path;

// Example: Custom S3 writer for raw event storage
struct S3SessionWriter {
    bucket: String,
    s3_client: S3Client,
}

#[async_trait]
impl SessionWriter for S3SessionWriter {
    async fn write_event(&self, event: &SessionEvent) -> WriterResult<()> {
        // Serialize event to JSON
        let json = serde_json::to_string(event)?;

        // Upload to S3 bucket
        let key = format!(
            "sessions/{}/events/{}.json",
            event.session_id,
            event.timestamp
        );

        self.s3_client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(json.into_bytes().into())
            .send()
            .await?;

        Ok(())
    }

    async fn write_batch(&self, events: &[SessionEvent]) -> WriterResult<()> {
        // Optional: Implement batch upload for better performance
        for event in events {
            self.write_event(event).await?;
        }
        Ok(())
    }

    fn supports_batching(&self) -> bool {
        true
    }
}

// Create session store with SQLite (for queries) + S3 (for raw events)
async fn create_hybrid_store() -> Result<SqliteSessionStore, Box<dyn std::error::Error>> {
    // Create SQLite writer for fast queries and stats
    let sqlite = SqliteWriter::new(Path::new("~/.lunaroute/sessions.db")).await?;

    // Create custom S3 writer for durable raw event storage
    let s3_writer = Arc::new(S3SessionWriter {
        bucket: "my-lunaroute-sessions".to_string(),
        s3_client: S3Client::new(),
    });

    // Combine both writers
    let store = SqliteSessionStore::with_writers(
        Some(Arc::new(sqlite)),  // SQLite for queries
        vec![s3_writer],          // S3 for raw events
        RecorderConfig {
            batch_size: 100,
            batch_timeout_ms: 100,
            channel_buffer_size: 10_000,
        },
    )?;

    Ok(store)
}
```

**Benefits of Custom Writers:**
- **Hybrid storage** - SQLite for queries, S3/CloudWatch for long-term storage
- **Cost optimization** - Keep hot data in SQLite, archive to cheaper object storage
- **Compliance** - Send audit logs to immutable storage (S3 with versioning)
- **Observability** - Send events to CloudWatch/Datadog for real-time monitoring
- **Multi-region** - Replicate sessions across regions for disaster recovery

**Available Types:**
- `SessionWriter` - Trait to implement for custom storage
- `SessionEvent` - Event data structure
- `RecorderConfig` - Batch and timeout configuration
- `WriterResult<T>` - Standard result type for writer operations
- `WriterError` - Error type for writer failures

### Production Features

#### Encryption at Rest

```rust
use lunaroute_session::jsonl_writer::{JsonlWriter, JsonlConfig};
use std::path::PathBuf;

// Configure with encryption
let config = JsonlConfig {
    cache_size: 100,
    buffer_size: 64 * 1024,
    encryption_password: Some("my-secure-password".to_string()),
    encryption_salt: None, // Auto-generated and persisted
};

let writer = JsonlWriter::with_config(
    PathBuf::from("./sessions"),
    config
);

// Sessions are now encrypted at rest with AES-256-GCM
// Salt is stored in ./sessions/.encryption_salt
```

#### Storage Metrics

```rust
use lunaroute_session::jsonl_writer::JsonlWriter;

let writer = JsonlWriter::new(PathBuf::from("./sessions"));

// ... write some events ...

// Get metrics snapshot
let metrics = writer.metrics();
println!("Events written: {}", metrics.events_written);
println!("Bytes written: {}", metrics.bytes_written);
println!("Cache hits: {}", metrics.cache_hits);
println!("Cache misses: {}", metrics.cache_misses);
println!("Cache evictions: {}", metrics.cache_evictions);
println!("Write errors: {}", metrics.write_errors);
```

#### Health Checks

```rust
use lunaroute_session::jsonl_writer::JsonlWriter;

let writer = JsonlWriter::new(PathBuf::from("./sessions"));

// Check storage health
let health = writer.health_check().await;

if health.healthy {
    println!("Storage is healthy");
} else {
    eprintln!("Storage unhealthy: {}", health.error.unwrap());
    // Take action: alert, failover, etc.
}
```

#### Important: Always Flush Before Dropping

```rust
use lunaroute_session::jsonl_writer::JsonlWriter;

let writer = JsonlWriter::new(PathBuf::from("./sessions"));

// ... write events ...

// CRITICAL: Always flush before dropping
writer.flush().await?;

// Now safe to drop the writer
drop(writer);
```

**Why this matters:**
- Buffered data is lost if not flushed
- No automatic flush on drop (unsafe in async Rust)
- Always call `flush()` explicitly before shutdown

## Security Features

### Encryption at Rest

**AES-256-GCM encryption** protects sessions on disk:

- Industry-standard authenticated encryption
- Argon2id key derivation from password (secure against GPU attacks)
- Persistent salt management (survives restarts)
- Optional per-deployment configuration
- Zero performance impact when disabled

### Session ID Security

**Crypto-secure session IDs** prevent attacks:

- 128-bit entropy from OsRng (not predictable UUID v4)
- 32 hex characters (filesystem-safe)
- Strict validation rejects path traversal attempts
- No sanitization (prevents ID collisions)
- Format: `[a-zA-Z0-9_-]{1,255}`

### Path Traversal Protection

**Strict validation** prevents malicious session IDs:

```rust
// REJECTED - Path traversal attempt
"../../../etc/passwd" → Error: "Invalid session ID: contains path traversal characters"

// REJECTED - Special characters
"test@#$%123" → Error: "Invalid session ID: contains unsafe characters"

// ACCEPTED - Safe format
"session-123-abc_def" → OK
```

### PII Detection Before Storage

All sensitive data is redacted before writing to disk:

1. **Request redaction** - User messages, tool arguments
2. **Response redaction** - AI messages, tool results
3. **Stream redaction** - Real-time chunk processing
4. **Metadata protection** - Session IDs and custom fields

### JSON-Aware Redaction

Prevents JSON corruption in tool calls:

```rust
// Original: {"email":"john@example.com","data":{"backup":"admin@example.com"}}
// Redacted: {"email":"[EMAIL]","data":{"backup":"[EMAIL]"}}
// Still valid JSON ✓
```

### HMAC-Based Tokenization

Secure, deterministic tokenization using HKDF:

```rust
let config = PIIConfig {
    redaction_mode: "tokenize".to_string(),
    hmac_secret: Some("my-secret-key".to_string()),
    // ...
};

// Same PII always produces same token
// "john@example.com" → "[EM:3kF9sL2p1Q7vN8h]"
```

### Overlapping Detection Handling

Automatically merges overlapping PII detections:

```rust
// Multiple patterns match "test@example.com"
// Keeps highest confidence detection
// Prevents text corruption from multiple redactions
```

## Storage Structure

```
sessions/
├── .encryption_salt          # Persistent salt for encryption (auto-generated)
├── 2025-01-15/
│   ├── session-123.jsonl     # Plaintext or encrypted session events
│   ├── session-123.jsonl.gz  # Compressed after retention policy
│   └── session-124.jsonl
├── 2025-01-16/
│   └── session-125.jsonl
└── sessions.db               # SQLite metadata (optional)
```

**Notes:**
- Session files organized by date (`YYYY-MM-DD` directories)
- Session IDs must be alphanumeric with hyphens/underscores only
- `.encryption_salt` file created automatically when encryption is enabled
- Compressed files retain original encryption (if enabled)

### JSONL Format

Each line is a JSON event:

```jsonl
{"event_type":"request","timestamp":"2025-01-15T10:30:00Z","data":{...}}
{"event_type":"response","timestamp":"2025-01-15T10:30:02Z","data":{...}}
{"event_type":"stream_chunk","timestamp":"2025-01-15T10:30:01Z","data":{...}}
{"event_type":"error","timestamp":"2025-01-15T10:30:03Z","data":{...}}
```

### SQLite Schema

```sql
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    status TEXT NOT NULL,
    total_tokens INTEGER,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    thinking_tokens INTEGER,
    request_count INTEGER,
    jsonl_path TEXT,
    metadata TEXT
);

CREATE INDEX idx_sessions_created_at ON sessions(created_at);
CREATE INDEX idx_sessions_status ON sessions(status);
CREATE INDEX idx_sessions_model ON sessions(model);
CREATE INDEX idx_sessions_provider ON sessions(provider);
```

## Testing

Run tests:
```bash
cargo test -p lunaroute-session
```

Run PII-specific tests:
```bash
cargo test -p lunaroute-session pii_redaction
```

## Dependencies

- `lunaroute-core` - Normalized types
- `lunaroute-storage` - Encryption utilities (AES-256-GCM, Argon2id)
- `lunaroute-pii` - PII detection and redaction
- `tokio` - Async runtime
- `serde` / `serde_json` - Serialization
- `lru` - LRU cache for file handles
- `rand` - Crypto-secure random number generation (OsRng)
- `hex` - Session ID encoding
- `zstd` - Compression
- `sqlx` - SQLite database (optional, feature-gated)

## License

See the main project LICENSE file.
