//! Configuration for PostgreSQL session store connection pool

use std::time::Duration;

/// Configuration for PostgreSQL connection pool
///
/// These settings control the behavior of the underlying sqlx connection pool.
///
/// # Example
/// ```
/// use lunaroute_session_postgres::PostgresSessionStoreConfig;
/// use std::time::Duration;
///
/// let config = PostgresSessionStoreConfig::default()
///     .with_max_connections(50)
///     .with_min_connections(10);
/// ```
#[derive(Debug, Clone)]
pub struct PostgresSessionStoreConfig {
    /// Maximum number of connections in the pool
    pub max_connections: u32,

    /// Minimum number of connections to maintain
    pub min_connections: u32,

    /// Timeout for acquiring a connection from the pool
    pub acquire_timeout: Duration,

    /// How long a connection can remain idle before being closed
    pub idle_timeout: Duration,

    /// Maximum lifetime of a connection (to handle connection refresh)
    pub max_lifetime: Duration,
}

impl Default for PostgresSessionStoreConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            min_connections: 5,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(600),  // 10 minutes
            max_lifetime: Duration::from_secs(1800),  // 30 minutes
        }
    }
}

impl PostgresSessionStoreConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum number of connections
    pub fn with_max_connections(mut self, max_connections: u32) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// Set minimum number of connections
    pub fn with_min_connections(mut self, min_connections: u32) -> Self {
        self.min_connections = min_connections;
        self
    }

    /// Set acquire timeout
    pub fn with_acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Set maximum lifetime
    pub fn with_max_lifetime(mut self, lifetime: Duration) -> Self {
        self.max_lifetime = lifetime;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PostgresSessionStoreConfig::default();
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.acquire_timeout, Duration::from_secs(5));
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
        assert_eq!(config.max_lifetime, Duration::from_secs(1800));
    }

    #[test]
    fn test_builder_pattern() {
        let config = PostgresSessionStoreConfig::new()
            .with_max_connections(50)
            .with_min_connections(10)
            .with_acquire_timeout(Duration::from_secs(3));

        assert_eq!(config.max_connections, 50);
        assert_eq!(config.min_connections, 10);
        assert_eq!(config.acquire_timeout, Duration::from_secs(3));

        // Other values should remain at defaults
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
    }
}
