# HTTP Body Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the HTTP body hardening cluster — cap inbound request bodies server-wide, stream bypass responses back (eliminate response-buffering DoS), fix `BypassError` JSON escaping, and delete the dead `body_size_limit_middleware`.

**Architecture:** A server-wide `tower::DefaultBodyLimit` layer caps inbound request bodies at the body-stream level (not the header). The bypass proxy streams upstream responses back via `axum::body::Body::from_stream(response.bytes_stream())` instead of buffering them. `BypassError::IntoResponse` uses `serde_json::json!` for proper escaping. The dead `body_size_limit_middleware` is deleted. One new config field (`max_request_body_bytes`, default 100 MiB) in `HttpServerSettings`.

**Tech Stack:** Rust 2024, axum 0.8 (`axum::body::Body::from_stream`), tower 0.5 (`tower::limit::DefaultBodyLimit`), reqwest (`bytes_stream()`), serde_json, wiremock-style mock provider tests in `lunaroute-integration-tests`.

## Global Constraints

- Rust edition 2024, MSRV 1.94 (workspace `Cargo.toml`). rustfmt: max width 100, 4-space, Unix.
- No new dependencies. `tower` (0.5, `features=["full"]` → includes `DefaultBodyLimit`), `axum` (0.8 → `Body::from_stream`), `reqwest`, `serde_json` are all already workspace deps.
- No DB migrations, no config-breaking changes. The new `max_request_body_bytes` field uses `#[serde(default)]` so existing configs without it get 100 MiB.
- `BypassProvider::new` gains one parameter (`max_request_body_bytes`); it is constructed only in `main.rs` and the integration tests, both updated in this PR. Not a public API boundary (internal type).
- No env-var override for `max_request_body_bytes` (out of scope; `HttpServerSettings` has no env-merge today).
- Tests use the existing `bypass_integration.rs` harness (mock provider via `axum::serve` on `127.0.0.1:0`, `BypassProvider::new`, `with_bypass`, `app.oneshot(request)`).
- Each task ends with `cargo test` green for the affected crate(s) and a commit.

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `crates/lunaroute-server/src/config.rs` | Modify | Add `max_request_body_bytes` field + default fn to `HttpServerSettings`. |
| `crates/lunaroute-server/src/main.rs` | Modify | Apply `DefaultBodyLimit` layer on the app; pass `max_request_body_bytes` to `BypassProvider::new`. |
| `crates/lunaroute-ingress/src/bypass.rs` | Modify | Add `max_request_body_bytes` to `BypassProvider` + `new`; bound `to_bytes` request read; stream response via `Body::from_stream`; fix `BypassError::IntoResponse` JSON escaping. |
| `crates/lunaroute-ingress/src/middleware.rs` | Modify | Delete `body_size_limit_middleware` fn + its 5 tests. |
| `crates/lunaroute-integration-tests/tests/bypass_integration.rs` | Modify | Update `BypassProvider::new` call sites (5th arg); add oversized-body 413 test + large-response streaming test. |

---

### Task 1: Add `max_request_body_bytes` config field (Fix 1a)

**Files:**
- Modify: `crates/lunaroute-server/src/config.rs` (`HttpServerSettings` struct ~line 236, `Default` impl ~line 271, default fns ~line 751)

**Interfaces:**
- Consumes: existing `HttpServerSettings` struct + `#[serde(default = "...")]` pattern.
- Produces: `pub max_request_body_bytes: usize` field on `HttpServerSettings`, default `100 * 1024 * 1024`, accessed as `config.http_server.max_request_body_bytes` by Task 2.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-server/src/config.rs`, inside the existing `mod tests` (find it via `rg -n 'mod tests' crates/lunaroute-server/src/config.rs`), add:

```rust
    #[test]
    fn test_http_server_settings_default_max_request_body_bytes() {
        let settings = HttpServerSettings::default();
        assert_eq!(
            settings.max_request_body_bytes,
            100 * 1024 * 1024,
            "default max_request_body_bytes should be 100 MiB"
        );
    }

    #[test]
    fn test_http_server_settings_yaml_overrides_max_request_body_bytes() {
        let yaml = r#"
tcp_nodelay: true
max_request_body_bytes: 5242880
"#;
        let settings: HttpServerSettings = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(settings.max_request_body_bytes, 5 * 1024 * 1024);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-server --lib config::tests::test_http_server_settings_default_max_request_body_bytes`
Expected: FAIL — compile error `no field max_request_body_bytes on type HttpServerSettings`.

- [ ] **Step 3: Add the field, default fn, and Default impl entry**

In the `HttpServerSettings` struct (after the `recv_buffer_size` field, ~line 268), add:

```rust
    /// Maximum inbound request body size in bytes. Applies to ALL routes
    /// (chat completions, messages, bypass paths). Enforced at the body-stream
    /// level by axum's DefaultBodyLimit, so chunked or omitted Content-Length
    /// cannot bypass it. Default: 100 MiB (large enough for big multimodal
    /// prompts, small enough to stop OOM attacks).
    #[serde(default = "default_max_request_body_bytes")]
    pub max_request_body_bytes: usize,
```

Near the other `default_*` fns (after `default_sse_keepalive_interval_secs`, ~line 759), add:

```rust
fn default_max_request_body_bytes() -> usize {
    100 * 1024 * 1024 // 100 MiB
}
```

In `impl Default for HttpServerSettings` (~line 271), add the field to the `Self { ... }` initializer (after `recv_buffer_size: None,`):

```rust
            max_request_body_bytes: default_max_request_body_bytes(),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-server --lib config`
Expected: PASS — both new tests green; existing `test_http_client_settings_*` and `test_yaml_deserialization_*` tests unaffected (the new field has a serde default).

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-server/src/config.rs
git commit -m "feat(config): add max_request_body_bytes to HttpServerSettings (100 MiB default)"
```

---

### Task 2: Apply server-wide `DefaultBodyLimit` + wire `BypassProvider` (Fix 1b)

**Files:**
- Modify: `crates/lunaroute-ingress/src/bypass.rs` (`BypassProvider` struct ~line 60, `new` ~line 68, `bypass_handler_impl` ~line 148)
- Modify: `crates/lunaroute-server/src/main.rs` (app assembly ~line 1306, `BypassProvider::new` call ~line 1287)

**Interfaces:**
- Consumes: `config.http_server.max_request_body_bytes` from Task 1; `tower::limit::DefaultBodyLimit`.
- Produces: `BypassProvider::new(base_url, api_key, name, client, max_request_body_bytes)` (5-arg). The bypass handler reads the request body with `to_bytes(body, provider.max_request_body_bytes)`.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-integration-tests/tests/bypass_integration.rs`, add a test that an oversized body is rejected. First add the `tower` import if not present (the harness already uses `tower::ServiceExt`). Add after the last `test_bypass_enabled_proxies_*` test:

```rust
#[tokio::test]
async fn test_bypass_rejects_oversized_request_body() {
    let provider_url = mock_provider_server().await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Small limit: 16 bytes.
    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url,
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
        16,
    ));
    let classifier = Arc::new(PathClassifier::new(true));
    let app = Router::new();
    let app = with_bypass(app, Some(bypass_provider), classifier);

    // Body of 64 bytes (well over the 16-byte limit). The bypass handler's
    // own to_bytes bound rejects it before forwarding.
    let big_body = "x".repeat(64);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/embeddings")
        .header("content-type", "application/json")
        .body(Body::from(big_body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "oversized body must be rejected with 413"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-integration-tests --test bypass_integration test_bypass_rejects_oversized_request_body`
Expected: FAIL — compile error: `BypassProvider::new` expects 4 args, got 5.

- [ ] **Step 3: Add `max_request_body_bytes` to `BypassProvider`**

In `crates/lunaroute-ingress/src/bypass.rs`, add the field to the `BypassProvider` struct (~line 60, after `client`):

```rust
pub struct BypassProvider {
    /// Base URL of the provider (e.g., "https://api.openai.com/v1")
    pub base_url: String,
    /// API key for authentication
    pub api_key: String,
    /// Provider name for logging
    pub name: String,
    /// HTTP client
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

Update `bypass_handler_impl` (~line 148): change `to_bytes(body, usize::MAX)` to `to_bytes(body, provider.max_request_body_bytes)`. (The `provider` binding is established at ~line 127 via `let provider = match provider { Some(p) => p, ... }` — confirm it's in scope at the `to_bytes` call; if the binding is shadowed later, use the in-scope name.)

```rust
    let body_bytes = match axum::body::to_bytes(body, provider.max_request_body_bytes).await {
```

- [ ] **Step 4: Update `BypassProvider::new` call site in main.rs and the layer**

In `crates/lunaroute-server/src/main.rs` at the `BypassProvider::new` call (~line 1287), add the 5th arg:

```rust
        Arc::new(BypassProvider::new(
            name,
            base_url,
            api_key,
            /* client Arc */ ,
            config.http_server.max_request_body_bytes,
        ))
```
(Read the existing call to get the exact arg order/names; insert `config.http_server.max_request_body_bytes` as the last arg. Keep the existing `client` arg in its current position.)

Then apply the layer at the app assembly (~line 1306). Replace:

```rust
    let app = api_router.merge(health_router);
```

with:

```rust
    let app = api_router
        .merge(health_router)
        .layer(tower::limit::DefaultBodyLimit::max(
            config.http_server.max_request_body_bytes,
        ));
```

- [ ] **Step 5: Update the existing `BypassProvider::new` call sites in bypass_integration.rs**

In `crates/lunaroute-integration-tests/tests/bypass_integration.rs`, the existing tests (`test_bypass_enabled_proxies_embeddings`, `_audio`, `_images`, and any others using `BypassProvider::new`) pass 4 args today. Add a 5th arg `usize::MAX` (or a realistic value) to each so they compile. For example:

```rust
    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url.clone(),
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
        usize::MAX, // tests: no request-body limit
    ));
```

Update every `BypassProvider::new(...)` in this file the same way (find them with `rg -n 'BypassProvider::new' crates/lunaroute-integration-tests/tests/bypass_integration.rs`).

Also check `crates/lunaroute-ingress/src/bypass.rs` for any test-internal `BypassProvider::new` call sites (the `mod tests` block, ~lines 312, 333, 351, 360) and add the 5th arg there too (`usize::MAX` for tests).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p lunaroute-integration-tests --test bypass_integration`
Expected: PASS — the new `test_bypass_rejects_oversized_request_body` passes (the handler's bounded `to_bytes` returns a 413-mapped error for the 64-byte body with a 16-byte limit), and the existing `test_bypass_enabled_proxies_*` tests still pass (they use `usize::MAX`).

Run: `cargo test -p lunaroute-ingress --lib bypass`
Expected: PASS — the bypass unit tests compile and pass.

Run: `cargo test -p lunaroute-server`
Expected: PASS — main.rs compiles with the layer; existing server tests unaffected.

- [ ] **Step 7: Commit**

```bash
git add crates/lunaroute-ingress/src/bypass.rs crates/lunaroute-server/src/main.rs crates/lunaroute-integration-tests/tests/bypass_integration.rs
git commit -m "fix(ingress): cap inbound request bodies via DefaultBodyLimit + bounded bypass read

Apply tower::DefaultBodyLimit server-wide (100 MiB default, configurable via
HttpServerSettings) so chunked/omitted Content-Length cannot bypass the cap.
The bypass handler's own to_bytes read now uses provider.max_request_body_bytes
(defense-in-depth). BypassProvider::new gains the limit as a 5th arg."
```

---

### Task 3: Stream the bypass response back (Fix 2)

**Files:**
- Modify: `crates/lunaroute-ingress/src/bypass.rs` (`proxy_request` ~lines 244-269)
- Modify: `crates/lunaroute-integration-tests/tests/bypass_integration.rs` (add streaming test)

**Interfaces:**
- Consumes: `reqwest::Response::bytes_stream()`, `axum::body::Body::from_stream`.
- Produces: `proxy_request` returns a streaming `Response` instead of a buffered one.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-integration-tests/tests/bypass_integration.rs`, add a test proving the response is delivered intact (the streaming refactor must not break the existing proxy behavior). Add after the Task 2 test:

```rust
#[tokio::test]
async fn test_bypass_streams_response_body_intact() {
    // Regression: after streaming the upstream response back via
    // Body::from_stream, the client must still receive the full body.
    let provider_url = mock_provider_server().await;
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let bypass_provider = Arc::new(BypassProvider::new(
        provider_url,
        "test-key".to_string(),
        "test-provider".to_string(),
        Arc::new(reqwest::Client::new()),
        usize::MAX,
    ));
    let classifier = Arc::new(PathClassifier::new(true));
    let app = Router::new();
    let app = with_bypass(app, Some(bypass_provider), classifier);

    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/embeddings")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"model":"text-embedding-3-small","input":"test"}"#,
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Collect the streamed body on the client side.
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        body_str.contains("embedding"),
        "streamed response body must contain the upstream payload, got: {body_str}"
    );
}
```

(This test passes today against the buffered code AND after the streaming refactor — it's a non-regression guard, not a RED test. The streaming refactor's correctness is proven by this test + the existing `test_bypass_enabled_proxies_*` tests all staying green.)

- [ ] **Step 2: Stream the response in `proxy_request`**

In `crates/lunaroute-ingress/src/bypass.rs`, in `proxy_request` (~lines 244-269), replace the buffered block. Find this block (the `status` and `provider_headers` bindings above it stay):

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

        // Skip hop-by-hop headers
        if is_hop_by_hop_header(name_str) {
            continue;
        }

        // Clone header to response
        response_header_map.insert(name.clone(), value.clone());
    }

    // Build axum response
    let mut response = Response::new(Body::from(response_bytes.to_vec()));
    *response.status_mut() = status;
    *response.headers_mut() = response_header_map;

    Ok(response)
```

and replace it with:

```rust
    // Stream the upstream response back to the client without buffering it.
    // Eliminates the response-side memory-exhaustion DoS: the proxy never
    // holds the full response body.
    let body_stream = response.bytes_stream();

    debug!("Bypass proxy streaming response back, status: {}", status);

    // Build response headers, filtering hop-by-hop headers
    let mut response_header_map = HeaderMap::new();

    for (name, value) in provider_headers.iter() {
        let name_str = name.as_str();

        // Skip hop-by-hop headers
        if is_hop_by_hop_header(name_str) {
            continue;
        }

        // Clone header to response
        response_header_map.insert(name.clone(), value.clone());
    }

    // Build axum response with a streaming body
    let mut response = Response::new(Body::from_stream(body_stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_header_map;

    Ok(response)
```

(`Body::from_stream` is `axum::body::Body::from_stream<S>` where `S: TryStream<Ok: Into<Bytes>, Error: Into<BoxError>> + Send + 'static`; `reqwest`'s `bytes_stream()` returns exactly that. `Body` and `Bytes` are already imported at the top of `bypass.rs`.)

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p lunaroute-integration-tests --test bypass_integration`
Expected: PASS — the new `test_bypass_streams_response_body_intact` passes, and all `test_bypass_enabled_proxies_*` tests still pass (they collect the body via `to_bytes` on the client side, which works for a streamed body too).

Run: `cargo test -p lunaroute-ingress --lib bypass`
Expected: PASS.

- [ ] **Step 4: Run the broader ingress + integration suite**

Run: `cargo test -p lunaroute-ingress -p lunaroute-integration-tests`
Expected: PASS (no regression in bypass or other ingress tests).

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-ingress/src/bypass.rs crates/lunaroute-integration-tests/tests/bypass_integration.rs
git commit -m "fix(ingress): stream bypass response back instead of buffering

proxy_request now builds the axum response with Body::from_stream(response.bytes_stream())
instead of response.bytes().await + Body::from(vec). The proxy no longer holds the
full upstream response in RAM, eliminating the response-side memory-exhaustion DoS."
```

---

### Task 4: Fix `BypassError` JSON escaping (Fix 3)

**Files:**
- Modify: `crates/lunaroute-ingress/src/bypass.rs` (`impl IntoResponse for BypassError` ~line 37-49)

**Interfaces:**
- Consumes: `serde_json::json!` (already a workspace dep, used in sibling modules).
- Produces: valid-JSON error responses regardless of `Display` content.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-ingress/src/bypass.rs`, inside the `mod tests` block (find via `rg -n 'mod tests' crates/lunaroute-ingress/src/bypass.rs`), add:

```rust
    #[tokio::test]
    async fn bypass_error_with_quote_in_display_produces_valid_json() {
        // A BypassError whose Display contains a double-quote and backslash
        // must serialize to valid JSON, not a broken literal.
        use axum::body::to_bytes;
        use axum::http::StatusCode;

        // RequestFailed wraps a reqwest::Error; constructing one directly is
        // awkward, so use BodyReadError which carries an arbitrary string.
        let err = BypassError::BodyReadError(r#"he said "hi" \n done"#.to_string());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes)
                .expect("error body must be valid JSON even with quotes/backslashes");
        assert_eq!(parsed["error"], "bypass_proxy_error");
        assert_eq!(parsed["message"], r#"he said "hi" \n done"#);
    }
```

(The test is `#[tokio::test] async fn` because `to_bytes(...).await` is async — the existing `bypass.rs` unit tests are sync `#[test]`, but this one needs async. `axum::body::to_bytes` suffices; no `http_body_util` import needed.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-ingress --lib bypass::tests::bypass_error_with_quote_in_display_produces_valid_json`
Expected: FAIL — the current `format!` interpolation produces `{"error":"bypass_proxy_error","message":"he said "hi" \n done"}` which is invalid JSON; `serde_json::from_slice` panics with a parse error.

- [ ] **Step 3: Fix `IntoResponse` to use `serde_json::json!`**

In `crates/lunaroute-ingress/src/bypass.rs`, in `impl IntoResponse for BypassError` (~line 44), replace:

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

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-ingress --lib bypass`
Expected: PASS — the new test passes (valid JSON, escaped quotes/backslashes), and existing bypass tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-ingress/src/bypass.rs
git commit -m "fix(ingress): escape BypassError JSON via serde_json::json!

The error body was built by interpolating the Display string into a JSON
literal with format!, which broke JSON when the message contained quotes or
backslashes (and could leak the upstream URL via reqwest::Error::Display).
Use serde_json::json! for proper escaping."
```

---

### Task 5: Delete the dead `body_size_limit_middleware` (Fix 4)

**Files:**
- Modify: `crates/lunaroute-ingress/src/middleware.rs` (delete `body_size_limit_middleware` fn + its 5 tests)

**Interfaces:**
- Consumes: nothing (dead code).
- Produces: nothing (deletion). Confirmed not re-exported from `lunaroute-ingress/src/lib.rs`.

- [ ] **Step 1: Confirm zero non-test references**

Run: `rg -n 'body_size_limit_middleware' crates --type rust`
Expected: only the definition in `middleware.rs` and its tests in the same file. No call sites in `main.rs`, routers, or other crates. (If any non-test reference appears, STOP and report it — the deletion would break a caller.)

- [ ] **Step 2: Delete the function and its tests**

In `crates/lunaroute-ingress/src/middleware.rs`:

(a) Delete the `body_size_limit_middleware` fn (~lines 70-84):

```rust
/// Middleware to enforce body size limits
pub async fn body_size_limit_middleware(
    req: Request,
    next: Next,
    max_size: usize,
) -> Result<Response, StatusCode> {
    // Check content-length header
    if let Some(content_length) = req.headers().get(header::CONTENT_LENGTH)
        && let Ok(length_str) = content_length.to_str()
        && let Ok(length) = length_str.parse::<usize>()
        && length > max_size
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    Ok(next.run(req).await)
}
```

(b) In the `mod tests` block, delete the 5 tests that reference it: `test_body_size_limit_within_limit`, `test_body_size_limit_exceeds_limit`, `test_body_size_limit_no_content_length`, `test_body_size_limit_at_limit`, `test_body_size_limit_malformed_content_length`. Find them with `rg -n 'fn test_body_size_limit' crates/lunaroute-ingress/src/middleware.rs` and remove each `#[tokio::test] async fn ... { ... }` block in full.

- [ ] **Step 3: Verify the crate still compiles and tests pass**

Run: `cargo test -p lunaroute-ingress --lib middleware`
Expected: PASS — the module compiles without `body_size_limit_middleware`; the remaining middleware tests (CORS, request context, etc.) pass.

Run: `cargo test -p lunaroute-ingress`
Expected: PASS — no other ingress test references the deleted fn.

- [ ] **Step 4: Commit**

```bash
git add crates/lunaroute-ingress/src/middleware.rs
git commit -m "refactor(ingress): delete dead body_size_limit_middleware

The middleware checked only the Content-Length header (bypassable via chunked
transfer or omitted header), never wrapped the body stream, and had zero call
sites outside its own tests — false-security dead code. The server-wide
DefaultBodyLimit (commit <prev>) is the enforced limit. Single source of truth."
```

---

## Final Verification

- [ ] **Run the full workspace test suite:** `cargo test --workspace --all-features`
Expected: PASS (all tests green; only DB/real-API tests ignored as before).

- [ ] **Run the exact CI gates locally:**
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-features -- -D warnings`
  - `cargo check --workspace --all-features`
  - `cargo test --workspace --all-features`
Expected: all clean (these are the 4 CI jobs' commands).

- [ ] **Spot-check the config default:** `cargo run -- ...` or a quick unit test confirms `max_request_body_bytes` defaults to 100 MiB and is overridable via YAML.

## Self-Review

**1. Spec coverage:**
- Fix 1 (server-wide `DefaultBodyLimit` + bounded bypass read) → Task 1 (config field) + Task 2 (layer + `BypassProvider` + bounded `to_bytes`). ✓
- Fix 2 (stream bypass response) → Task 3. ✓
- Fix 3 (`BypassError` JSON escaping) → Task 4. ✓
- Fix 4 (delete dead middleware) → Task 5. ✓
- Tests per fix (regression tests; Task 3 is a non-regression guard since the streaming change preserves observable behavior) → each task. ✓
- Config default + override → Task 1 tests. ✓

**2. Placeholder scan:** No "TBD"/"TODO"/"add appropriate". Every step has concrete code, exact commands, expected output. The two hedged instructions (Task 2 Step 3 "read the existing call to get the exact arg order"; Task 4 Step 1 "if `http_body_util::BodyExt` isn't needed, drop it") give concrete fallbacks. ✓

**3. Type consistency:**
- `max_request_body_bytes: usize` — consistent across Task 1 (config), Task 2 (`BypassProvider` field + `new` arg + `to_bytes` bound + main.rs call + test call sites). ✓
- `BypassProvider::new(base_url, api_key, name, client, max_request_body_bytes)` — 5-arg signature consistent across Task 2 definition, main.rs call, and all test call sites (Task 2 Step 5 + Task 3 test). ✓
- `Body::from_stream` + `response.bytes_stream()` — consistent in Task 3 impl and verified against axum-core 0.5.6. ✓
- `serde_json::json!({ "error": ..., "message": ... })` — consistent in Task 4 impl and test. ✓

No issues found. Plan is complete.
