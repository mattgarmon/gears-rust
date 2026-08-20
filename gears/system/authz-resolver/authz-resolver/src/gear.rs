//! `AuthZ` resolver gear.

use std::sync::Arc;

use async_trait::async_trait;
use authz_resolver_sdk::AuthZResolverApi;
use toolkit::api::OpenApiRegistry;
use toolkit::context::GearCtx;
use toolkit::contracts::SystemCapability;
use toolkit::{Gear, RestApiCapability};
use toolkit_contract::policy::PolicyStack;
use tracing::info;

use crate::config::AuthZResolverConfig;
use crate::domain::{AuthZResolverLocalClient, Service};

/// `AuthZ` Resolver gear.
///
/// This gear:
/// 1. Discovers plugin instances via types-registry
/// 2. Routes requests to the selected plugin based on vendor configuration
///
/// The `AuthZResolverPluginSpecV1` schema itself reaches `types-registry`
/// automatically via the `toolkit-gts` link-time inventory — no per-init
/// registration is needed. Plugin discovery is lazy: happens on first API
/// call after types-registry is ready.
///
/// `AuthZResolverClient` is exposed as a `#[toolkit::contract]`: `provides`
/// auto-wires the local impl into `ClientHub`, and the `rest` capability hosts
/// the contract's REST projection (`/authz-resolver/v1/evaluate`, an internal
/// platform-plane route) so out-of-process PEPs can reach the PDP over HTTP via
/// directory resolution.
#[toolkit::gear(
    name = "authz-resolver",
    deps = [types_registry],
    capabilities = [system, rest]
)]
#[toolkit::provides(
    contract = authz_resolver_sdk::AuthZResolverApi,
    local = Self::build_local,
    transports = [local, rest],
)]
#[derive(Default)]
pub(crate) struct AuthZResolver;

impl AuthZResolver {
    /// Local factory invoked by `#[toolkit::provides]` when wiring resolves to
    /// `ClientWiring::Local` (the in-process default for the provider itself).
    ///
    /// Builds the domain [`Service`] from the gear's config + `ClientHub` and
    /// wraps it in the object-safe [`AuthZResolverLocalClient`].
    fn build_local(
        ctx: &GearCtx,
        _policies: Arc<PolicyStack>,
    ) -> anyhow::Result<Arc<dyn AuthZResolverApi>> {
        let cfg: AuthZResolverConfig = ctx.config_or_default()?;
        info!(vendor = %cfg.vendor, "wiring authz-resolver local client");
        let svc = Arc::new(Service::new(ctx.client_hub(), cfg.vendor));
        Ok(Arc::new(AuthZResolverLocalClient::new(svc)))
    }
}

// Marked as `system` so that init() runs in the system-gear phase.
// This ensures the AuthZResolver client is available in ClientHub before
// other system gears that depend on it.
impl SystemCapability for AuthZResolver {}

#[async_trait]
impl Gear for AuthZResolver {
    async fn init(&self, ctx: &GearCtx) -> anyhow::Result<()> {
        // `#[toolkit::provides]`-generated wiring: validates the contract IR,
        // reads wiring config, and registers `Arc<dyn AuthZResolverClient>` in
        // the ClientHub (local impl by default).
        self.wire_auth_z_resolver_api(ctx).await?;
        Ok(())
    }
}

impl RestApiCapability for AuthZResolver {
    fn register_rest(
        &self,
        ctx: &GearCtx,
        router: axum::Router,
        openapi: &dyn OpenApiRegistry,
    ) -> anyhow::Result<axum::Router> {
        // Host the contract's REST projection. The route is authenticated and
        // NOT public, so the edge api-gateway does not expose it externally —
        // only in-cluster PEPs resolve it via the directory.
        let service = ctx.client_hub().get::<dyn AuthZResolverApi>()?;
        Ok(
            authz_resolver_sdk::rest::register_auth_z_resolver_api_rest_routes(
                router, openapi, service,
            ),
        )
    }
}
