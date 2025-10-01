# API Specification Comparison Report

**Date**: 2025-09-30
**Purpose**: Verify OpenAI and Anthropic API implementations match official specifications

---

## OpenAI Chat Completions API

### Official Specification (from platform.openai.com & Azure docs)

#### Required Request Fields
- `model` (string): Model identifier (e.g., "gpt-4")
- `messages` (array): Array of message objects with `role` and `content`
  - Roles: "system", "user", "assistant", "tool"

#### Optional Request Fields
- `temperature` (number, 0-2, default 1): Sampling temperature
- `top_p` (number, 0-1): Nucleus sampling probability
- `max_tokens` (integer): Maximum tokens to generate
- `stream` (boolean): Enable streaming responses
- `stop` (string or array): Stop sequences
- `n` (integer): Number of completions to generate
- `presence_penalty` (number, -2.0 to 2.0): Encourage new topics
- `frequency_penalty` (number, -2.0 to 2.0): Reduce repetition
- `user` (string): End-user identifier for abuse monitoring
- `tools` (array): Function definitions for tool calling
- `tool_choice` (string/object): Control tool calling behavior
- `response_format` (object): JSON mode configuration
- `seed` (integer): Deterministic sampling attempt
- `logit_bias` (object): Modify token probabilities
- `logprobs` (boolean): Return log probabilities
- `top_logprobs` (integer, 0-5): Number of most likely tokens

#### Response Fields
- `id` (string): Unique completion identifier
- `object` (string): Always "chat.completion"
- `created` (integer): Unix timestamp
- `model` (string): Model used
- `choices` (array): Array of completion choices
  - `index` (integer): Choice index
  - `message` (object): Assistant's response with `role` and `content`
  - `finish_reason` (string): "stop", "length", "tool_calls", "content_filter"
  - `tool_calls` (array, optional): Tool/function calls
- `usage` (object): Token usage statistics
  - `prompt_tokens` (integer)
  - `completion_tokens` (integer)
  - `total_tokens` (integer)
- `system_fingerprint` (string, optional): System configuration identifier

#### Message Object Structure
- `role` (string): "system", "user", "assistant", "tool"
- `content` (string or array): Text or multi-modal content
  - For text: simple string
  - For multi-modal: array of content blocks (text, image_url)
- `name` (string, optional): Name of the author
- `tool_calls` (array, optional): Tool/function calls made by assistant
- `tool_call_id` (string, optional): ID of tool call (for tool role)

### Our Implementation Status

#### ✅ Correctly Implemented
- `model`, `messages` (required fields)
- `temperature`, `top_p`, `max_tokens`, `stream`, `stop`, `n`, `presence_penalty`, `frequency_penalty`, `user`
- Response structure: `id`, `object`, `created`, `model`, `choices`, `usage`
- Choice structure: `index`, `message`, `finish_reason`
- Usage structure: `prompt_tokens`, `completion_tokens`, `total_tokens`
- Finish reasons: "stop", "length", "tool_calls", "content_filter", "error"
- Stream types: `OpenAIStreamChunk`, `OpenAIStreamChoice`, `OpenAIDelta`

#### ❌ Missing Fields
**Request:**
- `tools` (array) - Function definitions ⚠️ **Important for tool calling**
- `tool_choice` (string/object) - Tool selection control
- `response_format` (object) - JSON mode
- `seed` (integer) - Deterministic sampling
- `logit_bias` (object) - Token probability modification
- `logprobs` (boolean) - Log probability return
- `top_logprobs` (integer) - Most likely tokens

**Message:**
- `content` should support array of content blocks (we only support string) ⚠️ **Blocks multimodal**
- `tool_calls` in message (we have it in Message but not in OpenAIMessage)
- `tool_call_id` for tool role messages

**Response:**
- `system_fingerprint` field
- `tool_calls` in choice.message

#### 🐛 Issues
1. **Message.content is string-only**: Should be `string | array` for multimodal content
2. **No tool role support**: Missing "tool" role in role mapping
3. **OpenAIMessage incomplete**: Missing `tool_calls` and `tool_call_id` fields
4. **Finish reason mapping incomplete**: Missing "function_call" (older models)

---

## Anthropic Messages API

### Official Specification (from docs.claude.com)

#### Required Request Fields
- `model` (string): Model identifier (e.g., "claude-sonnet-4-20250514")
  - Length: 1-256 characters
- `messages` (array): Conversation history (limit: 100,000 messages)
  - Each message requires `role` and `content`
  - Roles: "user", "assistant" (alternating turns)
  - Content: string or array of content blocks
- `max_tokens` (integer): Maximum tokens to generate (minimum: 1)

#### Optional Request Fields
- `system` (string or array): System instructions/context
- `temperature` (number, 0.0-1.0): Sampling temperature
- `top_p` (number): Nucleus sampling
- `top_k` (integer): Top-k sampling
- `stop_sequences` (array): Custom stop sequences
- `stream` (boolean): Enable streaming
- `tools` (array): Tool/function definitions
- `tool_choice` (object): Tool selection control
- `metadata` (object): User ID and other tracking data

#### Response Fields
- `id` (string): Unique message identifier
- `type` (string): Always "message"
- `role` (string): Always "assistant"
- `content` (array): Array of content blocks
  - Text blocks: `{"type": "text", "text": "..."}`
  - Tool use blocks: `{"type": "tool_use", "id": "...", "name": "...", "input": {...}}`
  - Thinking blocks: `{"type": "thinking", "thinking": "..."}`
- `model` (string): Model used
- `stop_reason` (string): "end_turn", "max_tokens", "stop_sequence", "tool_use"
- `stop_sequence` (string, optional): Which stop sequence was hit
- `usage` (object): Token usage
  - `input_tokens` (integer)
  - `output_tokens` (integer)

#### Content Block Types
- **Text block**: `{"type": "text", "text": "string"}`
- **Image block**: `{"type": "image", "source": {...}}`
- **Tool use block**: `{"type": "tool_use", "id": "...", "name": "...", "input": {...}}`
- **Tool result block**: `{"type": "tool_result", "tool_use_id": "...", "content": "..."}`
- **Thinking block**: `{"type": "thinking", "thinking": "..."}`

### Our Implementation Status

#### ✅ Correctly Implemented
- `model`, `messages`, `max_tokens` (required fields)
- `system`, `temperature`, `top_p`, `top_k`, `stop_sequences`, `stream`
- Response structure: `id`, `type_`, `role`, `content`, `model`, `stop_reason`, `usage`
- Usage structure: `input_tokens`, `output_tokens`
- Finish reasons: "end_turn", "max_tokens", "tool_use", "content_filter", "error"
- Content blocks: Text type with `text` field

#### ❌ Missing Fields
**Request:**
- `tools` (array) - Tool/function definitions ⚠️ **Important for tool calling**
- `tool_choice` (object) - Tool selection control
- `metadata` (object) - User tracking data
- Message `content` should support array of blocks (we only support string) ⚠️ **Blocks multimodal**

**Response:**
- `stop_sequence` field (which stop sequence was hit)
- Content block types beyond Text:
  - Image blocks
  - Tool use blocks
  - Tool result blocks
  - Thinking blocks

**Message Structure:**
- `content` should be `string | array` not just `string`
- Missing support for image content blocks
- Missing support for tool result content blocks

#### 🐛 Issues
1. **AnthropicMessage.content is string-only**: Should support array of content blocks for multimodal
2. **AnthropicContent enum incomplete**: Only has Text variant, missing Image, ToolUse, ToolResult, Thinking
3. **No stop_sequence field**: Can't identify which stop sequence triggered completion
4. **Finish reason "content_filter" doesn't exist in spec**: Should only be "end_turn", "max_tokens", "stop_sequence", "tool_use"

---

## Critical Issues Summary

### Priority 1: Multimodal Content Support
Both APIs support multimodal content (text + images), but our implementations only handle string content.

**Impact**: Cannot handle image inputs or complex content structures.

**Fix Required**:
```rust
// OpenAI
pub enum OpenAIContent {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

pub struct OpenAIMessage {
    pub role: String,
    pub content: Either<String, Vec<OpenAIContent>>, // Support both formats
    // ...
}

// Anthropic
pub enum AnthropicContent {
    Text { text: String },
    Image { source: ImageSource },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String },
    Thinking { thinking: String },
}

pub struct AnthropicMessage {
    pub role: String,
    pub content: Either<String, Vec<AnthropicContent>>, // Support both formats
}
```

### Priority 2: Tool Calling Support
Both APIs support tool/function calling, but we have no support for:
- Tool definitions in requests
- Tool calls in responses
- Tool result content blocks

**Impact**: Cannot use function calling features of either API.

### Priority 3: Missing Request Fields
**OpenAI**: `tools`, `tool_choice`, `response_format`, `seed`, `logit_bias`, `logprobs`, `top_logprobs`
**Anthropic**: `tools`, `tool_choice`, `metadata`

### Priority 4: Response Completeness
**OpenAI**: Missing `system_fingerprint`, incomplete tool support
**Anthropic**: Missing `stop_sequence`, incomplete content block types

---

## Recommendations

### Immediate Fixes (Before Phase 4)
1. ✅ **Keep current string-only implementation for MVP** - Add TODO comments
2. ✅ **Document limitations** in code comments
3. ⚠️ **Add "tool" role support for OpenAI** - Easy addition
4. ⚠️ **Fix Anthropic finish_reason** - Remove "content_filter", add "stop_sequence"

### Phase 4+ Enhancements
1. Implement multimodal content support (text + images)
2. Implement tool calling support
3. Add missing request parameters
4. Complete response structures
5. Add proper content validation

### Breaking Changes to Avoid
- Don't change field names
- Keep backward compatibility with string content
- Use `Either<String, Vec<ContentBlock>>` for content fields to support both

---

## Conclusion

**Overall Assessment**: Our implementations cover ~70% of the core functionality correctly.

✅ **Strengths**:
- Core request/response structures correct
- Primary fields properly mapped
- Good foundation for expansion

⚠️ **Gaps**:
- No multimodal content support
- No tool calling support
- Missing advanced parameters

**Recommendation**: Current implementation is sufficient for **text-only, non-tool-calling use cases**. Before production, must add multimodal and tool support.
