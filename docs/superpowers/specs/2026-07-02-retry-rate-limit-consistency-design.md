# Retry / Rate-Limit Consistency — Design

**Date:** 2026-07-02
**Scope:** Batch C of the adversarial code-review findings — retry/rate-limit consistency (issues #8, #11, #14). Fixes two opposite 429-handling bugs (Anthropic 429 retried blindly ignoring `retry-after`; OpenAI 429 never retried at all) and a backoff overflow panic for large `max_retries`.
**Status:** Findings independently validated by a Codex GPT-5.5 (xhigh) pass that read the actual source (all three CONFIRMED).

## Context

LunaRoute's egress layer retries transient upstream failures in `with_retry` (`crates/lunaroute-egress/src/client.rs:127`). The two providers disagree on how to surface 429:

- **OpenAI** (`handle_openai_response` at `openai.rs:1829`, `handle_openai_passthrough_response` at `openai.rs:596`): reads the `retry-after` header and returns `EgressError::RateLimitExceeded { retry_after_secs }` for 429.
- **Anthropic** (`handle_anthropic_response` at `anthropic.rs:1022`): returns `EgressError::ProviderError { status_code, message }` for ALL non-success statuses (including 429), never reading `retry-after`.

`with_retry` then classifies errors: `ProviderError` with `status_code` 429 is retryable (retried with fixed exponential backoff, ignoring any retry-after), but `RateLimitExceeded` is NOT in the retryable list (`_ => false`), so OpenAI 429 is never retried. Net result: **Anthropic 429 is retried blindly (ignoring `retry-after`); OpenAI 429 is not retried at all.** Both are wrong, in opposite directions.

Additionally, the backoff expression `2u64.pow(attempt - 1) * 100` (client.rs:152) panics in debug (wraps in release) when `max_retries >= 65` — a misconfiguration that no config validation clamps.

## Goals

1. **Unified 429 policy:** 429 from either provider → the response handler returns `EgressError::RateLimitExceeded { retry_after_secs }` (reading the `retry-after` header). Both providers behave identically.
2. **`with_retry` honors `retry-after`:** retry `RateLimitExceeded` sleeping for `retry_after_secs` (capped at 60s per retry); fall back to (fixed, non-overflowing) exponential backoff when `retry_after` is absent; bail early (return the error) when `retry_after` exceeds the cap, so a daily-quota 429 (86400s) fails fast to the client instead of blocking for minutes.
3. **Fix the backoff overflow:** saturating exponentiation + a config clamp on `max_retries` (ceiling 10) so the panic is unreachable by construction.

## Non-goals

- Changes to the non-429 error paths (`ProviderError` 500/502/503/504, `HttpError`, `Timeout`) — their retry behavior is unchanged.
- A cumulative sleep budget across retries — `max_retries` (clamped to ≤10) already bounds the count; 10 × 60s = 600s worst case is acceptable for a proxy willing to retry. No separate cumulative cap.
- Changes to `parse_retry_after` (already solid: handles numeric + HTTP-date, caps at 48h). The Anthropic path just needs to *call* it.
- The other review batches (secrets-at-rest, async hygiene). Each is its own spec.

---

## Fix 1 (MEDIUM × 2): Unified 429 policy + retry-after-honoring retry (fixes #8 + #11)

### Change 1a — Anthropic returns `RateLimitExceeded` for 429 (fixes #8)

**File:** `crates/lunaroute-egress/src/anthropic.rs`, `handle_anthropic_response` (~line 1022).

Mirror the OpenAI handler's pattern: capture `retry-after` before consuming the body, return `RateLimitExceeded` for 429, `ProviderError` for other non-success.

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

`parse_retry_after` is `crate::parse_retry_after` (already public, already used by the OpenAI path). `debug!` and `EgressError` are already in scope. This makes both providers return the same error variant for 429.

### Change 1b — `with_retry` retries `RateLimitExceeded` honoring retry-after (fixes #11)

**File:** `crates/lunaroute-egress/src/client.rs`, `with_retry` (~line 127).

**New module-scope constant:**
```rust
/// Maximum sleep (seconds) for a single rate-limit retry. Longer retry-after
/// values (e.g. daily quotas ~86400s) bail to the client immediately instead
/// of blocking the request for minutes. 60s covers real-world per-minute
/// rate limits from both OpenAI and Anthropic.
const RATE_LIMIT_SLEEP_CAP_SECS: u64 = 60;
```

**Restructure the loop** so the sleep is driven by the *error* (which carries `retry_after_secs`), not computed before the operation. Today the loop sleeps at the top (before the operation, based only on `attempt`); the retry-after comes from the error at the bottom. The restructure moves the sleep to *after* the failed operation:

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
                        // retry-after too long: bail to the client immediately
                        debug!(
                            retry_after_secs = ras,
                            cap = RATE_LIMIT_SLEEP_CAP_SECS,
                            "rate-limit retry-after exceeds cap; returning error to client"
                        );
                        return Err(last_error.unwrap());
                    }
                    // Honor the provider's retry-after (seconds -> ms)
                    ras.saturating_mul(1000)
                }
                _ => {
                    // Exponential backoff for non-rate-limit errors, or
                    // rate-limit with no retry-after header.
                    // Saturating: prevents overflow for large max_retries.
                    (1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX))
                        .saturating_mul(100)
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
                        req_err.is_connect() || req_err.is_timeout() || req_err.is_request()
                    }
                    EgressError::ProviderError { status_code, .. } => {
                        // Retry on 429 (rate limit), 500, 502, 503, 504.
                        // 429 here is a defensive fallback: providers SHOULD
                        // return RateLimitExceeded for 429 (which has its own
                        // arm below), but if a path returns ProviderError for
                        // 429 it still retries with exponential backoff.
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

**Key behaviors of the restructure:**
- The sleep now happens at the *top* of each retry iteration (still before the operation), but its duration is computed from `last_error` (the previous attempt's error) — so retry-after is honored. `last_error` is already set at the bottom of the loop today; this just reads it at the top of the next iteration.
- `RateLimitExceeded { retry_after_secs: Some(ras) }` with `ras > 60` → bail immediately (return the error), don't sleep.
- `RateLimitExceeded { retry_after_secs: Some(ras) }` with `ras <= 60` → sleep `ras * 1000` ms.
- `RateLimitExceeded { retry_after_secs: None }` → exponential backoff (no header to honor).
- `ProviderError` 429 (defensive fallback) → exponential backoff (no retry-after available on this variant).
- Other retryable errors → exponential backoff.

### Behavior contract

| Scenario | Today | After |
|---|---|---|
| Anthropic 429, `retry-after: 2s` | retried blindly at 100/200/400ms (ignores header) → likely more 429s | retried at 2s, 2s, 2s (honors header) up to `max_retries` |
| OpenAI 429, `retry-after: 2s` | NOT retried → immediate client failure | retried at 2s up to `max_retries` |
| Either 429, `retry-after: 50s` | Anthropic: blind retry; OpenAI: bail | retried at 50s (under 60s cap) |
| Either 429, `retry-after: 86400s` (daily quota) | Anthropic: blind retry; OpenAI: bail | bail immediately, return `RateLimitExceeded { retry_after_secs: Some(86400) }` to client |
| Either 429, no `retry-after` header | Anthropic: blind exponential; OpenAI: bail | exponential backoff (100/200/400ms) for both |
| Non-429 retryable (500/502/503/504, timeout, network) | exponential backoff | unchanged (exponential backoff) |

---

## Fix 2 (LOW): Backoff overflow + config clamp (fixes #14)

**File:** `crates/lunaroute-egress/src/client.rs` (backoff expression, now inside the restructured `with_retry`) + `crates/lunaroute-server/src/config.rs` (`max_retries` clamp).

### Change 2a — Saturating backoff expression

The exponential backoff (used for non-rate-limit errors, and rate-limit with no header) is now (per Fix 1b's code):

```rust
(1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX)).saturating_mul(100)
```

`checked_shl(n)` returns `Some(1 << n)` for `n < 64`, `None` for `n >= 64` → `unwrap_or(u64::MAX)` caps the base; `saturating_mul(100)` caps the product. For realistic `max_retries` (3-10) the value is identical to today (`100, 200, 400...`). For pathological `max_retries >= 65` it saturates to `u64::MAX` ms instead of panicking/wrapping.

### Change 2b — Clamp `max_retries` at config parse (ceiling 10)

**File:** `crates/lunaroute-server/src/config.rs`, in `to_http_client_config` (~line 225, where `max_retries: self.max_retries.unwrap_or(defaults.max_retries)` is set):

```rust
max_retries: self.max_retries.unwrap_or(defaults.max_retries).min(10),
```

Add a warn log when clamping (compare the unwrapped value to 10):

```rust
let max_retries = self.max_retries.unwrap_or(defaults.max_retries);
if max_retries > 10 {
    warn!(
        requested = max_retries,
        clamped = 10,
        "max_retries exceeds safe ceiling; clamping to 10"
    );
}
// then use max_retries.min(10) in the struct literal
```

Ceiling 10: 10 retries × ~60s worst-case rate-limit sleep = 600s, already beyond sane for a proxy. The clamp makes the overflow unreachable by construction (10 << 64 is far from overflow); the saturating expression (2a) is belt-and-suspenders.

### Behavior contract

- `max_retries: 3` (default) → backoff `100, 200, 400` ms — unchanged.
- `max_retries: 10` (the ceiling) → backoff `100, 200, 400, 800, ...` up to `51200` ms — no overflow.
- `max_retries: 100` (would panic today) → clamped to 10 with a warn log; no panic.
- Even without the clamp (e.g. a future caller bypassing config), the saturating expression won't panic.

---

## Validation status

All three findings independently CONFIRMED by a Codex GPT-5.5 (xhigh) validator that read the actual source:

| # | Issue | Validator verdict | Evidence (file:line read by validator) |
|---|---|---|---|
| 8 | `with_retry` retries Anthropic 429 ignoring `retry-after` | CONFIRMED | client.rs:138-157 (retries `ProviderError` 429 with fixed backoff); anthropic.rs:1004-1014 (429→`ProviderError`, no retry-after reading) |
| 11 | OpenAI passthrough 429 `RateLimitExceeded` never retried; `ProviderError` 429 is | CONFIRMED | client.rs:155-160 (`_ => false` for `RateLimitExceeded`); openai.rs:616-621 (returns `RateLimitExceeded`) |
| 14 | `2u64.pow(attempt-1)` backoff overflow panic for `max_retries ≥ 65` | CONFIRMED | client.rs:138-139 (the `pow`); config.rs:174-194 (`max_retries: Option<u32>` unclamped) |

## Rollout

The fixes are in one PR but ship as a coherent unit (they share `with_retry` and the response handlers, and #8 + #11 are interdependent — fixing #8 without #11 would make Anthropic 429 unretried, worse than today). Recommended order within the PR (dependency, soft):

1. **Fix 1a** (Anthropic → `RateLimitExceeded`) + **Fix 1b** (`with_retry` honors retry-after) together — they must land as a unit to avoid the transient-worse intermediate state.
2. **Fix 2** (saturating backoff + config clamp) — folds into the `with_retry` restructure naturally (the new backoff expression is part of Fix 1b's code).

No database migrations. No new config fields (`max_retries` already exists; we only clamp it). No public API changes (`EgressError::RateLimitExceeded` already exists; `with_retry`'s signature is unchanged).

## Open questions

None at design time. The two behavioral decisions (60s per-retry cap; no cumulative cap; clamp ceiling 10) were made during brainstorming.
