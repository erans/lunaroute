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

```toml
[session_recording]
enabled = true

[session_recording.worker]
batch_size = 100
flush_interval_secs = 5
channel_capacity = 10000

[session_recording.jsonl]
base_path = "./sessions"
compression = true
```

### PII Configuration

```toml
[session_recording.pii]
enabled = true
detect_email = true
detect_phone = true
detect_ssn = true
detect_credit_card = true
detect_ip_address = true
min_confidence = 0.7
redaction_mode = "mask"  # or "remove", "tokenize", "partial"
partial_show_chars = 4
hmac_secret = "your-secret-key"  # Required for "tokenize" mode

[[session_recording.pii.custom_patterns]]
name = "api_key"
pattern = "sk-[a-zA-Z0-9]{32}"
confidence = 0.95
redaction_mode = "mask"
placeholder = "[API_KEY]"
```

### Retention Policies

```toml
[session_recording.retention]
max_age_days = 90              # Delete sessions older than 90 days
max_size_mb = 10240            # Keep total storage under 10GB
compress_after_days = 7        # Compress sessions after 7 days
cleanup_interval_hours = 24    # Run cleanup daily
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

## Security Features

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
├── 2025-01-15/
│   ├── session-123.jsonl
│   ├── session-123.jsonl.gz
│   └── session-124.jsonl
├── 2025-01-16/
│   └── session-125.jsonl
└── sessions.db
```

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
- `lunaroute-pii` - PII detection and redaction
- `tokio` - Async runtime
- `serde` / `serde_json` - Serialization
- `flate2` - Gzip compression
- `rusqlite` - SQLite database

## License

See the main project LICENSE file.
