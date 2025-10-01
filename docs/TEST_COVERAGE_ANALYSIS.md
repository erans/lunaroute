# Test Coverage Analysis - Phase 3 Ingress Layer

**Date**: 2025-09-30
**Coverage**: 52.25% (197/377 lines)

---

## Summary

We have **20 tests** with good happy-path coverage, but significant gaps in:
- Error handling (0% coverage)
- Edge cases (partial coverage)
- Integration tests (0% coverage)
- Critical middleware (body_size_limit: 0%)

## Detailed Coverage Breakdown

### ✅ Well-Tested (80%+ coverage)

#### openai.rs - 87% (55/63 lines)
- ✅ Basic request conversion
- ✅ Response conversion
- ✅ Role mapping (system, user, assistant, tool)
- ✅ Invalid role rejection
- ✅ Endpoint handler
- ❌ Missing: Multimodal Parts handling, all finish_reasons, router integration

#### anthropic.rs - 87% (48/55 lines)
- ✅ Basic request conversion
- ✅ Response conversion
- ✅ System prompt handling
- ✅ Invalid role rejection
- ✅ Endpoint handler
- ❌ Missing: Some finish_reason paths, router integration

### ⚠️ Partially Tested (60-79% coverage)

#### types.rs - 75% (54/72 lines)
- ✅ RequestId generation
- ✅ TraceContext parsing
- ✅ RequestMetadata builder
- ✅ StreamEvent SSE formatting
- ❌ **Missing: IngressError → HTTP response (18 uncovered lines)**
- ❌ Missing: Error JSON serialization
- ❌ Missing: All error variants

#### middleware.rs - 63% (40/63 lines)
- ✅ Basic request context
- ✅ Traceparent parsing
- ✅ CORS headers
- ✅ Security headers
- ❌ **Missing: body_size_limit_middleware (COMPLETELY UNTESTED)**
- ❌ **Missing: extract_metadata helper (COMPLETELY UNTESTED)**
- ❌ Missing: X-Forwarded-For edge cases
- ❌ Missing: Malformed headers

---

## Critical Untested Code

### 1. body_size_limit_middleware (Priority: CRITICAL)
**Lines**: 65, 71-75, 78
**Coverage**: 0%

```rust
pub async fn body_size_limit_middleware(
    req: Request,
    next: Next,
    max_size: usize,
) -> Result<Response, StatusCode>
```

**Why Critical**:
- Security feature preventing DoS attacks
- Returns StatusCode::PAYLOAD_TOO_LARGE
- Has a bug (parameter passing issue noted by reviewer)

**Tests Needed**:
- Request within size limit → OK
- Request exceeding limit → 413 Payload Too Large
- Request without Content-Length → OK (pass through)
- Edge case: exactly at limit

---

### 2. IngressError HTTP Response Conversion (Priority: HIGH)
**Lines**: 136, 139-161
**Coverage**: 0%

```rust
impl axum::response::IntoResponse for IngressError {
    fn into_response(self) -> axum::response::Response {
        // Maps errors to HTTP status codes and JSON
    }
}
```

**Why Critical**:
- Every error goes through this
- Returns JSON error responses to clients
- Different status codes per error type

**Tests Needed**:
- InvalidRequest → 400 with JSON body
- MissingHeader → 400 with JSON body
- AuthenticationFailed → 401 with JSON body
- RequestTooLarge → 413 with JSON body
- Timeout → 408 with JSON body
- Serialization error → 400 with JSON body
- Internal error → 500 with JSON body
- Verify JSON structure matches spec

---

### 3. extract_metadata Helper (Priority: MEDIUM)
**Lines**: 128-142
**Coverage**: 0%

```rust
pub fn extract_metadata(headers: &HeaderMap) -> Option<RequestMetadata>
```

**Why Important**:
- Fallback for when extensions aren't available
- Duplicates logic from middleware

**Tests Needed**:
- Extract traceparent from headers
- Extract user agent from headers
- Handle missing headers gracefully

---

## Missing Test Scenarios

### Edge Cases (Priority: HIGH)

#### OpenAI Adapter
- [ ] Empty messages array
- [ ] Multiple choices in response (n > 1)
- [ ] All finish_reason variants:
  - [ ] "stop"
  - [ ] "length"
  - [ ] "tool_calls"
  - [ ] "content_filter"
  - [ ] "error"
- [ ] Message with all optional fields
- [ ] Message with no optional fields
- [ ] Multimodal Parts content (currently returns empty string)
- [ ] Very long message content (>100K chars)

#### Anthropic Adapter
- [ ] Empty messages array
- [ ] System prompt as string
- [ ] System prompt as array (if supported)
- [ ] All finish_reason variants:
  - [ ] "end_turn"
  - [ ] "max_tokens"
  - [ ] "stop_sequence" (with actual sequence)
  - [ ] "tool_use"
- [ ] Multiple stop sequences hit

#### Middleware
- [ ] Malformed traceparent header
- [ ] Multiple IPs in X-Forwarded-For (take first)
- [ ] X-Real-IP fallback when X-Forwarded-For missing
- [ ] Very long User-Agent string
- [ ] Missing all optional headers
- [ ] Empty header values

---

### Error Handling (Priority: CRITICAL)

#### Request Validation
- [ ] Missing required field (model)
- [ ] Missing required field (messages)
- [ ] Invalid temperature (< 0 or > 2.0)
- [ ] Invalid top_p (< 0 or > 1.0)
- [ ] Negative max_tokens
- [ ] Empty string in required field

#### HTTP Errors
- [ ] Each IngressError variant → correct status code
- [ ] Error JSON structure matches OpenAI/Anthropic format
- [ ] Error messages are helpful and don't leak internals

---

### Integration Tests (Priority: HIGH)

#### Full Request Cycle
- [ ] POST /v1/chat/completions with valid request
- [ ] POST /v1/messages with valid request
- [ ] Request → middleware → handler → response
- [ ] Verify response headers (x-request-id, CORS, security)

#### Router Tests
- [ ] openai::router() mounts correctly
- [ ] anthropic::router() mounts correctly
- [ ] 404 for unknown routes
- [ ] OPTIONS request handling (CORS preflight)

#### Middleware Chain
- [ ] Middleware executes in correct order
- [ ] Request metadata available in handlers
- [ ] All middleware headers present in response

---

## Recommended Test Additions

### Immediate (Before Phase 4)

**1. Add body_size_limit_middleware tests** (5 tests)
```rust
#[tokio::test]
async fn test_body_size_within_limit()
async fn test_body_size_exceeds_limit()
async fn test_body_size_no_content_length()
async fn test_body_size_at_limit()
```

**2. Add IngressError response tests** (7 tests)
```rust
#[test]
fn test_error_invalid_request_response()
fn test_error_missing_header_response()
fn test_error_authentication_failed_response()
fn test_error_request_too_large_response()
fn test_error_timeout_response()
fn test_error_serialization_response()
fn test_error_internal_response()
```

**3. Add edge case tests** (10 tests)
```rust
// OpenAI
fn test_empty_messages_array()
fn test_multiple_choices()
fn test_all_finish_reasons()
fn test_multimodal_parts_content()

// Anthropic
fn test_anthropic_empty_messages()
fn test_anthropic_all_finish_reasons()

// Middleware
fn test_malformed_traceparent()
fn test_multiple_forwarded_ips()
fn test_extract_metadata_helper()
fn test_missing_all_headers()
```

**Total: ~22 new tests** → Would bring coverage to **~70-75%**

### Phase 4+ Enhancements

**4. Integration tests** (10 tests)
- Full request/response cycles
- Router mounting
- Middleware chain verification
- CORS preflight
- Error responses end-to-end

**5. Property-based tests** (optional)
- Fuzz testing for malformed inputs
- Random valid requests always succeed
- All errors return valid JSON

---

## Current Test Quality Assessment

### Strengths ✅
- Good happy-path coverage
- Tests are well-structured and clear
- Good use of test data fixtures
- Tests run fast (<0.01s)

### Weaknesses ❌
- No negative testing (error cases)
- No integration tests
- Missing critical middleware tests
- No boundary value testing
- No property-based testing

---

## Recommendation

**Minimum viable**: Add 22 tests (body size, error responses, edge cases)
**Target coverage**: 70-75% (currently 52%)
**Time estimate**: 1-2 hours for 22 tests

**Should we add these now before Phase 4?**
Yes, because:
1. body_size_limit is a security feature
2. Error handling affects user experience
3. Edge cases will bite us later
4. Better to test now than debug in production
