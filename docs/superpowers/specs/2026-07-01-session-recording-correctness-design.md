# Session Recording Correctness — Design

**Date:** 2026-07-01
**Scope:** Batch A of the adversarial code-review findings — session-recording correctness cluster (issues #1, #2, #6, #7).
**Status:** Validated by an independent Codex GPT-5.5 (xhigh) pass that read the actual source (14/15 consolidated issues CONFIRMED, the 15th PARTIAL and not in this batch).

## Context

An adversarial review of the LunaRoute codebase (a Rust local proxy for AI coding assistants) found a CRITICAL defect and three related correctness bugs in the session-recording subsystem. Session recording persists every proxied LLM interaction (prompts, responses, tool calls, token usage, streaming stats) to either SQLite (single-tenant) or PostgreSQL (multi-tenant). The CRITICAL bug means that in **every PostgreSQL-enabled deployment, 100% of recording events are silently dropped** — session history, token accounting, and tool-call metrics are never persisted. SQLite mode (the dev default) works, which is why this went unnoticed.

All four findings were independently validated against the actual source code by a second model (Codex GPT-5.5 xhigh); see the per-issue evidence below.

## Goals

1. Make session recording actually persist events in PostgreSQL mode (the CRITICAL).
2. Emit the terminal `End` (finish_reason) event on the normalized Anthropic streaming path.
3. Write a `Completed` event when a client disconnects mid-stream, so sessions are not stuck "in progress."
4. Accumulate the full tool-call argument string across OpenAI streaming deltas.

## Non-goals

- Per-request tenant resolution (auth → tenant mapping). The design *enables* this later (the "bridge" shape), but does not implement it.
- Changes to the passthrough (raw-bytes) Anthropic/OpenAI paths — only the normalized provider path is in scope for #2.
- The other review batches (HTTP body hardening, retry/rate-limit consistency, secrets-at-rest/logging, async hygiene). Each gets its own spec.
- Refactoring `AppState`. It already has the right shape; we route around it rather than expand its surface.

## Tenant model

`tenant_id` is a **process-wide startup constant**, not per-request:

- **File/SQLite mode** (`bootstrap.source == File`): `tenant_id = None` (main.rs:548).
- **Database/Postgres mode** (`bootstrap.source == Database`): `tenant_id = bootstrap.tenant_id.map(TenantId::from_uuid)` (main.rs:569-571). Always `Some` in a correctly configured DB deployment.

The `SessionStore` trait (`crates/lunaroute-core/src/session_store.rs:56`) already encodes the contract: `write_event(tenant_id: Option<TenantId>, event)` where `None` = single-tenant and `Some` = multi-tenant. `PostgresSessionStore::write_event` enforces this by rejecting `None` with `Error::TenantRequired` (postgres_session_store.rs:545-549).

The bug: recording call sites pass `None` unconditionally (~30 sites: openai.rs, anthropic.rs, async_stream_parser.rs, session_store_recording_provider.rs:427), while the `AppState` that *would* inject `tenant_id` is explicitly unused by routes (main.rs:653-656). Routes use the raw `session_store_for_passthrough`.

---

## Fix 1 (CRITICAL): `TenantScopedStore` decorator

**Issue:** Recording writes pass `None` for tenant_id to a `PostgresSessionStore` that requires `Some`; the error is swallowed via `let _ =` / log-and-continue. All recording is dropped in Postgres mode.

**Approach chosen:** "Bridge" — a thin decorator that resolves an implicit default tenant, leaving the door open for per-request tenancy later without a rewrite. (Alternatives considered and rejected: a new `RecordingStore` trait requiring ~30 call-site edits; or baking a default tenant into `PostgresSessionStore`, which puts tenant *resolution* in the wrong layer.)

### New type

**File:** `crates/lunaroute-session/src/tenant_scoped_store.rs` (new), re-exported from `crates/lunaroute-session/src/lib.rs`.

```rust
/// Wraps a `SessionStore` and resolves an implicit process-wide default tenant
/// for calls that pass `None`. Used for recording, where the caller is the proxy
/// itself (no per-request tenant resolution yet).
///
/// Resolution rule: `resolved = tenant_id.or(self.default_tenant)`.
///
/// - When `default_tenant == Some(t)` (Postgres mode): `None` → `Some(t)`,
///   and an explicit `Some(u)` overrides the default.
/// - When `default_tenant == None` (SQLite/file mode): `None` → `None`
///   (unchanged behavior — SQLite requires `None`).
///
/// Future per-request tenancy: a caller that resolves its own tenant passes
/// `Some(req_tenant)`, which takes precedence. No rewrite of this wrapper needed.
///
/// Layering: this type is a tenant *resolver* in the recording/usage domain.
/// `PostgresSessionStore` remains the tenant *enforcer* and keeps its strict
/// "reject `None`" contract — the wrapper never bypasses it, it ensures the
/// store is called with a resolved `Some`.
pub struct TenantScopedStore {
    inner: Arc<dyn SessionStore>,
    default_tenant: Option<TenantId>,
}

impl TenantScopedStore {
    pub fn new(inner: Arc<dyn SessionStore>, default_tenant: Option<TenantId>) -> Self {
        Self { inner, default_tenant }
    }
}
```

### Trait impl

All 8 trait methods (`write_event`, `search`, `get_session`, `cleanup`, `get_stats`, `flush`, `list_sessions`, and any others) delegate identically:

```rust
async fn write_event(&self, tenant_id: Option<TenantId>, event: SessionEvent) -> Result<()> {
    let tid = tenant_id.or(self.default_tenant);
    self.inner.write_event(tid, event).await
}
```

`flush()` has no tenant arg → delegate directly. Methods with extra args (`search`, `get_session`, `cleanup`, `get_stats`, `list_sessions`) resolve the tenant then pass remaining args through unchanged.

### Startup wiring (single seam)

**File:** `crates/lunaroute-server/src/main.rs`, where `session_store_for_passthrough` is created (~line 633).

Before:
```rust
let session_store_for_passthrough = session_store.clone();
```

After:
```rust
let session_store_for_passthrough = session_store
    .clone()
    .map(|s| Arc::new(TenantScopedStore::new(s, tenant_id)) as Arc<dyn SessionStore>);
```

No other call-site changes. Routers and `SessionStoreRecordingProvider` already receive `session_store_for_passthrough` (main.rs:1191-1260), so they transparently get the scoped store, and every existing `store.write_event(None, ev)` resolves `None` → startup tenant_id.

### Why this is the bridge (option C)

Today `default_tenant` = the startup constant. Later, when a request-scoped auth layer resolves a tenant, that call site passes `Some(req_tenant)` and the wrapper defers to it. The per-request path requires zero changes to this wrapper. The semantic shift — `None` now means "use the process default" rather than "single-tenant" — is desirable here (it *is* the bug fix) and is documented on the type.

### Tests

- Unit (mock store recording the `tenant_id` it received): with `default=Some(T)`, a call with `None` is forwarded as `Some(T)`; a call with `Some(U)` is forwarded as `Some(U)` (override wins).
- Unit: with `default=None`, `None` is forwarded as `None` (SQLite path unchanged).
- Integration (regression test for the CRITICAL): a `PostgresSessionStore` wrapped by `TenantScopedStore(default=Some(tenant))`; assert `write_event(None, event)` now persists a row (the original bug returns `TenantRequired` and persists nothing).

---

## Fix 2 (HIGH): Anthropic normalized stream drops the `End` event

**Issue:** In `create_anthropic_stream` (`crates/lunaroute-egress/src/anthropic.rs`), the `MessageDelta` arm of the stream combinator builds a `Vec` of up to two events (`Usage` then `End`) but returns only the first via `events_to_emit.into_iter().next().unwrap()` because the `scan` combinator yields `Option<T>`, not `Option<Vec<T>>`. The `MessageStop` arm returns `None` believing `End` was already sent. Real Anthropic `message_delta` almost always carries both `usage.output_tokens` and `delta.stop_reason`, so `End` (finish_reason) is almost never emitted on the normalized Anthropic path. The code's own comment admits it: *"For now, return the first event... ideally we'd use flat_map instead."*

**Approach chosen:** Replace the `scan` combinator (one-event slot) with a flattening combinator so each upstream SSE event can yield 0..N normalized events.

### Combinator change

The upstream SSE event stream is mapped to `AnthropicStreamEvent`, then `flat_map`'d through an `emit_events` function that returns a `futures::stream::iter(...)` of normalized events:

```rust
let stream = event_stream
    .map(|chunk| parse_anthropic_event(&chunk))      // SSE → AnthropicStreamEvent
    .flat_map(|ev| futures::stream::iter(emit_events(ev)));  // 1 SSE event → 0..N normalized
```

`emit_events` encodes the per-arm emission rules:

- `MessageDelta { delta, usage }`:
  - If `usage.output_tokens > 0` → emit `Usage`.
  - If `delta.stop_reason.is_some()` → emit `End` (mapped via the existing `end_turn→Stop`, `max_tokens→Length`, `tool_use→ToolCalls`, `stop_sequence→Stop`, `_→Stop` table).
  - Both present → `Usage` then `End`, in that order (preserves the current ordering intent).
- `ContentBlockStart`/`ContentBlockDelta`/`ContentBlockStop`/`MessageStart`/etc. → their current single normalized event (or none), unchanged.
- `MessageStop` → emits nothing. It no longer needs to claim "End already sent" — `End` was emitted by the `MessageDelta` that carried `stop_reason`. (If a stream ends with no `stop_reason`-carrying `MessageDelta`, no `End` is synthesized here; the terminal/EOF handling in `SessionStoreRecordingStream` covers that — see Fix 3.)
- `Ping`/`Unknown` → nothing.

### Alternative rejected

Keep `scan` but stash the extra event in a `pending: Option<NormalizedStreamEvent>` polled out next iteration. Works, but spreads state across poll cycles and re-breaks the next time an arm needs 3 events. `flat_map` is the idiomatic fix the code already asks for.

### Behavior contract

- `Usage`, when emitted, always precedes `End` in the same delta.
- `End` is emitted at most once per stream, on the first `MessageDelta` carrying `stop_reason`.
- Passthrough Anthropic (raw bytes) is untouched — only the normalized provider path changes.

### Tests

The existing tests at anthropic.rs:1317-1453 cover the `stop_reason` cases; re-run them to confirm none regress. Add (regression tests, fail today / pass after):
- `message_delta` with both `output_tokens > 0` AND `stop_reason` → yields `Usage` then `End` (this is the regression test; fails today, passes after).
- `message_delta` with only `stop_reason` (no usage) → yields just `End`.
- `message_delta` with only `usage` (no stop_reason) → yields just `Usage`, defers `End` until a later stop.

---

## Fix 3 (MEDIUM): `Completed` event lost on early client disconnect

**Issue:** `SessionStoreRecordingStream` (`crates/lunaroute-session/src/session_store_recording_provider.rs`) writes the terminal `Completed` event only from `complete()`, called inside `poll_next` on `Poll::Ready(None)`/`Some(Err)`. On early client disconnect, axum drops the response body without polling to completion, so `complete()` never runs and the session record stays "in progress" with no `Completed`.

**Approach chosen (simplified from the original sketch):** Add `impl Drop` that delegates to the existing `complete()`. The existing `complete()` already (a) guards double-completion via `if self.completed { return; }`, (b) sets `self.completed = true`, and (c) spawns the `Completed` event with the full `FinalSessionStats`. So `Drop` is a 4-line guard, not a parallel event construction.

### Drop impl

**File:** `crates/lunaroute-session/src/session_store_recording_provider.rs`.

```rust
impl Drop for SessionStoreRecordingStream {
    fn drop(&mut self) {
        if !self.completed {
            self.complete(false, Some("interrupted: client disconnected".to_string()));
        }
    }
}
```

### Why the "interrupted" marker

The drop-path event uses `success: false` with `error: Some("interrupted: client disconnected")`. This distinguishes *client disconnect* from *upstream failure* (which `complete()` reaches via `Poll::Ready(Some(Err))` with the upstream error message). Observability benefit: a dashboard can compute disconnect rate separately from error rate. The normal completion paths (`Poll::Ready(None)` → `complete(true, None)`, error → `complete(false, Some(err))`) keep their existing semantics.

### Why fire-and-forget `spawn_write_event`

`Drop` is synchronous and cannot hold a borrowed runtime handle. The existing `complete()` already uses `spawn_write_event` (a `tokio::spawn` wrapper) for exactly this reason, so `Drop` reuses it. Trade-off: during *process* shutdown the runtime may be torn down and the spawn is lost — but that is the graceful-shutdown `flush()` path's responsibility, not `Drop`'s. The in-process client-disconnect case (the actual bug) is covered.

### Struct fields (verified)

The struct already holds every field `complete()` needs: `session_store`, `session_id`, `request_id`, `requested_model`, `started`, `first_event_seen`, `ttft_ms`, `chunk_count`, `usage`, `finish_reason`, `completed`. No new fields required.

### Tests

- Unit (mock store capturing events): construct the stream, poll once (so `completed == false`), drop without draining → assert a `Completed { success: false, error: Some("interrupted: client disconnected") }` was written. This is the regression test (fails today, passes after).
- Unit: poll to natural completion → drop → assert **no** duplicate `Completed` (the `completed` flag guards it; this also confirms the existing `complete()` idempotency guard holds under Drop).

---

## Fix 4 (MEDIUM): OpenAI tool-arg fragments dropped after the first delta

**Issue:** In the OpenAI streaming parser (`crates/lunaroute-ingress/src/async_stream_parser.rs`, the OpenAI `delta.tool_calls` block at ~lines 281-315 — the second parser function in the file, not the Anthropic one at ~145), the `arguments` accumulation is nested inside the `if let Some(name) = function.get("name")` guard. In OpenAI streaming, only the first delta for a tool call carries `function.name`; subsequent deltas carry only `function.arguments` (no `name`), so all post-first-delta argument fragments are skipped. Recorded `tool_arguments` are empty or truncated to the first fragment; tool counts remain correct. The client stream is unaffected — only session recording is wrong.

**Approach chosen:** Hoist the `index → tool_id` registration and the `arguments` accumulation out of the `name` guard so argument fragments accumulate on every delta.

### Restructured block

```rust
if let Some(tool_calls_arr) = delta.get("tool_calls").and_then(|t| t.as_array()) {
    for tool_call in tool_calls_arr {
        let index = tool_call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;

        // First delta for this tool call carries function.name → register it.
        if let Some(function) = tool_call.get("function")
            && let Some(name) = function.get("name").and_then(|n| n.as_str())
        {
            let tool_id = tool_call.get("id").and_then(|id| id.as_str()).map(|s| s.to_string())
                .or_else(|| tool_call.get("index").and_then(|i| i.as_u64()).map(|i| format!("index_{}", i)))
                .unwrap_or_else(|| format!("{}_{}", name, seen_tool_ids.len()));
            tool_id_by_index.insert(index, tool_id.clone());
            tool_name_by_id.insert(tool_id.clone(), name.to_string());
            if seen_tool_ids.insert(tool_id) {
                *tool_calls.entry(name.to_string()).or_insert(0) += 1;
            }
        }

        // Argument fragments arrive on EVERY delta (first + subsequent).
        // Hoisted OUT of the name guard — this is the fix.
        if let Some(function) = tool_call.get("function")
            && let Some(arguments) = function.get("arguments").and_then(|a| a.as_str())
            && let Some(tool_id) = tool_id_by_index.get(&index)
        {
            tool_args_by_id
                .entry(tool_id.clone())
                .or_default()
                .push_str(arguments);
        }
    }
}
```

### Behavior contract

- Tool counts unchanged (the `name` guard still runs first in the same iteration for the first delta).
- `tool_arguments` now accumulates the full JSON argument string across all fragments.
- First-delta behavior identical to today: a delta carrying both `name` and `arguments` registers the name *and* appends the arguments in the same iteration (name guard runs first, args block runs second).
- No effect on the client stream — this parser only feeds session recording.

### Tests

- Unit (regression test, fails today): feed a 3-delta OpenAI tool-call sequence — delta1: `{name, id}`, delta2: `{arguments: part A}` (no name), delta3: `{arguments: part B}` (no name) → assert `tool_arguments == "A"+"B"` and the tool count is 1.
- Unit (no regression): a single delta carrying both `name` and `arguments` → `tool_arguments` contains the args, count is 1.

---

## Validation status

All four fixes target issues independently CONFIRMED by a Codex GPT-5.5 (xhigh) validator that read the actual source:

| # | Issue | Validator verdict | Evidence (file:line read by validator) |
|---|---|---|---|
| 1 | Postgres recording 100% broken | CONFIRMED | postgres_session_store.rs:545-549; recording_provider.rs:427; main.rs:653-656 |
| 2 | Anthropic `End` dropped | CONFIRMED | anthropic.rs:924-960; MessageStop returns None at :963-966 |
| 6 | `Completed` lost on disconnect | CONFIRMED | recording_provider.rs:408-414; no `Drop` impl in file |
| 7 | OpenAI tool-arg fragments dropped | CONFIRMED | async_stream_parser.rs:281-315 (args append inside `name` guard) |

## Rollout

All four fixes are localized and independently shippable. Recommended order (dependency, not hard requirement):

1. **Fix 1** (CRITICAL) first — it's the highest-impact and the wiring seam is tiny.
2. **Fix 3** (Drop) next — small and self-contained.
3. **Fix 4** (parser hoist) — pure restructuring.
4. **Fix 2** (stream combinator) — slightly larger; lands last with its new tests.

No database migrations. No config changes. No public API changes (the new `TenantScopedStore` is internal to `lunaroute-session`).

## Open questions

None at design time. Field names for Fix 3 were verified against the source; the `Drop`-delegates-to-`complete()` simplification removes the only design uncertainty raised in brainstorming.
