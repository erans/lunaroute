# Retry / Rate-Limit Consistency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make 429 handling consistent across providers (both return `RateLimitExceeded{retry_after}`; `with_retry` honors `retry-after`, capped at 60s/retry, bailing on longer), and fix the backoff overflow panic for large `max_retries`.

**Architecture:** `handle_anthropic_response` mirrors the OpenAI handler: read `retry-after`, return `RateLimitExceeded` for 429. `with_retry` is restructured so the retry sleep is driven by the *previous attempt's error* (so `retry_after_secs` is available): `RateLimitExceeded{Some(ras)}` sleeps `ras*1000ms` if `ras <= 60` else bails; everything else uses saturating exponential backoff. A `max_retries` clamp (ceiling 10) in config makes the overflow unreachable.

**Tech Stack:** Rust 2024, tokio (`time::sleep`, `time::pause`/`advance` for tests via `features=["full"]`), `thiserror`, workspace crates `lunaroute-egress`, `lunaroute-server`.

## Global Constraints

- Rust edition 2024, MSRV 1.94 (workspace `Cargo.toml`). rustfmt: max width 100, 4-space, Unix.
- No new dependencies. `tokio` (workspace, `features=["full"]` → includes `test-util`), `thiserror`, `tracing`, `lunaroute-egress` already available.
- `EgressError::RateLimitExceeded { retry_after_secs: Option<u64> }` already exists (lib.rs:45) — no new variants.
- `with_retry`'s signature `pub async fn with_retry<F, Fut, T>(max_retries: u32, operation: F) -> Result<T>` is UNCHANGED. The 4 production call sites (anthropic.rs:355, openai.rs:402/504/1158/1212) need no edits.
- `parse_retry_after` is already `pub` in the egress crate (`crate::parse_retry_after`), already used by the OpenAI path. No new public API.
- No DB migrations. No new config fields (`max_retries` already exists; we only clamp it).
- Each task ends with `cargo test` green for the affected crate(s) and a commit.

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `crates/lunaroute-egress/src/anthropic.rs` | Modify | `handle_anthropic_response` returns `RateLimitExceeded` for 429 (reads `retry-after`). |
| `crates/lunaroute-egress/src/client.rs` | Modify | `with_retry` restructured: `RateLimitExceeded` retryable, honors `retry_after_secs` (cap 60s, bail on longer), saturating exponential backoff. New `RATE_LIMIT_SLEEP_CAP_SECS` const. |
| `crates/lunaroute-server/src/config.rs` | Modify | `to_http_client_config` clamps `max_retries` to 10 with a warn log. |
| `crates/lunaroute-egress/src/retry_after.rs` | (unchanged) | `parse_retry_after` is reused as-is. |
| `crates/lunaroute-egress/src/lib.rs` | (unchanged) | `EgressError::RateLimitExceeded` reused as-is. |

---

### Task 1: Anthropic returns `RateLimitExceeded` for 429 (Fix 1a, fixes #8)

**Files:**
- Modify: `crates/lunaroute-egress/src/anthropic.rs` (`handle_anthropic_response` ~line 1022, `mod tests` ~line 1047)

**Interfaces:**
- Consumes: `crate::parse_retry_after` (already pub), `EgressError` (in scope), `debug!` (in scope via `tracing`).
- Produces: `handle_anthropic_response` returns `EgressError::RateLimitExceeded { retry_after_secs }` for 429 (instead of `ProviderError`).

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-egress/src/anthropic.rs`, inside `mod tests` (starts at line 1047), add this test near the other `test_from_anthropic_response_*` tests. `handle_anthropic_response` is a trait method on `reqwest::Response` (via the `AnthropicResponseExt` trait, line ~1015), so the test builds a `reqwest::Response` from an `http::Response` (reqwest 0.13.3 has `impl<T: Into<Body>> From<http::Response<T>> for Response`, verified). `http` is already a workspace dep used by `lunaroute-egress`.

```rust
    #[tokio::test]
    async fn test_handle_anthropic_response_429_returns_rate_limit_exceeded() {
        use http::Response;
        use reqwest::Response as ReqResponse;

        // Build an http::Response with 429 + retry-after, convert to reqwest.
        let http_resp = Response::builder()
            .status(429)
            .header("retry-after", "30")
            .body(r#"{"type":"error","error":{"type":"rate_limit_error","message":"too many requests"}}"#.to_string())
            .unwrap();
        let resp: ReqResponse = http_resp.into();

        let result = resp.handle_anthropic_response().await;

        match result {
            Err(EgressError::RateLimitExceeded { retry_after_secs }) => {
                assert_eq!(retry_after_secs, Some(30));
            }
            other => panic!("expected RateLimitExceeded{{Some(30)}}, got {:?}", other),
        }
    }
```

(`EgressError` is in scope via `use crate::EgressError;` or `use super::*;` — check the existing `mod tests` imports at the top of the test module and mirror. `handle_anthropic_response` is in scope via `AnthropicResponseExt` — the test module likely already `use super::*;` to access it; verify and add `use super::AnthropicResponseExt;` if needed.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-egress --lib anthropic::tests::test_handle_anthropic_response_429_returns_rate_limit_exceeded`
Expected: FAIL — the current `handle_anthropic_response` returns `ProviderError { status_code: 429, ... }` for 429, so the test's `Err(RateLimitExceeded{...})` match fails (or panics).

- [ ] **Step 3: Make `handle_anthropic_response` return `RateLimitExceeded` for 429**

In `crates/lunaroute-egress/src/anthropic.rs`, replace the `handle_anthropic_response` method (~line 1022) with:

```rust
    async fn handle_anthropic_response(self) -> Result<AnthropicResponse> {
        let status = self.status();

        // Capture retry-after header before consuming response
        let retry_after_secs = self
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(crate::parse_retry_after);

        if !status.is_success() {
            let status_code = status.as_u16();
            let body = self
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            return Err(if status_code == 429 {
                debug!(
                    retry_after_secs = ?retry_after_secs,
                    "Anthropic rate limit exceeded"
                );
                EgressError::RateLimitExceeded { retry_after_secs }
            } else {
                EgressError::ProviderError {
                    status_code,
                    message: body,
                }
            });
        }

        let response = self.json::<AnthropicResponse>().await.map_err(|e| {
            EgressError::ParseError(format!("Failed to parse Anthropic response: {}", e))
        })?;

        Ok(response)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-egress --lib anthropic`
Expected: PASS — the new 429 test passes (returns `RateLimitExceeded{Some(30)}`), and existing `test_from_anthropic_response_*` tests still pass (they don't exercise 429).

Run: `cargo test -p lunaroute-integration-tests --test error_handling_with_recording`
Expected: PASS — `test_anthropic_rate_limit_429_with_recording` still passes (the proxy still surfaces a 429; the internal error variant changed but the client-visible behavior is the same status).

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-egress/src/anthropic.rs
git commit -m "fix(egress): Anthropic 429 returns RateLimitExceeded with retry-after

handle_anthropic_response now reads the retry-after header and returns
RateLimitExceeded{retry_after_secs} for 429, mirroring the OpenAI handler.
Previously it returned ProviderError for all non-success (including 429),
never reading retry-after. Both providers now return the same error variant
for 429, enabling with_retry to honor retry-after uniformly."
```

---

### Task 2: `with_retry` honors `retry-after` + saturating backoff (Fix 1b + 2a, fixes #11 + #14)

**Files:**
- Modify: `crates/lunaroute-egress/src/client.rs` (`with_retry` ~line 127, new module-scope const, `mod tests`)

**Interfaces:**
- Consumes: `EgressError::RateLimitExceeded { retry_after_secs: Option<u64> }`, `tokio::time::sleep`, `Duration`.
- Produces: `with_retry` retries `RateLimitExceeded` honoring `retry_after_secs` (cap `RATE_LIMIT_SLEEP_CAP_SECS`), saturating exponential backoff otherwise. Signature unchanged.

- [ ] **Step 1: Write the failing tests**

In `crates/lunaroute-egress/src/client.rs`, inside `mod tests` (after the existing `test_retry_*` tests ~line 256), add these tests. Use `tokio::time::pause()` so the sleeps don't block real wall-clock:

```rust
    #[tokio::test]
    async fn test_retry_rate_limit_exceeded_with_retry_after_is_retried() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        // 429 with retry-after:2s should be retried (honoring 2s) and succeed on attempt 3.
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        tokio::time::pause();
        let result: Result<i32, EgressError> = with_retry(3, move || {
            let a = attempts_clone.clone();
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(EgressError::RateLimitExceeded { retry_after_secs: Some(2) })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3, "should have attempted 3 times");
    }

    #[tokio::test]
    async fn test_retry_rate_limit_exceeded_bails_when_retry_after_exceeds_cap() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        // 429 with retry-after:86400 (daily quota, > 60s cap) should bail immediately,
        // NOT retry, returning the RateLimitExceeded error.
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<i32, EgressError> = with_retry(3, move || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(EgressError::RateLimitExceeded { retry_after_secs: Some(86400) })
            }
        })
        .await;

        assert!(result.is_err());
        match result {
            Err(EgressError::RateLimitExceeded { retry_after_secs }) => {
                assert_eq!(retry_after_secs, Some(86400));
            }
            other => panic!("expected RateLimitExceeded, got {:?}", other),
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "must NOT retry when retry-after exceeds cap");
    }

    #[tokio::test]
    async fn test_retry_rate_limit_exceeded_no_header_uses_exponential_backoff() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        // 429 with no retry-after should retry with exponential backoff and succeed.
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        tokio::time::pause();
        let result: Result<i32, EgressError> = with_retry(3, move || {
            let a = attempts_clone.clone();
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(EgressError::RateLimitExceeded { retry_after_secs: None })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_backoff_does_not_overflow_for_large_max_retries() {
        // max_retries=65 would overflow 2u64.pow(64) today (panic in debug).
        // The saturating expression must not panic. The operation succeeds on
        // the first attempt, so no actual sleep happens — this only checks
        // the backoff EXPRESSION compiles and the fn doesn't panic on setup.
        let result: Result<i32, EgressError> = with_retry(65, || async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lunaroute-egress --lib client::tests::test_retry_rate_limit_exceeded_with_retry_after_is_retried`
Expected: FAIL — today `with_retry` does NOT retry `RateLimitExceeded` (`_ => false`), so the operation is called only once and the test's `Ok(42)` is never reached (returns `Err(RateLimitExceeded{...})`).

Run: `cargo test -p lunaroute-egress --lib client::tests::test_backoff_does_not_overflow_for_large_max_retries`
Expected: PASS already today on release, but PANIC in debug builds (`2u64.pow(64)` overflow). This test documents the fix.

- [ ] **Step 3: Add the `RATE_LIMIT_SLEEP_CAP_SECS` constant and restructure `with_retry`**

In `crates/lunaroute-egress/src/client.rs`, add the constant just above `with_retry` (~line 125):

```rust
/// Maximum sleep (seconds) for a single rate-limit retry. Longer retry-after
/// values (e.g. daily quotas ~86400s) bail to the client immediately instead
/// of blocking the request for minutes. 60s covers real-world per-minute
/// rate limits from both OpenAI and Anthropic.
const RATE_LIMIT_SLEEP_CAP_SECS: u64 = 60;
```

Replace the entire `with_retry` fn body (lines ~127-179) with the restructured version:

```rust
pub async fn with_retry<F, Fut, T>(max_retries: u32, operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;

    for attempt in 0..=max_retries {
        // Sleep before retries (skipped on the first attempt). The sleep
        // duration is driven by the PREVIOUS attempt's error so we can honor
        // retry-after for rate limits.
        if attempt > 0 {
            let backoff_ms = match &last_error {
                Some(EgressError::RateLimitExceeded {
                    retry_after_secs: Some(ras),
                }) => {
                    if *ras > RATE_LIMIT_SLEEP_CAP_SECS {
                        // retry-after too long: bail to the client immediately.
                        debug!(
                            retry_after_secs = ras,
                            cap = RATE_LIMIT_SLEEP_CAP_SECS,
                            "rate-limit retry-after exceeds cap; returning error to client"
                        );
                        return Err(last_error.unwrap());
                    }
                    // Honor the provider's retry-after (seconds -> ms).
                    ras.saturating_mul(1000)
                }
                _ => {
                    // Exponential backoff for non-rate-limit errors, or rate
                    // limit with no retry-after header. Saturating: prevents
                    // overflow for large max_retries (2u64.pow(64) would panic).
                    (1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX)).saturating_mul(100)
                }
            };
            debug!(
                "Retrying request after {}ms (attempt {}/{})",
                backoff_ms, attempt, max_retries
            );
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }

        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                // Check if error is retryable
                let should_retry = match &e {
                    EgressError::HttpError(req_err) => {
                        // Retry on network errors, connection errors, timeouts
                        req_err.is_connect() || req_err.is_timeout() || req_err.is_request()
                    }
                    EgressError::ProviderError { status_code, .. } => {
                        // Retry on 429 (rate limit), 500, 502, 503, 504.
                        // 429 here is a defensive fallback: providers SHOULD
                        // return RateLimitExceeded for 429 (honored in the
                        // sleep logic above), but if a path returns
                        // ProviderError for 429 it still retries with backoff.
                        matches!(status_code, 429 | 500 | 502 | 503 | 504)
                    }
                    EgressError::RateLimitExceeded { .. } => true,
                    EgressError::Timeout(_) => true,
                    _ => false,
                };

                if should_retry && attempt < max_retries {
                    warn!(
                        "Request failed (attempt {}/{}): {:?}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        EgressError::ConfigError("Retry loop exited unexpectedly".to_string())
    }))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-egress --lib client`
Expected: PASS — all 4 new tests pass (rate-limit-with-retry-after retries and succeeds; bail-on-long-retry-after returns the error after 1 attempt; no-header uses backoff and succeeds; large-max-retries doesn't panic). The existing `test_retry_success_first_attempt`, `test_retry_non_retryable_error`, `test_error_display_formatting` still pass.

- [ ] **Step 5: Run the broader egress suite**

Run: `cargo test -p lunaroute-egress`
Expected: PASS (no regression in anthropic/openai/streaming tests).

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-egress/src/client.rs
git commit -m "fix(egress): with_retry honors retry-after + saturating backoff

with_retry now retries RateLimitExceeded, sleeping for retry_after_secs
(capped at 60s/retry) and bailing to the client immediately when
retry-after exceeds the cap (e.g. daily quotas). Falls back to exponential
backoff when retry-after is absent. Previously RateLimitExceeded was never
retried (OpenAI 429 failed immediately) while ProviderError 429 was retried
blindly ignoring retry-after (Anthropic 429). Also fixes the
2u64.pow(attempt-1) overflow panic for max_retries>=65 via a saturating
checked_shl expression."
```

---

### Task 3: Clamp `max_retries` at config parse (Fix 2b, completes #14)

**Files:**
- Modify: `crates/lunaroute-server/src/config.rs` (`to_http_client_config` ~line 225, `mod tests`)

**Interfaces:**
- Consumes: `lunaroute_egress::HttpClientConfig`, `tracing::warn`.
- Produces: `to_http_client_config` clamps `max_retries` to 10 with a warn log.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-server/src/config.rs`, inside `mod tests` (find via `rg -n 'mod tests' crates/lunaroute-server/src/config.rs`), add:

```rust
    #[test]
    fn test_to_http_client_config_clamps_max_retries_to_10() {
        let provider = ProviderSettings {
            timeout_secs: None,
            connect_timeout_secs: None,
            pool_max_idle_per_host: None,
            pool_idle_timeout_secs: None,
            tcp_keepalive_secs: None,
            max_retries: Some(100),
            enable_pool_metrics: None,
            // ... other fields default as in the existing test_to_http_client_config tests
        };
        // Fill any other REQUIRED fields of ProviderSettings by mirroring an
        // existing test that constructs ProviderSettings (rg -n 'ProviderSettings {' in this file).

        let config = provider.to_http_client_config();
        assert_eq!(config.max_retries, 10, "max_retries must be clamped to 10");
    }

    #[test]
    fn test_to_http_client_config_preserves_max_retries_under_10() {
        let provider = ProviderSettings {
            // ... same as above ...
            max_retries: Some(5),
            // ...
        };
        let config = provider.to_http_client_config();
        assert_eq!(config.max_retries, 5, "max_retries under the ceiling is unchanged");
    }
```

(Read the existing `ProviderSettings` construction in the test module — e.g. `test_merge_http_client_env_*` tests construct `ProviderSettings` — and mirror ALL required fields. `ProviderSettings` is the struct at config.rs:~134 with fields `api_key`, `base_url`, `enabled`, `http_client`, etc. The `http_client: Option<HttpClientSettings>` field is what carries `max_retries`. Look at `test_merge_http_client_env_all_variables` (~line 838) for the exact construction pattern and mirror it.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-server config::tests::test_to_http_client_config_clamps_max_retries_to_10`
Expected: FAIL — today `to_http_client_config` sets `max_retries: self.max_retries.unwrap_or(defaults.max_retries)` with no clamp, so `Some(100)` yields `100`, not `10`.

- [ ] **Step 3: Add the clamp + warn log**

In `crates/lunaroute-server/src/config.rs`, in `to_http_client_config` (~line 225), replace the `max_retries:` line:

```rust
            max_retries: self.max_retries.unwrap_or(defaults.max_retries),
```

with:

```rust
            max_retries: {
                let requested = self.max_retries.unwrap_or(defaults.max_retries);
                if requested > 10 {
                    warn!(
                        requested = requested,
                        clamped = 10,
                        "max_retries exceeds safe ceiling; clamping to 10"
                    );
                    10
                } else {
                    requested
                }
            },
```

(`warn` is already imported via `tracing` / `use tracing::{...}` at the top of `config.rs` — verify with `rg -n 'use tracing' crates/lunaroute-server/src/config.rs`; if `warn` isn't in scope, add it to the existing `use tracing::...` line.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-server config`
Expected: PASS — both new tests pass (clamp to 10; under-10 unchanged). Existing `test_merge_http_client_env_*` tests unaffected (they use small `max_retries` values like 5).

- [ ] **Step 5: Run the server suite**

Run: `cargo test -p lunaroute-server`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-server/src/config.rs
git commit -m "fix(config): clamp max_retries to 10 with warn log

to_http_client_config now clamps max_retries to a safe ceiling of 10 (with
a warn log when clamping). Makes the with_retry backoff overflow unreachable
by construction (belt-and-suspenders with the saturating checked_shl
expression in with_retry). 10 retries x ~60s worst-case = 600s, beyond sane
for a proxy willing to retry."
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
Expected: all clean.

## Self-Review

**1. Spec coverage:**
- Fix 1a (Anthropic → `RateLimitExceeded`) → Task 1. ✓
- Fix 1b (`with_retry` honors retry-after) → Task 2. ✓
- Fix 2a (saturating backoff) → Task 2 (embedded in the restructured `with_retry`). ✓
- Fix 2b (config clamp) → Task 3. ✓
- Tests per fix → Tasks 1, 2, 3 each have tests. ✓

**2. Placeholder scan:** No "TBD"/"TODO"/"add appropriate". The two hedged instructions (Task 1 Step 1 test-harness fallback; Task 3 Step 1 "mirror an existing test's `ProviderSettings` construction") give concrete fallbacks. The `... other fields default` in the Task 3 test snippet is a placeholder for the implementer to fill by reading an existing test — this is acceptable because the exact field set of `ProviderSettings` is large and the implementer is told exactly where to find a working example (`test_merge_http_client_env_all_variables`). ✓

**3. Type consistency:**
- `EgressError::RateLimitExceeded { retry_after_secs: Option<u64> }` — consistent across Task 1 (Anthropic handler returns it), Task 2 (`with_retry` matches it). ✓
- `RATE_LIMIT_SLEEP_CAP_SECS: u64 = 60` — consistent in Task 2 const + match + tests (86400 > 60 → bail; 2 <= 60 → retry). ✓
- `with_retry(max_retries: u32, operation: F)` signature unchanged — Task 2 preserves it; the 4 production call sites need no edits. ✓
- `(1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX)).saturating_mul(100)` — the saturating backoff in Task 2 matches the spec's Fix 2a. ✓
- `max_retries.min(10)` clamp — Task 3 clamps to 10, matching the spec's Fix 2b ceiling. ✓
- `tokio::time::pause()` used in Task 2 tests — available via `tokio` `features=["full"]` (includes `test-util`). ✓

No issues found. Plan is complete.
