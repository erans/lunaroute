# Secrets / Logging Hygiene Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three secrets/logging hygiene defects: redact client auth headers in Anthropic passthrough log loops (#5), create session files with mode 0600 (#10), and downgrade `LoggingProvider` content lines from `info!` to `debug!` (#15). Issue #12 was deferred (dead code in production — see spec).

**Architecture:** Fix 1 adds a `redact_header_line` helper in `anthropic.rs` and calls it in the two no-key-passthrough header *log* loops (lines 94, 242 — the ones printing `debug!("│ {}: {}", name, value)`); the *forwarding* loops (121, 134) and the already-filtering configured-key loops (271, 289) are untouched. Fix 2 swaps `File::create` for `OpenOptions::mode(0o600)` in `AtomicWriter::new`. Fix 3 changes three `info!` → `debug!` on content lines in `LoggingProvider`.

**Tech Stack:** Rust 2024, `std::os::unix::fs::{OpenOptionsExt, PermissionsExt}` (unix-only; CI is ubuntu+macos), `tracing` (`info!`/`debug!`), workspace crates `lunaroute-egress`, `lunaroute-storage`, `lunaroute-server`.

## Global Constraints

- Rust edition 2024, MSRV 1.94 (workspace `Cargo.toml`). rustfmt: max width 100, 4-space, Unix.
- No new dependencies. `std::os::unix::fs::OpenOptionsExt`/`PermissionsExt` are std; `tracing`, `tempfile` (dev-dep, already used by `atomic_writer.rs` tests) already available.
- **Fix 1 touches ONLY the two log loops at `anthropic.rs` lines 94 and 242** (the ones with `debug!("│ {}: {}", name, value)` preceded by the masked `debug!("│ x-api-key: <api_key>")`). The *forwarding* loops at lines 121 and 134 (`request_builder.header(...)`) MUST NOT change — only *logging* changes, not *forwarding*. The configured-key-path loops at 271 and 289 already redact (`[FILTERED]`); leave them.
- **Fix 2 is unix-only** (`OpenOptionsExt::mode`). The crate already uses unix APIs unconditionally (`file_lock.rs` uses `std::os::unix::io::AsRawFd` with no `#[cfg(unix)]` guard), so unconditional use matches the codebase convention. CI runs ubuntu + macos (both unix).
- **Fix 3 changes exactly three `info!` → `debug!`** (the content lines: `│ Content: {}`, `│ 📝 {}`, `│ 🔧 Tool call: {}`). Metadata lines (REQUEST/RESPONSE/STREAMING headers, Model, Messages count) stay `info!`. No new config flag.
- No DB migrations. No new config fields. No public API changes (`redact_header_line` is private; `AtomicWriter::new` signature unchanged).
- Each task ends with `cargo test` green for the affected crate(s) and a commit.

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `crates/lunaroute-egress/src/anthropic.rs` | Modify | Add `redact_header_line` helper; use it in the two log loops (lines 94, 242). Add a unit test for the helper. |
| `crates/lunaroute-storage/src/atomic_writer.rs` | Modify | `OpenOptions::mode(0o600)` in `AtomicWriter::new`. Add a mode-0600 assertion test. |
| `crates/lunaroute-server/src/main.rs` | Modify | Three `info!` → `debug!` on `LoggingProvider` content lines. |

---

### Task 1: Redact auth headers in Anthropic passthrough log loops (Fix 1, #5)

**Files:**
- Modify: `crates/lunaroute-egress/src/anthropic.rs` (add `redact_header_line` helper near the top of the file; replace the two log loops at lines 94 and 242; add a unit test in `mod tests`)

**Interfaces:**
- Consumes: `debug!` (in scope), `axum::http::HeaderName`/`HeaderValue` (in scope via existing imports).
- Produces: `fn redact_header_line(name: &str, value: &str) -> String` (private, module-scope). Used by both log loops + the test.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-egress/src/anthropic.rs`, inside `mod tests` (starts at line 1047; `use super::*;` at line 1050 brings `redact_header_line` into scope once added). Add:

```rust
    #[test]
    fn test_redact_header_line_redacts_auth() {
        // Auth headers are redacted (value replaced with <redacted>).
        let auth = redact_header_line("authorization", "Bearer sk-secret-123");
        assert!(auth.contains("<redacted>"), "authorization must be redacted: {auth}");
        assert!(!auth.contains("sk-secret-123"), "authorization value must NOT appear: {auth}");

        let api_key = redact_header_line("x-api-key", "sk-ant-api03-xyz");
        assert!(api_key.contains("<redacted>"));
        assert!(!api_key.contains("sk-ant-api03-xyz"));

        // Case-insensitive: "Authorization" and "X-API-Key" also redacted.
        assert!(redact_header_line("Authorization", "Bearer x").contains("<redacted>"));
        assert!(redact_header_line("X-API-Key", "y").contains("<redacted>"));

        // Non-auth headers are logged verbatim.
        let ct = redact_header_line("content-type", "application/json");
        assert_eq!(ct, "│ content-type: application/json");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-egress --lib anthropic::tests::test_redact_header_line_redacts_auth`
Expected: FAIL — compile error `cannot find function 'redact_header_line'` (not yet defined).

- [ ] **Step 3: Add the `redact_header_line` helper**

In `crates/lunaroute-egress/src/anthropic.rs`, near the top of the file (after the `use` statements and before the first item — or just above the first `impl`/`struct`), add:

```rust
/// Format a header for debug logging, redacting auth values.
///
/// `authorization` and `x-api-key` are replaced with `<redacted>` so client
/// credentials don't leak into logs in the no-key passthrough mode. Other
/// headers are logged verbatim.
fn redact_header_line(name: &str, value: &str) -> String {
    let name_lower = name.to_lowercase();
    if name_lower == "authorization" || name_lower == "x-api-key" {
        format!("│ {}: <redacted>", name)
    } else {
        format!("│ {}: {}", name, value)
    }
}
```

- [ ] **Step 4: Use the helper in the two log loops (lines 94 and 242)**

In `crates/lunaroute-egress/src/anthropic.rs`, replace the two log loops. Find them by the unique pattern: a `for (name, value) in &headers {` loop whose body is `debug!("│ {}: {}", name, value);` and which is preceded by `debug!("│ x-api-key: <api_key>");` (lines 94 and 242). Replace EACH:

```rust
// BEFORE (both sites, lines ~94 and ~242):
        for (name, value) in &headers {
            debug!("│ {}: {}", name, value);
        }

// AFTER (both sites):
        for (name, value) in &headers {
            debug!(
                "{}",
                redact_header_line(name.as_str(), value.to_str().unwrap_or("<non-utf8>"))
            );
        }
```

**CRITICAL — do NOT touch the forwarding loops at lines 121 and 134** (their bodies are `request_builder = request_builder.header(name, value);` — they forward headers to the upstream, not log them). Only the two *log* loops (bodies `debug!("│ {}: {}", name, value);`, preceded by the masked `x-api-key: <api_key>` line) change. Verify after editing: `grep -n 'request_builder.header(name, value)' crates/lunaroute-egress/src/anthropic.rs` should still show lines 121/134 (forwarding intact); `grep -n 'redact_header_line' crates/lunaroute-egress/src/anthropic.rs` should show the helper + the two call sites.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p lunaroute-egress --lib anthropic`
Expected: PASS — the new `test_redact_header_line_redacts_auth` passes; existing `test_*` tests unaffected (the forwarding loops are unchanged, so passthrough behavior is identical).

Run: `cargo test -p lunaroute-integration-tests --test passthrough_streaming_recording`
Expected: PASS — the passthrough integration tests (which exercise the forwarding path) still pass (forwarding unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-egress/src/anthropic.rs
git commit -m "fix(egress): redact client auth headers in Anthropic passthrough log loops

Two debug! log loops in send_passthrough (non-streaming ~line 94 and streaming
~line 242) printed ALL client headers verbatim, including authorization and
x-api-key in the no-key passthrough mode. Extract redact_header_line helper
(redacts authorization/x-api-key to <redacted>, case-insensitive) and use it
in both log loops. Only logging changes; header forwarding is untouched."
```

---

### Task 2: Session files created with mode 0600 (Fix 2, #10)

**Files:**
- Modify: `crates/lunaroute-storage/src/atomic_writer.rs` (`AtomicWriter::new` ~line 29; `mod tests` ~line 91)

**Interfaces:**
- Consumes: `std::fs::OpenOptions`, `std::os::unix::fs::OpenOptionsExt` (unix-only).
- Produces: `AtomicWriter::new` creates temp files with mode 0600; `fs::rename` preserves the mode to the final path. Signature unchanged.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-storage/src/atomic_writer.rs`, inside `mod tests` (starts at line 91; `use super::*;` and `use std::fs;` + `use tempfile::TempDir;` at lines 92-94). Add:

```rust
    #[cfg(unix)]
    #[test]
    fn test_atomic_write_creates_file_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session_request.bin");

        let mut writer = AtomicWriter::new(&path).unwrap();
        writer.write(b"secret prompt content").unwrap();
        writer.commit().unwrap(); // rename to final path

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "session file must be 0600 (owner rw only), got {:o}",
            mode & 0o777
        );
    }
```

(`writer.commit()` is the method that calls `fs::rename` — `pub fn commit(mut self)` at line 45, consumes the writer. The existing `test_atomic_write_success` uses the same `new().write().commit()` pattern.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-storage --lib atomic_writer::tests::test_atomic_write_creates_file_with_mode_0600`
Expected: FAIL — today `File::create` produces 0644 (umask 022), so `mode & 0o777 == 0o644 != 0o600`.

- [ ] **Step 3: Replace `File::create` with `OpenOptions::mode(0o600)`**

In `crates/lunaroute-storage/src/atomic_writer.rs`, update the imports at the top of the file (line 4 is `use std::fs::{self, File};`):

```rust
use std::fs::{self, OpenOptions};
```

(Keep `File` if it's used elsewhere in the file — check with `grep -n 'File::' crates/lunaroute-storage/src/atomic_writer.rs`. If `File` is still used, use `use std::fs::{self, File, OpenOptions};`. The `use std::io::Write;` at line 5 stays.)

Add the unix extension import (near the top, after the other `use` statements):

```rust
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
```

In `AtomicWriter::new` (~line 29), replace:

```rust
        // Create the temporary file
        let file = File::create(&temp_path)?;
```

with:

```rust
        // Create the temporary file with restrictive permissions (0600) so
        // other local users can't read recorded session content (prompts,
        // tool I/O). fs::rename preserves the mode to the final path.
        #[cfg(unix)]
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp_path)?;
        #[cfg(not(unix))]
        let file = File::create(&temp_path)?;
```

(The `#[cfg(unix)]`/`#[cfg(not(unix))]` pair is belt-and-suspenders — matches the codebase's unix usage while keeping non-unix builds working. If the existing `file_lock.rs` convention is *unconditional* unix (no `#[cfg]`), drop the `#[cfg]` guards and the `#[cfg(not(unix))]` fallback to match — but the guarded version is safer and compiles on both. Use whichever compiles cleanest; the test is `#[cfg(unix)]` either way.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-storage --lib atomic_writer`
Expected: PASS — the new `test_atomic_write_creates_file_with_mode_0600` passes (mode is now 0600); existing `test_atomic_write_*` tests still pass (the `OpenOptions` flags match `File::create`'s behavior: write+create+truncate).

- [ ] **Step 5: Run the storage suite + check other writers aren't broken**

Run: `cargo test -p lunaroute-storage`
Expected: PASS (the `rolling_writer` and `file_lock` tests use their own `OpenOptions` calls, unaffected; `session::tests` use `AtomicWriter` and now get 0600 files, which doesn't break their content assertions).

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-storage/src/atomic_writer.rs
git commit -m "fix(storage): create session files with mode 0600

AtomicWriter::new used File::create, producing files with default perms
(umask 022 -> 0644). FileSessionStore writes raw request/response bodies
(prompts, tool I/O) via AtomicWriter, so on shared/multi-user hosts other
local users could read recorded session content. Use OpenOptions::mode(0o600)
for the temp file; fs::rename preserves the mode to the final path."
```

---

### Task 3: Downgrade LoggingProvider content lines to `debug!` (Fix 3, #15)

**Files:**
- Modify: `crates/lunaroute-server/src/main.rs` (`LoggingProvider` impl, ~lines 213-300)

**Interfaces:**
- Consumes: `tracing::{info, debug}` (in scope).
- Produces: three `info!` → `debug!` edits. No signature/config changes.

- [ ] **Step 1: Confirm the exact three content lines**

In `crates/lunaroute-server/src/main.rs`, locate the three content lines in `LoggingProvider`:

```bash
grep -n 'info!("│ Content:' crates/lunaroute-server/src/main.rs
grep -n 'info!("│ 📝' crates/lunaroute-server/src/main.rs
grep -n 'info!("│ 🔧 Tool call:' crates/lunaroute-server/src/main.rs
```

(Expect one match each: `│ Content: {}` ~line 236, `│ 📝 {}` ~line 290, `│ 🔧 Tool call: {}` ~line 296.) Verify the surrounding lines are `info!` (metadata) that must NOT change (REQUEST/RESPONSE/STREAMING/Model/Messages).

- [ ] **Step 2: Downgrade the three content lines**

In `crates/lunaroute-server/src/main.rs`, change exactly these three `info!` → `debug!`:

```rust
// ~line 236 (non-streaming response content):
// BEFORE:
                info!("│ Content: {}", text);
// AFTER:
                debug!("│ Content: {}", text);

// ~line 290 (streaming chunk content):
// BEFORE:
                            info!("│ 📝 {}", content);
// AFTER:
                            debug!("│ 📝 {}", content);

// ~line 296 (tool call — reveals usage; args would be content):
// BEFORE:
                                info!("│ 🔧 Tool call: {}", name);
// AFTER:
                                debug!("│ 🔧 Tool call: {}", name);
```

**Do NOT change** the metadata `info!` lines: `│ REQUEST to {}`, `│ RESPONSE from {}`, `│ STREAMING from {}`, `│ Model: {}`, `│ Messages: {} messages`, the `┌─`/`├─`/`└─` box-drawing lines. Those stay `info!`.

- [ ] **Step 3: Verify the changes**

Run: `grep -n 'info!("│ Content:\|info!("│ 📝\|info!("│ 🔧' crates/lunaroute-server/src/main.rs`
Expected: NO matches (all three are now `debug!`).

Run: `grep -n 'debug!("│ Content:\|debug!("│ 📝\|debug!("│ 🔧' crates/lunaroute-server/src/main.rs`
Expected: three matches (the downgraded lines).

Run: `cargo build -p lunaroute-server`
Expected: PASS (compiles; `debug!` is already in scope via `tracing`).

- [ ] **Step 4: Run the server suite (no behavior change, just compiles + existing tests green)**

Run: `cargo test -p lunaroute-server`
Expected: PASS (existing tests don't assert on `info!` output; the downgraded lines are observability-only).

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-server/src/main.rs
git commit -m "fix(server): downgrade LoggingProvider content lines to debug!

LoggingProvider (gated behind config.logging.log_requests) logged full
prompt/response content at info! (│ Content, │ 📝 streaming chunks, │ 🔧 Tool
call). An operator enabling log_requests for debugging got raw content in
stdout -> log aggregator -> PII leak. Downgrade the three content lines to
debug!; metadata lines (REQUEST/RESPONSE/Model/Messages count) stay info!.
With log_requests=true + RUST_LOG=info (default) only metadata appears;
content requires RUST_LOG=debug. No new config flag."
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
Expected: all clean. (Fix 2's `#[cfg(unix)]` test runs on ubuntu+macos CI; the `#[cfg(not(unix))]` fallback in `AtomicWriter::new` keeps non-unix clippy/check happy if it ever runs.)

## Self-Review

**1. Spec coverage:**
- Fix 1 (Anthropic header redaction, #5) → Task 1. ✓
- Fix 2 (session file perms 0600, #10) → Task 2. ✓
- Fix 3 (content log downgrade, #15) → Task 3. ✓
- #12 deferred → no code (documented in spec). ✓
- Tests per fix → Task 1 (helper test), Task 2 (mode-0600 test), Task 3 (no test — mechanical level change, documented). ✓

**2. Placeholder scan:** No "TBD"/"TODO". The two hedged instructions (Task 2 Step 3 "if `File` is still used, keep it in the import"; "use whichever compiles cleanest" re: `#[cfg(unix)]` guards) give concrete fallbacks based on grep checks. ✓

**3. Type consistency:**
- `redact_header_line(name: &str, value: &str) -> String` — consistent across Task 1 helper, both call sites (`name.as_str()`, `value.to_str().unwrap_or(...)`), and the test. ✓
- `OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)` — matches `File::create`'s semantics (write+create+truncate); `PermissionsExt::mode()` in the test. ✓
- Three `info!` → `debug!` on the exact content lines (236, 290, 296); metadata stays `info!`. ✓

**4. Regression risk:**
- Task 1: ONLY the two log loops (94, 242) change. Forwarding loops (121, 134) untouched — verified by grep after edit. The configured-key-path loops (271, 289) already redact (`[FILTERED]`) and are untouched. Passthrough behavior unchanged (Task 1 Step 5 verifies via `passthrough_streaming_recording`). ✓
- Task 2: `OpenOptions` flags match `File::create`; only the mode changes. Other writers (`rolling_writer`, `file_lock`) use their own `OpenOptions` and are unaffected. ✓
- Task 3: three `info!` → `debug!`; no behavior change, observability-only. ✓

No issues found. Plan is complete.
