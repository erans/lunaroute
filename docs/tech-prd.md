# LunaRoute Technical Specification

**Version:** 1.0.0  
**Date:** 2025-10-01  
**Status:** Implementation Ready

## 1. System Architecture

### 1.1 Core Design Principles

- **Zero-copy streaming**: Minimize allocations and copies in the hot path
- **Lock-free data structures**: Use crossbeam channels and dashmap for concurrent access
- **Memory pooling**: Pre-allocate buffers for request/response handling
- **Async-first**: Tokio runtime with careful tuning for low latency
- **Circuit breaking**: Fail fast with exponential backoff
- **Backpressure-aware**: Respect downstream capacity
- **File-based storage**: Simple file-based configuration and session storage for MVP

### 1.2 High-Level Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                         Ingress Layer                        │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ OpenAI       │  │ Anthropic    │  │ Health/      │      │
│  │ Listener     │  │ Listener     │  │ Metrics      │      │
│  │ :8080        │  │ :8081        │  │ :9090        │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│                    Normalization Pipeline                     │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Request Parser → Validator → Normalizer → PII      │    │
│  │                                           Redactor   │    │
│  └─────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│                      Routing Engine                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ Rule         │  │ Health       │  │ Budget       │      │
│  │ Matcher      │  │ Monitor      │  │ Enforcer     │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│                       Egress Layer                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ OpenAI       │  │ Anthropic    │  │ Future       │      │
│  │ Connector    │  │ Connector    │  │ Providers    │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│                    Observability Layer                       │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ Session      │  │ Metrics      │  │ Tracing      │      │
│  │ Recorder     │  │ Collector    │  │ (OTLP)       │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└──────────────────────────────────────────────────────────────┘
```

## 2. Component Specifications

### 2.1 Ingress Adapters

```rust
// crates/lunaroute-ingress/src/lib.rs

use axum::{
    body::StreamBody,
    extract::{State, Path, Query, TypedHeader},
    response::Response,
};
use hyper::StatusCode;
use tokio::sync::mpsc;
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;

pub struct IngressConfig {
    pub bind_addr: SocketAddr,
    pub protocol: IngressProtocol,
    pub max_body_size: usize,           // 10MB default
    pub stream_buffer_size: usize,       // 64KB default
    pub header_timeout_ms: u64,          // 5000ms default
    pub keepalive_interval_ms: u64,      // 30000ms default
}

pub enum IngressProtocol {
    OpenAI,
    Anthropic,
}

pub struct IngressListener {
    config: Arc<IngressConfig>,
    normalizer: Arc<Normalizer>,
    auth: Arc<AuthService>,
    metrics: Arc<MetricsCollector>,
}

impl IngressListener {
    pub async fn start(self) -> Result<()> {
        let app = Router::new()
            .route("/v1/chat/completions", post(self.handle_openai_chat))
            .route("/v1/messages", post(self.handle_anthropic_messages))
            .layer(
                ServiceBuilder::new()
                    .layer(TimeoutLayer::new(Duration::from_millis(
                        self.config.header_timeout_ms
                    )))
                    .layer(RequestBodyLimitLayer::new(self.config.max_body_size))
                    .layer(CompressionLayer::new())
                    .layer(TraceLayer::new_for_http())
            );
            
        axum::Server::bind(&self.config.bind_addr)
            .http2_keep_alive_interval(Some(Duration::from_millis(
                self.config.keepalive_interval_ms
            )))
            .http2_keep_alive_timeout(Duration::from_secs(20))
            .serve(app.into_make_service())
            .await?;
            
        Ok(())
    }
    
    async fn handle_openai_chat(&self, req: OpenAIRequest) -> Response {
        // Request parsing with zero-copy where possible
        let normalized = self.normalizer.normalize_openai(req)?;
        self.process_normalized(normalized).await
    }
}

// Streaming handler with backpressure
pub struct StreamHandler {
    rx: mpsc::Receiver<StreamEvent>,
    tx: mpsc::Sender<StreamEvent>,
    buffer: BytesMut,
    flush_interval: Duration,
}

impl StreamHandler {
    pub async fn pump_stream(&mut self) -> Result<()> {
        let mut flush_timer = tokio::time::interval(self.flush_interval);
        
        loop {
            tokio::select! {
                biased;
                
                Some(event) = self.rx.recv() => {
                    self.buffer.extend_from_slice(&event.to_bytes());
                    
                    if self.buffer.len() >= CHUNK_THRESHOLD {
                        self.flush().await?;
                    }
                }
                
                _ = flush_timer.tick() => {
                    if !self.buffer.is_empty() {
                        self.flush().await?;
                    }
                }
            }
        }
    }
}
```

### 2.2 Normalization Layer

```rust
// crates/lunaroute-core/src/normalize.rs

use bytes::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct NormalizedRequest {
    pub id: RequestId,
    pub messages: Vec<Message>,
    pub model: String,
    pub parameters: Parameters,
    pub metadata: RequestMetadata,
    pub stream: bool,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Clone, Debug)]
pub enum MessageContent {
    Text(String),
    MultiModal(Vec<ContentBlock>),
}

#[derive(Clone, Debug)]
pub struct Parameters {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<i32>,
    pub max_tokens: Option<i32>,
    pub stop_sequences: Vec<String>,
    pub presence_penalty: Option<f32>,
    pub frequency_penalty: Option<f32>,
}

pub struct Normalizer {
    validators: Arc<ValidatorChain>,
    pii_redactor: Arc<PIIRedactor>,
}

impl Normalizer {
    pub fn normalize_openai(&self, req: OpenAIRequest) -> Result<NormalizedRequest> {
        // Use SIMD for string validation where possible
        self.validators.validate_openai(&req)?;
        
        let mut normalized = NormalizedRequest {
            id: RequestId::new(),
            messages: self.convert_openai_messages(req.messages)?,
            model: req.model,
            parameters: self.extract_openai_params(req)?,
            metadata: self.build_metadata(&req),
            stream: req.stream.unwrap_or(false),
        };
        
        // Apply PII redaction
        if self.pii_redactor.is_enabled() {
            normalized = self.pii_redactor.redact_request(normalized)?;
        }
        
        Ok(normalized)
    }
    
    fn convert_openai_messages(&self, messages: Vec<OpenAIMessage>) -> Result<Vec<Message>> {
        // Preallocate capacity
        let mut result = Vec::with_capacity(messages.len());
        
        for msg in messages {
            result.push(Message {
                role: self.map_role(msg.role)?,
                content: self.convert_content(msg.content)?,
            });
        }
        
        Ok(result)
    }
}

// Stream normalization with minimal allocations
pub enum NormalizedStreamEvent {
    Start { id: String, model: String },
    Delta { content: String, index: usize },
    ToolCall { name: String, arguments: String },
    Usage { prompt_tokens: u32, completion_tokens: u32 },
    End { finish_reason: Option<String> },
    Error { code: u16, message: String },
}

impl NormalizedStreamEvent {
    pub fn from_sse_line(line: &[u8], format: StreamFormat) -> Result<Self> {
        // Zero-copy parsing where possible
        match format {
            StreamFormat::OpenAI => Self::parse_openai_sse(line),
            StreamFormat::Anthropic => Self::parse_anthropic_event(line),
        }
    }
}
```

### 2.3 Routing Engine

```rust
// crates/lunaroute-routing/src/engine.rs

use crossbeam::channel::{bounded, Sender, Receiver};
use dashmap::DashMap;
use parking_lot::RwLock;

pub struct RoutingEngine {
    rules: Arc<RwLock<RouteTable>>,
    health_monitor: Arc<HealthMonitor>,
    budget_enforcer: Arc<BudgetEnforcer>,
    circuit_breakers: DashMap<ProviderId, CircuitBreaker>,
}

pub struct RouteTable {
    rules: Vec<RoutingRule>,
    compiled_matchers: Vec<CompiledMatcher>,
}

impl RoutingEngine {
    pub async fn route(&self, req: &NormalizedRequest) -> Result<RouteDecision> {
        let start = Instant::now();
        
        // Fast path: check cache
        if let Some(cached) = self.check_route_cache(&req.id) {
            return Ok(cached);
        }
        
        // Evaluate rules in priority order
        let rules = self.rules.read();
        for (rule, matcher) in rules.rules.iter().zip(&rules.compiled_matchers) {
            if matcher.matches(req) {
                let decision = self.evaluate_rule(rule, req).await?;
                
                // Check health and budgets
                if self.validate_decision(&decision, req).await? {
                    self.cache_decision(&req.id, &decision);
                    
                    metrics::histogram!("routing.decision.latency_us")
                        .record(start.elapsed().as_micros() as f64);
                    
                    return Ok(decision);
                }
            }
        }
        
        Err(RoutingError::NoMatchingRoute)
    }
    
    async fn evaluate_rule(&self, rule: &RoutingRule, req: &NormalizedRequest) -> Result<RouteDecision> {
        let primary = TargetProvider {
            provider: rule.target.provider.clone(),
            model: rule.target.model.clone(),
            endpoint: self.resolve_endpoint(&rule.target.provider)?,
        };
        
        // Build fallback chain with health checks
        let mut fallbacks = Vec::new();
        for fallback in &rule.fallbacks {
            if self.health_monitor.is_healthy(&fallback.provider).await {
                fallbacks.push(TargetProvider {
                    provider: fallback.provider.clone(),
                    model: fallback.model.clone(),
                    endpoint: self.resolve_endpoint(&fallback.provider)?,
                });
            }
        }
        
        Ok(RouteDecision {
            primary,
            fallbacks,
            sticky_key: self.compute_sticky_key(req),
            metadata: rule.metadata.clone(),
        })
    }
}

// Circuit breaker with token bucket
pub struct CircuitBreaker {
    state: AtomicU8, // 0=closed, 1=open, 2=half-open
    failures: AtomicU32,
    success: AtomicU32,
    last_failure: AtomicU64,
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    pub fn check(&self) -> CircuitState {
        match self.state.load(Ordering::Acquire) {
            0 => CircuitState::Closed,
            1 => {
                let elapsed = self.elapsed_since_failure();
                if elapsed > self.config.reset_timeout {
                    self.state.store(2, Ordering::Release);
                    CircuitState::HalfOpen
                } else {
                    CircuitState::Open
                }
            }
            2 => CircuitState::HalfOpen,
            _ => unreachable!(),
        }
    }
    
    pub fn record_success(&self) {
        self.success.fetch_add(1, Ordering::Relaxed);
        
        if self.state.load(Ordering::Acquire) == 2 {
            if self.success.load(Ordering::Relaxed) >= self.config.half_open_requests {
                self.reset();
            }
        }
    }
    
    pub fn record_failure(&self) {
        let failures = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_failure.store(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            Ordering::Release
        );
        
        if failures >= self.config.failure_threshold {
            self.state.store(1, Ordering::Release);
        }
    }
}
```

### 2.4 Egress Connectors

```rust
// crates/lunaroute-egress/src/provider.rs

use hyper::{Body, Client, Request};
use hyper_rustls::HttpsConnectorBuilder;
use rustls::ClientConfig;
use tokio::time::timeout;

#[async_trait]
pub trait Provider: Send + Sync {
    async fn send(&self, req: NormalizedRequest) -> Result<NormalizedResponse>;
    async fn stream(&self, req: NormalizedRequest) -> Result<StreamHandle>;
    fn capabilities(&self) -> &ProviderCapabilities;
}

pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub max_context_length: usize,
    pub rate_limits: RateLimitConfig,
}

pub struct OpenAIConnector {
    client: Client<HttpsConnector<HttpConnector>>,
    config: OpenAIConfig,
    rate_limiter: Arc<RateLimiter>,
    connection_pool: Arc<ConnectionPool>,
}

impl OpenAIConnector {
    pub fn new(config: OpenAIConfig) -> Result<Self> {
        // Configure HTTP/2 with optimized settings
        let https = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http2()
            .build();
        
        let client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(100)
            .http2_keep_alive_interval(Some(Duration::from_secs(10)))
            .http2_keep_alive_timeout(Duration::from_secs(30))
            .http2_initial_stream_window_size(1024 * 1024) // 1MB
            .build(https);
        
        Ok(Self {
            client,
            config,
            rate_limiter: Arc::new(RateLimiter::new(config.rate_limits)),
            connection_pool: Arc::new(ConnectionPool::new()),
        })
    }
    
    async fn send_request(&self, body: Bytes) -> Result<Response<Body>> {
        // Acquire rate limit token
        self.rate_limiter.acquire().await?;
        
        let req = Request::builder()
            .method("POST")
            .uri(&self.config.endpoint)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .body(Body::from(body))?;
        
        // Apply timeout
        let response = timeout(
            Duration::from_millis(self.config.timeout_ms),
            self.client.request(req)
        ).await??;
        
        Ok(response)
    }
}

#[async_trait]
impl Provider for OpenAIConnector {
    async fn send(&self, req: NormalizedRequest) -> Result<NormalizedResponse> {
        let openai_req = self.to_openai_format(req)?;
        let body = serde_json::to_vec(&openai_req)?;
        
        let response = self.send_request(Bytes::from(body)).await?;
        let body = hyper::body::to_bytes(response.into_body()).await?;
        
        let openai_resp: OpenAIResponse = serde_json::from_slice(&body)?;
        self.from_openai_format(openai_resp)
    }
    
    async fn stream(&self, req: NormalizedRequest) -> Result<StreamHandle> {
        let mut openai_req = self.to_openai_format(req)?;
        openai_req.stream = Some(true);
        
        let body = serde_json::to_vec(&openai_req)?;
        let response = self.send_request(Bytes::from(body)).await?;
        
        // Create streaming handler
        let (tx, rx) = mpsc::channel(32);
        let body = response.into_body();
        
        tokio::spawn(async move {
            let mut reader = StreamReader::new(body);
            
            while let Some(line) = reader.read_line().await {
                if line.starts_with(b"data: ") {
                    let event = NormalizedStreamEvent::from_sse_line(
                        &line[6..],
                        StreamFormat::OpenAI
                    )?;
                    
                    if tx.send(event).await.is_err() {
                        break; // Receiver dropped
                    }
                }
            }
        });
        
        Ok(StreamHandle { rx })
    }
}

// Connection pooling with warmup
pub struct ConnectionPool {
    pools: DashMap<String, Vec<Connection>>,
    min_idle: usize,
    max_idle: usize,
}

impl ConnectionPool {
    pub async fn warmup(&self, endpoint: &str, count: usize) -> Result<()> {
        let mut connections = Vec::with_capacity(count);
        
        for _ in 0..count {
            let conn = self.create_connection(endpoint).await?;
            connections.push(conn);
        }
        
        self.pools.insert(endpoint.to_string(), connections);
        Ok(())
    }
}
```

### 2.5 Session Recording

```rust
// crates/lunaroute-session/src/recorder.rs

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};

pub struct SessionRecorder {
    storage: Arc<dyn SessionStorage>,
    encryptor: Arc<Encryptor>,
    buffer_pool: Arc<BufferPool>,
    config: SessionConfig,
}

impl SessionRecorder {
    pub async fn record_request(&self, req: &NormalizedRequest, metadata: SessionMetadata) -> Result<SessionId> {
        let session_id = SessionId::new();
        
        // Get buffer from pool
        let mut buffer = self.buffer_pool.acquire().await;
        
        // Serialize with compression
        let compressed = self.compress_request(req, &mut buffer)?;
        
        // Encrypt if required
        let payload = if self.config.encrypt_at_rest {
            self.encryptor.encrypt(&compressed)?
        } else {
            compressed
        };
        
        // Store with metadata
        let record = SessionRecord {
            id: session_id.clone(),
            tenant: metadata.tenant,
            user_key: metadata.user_key,
            timestamp: Utc::now(),
            payload_ref: self.storage.store(payload).await?,
            metadata,
        };
        
        self.index_session(record).await?;
        
        // Return buffer to pool
        self.buffer_pool.release(buffer);
        
        Ok(session_id)
    }
    
    pub fn create_stream_recorder(&self, session_id: SessionId) -> StreamRecorder {
        StreamRecorder {
            session_id,
            sequence: AtomicU64::new(0),
            buffer: BytesMut::with_capacity(64 * 1024),
            storage: self.storage.clone(),
            encryptor: self.encryptor.clone(),
        }
    }
}

pub struct StreamRecorder {
    session_id: SessionId,
    sequence: AtomicU64,
    buffer: BytesMut,
    storage: Arc<dyn SessionStorage>,
    encryptor: Arc<Encryptor>,
}

impl StreamRecorder {
    pub async fn record_event(&mut self, event: &NormalizedStreamEvent) -> Result<()> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        
        // Write with sequence number for ordering
        let entry = StreamEntry {
            sequence: seq,
            timestamp: Utc::now(),
            event: event.clone(),
        };
        
        // Batch writes for efficiency
        self.buffer.extend_from_slice(&serde_json::to_vec(&entry)?);
        self.buffer.push(b'\n');
        
        if self.buffer.len() >= FLUSH_THRESHOLD {
            self.flush().await?;
        }
        
        Ok(())
    }
    
    async fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        
        let payload = if self.encryptor.is_enabled() {
            self.encryptor.encrypt(&self.buffer)?
        } else {
            self.buffer.to_vec()
        };
        
        self.storage.append_stream(self.session_id, payload).await?;
        self.buffer.clear();
        
        Ok(())
    }
}

// Buffer pool to reduce allocations
pub struct BufferPool {
    pool: ArrayQueue<BytesMut>,
    capacity: usize,
    buffer_size: usize,
}

impl BufferPool {
    pub fn new(capacity: usize, buffer_size: usize) -> Self {
        let pool = ArrayQueue::new(capacity);
        
        // Pre-populate pool
        for _ in 0..capacity {
            let _ = pool.push(BytesMut::with_capacity(buffer_size));
        }
        
        Self { pool, capacity, buffer_size }
    }
    
    pub async fn acquire(&self) -> BytesMut {
        self.pool.pop().unwrap_or_else(|| {
            BytesMut::with_capacity(self.buffer_size)
        })
    }
    
    pub fn release(&self, mut buffer: BytesMut) {
        buffer.clear();
        let _ = self.pool.push(buffer); // Ignore if pool is full
    }
}
```

### 2.6 PII Redaction

```rust
// crates/lunaroute-pii/src/redactor.rs

use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use regex::Regex;
use ring::hmac;

pub struct PIIRedactor {
    detectors: Vec<Box<dyn PIIDetector>>,
    tokenizer: Tokenizer,
    mode: RedactionMode,
}

#[derive(Clone)]
pub enum RedactionMode {
    Remove,
    Tokenize,
    Mask { reveal_chars: usize },
}

pub trait PIIDetector: Send + Sync {
    fn detect(&self, text: &str) -> Vec<PIIMatch>;
    fn detect_streaming(&mut self, chunk: &[u8]) -> Vec<PIIMatch>;
}

pub struct EmailDetector {
    pattern: Regex,
}

impl EmailDetector {
    pub fn new() -> Self {
        Self {
            pattern: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}\b").unwrap(),
        }
    }
}

impl PIIDetector for EmailDetector {
    fn detect(&self, text: &str) -> Vec<PIIMatch> {
        self.pattern.find_iter(text)
            .map(|m| PIIMatch {
                start: m.start(),
                end: m.end(),
                pii_type: PIIType::Email,
                confidence: 1.0,
            })
            .collect()
    }
    
    fn detect_streaming(&mut self, chunk: &[u8]) -> Vec<PIIMatch> {
        // Handle chunk boundaries for streaming
        if let Ok(text) = str::from_utf8(chunk) {
            self.detect(text)
        } else {
            Vec::new()
        }
    }
}

pub struct Tokenizer {
    key: hmac::Key,
    salt: Vec<u8>,
}

impl Tokenizer {
    pub fn tokenize(&self, value: &str, pii_type: PIIType) -> String {
        let mut ctx = hmac::Context::with_key(&self.key);
        ctx.update(&self.salt);
        ctx.update(pii_type.as_bytes());
        ctx.update(value.as_bytes());
        
        let tag = ctx.sign();
        format!("{{{{REDACTED_{:?}_{}}}}}", pii_type, base64::encode(tag))
    }
    
    pub fn create_vault_entry(&self, token: &str, original: &str) -> VaultEntry {
        VaultEntry {
            token: token.to_string(),
            original: self.encrypt_value(original),
            created_at: Utc::now(),
            access_count: 0,
        }
    }
}

// Efficient multi-pattern matcher
pub struct MultiPatternDetector {
    ac: AhoCorasick,
    patterns: Vec<PIIPattern>,
}

impl MultiPatternDetector {
    pub fn new(patterns: Vec<PIIPattern>) -> Self {
        let pattern_strings: Vec<String> = patterns.iter()
            .map(|p| p.pattern.clone())
            .collect();
        
        let ac = AhoCorasickBuilder::new()
            .case_insensitive(true)
            .build(pattern_strings);
        
        Self { ac, patterns }
    }
}

impl PIIDetector for MultiPatternDetector {
    fn detect(&self, text: &str) -> Vec<PIIMatch> {
        let mut matches = Vec::new();
        
        for mat in self.ac.find_iter(text) {
            let pattern = &self.patterns[mat.pattern()];
            matches.push(PIIMatch {
                start: mat.start(),
                end: mat.end(),
                pii_type: pattern.pii_type.clone(),
                confidence: pattern.confidence,
            });
        }
        
        matches
    }
}
```

### 2.7 Metrics and Observability

```rust
// crates/lunaroute-observability/src/metrics.rs

use prometheus::{
    Encoder, Histogram, HistogramOpts, IntCounter, Registry, TextEncoder,
};
use opentelemetry::{
    global,
    sdk::{export::trace::stdout, trace as sdktrace},
    trace::{Tracer, TracerProvider},
};

pub struct MetricsCollector {
    registry: Registry,
    
    // Latency histograms with microsecond precision
    ingress_latency: Histogram,
    normalization_latency: Histogram,
    routing_latency: Histogram,
    egress_latency: Histogram,
    total_latency: Histogram,
    
    // Counters
    requests_total: IntCounter,
    requests_success: IntCounter,
    requests_failed: IntCounter,
    fallbacks_triggered: IntCounter,
    
    // Token metrics
    tokens_prompted: IntCounter,
    tokens_completed: IntCounter,
    
    // PII metrics
    pii_detections: IntCounter,
    pii_redactions: IntCounter,
}

impl MetricsCollector {
    pub fn new() -> Result<Self> {
        let registry = Registry::new();
        
        // Configure histograms with appropriate buckets for microsecond latencies
        let ingress_latency = Histogram::with_opts(
            HistogramOpts::new("ingress_latency_us", "Ingress processing latency in microseconds")
                .buckets(vec![10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0])
        )?;
        
        registry.register(Box::new(ingress_latency.clone()))?;
        
        // ... register other metrics
        
        Ok(Self {
            registry,
            ingress_latency,
            // ... other fields
        })
    }
    
    pub fn record_request_timing(&self, phase: RequestPhase, duration: Duration) {
        let micros = duration.as_micros() as f64;
        
        match phase {
            RequestPhase::Ingress => self.ingress_latency.observe(micros),
            RequestPhase::Normalization => self.normalization_latency.observe(micros),
            RequestPhase::Routing => self.routing_latency.observe(micros),
            RequestPhase::Egress => self.egress_latency.observe(micros),
            RequestPhase::Total => self.total_latency.observe(micros),
        }
    }
    
    pub async fn serve_metrics(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer)?;
        
        Ok(String::from_utf8(buffer)?)
    }
}

// Distributed tracing
pub struct TracingService {
    tracer: Box<dyn Tracer>,
}

impl TracingService {
    pub fn init() -> Result<Self> {
        global::set_text_map_propagator(TraceContextPropagator::new());
        
        let provider = sdktrace::TracerProvider::builder()
            .with_batch_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint("http://localhost:4317"),
                sdktrace::runtime::Tokio,
            )
            .with_config(
                sdktrace::config()
                    .with_sampler(sdktrace::Sampler::TraceIdRatioBased(0.1))
                    .with_resource(Resource::new(vec![
                        KeyValue::new("service.name", "lunaroute"),
                    ]))
            )
            .build();
        
        let tracer = provider.tracer("lunaroute");
        
        Ok(Self {
            tracer: Box::new(tracer),
        })
    }
    
    pub fn start_span(&self, name: &str) -> Span {
        self.tracer.start(name)
    }
}
```

## 3. Storage Layer

### 3.1 Storage Abstraction

```rust
// crates/lunaroute-storage/src/lib.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[async_trait]
pub trait ConfigStore: Send + Sync {
    async fn load_config(&self) -> Result<Configuration>;
    async fn watch_changes(&self) -> Result<ConfigWatcher>;
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn write_session(&self, session: SessionRecord) -> Result<()>;
    async fn read_session(&self, id: &SessionId) -> Result<SessionRecord>;
    async fn append_stream_event(&self, id: &SessionId, event: StreamEvent) -> Result<()>;
    async fn query_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionRecord>>;
}

#[async_trait]
pub trait StateStore: Send + Sync {
    async fn get_rate_limit(&self, key: &str) -> Result<RateLimitState>;
    async fn update_rate_limit(&self, key: &str, state: RateLimitState) -> Result<()>;
    
    async fn get_circuit_state(&self, key: &str) -> Result<CircuitState>;
    async fn update_circuit_state(&self, key: &str, state: CircuitState) -> Result<()>;
    
    async fn get_budget(&self, key: &str) -> Result<BudgetState>;
    async fn update_budget(&self, key: &str, state: BudgetState) -> Result<()>;
}

// File-based implementations

pub struct FileConfigStore {
    base_path: PathBuf,
    hot_reload: bool,
}

impl FileConfigStore {
    pub fn new(base_path: PathBuf, hot_reload: bool) -> Self {
        // Default to ~/.config/lunaroute if not specified
        let base_path = if base_path.as_os_str().is_empty() {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("lunaroute")
        } else {
            base_path
        };
        
        Self { base_path, hot_reload }
    }
    
    async fn load_file<T: DeserializeOwned>(&self, filename: &str) -> Result<T> {
        let path = self.base_path.join(filename);
        let contents = tokio::fs::read_to_string(path).await?;
        
        match path.extension().and_then(|s| s.to_str()) {
            Some("json") => Ok(serde_json::from_str(&contents)?),
            Some("yaml") | Some("yml") => Ok(serde_yaml::from_str(&contents)?),
            Some("toml") => Ok(toml::from_str(&contents)?),
            _ => Err(anyhow::anyhow!("Unsupported config format")),
        }
    }
}

#[async_trait]
impl ConfigStore for FileConfigStore {
    async fn load_config(&self) -> Result<Configuration> {
        let mut config = Configuration::default();
        
        // Load main config
        config.server = self.load_file("server.yaml").await?;
        
        // Load routing rules
        config.routing_rules = self.load_file("routes.json").await?;
        
        // Load API keys
        config.api_keys = self.load_file("keys.json").await?;
        
        // Load budgets
        config.budgets = self.load_file("budgets.json").await?;
        
        // Load prompt patches
        if self.base_path.join("prompts.json").exists() {
            config.prompt_patches = self.load_file("prompts.json").await?;
        }
        
        Ok(config)
    }
    
    async fn watch_changes(&self) -> Result<ConfigWatcher> {
        use notify::{Watcher, RecursiveMode, watcher};
        
        let (tx, rx) = mpsc::channel(32);
        let mut watcher = watcher(tx, Duration::from_secs(2))?;
        
        watcher.watch(&self.base_path, RecursiveMode::NonRecursive)?;
        
        Ok(ConfigWatcher { rx })
    }
}

pub struct FileSessionStore {
    base_path: PathBuf,
    index: Arc<RwLock<SessionIndex>>,
    compression: CompressionType,
    encryption: Option<Encryptor>,
}

impl FileSessionStore {
    pub fn new(base_path: PathBuf) -> Result<Self> {
        // Default to ~/.config/lunaroute/sessions if not specified
        let base_path = if base_path.as_os_str().is_empty() {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("lunaroute")
                .join("sessions")
        } else {
            base_path
        };
        
        std::fs::create_dir_all(&base_path)?;
        
        // Load or create index
        let index_path = base_path.join("index.json");
        let index = if index_path.exists() {
            let data = std::fs::read_to_string(&index_path)?;
            serde_json::from_str(&data)?
        } else {
            SessionIndex::new()
        };
        
        Ok(Self {
            base_path,
            index: Arc::new(RwLock::new(index)),
            compression: CompressionType::Zstd,
            encryption: None,
        })
    }
    
    fn session_path(&self, id: &SessionId) -> PathBuf {
        // Organize by date for easier management
        let date = id.timestamp().format("%Y/%m/%d");
        self.base_path
            .join(date.to_string())
            .join(format!("{}.session", id))
    }
    
    fn ensure_parent_dir(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn write_session(&self, session: SessionRecord) -> Result<()> {
        let path = self.session_path(&session.id);
        self.ensure_parent_dir(&path)?;
        
        // Serialize and compress
        let data = serde_json::to_vec(&session)?;
        let compressed = self.compress(&data)?;
        
        // Optionally encrypt
        let final_data = if let Some(enc) = &self.encryption {
            enc.encrypt(&compressed)?
        } else {
            compressed
        };
        
        // Write atomically
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, final_data).await?;
        tokio::fs::rename(temp_path, &path).await?;
        
        // Update index
        {
            let mut index = self.index.write();
            index.add_session(session.metadata());
            
            // Periodically persist index
            if index.needs_flush() {
                let index_data = serde_json::to_vec_pretty(&*index)?;
                let index_path = self.base_path.join("index.json");
                tokio::fs::write(index_path, index_data).await?;
            }
        }
        
        Ok(())
    }
    
    async fn append_stream_event(&self, id: &SessionId, event: StreamEvent) -> Result<()> {
        let stream_path = self.session_path(id).with_extension("stream");
        self.ensure_parent_dir(&stream_path)?;
        
        // Append to stream file (newline-delimited JSON)
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stream_path)
            .await?;
        
        let line = serde_json::to_string(&event)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        
        Ok(())
    }
    
    async fn query_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionRecord>> {
        let index = self.index.read();
        let matching_ids = index.query(&filter);
        
        let mut sessions = Vec::new();
        for id in matching_ids {
            if let Ok(session) = self.read_session(&id).await {
                sessions.push(session);
            }
        }
        
        Ok(sessions)
    }
}

// In-memory state with periodic persistence
pub struct FileStateStore {
    base_path: PathBuf,
    rate_limits: Arc<DashMap<String, RateLimitState>>,
    circuits: Arc<DashMap<String, CircuitState>>,
    budgets: Arc<DashMap<String, BudgetState>>,
    persist_interval: Duration,
}

impl FileStateStore {
    pub fn new(base_path: PathBuf) -> Result<Self> {
        // Default to ~/.config/lunaroute/state if not specified
        let base_path = if base_path.as_os_str().is_empty() {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("lunaroute")
                .join("state")
        } else {
            base_path
        };
        
        std::fs::create_dir_all(&base_path)?;
        
        let store = Self {
            base_path: base_path.clone(),
            rate_limits: Arc::new(DashMap::new()),
            circuits: Arc::new(DashMap::new()),
            budgets: Arc::new(DashMap::new()),
            persist_interval: Duration::from_secs(10),
        };
        
        // Load existing state
        store.load_state()?;
        
        // Start background persistence
        store.start_persistence_loop();
        
        Ok(store)
    }
    
    fn load_state(&self) -> Result<()> {
        // Load rate limits
        let rate_limit_path = self.base_path.join("rate_limits.json");
        if rate_limit_path.exists() {
            let data = std::fs::read_to_string(&rate_limit_path)?;
            let states: HashMap<String, RateLimitState> = serde_json::from_str(&data)?;
            for (k, v) in states {
                self.rate_limits.insert(k, v);
            }
        }
        
        // Load circuit states
        let circuit_path = self.base_path.join("circuits.json");
        if circuit_path.exists() {
            let data = std::fs::read_to_string(&circuit_path)?;
            let states: HashMap<String, CircuitState> = serde_json::from_str(&data)?;
            for (k, v) in states {
                self.circuits.insert(k, v);
            }
        }
        
        Ok(())
    }
    
    fn start_persistence_loop(&self) {
        let base_path = self.base_path.clone();
        let rate_limits = self.rate_limits.clone();
        let circuits = self.circuits.clone();
        let budgets = self.budgets.clone();
        let interval = self.persist_interval;
        
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            
            loop {
                ticker.tick().await;
                
                // Persist rate limits
                let rate_limit_data: HashMap<_, _> = rate_limits.iter()
                    .map(|e| (e.key().clone(), e.value().clone()))
                    .collect();
                
                if !rate_limit_data.is_empty() {
                    let path = base_path.join("rate_limits.json");
                    if let Ok(json) = serde_json::to_vec_pretty(&rate_limit_data) {
                        let _ = tokio::fs::write(path, json).await;
                    }
                }
                
                // Persist circuits
                let circuit_data: HashMap<_, _> = circuits.iter()
                    .map(|e| (e.key().clone(), e.value().clone()))
                    .collect();
                
                if !circuit_data.is_empty() {
                    let path = base_path.join("circuits.json");
                    if let Ok(json) = serde_json::to_vec_pretty(&circuit_data) {
                        let _ = tokio::fs::write(path, json).await;
                    }
                }
                
                // Persist budgets
                let budget_data: HashMap<_, _> = budgets.iter()
                    .map(|e| (e.key().clone(), e.value().clone()))
                    .collect();
                
                if !budget_data.is_empty() {
                    let path = base_path.join("budgets.json");
                    if let Ok(json) = serde_json::to_vec_pretty(&budget_data) {
                        let _ = tokio::fs::write(path, json).await;
                    }
                }
            }
        });
    }
}

#[async_trait]
impl StateStore for FileStateStore {
    async fn get_rate_limit(&self, key: &str) -> Result<RateLimitState> {
        self.rate_limits.get(key)
            .map(|e| e.value().clone())
            .ok_or_else(|| anyhow::anyhow!("Rate limit not found"))
    }
    
    async fn update_rate_limit(&self, key: &str, state: RateLimitState) -> Result<()> {
        self.rate_limits.insert(key.to_string(), state);
        Ok(())
    }
    
    // Similar implementations for circuit and budget methods...
}

// Session index for efficient queries
#[derive(Serialize, Deserialize)]
pub struct SessionIndex {
    by_user: HashMap<String, Vec<SessionMetadata>>,
    by_date: BTreeMap<DateTime<Utc>, Vec<SessionId>>,
    by_tenant: HashMap<String, Vec<SessionMetadata>>,
    total_sessions: usize,
    last_flush: DateTime<Utc>,
}

impl SessionIndex {
    pub fn new() -> Self {
        Self {
            by_user: HashMap::new(),
            by_date: BTreeMap::new(),
            by_tenant: HashMap::new(),
            total_sessions: 0,
            last_flush: Utc::now(),
        }
    }
    
    pub fn add_session(&mut self, metadata: SessionMetadata) {
        self.by_user.entry(metadata.user_key.clone())
            .or_insert_with(Vec::new)
            .push(metadata.clone());
        
        self.by_date.entry(metadata.timestamp.date())
            .or_insert_with(Vec::new)
            .push(metadata.id.clone());
        
        self.by_tenant.entry(metadata.tenant.clone())
            .or_insert_with(Vec::new)
            .push(metadata.clone());
        
        self.total_sessions += 1;
    }
    
    pub fn query(&self, filter: &SessionFilter) -> Vec<SessionId> {
        let mut results = Vec::new();
        
        // Apply filters
        if let Some(user) = &filter.user_key {
            if let Some(sessions) = self.by_user.get(user) {
                results.extend(sessions.iter().map(|s| s.id.clone()));
            }
        }
        
        // Date range filter
        if let Some(start) = filter.start_time {
            let range = self.by_date.range(start..filter.end_time.unwrap_or(Utc::now()));
            for (_, ids) in range {
                results.extend(ids.clone());
            }
        }
        
        results
    }
    
    pub fn needs_flush(&self) -> bool {
        self.total_sessions % 100 == 0 || 
        Utc::now().signed_duration_since(self.last_flush).num_seconds() > 60
    }
}
```

### 3.2 Compression and Encryption

```rust
// crates/lunaroute-storage/src/compression.rs

use zstd::stream::{encode_all, decode_all};
use lz4_flex::{compress_prepend_size, decompress_size_prepended};

#[derive(Clone, Copy)]
pub enum CompressionType {
    None,
    Zstd,
    Lz4,
}

pub struct Compressor {
    compression_type: CompressionType,
    level: i32,
}

impl Compressor {
    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        match self.compression_type {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Zstd => {
                encode_all(data, self.level).map_err(Into::into)
            }
            CompressionType::Lz4 => {
                Ok(compress_prepend_size(data))
            }
        }
    }
    
    pub fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        match self.compression_type {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Zstd => {
                decode_all(data).map_err(Into::into)
            }
            CompressionType::Lz4 => {
                decompress_size_prepended(data).map_err(Into::into)
            }
        }
    }
}

// crates/lunaroute-storage/src/encryption.rs

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};

pub struct Encryptor {
    key: LessSafeKey,
    rng: SystemRandom,
}

impl Encryptor {
    pub fn from_key_file(path: &Path) -> Result<Self> {
        let key_bytes = std::fs::read(path)?;
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes)?;
        
        Ok(Self {
            key: LessSafeKey::new(unbound_key),
            rng: SystemRandom::new(),
        })
    }
    
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; 12];
        self.rng.fill(&mut nonce_bytes)?;
        
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aad = Aad::empty();
        
        let mut in_out = plaintext.to_vec();
        self.key.seal_in_place_append_tag(nonce, aad, &mut in_out)?;
        
        // Prepend nonce to ciphertext
        let mut result = nonce_bytes.to_vec();
        result.extend(in_out);
        
        Ok(result)
    }
    
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < 12 {
            return Err(anyhow::anyhow!("Invalid ciphertext"));
        }
        
        let (nonce_bytes, encrypted) = ciphertext.split_at(12);
        let nonce = Nonce::assume_unique_for_key(
            nonce_bytes.try_into().map_err(|_| anyhow::anyhow!("Invalid nonce"))?
        );
        
        let aad = Aad::empty();
        let mut in_out = encrypted.to_vec();
        
        self.key.open_in_place(nonce, aad, &mut in_out)?;
        
        // Remove tag
        let plaintext_len = in_out.len() - AES_256_GCM.tag_len();
        in_out.truncate(plaintext_len);
        
        Ok(in_out)
    }
}
```

### 3.3 Efficient File Operations

```rust
// crates/lunaroute-storage/src/io.rs

use memmap2::{MmapOptions, Mmap};
use tokio::io::{AsyncWriteExt, AsyncReadExt};

/// Memory-mapped file reader for large session files
pub struct MappedReader {
    mmap: Mmap,
    position: usize,
}

impl MappedReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        
        Ok(Self {
            mmap,
            position: 0,
        })
    }
    
    pub fn read_line(&mut self) -> Option<&[u8]> {
        if self.position >= self.mmap.len() {
            return None;
        }
        
        let slice = &self.mmap[self.position..];
        if let Some(newline_pos) = slice.iter().position(|&b| b == b'\n') {
            let line = &slice[..newline_pos];
            self.position += newline_pos + 1;
            Some(line)
        } else {
            self.position = self.mmap.len();
            Some(slice)
        }
    }
}

/// Buffered async writer with atomic writes
pub struct AtomicWriter {
    path: PathBuf,
    buffer: BytesMut,
    flush_size: usize,
}

impl AtomicWriter {
    pub fn new(path: PathBuf, flush_size: usize) -> Self {
        Self {
            path,
            buffer: BytesMut::with_capacity(flush_size * 2),
            flush_size,
        }
    }
    
    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(data);
        
        if self.buffer.len() >= self.flush_size {
            self.flush().await?;
        }
        
        Ok(())
    }
    
    pub async fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        
        // Write to temp file then rename atomically
        let temp_path = self.path.with_extension("tmp");
        
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .await?;
        
        file.write_all(&self.buffer).await?;
        file.sync_all().await?;
        drop(file);
        
        tokio::fs::rename(temp_path, &self.path).await?;
        self.buffer.clear();
        
        Ok(())
    }
}

/// Rolling file writer for session streams
pub struct RollingWriter {
    base_path: PathBuf,
    current_file: Option<File>,
    current_size: usize,
    max_size: usize,
    file_index: usize,
}

impl RollingWriter {
    pub fn new(base_path: PathBuf, max_size: usize) -> Self {
        Self {
            base_path,
            current_file: None,
            current_size: 0,
            max_size,
            file_index: 0,
        }
    }
    
    async fn rotate(&mut self) -> Result<()> {
        if let Some(mut file) = self.current_file.take() {
            file.sync_all().await?;
        }
        
        self.file_index += 1;
        let path = self.base_path.with_extension(format!("stream.{}", self.file_index));
        
        self.current_file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await?
        );
        self.current_size = 0;
        
        Ok(())
    }
    
    pub async fn write_event(&mut self, event: &StreamEvent) -> Result<()> {
        let data = serde_json::to_vec(event)?;
        let size = data.len() + 1; // +1 for newline
        
        if self.current_size + size > self.max_size {
            self.rotate().await?;
        }
        
        if self.current_file.is_none() {
            self.rotate().await?;
        }
        
        if let Some(file) = &mut self.current_file {
            file.write_all(&data).await?;
            file.write_all(b"\n").await?;
            self.current_size += size;
        }
        
        Ok(())
    }
}

## 4. Performance Optimizations

### 4.1 Memory Management

```rust
// Pre-allocated arena for request processing
pub struct RequestArena {
    memory: Vec<u8>,
    offset: AtomicUsize,
    size: usize,
}

impl RequestArena {
    pub fn new(size: usize) -> Self {
        Self {
            memory: vec![0u8; size],
            offset: AtomicUsize::new(0),
            size,
        }
    }
    
    pub fn allocate(&self, size: usize) -> Option<&mut [u8]> {
        let start = self.offset.fetch_add(size, Ordering::Relaxed);
        
        if start + size <= self.size {
            unsafe {
                Some(&mut *(&self.memory[start..start + size] as *const [u8] as *mut [u8]))
            }
        } else {
            None
        }
    }
    
    pub fn reset(&self) {
        self.offset.store(0, Ordering::Relaxed);
    }
}

// SIMD-accelerated string operations
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

pub fn find_json_delimiter(data: &[u8]) -> Option<usize> {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            return find_delimiter_avx2(data);
        }
    }
    
    // Fallback to scalar
    data.iter().position(|&b| b == b',' || b == b'}')
}

#[cfg(target_arch = "x86_64")]
unsafe fn find_delimiter_avx2(data: &[u8]) -> Option<usize> {
    let comma = _mm256_set1_epi8(b',' as i8);
    let brace = _mm256_set1_epi8(b'}' as i8);
    
    let chunks = data.chunks_exact(32);
    let remainder = chunks.remainder();
    
    for (i, chunk) in chunks.enumerate() {
        let v = _mm256_loadu_si256(chunk.as_ptr() as *const __m256i);
        let comma_mask = _mm256_cmpeq_epi8(v, comma);
        let brace_mask = _mm256_cmpeq_epi8(v, brace);
        let combined = _mm256_or_si256(comma_mask, brace_mask);
        
        let mask = _mm256_movemask_epi8(combined);
        if mask != 0 {
            return Some(i * 32 + mask.trailing_zeros() as usize);
        }
    }
    
    // Check remainder
    remainder.iter().position(|&b| b == b',' || b == b'}')
        .map(|pos| data.len() - remainder.len() + pos)
}
```

### 4.2 Tokio Runtime Tuning

```rust
// main.rs
use tokio::runtime::{Builder, Runtime};

fn create_runtime() -> Runtime {
    Builder::new_multi_thread()
        .worker_threads(num_cpus::get())
        .thread_name("lunaroute-worker")
        .thread_stack_size(2 * 1024 * 1024) // 2MB stacks
        .max_blocking_threads(32)
        .enable_all()
        .build()
        .expect("Failed to create runtime")
}

// CPU affinity for latency-critical threads
#[cfg(target_os = "linux")]
fn set_cpu_affinity(cpu_id: usize) {
    use libc::{cpu_set_t, CPU_SET, CPU_ZERO, sched_setaffinity};
    
    unsafe {
        let mut set: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut set);
        CPU_SET(cpu_id, &mut set);
        
        sched_setaffinity(
            0, // Current thread
            std::mem::size_of::<cpu_set_t>(),
            &set as *const cpu_set_t
        );
    }
}
```

## 5. Testing Framework

### 5.1 Load Testing

```rust
// tests/load/src/main.rs
use goose::prelude::*;

async fn openai_chat_completion(user: &mut GooseUser) -> TransactionResult {
    let request = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hello, how are you?"}
        ],
        "stream": false
    });
    
    let _response = user.post("/v1/chat/completions", &request.to_string())
        .await?;
    
    Ok(())
}

async fn streaming_request(user: &mut GooseUser) -> TransactionResult {
    let request = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Write a short story"}
        ],
        "stream": true
    });
    
    let mut response = user.post_json("/v1/chat/completions", &request)
        .await?;
    
    // Consume stream
    while let Some(chunk) = response.chunk().await? {
        // Process SSE events
        let _ = std::str::from_utf8(&chunk);
    }
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    GooseAttack::initialize()?
        .register_scenario(scenario!("LoadTest")
            .register_transaction(transaction!(openai_chat_completion).set_weight(8)?)
            .register_transaction(transaction!(streaming_request).set_weight(2)?)
        )
        .execute()
        .await?;
    
    Ok(())
}
```

### 5.2 Compatibility Testing

```rust
// tests/compat/src/lib.rs
use serde_json::Value;

pub struct CompatibilityTester {
    golden_fixtures: HashMap<String, GoldenFixture>,
}

#[derive(Debug)]
pub struct GoldenFixture {
    pub input: Value,
    pub expected_normalized: Value,
    pub expected_output: Value,
}

impl CompatibilityTester {
    pub async fn test_openai_to_anthropic(&self) -> Result<()> {
        for (name, fixture) in &self.golden_fixtures {
            let normalized = normalize_openai_request(&fixture.input)?;
            assert_eq!(normalized, fixture.expected_normalized, "Failed: {}", name);
            
            let anthropic = convert_to_anthropic(&normalized)?;
            let response = send_to_anthropic_mock(anthropic).await?;
            
            let openai_response = convert_anthropic_response_to_openai(response)?;
            assert_eq!(openai_response, fixture.expected_output, "Failed: {}", name);
        }
        
        Ok(())
    }
}
```

## 6. Configuration Management

### 6.1 File-Based Configuration Structure

```yaml
# ~/.config/lunaroute/server.yaml
server:
  workers: auto  # Use CPU count
  max_connections: 10000
  keepalive_timeout_secs: 60
  
listeners:
  - id: openai_primary
    type: openai
    bind: 0.0.0.0:8080
    tls:
      cert: ~/.config/lunaroute/certs/server.crt
      key: ~/.config/lunaroute/certs/server.key
    max_body_size_mb: 10
    
  - id: anthropic_primary  
    type: anthropic
    bind: 0.0.0.0:8081
    tls:
      cert: ~/.config/lunaroute/certs/server.crt
      key: ~/.config/lunaroute/certs/server.key
    max_body_size_mb: 10
    
providers:
  - name: openai
    base_url: https://api.openai.com
    timeout_ms: 30000
    max_retries: 2
    connection_pool:
      min_idle: 10
      max_idle: 100
      
  - name: anthropic
    base_url: https://api.anthropic.com  
    timeout_ms: 30000
    max_retries: 2
    connection_pool:
      min_idle: 10
      max_idle: 100

storage:
  config_path: ~/.config/lunaroute
  data_path: ~/.config/lunaroute/data
  session_retention_days: 30
  compression: zstd
  encryption:
    enabled: false
    key_file: ~/.config/lunaroute/keys/session.key
  
observability:
  metrics:
    enabled: true
    bind: 0.0.0.0:9090
    
  tracing:
    enabled: true
    endpoint: http://localhost:4317
    sample_rate: 0.1
    
  logging:
    level: info
    format: json
    
performance:
  buffer_pool_size: 1000
  buffer_size_kb: 64
  arena_size_mb: 100
  stream_flush_interval_ms: 100
  state_persist_interval_secs: 10
```

```json
// ~/.config/lunaroute/routes.json
{
  "rules": [
    {
      "id": "rule_01",
      "priority": 100,
      "listener": "openai",
      "match": {
        "model": "gpt-4o",
        "headers": {
          "X-User-Tier": "paid"
        }
      },
      "target": {
        "provider": "anthropic",
        "model": "claude-3.5-sonnet"
      },
      "fallbacks": [
        {
          "provider": "openai",
          "model": "gpt-4o-mini"
        }
      ],
      "options": {
        "strip_unsupported_tools": true,
        "timeout_ms": 20000
      }
    },
    {
      "id": "rule_02",
      "priority": 200,
      "listener": "anthropic",
      "match": {
        "model": "claude-3.5-haiku"
      },
      "target": {
        "provider": "openai",
        "model": "gpt-4o-mini"
      },
      "fallbacks": []
    }
  ]
}
```

```json
// ~/.config/lunaroute/keys.json
{
  "api_keys": [
    {
      "id": "key_001",
      "key_hash": "$argon2id$v=19$m=65536,t=3,p=4$...",
      "name": "Production API Key",
      "tenant": "default",
      "scopes": ["openai_ingress", "anthropic_ingress"],
      "rate_limit": {
        "requests_per_second": 100,
        "burst": 200
      },
      "budget_id": "budget_001",
      "created_at": "2025-10-01T00:00:00Z",
      "expires_at": null
    }
  ]
}
```

```json
// ~/.config/lunaroute/budgets.json
{
  "budgets": [
    {
      "id": "budget_001",
      "scope": {
        "type": "key",
        "id": "key_001"
      },
      "windows": [
        {
          "type": "daily",
          "limits": {
            "tokens": 1000000,
            "requests": 10000,
            "cost_cents": 5000
          }
        },
        {
          "type": "monthly",
          "limits": {
            "tokens": 20000000,
            "requests": 200000,
            "cost_cents": 100000
          }
        }
      ],
      "enforcement": {
        "soft_limit_percent": 80,
        "actions": ["throttle", "reroute_to_cheaper"]
      }
    }
  ]
}
```

### 6.2 Directory Structure

```
~/.config/lunaroute/
├── server.yaml              # Main server configuration
├── routes.json              # Routing rules
├── keys.json                # API keys
├── budgets.json             # Budget configurations
├── prompts.json             # System prompt patches (optional)
├── providers/               # Provider-specific configs
│   ├── openai.yaml
│   └── anthropic.yaml
├── certs/                   # TLS certificates
│   ├── server.crt
│   └── server.key
├── keys/                    # Encryption keys
│   └── session.key
├── data/                    # Runtime data
│   ├── sessions/            # Session storage
│   │   └── 2025/
│   │       └── 10/
│   │           └── 01/
│   │               ├── sess_abc123.session
│   │               └── sess_abc123.stream
│   ├── state/               # Runtime state
│   │   ├── rate_limits.json
│   │   ├── circuits.json
│   │   └── budgets.json
│   └── index.json          # Session index
└── logs/                    # Application logs
```

## 7. Main Application Entry Point

```rust
// main.rs
use std::path::PathBuf;
use clap::Parser;
use dirs;

#[derive(Parser)]
struct Args {
    #[arg(short, long)]
    config_dir: Option<PathBuf>,
    
    #[arg(short, long)]
    data_dir: Option<PathBuf>,
    
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // Default to ~/.config/lunaroute if not specified
    let config_dir = args.config_dir.unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("lunaroute")
    });
    
    let data_dir = args.data_dir.unwrap_or_else(|| {
        config_dir.join("data")
    });
    
    // Ensure directories exist
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&data_dir)?;
    
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(args.log_level)
        .json()
        .init();
    
    // Load configuration
    let config_store = FileConfigStore::new(config_dir.clone(), true);
    let config = config_store.load_config().await?;
    
    // Initialize storage backends
    let session_store = Arc::new(FileSessionStore::new(data_dir.join("sessions"))?);
    let state_store = Arc::new(FileStateStore::new(data_dir.join("state"))?);
    
    // Watch for config changes
    if config.server.hot_reload {
        let watcher = config_store.watch_changes().await?;
        tokio::spawn(async move {
            while let Some(event) = watcher.rx.recv().await {
                info!("Configuration changed: {:?}", event);
                // Reload configuration
            }
        });
    }
    
    // Initialize components
    let normalizer = Arc::new(Normalizer::new(config.pii.clone()));
    let router = Arc::new(RoutingEngine::new(config.routing_rules.clone(), state_store.clone()));
    let metrics = Arc::new(MetricsCollector::new()?);
    
    // Start ingress listeners
    let mut tasks = Vec::new();
    
    for listener_config in config.listeners {
        let listener = IngressListener::new(
            listener_config,
            normalizer.clone(),
            router.clone(),
            session_store.clone(),
            metrics.clone(),
        );
        
        tasks.push(tokio::spawn(listener.start()));
    }
    
    // Start metrics server
    let metrics_server = MetricsServer::new(config.observability.metrics, metrics.clone());
    tasks.push(tokio::spawn(metrics_server.start()));
    
    // Wait for all tasks
    futures::future::join_all(tasks).await;
    
    Ok(())
}
```

## 8. Deployment

### 8.1 Dockerfile

```dockerfile
# Build stage
FROM rust:1.75 as builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build with optimizations
RUN cargo build --release --features "jemalloc simd"

# Runtime stage
FROM ubuntu:22.04

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create user and directories
RUN useradd -m -s /bin/bash lunaroute && \
    mkdir -p /home/lunaroute/.config/lunaroute/data && \
    chown -R lunaroute:lunaroute /home/lunaroute

COPY --from=builder /app/target/release/lunaroute /usr/local/bin/

# Use jemalloc for better memory performance
ENV LD_PRELOAD=/usr/lib/x86_64-linux-gnu/libjemalloc.so.2
ENV HOME=/home/lunaroute

EXPOSE 8080 8081 9090

USER lunaroute
WORKDIR /home/lunaroute

ENTRYPOINT ["/usr/local/bin/lunaroute"]
CMD ["--config-dir", "/home/lunaroute/.config/lunaroute"]
```

### 8.2 Docker Compose for Development

```yaml
version: '3.8'

services:
  lunaroute:
    build: .
    ports:
      - "8080:8080"  # OpenAI API
      - "8081:8081"  # Anthropic API
      - "9090:9090"  # Metrics
    volumes:
      - ./config:/home/lunaroute/.config/lunaroute:rw
      - lunaroute_data:/home/lunaroute/.config/lunaroute/data
    environment:
      - RUST_LOG=debug
      - OPENAI_API_KEY=${OPENAI_API_KEY}
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
    restart: unless-stopped
    
  # Optional: Prometheus for metrics collection
  prometheus:
    image: prom/prometheus:latest
    ports:
      - "9091:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
      - prometheus_data:/prometheus
    command:
      - '--config.file=/etc/prometheus/prometheus.yml'
      - '--storage.tsdb.path=/prometheus'
      
  # Optional: Grafana for visualization
  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    volumes:
      - grafana_data:/var/lib/grafana
      - ./grafana/dashboards:/etc/grafana/provisioning/dashboards:ro
      - ./grafana/datasources:/etc/grafana/provisioning/datasources:ro
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
      
volumes:
  lunaroute_data:
  prometheus_data:
  grafana_data:
```

### 8.3 Systemd Service (for Linux servers)

```ini
# /etc/systemd/system/lunaroute.service
[Unit]
Description=LunaRoute LLM Proxy
After=network.target

[Service]
Type=simple
User=lunaroute
Group=lunaroute
WorkingDirectory=/home/lunaroute
ExecStart=/usr/local/bin/lunaroute --config-dir /home/lunaroute/.config/lunaroute
Restart=on-failure
RestartSec=5
StandardOutput=append:/home/lunaroute/.config/lunaroute/logs/lunaroute.log
StandardError=append:/home/lunaroute/.config/lunaroute/logs/lunaroute.error.log

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=false
ReadWritePaths=/home/lunaroute/.config/lunaroute

[Install]
WantedBy=multi-user.target
```

## 9. CLI Tool

```rust
// crates/lunaroute-cli/src/main.rs
use clap::{Parser, Subcommand};
use dirs;

#[derive(Parser)]
#[command(name = "lunaroute")]
#[command(about = "LunaRoute CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    #[arg(long)]
    config_dir: Option<PathBuf>,
    
    #[arg(long, default_value = "http://localhost:8080")]
    endpoint: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Test routing rules
    Route {
        #[arg(long)]
        request: PathBuf,
        
        #[arg(long)]
        rule: Option<PathBuf>,
        
        #[arg(long)]
        dry_run: bool,
    },
    
    /// Export sessions
    Export {
        #[arg(long)]
        since: String,
        
        #[arg(long, default_value = "ndjson")]
        format: String,
        
        #[arg(long)]
        output: PathBuf,
    },
    
    /// Manage API keys
    Keys {
        #[command(subcommand)]
        action: KeyActions,
    },
    
    /// View metrics
    Metrics {
        #[arg(long)]
        provider: Option<String>,
        
        #[arg(long)]
        window: Option<String>,
    },
    
    /// Initialize configuration
    Init {
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum KeyActions {
    Create {
        name: String,
        #[arg(long)]
        scopes: Vec<String>,
    },
    List,
    Delete {
        id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    let config_dir = cli.config_dir.unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("lunaroute")
    });
    
    match cli.command {
        Commands::Init { force } => {
            if config_dir.exists() && !force {
                eprintln!("Configuration directory already exists. Use --force to overwrite.");
                std::process::exit(1);
            }
            
            // Create directory structure
            std::fs::create_dir_all(&config_dir)?;
            std::fs::create_dir_all(config_dir.join("data"))?;
            std::fs::create_dir_all(config_dir.join("certs"))?;
            std::fs::create_dir_all(config_dir.join("keys"))?;
            
            // Write default configuration files
            let server_config = include_str!("../templates/server.yaml");
            std::fs::write(config_dir.join("server.yaml"), server_config)?;
            
            let routes_config = include_str!("../templates/routes.json");
            std::fs::write(config_dir.join("routes.json"), routes_config)?;
            
            let keys_config = include_str!("../templates/keys.json");
            std::fs::write(config_dir.join("keys.json"), keys_config)?;
            
            let budgets_config = include_str!("../templates/budgets.json");
            std::fs::write(config_dir.join("budgets.json"), budgets_config)?;
            
            println!("Configuration initialized at: {}", config_dir.display());
            println!("Next steps:");
            println!("  1. Add your API keys to the environment or keys.json");
            println!("  2. Configure routing rules in routes.json");
            println!("  3. Start LunaRoute with: lunaroute");
        }
        
        Commands::Route { request, rule, dry_run } => {
            let req = read_request(request)?;
            let rules = rule.map(read_rules).transpose()?;
            
            let client = LunaClient::new(&cli.endpoint);
            let result = client.test_route(req, rules, dry_run).await?;
            
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        
        Commands::Export { since, format, output } => {
            let client = LunaClient::new(&cli.endpoint);
            let sessions = client.export_sessions(since, format).await?;
            
            let mut file = File::create(output)?;
            file.write_all(&sessions)?;
            
            println!("Exported {} bytes", sessions.len());
        }
        
        Commands::Keys { action } => {
            match action {
                KeyActions::Create { name, scopes } => {
                    let key = generate_api_key();
                    let hash = hash_api_key(&key)?;
                    
                    let key_entry = json!({
                        "id": format!("key_{}", uuid::Uuid::new_v4()),
                        "name": name,
                        "key_hash": hash,
                        "scopes": scopes,
                        "created_at": Utc::now().to_rfc3339(),
                    });
                    
                    // Load existing keys
                    let keys_path = config_dir.join("keys.json");
                    let mut keys_config: Value = if keys_path.exists() {
                        serde_json::from_str(&std::fs::read_to_string(&keys_path)?)?
                    } else {
                        json!({"api_keys": []})
                    };
                    
                    keys_config["api_keys"].as_array_mut().unwrap().push(key_entry);
                    
                    std::fs::write(keys_path, serde_json::to_string_pretty(&keys_config)?)?;
                    
                    println!("API Key created: {}", key);
                    println!("Store this key securely - it cannot be retrieved again.");
                }
                
                KeyActions::List => {
                    let keys_path = config_dir.join("keys.json");
                    if keys_path.exists() {
                        let keys_config: Value = serde_json::from_str(
                            &std::fs::read_to_string(&keys_path)?
                        )?;
                        
                        println!("{}", serde_json::to_string_pretty(&keys_config["api_keys"])?);
                    } else {
                        println!("No API keys configured");
                    }
                }
                
                KeyActions::Delete { id } => {
                    let keys_path = config_dir.join("keys.json");
                    let mut keys_config: Value = serde_json::from_str(
                        &std::fs::read_to_string(&keys_path)?
                    )?;
                    
                    let keys = keys_config["api_keys"].as_array_mut().unwrap();
                    keys.retain(|k| k["id"] != id);
                    
                    std::fs::write(keys_path, serde_json::to_string_pretty(&keys_config)?)?;
                    println!("API key {} deleted", id);
                }
            }
        }
        
        Commands::Metrics { provider, window } => {
            let client = LunaClient::new(&cli.endpoint);
            let metrics = client.get_metrics(provider, window).await?;
            
            println!("{}", metrics);
        }
    }
    
    Ok(())
}

fn generate_api_key() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    
    (0..32)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn hash_api_key(key: &str) -> Result<String> {
    use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
    
    let salt = SaltString::generate(&mut rand::thread_rng());
    let argon2 = Argon2::default();
    
    Ok(argon2.hash_password(key.as_bytes(), &salt)?.to_string())
}
```

## 10. Monitoring and Alerts

```yaml
# prometheus.yml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'lunaroute'
    static_configs:
      - targets: ['localhost:9090']

rule_files:
  - 'rules.yml'

# rules.yml
groups:
  - name: lunaroute
    interval: 30s
    rules:
      - alert: HighLatency
        expr: histogram_quantile(0.95, rate(request_latency_bucket[5m])) > 35000
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High p95 latency detected"
          description: "p95 latency is {{ $value }}μs"
          
      - alert: HighErrorRate
        expr: rate(requests_failed_total[5m]) / rate(requests_total[5m]) > 0.01
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "High error rate detected"
          description: "Error rate is {{ $value | humanizePercentage }}"
          
      - alert: CircuitBreakerOpen
        expr: circuit_breaker_state == 1
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "Circuit breaker open for {{ $labels.provider }}"
          
      - alert: BudgetExceeded
        expr: budget_usage_percent > 90
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "Budget usage at {{ $value }}% for {{ $labels.budget_id }}"
```

## 11. Security Considerations

- All configuration and session data stored under user's home directory (`~/.config/lunaroute`)
- API keys hashed with Argon2id before storage
- Session payloads optionally encrypted with AES-256-GCM
- TLS 1.3 minimum for all external connections
- File permissions restricted to user only (700 for directories, 600 for files)
- Atomic file operations prevent corruption
- Rate limiting at multiple levels (global, tenant, key)
- Input validation using schema validators
- PII tokenization with HMAC-SHA256 for reversibility
- Audit logging for all configuration changes
- Security headers (HSTS, CSP, etc.) on all HTTP responses

## 12. Quick Start Guide

```bash
# Install LunaRoute
cargo install --path .

# Initialize configuration
lunaroute init

# Set API keys
export OPENAI_API_KEY="your-openai-key"
export ANTHROPIC_API_KEY="your-anthropic-key"

# Start the proxy
lunaroute

# Or run with Docker
docker-compose up

# Test with curl (OpenAI format)
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer your-lunaroute-key" \
  -d '{
    "model": "gpt-4",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# View metrics
curl http://localhost:9090/metrics

# Export sessions
lunaroute export --since "1h" --output sessions.ndjson
```
