//! `AuthZ` Resolver SDK
//!
//! This crate provides the public API for the `authz_resolver` gear:
//!
//! - [`AuthZResolverClient`] - Public API trait for consumers
//! - [`AuthZResolverPluginClient`] - Plugin API trait for implementations
//! - [`EvaluationRequest`], [`EvaluationResponse`] - Evaluation models
//! - [`Constraint`], [`Predicate`] - Constraint types
//! - [`AuthZResolverError`] - Error types
//! - [`AuthZResolverPluginSpecV1`] - GTS schema for plugin discovery
//! - [`pep`] - PEP helpers ([`PolicyEnforcer`], [`ResourceType`], compiler)
//!
//! ## Usage
//!
//! ```ignore
//! use authz_resolver_sdk::{
//!     AuthZResolverClient,
//!     pep::{AccessRequest, PolicyEnforcer, ResourceType},
//! };
//!
//! const USER: ResourceType = ResourceType::from_static(
//!     gts_id!("cf.core.users.user.v1~"),
//!     &["owner_tenant_id", "id"],
//! );
//!
//! // Get the client from ClientHub
//! let authz = hub.get::<dyn AuthZResolverClient>()?;
//!
//! // Create an enforcer (once, during init - serves all resource types)
//! let enforcer = PolicyEnforcer::new(authz);
//!
//! // All CRUD operations return AccessScope (PDP always returns constraints)
//! let scope = enforcer.access_scope(&ctx, &USER, "get", Some(id)).await?;
//!
//! // CREATE - also returns AccessScope with constraints from PDP
//! let scope = enforcer.access_scope_with(
//!     &ctx, &USER, "create", None,
//!     &AccessRequest::new()
//!         .context_tenant_id(target_tenant_id)
//!         .resource_property("owner_tenant_id", target_tenant_id),
//! ).await?;
//! ```
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod api;
pub mod constraints;
pub mod error;
pub mod gts;
pub mod models;
pub mod pep;
pub mod plugin_api;

/// REST projection of the [`AuthZResolverClient`] contract (HTTP transport).
///
/// The base contract in [`api`] is transport-agnostic; this module carries the
/// HTTP method/path annotations consumed by `#[toolkit::rest_contract]` and,
/// under `rest-client`, emits the generated directory-resolving REST client
/// used by out-of-process PEP consumers.
pub mod rest;

// Re-export the contract trait and its generated IR builder. The trait name
// must carry the `Api` suffix (contract-kind convention); `AuthZResolverClient`
// is kept as a backwards-compatible alias for existing in-process consumers.
#[doc(hidden)]
pub use api::AuthZResolverApi as AuthZResolverClient;
pub use api::{AuthZResolverApi, auth_z_resolver_api_ir};

// REST projection: base projection trait + HTTP binding builder (always), plus
// the generated clients when `rest-client` is enabled.
pub use constraints::{
    Constraint, EqPredicate, InGroupPredicate, InGroupSubtreePredicate, InPredicate,
    InTenantSubtreePredicate, Predicate,
};
pub use error::AuthZResolverError;
pub use gts::AuthZResolverPluginSpecV1;
pub use models::{
    Action, BarrierMode, Capability, DenyReason, EvaluationRequest, EvaluationRequestContext,
    EvaluationResponse, EvaluationResponseContext, Resource, Subject, TenantContext, TenantMode,
};
pub use pep::{AccessRequest, EnforcerError, IntoPropertyValue, PolicyEnforcer, ResourceType};
pub use plugin_api::AuthZResolverPluginClient;
pub use rest::{AuthZResolverApiRest, auth_z_resolver_api_rest_http_binding};
#[cfg(feature = "rest-client")]
pub use rest::{AuthZResolverApiRestClient, AuthZResolverApiRestResolvingClient};
