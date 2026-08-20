//! Local (in-process) client for the `AuthZ` resolver.

use std::sync::Arc;

use async_trait::async_trait;
use authz_resolver_sdk::{AuthZResolverApi, EvaluationRequest, EvaluationResponse};
use toolkit_canonical_errors::CanonicalError;
use toolkit_macros::domain_model;
use toolkit_security::SecurityContext;

use super::{DomainError, Service};

/// Local client wrapping the service.
#[domain_model]
pub struct AuthZResolverLocalClient {
    svc: Arc<Service>,
}

impl AuthZResolverLocalClient {
    #[must_use]
    pub fn new(svc: Arc<Service>) -> Self {
        Self { svc }
    }
}

/// Map an infrastructure `DomainError` onto the contract's `CanonicalError`.
/// Access denial is never surfaced here — it rides in `EvaluationResponse`.
fn log_and_convert(op: &str, e: DomainError) -> CanonicalError {
    tracing::error!(operation = op, error = ?e, "authz_resolver call failed");
    CanonicalError::internal(e.to_string()).create()
}

#[async_trait]
impl AuthZResolverApi for AuthZResolverLocalClient {
    async fn evaluate(
        &self,
        _ctx: SecurityContext,
        req: EvaluationRequest,
    ) -> Result<EvaluationResponse, CanonicalError> {
        // The subject identity travels inside `req` (AuthZEN Subject); the
        // in-process PDP does not need the caller's `SecurityContext`.
        self.svc
            .evaluate(req)
            .await
            .map_err(|e| log_and_convert("evaluate", e))
    }
}
