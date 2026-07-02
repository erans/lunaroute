# Retry / Rate-Limit Consistency — Design (revised)

**Date:** 2026-07-02 (revised after SDD Task 2 surfaced an architecture conflict)
**Scope:** Batch C of the adversarial code-review findings — retry/rate-limit consistency (issues #8, #14; #11 re-evaluated).
**Status:** Findings independently validated by a Codex GPT-5.5 (xhigh) pass. The original design was revised after Task 2's implementation broke 6 routing integration tests, revealing that the routing layer — not the egress `with_retry` — is the intended owner of 429 retry/failover.

## Context (corrected)

LunaRoute has TWO layers that touch 429:

- **Egress `with_retry`** (`crates/lunaroute-egress/src/client.rs:127`): retries transient upstream errors (network, timeout, 500/502/503/504) on the SAME provider. It classifies errors: `ProviderError{429}` is retryable (blind exponential backoff, ignores `retry-after`); `RateLimitExceeded{...}` is NOT retryable (`_ => false`) — it propagates up.
- **Routing layer** (`crates/lunaroute-routing/src/provider_router.rs:255`, `strategy.rs`): when a request fails with `Error::RateLimitExceeded { retry_after_secs }`, the router records the provider as rate-limited (`record_rate_limit(retry_after_secs, ...)`), marks it rate-limited until `retry-after` expires, and **switches to an alternative provider** (`limits_alternative_strategy`). This is a complete, retry-after-aware 429 failover system at the routing layer.

The two providers disagreed on how to surface 429:
- **OpenAI** (`handle_openai_response`): reads `retry-after`, returns `RateLimitExceeded { retry_after_secs }` → propagates to routing → failover. ✅ correct
- **Anthropic** (`handle_anthropic_response`): returned `ProviderError { status_code: 429, ... }` for ALL non-success (including 429), never reading `retry-after` → egress `with_retry` retries it blindly (3× exponential backoff, ignoring retry-after) on the SAME provider → the routing layer never sees `RateLimitExceeded`, so **no failover**. ❌ the actual bug

## What the original review found vs what's really true

| # | Original finding | Re-evaluation |
|---|---|---|
| #8 | "Anthropic 429 retried blindly, ignores retry-after" | **CONFIRMED, real bug.** Anthropic returns `ProviderError` for 429, so egress retries it blindly (no failover, ignores retry-after). Fix: Anthropic returns `RateLimitExceeded` (Task 1). |
| #11 | "OpenAI 429 (`RateLimitExceeded`) never retried" | **MISDIAGNOSED.** It is *correct* that egress does NOT retry `RateLimitExceeded` — that error is meant to propagate to the routing layer, which does retry-after-aware failover (a better behavior than same-provider retry). The asymmetry was NOT "OpenAI unretried vs Anthropic retried" — it was "OpenAI correctly propagates to routing; Anthropic wrongly retries at egress." Task 1 (Anthropic → `RateLimitExceeded`) fixes the asymmetry by making Anthropic also propagate to routing. **No `with_retry` change needed for 429.** |
| #14 | "backoff `2u64.pow(attempt-1)` overflow panic for max_retries ≥ 65" | **CONFIRMED, real bug.** The overflow is in the non-429 retry path (500/502/503/504, network, timeout). Fix: saturating expression + config clamp. |

The original design's "make `with_retry` retry `RateLimitExceeded` honoring retry-after" was **wrong**: it would make the egress layer retry the same rate-limited provider (sleeping up to 60s × 3) before the routing layer could failover to an alternative — defeating the routing layer's failover design, breaking the `limits_alternative_strategy` tests (which assert `.expect(1)` on the primary — exactly 1 request then switch), and making tests take 360s (CI timeout).

## Goals (revised)

1. **Unified 429 propagation:** 429 from either provider → the egress response handler returns `EgressError::RateLimitExceeded { retry_after_secs }` (reading `retry-after`), which `with_retry` does NOT retry — it propagates to the routing layer for retry-after-aware failover. (Task 1 — DONE, commit `d32d3fe`.)
2. **Fix the backoff overflow (#14):** saturating exponentiation + config clamp on `max_retries` (ceiling 10) so the panic is unreachable. The backoff is used for the non-429 retryable errors (500/502/503/504, network, timeout) — those still retry at egress (correct: same-provider retry for transient errors). This does NOT touch the 429 path.

## Non-goals (revised)

- **Do NOT make `with_retry` retry `RateLimitExceeded`.** `RateLimitExceeded` must propagate to the routing layer (which does failover). The original design's retry-after-honoring retry at egress is dropped — it conflicts with the routing layer's failover.
- No changes to the routing layer's rate-limit/failover (`provider_router.rs`, `strategy.rs`) — it's already correct and complete.
- No changes to `parse_retry_after` (already solid, used by both handler paths + routing).
- The other review batches (secrets-at-rest, async hygiene). Each is its own spec.

---

## Fix 1 (MEDIUM, fixes #8): Anthropic returns `RateLimitExceeded` for 429 — DONE

**Status:** Implemented and reviewed (commit `d32d3fe`, Task 1 of this batch). `handle_anthropic_response` now reads `retry-after` and returns `RateLimitExceeded { retry_after_secs }` for 429 (mirroring OpenAI). Both providers now propagate 429 to the routing layer uniformly.

**Net effect:** Anthropic 429 now triggers routing-layer failover (to an alternative provider) instead of being blind-retried at egress. This is the real fix for #8 — and it simultaneously resolves the #11 asymmetry (both providers now behave identically: `RateLimitExceeded` propagates to routing).

## Fix 2 (LOW, fixes #14): Saturating backoff + config clamp

**Location:** `crates/lunaroute-egress/src/client.rs` (backoff expression at line 152) + `crates/lunaroute-server/src/config.rs` (`to_http_client_config` ~line 225).

**Root cause:** `let backoff_ms = 2u64.pow(attempt - 1) * 100;`. For `max_retries >= 65`, `attempt - 1` reaches 64 and `2u64.pow(64)` overflows — panic in debug, wraparound in release. No config validation clamps `max_retries`. This affects the non-429 retryable errors (500/502/503/504, network, timeout) — those still retry at egress (correct behavior, unchanged).

### Change 2a — Saturating backoff expression

In `with_retry` (client.rs:152), replace:

```rust
let backoff_ms = 2u64.pow(attempt - 1) * 100; // Exponential backoff: 100ms, 200ms, 400ms
```

with:

```rust
// Exponential backoff: 100ms, 200ms, 400ms ... saturating to avoid overflow
// for large max_retries (2u64.pow(64) would panic; checked_shl + saturating_mul
// caps instead). For realistic max_retries (3-10) values are identical to today.
let backoff_ms = (1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX)).saturating_mul(100);
```

`checked_shl(n)` returns `Some(1 << n)` for `n < 64`, `None` for `n >= 64` → `unwrap_or(u64::MAX)` caps the base; `saturating_mul(100)` caps the product. For realistic `max_retries` (3-10) the value is identical to today (`100, 200, 400...`). For pathological `max_retries >= 65` it saturates instead of panicking.

**No loop restructure.** The `with_retry` loop keeps its current shape (sleep at the top, computed from `attempt`). Only the backoff expression changes. The 429 classification (`ProviderError{429}` retryable, `RateLimitExceeded` not retryable) is UNCHANGED — that's the correct behavior (propagate to routing).

### Change 2b — Clamp `max_retries` at config parse (ceiling 10)

**File:** `crates/lunaroute-server/src/config.rs`, `to_http_client_config` (~line 225). Replace:

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

(`warn` is already in scope via `tracing` in config.rs — verify with `rg -n 'use tracing' crates/lunaroute-server/src/config.rs`; add `warn` to the existing `use tracing::...` line if not present.)

Ceiling 10: 10 retries × worst-case backoff (51200 ms at attempt 10) is already beyond sane for a proxy. The clamp makes the overflow unreachable by construction; the saturating expression (2a) is belt-and-suspenders.

### Behavior contract

- `max_retries: 3` (default) → backoff `100, 200, 400` ms — unchanged.
- `max_retries: 10` (ceiling) → backoff up to `51200` ms — no overflow.
- `max_retries: 100` (would panic today) → clamped to 10 with a warn log; no panic.
- 429 handling UNCHANGED: `ProviderError{429}` retried at egress (existing behavior, for any path that still returns it); `RateLimitExceeded{429}` propagates to routing (Task 1 made Anthropic use this path). The routing layer's failover is the 429 defense.

### Tests

- Unit (client.rs): `test_backoff_does_not_overflow_for_large_max_retries` — `with_retry(65, || async { Ok(42) })` succeeds on first attempt, no panic (documents the saturating expression is safe). Add a second test that forces the retry path with a non-429 retryable error (e.g. `ProviderError{500}`) and `max_retries: 65`, using `tokio::time::pause()` to avoid real sleeps, asserting it eventually returns the error without panicking. This proves the backoff expression doesn't overflow even on the retry path.
- Unit (config.rs): `test_to_http_client_config_clamps_max_retries_to_10` (Some(100) → 10) and `test_to_http_client_config_preserves_max_retries_under_10` (Some(5) → 5). Mirror the existing `ProviderSettings` construction in `test_merge_http_client_env_all_variables`.

---

## Validation status

| # | Issue | Validator verdict | Re-evaluation |
|---|---|---|---|
| 8 | Anthropic 429 retried blindly, ignores retry-after | CONFIRMED | Real bug. Fixed by Task 1 (Anthropic → `RateLimitExceeded`, propagates to routing). DONE. |
| 11 | OpenAI 429 (`RateLimitExceeded`) never retried | CONFIRMED | **MISDIAGNOSED.** Egress correctly does NOT retry `RateLimitExceeded` — it propagates to the routing layer's failover. The asymmetry was Anthropic returning `ProviderError` (retried blindly) — fixed by Task 1. No egress change needed. |
| 14 | `2u64.pow(attempt-1)` overflow panic | CONFIRMED | Real bug. Fixed by saturating backoff + config clamp (Fix 2). |

## Rollout (revised)

- **Fix 1 (Task 1): DONE** — commit `d32d3fe`, reviewed and approved. Anthropic → `RateLimitExceeded`.
- **Fix 2: pending** — saturating backoff + config clamp. Small, isolated to `client.rs` (one expression) + `config.rs` (one clamp). No loop restructure, no 429-classification change, no routing-layer change. No risk to the `limits_alternative_strategy` tests (they test 429 failover at routing; Fix 2 doesn't touch the 429 path).

No database migrations. No new config fields (`max_retries` already exists; we only clamp it). No public API changes (`with_retry` signature unchanged; `EgressError::RateLimitExceeded` reused as-is).

## Open questions

None at design time. The architecture correction (routing layer owns 429; egress propagates) was confirmed by reading `provider_router.rs:255` (`Error::RateLimitExceeded { retry_after_secs }` → `record_rate_limit` + switch to alternative) and the `limits_alternative_strategy` tests (which assert `.expect(1)` on the primary — exactly 1 request then failover).
