//! Metrics for PostgreSQL session store
//!
//! Provides Prometheus metrics for monitoring the PostgreSQL session store:
//! - Event write operations (success/failure, latency)
//! - Session retrieval operations (success/failure, latency)
//! - Connection pool health (from sqlx)
//! - Database query performance
//! - Migration tracking

use prometheus::{CounterVec, GaugeVec, Histogram, HistogramOpts, HistogramVec, Opts, Registry};
use std::sync::Arc;

/// Metrics collector for PostgreSQL session store
#[derive(Clone)]
pub struct SessionStoreMetrics {
    /// Prometheus registry
    registry: Arc<Registry>,

    // Event write metrics
    /// Total events written
    pub events_written_total: CounterVec,
    /// Failed event writes
    pub events_write_errors_total: CounterVec,
    /// Event write duration
    pub event_write_duration_seconds: HistogramVec,

    // Session retrieval metrics
    /// Total session retrievals
    pub sessions_retrieved_total: CounterVec,
    /// Failed session retrievals
    pub sessions_retrieval_errors_total: CounterVec,
    /// Session retrieval duration
    pub session_retrieval_duration_seconds: Histogram,

    // Search and list metrics
    /// Total search operations
    pub search_operations_total: CounterVec,
    /// Search operation duration
    pub search_duration_seconds: Histogram,
    /// List operations total
    pub list_operations_total: CounterVec,
    /// List operation duration
    pub list_duration_seconds: Histogram,

    // Connection pool metrics (from sqlx)
    /// Current pool size
    pub pool_connections_total: GaugeVec,
    /// Idle connections in pool
    pub pool_connections_idle: GaugeVec,
    /// Active connections (in use)
    pub pool_connections_active: GaugeVec,
    /// Connection acquisition duration
    pub pool_acquire_duration_seconds: Histogram,

    // Migration metrics
    /// Applied migrations count
    pub migrations_applied_total: GaugeVec,
    /// Current schema version
    pub schema_version: GaugeVec,
    /// Migration duration
    pub migration_duration_seconds: HistogramVec,

    // TimescaleDB metrics
    /// TimescaleDB availability
    pub timescaledb_enabled: GaugeVec,
    /// Hypertable conversion success
    pub hypertable_conversions_total: CounterVec,
}

impl SessionStoreMetrics {
    /// Create a new metrics collector
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        // Event write metrics
        let events_written_total = CounterVec::new(
            Opts::new(
                "session_store_events_written_total",
                "Total number of session events written to PostgreSQL",
            ),
            &["tenant_id", "event_type"],
        )?;

        let events_write_errors_total = CounterVec::new(
            Opts::new(
                "session_store_events_write_errors_total",
                "Total number of event write errors",
            ),
            &["tenant_id", "event_type", "error_type"],
        )?;

        let event_write_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "session_store_event_write_duration_seconds",
                "Event write operation duration in seconds",
            )
            .buckets(vec![
                0.0001, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1,
            ]),
            &["event_type"],
        )?;

        // Session retrieval metrics
        let sessions_retrieved_total = CounterVec::new(
            Opts::new(
                "session_store_sessions_retrieved_total",
                "Total number of sessions retrieved from PostgreSQL",
            ),
            &["tenant_id", "status"],
        )?;

        let sessions_retrieval_errors_total = CounterVec::new(
            Opts::new(
                "session_store_sessions_retrieval_errors_total",
                "Total number of session retrieval errors",
            ),
            &["tenant_id", "error_type"],
        )?;

        let session_retrieval_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "session_store_session_retrieval_duration_seconds",
                "Session retrieval operation duration in seconds",
            )
            .buckets(vec![0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25]),
        )?;

        // Search and list metrics
        let search_operations_total = CounterVec::new(
            Opts::new(
                "session_store_search_operations_total",
                "Total number of search operations",
            ),
            &["tenant_id", "status"],
        )?;

        let search_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "session_store_search_duration_seconds",
                "Search operation duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
        )?;

        let list_operations_total = CounterVec::new(
            Opts::new(
                "session_store_list_operations_total",
                "Total number of list operations",
            ),
            &["tenant_id", "status"],
        )?;

        let list_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "session_store_list_duration_seconds",
                "List operation duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
        )?;

        // Connection pool metrics
        let pool_connections_total = GaugeVec::new(
            Opts::new(
                "session_store_pool_connections_total",
                "Total number of connections in the pool",
            ),
            &["database"],
        )?;

        let pool_connections_idle = GaugeVec::new(
            Opts::new(
                "session_store_pool_connections_idle",
                "Number of idle connections in the pool",
            ),
            &["database"],
        )?;

        let pool_connections_active = GaugeVec::new(
            Opts::new(
                "session_store_pool_connections_active",
                "Number of active connections (in use)",
            ),
            &["database"],
        )?;

        let pool_acquire_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "session_store_pool_acquire_duration_seconds",
                "Connection acquisition duration in seconds",
            )
            .buckets(vec![
                0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0,
            ]),
        )?;

        // Migration metrics
        let migrations_applied_total = GaugeVec::new(
            Opts::new(
                "session_store_migrations_applied_total",
                "Total number of applied migrations",
            ),
            &["database"],
        )?;

        let schema_version = GaugeVec::new(
            Opts::new("session_store_schema_version", "Current schema version"),
            &["database"],
        )?;

        let migration_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "session_store_migration_duration_seconds",
                "Migration execution duration in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]),
            &["migration_version"],
        )?;

        // TimescaleDB metrics
        let timescaledb_enabled = GaugeVec::new(
            Opts::new(
                "session_store_timescaledb_enabled",
                "Whether TimescaleDB extension is available (1=yes, 0=no)",
            ),
            &["database"],
        )?;

        let hypertable_conversions_total = CounterVec::new(
            Opts::new(
                "session_store_hypertable_conversions_total",
                "Total number of successful hypertable conversions",
            ),
            &["table_name"],
        )?;

        // Register all metrics
        registry.register(Box::new(events_written_total.clone()))?;
        registry.register(Box::new(events_write_errors_total.clone()))?;
        registry.register(Box::new(event_write_duration_seconds.clone()))?;
        registry.register(Box::new(sessions_retrieved_total.clone()))?;
        registry.register(Box::new(sessions_retrieval_errors_total.clone()))?;
        registry.register(Box::new(session_retrieval_duration_seconds.clone()))?;
        registry.register(Box::new(search_operations_total.clone()))?;
        registry.register(Box::new(search_duration_seconds.clone()))?;
        registry.register(Box::new(list_operations_total.clone()))?;
        registry.register(Box::new(list_duration_seconds.clone()))?;
        registry.register(Box::new(pool_connections_total.clone()))?;
        registry.register(Box::new(pool_connections_idle.clone()))?;
        registry.register(Box::new(pool_connections_active.clone()))?;
        registry.register(Box::new(pool_acquire_duration_seconds.clone()))?;
        registry.register(Box::new(migrations_applied_total.clone()))?;
        registry.register(Box::new(schema_version.clone()))?;
        registry.register(Box::new(migration_duration_seconds.clone()))?;
        registry.register(Box::new(timescaledb_enabled.clone()))?;
        registry.register(Box::new(hypertable_conversions_total.clone()))?;

        Ok(Self {
            registry: Arc::new(registry),
            events_written_total,
            events_write_errors_total,
            event_write_duration_seconds,
            sessions_retrieved_total,
            sessions_retrieval_errors_total,
            session_retrieval_duration_seconds,
            search_operations_total,
            search_duration_seconds,
            list_operations_total,
            list_duration_seconds,
            pool_connections_total,
            pool_connections_idle,
            pool_connections_active,
            pool_acquire_duration_seconds,
            migrations_applied_total,
            schema_version,
            migration_duration_seconds,
            timescaledb_enabled,
            hypertable_conversions_total,
        })
    }

    /// Get the Prometheus registry for exporting metrics
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Record a successful event write
    pub fn record_event_written(&self, tenant_id: &str, event_type: &str, duration_secs: f64) {
        self.events_written_total
            .with_label_values(&[tenant_id, event_type])
            .inc();
        self.event_write_duration_seconds
            .with_label_values(&[event_type])
            .observe(duration_secs);
    }

    /// Record an event write error
    pub fn record_event_write_error(&self, tenant_id: &str, event_type: &str, error_type: &str) {
        self.events_write_errors_total
            .with_label_values(&[tenant_id, event_type, error_type])
            .inc();
    }

    /// Record a successful session retrieval
    pub fn record_session_retrieved(&self, tenant_id: &str, duration_secs: f64, found: bool) {
        let status = if found { "found" } else { "not_found" };
        self.sessions_retrieved_total
            .with_label_values(&[tenant_id, status])
            .inc();
        self.session_retrieval_duration_seconds
            .observe(duration_secs);
    }

    /// Record a session retrieval error
    pub fn record_session_retrieval_error(&self, tenant_id: &str, error_type: &str) {
        self.sessions_retrieval_errors_total
            .with_label_values(&[tenant_id, error_type])
            .inc();
    }

    /// Record a search operation
    pub fn record_search(&self, tenant_id: &str, duration_secs: f64, success: bool) {
        let status = if success { "success" } else { "error" };
        self.search_operations_total
            .with_label_values(&[tenant_id, status])
            .inc();
        if success {
            self.search_duration_seconds.observe(duration_secs);
        }
    }

    /// Record a list operation
    pub fn record_list(&self, tenant_id: &str, duration_secs: f64, success: bool) {
        let status = if success { "success" } else { "error" };
        self.list_operations_total
            .with_label_values(&[tenant_id, status])
            .inc();
        if success {
            self.list_duration_seconds.observe(duration_secs);
        }
    }

    /// Update connection pool metrics
    pub fn update_pool_metrics(&self, database: &str, total: usize, idle: usize) {
        let active = total.saturating_sub(idle);
        self.pool_connections_total
            .with_label_values(&[database])
            .set(total as f64);
        self.pool_connections_idle
            .with_label_values(&[database])
            .set(idle as f64);
        self.pool_connections_active
            .with_label_values(&[database])
            .set(active as f64);
    }

    /// Record connection acquisition duration
    pub fn record_pool_acquire(&self, duration_secs: f64) {
        self.pool_acquire_duration_seconds.observe(duration_secs);
    }

    /// Update migration metrics
    pub fn update_migration_metrics(&self, database: &str, count: usize, version: Option<i32>) {
        self.migrations_applied_total
            .with_label_values(&[database])
            .set(count as f64);
        if let Some(v) = version {
            self.schema_version
                .with_label_values(&[database])
                .set(v as f64);
        }
    }

    /// Record a migration execution
    pub fn record_migration(&self, version: i32, duration_secs: f64) {
        self.migration_duration_seconds
            .with_label_values(&[&version.to_string()])
            .observe(duration_secs);
    }

    /// Set TimescaleDB availability
    pub fn set_timescaledb_enabled(&self, database: &str, enabled: bool) {
        self.timescaledb_enabled
            .with_label_values(&[database])
            .set(if enabled { 1.0 } else { 0.0 });
    }

    /// Record a hypertable conversion
    pub fn record_hypertable_conversion(&self, table_name: &str) {
        self.hypertable_conversions_total
            .with_label_values(&[table_name])
            .inc();
    }
}

impl Default for SessionStoreMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create session store metrics")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = SessionStoreMetrics::new().unwrap();
        assert!(!metrics.registry().gather().is_empty());
    }

    #[test]
    fn test_record_event_written() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.record_event_written("tenant-1", "Started", 0.001);
        metrics.record_event_written("tenant-1", "Started", 0.002);

        let gathered = metrics.registry().gather();
        let written_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_events_written_total")
            .expect("events_written_total metric not found");

        assert_eq!(
            written_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            2.0
        );
    }

    #[test]
    fn test_record_event_write_error() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.record_event_write_error("tenant-1", "Started", "database_error");

        let gathered = metrics.registry().gather();
        let error_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_events_write_errors_total")
            .expect("events_write_errors_total metric not found");

        assert_eq!(
            error_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            1.0
        );
    }

    #[test]
    fn test_record_session_retrieved() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.record_session_retrieved("tenant-1", 0.005, true);
        metrics.record_session_retrieved("tenant-1", 0.003, false);

        let gathered = metrics.registry().gather();
        let retrieved_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_sessions_retrieved_total")
            .expect("sessions_retrieved_total metric not found");

        // Should have 2 label sets (found and not_found)
        assert_eq!(retrieved_metric.metric.len(), 2);
    }

    #[test]
    fn test_update_pool_metrics() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.update_pool_metrics("postgres", 20, 15);

        let gathered = metrics.registry().gather();
        let total_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_pool_connections_total")
            .expect("pool_connections_total metric not found");

        assert_eq!(
            total_metric.metric[0]
                .gauge
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            20.0
        );

        let idle_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_pool_connections_idle")
            .expect("pool_connections_idle metric not found");

        assert_eq!(
            idle_metric.metric[0].gauge.as_ref().unwrap().value.unwrap(),
            15.0
        );

        let active_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_pool_connections_active")
            .expect("pool_connections_active metric not found");

        assert_eq!(
            active_metric.metric[0]
                .gauge
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            5.0
        );
    }

    #[test]
    fn test_update_migration_metrics() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.update_migration_metrics("postgres", 5, Some(5));

        let gathered = metrics.registry().gather();
        let count_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_migrations_applied_total")
            .expect("migrations_applied_total metric not found");

        assert_eq!(
            count_metric.metric[0]
                .gauge
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            5.0
        );

        let version_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_schema_version")
            .expect("schema_version metric not found");

        assert_eq!(
            version_metric.metric[0]
                .gauge
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            5.0
        );
    }

    #[test]
    fn test_set_timescaledb_enabled() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.set_timescaledb_enabled("postgres", true);

        let gathered = metrics.registry().gather();
        let ts_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_timescaledb_enabled")
            .expect("timescaledb_enabled metric not found");

        assert_eq!(
            ts_metric.metric[0].gauge.as_ref().unwrap().value.unwrap(),
            1.0
        );
    }

    #[test]
    fn test_record_hypertable_conversion() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.record_hypertable_conversion("sessions");
        metrics.record_hypertable_conversion("tool_stats");

        let gathered = metrics.registry().gather();
        let conversion_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_hypertable_conversions_total")
            .expect("hypertable_conversions_total metric not found");

        assert_eq!(conversion_metric.metric.len(), 2);
    }

    #[test]
    fn test_record_search() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.record_search("tenant-1", 0.05, true);
        metrics.record_search("tenant-1", 0.03, true);

        let gathered = metrics.registry().gather();
        let search_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_search_operations_total")
            .expect("search_operations_total metric not found");

        assert_eq!(
            search_metric.metric[0]
                .counter
                .as_ref()
                .unwrap()
                .value
                .unwrap(),
            2.0
        );
    }

    #[test]
    fn test_record_list() {
        let metrics = SessionStoreMetrics::new().unwrap();
        metrics.record_list("tenant-1", 0.02, true);
        metrics.record_list("tenant-2", 0.01, true);

        let gathered = metrics.registry().gather();
        let list_metric = gathered
            .iter()
            .find(|m| m.name() == "session_store_list_operations_total")
            .expect("list_operations_total metric not found");

        // Should have 2 tenants
        assert_eq!(list_metric.metric.len(), 2);
    }
}
