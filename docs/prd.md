# LunaRoute – Product Requirements Document (PRD)

**Status:** Draft v0.1
**Owner:** Eran Sandler
**Date:** 2025‑10‑01
**Tech:** Rust (Tokio, Hyper/Axum), Prometheus, OpenTelemetry, Postgres, Redis

---

## 1) Executive summary

LunaRoute is a low‑latency proxy/router that sits between client apps and LLM providers. It speaks both OpenAI and Anthropic dialects, translates between them when needed, records sessions, enforces budgets and auth, performs PII redaction, and routes traffic using simple rules in the MVP and smarter policies later.

Primary objective: drop‑in compatibility for both OpenAI and Anthropic APIs with the option to redirect requests to any provider without changing client code.

---

## 2) Goals and non‑goals

**Goals**

* Inbound protocol compatibility for OpenAI and Anthropic (chat/messages + streaming).
* Translate OpenAI⇄Anthropic requests and responses.
* Session recording with replay/export.
* Stats: latency, errors, token usage, cost estimation.
* Basic routing rules (per listener/model mapping, simple fallback).
* Central system‑prompt control with opt‑in A/B tests.
* Metrics endpoint for Prometheus and traces via OpenTelemetry.
* Authentication and per‑user scopes.
* Budgets and enforcement tied to routing.
* PII substitution/redaction before egress.

**Non‑goals (v0.1)**

* Full support for every vendor feature (images, audio, function/tool ecosystems beyond basic function/tools).
* Complex policy language, ML‑driven routing, or multi‑region global control plane.
* Full admin UI (CLI + JSON/HTTP config is sufficient for MVP).

---

## 3) Target users and key use cases

**Users**: platform teams, app developers, ops/security, finance.
**Use cases**

1. Speak OpenAI on ingress but route to Anthropic or another vendor.
2. Speak Anthropic on ingress but route to OpenAI or a local model.
3. Centralize and A/B test system prompts without code changes.
4. Enforce per‑user budgets and priority routing for paid users.
5. Capture full sessions for analytics, debugging, and audit.
6. Redact PII before sending to external providers.

---

## 4) System overview and architecture

**Key components**

* **Ingress adapters**: Listeners that accept OpenAI or Anthropic dialects, including streaming.
* **Normalization layer**: Converts inbound requests into a provider‑agnostic `NormalizedRequest` and `NormalizedStreamEvent`.
* **Router**: Applies rules and policies to select a target provider/model and fallback chain.
* **Egress connectors**: Provider modules with a common trait for sending requests and translating responses back to the requester dialect.
* **Session recorder**: Tap for request/response bodies and stream events with encryption and retention controls.
* **Policy/config store**: File-based config (TOML/JSON/YAML) on disk with hot-reload and validation; DB optional in future. Rules, prompts, budgets, and keys are defined in versioned config files.
* **Observability**: Metrics `/metrics`, health checks, structured logs, OpenTelemetry traces.
* **Admin surface**: File-first workflow (edit config files), hot-reload endpoint `/admin/reload`, and HTTP JSON helpers + CLI to validate, diff, and apply configs; keys, rules, prompts, budgets are source-controlled files.

**Data stores**

* **Filesystem-first (MVP)**: All configs and session artifacts are written to local or network-mounted storage using a deterministic folder layout (see below). No database required.
* **Pluggable storage**: A `Storage` trait abstracts reads/writes. Future drivers may include S3/R2/GCS, Postgres (for indexes), or Redis (for ephemeral counters). Swapping drivers requires no API changes.

**Filesystem layout (MVP)**

```
/var/lib/lunaroute/
  config/
    lunaroute.toml           # main config (listeners, routes, prompts, budgets)
    routes.d/*.toml          # optional includes
    prompts.d/*.toml
  sessions/
    YYYY/MM/DD/<tenant>/<session_id>/
      request.json           # inbound normalized request
      response.json          # normalized final response summary
      events.ndjson          # stream transcript (ordered, chunked)
      meta.json              # routing decisions, timings, usage, redaction mode
  logs/
  keys/                      # encrypted provider keys (optional)
```

**Performance principles**

* Zero‑copy streaming where possible, backpressure aware.
* Timeouts and circuit breakers on egress.
* Hedged retries (optional) within strict latency budgets.

---

## 5) Functional requirements

### 5.1 Inbound protocol compatibility (MVP)

* **OpenAI**: `/v1/chat/completions` (incl. streaming), `/v1/completions` (optional), `/v1/embeddings` (optional).
* **Anthropic**: `/v1/messages` (incl. streaming).
* Streaming must preserve chunk cadence and keepalive behavior.
* Tool/function calling: pass‑through for OpenAI `tools` and Anthropic `tool_use/tool_result`. MVP supports text‑only content; tools forwarded if target provider supports analogous behavior; otherwise 400 with clear error.

### 5.2 Dialect translation (OpenAI⇄Anthropic)

**Request mapping**

* Roles: `system` + `user` + `assistant` map to Anthropic `system` + `user`/`assistant`.
* Content blocks: text only in MVP; image/audio blocks are non‑goals.
* Parameters: `temperature`, `top_p`/`top_k`, `max_tokens`, `stop`, `stream`, `metadata` carried when supported; unsupported params are ignored or warned.
* Tool/function calling: forwarded if destination supports; otherwise reject or strip according to rule.
* Model names are rewritten per routing rule.

**Response mapping**

* Text output combined into the requester dialect.
* Usage fields unified to `usage.total_tokens`, `prompt_tokens`, `completion_tokens` with best‑effort mapping.
* Error codes mapped to 4xx/5xx JSON with provider detail preserved in `error.details`.

**Streaming mapping**

* OpenAI SSE `data: {object:"chat.completion.chunk", choices:[{delta:{content}}]}` ⇄ Anthropic event stream (`message_start`, `content_block_delta`, `message_delta`, `message_stop`).
* Normalized events: `stream_start`, `delta`, `tool_call`, `tool_result`, `end`. Adapters convert to/from provider streams.
* Flush intervals configurable; line‑delimited SSE for OpenAI; event‑name based for Anthropic.

### 5.3 Basic routing (MVP)

* Match by **listener** (openai|anthropic) and **incoming model** → target `{provider, model}`.
* Optional header or query param overrides: `X-Luna-Route`, `?route=`.
* Fallback list: ordered attempts on failure types `[timeouts, 5xx, rate_limit]`.
* Health checks and circuit breakers per target with exponential backoff.
* Sticky routing by user key when requested.

### 5.4 Session recording

* Write session artifacts to the filesystem under `sessions/YYYY/MM/DD/<tenant>/<session_id>/`.
* Files:

  * `request.json` (normalized request + inbound headers allowlist).
  * `events.ndjson` (lossless streaming transcript with sequence numbers and timestamps).
  * `response.json` (final message(s), usage summary, provider return metadata).
  * `meta.json` (chosen route, fallbacks tried, timing, redaction mode, experiment bucket ids).
* Apply PII redaction/tokenization **before** persisting; token vault can be file-backed (default) with future pluggable KMS/HSM.
* Retention via rolling directory pruning policy (configurable: e.g., 7/30/90 days) and optional compression of `events.ndjson`.
* Export = copy or tarball of the session directory; signed links are a future driver feature.

### 5.5 Stats and analytics

* Per user/model/provider: request count, success rate, p50/p95/p99 latency, tokens in/out, estimated cost, error types.
* Top N prompts by volume (hash only), top error routes, fallback frequency.
* Rolling windows: last 5m/1h/24h; persisted hourly aggregates.

### 5.6 Smart routing v1 (post‑MVP)

* Weighted round‑robin with dynamic weights from health/latency.
* Cost‑aware routing: prefer cheapest under SLO.
* Capacity tags: `tier=free|paid`, `region=us|eu`, `capability=tools|json`.
* Header/param based rules: `X‑User‑Tier`, `X‑Experiment`, `?capability=tools`.
* Sticky A/B buckets by user hash.

### 5.7 System prompt control and A/B

* Prompt patches applied just‑in‑time on the normalized request.
* Rule types: **replace**, **prepend**, **append**, **json‑patch** (RFC6902) against a `system` message.
* Experiments: `% split` across named patches with sticky assignment by user key.
* Audit: patch id recorded in session.

### 5.8 Metrics endpoint

* `/metrics` Prometheus text; counters, histograms (reqs, latency, tokens, egress status, redactions performed).
* Health: `/healthz` liveness, `/readyz` readiness.
* Tracing: W3C TraceContext and OTLP exporters.

### 5.9 Authentication and authorization

* Ingress API keys scoped to tenant, user, and listeners.
* Key metadata: `name`, `scopes` (`openai_ingress`, `anthropic_ingress`), rate limits, budget links.
* Hash keys at rest (bcrypt/argon2); last‑used timestamps.
* Optional JWT/OIDC and mTLS per tenant.

### 5.10 Budgets and enforcement

* Budgets per key, user, or tenant with limits on tokens, requests, or **estimated $ cost**.
* Enforcement actions: throttle, hard reject, reroute to cheaper/free model, or require special header override.
* Budget windows: daily, monthly, rolling 30d.
* Cost estimation via price table per provider/model; versioned and cacheable.

### 5.11 PII substitution/redaction

* Deterministic tokenization using HMAC‑SHA256 with per‑tenant salt.
* Built‑in detectors: email, phone, SSN, credit card (Luhn), IP, names (dictionary), locations; custom regex.
* Modes: **redact** (remove), **tokenize** (reversible via vault), **mask** (partial).
* Apply on ingress before routing; de‑tokenize for session exports only with explicit scope.
* Streaming‑safe: detector works incrementally on chunk boundaries.

### 5.12 Admin APIs and CLI (JSON over HTTP)

* Keys: create, rotate, list, disable.
* Rules: create, list, delete, dry‑run.
* Prompts: upsert patches and experiments.
* Budgets: upsert, get usage.
* Sessions: query, get transcript, export link.
* Price tables: upload and activate version.

### 5.13 Observability

* Structured JSON logs with request id, route, provider, usage, budget action.
* Trace spans around normalization, routing, egress, stream pump.
* Redaction on logs for PII.

### 5.14 Error handling and retries

* Normalize provider errors into a common shape with `provider_code` and `provider_message`.
* Retries disabled by default for non‑idempotent calls; optional hedging on slow start with cap.
* Return 4xx for validation and policy blocks; 5xx when all routes fail; `Retry‑After` when rate limited.

---

## 6) Non‑functional requirements

**Latency targets**

* Added tail latency budget ≤ 20–35 ms p95 over direct provider for non‑streaming.
* First byte on stream ≤ 150 ms p95 after provider start; stream passthrough overhead ≤ 5 ms per chunk.

**Throughput & scale**

* Single node: ≥ 1k RPS sustained with streaming mix 20%.
* Horizontal scale via stateless workers; sticky keys via consistent hash if needed.

**Reliability**

* 99.9% availability target for ingress.
* Graceful shutdown with in‑flight stream drain or cutover.
* Filesystem durability: recommend durable volumes (RAID1/ZFS, or networked FS) and backpressure safeguards when disk is near full.

**Security**

* TLS everywhere; provider keys stored encrypted (KMS/age).
* Secrets never logged; session payloads encrypted at rest.
* Multi‑tenant isolation at policy level; optional network egress allowlist.

---

## 7) Data models (JSON schemas, conceptual)

**Routing rule**

```json
{
  "id": "rule_01",
  "listener": "openai|anthropic",
  "match": { "incoming_model": "gpt-4o", "headers": {"X-User-Tier": "paid"} },
  "target": { "provider": "anthropic", "model": "claude-3.5-sonnet" },
  "fallbacks": [
    { "provider": "openai", "model": "gpt-4o-mini" }
  ],
  "options": { "strip_unsupported_tools": true, "timeout_ms": 20000 }
}
```

**Prompt patch**

```json
{
  "id": "patch_prod_v1",
  "scope": { "tenant": "acme", "listener": "openai" },
  "action": "prepend|append|replace|json-patch",
  "system": "You are Acme’s helpful assistant. Answer concisely.",
  "experiment": { "name": "sys_prompt_ab_1", "buckets": [{"id":"A","pct":50},{"id":"B","pct":50}] }
}
```

**Budget**

```json
{
  "id": "budget_01",
  "scope": { "user_key": "usr_abc" },
  "limits": { "monthly_usd": 50, "daily_tokens": 200000 },
  "actions": ["reroute_to:openai:gpt-4o-mini", "throttle", "reject"]
}
```

**Session record (index)**

```json
{
  "id": "sess_20251001_1234",
  "tenant": "acme",
  "user_key": "usr_abc",
  "listener": "openai",
  "route": {"provider": "anthropic", "model": "claude-3.5-sonnet"},
  "timing_ms": {"ingress_to_egress": 18, "ttfb": 112, "total": 1543},
  "usage": {"prompt_tokens": 512, "completion_tokens": 128, "est_cost_usd": 0.021},
  "pii_mode": "tokenize",
  "payload_ref": "s3://lunaroute/sessions/sess_20251001_1234.ndjson"
}
```

---

## 8) Configuration examples (JSON)

**Listeners and provider endpoints**

**Storage configuration (MVP)**

```json
{
  "storage": {
    "driver": "filesystem",
    "root": "/var/lib/lunaroute"
  }
}
```

**Alternative (future) S3 driver**

```json
{
  "storage": {
    "driver": "s3",
    "bucket": "lunaroute-sessions",
    "prefix": "prod/",
    "region": "us-west-2"
  }
}
```

**Listeners and provider endpoints (example)**

```json
{
  "listeners": [
    {"id":"ingress_openai","type":"openai","bind":"0.0.0.0:8080"},
    {"id":"ingress_anthropic","type":"anthropic","bind":"0.0.0.0:8081"}
  ],
  "providers": [
    {"name":"openai","base_url":"https://api.openai.com","auth":"env:OPENAI_API_KEY"},
    {"name":"anthropic","base_url":"https://api.anthropic.com","auth":"env:ANTHROPIC_API_KEY"}
  ]
}
```

**Basic routing map**

```json
{
  "rules": [
    {
      "listener":"openai",
      "match":{"incoming_model":"gpt-4o"},
      "target":{"provider":"anthropic","model":"claude-3.5-sonnet"},
      "fallbacks":[{"provider":"openai","model":"gpt-4o-mini"}]
    },
    {
      "listener":"anthropic",
      "match":{"incoming_model":"claude-3.5-haiku"},
      "target":{"provider":"openai","model":"gpt-4o-mini"}
    }
  ]
}
```

**PII rules**

```json
{
  "pii": {
    "mode": "tokenize",
    "detectors": ["email","phone","credit_card","ip"],
    "custom_regex": ["(?i)\bssn:?[ -]?([0-9]{3}-[0-9]{2}-[0-9]{4})\b"],
    "salt_ref": "kms:projects/acme/keys/luna-pii"
  }
}
```

---

## 9) Success metrics

* p95 added latency ≤ 35 ms for non‑streaming; p95 stream TTFB ≤ 150 ms.
* 99.9% availability on ingress over 30 days.
* < 0.1% incorrect translation incidents in compatibility test suite.
* Session capture > 99.99% completeness for streams.
* Budget enforcement accuracy within 1% of cost estimates.

---

## 10) Test plan

* **Compatibility**: golden fixtures of OpenAI and Anthropic requests/responses (non‑stream and stream) with byte‑level assertions.
* **Load**: k6/gatling scenarios with 80/20 read/write streams; measure p50/95/99 and backpressure.
* **Chaos**: inject timeouts, 5xx, rate limits; verify fallback and circuit behavior.
* **PII**: redaction/tokenization correctness tests; reversible tokens gated by scope.
* **Budgets**: unit + integration with rolling window counters and cost tables.

---

## 11) Milestones

**MVP (v0.1)**

* OpenAI and Anthropic listeners with streaming.
* Normalization and two egress connectors (OpenAI, Anthropic).
* Basic routing rules + fallbacks.
* Session recording (text only) with encrypted storage.
* Metrics, health, tracing.
* API keys, simple budgets, PII redaction/tokenization.
* CLI + JSON admin APIs.

**v0.2**

* Weighted and cost‑aware routing; sticky A/B.
* Prompt patching with experiments; usage dashboards.
* Embeddings and image gen pass‑through where reasonable.

**v1.0**

* Pluggable provider SDK (Bedrock, Vertex, Azure OpenAI, local engines).
* Admin UI; multi‑region policy sync; disaster recovery playbooks.

---

## 12) Open questions and risks

* **Tool/function parity**: surface area differences between providers can break translations. MVP will gate tools behind feature flags per route.
* **Pricing drift**: cost tables require regular updates. Provide versioned tables and a validator.
* **PII recall**: deterministic tokens are powerful but must be strictly access‑controlled and audited.
* **Legal**: session storage retention and cross‑region data flows may have compliance requirements (GDPR, SOC2).
* **Vendor limits**: strict rate limits and content policies differ; provide clear error mapping and docs.

---

## 13) Developer notes (implementation sketch)

* **Traits**: `Provider` with `send(normalized_req) -> Stream<NormalizedEvent>`; `Storage` with `put(path, bytes)`, `get(path)`, `list(prefix)`, `delete(path)`; default impl `FilesystemStorage`.
* **Config**: Strongly-typed config loader (serde + figment) supporting TOML/JSON/YAML; hot-reload via inotify + admin endpoint; schema validation.
* **Transport**: Hyper client with HTTP/2 where supported, connection pools tuned for low latency.
* **Streaming**: bounded channels for ingress↔egress; heartbeat keepalives; flush on newline.
* **Backpressure**: propagate when downstream is slow; drop policy for oversized queues; disk‑space watermarks guard writes.
* **Security**: secrets from env or file vault; structured logging with redaction middleware.
* **CLI**: `luna validate config ./config` and `luna export session sess_... --out ./out.tgz`.

---

## 14) Appendix: Translation cheat‑sheet (text‑only, MVP)

**Roles**

* OpenAI `system` → Anthropic `system`
* OpenAI `user` → Anthropic `user`
* OpenAI `assistant` → Anthropic `assistant`

**Streaming**

* OpenAI SSE `delta.content` ⇄ Anthropic `content_block_delta` text.
* Start/stop events mapped to normalized `stream_start`/`end`.
* Usage summarized on final event.

**Unsupported examples**

* Images/audio/tool streaming require explicit opt‑in and may force provider‑specific routes.
