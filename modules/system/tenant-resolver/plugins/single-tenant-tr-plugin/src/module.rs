//! Single-tenant resolver plugin module.

use std::sync::Arc;

use async_trait::async_trait;
use modkit::Module;
use modkit::client_hub::ClientScope;
use modkit::context::ModuleCtx;
use modkit::gts::PluginV1;
use tenant_resolver_sdk::{TenantResolverPluginClient, TenantResolverPluginSpecV1};
use tracing::info;
use types_registry_sdk::{RegisterResult, TypesRegistryClient};

use crate::config::SingleTenantTrPluginConfig;
use crate::domain::Service;

/// Single-tenant resolver plugin module.
///
/// Zero-configuration plugin for single-tenant deployments.
/// Returns the tenant from security context as the only accessible tenant.
#[modkit::module(
    name = "single-tenant-tr-plugin",
    deps = ["types-registry"]
)]
pub struct SingleTenantTrPlugin;

impl Default for SingleTenantTrPlugin {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Module for SingleTenantTrPlugin {
    async fn init(&self, ctx: &ModuleCtx) -> anyhow::Result<()> {
        let cfg: SingleTenantTrPluginConfig = ctx.config_or_default()?;
        info!(
            vendor = %cfg.vendor,
            priority = cfg.priority,
            "Loaded single-tenant resolver plugin configuration"
        );

        // Build registration payload and instance id for this plugin.
        let (instance_id, instance_json) =
            PluginV1::<TenantResolverPluginSpecV1>::build_registration(
                "cf.builtin.single_tenant_resolver.plugin.v1",
                &cfg.vendor,
                cfg.priority,
            )?;

        // Publish to types-registry.
        let registry = ctx.client_hub().get::<dyn TypesRegistryClient>()?;
        let results = registry.register(vec![instance_json]).await?;
        RegisterResult::ensure_all_ok(&results)?;

        // Create service and register scoped client in ClientHub
        let service = Arc::new(Service);
        let api: Arc<dyn TenantResolverPluginClient> = service;
        ctx.client_hub()
            .register_scoped::<dyn TenantResolverPluginClient>(
                ClientScope::gts_id(&instance_id),
                api,
            );

        info!(instance_id = %instance_id, vendor = %cfg.vendor, priority = cfg.priority, "registered");
        Ok(())
    }
}
