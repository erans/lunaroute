//! End-to-end integration tests for LunaRoute
//!
//! These tests wire together ingress and egress layers to verify
//! the full request flow through the gateway.

#[cfg(test)]
mod e2e_tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use lunaroute_egress::openai::{OpenAIConfig, OpenAIConnector};
    use lunaroute_ingress::openai;
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn test_e2e_openai_chat_completion() {
        // Start mock OpenAI server
        let mock_server = MockServer::start().await;

        // Mock OpenAI API response
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-e2e-test",
                "object": "chat.completion",
                "created": 1234567890,
                "model": "gpt-4",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello! I'm the mocked OpenAI API. How can I help you?"
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 20,
                    "completion_tokens": 15,
                    "total_tokens": 35
                }
            })))
            .mount(&mock_server)
            .await;

        // Create egress connector pointing to mock server
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: mock_server.uri(),
            organization: None,
            client_config: Default::default(),
        };
        let connector = Arc::new(OpenAIConnector::new(config).unwrap());

        // Create ingress router with the connector
        let app = openai::router(connector);

        // Create HTTP request to ingress
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-4",
                    "messages": [
                        {"role": "user", "content": "Hello, how are you?"}
                    ],
                    "temperature": 0.7,
                    "max_tokens": 100
                })
                .to_string(),
            ))
            .unwrap();

        // Send request through the full stack
        let response = app.oneshot(request).await.unwrap();

        // Verify response
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Verify the response went through the full flow
        assert_eq!(json["id"], "chatcmpl-e2e-test");
        assert_eq!(json["model"], "gpt-4");
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(
            json["choices"][0]["message"]["content"],
            "Hello! I'm the mocked OpenAI API. How can I help you?"
        );
        assert_eq!(json["choices"][0]["message"]["role"], "assistant");
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
        assert_eq!(json["usage"]["prompt_tokens"], 20);
        assert_eq!(json["usage"]["completion_tokens"], 15);
        assert_eq!(json["usage"]["total_tokens"], 35);
    }

    #[tokio::test]
    async fn test_e2e_openai_with_system_message() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-system",
                "object": "chat.completion",
                "created": 1234567890,
                "model": "gpt-4",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "I am a helpful assistant"
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 30,
                    "completion_tokens": 8,
                    "total_tokens": 38
                }
            })))
            .mount(&mock_server)
            .await;

        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: mock_server.uri(),
            organization: None,
            client_config: Default::default(),
        };
        let connector = Arc::new(OpenAIConnector::new(config).unwrap());
        let app = openai::router(connector);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-4",
                    "messages": [
                        {"role": "system", "content": "You are a helpful assistant"},
                        {"role": "user", "content": "Who are you?"}
                    ]
                })
                .to_string(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["choices"][0]["message"]["content"], "I am a helpful assistant");
    }

    #[tokio::test]
    async fn test_e2e_openai_provider_error() {
        let mock_server = MockServer::start().await;

        // Mock API error
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": {
                    "message": "Internal server error",
                    "type": "server_error"
                }
            })))
            .mount(&mock_server)
            .await;

        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: mock_server.uri(),
            organization: None,
            client_config: Default::default(),
        };
        let connector = Arc::new(OpenAIConnector::new(config).unwrap());
        let app = openai::router(connector);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-4",
                    "messages": [
                        {"role": "user", "content": "Test"}
                    ]
                })
                .to_string(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should return 502 Bad Gateway (provider error)
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Provider error"));
    }

    #[tokio::test]
    async fn test_e2e_openai_validation_error() {
        let mock_server = MockServer::start().await;

        // Mock server should not be called
        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: mock_server.uri(),
            organization: None,
            client_config: Default::default(),
        };
        let connector = Arc::new(OpenAIConnector::new(config).unwrap());
        let app = openai::router(connector);

        // Invalid request (temperature out of range)
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-4",
                    "messages": [
                        {"role": "user", "content": "Test"}
                    ],
                    "temperature": 5.0  // Invalid: > 2.0
                })
                .to_string(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Should fail at ingress validation before hitting egress
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("temperature"));
    }

    #[tokio::test]
    async fn test_e2e_openai_with_tools() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-tools",
                "object": "chat.completion",
                "created": 1234567890,
                "model": "gpt-4",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": "call_weather",
                            "type": "function",
                            "function": {
                                "name": "get_weather",
                                "arguments": "{\"location\":\"San Francisco\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {
                    "prompt_tokens": 25,
                    "completion_tokens": 15,
                    "total_tokens": 40
                }
            })))
            .mount(&mock_server)
            .await;

        let config = OpenAIConfig {
            api_key: "test-key".to_string(),
            base_url: mock_server.uri(),
            organization: None,
            client_config: Default::default(),
        };
        let connector = Arc::new(OpenAIConnector::new(config).unwrap());
        let app = openai::router(connector);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-4",
                    "messages": [
                        {"role": "user", "content": "What's the weather in San Francisco?"}
                    ],
                    "tools": [{
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "description": "Get weather for a location",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "location": {"type": "string"}
                                },
                                "required": ["location"]
                            }
                        }
                    }]
                })
                .to_string(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(json["choices"][0]["message"]["tool_calls"][0]["id"], "call_weather");
        assert_eq!(
            json["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
    }
}
