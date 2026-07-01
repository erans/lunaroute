# Session Recording Correctness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix four session-recording correctness defects (CRITICAL Postgres recording drop, Anthropic `End` event loss, `Completed`-on-disconnect loss, OpenAI tool-arg fragment loss) so sessions are fully and accurately persisted.

**Architecture:** Fix 1 adds a `TenantScopedStore` decorator that resolves an implicit default tenant at the single startup wiring seam (zero call-site edits). Fix 2 converts the `scan` (one-event-per-item) combinator to emit a `Vec` per item then `flat_map`s it, with the `MessageDelta` emission extracted as a pure, unit-tested helper. Fix 3 adds a 4-line `impl Drop` on the recording stream that delegates to the existing `complete()`. Fix 4 hoists OpenAI tool-argument accumulation out of the `function.name` guard. All fixes are TDD with regression tests that fail before and pass after.

**Tech Stack:** Rust 2024, tokio, axum, futures (`StreamExt`, `scan`, `flat_map`), `async_trait`, `serde_json`, workspace crates `lunaroute-core` (`SessionStore` trait, `TenantId`), `lunaroute-session`, `lunaroute-egress`, `lunaroute-ingress`.

## Global Constraints

- Rust edition 2024, MSRV 1.94 (from `Cargo.toml` workspace).
- Max width 100, 4-space indent, Unix newlines (from `rustfmt.toml`).
- No new dependencies. Reuse existing `futures`, `async_trait`, `tokio`, `serde_json`, `lunaroute-core`.
- `SessionStore::write_event(tenant_id: Option<TenantId>, event: SessionEvent)` signature is **unchanged** (crate-internal `lunaroute-core` API stays public API).
- No DB migrations, no config changes, no public API additions (the new `TenantScopedStore` is internal to `lunaroute-session`).
- Tests use `#[tokio::test]` and the existing `InMemorySessionStore` mock pattern (see `crates/lunaroute-ingress/tests/routed_recording.rs:20-80`).
- Each task ends with `cargo test` green for the affected crate and a commit.

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `crates/lunaroute-session/src/tenant_scoped_store.rs` | **Create** | `TenantScopedStore` decorator: `SessionStore` impl that resolves `None` → default tenant. |
| `crates/lunaroute-session/src/lib.rs` | Modify | Add `pub mod tenant_scoped_store;` and `pub use tenant_scoped_store::TenantScopedStore;`. |
| `crates/lunaroute-server/src/main.rs` | Modify (1 line) | Wrap `session_store_for_passthrough` in `TenantScopedStore`. |
| `crates/lunaroute-egress/src/anthropic.rs` | Modify | Extract `emit_message_delta_events`; convert `scan` to `Vec` + `flat_map`. |
| `crates/lunaroute-session/src/session_store_recording_provider.rs` | Modify | Add `impl Drop` delegating to `complete()`; add `#[cfg(test)]` test constructor. |
| `crates/lunaroute-ingress/src/async_stream_parser.rs` | Modify | Hoist OpenAI tool-argument accumulation out of the `name` guard. |

---

### Task 1: `TenantScopedStore` decorator (Fix 1 — CRITICAL)

**Files:**
- Create: `crates/lunaroute-session/src/tenant_scoped_store.rs`
- Modify: `crates/lunaroute-session/src/lib.rs` (add module + re-export)
- Modify: `crates/lunaroute-server/src/main.rs` (one-line wiring, ~line 633)

**Interfaces:**
- Consumes: `lunaroute_core::session_store::SessionStore` (trait, 8 methods, all take `Option<TenantId>` first), `lunaroute_core::tenant::TenantId`, `std::sync::Arc`.
- Produces: `pub struct TenantScopedStore` with `pub fn new(inner: Arc<dyn SessionStore>, default_tenant: Option<TenantId>) -> Self`, implementing `SessionStore`. Consumed by `main.rs` as `Arc::new(TenantScopedStore::new(s, tenant_id)) as Arc<dyn SessionStore>`.

- [ ] **Step 1: Write the failing test**

Create `crates/lunaroute-session/src/tenant_scoped_store.rs` with the test module first (the impl will be added in Step 3 to make it compile). Begin the file with:

```rust
//! Tenant-scoped session store decorator.
//!
//! Wraps a `SessionStore` and resolves an implicit process-wide default tenant
//! for calls that pass `None`. Used for recording, where the caller is the proxy
//! itself (no per-request tenant resolution yet).
//!
//! Resolution rule: `resolved = tenant_id.or(self.default_tenant)`.
//!
//! - When `default_tenant == Some(t)` (Postgres mode): `None` → `Some(t)`,
//!   and an explicit `Some(u)` overrides the default.
//! - When `default_tenant == None` (SQLite/file mode): `None` → `None`
//!   (unchanged behavior — SQLite requires `None`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lunaroute_core::{
    Result,
    session_store::{
        AggregateStats, CleanupStats, RetentionPolicy, SearchQuery, SearchResults, Session,
        SessionEvent, SessionStore, TimeRange,
    },
    tenant::TenantId,
};

/// Wraps a `SessionStore` and resolves an implicit default tenant.
///
/// See module docs for the resolution rule and the "bridge" rationale.
pub struct TenantScopedStore {
    inner: Arc<dyn SessionStore>,
    default_tenant: Option<TenantId>,
}

impl TenantScopedStore {
    pub fn new(inner: Arc<dyn SessionStore>, default_tenant: Option<TenantId>) -> Self {
        Self { inner, default_tenant }
    }
}

#[async_trait]
impl SessionStore for TenantScopedStore {
    async fn write_event(
        &self,
        tenant_id: Option<TenantId>,
        event: SessionEvent,
    ) -> Result<()> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.write_event(tid, event).await
    }

    async fn search(
        &self,
        tenant_id: Option<TenantId>,
        query: SearchQuery,
    ) -> Result<SearchResults> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.search(tid, query).await
    }

    async fn get_session(
        &self,
        tenant_id: Option<TenantId>,
        session_id: &str,
    ) -> Result<Session> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.get_session(tid, session_id).await
    }

    async fn cleanup(
        &self,
        tenant_id: Option<TenantId>,
        retention: RetentionPolicy,
    ) -> Result<CleanupStats> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.cleanup(tid, retention).await
    }

    async fn get_stats(
        &self,
        tenant_id: Option<TenantId>,
        time_range: TimeRange,
    ) -> Result<AggregateStats> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.get_stats(tid, time_range).await
    }

    async fn list_sessions(
        &self,
        tenant_id: Option<TenantId>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Session>> {
        let tid = tenant_id.or(self.default_tenant);
        self.inner.list_sessions(tid, limit, offset).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunaroute_core::tenant::TenantId;
    use uuid::Uuid;

    /// Mock store that records every `tenant_id` it received for `write_event`.
    struct CapturingStore {
        seen: Mutex<Vec<Option<TenantId>>>,
    }

    impl CapturingStore {
        fn new() -> Self {
            Self { seen: Mutex::new(Vec::new()) }
        }
        fn seen(&self) -> Vec<Option<TenantId>> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SessionStore for CapturingStore {
        async fn write_event(
            &self,
            tenant_id: Option<TenantId>,
            _event: SessionEvent,
        ) -> Result<()> {
            self.seen.lock().unwrap().push(tenant_id);
            Ok(())
        }
        async fn search(
            &self,
            _t: Option<TenantId>,
            _q: SearchQuery,
        ) -> Result<SearchResults> {
            Ok(serde_json::json!({"sessions": []}))
        }
        async fn get_session(
            &self,
            _t: Option<TenantId>,
            _id: &str,
        ) -> Result<Session> {
            Ok(serde_json::json!(null))
        }
        async fn cleanup(
            &self,
            _t: Option<TenantId>,
            _r: RetentionPolicy,
        ) -> Result<CleanupStats> {
            Ok(serde_json::json!({"deleted": 0}))
        }
        async fn get_stats(
            &self,
            _t: Option<TenantId>,
            _tr: TimeRange,
        ) -> Result<AggregateStats> {
            Ok(serde_json::json!({}))
        }
        async fn list_sessions(
            &self,
            _t: Option<TenantId>,
            _l: usize,
            _o: usize,
        ) -> Result<Vec<Session>> {
            Ok(Vec::new())
        }
    }

    fn tid() -> TenantId {
        TenantId::from_uuid(Uuid::new_v4())
    }

    #[tokio::test]
    async fn none_resolves_to_default_when_default_set() {
        let inner = Arc::new(CapturingStore::new());
        let default = tid();
        let scoped = TenantScopedStore::new(inner.clone(), Some(default));
        scoped
            .write_event(None, serde_json::json!({"type": "Started"}))
            .await
            .unwrap();
        assert_eq!(inner.seen(), vec![Some(default)]);
    }

    #[tokio::test]
    async fn explicit_tenant_overrides_default() {
        let inner = Arc::new(CapturingStore::new());
        let default = tid();
        let request_tenant = tid();
        let scoped = TenantScopedStore::new(inner.clone(), Some(default));
        scoped
            .write_event(Some(request_tenant), serde_json::json!({"type": "Started"}))
            .await
            .unwrap();
        assert_eq!(inner.seen(), vec![Some(request_tenant)]);
    }

    #[tokio::test]
    async fn none_passes_through_when_no_default() {
        let inner = Arc::new(CapturingStore::new());
        let scoped = TenantScopedStore::new(inner.clone(), None);
        scoped
            .write_event(None, serde_json::json!({"type": "Started"}))
            .await
            .unwrap();
        assert_eq!(inner.seen(), vec![None]);
    }
}
```

- [ ] **Step 2: Run test to verify it compiles and passes**

Run: `cargo test -p lunaroute-session --lib tenant_scoped_store`
Expected: PASS (3 tests). The impl is already in the same file, so this should compile and pass on the first run — confirming the decorator works in isolation before wiring.

- [ ] **Step 3: Wire `TenantScopedStore` into the startup path**

In `crates/lunaroute-session/src/lib.rs`, after the line `pub mod session_store_recording_provider;` (around line 8), add:

```rust
pub mod tenant_scoped_store;
```

And in the re-export block (after `pub use session_store_recording_provider::SessionStoreRecordingProvider;` around line 30), add:

```rust
pub use tenant_scoped_store::TenantScopedStore;
```

In `crates/lunaroute-server/src/main.rs`, replace the line (around line 633):

```rust
    let session_store_for_passthrough = session_store.clone();
```

with:

```rust
    let session_store_for_passthrough = session_store
        .clone()
        .map(|s| Arc::new(lunaroute_session::TenantScopedStore::new(s, tenant_id)) as Arc<dyn SessionStore>);
```

(`tenant_id` is the `Option<lunaroute_core::tenant::TenantId>` bound at main.rs:504, already in scope. `Arc` is already imported in main.rs.)

- [ ] **Step 4: Run the workspace test suite to verify no regression**

Run: `cargo test -p lunaroute-server -p lunaroute-session`
Expected: PASS (all existing tests green; `test_app_state_tenant_id` and the recording tests unaffected — they don't go through `session_store_for_passthrough`).

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-session/src/tenant_scoped_store.rs crates/lunaroute-session/src/lib.rs crates/lunaroute-server/src/main.rs
git commit -m "fix(session): record sessions under resolved tenant via TenantScopedStore

In Postgres mode, recording call sites passed None for tenant_id to a
PostgresSessionStore that requires Some, silently dropping 100% of
recording events. Wrap the passthrough store in a TenantScopedStore
decorator that resolves None -> startup default tenant at the single
wiring seam. Zero edits to the ~30 recording call sites."
```

---

### Task 2: Emit Anthropic `End` event on combined `message_delta` (Fix 2 — HIGH)

**Files:**
- Modify: `crates/lunaroute-egress/src/anthropic.rs` (extract `emit_message_delta_events`; convert `scan` to `Vec` + `flat_map` at ~line 823-966; update the test-local `parse_sse_events` helper at ~line 1723 to call the new pure fn).

**Interfaces:**
- Consumes: `AnthropicStreamMessageDelta`, `AnthropicStreamUsage` (existing types in this file), `NormalizedStreamEvent`, `Usage`, `FinishReason` (from `lunaroute_core::normalized`).
- Produces: `fn emit_message_delta_events(usage: &AnthropicStreamUsage, delta: &AnthropicStreamMessageDelta) -> Vec<lunaroute_core::Result<NormalizedStreamEvent>>` — a free function at module scope, unit-tested, called by both `create_anthropic_stream` and the test helper.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-egress/src/anthropic.rs`, inside the existing `mod tests` (starts at line 1026), add this test near the other `test_stream_*` tests (e.g. after `test_stream_finish_reasons` around line 2014):

```rust
        #[tokio::test]
        async fn test_message_delta_with_both_usage_and_stop_emits_both() {
            // Regression: a SINGLE message_delta carrying BOTH output_tokens>0 AND
            // stop_reason must emit Usage THEN End. The scan-with-next() bug dropped End.
            let usage = AnthropicStreamUsage { output_tokens: 42 };
            let delta = AnthropicStreamMessageDelta {
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
            };
            let emitted = emit_message_delta_events(&usage, &delta);
            assert_eq!(emitted.len(), 2, "expected Usage + End");
            assert!(matches!(emitted[0], Ok(NormalizedStreamEvent::Usage { usage }) if usage.completion_tokens == 42));
            assert!(matches!(emitted[1], Ok(NormalizedStreamEvent::End { finish_reason: FinishReason::Stop })));
        }

        #[tokio::test]
        async fn test_message_delta_only_stop_emits_only_end() {
            let usage = AnthropicStreamUsage { output_tokens: 0 };
            let delta = AnthropicStreamMessageDelta {
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
            };
            let emitted = emit_message_delta_events(&usage, &delta);
            assert_eq!(emitted.len(), 1);
            assert!(matches!(emitted[0], Ok(NormalizedStreamEvent::End { finish_reason: FinishReason::ToolCalls })));
        }

        #[tokio::test]
        async fn test_message_delta_only_usage_emits_only_usage() {
            let usage = AnthropicStreamUsage { output_tokens: 7 };
            let delta = AnthropicStreamMessageDelta {
                stop_reason: None,
                stop_sequence: None,
            };
            let emitted = emit_message_delta_events(&usage, &delta);
            assert_eq!(emitted.len(), 1);
            assert!(matches!(emitted[0], Ok(NormalizedStreamEvent::Usage { usage }) if usage.completion_tokens == 7));
        }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-egress test_message_delta_with_both_usage_and_stop_emits_both`
Expected: FAIL with `cannot find function 'emit_message_delta_events'` (compile error). This confirms the test exercises the not-yet-extracted function.

- [ ] **Step 3: Extract the pure `emit_message_delta_events` helper**

In `crates/lunaroute-egress/src/anthropic.rs`, add this free function just above `fn create_anthropic_stream` (around line 813):

```rust
/// Emit normalized events for a single Anthropic `message_delta`.
///
/// A real Anthropic `message_delta` typically carries BOTH `usage.output_tokens`
/// and `delta.stop_reason` in the same event. We emit `Usage` first (if any),
/// then `End` (if any). Returns an empty vec if neither is present.
///
/// Extracted as a pure function so both `create_anthropic_stream` and the
/// test helper exercise the same logic.
fn emit_message_delta_events(
    usage: &AnthropicStreamUsage,
    delta: &AnthropicStreamMessageDelta,
) -> Vec<lunaroute_core::Result<NormalizedStreamEvent>> {
    let mut out = Vec::new();
    if usage.output_tokens > 0 {
        out.push(Ok(NormalizedStreamEvent::Usage {
            usage: Usage {
                prompt_tokens: 0, // Not available in delta
                completion_tokens: usage.output_tokens,
                total_tokens: usage.output_tokens,
            },
        }));
    }
    if let Some(stop_reason) = &delta.stop_reason {
        let reason = match stop_reason.as_str() {
            "end_turn" => FinishReason::Stop,
            "max_tokens" => FinishReason::Length,
            "tool_use" => FinishReason::ToolCalls,
            "stop_sequence" => FinishReason::Stop,
            _ => FinishReason::Stop,
        };
        out.push(Ok(NormalizedStreamEvent::End { finish_reason: reason }));
    }
    out
}
```

(Confirm `AnthropicStreamUsage`, `AnthropicStreamMessageDelta`, `Usage`, `FinishReason`, `NormalizedStreamEvent` are all in scope at this point in the file — they are already used in the existing `scan` closure.)

- [ ] **Step 4: Convert `scan` to emit `Vec` + `flat_map`, and call the helper**

In `create_anthropic_stream` (around line 823), the stream is built as:

```rust
    let stream = event_stream.scan(
        (None, HashMap::new(), HashMap::new()),
        |(stream_id, tool_call_states, tool_args_buffers): &mut (
            Option<String>,
            HashMap<u32, (String, String)>,
            HashMap<u32, String>,
        ),
         result| {
```

Leave the `scan` state and all other arms unchanged, but make these two edits:

**(a) MessageDelta arm** — replace the entire body that builds `events_to_emit` and returns `into_iter().next().unwrap()` (around lines 930-966) with a call to the helper:

```rust
                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                    let events = emit_message_delta_events(&usage, &delta);
                    return futures::future::ready(if events.is_empty() {
                        None
                    } else {
                        Some(events)
                    });
                }
```

**(b) scan item type → `flat_map`.** The `scan` closure currently returns `Option<Result<NormalizedStreamEvent>>` via `futures::future::ready(Some(single))` or `ready(None)`. Change every arm's return value so the closure returns `Option<Vec<Result<NormalizedStreamEvent>>>`:

- Every `return futures::future::ready(Some(single_event));` becomes `return futures::future::ready(Some(vec![single_event]));`
- Every `return futures::future::ready(None);` becomes `return futures::future::ready(None);` (unchanged — `None` already means "emit nothing this item").

(Concretely the arms returning a single event are the `MessageStart`/`ContentBlockStart`/`ContentBlockDelta`/`ToolCallDelta`-style arms; read each `return futures::future::ready(Some(...))` in the closure and wrap its payload in `vec![...]`. The `MessageDelta` arm now returns `Some(events)` from the helper. Arms that returned `None` (`ContentBlockStop`, `MessageStop`, `Ping`, `Unknown`, and the empty-emit cases) stay `None`.)

Then, after the `.scan(...)` call closes, add `.flat_map(|events| futures::stream::iter(events))` so the stream of `Vec` becomes a stream of individual events. The final return wraps it: `Box::pin(stream)` (unchanged).

After the `scan` block's closing `);`, insert:

```rust
    let stream = stream.flat_map(|events| futures::stream::iter(events));
```

(Re-binding to the same `stream` variable keeps the final `Box::pin(stream)` working.)

- [ ] **Step 5: Update the test-local `parse_sse_events` helper to use the real function**

In `mod tests`, the `parse_sse_events` helper (around line 1723) has a `MessageDelta` arm that re-implements the emission. Replace that arm's body with a call to the real helper so the existing stream tests exercise the real logic:

```rust
                    AnthropicStreamEvent::MessageDelta { delta, usage } => {
                        results.extend(emit_message_delta_events(&usage, &delta));
                    }
```

(Leave the `MessageStop`/`Ping`/`Unknown` arms in the helper as-is — they already do nothing for those.)

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p lunaroute-egress --lib anthropic`
Expected: PASS. The three new `test_message_delta_*` tests pass, AND the existing `test_stream_text_content` / `test_stream_tool_calls` / `test_stream_finish_reasons` tests still pass (they use split deltas, which the helper handles identically).

- [ ] **Step 7: Commit**

```bash
git add crates/lunaroute-egress/src/anthropic.rs
git commit -m "fix(egress): emit Anthropic End event when message_delta carries usage+stop

The normalized Anthropic stream used scan (one event per item) and returned
only the first of {Usage, End}, dropping End on the common case where a
single message_delta carries both output_tokens and stop_reason. Extract
emit_message_delta_events as a pure helper, convert scan to Vec+flat_map,
and route the test helper through the same function."
```

---

### Task 3: `Completed` event on early client disconnect (Fix 3 — MEDIUM)

**Files:**
- Modify: `crates/lunaroute-session/src/session_store_recording_provider.rs` (add `impl Drop`; add a `#[cfg(test)]` test constructor; add `mod tests`).

**Interfaces:**
- Consumes: existing `SessionStoreRecordingStream` struct fields (verified present: `inner`, `session_store`, `session_id`, `request_id`, `requested_model`, `started`, `first_event_seen`, `ttft_ms`, `chunk_count`, `usage`, `finish_reason`, `completed`), the existing private method `complete(&mut self, success: bool, error: Option<String>)`, and `spawn_write_event`.
- Produces: `impl Drop for SessionStoreRecordingStream` (no new public surface); a `#[cfg(test)] fn new_for_test(...)` constructor for tests only.

- [ ] **Step 1: Write the failing test**

At the end of `crates/lunaroute-session/src/session_store_recording_provider.rs` (after `fn response_text`), add a test module. Because the struct fields are private and there's no existing test constructor, add a `#[cfg(test)]` constructor inside the `impl SessionStoreRecordingStream` block (Step 3 will add it; the test references it):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use lunaroute_core::session_store::SessionStore;
    use lunaroute_core::tenant::TenantId;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    /// Mock store capturing serialized Completed events.
    struct CapturingStore {
        events: Mutex<Vec<serde_json::Value>>,
    }
    impl CapturingStore {
        fn new() -> Self {
            Self { events: Mutex::new(Vec::new()) }
        }
        fn completed_events(&self) -> Vec<serde_json::Value> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("Completed"))
                .cloned()
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl SessionStore for CapturingStore {
        async fn write_event(
            &self,
            _t: Option<TenantId>,
            event: serde_json::Value,
        ) -> Result<()> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
        async fn search(&self, _t: Option<TenantId>, _q: serde_json::Value) -> Result<serde_json::Value> {
            Ok(serde_json::json!({"sessions": []}))
        }
        async fn get_session(&self, _t: Option<TenantId>, _id: &str) -> Result<serde_json::Value> {
            Ok(serde_json::json!(null))
        }
        async fn cleanup(&self, _t: Option<TenantId>, _r: serde_json::Value) -> Result<serde_json::Value> {
            Ok(serde_json::json!({"deleted": 0}))
        }
        async fn get_stats(&self, _t: Option<TenantId>, _tr: serde_json::Value) -> Result<serde_json::Value> {
            Ok(serde_json::json!({}))
        }
        async fn list_sessions(&self, _t: Option<TenantId>, _l: usize, _o: usize) -> Result<Vec<serde_json::Value>> {
            Ok(Vec::new())
        }
    }

    /// A drop must produce exactly one Completed{success:false, error:"interrupted..."}.
    #[tokio::test]
    async fn drop_without_completion_writes_interrupted_completed() {
        let store = Arc::new(CapturingStore::new()) as Arc<dyn SessionStore>;
        // inner stream that just yields one delta then waits (we won't poll to completion)
        let inner: Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin> =
            Box::new(stream::iter(vec![Ok(NormalizedStreamEvent::Delta {
                index: 0,
                delta: lunaroute_core::normalized::Delta {
                    role: None,
                    content: Some("hi".to_string()),
                },
            })]));
        let mut s = SessionStoreRecordingStream::new_for_test(
            inner,
            store.clone(),
            "sess-1".to_string(),
            "req-1".to_string(),
            "model-x".to_string(),
        );

        // Poll once so it starts but does NOT complete.
        use futures::StreamExt;
        use std::pin::pin;
        let mut pinned = pin!(&mut s);
        let _ = pinned.next().await;

        // Drop without draining to completion — simulates client disconnect.
        drop(s);

        // The Drop spawns the Completed event fire-and-forget; let it land.
        for _ in 0..20 {
            tokio::task::yield_now().await;
            if !store.completed_events().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let completed = store.completed_events();
        assert_eq!(completed.len(), 1, "drop should write exactly one Completed");
        assert_eq!(
            completed[0].get("success").and_then(|v| v.as_bool()),
            Some(false),
            "interrupted completion must be success:false"
        );
        let err = completed[0]
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert!(
            err.contains("interrupted"),
            "error should mention 'interrupted', got: {err}"
        );
    }

    /// A stream that completed normally must NOT produce a second Completed on drop.
    #[tokio::test]
    async fn drop_after_normal_completion_writes_no_duplicate() {
        let store = Arc::new(CapturingStore::new()) as Arc<dyn SessionStore>;
        let inner: Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin> =
            Box::new(stream::iter(vec![
                Ok(NormalizedStreamEvent::Delta {
                    index: 0,
                    delta: lunaroute_core::normalized::Delta {
                        role: None,
                        content: Some("hi".to_string()),
                    },
                }),
                Ok(NormalizedStreamEvent::End {
                    finish_reason: lunaroute_core::normalized::FinishReason::Stop,
                }),
            ]));
        let mut s = SessionStoreRecordingStream::new_for_test(
            inner,
            store.clone(),
            "sess-2".to_string(),
            "req-2".to_string(),
            "model-x".to_string(),
        );
        use futures::StreamExt;
        use std::pin::pin;
        let mut pinned = pin!(&mut s);
        while pinned.next().await.is_some() {}
        drop(s);

        for _ in 0..20 {
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(store.completed_events().len(), 1, "exactly one Completed, no duplicate on drop");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-session --lib session_store_recording_provider`
Expected: FAIL with `cannot find function 'new_for_test'` (compile error). This confirms the test needs the constructor added in Step 3.

- [ ] **Step 3: Add the test constructor and the `Drop` impl**

Inside the existing `impl SessionStoreRecordingStream { ... }` block (the one starting at line 315 that contains `mark_first_event` and `complete`), add a `#[cfg(test)]` constructor:

```rust
    #[cfg(test)]
    fn new_for_test(
        inner: Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>,
        session_store: Arc<dyn SessionStore>,
        session_id: String,
        request_id: String,
        requested_model: String,
    ) -> Self {
        Self {
            inner,
            session_store,
            session_id,
            request_id,
            requested_model,
            started: Instant::now(),
            first_event_seen: false,
            ttft_ms: 0,
            chunk_count: 0,
            usage: None,
            finish_reason: None,
            completed: false,
        }
    }
```

Then, after the `impl Stream for SessionStoreRecordingStream { ... }` block (which ends before `impl Unpin for SessionStoreRecordingStream {}`), add the `Drop` impl:

```rust
impl Drop for SessionStoreRecordingStream {
    fn drop(&mut self) {
        if !self.completed {
            self.complete(false, Some("interrupted: client disconnected".to_string()));
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-session --lib session_store_recording_provider`
Expected: PASS (2 new tests). The existing `recording_provider::tests` still pass (unaffected).

If `NormalizedStreamEvent::Delta`'s `delta` field type or the `Delta` struct fields differ from what's shown above (`role: Option<Role>, content: Option<String>`), inspect `crates/lunaroute-core/src/normalized.rs:286` and construct the `Delta` explicitly with the exact fields.

- [ ] **Step 5: Commit**

```bash
git add crates/lunaroute-session/src/session_store_recording_provider.rs
git commit -m "fix(session): write Completed event on early client disconnect

SessionStoreRecordingStream wrote Completed only from poll_next's EOF/error
arms, so a mid-stream client disconnect (axum drops the body without
polling to completion) left sessions stuck 'in progress'. Add impl Drop
that delegates to the existing complete() with success:false and an
'interrupted: client disconnected' marker, guarded by the completed flag."
```

---

### Task 4: Accumulate OpenAI tool-arg fragments across all deltas (Fix 4 — MEDIUM)

**Files:**
- Modify: `crates/lunaroute-ingress/src/async_stream_parser.rs` (OpenAI `delta.tool_calls` block at ~lines 281-315; add a regression test in `mod tests` at ~line 587).

**Interfaces:**
- Consumes: existing local maps `tool_id_by_index: HashMap<u32,String>`, `tool_args_by_id: HashMap<String,String>`, `tool_calls: HashMap<String,u32>`, `seen_tool_ids`, `tool_name_by_id` (all already declared at the top of the OpenAI parser function).
- Produces: no signature change — `parse_openai_stream` behavior is corrected in place.

- [ ] **Step 1: Write the failing test**

In `crates/lunaroute-ingress/src/async_stream_parser.rs`, inside `mod tests` (starts at line 587), add this test after `test_parse_openai_stream_with_tools` (around line 690):

```rust
    #[tokio::test]
    async fn test_parse_openai_stream_tool_arguments_across_multiple_deltas() {
        // Regression: OpenAI streams tool args across many deltas; only the FIRST
        // delta carries function.name, later deltas carry only function.arguments.
        // The old code nested args accumulation inside the name guard, dropping
        // all post-first fragments.
        let events: Vec<
            Result<eventsource_stream::Event, eventsource_stream::EventStreamError<std::convert::Infallible>>,
        > = vec![
            // delta 1: name + id, no args yet
            Ok(eventsource_stream::Event {
                event: "data".to_string(),
                data: r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_7","function":{"name":"get_weather"}}]}}]}"#
                    .to_string(),
                id: String::new(),
                retry: None,
            }),
            // delta 2: arguments part A, NO name
            Ok(eventsource_stream::Event {
                event: "data".to_string(),
                data: r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"loc\":"}}]}}]}"#
                    .to_string(),
                id: String::new(),
                retry: None,
            }),
            // delta 3: arguments part B, NO name
            Ok(eventsource_stream::Event {
                event: "data".to_string(),
                data: r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"NYC\"}"}}]}}]}"#
                    .to_string(),
                id: String::new(),
                retry: None,
            }),
        ];

        let stream = stream::iter(events);
        let parsed = parse_openai_stream(stream).await;

        assert_eq!(parsed.tool_summary.total_tool_calls, 1);
        assert_eq!(parsed.tool_summary.unique_tool_count, 1);
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].tool_name, "get_weather");
        assert_eq!(
            parsed.tool_calls[0].tool_arguments,
            "{\"loc\":\"NYC\"}",
            "arguments must accumulate across ALL deltas, not just the first"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lunaroute-ingress --lib async_stream_parser::tests::test_parse_openai_stream_tool_arguments_across_multiple_deltas`
Expected: FAIL — `tool_arguments` is empty or just the first fragment (`""`), not `"{\"loc\":\"NYC\"}"`. This confirms the bug.

- [ ] **Step 3: Hoist the arguments accumulation out of the `name` guard**

In `crates/lunaroute-ingress/src/async_stream_parser.rs`, in the OpenAI parser's `delta.tool_calls` block (around lines 281-315), replace the existing block:

```rust
                                    if let Some(function) = tool_call.get("function")
                                        && let Some(name) =
                                            function.get("name").and_then(|n| n.as_str())
                                    {
                                        // Use tool call ID if available, otherwise fall back to index-based tracking
                                        let tool_id = tool_call
                                            .get("id")
                                            .and_then(|id| id.as_str())
                                            .map(|s| s.to_string())
                                            .or_else(|| {
                                                tool_call
                                                    .get("index")
                                                    .and_then(|i| i.as_u64())
                                                    .map(|i| format!("index_{}", i))
                                            })
                                            .unwrap_or_else(|| {
                                                format!("{}_{}", name, seen_tool_ids.len())
                                            });

                                        // Track tool ID by index
                                        tool_id_by_index.insert(index, tool_id.clone());
                                        tool_name_by_id.insert(tool_id.clone(), name.to_string());

                                        // Only count if we haven't seen this tool ID before
                                        if seen_tool_ids.insert(tool_id) {
                                            *tool_calls.entry(name.to_string()).or_insert(0) += 1;
                                        }

                                        // Extract arguments if present
                                        if let Some(arguments) =
                                            function.get("arguments").and_then(|a| a.as_str())
                                        {
                                            // Append to existing arguments (for streaming)
                                            if let Some(tool_id) = tool_id_by_index.get(&index) {
                                                tool_args_by_id
                                                    .entry(tool_id.clone())
                                                    .or_default()
                                                    .push_str(arguments);
                                            }
                                        }
                                    }
```

with the restructured version that separates registration from argument accumulation:

```rust
                                    // First delta for this tool call carries function.name -> register it.
                                    if let Some(function) = tool_call.get("function")
                                        && let Some(name) =
                                            function.get("name").and_then(|n| n.as_str())
                                    {
                                        let tool_id = tool_call
                                            .get("id")
                                            .and_then(|id| id.as_str())
                                            .map(|s| s.to_string())
                                            .or_else(|| {
                                                tool_call
                                                    .get("index")
                                                    .and_then(|i| i.as_u64())
                                                    .map(|i| format!("index_{}", i))
                                            })
                                            .unwrap_or_else(|| {
                                                format!("{}_{}", name, seen_tool_ids.len())
                                            });

                                        tool_id_by_index.insert(index, tool_id.clone());
                                        tool_name_by_id.insert(tool_id.clone(), name.to_string());

                                        if seen_tool_ids.insert(tool_id) {
                                            *tool_calls.entry(name.to_string()).or_insert(0) += 1;
                                        }
                                    }

                                    // Argument fragments arrive on EVERY delta (first + subsequent).
                                    // Hoisted OUT of the name guard: later deltas have no name.
                                    if let Some(function) = tool_call.get("function")
                                        && let Some(arguments) =
                                            function.get("arguments").and_then(|a| a.as_str())
                                        && let Some(tool_id) = tool_id_by_index.get(&index)
                                    {
                                        tool_args_by_id
                                            .entry(tool_id.clone())
                                            .or_default()
                                            .push_str(arguments);
                                    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p lunaroute-ingress --lib async_stream_parser`
Expected: PASS. The new regression test passes (`tool_arguments == "{\"loc\":\"NYC\"}"`), and the existing `test_parse_openai_stream_with_tools` (single-delta name-only case) still passes — it asserts counts, not arguments.

- [ ] **Step 5: Run the broader ingress suite to confirm no regression**

Run: `cargo test -p lunaroute-ingress`
Expected: PASS (all ingress unit + integration tests green, including `routed_recording` and `passthrough_streaming_recording`).

- [ ] **Step 6: Commit**

```bash
git add crates/lunaroute-ingress/src/async_stream_parser.rs
git commit -m "fix(ingress): accumulate OpenAI tool-arg fragments across all deltas

The OpenAI stream parser nested argument accumulation inside the
function.name guard. Only the first delta carries name; later deltas
carry only arguments, so all post-first fragments were dropped and
recorded tool_arguments were truncated. Hoist the arguments block out
of the name guard."
```

---

## Final Verification

- [ ] **Run the full workspace test suite:** `cargo test --workspace`
Expected: PASS (all 9811+ tests green, same set of DB/real-API tests ignored as before).

- [ ] **Run clippy on changed crates:** `cargo clippy -p lunaroute-session -p lunaroute-egress -p lunaroute-ingress -p lunaroute-server --all-targets`
Expected: no new warnings.

## Self-Review

**1. Spec coverage:**
- Fix 1 (CRITICAL, `TenantScopedStore` decorator + startup wiring) → Task 1. ✓
- Fix 2 (HIGH, Anthropic `End` dropped, `flat_map`/pure helper) → Task 2. ✓
- Fix 3 (MEDIUM, `Completed` on disconnect, `impl Drop` delegating to `complete()`) → Task 3. ✓
- Fix 4 (MEDIUM, OpenAI tool-arg fragments, hoist out of name guard) → Task 4. ✓
- Tests per fix (regression tests that fail before / pass after) → each task Step 1-2. ✓
- Rollout order (1 → 3 → 4 → 2) — the plan implements 1,2,3,4 in task order; the spec's "recommended order" was dependency-soft and tasks are independent, so this is fine. ✓

**2. Placeholder scan:** No "TBD"/"TODO"/"add appropriate" — every step has concrete code, exact commands, and expected output. The one hedged instruction (Task 3 Step 4 note about `StreamDelta::Default`) gives a concrete fallback (inspect the type and construct fields explicitly). ✓

**3. Type consistency:**
- `TenantScopedStore::new(Arc<dyn SessionStore>, Option<TenantId>)` — consistent across Task 1 test, impl, and main.rs wiring. ✓
- `emit_message_delta_events(&AnthropicStreamUsage, &AnthropicStreamMessageDelta) -> Vec<Result<NormalizedStreamEvent>>` — consistent across Task 2 test, definition, and helper call site. ✓
- `SessionStoreRecordingStream::new_for_test(Box<dyn Stream<Item=Result<NormalizedStreamEvent>>+Send+Unpin>, Arc<dyn SessionStore>, String, String, String)` — consistent across Task 3 test and constructor. ✓
- `SessionStore` mock trait impls use the exact trait method signatures from `lunaroute-core/src/session_store.rs` (`Option<TenantId>` first, `serde_json::Value` aliases). ✓
- `parse_openai_stream` is the existing async fn name (confirmed at async_stream_parser.rs:690 test). ✓

No issues found. Plan is complete.
