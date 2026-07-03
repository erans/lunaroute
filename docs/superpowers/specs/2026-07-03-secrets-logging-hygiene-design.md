# Secrets / Logging Hygiene — Design

**Date:** 2026-07-03
**Scope:** Batch D of the adversarial code-review findings — secrets-in-logs and secrets-at-rest (issues #5, #10, #15). Issue #12 (spoofable X-Forwarded-For) was **deferred** — its trust logic lives in `request_context_middleware`, which is never applied in production (the OpenAI/Anthropic routers use `Router::new().route(...).with_state(...)` with no `from_fn` layer), and production recording handlers hardcode `client_ip: None` (openai.rs:1024/1691/2463, anthropic.rs:1377). The spoofing risk doesn't manifest; "fixing" dead code is busywork. Building a real audit trail is a feature, not a bugfix, and belongs in its own brainstorm. Noted here for a future batch.
**Status:** Findings independently validated by a Codex GPT-5.5 (xhigh) pass that read the actual source (#5 CONFIRMED, #10 CONFIRMED, #15 CONFIRMED — all config-gated hence LOW).

## Context

Three real production issues where secrets or PII can leak:

- **#5 — Anthropic passthrough logs client auth headers in cleartext.** Two `for (name, value) in &headers { debug!("│ {}: {}", name, value); }` log loops in `crates/lunaroute-egress/src/anthropic.rs` (the non-streaming `send_passthrough` ~line 94 and the streaming passthrough ~line 242) print ALL client-supplied headers verbatim at `debug!`. In the documented "no API key — will use client auth" passthrough mode, the client's `authorization`/`x-api-key` is forwarded to the egress and thus written to logs in cleartext whenever debug logging is enabled.
- **#10 — Session files are world-readable (0644).** `AtomicWriter::new` (`crates/lunaroute-storage/src/atomic_writer.rs:29`) creates the temp file with `File::create` (default umask 022 → 0644); `fs::rename` (line 59) preserves those perms to the final `request.bin`/`response.bin`/`metadata.json`. `FileSessionStore` uses `AtomicWriter` to write raw request/response bodies (user prompts, possibly tool I/O containing secrets). On shared/multi-user hosts, other local users can read recorded session content.
- **#15 — `LoggingProvider` logs full prompt/response content at `info!`.** Gated behind `config.logging.log_requests` (main.rs:790,841 — operator opt-in, hence LOW), but when enabled it logs `info!("│ Content: {}", text)` (main.rs:236), `info!("│ 📝 {}", content)` (streaming, :290), and `info!("│ 🔧 Tool call: {}", name)` (:296). An operator who enables `log_requests` for debugging gets raw content in stdout → log aggregator → PII leak.

## Goals

1. **#5:** Redact `authorization`/`x-api-key` in the Anthropic passthrough header-log loops (both sites). Client auth values no longer appear in logs; the masked `<redacted>` placeholder preserves observability that the header was present.
2. **#10:** Create session files with mode 0600 (owner read/write only) via `OpenOptions::mode`. Other local users can no longer read recorded prompts.
3. **#15:** Downgrade the *content* log lines (prompt/response text, streaming chunks, tool calls) from `info!` to `debug!`; keep the *metadata* lines (model, message counts, provider, status) at `info!`. An operator with `log_requests=true` + default `RUST_LOG=info` sees only metadata; content requires `RUST_LOG=debug`. No new config flag.

## Non-goals

- **#12 (deferred):** the X-Forwarded-For trust logic is dead code in production. Not fixed in this batch. Building a real `client_ip` audit trail (trusted-proxy model + `ConnectInfo` + wiring into production routers + recording handlers) is a feature, not a bugfix, and gets its own brainstorm. Noted for future.
- Changes to which headers are *forwarded* to the upstream (#5 only changes *logging*, not forwarding).
- Changes to the parent directory perms of session files (created via `create_dir_all`, default 0755 — readable but not writable by others; the *files* hold the secrets, and they get 0600).
- The other review batch (async hygiene, #9). Its own spec.

---

## Fix 1 (MEDIUM, #5): Redact auth headers in Anthropic passthrough log loops

**Location:** `crates/lunaroute-egress/src/anthropic.rs`, two log loops (~line 94 in `send_passthrough`, ~line 242 in the streaming passthrough).

### Change — extract a redact helper + redact in both loops

Add a small private helper near the top of the file (single source of truth, testable):

```rust
/// Format a header for debug logging, redacting auth values.
fn redact_header_line(name: &str, value: &str) -> String {
    let name_lower = name.to_lowercase();
    if name_lower == "authorization" || name_lower == "x-api-key" {
        format!("│ {}: <redacted>", name)
    } else {
        format!("│ {}: {}", name, value)
    }
}
```

Replace both log loops:

```rust
// BEFORE (both sites):
for (name, value) in &headers {
    debug!("│ {}: {}", name, value);
}

// AFTER (both sites):
for (name, value) in &headers {
    debug!("{}", redact_header_line(name.as_str(), value.to_str().unwrap_or("<non-utf8>")));
}
```

(`value.to_str()` can fail for non-UTF8 header values; `unwrap_or("<non-utf8>")` avoids a panic and matches the codebase's existing `if let Ok(value_str) = value.to_str()` defensive pattern in `bypass.rs`.) The masked `debug!("│ x-api-key: <api_key>")` line above each loop (for the configured key) stays — it's already masked; only the client-headers loop changes.

### Behavior contract

- Client `authorization`/`x-api-key` values no longer appear in debug logs; replaced with `<redacted>`.
- Other headers logged unchanged.
- No change to which headers are *forwarded* to the upstream (the forwarding logic is separate from these log loops and is unchanged).
- Both passthrough sites (non-streaming + streaming) behave identically.

### Tests

- Unit (`anthropic.rs` `mod tests`): `test_redact_header_line_redacts_auth` — assert `redact_header_line("authorization", "Bearer sk-...")` contains `<redacted>` and NOT `sk-...`; same for `x-api-key`; assert a non-auth header `redact_header_line("content-type", "application/json")` returns the value unchanged. This tests the helper directly (single source of truth used by both sites).

---

## Fix 2 (MEDIUM, #10): Session files created with mode 0600

**Location:** `crates/lunaroute-storage/src/atomic_writer.rs`, `AtomicWriter::new` (~line 29).

### Change — `OpenOptions::mode(0o600)`

Replace the `File::create` call:

```rust
// BEFORE:
let file = File::create(&temp_path)?;

// AFTER:
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;  // unix-only; CI runs ubuntu+macos (both unix)

let file = OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .mode(0o600)
    .open(&temp_path)?;
```

`OpenOptionsExt::mode` is unix-only. The crate already uses unix-specific APIs unconditionally (`file_lock.rs` uses `std::os::unix::io::AsRawFd` with no `#[cfg(unix)]` guard), so unconditional use matches the codebase convention. CI runs ubuntu + macos (both unix), so this compiles on all CI targets. (If a windows clippy/check surfaces, add `#[cfg(unix)]` — but the existing convention is unconditional, so match it.)

`fs::rename` (line 59) preserves the temp file's mode to the final path, so `request.bin`/`response.bin`/`metadata.json` end up 0600.

### Behavior contract

- Session files created 0600 (owner read/write only). Other local users lose read access to recorded prompts.
- No change for the owner (full read/write).
- Parent directory perms unchanged (created via `create_dir_all`, default 0755).
- No change to file content, naming, or the atomic-write semantics.

### Tests

- Unit (`atomic_writer.rs` `mod tests`): `test_atomic_write_creates_file_with_mode_0600` — write a file via `AtomicWriter`, read back the mode with `std::os::unix::fs::PermissionsExt::mode()` (or `fs::metadata(path).permissions().mode()`), assert `mode & 0o777 == 0o600`. Unix-only — matches the existing `test_atomic_write_*` tests which run on the same CI matrix. (If the existing test module doesn't import `PermissionsExt`, add `use std::os::unix::fs::PermissionsExt;` inside the test or the module.)

---

## Fix 3 (LOW, #15): Downgrade content logging to `debug!`

**Location:** `crates/lunaroute-server/src/main.rs`, `LoggingProvider` impl (~lines 213-300).

### Change — content lines to `debug!`, metadata stays `info!`

| Line (~) | Today | After | Category |
|---|---|---|---|
| 216 `│ REQUEST to {} (non-streaming)` | `info!` | `info!` | metadata |
| 218 `│ Model: {}` | `info!` | `info!` | metadata |
| 219 `│ Messages: {} messages` | `info!` | `info!` | metadata |
| 230 `│ RESPONSE from {}` | `info!` | `info!` | metadata |
| 236 `│ Content: {}` | `info!` | **`debug!`** | content (PII) |
| 263 `│ REQUEST to {} (streaming)` | `info!` | `info!` | metadata |
| 265 `│ Model: {}` | `info!` | `info!` | metadata |
| 266 `│ Messages: {} messages` | `info!` | `info!` | metadata |
| 278 `│ STREAMING from {}` | `info!` | `info!` | metadata |
| 290 `│ 📝 {}` (streaming chunk) | `info!` | **`debug!`** | content (PII) |
| 296 `│ 🔧 Tool call: {}` | `info!` | **`debug!`** | content (reveals usage; args would be content) |

Mechanical: change `info!` → `debug!` on the three content lines only. The `log_requests` flag still gates the whole `LoggingProvider` (unchanged); the *level* now distinguishes metadata from content.

### Behavior contract

- `log_requests=false` (default): no `LoggingProvider` (unchanged).
- `log_requests=true` + `RUST_LOG=info` (default): metadata only (model, counts, provider, status). No prompt/response content.
- `log_requests=true` + `RUST_LOG=debug`: metadata + content (full text, chunks, tool calls).

### Tests

The existing logging behavior is observability-only (no tests assert on `info!` output). Adding a deterministic log-level test requires a log-capture harness (e.g. `tracing-subscriber`'s test layer), which the crate doesn't currently use. **No new test** for this fix — the change is mechanical (three `info!` → `debug!` edits) and verified by reading the level. Noted in the spec; if a harness exists later, a test asserting "content lines appear at debug, metadata at info" would be added then.

---

## Validation status

| # | Issue | Validator verdict | Evidence (file:line read by validator) |
|---|---|---|---|
| 5 | Anthropic passthrough logs client auth headers | CONFIRMED | anthropic.rs:94-95 (logs all headers before the configured-key filter at :119); also the streaming site at :242-243 (found by validator) |
| 10 | Session files use default file permissions | CONFIRMED | atomic_writer.rs:29 (`File::create`); session.rs:123-126 (writes session data through AtomicWriter) |
| 15 | `LoggingProvider` logs content when request logging enabled | CONFIRMED | main.rs:236 (`info!` content), :220/:244 (debug full JSON), gated at :788/:837 |
| 12 | Client IP trusts spoofable forwarding headers | CONFIRMED by validator | **Re-evaluated: dead code in production.** middleware.rs:30 trust logic is in `request_context_middleware`, never applied in production routers; production handlers hardcode `client_ip: None`. Deferred. |

## Rollout

The three fixes are independent and localized. Recommended order (dependency, soft):

1. **Fix 1** (Anthropic header redaction) — smallest, one helper + two loop edits + a unit test.
2. **Fix 2** (session file perms) — one `OpenOptions::mode` change + a mode-assertion test.
3. **Fix 3** (content log downgrade) — three `info!` → `debug!` edits, no test.

No database migrations. No new config fields (the `log_requests` flag already exists; Fix 3 reuses it). No public API changes (the `redact_header_line` helper is private; `AtomicWriter::new` signature unchanged). No new dependencies (`std::os::unix::fs::OpenOptionsExt` is std).

## Open questions

None at design time. The three behavioral decisions (defer #12; downgrade-to-debug for #15; 0600 for #10) were made during brainstorming. #12's deferral is documented for a future audit-trail brainstorm.
