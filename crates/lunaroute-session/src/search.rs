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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum SortOrder {
    /// Newest first (default)
    #[default]
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
            (total_count as usize).div_ceil(page_size).max(1)
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
    /// Maximum length for text search queries to prevent memory exhaustion
    const MAX_TEXT_SEARCH_LEN: usize = 1000;

    /// Maximum number of items in filter arrays to prevent DOS
    const MAX_FILTER_ARRAY_LEN: usize = 100;

    /// Maximum length for individual string elements (IPs, IDs, etc)
    const MAX_STRING_LEN: usize = 256;

    /// Base maximum page size for simple queries
    const MAX_PAGE_SIZE_BASE: usize = 1000;

    /// Reduced page size for moderate complexity queries
    const MAX_PAGE_SIZE_MODERATE: usize = 500;

    /// Minimum page size for high complexity queries
    const MAX_PAGE_SIZE_COMPLEX: usize = 100;

    /// Create a new filter builder
    pub fn builder() -> SessionFilterBuilder {
        SessionFilterBuilder::default()
    }

    /// Calculate query complexity score based on active filters
    ///
    /// Higher scores indicate more complex queries that should have smaller page sizes
    fn query_complexity(&self) -> u32 {
        let mut score = 0;

        // Text search is expensive (LIKE queries)
        if self.text_search.is_some() {
            score += 3;
        }

        // Time range queries are relatively cheap
        if self.time_range.is_some() {
            score += 1;
        }

        // Array filters (IN clauses) add complexity based on size
        score += (self.providers.len() / 10) as u32; // +1 per 10 items
        score += (self.models.len() / 10) as u32;
        score += (self.request_ids.len() / 10) as u32;
        score += (self.session_ids.len() / 10) as u32;
        score += (self.client_ips.len() / 10) as u32;
        score += (self.finish_reasons.len() / 10) as u32;

        // Boolean filters are cheap
        if self.success.is_some() {
            score += 1;
        }
        if self.is_streaming.is_some() {
            score += 1;
        }

        // Range filters are cheap
        if self.min_tokens.is_some() || self.max_tokens.is_some() {
            score += 1;
        }
        if self.min_duration_ms.is_some() || self.max_duration_ms.is_some() {
            score += 1;
        }

        score
    }

    /// Get the maximum allowed page size for this query based on complexity
    fn max_page_size_for_query(&self) -> usize {
        let complexity = self.query_complexity();

        if complexity <= 1 {
            // Simple query: single filter or no filters
            Self::MAX_PAGE_SIZE_BASE
        } else if complexity <= 5 {
            // Moderate complexity: text search or multiple filters
            Self::MAX_PAGE_SIZE_MODERATE
        } else {
            // High complexity: text search + multiple large arrays
            Self::MAX_PAGE_SIZE_COMPLEX
        }
    }

    /// Validate the filter parameters
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref time_range) = self.time_range {
            if time_range.start > time_range.end {
                return Err("start time must be before end time".to_string());
            }

            // Validate timestamps are reasonable (within 10 years past/future)
            let now = Utc::now();
            let ten_years_ago = now - chrono::Duration::days(3650);
            let ten_years_future = now + chrono::Duration::days(3650);

            if time_range.start < ten_years_ago {
                return Err("start time is too far in the past (> 10 years)".to_string());
            }

            if time_range.end > ten_years_future {
                return Err("end time is too far in the future (> 10 years)".to_string());
            }

            // Note: DateTime<Utc> enforces UTC timezone at the type level,
            // so no additional runtime validation is needed for timezone correctness
        }

        if let (Some(min), Some(max)) = (self.min_tokens, self.max_tokens)
            && min > max
        {
            return Err("min_tokens must be less than or equal to max_tokens".to_string());
        }

        if let (Some(min), Some(max)) = (self.min_duration_ms, self.max_duration_ms)
            && min > max
        {
            return Err("min_duration_ms must be less than or equal to max_duration_ms".to_string());
        }

        if self.page_size == 0 {
            return Err("page_size must be greater than 0".to_string());
        }

        // Apply progressive page size limits based on query complexity
        let max_page_size = self.max_page_size_for_query();
        if self.page_size > max_page_size {
            return Err(format!(
                "page_size {} exceeds maximum {} for this query complexity (reduce filters or page size)",
                self.page_size, max_page_size
            ));
        }

        // Validate text search length to prevent memory exhaustion
        if let Some(ref text_search) = self.text_search
            && text_search.len() > Self::MAX_TEXT_SEARCH_LEN
        {
            return Err(format!(
                "text_search exceeds maximum length of {}",
                Self::MAX_TEXT_SEARCH_LEN
            ));
        }

        // Validate array lengths to prevent DOS via large IN clauses
        if self.providers.len() > Self::MAX_FILTER_ARRAY_LEN {
            return Err(format!(
                "providers array exceeds maximum length of {}",
                Self::MAX_FILTER_ARRAY_LEN
            ));
        }

        if self.models.len() > Self::MAX_FILTER_ARRAY_LEN {
            return Err(format!(
                "models array exceeds maximum length of {}",
                Self::MAX_FILTER_ARRAY_LEN
            ));
        }

        if self.request_ids.len() > Self::MAX_FILTER_ARRAY_LEN {
            return Err(format!(
                "request_ids array exceeds maximum length of {}",
                Self::MAX_FILTER_ARRAY_LEN
            ));
        }

        if self.session_ids.len() > Self::MAX_FILTER_ARRAY_LEN {
            return Err(format!(
                "session_ids array exceeds maximum length of {}",
                Self::MAX_FILTER_ARRAY_LEN
            ));
        }

        if self.client_ips.len() > Self::MAX_FILTER_ARRAY_LEN {
            return Err(format!(
                "client_ips array exceeds maximum length of {}",
                Self::MAX_FILTER_ARRAY_LEN
            ));
        }

        if self.finish_reasons.len() > Self::MAX_FILTER_ARRAY_LEN {
            return Err(format!(
                "finish_reasons array exceeds maximum length of {}",
                Self::MAX_FILTER_ARRAY_LEN
            ));
        }

        // Validate individual string lengths in arrays
        for provider in &self.providers {
            if provider.len() > Self::MAX_STRING_LEN {
                return Err(format!(
                    "provider name exceeds maximum length of {}",
                    Self::MAX_STRING_LEN
                ));
            }
        }

        for model in &self.models {
            if model.len() > Self::MAX_STRING_LEN {
                return Err(format!(
                    "model name exceeds maximum length of {}",
                    Self::MAX_STRING_LEN
                ));
            }
        }

        for id in &self.request_ids {
            if id.len() > Self::MAX_STRING_LEN {
                return Err(format!(
                    "request_id exceeds maximum length of {}",
                    Self::MAX_STRING_LEN
                ));
            }
        }

        for id in &self.session_ids {
            if id.len() > Self::MAX_STRING_LEN {
                return Err(format!(
                    "session_id exceeds maximum length of {}",
                    Self::MAX_STRING_LEN
                ));
            }
        }

        for ip in &self.client_ips {
            if ip.len() > Self::MAX_STRING_LEN {
                return Err(format!(
                    "client_ip exceeds maximum length of {}",
                    Self::MAX_STRING_LEN
                ));
            }
        }

        for reason in &self.finish_reasons {
            if reason.len() > Self::MAX_STRING_LEN {
                return Err(format!(
                    "finish_reason exceeds maximum length of {}",
                    Self::MAX_STRING_LEN
                ));
            }
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
    fn test_session_filter_validation_time_range_boundaries() {
        let now = Utc::now();

        // Valid: recent time range
        let hour_ago = now - chrono::Duration::hours(1);
        let result = SessionFilter::builder()
            .time_range(hour_ago, now)
            .build();
        assert!(result.is_ok());

        // Invalid: start time too far in past (> 10 years)
        let eleven_years_ago = now - chrono::Duration::days(11 * 365);
        let result = SessionFilter::builder()
            .time_range(eleven_years_ago, now)
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too far in the past"));

        // Invalid: end time too far in future (> 10 years)
        let eleven_years_future = now + chrono::Duration::days(11 * 365);
        let result = SessionFilter::builder()
            .time_range(now, eleven_years_future)
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too far in the future"));

        // Valid: exactly at boundaries (9.5 years)
        let nine_years_ago = now - chrono::Duration::days(9 * 365 + 180);
        let nine_years_future = now + chrono::Duration::days(9 * 365 + 180);
        let result = SessionFilter::builder()
            .time_range(nine_years_ago, nine_years_future)
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_session_filter_timezone_validation() {
        // DateTime<Utc> enforces UTC timezone at type level
        // This test verifies that UTC times work correctly
        let now = Utc::now();
        let hour_ago = now - chrono::Duration::hours(1);

        let result = SessionFilter::builder()
            .time_range(hour_ago, now)
            .build();

        assert!(result.is_ok());

        // DateTime<Utc> type guarantees UTC timezone by construction
        let filter = result.unwrap();
        assert!(filter.time_range.is_some());
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
        assert!(result.unwrap_err().contains("page_size must be greater than 0"));
    }

    #[test]
    fn test_progressive_page_size_limits_simple_query() {
        // Simple query: no filters - allows max 1000
        let result = SessionFilter::builder()
            .page_size(1000)
            .build();
        assert!(result.is_ok());

        let result = SessionFilter::builder()
            .page_size(1001)
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum"));

        // Simple query: time range only - allows max 1000
        let now = Utc::now();
        let hour_ago = now - chrono::Duration::hours(1);
        let result = SessionFilter::builder()
            .time_range(hour_ago, now)
            .page_size(1000)
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_progressive_page_size_limits_moderate_query() {
        // Moderate query: text search reduces limit to 500
        let result = SessionFilter::builder()
            .text_search("test".to_string())
            .page_size(500)
            .build();
        assert!(result.is_ok());

        let result = SessionFilter::builder()
            .text_search("test".to_string())
            .page_size(501)
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum 500"));

        // Moderate query: multiple filters
        let result = SessionFilter::builder()
            .providers(vec!["openai".to_string()])
            .models(vec!["gpt-4".to_string()])
            .success(true)
            .min_tokens(100)
            .page_size(500)
            .build();
        assert!(result.is_ok());

        let result = SessionFilter::builder()
            .providers(vec!["openai".to_string()])
            .models(vec!["gpt-4".to_string()])
            .success(true)
            .min_tokens(100)
            .page_size(501)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_progressive_page_size_limits_complex_query() {
        // Complex query: text search + large arrays reduces limit to 100
        let result = SessionFilter::builder()
            .text_search("test".to_string())
            .providers(vec!["provider".to_string(); 50]) // Large array
            .models(vec!["model".to_string(); 50]) // Large array
            .page_size(100)
            .build();
        assert!(result.is_ok());

        let result = SessionFilter::builder()
            .text_search("test".to_string())
            .providers(vec!["provider".to_string(); 50])
            .models(vec!["model".to_string(); 50])
            .page_size(101)
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum 100"));
    }

    #[test]
    fn test_query_complexity_calculation() {
        // Test that we can access complexity indirectly through validation
        let filter = SessionFilter::default();
        assert_eq!(filter.query_complexity(), 0); // No filters

        let now = Utc::now();
        let hour_ago = now - chrono::Duration::hours(1);
        let filter = SessionFilter::builder()
            .time_range(hour_ago, now)
            .build()
            .unwrap();
        assert_eq!(filter.query_complexity(), 1); // Time range only

        let filter = SessionFilter::builder()
            .text_search("test".to_string())
            .build()
            .unwrap();
        assert_eq!(filter.query_complexity(), 3); // Text search is expensive

        let filter = SessionFilter::builder()
            .text_search("test".to_string())
            .providers(vec!["p".to_string(); 50])
            .models(vec!["m".to_string(); 50])
            .build()
            .unwrap();
        // 3 (text) + 5 (providers/10) + 5 (models/10) = 13
        assert!(filter.query_complexity() >= 10); // High complexity
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

    #[test]
    fn test_session_filter_validation_text_search_length() {
        // Valid text search
        let result = SessionFilter::builder()
            .text_search("a".repeat(1000))
            .build();
        assert!(result.is_ok());

        // Text search exceeds max length
        let result = SessionFilter::builder()
            .text_search("a".repeat(1001))
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("text_search exceeds maximum length"));
    }

    #[test]
    fn test_session_filter_validation_array_lengths() {
        // Valid array length
        let result = SessionFilter::builder()
            .providers(vec!["provider".to_string(); 100])
            .build();
        assert!(result.is_ok());

        // Providers array exceeds max length
        let result = SessionFilter::builder()
            .providers(vec!["provider".to_string(); 101])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("providers array exceeds maximum length"));

        // Models array exceeds max length
        let result = SessionFilter::builder()
            .models(vec!["model".to_string(); 101])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("models array exceeds maximum length"));

        // Request IDs array exceeds max length
        let result = SessionFilter::builder()
            .request_ids(vec!["id".to_string(); 101])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("request_ids array exceeds maximum length"));

        // Session IDs array exceeds max length
        let result = SessionFilter::builder()
            .session_ids(vec!["id".to_string(); 101])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("session_ids array exceeds maximum length"));

        // Client IPs array exceeds max length
        let result = SessionFilter::builder()
            .client_ips(vec!["127.0.0.1".to_string(); 101])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("client_ips array exceeds maximum length"));

        // Finish reasons array exceeds max length
        let result = SessionFilter::builder()
            .finish_reasons(vec!["stop".to_string(); 101])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finish_reasons array exceeds maximum length"));
    }

    #[test]
    fn test_session_filter_validation_string_element_lengths() {
        // Valid string lengths
        let result = SessionFilter::builder()
            .providers(vec!["a".repeat(256)])
            .build();
        assert!(result.is_ok());

        // Provider name exceeds max length
        let result = SessionFilter::builder()
            .providers(vec!["a".repeat(257)])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("provider name exceeds maximum length"));

        // Model name exceeds max length
        let result = SessionFilter::builder()
            .models(vec!["a".repeat(257)])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("model name exceeds maximum length"));

        // Request ID exceeds max length
        let result = SessionFilter::builder()
            .request_ids(vec!["a".repeat(257)])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("request_id exceeds maximum length"));

        // Session ID exceeds max length
        let result = SessionFilter::builder()
            .session_ids(vec!["a".repeat(257)])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("session_id exceeds maximum length"));

        // Client IP exceeds max length
        let result = SessionFilter::builder()
            .client_ips(vec!["a".repeat(257)])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("client_ip exceeds maximum length"));

        // Finish reason exceeds max length
        let result = SessionFilter::builder()
            .finish_reasons(vec!["a".repeat(257)])
            .build();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finish_reason exceeds maximum length"));
    }
}
