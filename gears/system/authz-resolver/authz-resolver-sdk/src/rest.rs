//! REST projection of [`AuthZResolverClient`].
//!
//! Carries the HTTP method/path annotations consumed by
//! `#[toolkit::rest_contract]`. When the `rest-client` feature is enabled the
//! macro also emits `AuthZResolverClientRestClient` (and its directory-
//! resolving wrapper `AuthZResolverClientRestResolvingClient`) that implement
//! [`AuthZResolverClient`] over HTTP; when `rest-server` is enabled it emits
//! `register_auth_z_resolver_client_rest_routes` for the gear to host.
//!
//! The `evaluate` route is an **internal platform-plane** API (service-to-
//! service). It is deliberately not marked public, so the edge api-gateway does
//! not expose it to external clients — only trusted in-cluster callers reach it
//! directly via directory resolution.

use toolkit_canonical_errors::CanonicalError;
use toolkit_security::SecurityContext;

use crate::api::AuthZResolverApi;
use crate::models::{EvaluationRequest, EvaluationResponse};

/// HTTP projection of [`AuthZResolverApi`].
#[toolkit::rest_contract(base_path = "/authz-resolver/v1")]
pub trait AuthZResolverApiRest: AuthZResolverApi {
    /// `POST /authz-resolver/v1/evaluate` — evaluate an AuthZEN request.
    #[post("/evaluate")]
    async fn evaluate(
        &self,
        ctx: SecurityContext,
        req: EvaluationRequest,
    ) -> Result<EvaluationResponse, CanonicalError>;
}
