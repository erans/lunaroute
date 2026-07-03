# Retry / Rate-Limit Consistency Implementation Plan (revised)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the backoff overflow panic in `with_retry` for large `max_retries` (issue #14). Issue #8 (Anthropic 429) was already fixed by Task 1 (commit `d32d3fe`); issue #11 was re-evaluated as a misdiagnosis (egress correctly does NOT retry `RateLimitExceeded` — it propagates to the routing layer's failover).

**Architecture:** The routing layer (`lunaroute-routing`) owns 429 retry/failover (reads `retry-after`, switches to an alternative provider). The egress `with_retry` must NOT retry `RateLimitExceeded` — it propagates up. `with_retry` DOES retry transient non-429 errors (network, timeout, 500/502/503/504) on the same provider, using exponential backoff. The backoff expression `2u64.pow(attempt-1)*100` panics for `max_retries ≥ 65`; this plan replaces it with a saturating expression and clamps `max_retries` to 10 in config. No 429-classification change, no loop restructure, no routing-layer change.

**Tech Stack:** Rust 2024, tokio (`time::pause`/`advance` for tests via `features=["full"]`), `tracing` (`warn`), workspace crates `lunaroute-egress`, `lunaroute-server`.

## Global Constraints

- Rust edition 2024, MSRV 1.94 (workspace `Cargo.toml`). rustfmt: max width 100, 4-space, Unix.
- No new dependencies. `tokio` (workspace, `features=["full"]` → includes `test-util`), `tracing`, `lunaroute-egress` already available.
- `with_retry`'s signature `pub async fn with_retry<F, Fut, T>(max_retries: u32, operation: F) -> Result<T>` is UNCHANGED. The 4 production call sites need no edits.
- `EgressError::RateLimitExceeded { retry_after_secs: Option<u64> }` is UNCHANGED — and MUST remain non-retryable in `with_retry` (it propagates to the routing layer). Do NOT add `RateLimitExceeded` to the retryable classification.
- The 429 classification in `with_retry` is UNCHANGED: `ProviderError{429}` stays retryable (defensive fallback for any path that still returns it); `RateLimitExceeded` stays non-retryable (`_ => false`).
- No DB migrations. No new config fields (`max_retries` already exists; we only clamp it).
- **Must not break `limits_alternative_strategy` tests** — they assert 429 failover at the routing layer (`.expect(1)` on the primary). This plan does NOT touch the 429 path, so those tests stay green.

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `crates/lunaroute-egress/src/client.rs` | Modify | Replace `2u64.pow(attempt-1)*100` with saturating `checked_shl`/`saturating_mul`. Add overflow-safety tests. |
| `crates/lunaroute-server/src/config.rs` | Modify | `to_http_client_config` clamps `max_retries` to 10 with a warn log. Add clamp tests. |

---

### Task 2 (revised): Saturating backoff + config clamp (fixes #14)

**Note:** This replaces the original Task 2 (which made `with_retry` retry `RateLimitExceeded` — that change was reverted because it broke the routing layer's 429 failover design). The original Task 1 (Anthropic → `RateLimitExceeded`, commit `d32d3fe`) is DONE and stays. The original Task 3 (config clamp) folds into this task since the backoff + clamp both prevent the same overflow.

**Files:**
- Modify: `crates/lunaroute-egress/src/client.rs` (backoff expression ~line 152, `mod tests`)
- Modify: `crates/lunaroute-server/src/config.rs` (`to_http_client_config` ~line 225, `mod tests`)

**Interfaces:**
- Consumes: `EgressError::ProviderError` (for the retry-path overflow test), `tokio::time::pause`, `tracing::warn`.
- Produces: `with_retry` uses a saturating backoff expression; `to_http_client_config` clamps `max_retries` to 10. No signature changes.

- [ ] **Step 1: Write the failing tests**

**(a) Egress backoff overflow tests** — in `crates/lunaroute-egress/src/client.rs`, inside `mod tests` (after the existing `test_retry_*` tests ~line 256). The crate's `Result<T>` alias takes ONE generic param (`pub type Result<T> = std::result::Result<T, EgressError>`), so use `Result<i32>`, NOT `Result<i32, EgressError>`.

```rust
    #[tokio::test]
    async fn test_backoff_does_not_overflow_for_large_max_retries_success_path() {
        // max_retries=65 would overflow 2u64.pow(64) today (panic in debug).
        // Saturating expression must not panic. Operation succeeds on first
        // attempt, so no actual sleep — this checks the expression compiles
        // and the fn doesn't panic on setup.
        let result: Result<i32> = with_retry(65, || async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_backoff_does_not_overflow_for_large_max_retries_retry_path() {
        // Force the retry path with a non-429 retryable error and max_retries=65.
        // The backoff expression is evaluated on every retry; with the old
        // 2u64.pow(attempt-1) this would panic at attempt=64. Saturating
        // expression must not panic. Uses tokio::time::pause so the sleeps
        // don't block real wall-clock.
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        tokio::time::pause();
        let result: Result<i32> = with_retry(65, move || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                // Non-429 retryable error -> egress retries with exponential backoff.
                Err(EgressError::ProviderError {
                    status_code: 500,
                    message: "Internal error".to_string(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        // All 66 attempts (0..=65) ran without panicking.
        assert_eq!(attempts.load(Ordering::SeqCst), 66);
    }
```

**(b) Config clamp tests** — in `crates/lunaroute-server/src/config.rs`, inside `mod tests` (find via `rg -n 'mod tests' crates/lunaroute-server/src/config.rs`). Mirror the `ProviderSettings` construction in `test_merge_http_client_env_all_variables` (~line 838) for ALL required fields. Read that test first and copy the struct literal shape.

```rust
    #[test]
    fn test_to_http_client_config_clamps_max_retries_to_10() {
        // Construct a ProviderSettings with max_retries: Some(100), mirroring
        // the field set in test_merge_http_client_env_all_variables (~line 838).
        // Fill ALL required fields of ProviderSettings (and its http_client
        // sub-struct) by copying from that existing test.
        let provider = ProviderSettings {
            // ... mirror test_merge_http_client_env_all_variables, but:
            // http_client: Some(HttpClientSettings { max_retries: Some(100), .. }),
            // (and all other fields as in the existing test)
        };
        let config = provider.to_http_client_config();
        assert_eq!(config.max_retries, 10, "max_retries must be clamped to 10");
    }

    #[test]
    fn test_to_http_client_config_preserves_max_retries_under_10() {
        let provider = ProviderSettings {
            // ... same as above, but max_retries: Some(5)
        };
        let config = provider.to_http_client_config();
        assert_eq!(config.max_retries, 5, "max_retries under the ceiling is unchanged");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lunaroute-egress --lib client::tests::test_backoff_does_not_overflow_for_large_max_retries_retry_path`
Expected: FAIL — in debug builds, `2u64.pow(64)` panics at attempt 64 (overflow). In release, it wraps (test passes by accident — but the panic in debug is the bug). The `success_path` test may already pass (no retry → no overflow expression evaluated); it documents the fix.

Run: `cargo test -p lunaroute-server config::tests::test_to_http_client_config_clamps_max_retries_to_10`
Expected: FAIL — today `to_http_client_config` sets `max_retries: self.max_retries.unwrap_or(defaults.max_retries)` with no clamp, so `Some(100)` yields `100`, not `10`.

- [ ] **Step 3: Saturating backoff expression (Fix 2a)**

In `crates/lunaroute-egress/src/client.rs`, in `with_retry` (~line 152), replace:

```rust
            let backoff_ms = 2u64.pow(attempt - 1) * 100; // Exponential backoff: 100ms, 200ms, 400ms
```

with:

```rust
            // Exponential backoff: 100ms, 200ms, 400ms ... saturating to avoid
            // overflow for large max_retries (2u64.pow(64) would panic; checked_shl
            // + saturating_mul caps instead). For realistic max_retries (3-10)
            // values are identical to today.
            let backoff_ms = (1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX)).saturating_mul(100);
```

(`attempt` is `u32`; `u64::checked_shl` takes `u32`, so `1u64.checked_shl(attempt - 1)` compiles — `attempt - 1` is `u32`.) **No other change to `with_retry`** — the loop, the sleep placement, and the 429 classification are unchanged.

- [ ] **Step 4: Config clamp (Fix 2b)**

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

Verify `warn` is in scope: `rg -n 'use tracing' crates/lunaroute-server/src/config.rs`. If `warn` isn't in the existing `use tracing::...` line, add it.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p lunaroute-egress --lib client`
Expected: PASS — both new backoff tests pass (no panic on the retry path with max_retries=65); existing `test_retry_success_first_attempt`, `test_retry_non_retryable_error`, `test_error_display_formatting` still pass.

Run: `cargo test -p lunaroute-server config`
Expected: PASS — both new clamp tests pass; existing `test_merge_http_client_env_*` tests unaffected (they use small max_retries like 5).

- [ ] **Step 6: Run the broader suite — CRITICAL: confirm limits_alternative_strategy still passes**

Run: `cargo test -p lunaroute-egress -p lunaroute-server -p lunaroute_integration_tests --test limits_alternative_strategy`
Expected: PASS — the `limits_alternative_strategy` tests (which assert 429 failover at the routing layer) must be GREEN. This plan does NOT touch the 429 path, so they're unaffected. If any of these fail, STOP — the change went out of scope.

- [ ] **Step 7: Commit**

```bash
git add crates/lunaroute-egress/src/client.rs crates/lunaroute-server/src/config.rs
git commit -m "fix(egress): saturating backoff + clamp max_retries to 10 (overflow fix)

Replace 2u64.pow(attempt-1)*100 in with_retry with a saturating
checked_shl/saturating_mul expression, and clamp max_retries to 10 in
to_http_client_config (with a warn log). The old expression panicked in
debug (wrapped in release) for max_retries>=65. For realistic max_retries
(3-10) backoff values are unchanged. The 429 classification is UNCHANGED:
RateLimitExceeded propagates to the routing layer's failover (not retried
at egress); only transient non-429 errors retry at egress."
```

---

## Final Verification

- [ ] **Run the full workspace test suite:** `cargo test --workspace --all-features`
Expected: PASS (all tests green; only DB/real-API tests ignored as before). The `limits_alternative_strategy` suite must be green (6 tests, ~1-2s — NOT 360s).

- [ ] **Run the exact CI gates locally:**
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-features -- -D warnings`
  - `cargo check --workspace --all-features`
  - `cargo test --workspace --all-features`
Expected: all clean.

## Self-Review

**1. Spec coverage:**
- Fix 1 (Anthropic → `RateLimitExceeded`, fixes #8) → DONE (commit `d32d3fe`, Task 1, reviewed). ✓
- Fix 2a (saturating backoff, fixes #14 overflow) → Task 2 Step 3. ✓
- Fix 2b (config clamp, completes #14) → Task 2 Step 4. ✓
- #11 re-evaluated as misdiagnosis → no code change (documented in spec). ✓
- Tests per fix → Task 2 Steps 1 (tests). ✓

**2. Placeholder scan:** No "TBD"/"TODO". The config test snippet's `// ... mirror test_merge_http_client_env_all_variables` is a directed instruction to copy an existing test's `ProviderSettings` construction (with the exact source test named), not a vague placeholder. ✓

**3. Type consistency:**
- `Result<T>` alias (1 generic param) in egress — Task 2 tests use `Result<i32>`, NOT `Result<i32, EgressError>` (the original Task 2's `Result<i32, EgressError>` was a compile error; corrected here). ✓
- `1u64.checked_shl(attempt - 1)` — `attempt: u32`, `attempt-1: u32`, `u64::checked_shl(u32)` — compiles. ✓
- `max_retries.min(10)` clamp — Task 2 Step 4 clamps to 10, matching the spec's Fix 2b ceiling. ✓
- 429 classification UNCHANGED — `RateLimitExceeded` stays non-retryable (propagates to routing); `ProviderError{429}` stays retryable. ✓

**4. Regression risk:**
- This plan does NOT touch the 429 path, so `limits_alternative_strategy` (routing-layer 429 failover) stays green (Step 6 verifies). ✓
- The backoff expression change is value-identical for realistic max_retries (3-10), so existing retry tests are unaffected. ✓

No issues found. Plan is complete.
