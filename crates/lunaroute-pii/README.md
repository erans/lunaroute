# lunaroute-pii

**Production-ready PII detection and redaction for LLM applications**

Secure, high-performance PII detection and redaction library with support for multiple redaction modes, custom patterns, and streaming scenarios.

## Features

### Detection

- **Built-in PII Types**
  - Email addresses (RFC 5322 compliant)
  - Phone numbers (multiple formats)
  - Social Security Numbers (SSN)
  - Credit card numbers (Visa, MC, Amex, Discover)
  - IP addresses (IPv4 and IPv6)

- **Custom Patterns**
  - Regular expression-based detection
  - Per-pattern confidence scoring
  - Individual redaction mode per pattern
  - JSON-based detection format (prevents colon-splitting vulnerabilities)

### Redaction Modes

1. **Mask** - Replace with placeholder text
   ```rust
   "Email: john@example.com" → "Email: [EMAIL]"
   ```

2. **Remove** - Delete PII entirely
   ```rust
   "Email: john@example.com" → "Email: "
   ```

3. **Tokenize** - HMAC-based deterministic tokens
   ```rust
   "Email: john@example.com" → "Email: [EM:3kF9sL2p1Q7vN8h]"
   ```

4. **Partial** - Show last N characters
   ```rust
   "Card: 4532-0151-1416-7894" → "Card: ***************7894"
   ```

## Security Features

### HKDF Key Derivation

Uses HKDF (HMAC-based Key Derivation Function) to derive secure HMAC keys from user secrets:

```rust
use lunaroute_pii::{RedactorConfig, RedactionMode, StandardRedactor};

let config = RedactorConfig {
    mode: RedactionMode::Tokenize,
    hmac_secret: Some("my-secret-key".to_string()),
    partial_show_chars: 4,
    type_overrides: Vec::new(),
};

let redactor = StandardRedactor::new(config);
```

**Key features:**
- Salt: `"lunaroute-pii-redaction-v1"`
- Info: `"hmac-tokenization-key"`
- Output: 32-byte key for HMAC-SHA256
- Deterministic: Same input produces same token
- Secure: Derived keys never stored in plaintext

### Overlapping Detection Handling

Automatically merges overlapping detections based on confidence scores:

```rust
// Input: "test@example.com" with overlapping email/phone detections
// Output: Single redaction using highest confidence detection
```

### Custom Pattern Security

Uses JSON serialization to prevent injection attacks:

```rust
// Secure format: {"name":"pattern_name","text":"actual_pii_value"}
// Handles colons in PII: "postgres://user:pass@host:5432/db"
```

## Usage

### Basic Detection and Redaction

```rust
use lunaroute_pii::{
    DetectorConfig, RegexPIIDetector, PIIDetector,
    RedactorConfig, StandardRedactor, PIIRedactor, RedactionMode,
};

// Configure detector
let detector_config = DetectorConfig {
    detect_email: true,
    detect_phone: true,
    detect_ssn: true,
    detect_credit_card: true,
    detect_ip_address: true,
    custom_patterns: Vec::new(),
    min_confidence: 0.7,
};

let detector = RegexPIIDetector::new(detector_config)?;

// Configure redactor
let redactor_config = RedactorConfig {
    mode: RedactionMode::Mask,
    partial_show_chars: 4,
    hmac_secret: None,
    type_overrides: Vec::new(),
};

let redactor = StandardRedactor::new(redactor_config);

// Detect and redact
let text = "Contact me at john@example.com or call 555-123-4567";
let detections = detector.detect(text);
let redacted = redactor.redact(text, &detections);

println!("{}", redacted);
// Output: "Contact me at [EMAIL] or call [PHONE]"
```

### Custom Patterns

```rust
use lunaroute_pii::{CustomPattern, CustomRedactionMode};

let custom_patterns = vec![
    CustomPattern {
        name: "api_key".to_string(),
        pattern: r"sk-[a-zA-Z0-9]{32}".to_string(),
        confidence: 0.95,
        redaction_mode: CustomRedactionMode::Tokenize,
        placeholder: Some("[API_KEY]".to_string()),
    },
];

let detector_config = DetectorConfig {
    detect_email: false,
    detect_phone: false,
    detect_ssn: false,
    detect_credit_card: false,
    detect_ip_address: false,
    custom_patterns,
    min_confidence: 0.7,
};
```

### Tokenization with HMAC

```rust
let redactor_config = RedactorConfig {
    mode: RedactionMode::Tokenize,
    hmac_secret: Some("my-secret-key".to_string()),
    partial_show_chars: 4,
    type_overrides: Vec::new(),
};

let redactor = StandardRedactor::new(redactor_config);

let text = "Email: john@example.com";
let detections = detector.detect(text);
let redacted = redactor.redact(text, &detections);

println!("{}", redacted);
// Output: "Email: [EM:3kF9sL2p1Q7vN8h]"

// Same email always produces same token (deterministic)
```

### Type-Specific Redaction

```rust
use lunaroute_pii::{TypeRedactionOverride, PIIType};

let redactor_config = RedactorConfig {
    mode: RedactionMode::Mask,
    partial_show_chars: 4,
    hmac_secret: None,
    type_overrides: vec![
        TypeRedactionOverride {
            pii_type: PIIType::Email,
            mode: RedactionMode::Remove,
            replacement: None,
        },
        TypeRedactionOverride {
            pii_type: PIIType::CreditCard,
            mode: RedactionMode::Partial,
            replacement: None,
        },
    ],
};
```

## Streaming Support

The detector works on text chunks, making it suitable for streaming scenarios:

```rust
// Process each chunk independently
for chunk in stream {
    let detections = detector.detect(&chunk);
    let redacted = redactor.redact(&chunk, &detections);
    output.write(redacted);
}
```

**Note:** Detections spanning chunk boundaries are not currently handled. Consider using a sliding window or buffering approach for production streaming scenarios.

## Performance

- **Regex-based detection** using `regex` and `aho-corasick` crates
- **Zero-copy operations** where possible
- **Sorted detections** by position for efficient processing
- **Overlapping detection merging** prevents redundant work

## Testing

Run tests:
```bash
cargo test -p lunaroute-pii
```

Run with coverage:
```bash
cargo tarpaulin --package lunaroute-pii --out Stdout
```

## Dependencies

- `regex` - Pattern matching
- `aho-corasick` - Efficient multi-pattern matching
- `hmac` / `sha2` - HMAC-SHA256 for tokenization
- `hkdf` - Secure key derivation
- `base64` - Token encoding
- `serde` / `serde_json` - Serialization
- `thiserror` - Error handling

## License

See the main project LICENSE file.
