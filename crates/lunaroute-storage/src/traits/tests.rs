//! Tests for storage trait types

use super::*;

#[test]
fn test_session_metadata_serialization() {
    let metadata = SessionMetadata {
        id: "session_123".to_string(),
        created_at: 1234567890,
        model: "gpt-4".to_string(),
        provider: "openai".to_string(),
        user_id: Some("user_456".to_string()),
        tags: vec!["production".to_string(), "api".to_string()],
        total_tokens: 1000,
        total_cost: Some(0.03),
    };

    let json = serde_json::to_string(&metadata).unwrap();
    let deserialized: SessionMetadata = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.id, "session_123");
    assert_eq!(deserialized.total_tokens, 1000);
    assert_eq!(deserialized.tags.len(), 2);
}

#[test]
fn test_session_filter_default() {
    let filter = SessionFilter::default();

    assert!(filter.user_id.is_none());
    assert!(filter.provider.is_none());
    assert!(filter.model.is_none());
    assert!(filter.start_time.is_none());
    assert!(filter.end_time.is_none());
    assert_eq!(filter.tags.len(), 0);
    assert!(filter.limit.is_none());
}

#[test]
fn test_session_filter_with_values() {
    let filter = SessionFilter {
        user_id: Some("user_123".to_string()),
        provider: Some("openai".to_string()),
        model: Some("gpt-4".to_string()),
        start_time: Some(1000000),
        end_time: Some(2000000),
        tags: vec!["test".to_string()],
        limit: Some(100),
    };

    assert_eq!(filter.user_id.as_ref().unwrap(), "user_123");
    assert_eq!(filter.limit.unwrap(), 100);
}

#[test]
fn test_session_info_serialization() {
    let info = SessionInfo {
        id: "session_123".to_string(),
        metadata: SessionMetadata {
            id: "session_123".to_string(),
            created_at: 1234567890,
            model: "gpt-4".to_string(),
            provider: "openai".to_string(),
            user_id: None,
            tags: vec![],
            total_tokens: 500,
            total_cost: None,
        },
        size_bytes: 1024,
    };

    let json = serde_json::to_string(&info).unwrap();
    let deserialized: SessionInfo = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.size_bytes, 1024);
}

#[test]
fn test_session_data_structure() {
    let data = SessionData {
        metadata: SessionMetadata {
            id: "session_123".to_string(),
            created_at: 1234567890,
            model: "gpt-4".to_string(),
            provider: "openai".to_string(),
            user_id: Some("user_456".to_string()),
            tags: vec![],
            total_tokens: 100,
            total_cost: Some(0.01),
        },
        request: vec![1, 2, 3, 4],
        response: Some(vec![5, 6, 7, 8]),
        stream_events: vec![vec![9, 10], vec![11, 12]],
    };

    assert_eq!(data.request.len(), 4);
    assert!(data.response.is_some());
    assert_eq!(data.stream_events.len(), 2);
}

#[test]
fn test_retention_policy() {
    let policy = RetentionPolicy {
        max_age_seconds: 86400 * 30, // 30 days
        max_sessions: Some(10000),
        max_total_size_bytes: Some(1024 * 1024 * 1024), // 1GB
    };

    assert_eq!(policy.max_age_seconds, 2592000);
    assert_eq!(policy.max_sessions.unwrap(), 10000);
}

#[test]
fn test_storage_error_variants() {
    let io_error = StorageError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file not found",
    ));
    assert!(io_error.to_string().contains("IO error"));

    let not_found = StorageError::NotFound("session_123".to_string());
    assert!(not_found.to_string().contains("Not found"));

    let invalid = StorageError::InvalidData("bad format".to_string());
    assert!(invalid.to_string().contains("Invalid data"));

    let serialization = StorageError::Serialization("json error".to_string());
    assert!(serialization.to_string().contains("Serialization"));

    let config = StorageError::Config("invalid config".to_string());
    assert!(config.to_string().contains("Configuration"));
}

#[test]
fn test_session_metadata_optional_fields() {
    let metadata = SessionMetadata {
        id: "session_123".to_string(),
        created_at: 1234567890,
        model: "claude-3".to_string(),
        provider: "anthropic".to_string(),
        user_id: None,
        tags: vec![],
        total_tokens: 0,
        total_cost: None,
    };

    assert!(metadata.user_id.is_none());
    assert!(metadata.total_cost.is_none());
    assert_eq!(metadata.tags.len(), 0);
}

#[test]
fn test_session_data_without_response() {
    let data = SessionData {
        metadata: SessionMetadata {
            id: "session_123".to_string(),
            created_at: 1234567890,
            model: "gpt-4".to_string(),
            provider: "openai".to_string(),
            user_id: None,
            tags: vec![],
            total_tokens: 0,
            total_cost: None,
        },
        request: vec![1, 2, 3],
        response: None,
        stream_events: vec![],
    };

    assert!(data.response.is_none());
    assert_eq!(data.stream_events.len(), 0);
}
