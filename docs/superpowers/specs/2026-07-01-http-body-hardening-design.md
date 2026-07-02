# HTTP Body Hardening — Design

**Date:** 2026-07-01
**Scope:** Batch B of the adversarial code-review findings — the HTTP body hardening cluster (issues #3, #4, #13). Fixes the HIGH-severity memory-exhaustion DoS in the bypass proxy, deletes the misleading dead `body_size_limit_middleware`, and fixes a JSON-escaping bug in bypass error responses.
**Status:** Findings independently validated by a Codex GPT-5.5 (xhigh) pass that read the actual source (#3 and the new streaming-header-log site CONFIRMED; #13 PARTIAL — unescaped JSON is real, the URL-leak specifics are unproven but the fix is warranted).

## Context

LunaRoute's bypass proxy (`crates/lunaroute-ingress/src/bypass.rs`) forwards non-intercepted API paths (`/v1/embeddings`, `/v1/audio/*`, `/v1/images/*`, `/v1/files/*`) directly to an upstream provider. It buffers the entire client request body (`axum::body::to_bytes(body, usize::MAX)`) and the entire upstream response (`response.bytes().await`) into RAM. No server-wide body limit is applied (`main.rs:1306` builds `app = api_router.merge(health_router)` with no `DefaultBodyLimit`). A second, dead `body_size_limit_middleware` exists in `middleware.rs`, checks only the `Content-Length` header (bypassable), and has zero call sites — it provides false security.

An attacker who can reach the proxy can POST an arbitrarily large body to a bypassed endpoint, or cause the upstream to return a huge response, exhausting RAM and crashing the proxy process — which holds the configured provider API keys and serves all other users.

## Goals

1. Cap inbound request bodies server-wide at the body-stream level (not the header), so chunked/omitted `Content-Length` cannot bypass it. Covers ALL routes, not just bypass.
2. Eliminate the response-buffering DoS in the bypass proxy by streaming upstream responses back to the client instead of buffering them.
3. Remove the dead, misleading `body_size_limit_middleware` (and its tests) so the enforced limit lives in one place.
4. Fix the `BypassError::IntoResponse` JSON-escaping bug so error responses are always valid JSON regardless of the `Display` content.

## Non-goals

- A response-body size cap. The bypass streams responses true-pass-through (no cap); the request-side limit is the real DoS defense. A `max_response_body_bytes` cap is deferred until a hostile-upstream threat model emerges (noted for future).
- Env-var overrides for the new config field. `HttpServerSettings` has no env-merge today (only `ProviderSettings` does); adding one is out of scope. The limit is file/YAML-configurable, matching the existing `HttpServerSettings` fields.
- Changes to the intercepted (chat/messages) request paths' body handling — those are capped by the same server-wide `DefaultBodyLimit` (Goal 1) but their handler code is untouched.
- The other review batches (retry/rate-limit, secrets-at-rest, async hygiene). Each is its own spec.

---

## Fix 1 (HIGH): Server-wide request body limit + bounded bypass request read

**Issue:** No enforced inbound body limit anywhere. The bypass handler reads the request with `usize::MAX`. A huge POST body OOMs the proxy.

### Config addition

**File:** `crates/lunaroute-server/src/config.rs`, `HttpServerSettings` struct (after the existing `recv_buffer_size` field, ~line 268):

```rust
/// Maximum inbound request body size in bytes. Applies to ALL routes
/// (chat completions, messages, bypass paths). Enforced at the body-stream
/// level by axum's DefaultBodyLimit, so chunked or omitted Content-Length
/// cannot bypass it. Default: 100 MiB (large enough for big multimodal
/// prompts, small enough to stop OOM attacks).
#[serde(default = "default_max_request_body_bytes")]
pub max_request_body_bytes: usize,
```

Add the default function near the other `default_*` functions in the file:
```rust
fn default_max_request_body_bytes() -> usize {
    100 * 1024 * 1024 // 100 MiB
}
```

Add the field to `impl Default for HttpServerSettings` (line ~271):
```rust
max_request_body_bytes: default_max_request_body_bytes(),
```

### App assembly

**File:** `crates/lunaroute-server/src/main.rs`, replace the app assembly (~line 1306):

Before:
```rust
let app = api_router.merge(health_router);
```

After:
```rust
let app = api_router
    .merge(health_router)
    .layer(tower::limit::DefaultBodyLimit::max(
        config.http_server.max_request_body_bytes,
    ));
```

`tower = { version = "0.5", features = ["full"] }` is already a workspace dep; `DefaultBodyLimit` is in `tower::limit`. The layer wraps the body stream — axum rejects any request whose body exceeds the limit, regardless of `Content-Length`.

### Bounded request read (defense-in-depth)

**File:** `crates/lunaroute-ingress/src/bypass.rs`. The `DefaultBodyLimit` layer already caps the request, but the bypass handler's own `to_bytes` should use the same bound so it's correct and survives layer removal. Thread the limit into `BypassProvider`:

Add a field to `BypassProvider` (~line 60):
```rust
pub struct BypassProvider {
    pub base_url: String,
    pub api_key: String,
    pub name: String,
    pub client: Arc<Client>,
    /// Maximum inbound request body size in bytes (mirrors the server-wide
    /// DefaultBodyLimit; used for the bypass handler's own bounded read).
    pub max_request_body_bytes: usize,
}
```

Update `BypassProvider::new` (~line 68) to accept and store it:
```rust
pub fn new(
    base_url: String,
    api_key: String,
    name: String,
    client: Arc<Client>,
    max_request_body_bytes: usize,
) -> Self {
    Self {
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key,
        name,
        client,
        max_request_body_bytes,
    }
}
```

Update the single production call site in `main.rs:1287`:
```rust
Arc::new(BypassProvider::new(
    name,
    base_url,
    api_key,
    /* client */ ,
    config.http_server.max_request_body_bytes,
))
```

In `bypass_handler_impl` (~line 148), change:
```rust
let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
```
to:
```rust
let body_bytes = match axum::body::to_bytes(body, provider.max_request_body_bytes).await {
```
(the `provider` binding is established earlier in the function; move it above this read if needed).

### Why this is the right request-side fix

`DefaultBodyLimit` wraps the body *stream* — axum rejects the request when the stream exceeds the limit, regardless of `Content-Length`. This is the fix for the bypassable header-only check in the dead middleware. It covers every route, not just bypass.

### Tests

- Unit (`config.rs` tests): `HttpServerSettings::default()` has `max_request_body_bytes == 100 * 1024 * 1024`; a YAML with an explicit value overrides it.
- Integration (`bypass_integration.rs` or a new test): a POST to a bypassed path with a body larger than `max_request_body_bytes` returns `413 Payload Too Large` (the `DefaultBodyLimit` rejection). A body under the limit is proxied normally (existing `test_bypass_enabled_proxies_*` tests still pass).
- Unit (`bypass.rs` tests): the existing `BypassProvider::new` test call sites (`bypass.rs:312,333,351,360`) must be updated to pass a `max_request_body_bytes` arg (e.g. `usize::MAX` in tests, or a realistic value).

---

## Fix 2 (HIGH): Stream the bypass response back (eliminate response-buffering DoS)

**Issue:** `proxy_request` does `response.bytes().await?` — buffers the entire upstream response into RAM. An attacker who can make the upstream return a huge body OOMs the proxy.

### Streaming refactor

**File:** `crates/lunaroute-ingress/src/bypass.rs`, `proxy_request` (~lines 244-269). Replace the buffer-and-reconstruct with a streaming response:

Before:
```rust
// Read response body
let response_bytes = response.bytes().await?;

debug!(
    "Bypass proxy response: {} bytes, status: {}",
    response_bytes.len(),
    status
);

// Build response headers, filtering hop-by-hop headers
let mut response_header_map = HeaderMap::new();
for (name, value) in provider_headers.iter() {
    let name_str = name.as_str();
    if is_hop_by_hop_header(name_str) { continue; }
    response_header_map.insert(name.clone(), value.clone());
}

let mut response = Response::new(Body::from(response_bytes.to_vec()));
*response.status_mut() = status;
*response.headers_mut() = response_header_map;
Ok(response)
```

After:
```rust
// Stream the upstream response back to the client without buffering it
// (eliminates the response-side memory-exhaustion DoS: the proxy never
// holds the full response body).
let body_stream = response.bytes_stream();

// Build response headers, filtering hop-by-hop headers
let mut response_header_map = HeaderMap::new();
for (name, value) in provider_headers.iter() {
    let name_str = name.as_str();
    if is_hop_by_hop_header(name_str) { continue; }
    response_header_map.insert(name.clone(), value.clone());
}

let mut response = Response::new(Body::from_stream(body_stream));
*response.status_mut() = status;
*response.headers_mut() = response_header_map;
Ok(response)
```

`axum::body::Body::from_stream<S>` (verified in axum-core 0.5.6) requires `S: TryStream<Ok: Into<Bytes>, Error: Into<BoxError>> + Send + 'static`. `reqwest::Response::bytes_stream()` returns `impl Stream<Item = Result<Bytes, reqwest::Error>>` — a perfect fit. The codebase already uses `bytes_stream()` in 5 places (`openai.rs:1242,2732`, `anthropic.rs:1692`, `egress/anthropic.rs:856`, `egress/openai.rs:1706`).

The `debug!` log of `response_bytes.len()` is removed (we no longer have the length up front). If observability of response size is desired, it can be added later via a wrapping counter stream; not in scope here.

### Behavior contract

- Status and non-hop-by-hop headers from upstream are forwarded unchanged (same as today).
- The response body is streamed; the proxy never holds the full body.
- If the upstream errors mid-stream, reqwest/axum propagates a truncated body to the client. This is the standard streaming-proxy trade-off and matches the existing passthrough paths' behavior. (The old buffered code would have failed with a 502 *before* sending anything; streaming trades a cleaner failure for a mid-flight truncation.)
- No `max_response_body_bytes` cap (true pass-through, per the chosen approach). Noted as a deferred option for a hostile-upstream threat model.

### Tests

- Integration: an existing `test_bypass_enabled_proxies_*` test (e.g. `test_bypass_enabled_proxies_embeddings`) still passes — the streamed response must still deliver the correct body/headers/status to the test client. If the test reads the full body via `resp.bytes().await` it will still work (the stream collects on the client side).
- Integration: add `test_bypass_streams_large_response_without_oom` — mock the upstream to return a body larger than would fit comfortably in a test's memory budget (e.g. 10 MB of repeated bytes), assert the proxy returns it intact (status, a sampled subset of the body, and the content-length header forwarded). This proves streaming works end-to-end. If mocking a 10 MB upstream response is impractical in the existing `wiremock` harness (the existing bypass integration tests use `wiremock`), fall back to a 1 MB body and assert the response body equals the upstream body byte-for-byte — the streaming refactor's correctness is independent of body size; the large-body test is a belt-and-suspenders OOM check. The existing `test_bypass_enabled_proxies_embeddings` / `_audio` / `_images` tests are the primary correctness gate and must still pass.

---

## Fix 3 (LOW): Fix `BypassError` JSON escaping

**Issue:** `impl IntoResponse for BypassError` builds the body by interpolating `self`'s `Display` into a JSON string literal (`format!("{{\"error\":...,\"message\":\"{}\"}}", self)`). `Display` content containing `"` or `\` breaks the JSON or injects fields; for `RequestFailed(reqwest::Error)` the message may include the request URL, leaking the upstream base URL.

### Fix

**File:** `crates/lunaroute-ingress/src/bypass.rs`, `impl IntoResponse for BypassError` (~line 44). Replace:

```rust
let body = format!(
    "{{\"error\": \"bypass_proxy_error\", \"message\": \"{}\"}}",
    self
);
```

with:

```rust
let body = serde_json::json!({
    "error": "bypass_proxy_error",
    "message": self.to_string(),
})
.to_string();
```

`serde_json` is already a workspace dep and used in sibling modules. `serde_json::json!` properly escapes `"` / `\` / control chars. The validator marked the URL-leak framing as PARTIAL (unescaped JSON is real; the URL content of `reqwest::Error`'s Display is not fully proven), but this fix removes the entire escaping class and any URL disclosure regardless.

### Behavior contract

Error responses are now valid JSON for any `Display` content. Status mapping unchanged (`NoProviderAvailable → 503`, others → 502). No change to which errors surface to the client.

### Tests

- Unit: `BypassError::RequestFailed` (or any variant whose `Display` contains a `"` and `\`) serialized via `IntoResponse` produces a body that parses as valid JSON with the expected `error` and `message` fields. Use `serde_json::from_slice` on the response body to assert it parses. The old code would produce un-parseable JSON for a message containing `"`.

---

## Fix 4 (MEDIUM, collapsed): Delete the dead `body_size_limit_middleware`

**Issue:** `body_size_limit_middleware` (`crates/lunaroute-ingress/src/middleware.rs:~70`) checks only the `Content-Length` header (bypassable), never wraps the body stream, and has **zero call sites** outside its own tests. It's dead code that appears to provide DoS protection, giving false security. The real fix (Fix 1, `DefaultBodyLimit`) makes it redundant.

### Fix — delete it

**File:** `crates/lunaroute-ingress/src/middleware.rs`:
- Remove the `body_size_limit_middleware` async fn (~lines 70-84).
- Remove its 5 tests: `test_body_size_limit_within_limit`, `test_body_size_limit_exceeds_limit`, `test_body_size_limit_no_content_length`, `test_body_size_limit_at_limit`, `test_body_size_limit_malformed_content_length` (in the `mod tests` block).

**Re-export check (verified):** `body_size_limit_middleware` is NOT re-exported from `crates/lunaroute-ingress/src/lib.rs` (only `CorsConfig` is, from `middleware`). No external crate references it. Safe to delete.

### Behavior contract

No behavior change (it was never called). The enforced limit now lives in one place: the server-wide `DefaultBodyLimit` layer (Fix 1). Single source of truth, no false-security dead code.

### Tests

Removing the 5 tests is the test change. No replacement tests needed — Fix 1's integration test (413 on oversized body) is the replacement coverage.

---

## Validation status

All three validated findings target code independently CONFIRMED by a Codex GPT-5.5 (xhigh) validator that read the actual source:

| # | Issue | Validator verdict | Evidence (file:line read by validator) |
|---|---|---|---|
| 3 | Bypass buffers unbounded req+resp; no DefaultBodyLimit | CONFIRMED | bypass.rs:148 (`to_bytes(..., usize::MAX)`), :244 (`response.bytes()`), main.rs:1304/1403 (no limit layer) |
| 4 | `body_size_limit_middleware` checks only Content-Length | CONFIRMED | middleware.rs:74-82 (header-only check), :386-399 (no-CL test expects passthrough) |
| 13 | Bypass error JSON interpolation without escaping | PARTIAL | bypass.rs:44-46 (raw interpolation confirmed); `RequestFailed` Display is `reqwest::Error` via `{0}` (bypass.rs:21), URL-leak specifics unproven |

## Rollout

The four fixes are in one PR but ship as a coherent unit (they share code sites: `main.rs` app assembly, `bypass.rs`, `middleware.rs`, `config.rs`). Recommended order within the PR (dependency, soft):

1. **Fix 1** (config + `DefaultBodyLimit` + bounded `BypassProvider`) — the request-side DoS defense.
2. **Fix 2** (stream bypass response) — the response-side DoS elimination.
3. **Fix 3** (`BypassError` JSON escaping) — small, independent.
4. **Fix 4** (delete dead middleware + tests) — cleanup last.

No database migrations. One new config field (`max_request_body_bytes`) with a default — existing configs without it get the 100 MiB default via `serde(default)`. No public API changes (`BypassProvider::new` gains a parameter, but `BypassProvider` is constructed only in `main.rs`; its test call sites update in the same PR).

## Open questions

None at design time. The deferred items (env override for the config field; response-body cap) are explicitly out of scope and noted for future.
