//! File-based configuration store for single-tenant LunaRoute deployments
//!
//! This crate implements the `ConfigStore` trait using YAML files on disk.
//! It's designed for local/single-tenant deployments where configuration
//! is stored in a simple file.
//!
//! # Features
//! - File-based configuration storage
//! - Real-time file watching with `notify`
//! - YAML format support
//! - Configuration validation
//!
//! # Example
//! ```no_run
//! # use lunaroute_config_file::FileConfigStore;
//! # use lunaroute_core::ConfigStore;
//! # async fn example() -> lunaroute_core::Result<()> {
//! let store = FileConfigStore::new("~/.lunaroute/config.yaml").await?;
//! let config = store.get_config(None).await?;
//! # Ok(())
//! # }
//! ```

mod file_store;

pub use file_store::FileConfigStore;
