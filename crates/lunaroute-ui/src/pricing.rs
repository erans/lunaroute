//! Model pricing fetcher with disk-based caching from LiteLLM pricing database

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

const PRICING_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/refs/heads/main/model_prices_and_context_window.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours
const CACHE_FILENAME: &str = "litellm_pricing_cache.json";

/// Global pricing fetcher instance
pub static PRICING_FETCHER: Lazy<PricingFetcher> = Lazy::new(PricingFetcher::default);

/// Model pricing information from LiteLLM
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelPricing {
    #[serde(default)]
    pub input_cost_per_token: f64,
    #[serde(default)]
    pub output_cost_per_token: f64,
    #[serde(default)]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    pub max_input_tokens: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub litellm_provider: Option<String>,
}

impl ModelPricing {
    /// Convert per-token pricing to per-million-token pricing
    pub fn input_cost_per_million(&self) -> f64 {
        self.input_cost_per_token * 1_000_000.0
    }

    /// Convert per-token pricing to per-million-token pricing
    pub fn output_cost_per_million(&self) -> f64 {
        self.output_cost_per_token * 1_000_000.0
    }
}

/// Cached pricing data for a single model (in memory)
#[derive(Debug, Clone)]
struct CachedModelPricing {
    pricing: ModelPricing,
    fetched_at: SystemTime,
}

impl CachedModelPricing {
    fn is_expired(&self) -> bool {
        self.fetched_at
            .elapsed()
            .map(|elapsed| elapsed > CACHE_TTL)
            .unwrap_or(true)
    }
}

/// Metadata about the cached file on disk
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskCacheMetadata {
    downloaded_at: SystemTime,
}

impl DiskCacheMetadata {
    fn is_expired(&self) -> bool {
        self.downloaded_at
            .elapsed()
            .map(|elapsed| elapsed > CACHE_TTL)
            .unwrap_or(true)
    }
}

/// Pricing fetcher with disk-based full file cache and in-memory per-model cache
#[derive(Clone)]
pub struct PricingFetcher {
    // In-memory cache for individual models only
    model_cache: Arc<RwLock<HashMap<String, CachedModelPricing>>>,
    // Path to disk cache directory
    cache_dir: PathBuf,
    client: reqwest::Client,
}

impl PricingFetcher {
    /// Create a new pricing fetcher with custom cache directory
    pub fn new(cache_dir: PathBuf) -> Self {
        // Ensure cache directory exists
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            eprintln!("Warning: Failed to create pricing cache directory: {}", e);
        }

        Self {
            model_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_dir,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Create with default cache directory (~/.lunaroute/pricing_cache)
    pub fn with_default_cache_dir() -> Self {
        let cache_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".lunaroute")
            .join("pricing_cache");
        Self::new(cache_dir)
    }

    fn cache_file_path(&self) -> PathBuf {
        self.cache_dir.join(CACHE_FILENAME)
    }

    fn metadata_file_path(&self) -> PathBuf {
        self.cache_dir.join("metadata.json")
    }

    /// Check if disk cache is valid and fresh
    fn is_disk_cache_valid(&self) -> bool {
        let cache_file = self.cache_file_path();
        let metadata_file = self.metadata_file_path();

        if !cache_file.exists() || !metadata_file.exists() {
            return false;
        }

        // Check metadata
        if let Ok(metadata_str) = fs::read_to_string(&metadata_file) {
            if let Ok(metadata) = serde_json::from_str::<DiskCacheMetadata>(&metadata_str) {
                return !metadata.is_expired();
            }
        }

        false
    }

    /// Download pricing file to disk
    async fn download_pricing_to_disk(&self) -> Result<(), PricingError> {
        let response = self
            .client
            .get(PRICING_URL)
            .send()
            .await
            .map_err(|e| PricingError::FetchError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(PricingError::FetchError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        let content = response
            .text()
            .await
            .map_err(|e| PricingError::FetchError(e.to_string()))?;

        // Write to disk
        let cache_file = self.cache_file_path();
        fs::write(&cache_file, content)
            .map_err(|e| PricingError::CacheWriteError(e.to_string()))?;

        // Write metadata
        let metadata = DiskCacheMetadata {
            downloaded_at: SystemTime::now(),
        };
        let metadata_str = serde_json::to_string(&metadata)
            .map_err(|e| PricingError::CacheWriteError(e.to_string()))?;
        fs::write(self.metadata_file_path(), metadata_str)
            .map_err(|e| PricingError::CacheWriteError(e.to_string()))?;

        Ok(())
    }

    /// Ensure pricing file is available and fresh on disk
    async fn ensure_disk_cache(&self) -> Result<(), PricingError> {
        if !self.is_disk_cache_valid() {
            self.download_pricing_to_disk().await?;
        }
        Ok(())
    }

    /// Read pricing for a specific model from disk cache
    fn read_model_from_disk(&self, model_name: &str) -> Result<ModelPricing, PricingError> {
        let cache_file = self.cache_file_path();
        let content = fs::read_to_string(&cache_file)
            .map_err(|e| PricingError::CacheReadError(e.to_string()))?;

        let all_pricing: HashMap<String, ModelPricing> =
            serde_json::from_str(&content).map_err(|e| PricingError::ParseError(e.to_string()))?;

        all_pricing
            .get(model_name)
            .cloned()
            .ok_or_else(|| PricingError::ModelNotFound(model_name.to_string()))
    }

    /// Get pricing for a specific model
    /// 1. Check in-memory cache first
    /// 2. If not in memory or expired, ensure disk cache is fresh
    /// 3. Read from disk and cache in memory
    pub async fn get_model_pricing(&self, model_name: &str) -> Option<ModelPricing> {
        // Check in-memory cache first
        {
            let cache = self.model_cache.read().await;
            if let Some(cached) = cache.get(model_name) {
                if !cached.is_expired() {
                    return Some(cached.pricing.clone());
                }
            }
        }

        // Ensure disk cache is fresh
        if self.ensure_disk_cache().await.is_err() {
            // On fetch error, try to read from stale disk cache if available
            if let Ok(pricing) = self.read_model_from_disk(model_name) {
                return Some(pricing);
            }
            return None;
        }

        // Read from disk
        match self.read_model_from_disk(model_name) {
            Ok(pricing) => {
                // Update in-memory cache
                let mut cache = self.model_cache.write().await;
                cache.insert(
                    model_name.to_string(),
                    CachedModelPricing {
                        pricing: pricing.clone(),
                        fetched_at: SystemTime::now(),
                    },
                );
                Some(pricing)
            }
            Err(_) => None,
        }
    }

    /// Clear expired entries from in-memory cache
    pub async fn cleanup_expired_cache(&self) {
        let mut cache = self.model_cache.write().await;
        cache.retain(|_, cached| !cached.is_expired());
    }

    /// Force refresh disk cache
    pub async fn force_refresh(&self) -> Result<(), PricingError> {
        self.download_pricing_to_disk().await
    }

    /// Get cache statistics (for debugging)
    pub async fn cache_stats(&self) -> (usize, usize, bool) {
        let cache = self.model_cache.read().await;
        let total = cache.len();
        let expired = cache.values().filter(|c| c.is_expired()).count();
        let disk_valid = self.is_disk_cache_valid();
        (total, expired, disk_valid)
    }

    /// Batch fetch pricing for multiple models at once (optimized for bulk queries)
    pub async fn get_batch_pricing(&self, model_names: &[String]) -> HashMap<String, ModelPricing> {
        let mut result = HashMap::new();

        // First, ensure disk cache is fresh (one-time check for all models)
        if self.ensure_disk_cache().await.is_err() {
            // If we can't fetch, try to use what we have in memory
            let cache = self.model_cache.read().await;
            for model_name in model_names {
                if let Some(cached) = cache.get(model_name) {
                    result.insert(model_name.clone(), cached.pricing.clone());
                }
            }
            return result;
        }

        // Now fetch all requested models from disk in one read
        for model_name in model_names {
            // Check in-memory cache first
            {
                let cache = self.model_cache.read().await;
                if let Some(cached) = cache.get(model_name) {
                    if !cached.is_expired() {
                        result.insert(model_name.clone(), cached.pricing.clone());
                        continue;
                    }
                }
            }

            // Not in memory, read from disk
            if let Ok(pricing) = self.read_model_from_disk(model_name) {
                // Update in-memory cache
                let mut cache = self.model_cache.write().await;
                cache.insert(
                    model_name.clone(),
                    CachedModelPricing {
                        pricing: pricing.clone(),
                        fetched_at: SystemTime::now(),
                    },
                );
                result.insert(model_name.clone(), pricing);
            }
        }

        result
    }
}

impl Default for PricingFetcher {
    fn default() -> Self {
        Self::with_default_cache_dir()
    }
}

/// Pricing fetch errors
#[derive(Debug, Clone)]
pub enum PricingError {
    FetchError(String),
    ParseError(String),
    ModelNotFound(String),
    CacheWriteError(String),
    CacheReadError(String),
}

impl std::fmt::Display for PricingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PricingError::FetchError(msg) => write!(f, "Failed to fetch pricing: {}", msg),
            PricingError::ParseError(msg) => write!(f, "Failed to parse pricing: {}", msg),
            PricingError::ModelNotFound(model) => {
                write!(f, "Model '{}' not found in pricing database", model)
            }
            PricingError::CacheWriteError(msg) => write!(f, "Failed to write cache: {}", msg),
            PricingError::CacheReadError(msg) => write!(f, "Failed to read cache: {}", msg),
        }
    }
}

impl std::error::Error for PricingError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_model_pricing_conversion() {
        let pricing = ModelPricing {
            input_cost_per_token: 1e-06,  // $0.000001 per token
            output_cost_per_token: 5e-06, // $0.000005 per token
            cache_creation_input_token_cost: None,
            cache_read_input_token_cost: None,
            max_input_tokens: None,
            max_output_tokens: None,
            litellm_provider: None,
        };

        assert!((pricing.input_cost_per_million() - 1.0).abs() < 0.001);
        assert!((pricing.output_cost_per_million() - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_expiry() {
        let metadata = DiskCacheMetadata {
            downloaded_at: SystemTime::now() - Duration::from_secs(25 * 60 * 60), // 25 hours ago
        };
        assert!(metadata.is_expired());
    }

    #[test]
    fn test_cache_not_expired() {
        let metadata = DiskCacheMetadata {
            downloaded_at: SystemTime::now() - Duration::from_secs(1 * 60 * 60), // 1 hour ago
        };
        assert!(!metadata.is_expired());
    }

    #[tokio::test]
    async fn test_pricing_fetcher_creation() {
        let temp_dir = TempDir::new().unwrap();
        let fetcher = PricingFetcher::new(temp_dir.path().to_path_buf());
        let (total, expired, disk_valid) = fetcher.cache_stats().await;
        assert_eq!(total, 0);
        assert_eq!(expired, 0);
        assert!(!disk_valid); // No cache file yet
    }

    #[tokio::test]
    async fn test_cache_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let fetcher = PricingFetcher::new(temp_dir.path().to_path_buf());

        // Manually add some expired cache entries
        {
            let mut cache = fetcher.model_cache.write().await;
            cache.insert(
                "test-model".to_string(),
                CachedModelPricing {
                    pricing: ModelPricing {
                        input_cost_per_token: 1e-06,
                        output_cost_per_token: 5e-06,
                        cache_creation_input_token_cost: None,
                        cache_read_input_token_cost: None,
                        max_input_tokens: None,
                        max_output_tokens: None,
                        litellm_provider: None,
                    },
                    fetched_at: SystemTime::now() - Duration::from_secs(25 * 60 * 60),
                },
            );
        }

        let (total, expired, _) = fetcher.cache_stats().await;
        assert_eq!(total, 1);
        assert_eq!(expired, 1);

        fetcher.cleanup_expired_cache().await;

        let (total, expired, _) = fetcher.cache_stats().await;
        assert_eq!(total, 0);
        assert_eq!(expired, 0);
    }

    #[test]
    fn test_disk_cache_read_write() {
        let temp_dir = TempDir::new().unwrap();
        let cache_file = temp_dir.path().join(CACHE_FILENAME);

        // Create sample pricing data
        let mut pricing_data = HashMap::new();
        pricing_data.insert(
            "test-model".to_string(),
            ModelPricing {
                input_cost_per_token: 1e-06,
                output_cost_per_token: 5e-06,
                cache_creation_input_token_cost: None,
                cache_read_input_token_cost: None,
                max_input_tokens: Some(100000),
                max_output_tokens: Some(4096),
                litellm_provider: Some("test-provider".to_string()),
            },
        );

        // Write to disk
        let json = serde_json::to_string(&pricing_data).unwrap();
        fs::write(&cache_file, json).unwrap();

        // Read back
        let content = fs::read_to_string(&cache_file).unwrap();
        let loaded: HashMap<String, ModelPricing> = serde_json::from_str(&content).unwrap();

        assert!(loaded.contains_key("test-model"));
        let pricing = &loaded["test-model"];
        assert!((pricing.input_cost_per_token - 1e-06).abs() < 1e-10);
        assert_eq!(pricing.max_input_tokens, Some(100000));
    }
}
