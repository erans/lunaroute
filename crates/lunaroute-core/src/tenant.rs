//! Tenant types and context for multi-tenancy support

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

use crate::{Error, Result};

/// Unique identifier for a tenant in multi-tenant deployments.
///
/// In single-tenant mode, this is `None` in `TenantContext`.
/// In multi-tenant mode, each request must have an associated `TenantId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(Uuid);

impl TenantId {
    /// Create a new random tenant ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create a tenant ID from a UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the inner UUID
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    /// Parse a tenant ID from a string
    pub fn from_string(s: &str) -> Result<Self> {
        let uuid = Uuid::parse_str(s)
            .map_err(|e| Error::InvalidTenant(format!("Invalid tenant ID format: {}", e)))?;
        Ok(Self(uuid))
    }

    /// Create a tenant ID from a subdomain
    ///
    /// This performs a lookup from subdomain to tenant ID.
    /// In production, this would query a cache or database.
    pub fn from_subdomain(_subdomain: &str) -> Result<Self> {
        // TODO: Implement actual subdomain -> tenant_id mapping
        // For now, return an error
        Err(Error::InvalidTenant(
            "Subdomain mapping not implemented".to_string(),
        ))
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TenantId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_string(s)
    }
}

/// Tenant context containing configuration and session stores.
///
/// This struct is the core of the dependency injection pattern used
/// throughout LunaRoute. It holds trait objects for the config and
/// session stores, allowing different implementations to be swapped
/// without changing business logic.
///
/// # Single-Tenant Mode
/// ```no_run
/// # use lunaroute_core::tenant::{TenantContext, TenantId};
/// # use std::sync::Arc;
/// # fn get_stores() -> (Arc<dyn std::any::Any + Send + Sync>, Arc<dyn std::any::Any + Send + Sync>) { todo!() }
/// let (config_store, session_store) = get_stores();
/// let context = TenantContext {
///     tenant_id: None,  // Single-tenant mode
///     config_store,
///     session_store,
/// };
/// ```
///
/// # Multi-Tenant Mode
/// ```no_run
/// # use lunaroute_core::tenant::{TenantContext, TenantId};
/// # use std::sync::Arc;
/// # fn get_stores() -> (Arc<dyn std::any::Any + Send + Sync>, Arc<dyn std::any::Any + Send + Sync>) { todo!() }
/// # fn extract_tenant_from_request() -> TenantId { todo!() }
/// let tenant_id = extract_tenant_from_request();
/// let (config_store, session_store) = get_stores();
/// let context = TenantContext {
///     tenant_id: Some(tenant_id),
///     config_store,
///     session_store,
/// };
/// ```
pub struct TenantContext {
    /// Optional tenant ID (None = single-tenant mode)
    pub tenant_id: Option<TenantId>,

    /// Configuration store (trait object)
    ///
    /// This will be `Arc<dyn ConfigStore>` but we can't reference
    /// ConfigStore here due to module organization. The actual type
    /// is defined in the `config_store` module.
    pub config_store: Arc<dyn std::any::Any + Send + Sync>,

    /// Session store (trait object)
    ///
    /// This will be `Arc<dyn SessionStore>` but we can't reference
    /// SessionStore here due to module organization. The actual type
    /// is defined in the `session_store` module.
    pub session_store: Arc<dyn std::any::Any + Send + Sync>,
}

impl TenantContext {
    /// Check if this is single-tenant mode
    pub fn is_single_tenant(&self) -> bool {
        self.tenant_id.is_none()
    }

    /// Check if this is multi-tenant mode
    pub fn is_multi_tenant(&self) -> bool {
        self.tenant_id.is_some()
    }

    /// Get the tenant ID, returning an error if in single-tenant mode
    pub fn require_tenant(&self) -> Result<TenantId> {
        self.tenant_id
            .ok_or_else(|| Error::TenantRequired("Operation requires tenant ID".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_id_creation() {
        let id1 = TenantId::new();
        let id2 = TenantId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_tenant_id_from_string() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let tenant_id = TenantId::from_string(uuid_str).unwrap();
        assert_eq!(tenant_id.to_string(), uuid_str);
    }

    #[test]
    fn test_tenant_id_invalid_string() {
        let result = TenantId::from_string("not-a-uuid");
        assert!(result.is_err());
    }

    #[test]
    fn test_tenant_id_display() {
        let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let tenant_id = TenantId::from_uuid(uuid);
        assert_eq!(
            tenant_id.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }
}
