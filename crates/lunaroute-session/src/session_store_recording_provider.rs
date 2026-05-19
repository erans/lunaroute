//! Provider wrapper that records normalized routed requests through SessionStore.

use crate::events::{
    FinalSessionStats, PerformanceMetrics, RequestStats, ResponseStats, SessionEvent,
    SessionMetadata, StreamingStats, TokenStats, TokenTotals, ToolStats, ToolUsageSummary,
};
use async_trait::async_trait;
use futures::Stream;
use lunaroute_core::{
    Result,
    normalized::{
        ContentPart, FinishReason, MessageContent, NormalizedRequest, NormalizedResponse,
        NormalizedStreamEvent, Usage,
    },
    provider::{Provider, ProviderCapabilities},
    session_store::SessionStore,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

/// Records requests that go through the normalized Provider path.
pub struct SessionStoreRecordingProvider {
    inner: Arc<dyn Provider>,
    session_store: Arc<dyn SessionStore>,
    provider_name: String,
    listener_name: String,
}

struct CompletionRecord {
    session_id: String,
    request_id: String,
    success: bool,
    error: Option<String>,
    finish_reason: Option<String>,
    total_duration_ms: u64,
    tokens: TokenTotals,
    tool_summary: ToolUsageSummary,
    streaming_stats: Option<StreamingStats>,
}

impl SessionStoreRecordingProvider {
    pub fn new(
        inner: Arc<dyn Provider>,
        session_store: Arc<dyn SessionStore>,
        provider_name: impl Into<String>,
        listener_name: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            session_store,
            provider_name: provider_name.into(),
            listener_name: listener_name.into(),
        }
    }

    async fn record_started(
        &self,
        session_id: String,
        request_id: String,
        request: &NormalizedRequest,
    ) {
        write_event(
            self.session_store.clone(),
            SessionEvent::Started {
                session_id,
                request_id,
                timestamp: chrono::Utc::now(),
                model_requested: request.model.clone(),
                provider: self.provider_name.clone(),
                listener: self.listener_name.clone(),
                is_streaming: request.stream,
                metadata: SessionMetadata {
                    client_ip: None,
                    user_agent: None,
                    api_version: None,
                    request_headers: HashMap::new(),
                    session_tags: Vec::new(),
                },
            },
        )
        .await;
    }

    async fn record_request(
        &self,
        session_id: String,
        request_id: String,
        request: &NormalizedRequest,
        pre_processing_ms: f64,
    ) {
        let request_json = serde_json::to_value(request).unwrap_or(serde_json::Value::Null);
        let request_size_bytes = request_json.to_string().len();
        let has_system_prompt = request.system.is_some()
            || request
                .messages
                .iter()
                .any(|message| matches!(message.role, lunaroute_core::normalized::Role::System));

        write_event(
            self.session_store.clone(),
            SessionEvent::RequestRecorded {
                session_id,
                request_id,
                timestamp: chrono::Utc::now(),
                request_text: request_text(request),
                request_json,
                estimated_tokens: 0,
                stats: RequestStats {
                    pre_processing_ms,
                    request_size_bytes,
                    message_count: request.messages.len(),
                    has_system_prompt,
                    has_tools: !request.tools.is_empty(),
                    tool_count: request.tools.len(),
                },
            },
        )
        .await;
    }

    async fn record_response(
        &self,
        session_id: String,
        request_id: String,
        response: &NormalizedResponse,
        provider_latency_ms: u64,
    ) {
        let response_json = serde_json::to_value(response).unwrap_or(serde_json::Value::Null);
        let response_size_bytes = response_json.to_string().len();

        write_event(
            self.session_store.clone(),
            SessionEvent::ResponseRecorded {
                session_id,
                request_id,
                timestamp: chrono::Utc::now(),
                response_text: response_text(response),
                response_json,
                model_used: response.model.clone(),
                stats: ResponseStats {
                    provider_latency_ms,
                    post_processing_ms: 0.0,
                    total_proxy_overhead_ms: 0.0,
                    tokens: token_stats(response.usage),
                    tool_calls: Vec::new(),
                    response_size_bytes,
                    content_blocks: response.choices.len(),
                    has_refusal: false,
                    is_streaming: false,
                    chunk_count: None,
                    streaming_duration_ms: None,
                },
            },
        )
        .await;
    }

    async fn record_completed(&self, record: CompletionRecord) {
        write_event(
            self.session_store.clone(),
            SessionEvent::Completed {
                session_id: record.session_id,
                request_id: record.request_id,
                timestamp: chrono::Utc::now(),
                success: record.success,
                error: record.error,
                finish_reason: record.finish_reason,
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms: record.total_duration_ms,
                    provider_time_ms: record.total_duration_ms,
                    proxy_overhead_ms: 0.0,
                    total_tokens: record.tokens,
                    tool_summary: record.tool_summary,
                    performance: PerformanceMetrics::default(),
                    streaming_stats: record.streaming_stats,
                    estimated_cost: None,
                }),
            },
        )
        .await;
    }
}

#[async_trait]
impl Provider for SessionStoreRecordingProvider {
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let started = Instant::now();

        self.record_started(session_id.clone(), request_id.clone(), &request)
            .await;
        self.record_request(session_id.clone(), request_id.clone(), &request, 0.0)
            .await;

        let result = self.inner.send(request).await;
        let total_duration_ms = elapsed_ms(started);

        match &result {
            Ok(response) => {
                self.record_response(
                    session_id.clone(),
                    request_id.clone(),
                    response,
                    total_duration_ms,
                )
                .await;

                self.record_completed(CompletionRecord {
                    session_id,
                    request_id,
                    success: true,
                    error: None,
                    finish_reason: response_finish_reason(response),
                    total_duration_ms,
                    tokens: totals_from_usage(response.usage, &response.model),
                    tool_summary: tool_summary_from_response(response),
                    streaming_stats: None,
                })
                .await;
            }
            Err(error) => {
                self.record_completed(CompletionRecord {
                    session_id,
                    request_id,
                    success: false,
                    error: Some(error.to_string()),
                    finish_reason: None,
                    total_duration_ms,
                    tokens: TokenTotals::default(),
                    tool_summary: ToolUsageSummary::default(),
                    streaming_stats: None,
                })
                .await;
            }
        }

        result
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let started = Instant::now();
        let requested_model = request.model.clone();

        self.record_started(session_id.clone(), request_id.clone(), &request)
            .await;
        self.record_request(session_id.clone(), request_id.clone(), &request, 0.0)
            .await;

        match self.inner.stream(request).await {
            Ok(stream) => Ok(Box::new(SessionStoreRecordingStream {
                inner: stream,
                session_store: self.session_store.clone(),
                session_id,
                request_id,
                requested_model,
                started,
                first_event_seen: false,
                ttft_ms: 0,
                chunk_count: 0,
                usage: None,
                finish_reason: None,
                completed: false,
            })),
            Err(error) => {
                self.record_completed(CompletionRecord {
                    session_id,
                    request_id,
                    success: false,
                    error: Some(error.to_string()),
                    finish_reason: None,
                    total_duration_ms: elapsed_ms(started),
                    tokens: TokenTotals::default(),
                    tool_summary: ToolUsageSummary::default(),
                    streaming_stats: None,
                })
                .await;
                Err(error)
            }
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.inner.capabilities()
    }

    fn get_notification_message(&self) -> Option<&str> {
        self.inner.get_notification_message()
    }
}

struct SessionStoreRecordingStream {
    inner: Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>,
    session_store: Arc<dyn SessionStore>,
    session_id: String,
    request_id: String,
    requested_model: String,
    started: Instant,
    first_event_seen: bool,
    ttft_ms: u64,
    chunk_count: u32,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    completed: bool,
}

impl SessionStoreRecordingStream {
    fn mark_first_event(&mut self) {
        if self.first_event_seen {
            return;
        }

        self.first_event_seen = true;
        self.ttft_ms = elapsed_ms(self.started);
        spawn_write_event(
            self.session_store.clone(),
            SessionEvent::StreamStarted {
                session_id: self.session_id.clone(),
                request_id: self.request_id.clone(),
                timestamp: chrono::Utc::now(),
                time_to_first_token_ms: self.ttft_ms,
            },
        );
    }

    fn complete(&mut self, success: bool, error: Option<String>) {
        if self.completed {
            return;
        }

        self.completed = true;
        let total_duration_ms = elapsed_ms(self.started);
        let usage = self.usage.unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });
        let tokens = totals_from_usage(usage, &self.requested_model);
        let streaming_stats = Some(StreamingStats {
            time_to_first_token_ms: self.ttft_ms,
            total_chunks: self.chunk_count,
            streaming_duration_ms: total_duration_ms,
            avg_chunk_latency_ms: if self.chunk_count == 0 {
                0.0
            } else {
                total_duration_ms as f64 / self.chunk_count as f64
            },
            p50_chunk_latency_ms: None,
            p95_chunk_latency_ms: None,
            p99_chunk_latency_ms: None,
            max_chunk_latency_ms: 0,
            min_chunk_latency_ms: 0,
        });

        spawn_write_event(
            self.session_store.clone(),
            SessionEvent::Completed {
                session_id: self.session_id.clone(),
                request_id: self.request_id.clone(),
                timestamp: chrono::Utc::now(),
                success,
                error,
                finish_reason: self.finish_reason.clone(),
                final_stats: Box::new(FinalSessionStats {
                    total_duration_ms,
                    provider_time_ms: total_duration_ms,
                    proxy_overhead_ms: 0.0,
                    total_tokens: tokens,
                    tool_summary: ToolUsageSummary::default(),
                    performance: PerformanceMetrics::default(),
                    streaming_stats,
                    estimated_cost: None,
                }),
            },
        );
    }
}

impl Stream for SessionStoreRecordingStream {
    type Item = Result<NormalizedStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                self.mark_first_event();
                self.chunk_count = self.chunk_count.saturating_add(1);

                match &event {
                    NormalizedStreamEvent::Usage { usage } => {
                        self.usage = Some(*usage);
                    }
                    NormalizedStreamEvent::End { finish_reason } => {
                        self.finish_reason = Some(finish_reason_to_string(*finish_reason));
                    }
                    _ => {}
                }

                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                let message = error.to_string();
                self.complete(false, Some(message));
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.complete(true, None);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Unpin for SessionStoreRecordingStream {}

async fn write_event(session_store: Arc<dyn SessionStore>, event: SessionEvent) {
    match serde_json::to_value(event) {
        Ok(json) => {
            if let Err(error) = session_store.write_event(None, json).await {
                tracing::error!(error = %error, "Failed to write routed session event");
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "Failed to serialize routed session event");
        }
    }
}

fn spawn_write_event(session_store: Arc<dyn SessionStore>, event: SessionEvent) {
    tokio::spawn(async move {
        write_event(session_store, event).await;
    });
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn request_text(request: &NormalizedRequest) -> String {
    request
        .messages
        .last()
        .map(|message| content_to_text(&message.content))
        .unwrap_or_default()
}

fn response_text(response: &NormalizedResponse) -> String {
    response
        .choices
        .iter()
        .map(|choice| content_to_text(&choice.message.content))
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_to_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn response_finish_reason(response: &NormalizedResponse) -> Option<String> {
    response
        .choices
        .first()
        .and_then(|choice| choice.finish_reason)
        .map(finish_reason_to_string)
}

fn finish_reason_to_string(finish_reason: FinishReason) -> String {
    match finish_reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Error => "error",
    }
    .to_string()
}

fn token_stats(usage: Usage) -> TokenStats {
    TokenStats {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        thinking_tokens: None,
        reasoning_tokens: None,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        audio_input_tokens: None,
        audio_output_tokens: None,
        thinking_percentage: None,
        tokens_per_second: None,
    }
}

fn totals_from_usage(usage: Usage, model: &str) -> TokenTotals {
    let mut by_model = HashMap::new();
    by_model.insert(model.to_string(), token_stats(usage));

    TokenTotals {
        total_input: usage.prompt_tokens as u64,
        total_output: usage.completion_tokens as u64,
        total_thinking: 0,
        total_reasoning: 0,
        total_cached: 0,
        total_cache_read: 0,
        total_cache_creation: 0,
        total_audio_input: 0,
        total_audio_output: 0,
        grand_total: usage.total_tokens as u64,
        by_model,
    }
}

fn tool_summary_from_response(response: &NormalizedResponse) -> ToolUsageSummary {
    let mut by_tool = HashMap::new();
    let mut total_tool_calls = 0;

    for tool_call in response
        .choices
        .iter()
        .flat_map(|choice| &choice.message.tool_calls)
    {
        total_tool_calls += 1;
        by_tool
            .entry(tool_call.function.name.clone())
            .and_modify(|stats: &mut ToolStats| {
                stats.call_count = stats.call_count.saturating_add(1);
            })
            .or_insert(ToolStats {
                call_count: 1,
                total_execution_time_ms: 0,
                avg_execution_time_ms: 0,
                error_count: 0,
            });
    }

    ToolUsageSummary {
        total_tool_calls,
        unique_tool_count: by_tool.len() as u32,
        by_tool,
        total_tool_time_ms: 0,
        tool_error_count: 0,
    }
}
