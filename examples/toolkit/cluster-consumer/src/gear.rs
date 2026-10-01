//! Gear definition, cluster profile marker, REST wiring, and the leader-elected
//! background sweeper's lifecycle.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use axum::Router;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use cluster_sdk::ClusterProfile;
use toolkit::api::OpenApiRegistry;
use toolkit::{ClientHub, Gear, GearCtx, RestApiCapability, RunnableCapability};

use crate::domain::ReservationService;
use crate::rest;
use crate::sweeper::{self, NodeState};

/// The cluster profile this consumer binds. Its [`NAME`](ClusterProfile::NAME)
/// must match a profile the cluster gear serves (see `config/oop-cluster.yaml`,
/// which configures a `demo` profile on the `standalone` backend).
#[derive(Clone, Copy)]
pub struct DemoProfile;

impl ClusterProfile for DemoProfile {
    const NAME: &'static str = "demo";
}

// The consumer's entire cluster-facing declaration: this line, plus the
// `.profile(DemoProfile)` call in each resolver (see `domain.rs`). No wiring
// call, no endpoint, no mode flag — the framework replays cluster-sdk's
// `ConsumerRegistration` for us (DESIGN §4.9.2).
cluster_sdk::register_cluster_profile!(DemoProfile);

/// A running sweeper task and the token that stops it.
struct SweeperTask {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

/// Cluster-consuming demo gear: a seat-reservation service.
///
/// - `#[toolkit::gear]` registers it under `cluster-consumer` (the kebab-case of
///   the struct ident) with REST and `stateful` (background-task) capabilities.
/// - It deliberately declares **no** `deps = [cluster]`: a Profile-3 consumer
///   does not link the cluster gear, and `deps` is a hard topo-sort edge that
///   would make the process refuse to start. Ordering (cluster is a `system`
///   gear) and readiness gating come from cluster-sdk's `ConsumerRegistration`.
///
/// State: the `ClientHub` is captured at `init` so the background sweeper (whose
/// `start` receives no `GearCtx`) can resolve facades; `node` is shared with the
/// REST layer so `/status` reports the sweeper's live role.
#[toolkit::gear(name = "cluster-consumer", capabilities = [rest, stateful])]
pub struct ClusterConsumer {
    hub: OnceLock<Arc<ClientHub>>,
    node: Arc<NodeState>,
    sweeper: Mutex<Option<SweeperTask>>,
}

impl Default for ClusterConsumer {
    fn default() -> Self {
        Self {
            hub: OnceLock::new(),
            node: Arc::new(NodeState::new()),
            sweeper: Mutex::new(None),
        }
    }
}

impl ClusterConsumer {
    /// Build a reservation service over the captured hub and shared node state.
    fn service(&self) -> Result<Arc<ReservationService>> {
        let hub = self
            .hub
            .get()
            .context("cluster-consumer: client hub was not captured at init")?;
        Ok(Arc::new(ReservationService::new(
            Arc::clone(hub),
            Arc::clone(&self.node),
        )))
    }
}

#[async_trait]
impl Gear for ClusterConsumer {
    async fn init(&self, ctx: &GearCtx) -> Result<()> {
        // Lazy consumption: nothing is resolved at init (§4.9.1 forbids it — no
        // provider's `start` has run yet). Capture the hub so `start` (which gets
        // no ctx) can resolve the cluster facades once providers are up.
        if self.hub.set(ctx.client_hub()).is_err() {
            tracing::debug!("cluster-consumer: client hub was already captured");
        }
        tracing::info!("cluster-consumer initialized");
        Ok(())
    }
}

impl RestApiCapability for ClusterConsumer {
    fn register_rest(
        &self,
        ctx: &GearCtx,
        router: Router,
        openapi: &dyn OpenApiRegistry,
    ) -> Result<Router> {
        tracing::info!("Registering cluster-consumer REST routes");
        let service = Arc::new(ReservationService::new(
            ctx.client_hub(),
            Arc::clone(&self.node),
        ));
        Ok(rest::register_routes(router, openapi, service))
    }
}

#[async_trait]
impl RunnableCapability for ClusterConsumer {
    async fn start(&self, cancel: CancellationToken) -> Result<()> {
        // Spawn the leader-elected sweeper on a child token so both this gear's
        // `stop` and a root shutdown end it. On loopback (cluster unreachable) it
        // simply campaigns-and-backs-off; it never blocks startup.
        let service = self.service()?;
        let child = cancel.child_token();
        let handle = tokio::spawn(sweeper::run(service, child.clone()));
        *self.sweeper.lock().unwrap_or_else(PoisonError::into_inner) = Some(SweeperTask {
            cancel: child,
            handle,
        });
        tracing::info!("cluster-consumer sweeper started");
        Ok(())
    }

    async fn stop(&self, deadline_token: CancellationToken) -> Result<()> {
        let Some(task) = self
            .sweeper
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return Ok(());
        };
        // Request cooperative shutdown, then wait for the task to drain up to the
        // framework's hard deadline, aborting if it overruns.
        task.cancel.cancel();
        let mut handle = task.handle;
        tokio::select! {
            _ = &mut handle => {}
            () = deadline_token.cancelled() => handle.abort(),
        }
        tracing::info!("cluster-consumer sweeper stopped");
        Ok(())
    }
}
