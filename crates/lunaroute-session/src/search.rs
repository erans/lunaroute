//! Session search and filtering capabilities

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Filter criteria for searching sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFilter {
    /// Filter by time range (inclusive)
    #[serde(default)]
    pub time_range: Option<TimeRange>,

    /// Filter by provider names
    #[serde(default)]
    pub providers: Vec<String>,

    /// Filter by model names (requested or used)
    #[serde(default)]
    pub models: Vec<String>,

    /// Filter by request IDs
    #[serde(default)]
    pub request_ids: Vec<String>,

    /// Filter by session IDs
    #[serde(default)]
    pub session_ids: Vec<String>,

    /// Filter by success status
    #[serde(default)]
    pub success: Option<bool>,

    /// Filter by streaming status
    #[serde(default)]
    pub is_streaming: Option<bool>,

    /// Minimum total tokens
    #[serde(default)]
    pub min_tokens: Option<u32>,

    /// Maximum total tokens
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Minimum duration in milliseconds
    #[serde(default)]
    pub min_duration_ms: Option<u64>,

    /// Maximum duration in milliseconds
    #[serde(default)]
    pub max_duration_ms: Option<u64>,

    /// Filter by client IP addresses
    #[serde(default)]
    pub client_ips: Vec<String>,

    /// Filter by finish reasons
    #[serde(default)]
    pub finish_reasons: Vec<String>,

    /// Full-text search in request/response text
    #[serde(default)]
    pub text_search: Option<String>,

    /// Pagination: number of results per page
    #[serde(default = "default_page_size")]
    pub page_size: usize,

    /// Pagination: page offset (0-indexed)
    #[serde(default)]
    pub page: usize,

    /// Sort order
    #[serde(default)]
    pub sort: SortOrder,
}

fn default_page_size() -> usize {
    50
}

impl Default for SessionFilter {
    fn default() -> Self {
        Self {
            time_range: None,
            providers: Vec::new(),
            models: Vec::new(),
            request_ids: Vec::new(),
            session_ids: Vec::new(),
            success: None,
            is_streaming: None,
            min_tokens: None,
            max_tokens: None,
            min_duration_ms: None,
            max_duration_ms: None,
            client_ips: Vec::new(),
            finish_reasons: Vec::new(),
            text_search: None,
            page_size: default_page_size(),
            page: 0,
            sort: SortOrder::default(),
        }
    }
}

/// Time range filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    /// Start time (inclusive)
    pub start: DateTime<Utc>,

    /// End time (inclusive)
    pub end: DateTime<Utc>,
}

/// Sort order for search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SortOrder {
    /// Newest first (default)
    NewestFirst,

    /// Oldest first
    OldestFirst,

    /// Highest token count first
    HighestTokens,

    /// Longest duration first
    LongestDuration,

    /// Shortest duration first
    ShortestDuration,
}

impl Default for SortOrder {
    fn default() -> Self {
        SortOrder::NewestFirst
    }
}

/// Search results with pagination metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults<T> {
    /// Matching items
    pub items: Vec<T>,

    /// Total count of matching items (for pagination)
    pub total_count: u64,

    /// Current page (0-indexed)
    pub page: usize,

    /// Page size
    pub page_size: usize,

    /// Total pages
    pub total_pages: usize,
}

impl<T> SearchResults<T> {
    pub fn new(items: Vec<T>, total_count: u64, page: usize, page_size: usize) -> Self {
        let total_pages = if page_size > 0 {
            ((total_count as usize + page_size - 1) / page_size).max(1)
        } else {
            1
        };

        Self {
            items,
            total_count,
            page,
            page_size,
            total_pages,
        }
    }

    pub fn has_next_page(&self) -> bool {
        self.page + 1 < self.total_pages
    }

    pub fn has_prev_page(&self) -> bool {
        self.page > 0
    }
}

/// Simplified session record for search results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlite-writer", derive(sqlx::FromRow))]
pub struct SessionRecord {
    pub session_id: String,
    pub request_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub provider: String,
    pub model_requested: String,
    pub model_used: Option<String>,
    pub success: Option<bool>,
    pub error_message: Option<String>,
    pub finish_reason: Option<String>,
    pub total_duration_ms: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub is_streaming: bool,
    pub client_ip: Option<String>,
}

/// Session statistics aggregation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAggregates {
    pub total_sessions: u64,
    pub successful_sessions: u64,
    pub failed_sessions: u64,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub avg_duration_ms: f64,
    pub p50_duration_ms: Option<u64>,
    pub p95_duration_ms: Option<u64>,
    pub p99_duration_ms: Option<u64>,
    pub sessions_by_provider: HashMap<String, u64>,
    pub sessions_by_model: HashMap<String, u64>,
}

impl SessionFilter {
    /// Create a new filter builder
    pub fn builder() -> SessionFilterBuilder {
        SessionFilterBuilder::default()
    }

    /// Validate the filter parameters
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref time_range) = self.time_range {
            if time_range.start > time_range.end {
                return Err("start time must be before end time".to_string());
            }
        }

        if let (Some(min), Some(max)) = (self.min_tokens, self.max_tokens) {
            if min > max {
                return Err("min_tokens must be less than or equal to max_tokens".to_string());
            }
        }

        if let (Some(min), Some(max)) = (self.min_duration_ms, self.max_duration_ms) {
            if min > max {
                return Err("min_duration_ms must be less than or equal to max_duration_ms".to_string());
            }
        }

        if self.page_size == 0 {
            return Err("page_size must be greater than 0".to_string());
        }

        if self.page_size > 1000 {
            return Err("page_size cannot exceed 1000".to_string());
        }

        Ok(())
    }
}

/// Builder for SessionFilter
#[derive(Debug, Default)]
pub struct SessionFilterBuilder {
    filter: SessionFilter,
}

impl SessionFilterBuilder {
    pub fn time_range(mut self, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        self.filter.time_range = Some(TimeRange { start, end });
        self
    }

    pub fn providers(mut self, providers: Vec<String>) -> Self {
        self.filter.providers = providers;
        self
    }

    pub fn models(mut self, models: Vec<String>) -> Self {
        self.filter.models = models;
        self
    }

    pub fn request_ids(mut self, request_ids: Vec<String>) -> Self {
        self.filter.request_ids = request_ids;
        self
    }

    pub fn session_ids(mut self, session_ids: Vec<String>) -> Self {
        self.filter.session_ids = session_ids;
        self
    }

    pub fn success(mut self, success: bool) -> Self {
        self.filter.success = Some(success);
        self
    }

    pub fn streaming(mut self, is_streaming: bool) -> Self {
        self.filter.is_streaming = Some(is_streaming);
        self
    }

    pub fn min_tokens(mut self, min: u32) -> Self {
        self.filter.min_tokens = Some(min);
        self
    }

    pub fn max_tokens(mut self, max: u32) -> Self {
        self.filter.max_tokens = Some(max);
        self
    }

    pub fn min_duration_ms(mut self, min: u64) -> Self {
        self.filter.min_duration_ms = Some(min);
        self
    }

    pub fn max_duration_ms(mut self, max: u64) -> Self {
        self.filter.max_duration_ms = Some(max);
        self
    }

    pub fn client_ips(mut self, ips: Vec<String>) -> Self {
        self.filter.client_ips = ips;
        self
    }

    pub fn finish_reasons(mut self, reasons: Vec<String>) -> Self {
        self.filter.finish_reasons = reasons;
        self
    }

    pub fn text_search(mut self, query: String) -> Self {
        self.filter.text_search = Some(query);
        self
    }

    pub fn page_size(mut self, size: usize) -> Self {
        self.filter.page_size = size;
        self
    }

    pub fn page(mut self, page: usize) -> Self {
        self.filter.page = page;
        self
    }

    pub fn sort(mut self, sort: SortOrder) -> Self {
        self.filter.sort = sort;
        self
    }

    pub fn build(self) -> Result<SessionFilter, String> {
        self.filter.validate()?;
        Ok(self.filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_filter_builder() {
        let now = Utc::now();
        let hour_ago = now - chrono::Duration::hours(1);

        let filter = SessionFilter::builder()
            .time_range(hour_ago, now)
            .providers(vec!["openai".to_string()])
            .models(vec!["gpt-4".to_string()])
            .success(true)
            .page_size(25)
            .build()
            .unwrap();

        assert!(filter.time_range.is_some());
        assert_eq!(filter.providers.len(), 1);
        assert_eq!(filter.page_size, 25);
    }

    #[test]
    fn test_session_filter_validation_time_range() {
        let now = Utc::now();
        let future = now + chrono::Duration::hours(1);

        let result = SessionFilter::builder()
            .time_range(future, now)
            .build();

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("start time must be before end time"));
    }

    #[test]
    fn test_session_filter_validation_tokens() {
        let result = SessionFilter::builder()
            .min_tokens(100)
            .max_tokens(50)
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_session_filter_validation_page_size() {
        let result = SessionFilter::builder()
            .page_size(0)
            .build();

        assert!(result.is_err());

        let result = SessionFilter::builder()
            .page_size(2000)
            .build();

        assert!(result.is_err());
    }

    #[test]
    fn test_search_results_pagination() {
        let results = SearchResults::new(
            vec![1, 2, 3, 4, 5],
            100,
            0,
            10,
        );

        assert_eq!(results.total_count, 100);
        assert_eq!(results.total_pages, 10);
        assert!(results.has_next_page());
        assert!(!results.has_prev_page());

        let results = SearchResults::new(
            vec![1, 2, 3],
            100,
            5,
            10,
        );

        assert!(results.has_next_page());
        assert!(results.has_prev_page());

        let results = SearchResults::new(
            vec![1, 2],
            100,
            9,
            10,
        );

        assert!(!results.has_next_page());
        assert!(results.has_prev_page());
    }
}
