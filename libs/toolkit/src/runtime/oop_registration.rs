//! Background self-registration and dependency resolution for `OoP` gears
//! (`cpt-cf-component-oop-bootstrap`, OoP-4 tasks 4.3/4.4).
//!
//! Both run as non-blocking background tasks so the HTTP server and its probes
//! are up immediately:
//!
//! - **Self-registration** ([`registration_loop`]) registers the instance's
//!   gRPC/REST endpoints and OpenAPI spec with `DirectoryService`, retrying with
//!   exponential backoff (100ms → 30s cap) and periodically re-registering to
//!   self-heal after a Flight Control restart / connection loss.
//! - **Dependency resolution** ([`resolve_deps`]) polls
//!   `DirectoryService.resolve_rest_service` for each declared dependency, wires
//!   the resolved base URL into the [`ClientHub`] via [`ResolvedRestEndpoints`],
//!   and marks the dep resolved so `/readyz` can flip to `200`.
//!
//! In Profile 1 (in-process) dependency resolution is a no-op — the topo-sorted
//! `HostRuntime` already guarantees deps are initialized.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

use cf_system_sdks::directory::{DirectoryClient, RegisterInstanceInfo, ServiceEndpoint};

use super::readiness::ReadinessState;

/// Initial retry backoff for registration and dependency polling.
const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
/// Maximum retry backoff (cap).
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Interval at which a successfully-registered instance re-registers to
/// self-heal after a directory restart / connection loss.
const RE_REGISTER_INTERVAL: Duration = Duration::from_secs(30);

/// Next backoff in the exponential schedule (doubles, capped at [`MAX_BACKOFF`]).
fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_BACKOFF)
}

/// Sleep for `dur`, returning early (`false`) if `cancel` fires first.
async fn sleep_or_cancel(dur: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        () = cancel.cancelled() => false,
        () = tokio::time::sleep(dur) => true,
    }
}

/// Resolved REST base URLs for the gear's dependencies, keyed by gear name.
///
/// Registered into the [`ClientHub`] by the `OoP` bootstrap and populated as
/// dependencies resolve. Until typed REST client codegen lands, gears look up a
/// dependency's base URL here (`hub.get::<ResolvedRestEndpoints>()`).
#[derive(Debug, Default)]
pub struct ResolvedRestEndpoints {
    inner: DashMap<String, String>,
}

impl ResolvedRestEndpoints {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a resolved base URL for `gear`.
    pub fn set(&self, gear: impl Into<String>, uri: impl Into<String>) {
        self.inner.insert(gear.into(), uri.into());
    }

    /// Look up the resolved base URL for `gear`, if known.
    #[must_use]
    pub fn get(&self, gear: &str) -> Option<String> {
        self.inner.get(gear).map(|v| v.value().clone())
    }

    /// Number of resolved dependencies.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether no dependencies have resolved yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Register `info` with the directory, retrying with exponential backoff until
/// success or cancellation. Returns `true` on success, `false` if cancelled.
async fn register_once_with_backoff(
    directory: &Arc<dyn DirectoryClient>,
    info: &RegisterInstanceInfo,
    cancel: &CancellationToken,
) -> bool {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        if cancel.is_cancelled() {
            return false;
        }
        match directory.register_instance(clone_info(info)).await {
            Ok(()) => {
                tracing::info!(gear = %info.gear, instance = %info.instance_id, "registered with DirectoryService");
                return true;
            }
            Err(e) => {
                tracing::warn!(
                    gear = %info.gear,
                    error = %e,
                    backoff_ms = backoff.as_millis(),
                    "registration attempt failed; retrying"
                );
                if !sleep_or_cancel(backoff, cancel).await {
                    return false;
                }
                backoff = next_backoff(backoff);
            }
        }
    }
}

/// Clone a [`RegisterInstanceInfo`] (it is not `Clone` in the SDK).
fn clone_info(info: &RegisterInstanceInfo) -> RegisterInstanceInfo {
    RegisterInstanceInfo {
        gear: info.gear.clone(),
        instance_id: info.instance_id.clone(),
        grpc_services: info
            .grpc_services
            .iter()
            .map(|(n, ep)| (n.clone(), ServiceEndpoint::new(ep.uri.clone())))
            .collect(),
        version: info.version.clone(),
        rest_endpoint: info
            .rest_endpoint
            .as_ref()
            .map(|ep| ServiceEndpoint::new(ep.uri.clone())),
        openapi_spec: info.openapi_spec.clone(),
    }
}

/// Background self-registration loop.
///
/// Registers `info` (with backoff), then re-registers every
/// [`RE_REGISTER_INTERVAL`] to self-heal after a directory restart, until
/// `cancel` fires.
pub(crate) async fn registration_loop(
    directory: Arc<dyn DirectoryClient>,
    info: RegisterInstanceInfo,
    cancel: CancellationToken,
) {
    if !register_once_with_backoff(&directory, &info, &cancel).await {
        return;
    }
    loop {
        if !sleep_or_cancel(RE_REGISTER_INTERVAL, &cancel).await {
            return;
        }
        // Best-effort periodic re-registration (idempotent on the directory).
        if !register_once_with_backoff(&directory, &info, &cancel).await {
            return;
        }
    }
}

/// Poll for a single dependency until resolved (or cancelled), then wire it into
/// the resolved-endpoints registry and mark it resolved in readiness.
async fn resolve_one_dep(
    directory: Arc<dyn DirectoryClient>,
    dep: String,
    readiness: Arc<ReadinessState>,
    resolved: Arc<ResolvedRestEndpoints>,
    cancel: CancellationToken,
) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match directory.resolve_rest_service(&dep).await {
            Ok(endpoint) => {
                tracing::info!(dep = %dep, endpoint = %endpoint.uri, "resolved REST dependency");
                resolved.set(dep.clone(), endpoint.uri);
                readiness.mark_dep_resolved(&dep);
                return;
            }
            Err(e) => {
                tracing::debug!(dep = %dep, error = %e, "dependency not yet resolvable; retrying");
                if !sleep_or_cancel(backoff, &cancel).await {
                    return;
                }
                backoff = next_backoff(backoff);
            }
        }
    }
}

/// Spawn one resolution task per dependency. Each resolves independently and
/// gates `/readyz` via `readiness`. A no-op when `deps` is empty (Profile 1).
pub(crate) fn resolve_deps(
    directory: Arc<dyn DirectoryClient>,
    deps: Vec<String>,
    readiness: Arc<ReadinessState>,
    resolved: Arc<ResolvedRestEndpoints>,
    cancel: CancellationToken,
) {
    for dep in deps {
        let directory = Arc::clone(&directory);
        let readiness = Arc::clone(&readiness);
        let resolved = Arc::clone(&resolved);
        let cancel = cancel.clone();
        tokio::spawn(resolve_one_dep(directory, dep, readiness, resolved, cancel));
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "oop_registration_tests.rs"]
mod tests;
