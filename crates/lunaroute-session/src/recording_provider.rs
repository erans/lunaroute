//! Recording provider wrapper
//!
//! This module provides a Provider wrapper that automatically records all
//! requests and responses to a session store.

use crate::recorder::SessionRecorder;
use crate::session::SessionMetadata;
use lunaroute_core::{
    normalized::{NormalizedRequest, NormalizedResponse, NormalizedStreamEvent},
    provider::{Provider, ProviderCapabilities},
    Result,
};
use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

/// Recording provider wrapper
///
/// Wraps a Provider and records all requests/responses to a SessionRecorder.
pub struct RecordingProvider<P: Provider, R: SessionRecorder> {
    provider: Arc<P>,
    recorder: Arc<R>,
    provider_name: String,
    listener_name: String,
}

impl<P: Provider, R: SessionRecorder> RecordingProvider<P, R> {
    /// Create a new recording provider
    pub fn new(
        provider: Arc<P>,
        recorder: Arc<R>,
        provider_name: String,
        listener_name: String,
    ) -> Self {
        Self {
            provider,
            recorder,
            provider_name,
            listener_name,
        }
    }
}

#[async_trait]
impl<P: Provider + Send + Sync, R: SessionRecorder + Send + Sync + 'static> Provider
    for RecordingProvider<P, R>
{
    async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
        // Generate session ID
        let session_id = self.recorder.generate_session_id();

        // Create initial metadata
        let mut metadata = SessionMetadata::new(
            session_id.clone(),
            request.model.clone(),
            self.provider_name.clone(),
            self.listener_name.clone(),
        )
        .with_streaming(false);

        // Start recording session
        let start = std::time::Instant::now();
        if let Err(e) = self
            .recorder
            .start_session(session_id.clone(), &request, metadata.clone())
            .await
        {
            tracing::error!(error = %e, "Failed to start session recording");
        }

        // Execute request
        let result = self.provider.send(request).await;
        let latency = start.elapsed().as_secs_f64();

        match &result {
            Ok(response) => {
                // Record successful response
                if let Err(e) = self
                    .recorder
                    .record_response(&session_id, response)
                    .await
                {
                    tracing::error!(error = %e, "Failed to record response");
                }

                // Update metadata with success info
                metadata = metadata
                    .with_usage(response.usage.prompt_tokens, response.usage.completion_tokens)
                    .with_success(
                        latency,
                        response.choices.first().and_then(|c| c.finish_reason.as_ref()).map(|r| format!("{:?}", r)),
                    );
            }
            Err(e) => {
                // Update metadata with error info
                metadata = metadata.with_error(e.to_string(), latency);
            }
        }

        // Complete session recording
        if let Err(e) = self.recorder.complete_session(&session_id, metadata).await {
            tracing::error!(error = %e, "Failed to complete session recording");
        }

        result
    }

    async fn stream(
        &self,
        request: NormalizedRequest,
    ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
        // Generate session ID
        let session_id = self.recorder.generate_session_id();

        // Create initial metadata
        let metadata = SessionMetadata::new(
            session_id.clone(),
            request.model.clone(),
            self.provider_name.clone(),
            self.listener_name.clone(),
        )
        .with_streaming(true);

        // Start recording session
        let start = std::time::Instant::now();
        if let Err(e) = self
            .recorder
            .start_session(session_id.clone(), &request, metadata.clone())
            .await
        {
            tracing::error!(error = %e, "Failed to start session recording");
        }

        // Execute streaming request
        let stream = self.provider.stream(request).await?;

        // Create channel for ordered event recording
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<NormalizedStreamEvent>();

        // Spawn background task to record events sequentially
        let recorder_clone = self.recorder.clone();
        let session_id_clone = session_id.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let Err(e) = recorder_clone.record_stream_event(&session_id_clone, &event).await {
                    tracing::error!(session_id = %session_id_clone, error = %e, "Failed to record stream event");
                }
            }
        });

        // Wrap the stream to record events
        let recording_stream = RecordingStream {
            inner: Box::pin(stream),
            recorder: self.recorder.clone(),
            session_id: session_id.clone(),
            metadata,
            start_time: start,
            prompt_tokens: 0,
            completion_tokens: 0,
            finish_reason: None,
            had_error: false,
            event_tx,
        };

        Ok(Box::new(recording_stream))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.provider.capabilities()
    }
}

/// Stream wrapper that records events
struct RecordingStream<R: SessionRecorder> {
    inner: Pin<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send>>,
    recorder: Arc<R>,
    session_id: String,
    metadata: SessionMetadata,
    start_time: std::time::Instant,
    prompt_tokens: u32,
    completion_tokens: u32,
    finish_reason: Option<String>,
    had_error: bool,
    // Channel for ordered event recording
    event_tx: mpsc::UnboundedSender<NormalizedStreamEvent>,
}

impl<R: SessionRecorder + 'static> Stream for RecordingStream<R> {
    type Item = Result<NormalizedStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                let event_clone = event.clone();

                // Track usage and finish reason
                match &event {
                    NormalizedStreamEvent::Usage { usage } => {
                        self.prompt_tokens = usage.prompt_tokens;
                        self.completion_tokens = usage.completion_tokens;
                    }
                    NormalizedStreamEvent::End { finish_reason } => {
                        self.finish_reason = Some(format!("{:?}", finish_reason));
                    }
                    _ => {}
                }

                // Send event to channel for ordered recording
                // Ignore send errors (channel closed means recorder task ended)
                let _ = self.event_tx.send(event_clone);

                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.had_error = true;

                // Complete session with error
                let latency = self.start_time.elapsed().as_secs_f64();
                let mut metadata = self.metadata.clone();
                metadata = metadata.with_error(e.to_string(), latency);

                let recorder = self.recorder.clone();
                let session_id = self.session_id.clone();

                tokio::spawn(async move {
                    if let Err(err) = recorder.complete_session(&session_id, metadata).await {
                        tracing::error!(error = %err, "Failed to complete session with error");
                    }
                });

                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                // Stream ended, complete session
                if !self.had_error {
                    let latency = self.start_time.elapsed().as_secs_f64();
                    let mut metadata = self.metadata.clone();

                    if self.prompt_tokens > 0 || self.completion_tokens > 0 {
                        metadata = metadata.with_usage(self.prompt_tokens, self.completion_tokens);
                    }

                    metadata = metadata.with_success(latency, self.finish_reason.clone());

                    let recorder = self.recorder.clone();
                    let session_id = self.session_id.clone();

                    tokio::spawn(async move {
                        if let Err(e) = recorder.complete_session(&session_id, metadata).await {
                            tracing::error!(error = %e, "Failed to complete session");
                        }
                    });
                }

                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<R: SessionRecorder + 'static> Unpin for RecordingStream<R> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::FileSessionRecorder;
    use lunaroute_core::normalized::{Choice, Delta, FinishReason, Message, MessageContent, Role, Usage};
    use futures::StreamExt;
    use std::collections::HashMap;
    use tempfile::TempDir;

    // Mock provider for testing
    #[derive(Clone)]
    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn send(&self, request: NormalizedRequest) -> Result<NormalizedResponse> {
            Ok(NormalizedResponse {
                id: "test-id".to_string(),
                model: request.model,
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: Role::Assistant,
                        content: MessageContent::Text("Test response".to_string()),
                        name: None,
                        tool_calls: vec![],
                        tool_call_id: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                }],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 20,
                    total_tokens: 30,
                },
                created: 1234567890,
                metadata: HashMap::new(),
            })
        }

        async fn stream(
            &self,
            _request: NormalizedRequest,
        ) -> Result<Box<dyn Stream<Item = Result<NormalizedStreamEvent>> + Send + Unpin>> {
            let events = vec![
                Ok(NormalizedStreamEvent::Start {
                    id: "stream-id".to_string(),
                    model: "test-model".to_string(),
                }),
                Ok(NormalizedStreamEvent::Delta {
                    index: 0,
                    delta: Delta {
                        role: Some(Role::Assistant),
                        content: Some("Hello".to_string()),
                    },
                }),
                Ok(NormalizedStreamEvent::Usage {
                    usage: Usage {
                        prompt_tokens: 5,
                        completion_tokens: 3,
                        total_tokens: 8,
                    },
                }),
                Ok(NormalizedStreamEvent::End {
                    finish_reason: FinishReason::Stop,
                }),
            ];

            Ok(Box::new(futures::stream::iter(events)))
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
            }
        }
    }

    fn create_test_request() -> NormalizedRequest {
        NormalizedRequest {
            model: "gpt-5-mini".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("test".to_string()),
                name: None,
                tool_calls: vec![],
                tool_call_id: None,
            }],
            system: None,
            max_tokens: Some(100),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            stream: false,
            tools: vec![],
            tool_choice: None,
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_recording_provider_send() {
        let temp_dir = TempDir::new().unwrap();
        let recorder = Arc::new(FileSessionRecorder::new(temp_dir.path()));
        let provider = Arc::new(MockProvider);

        let recording_provider =
            RecordingProvider::new(provider, recorder.clone(), "test-provider".to_string(), "openai".to_string());

        let request = create_test_request();
        let response = recording_provider.send(request).await.unwrap();

        assert_eq!(response.usage.total_tokens, 30);

        // Small delay to ensure recording completes
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Query sessions
        let sessions = recorder.query_sessions(&crate::session::SessionQuery::new()).await.unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].model, "gpt-5-mini");
        assert_eq!(sessions[0].provider, "test-provider");
        assert!(sessions[0].success);
        assert_eq!(sessions[0].total_tokens, Some(30));
    }

    #[tokio::test]
    async fn test_recording_provider_stream() {
        let temp_dir = TempDir::new().unwrap();
        let recorder = Arc::new(FileSessionRecorder::new(temp_dir.path()));
        let provider = Arc::new(MockProvider);

        let recording_provider =
            RecordingProvider::new(provider, recorder.clone(), "test-provider".to_string(), "openai".to_string());

        let request = create_test_request();
        let mut stream = recording_provider.stream(request).await.unwrap();

        let mut event_count = 0;
        while (stream.next().await).is_some() {
            event_count += 1;
        }

        assert_eq!(event_count, 4);

        // Small delay to ensure recording completes
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Query sessions
        let sessions = recorder.query_sessions(&crate::session::SessionQuery::new()).await.unwrap();

        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].streaming);
        assert!(sessions[0].success);
        assert_eq!(sessions[0].total_tokens, Some(8));
    }
}
