//! Framework-managed readiness state for the `OoP` bootstrap (`cpt-cf-fr-eventual-readiness`).
//!
//! An `OoP` gear becomes *live* the moment its HTTP server binds (`/healthz`),
//! but only becomes *ready* (`/readyz`) once every critical dependency has been
//! resolved **and** every gear-registered [`ReadinessCheck`] reports it can
//! serve traffic. This module owns that aggregate state and the three-state
//! health model (`Ready` / `NotReady` / `Degraded`, Spring Boot-style health
//! groups per DESIGN § 3.2 / ADR-0005).
//!
//! The evaluated aggregate is cached for [`READINESS_CACHE_TTL`] so a burst of
//! probe traffic cannot storm the registered checks. Any state mutation
//! (dependency resolved, draining flip, check registered) invalidates the cache
//! so transitions take effect immediately.
//!
//! Gears reach this via [`crate::context::GearCtx::runtime`] →
//! [`RuntimeHandle::register_readiness_check`]. In Profile 1 (in-process) the
//! handle is *detached* (topo-sort already guarantees deps), so registration is
//! a no-op.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;

/// How long an evaluated readiness aggregate is cached before re-evaluation.
///
/// Bounds the rate at which registered [`ReadinessCheck`]s are polled so probe
/// traffic (kubelet + gateway + sidecars) cannot storm them.
pub const READINESS_CACHE_TTL: Duration = Duration::from_secs(1);

/// Outcome of a single [`ReadinessCheck`].
///
/// `Degraded` is distinct from `NotReady`: a degraded gear can still serve
/// traffic (so `/readyz` stays `200`), but the reduced-functionality reason is
/// surfaced in the JSON body for operators. `NotReady` forces `/readyz → 503`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// The check passed; the gear can serve traffic for this concern.
    Ready,
    /// The check failed; the gear must not receive traffic. Forces `503`.
    NotReady {
        /// Operator-facing reason, surfaced in the `/readyz` body.
        reason: String,
    },
    /// The gear can serve traffic but with reduced functionality. Stays `200`,
    /// but the reason is reported in the `/readyz` body.
    Degraded {
        /// Operator-facing reason, surfaced in the `/readyz` body.
        reason: String,
    },
}

/// A custom, gear-supplied readiness check evaluated on every `/readyz`
/// request (subject to the [`READINESS_CACHE_TTL`] cache).
///
/// Register via [`RuntimeHandle::register_readiness_check`]. The returned future
/// is `Send` so checks can run on a multi-threaded runtime.
#[async_trait]
pub trait ReadinessCheck: Send + Sync {
    /// Evaluate this check.
    async fn check(&self) -> CheckResult;
}

/// Per-check entry in the serialized `/readyz` body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CheckReport {
    /// The check passed.
    Ready,
    /// The check failed with a reason.
    NotReady {
        /// Operator-facing reason.
        reason: String,
    },
    /// The check is degraded with a reason.
    Degraded {
        /// Operator-facing reason.
        reason: String,
    },
}

impl From<CheckResult> for CheckReport {
    fn from(value: CheckResult) -> Self {
        match value {
            CheckResult::Ready => Self::Ready,
            CheckResult::NotReady { reason } => Self::NotReady { reason },
            CheckResult::Degraded { reason } => Self::Degraded { reason },
        }
    }
}

/// The aggregate readiness report rendered as the `/readyz` response body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReadinessReport {
    /// Whether the gear is ready to receive traffic (`true` → `200`, else `503`).
    pub ready: bool,
    /// Whether the gear is draining (graceful shutdown in progress).
    pub draining: bool,
    /// Critical dependencies not yet resolved via `DirectoryService` / DNS.
    pub unresolved_deps: Vec<String>,
    /// Per-check reports for every registered [`ReadinessCheck`].
    pub checks: BTreeMap<String, CheckReport>,
}

/// Shared, framework-owned readiness state for an `OoP` gear instance.
///
/// Created by the `OoP` bootstrap with the gear's critical dependency names.
/// Cloned as an `Arc` into the probe router, the dependency-resolution task,
/// and each gear's [`RuntimeHandle`].
pub struct ReadinessState {
    /// Critical deps still awaiting resolution. Empty ⇒ deps satisfied.
    unresolved_deps: Mutex<BTreeSet<String>>,
    /// Gear-registered custom checks, keyed by name.
    checks: Mutex<BTreeMap<String, Arc<dyn ReadinessCheck>>>,
    /// Graceful-shutdown flag; when set, `/readyz` reports `503` (readiness flip).
    draining: AtomicBool,
    /// 1s cache of the last evaluated aggregate.
    cache: Mutex<Option<(Instant, ReadinessReport)>>,
}

impl std::fmt::Debug for ReadinessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadinessState")
            .field("unresolved_deps", &self.unresolved_deps.lock())
            .field("check_names", &self.checks.lock().keys().collect::<Vec<_>>())
            .field("draining", &self.draining.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl ReadinessState {
    /// Create a new readiness state seeded with the gear's critical dependency
    /// names. All listed deps start unresolved; the gear is not ready until each
    /// is marked resolved via [`mark_dep_resolved`](Self::mark_dep_resolved).
    #[must_use]
    pub fn new<I, S>(critical_deps: I) -> Arc<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Arc::new(Self {
            unresolved_deps: Mutex::new(critical_deps.into_iter().map(Into::into).collect()),
            checks: Mutex::new(BTreeMap::new()),
            draining: AtomicBool::new(false),
            cache: Mutex::new(None),
        })
    }

    /// Register a custom readiness check under `name`.
    ///
    /// Registering a check invalidates the cache so it is considered on the next
    /// `/readyz` evaluation. A duplicate name replaces the previous check.
    pub fn register_readiness_check(&self, name: impl Into<String>, check: Arc<dyn ReadinessCheck>) {
        let name = name.into();
        tracing::debug!(check = %name, "registered readiness check");
        self.checks.lock().insert(name, check);
        self.invalidate_cache();
    }

    /// Mark a critical dependency as resolved. Idempotent; unknown names are
    /// ignored. Invalidates the cache so readiness can flip to `200` promptly.
    pub fn mark_dep_resolved(&self, name: &str) {
        let removed = self.unresolved_deps.lock().remove(name);
        if removed {
            tracing::info!(dep = %name, "critical dependency resolved");
            self.invalidate_cache();
        }
    }

    /// Whether all critical dependencies have been resolved.
    #[must_use]
    pub fn all_deps_resolved(&self) -> bool {
        self.unresolved_deps.lock().is_empty()
    }

    /// Set (or clear) the draining flag. Setting it flips `/readyz` to `503`
    /// immediately (invalidates the cache) so upstreams pull the instance out of
    /// rotation while in-flight requests drain.
    pub fn set_draining(&self, draining: bool) {
        self.draining.store(draining, Ordering::SeqCst);
        self.invalidate_cache();
    }

    /// Whether the gear is currently draining.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    fn invalidate_cache(&self) {
        *self.cache.lock() = None;
    }

    /// Evaluate the aggregate readiness, honoring the [`READINESS_CACHE_TTL`]
    /// cache.
    ///
    /// The gear is ready when it is not draining, all critical deps are
    /// resolved, and no registered check reports `NotReady`. `Degraded` checks
    /// keep the gear ready but are recorded in the report body.
    pub async fn evaluate(&self) -> ReadinessReport {
        if let Some((at, report)) = self.cache.lock().as_ref()
            && at.elapsed() < READINESS_CACHE_TTL
        {
            return report.clone();
        }

        let report = self.evaluate_uncached().await;
        *self.cache.lock() = Some((Instant::now(), report.clone()));
        report
    }

    async fn evaluate_uncached(&self) -> ReadinessReport {
        let draining = self.is_draining();
        let unresolved_deps: Vec<String> =
            self.unresolved_deps.lock().iter().cloned().collect();

        // Snapshot the checks so we don't hold the lock across `.await`.
        let checks: Vec<(String, Arc<dyn ReadinessCheck>)> = self
            .checks
            .lock()
            .iter()
            .map(|(name, check)| (name.clone(), Arc::clone(check)))
            .collect();

        let mut check_reports = BTreeMap::new();
        let mut any_not_ready = false;
        for (name, check) in checks {
            let result = check.check().await;
            if matches!(result, CheckResult::NotReady { .. }) {
                any_not_ready = true;
            }
            check_reports.insert(name, CheckReport::from(result));
        }

        let ready = !draining && unresolved_deps.is_empty() && !any_not_ready;

        ReadinessReport {
            ready,
            draining,
            unresolved_deps,
            checks: check_reports,
        }
    }
}

/// A cheap, cloneable handle to the runtime services a gear may use at runtime.
///
/// Currently exposes the readiness API. In Profile 1 (in-process) the handle is
/// *detached* — there is no `OoP` readiness state (topo-sort already guarantees
/// deps), so [`register_readiness_check`](Self::register_readiness_check) is a
/// no-op. In Profile 2/3 (`OoP`) the bootstrap threads a live handle through
/// each [`crate::context::GearCtx`].
#[derive(Clone, Default)]
pub struct RuntimeHandle {
    readiness: Option<Arc<ReadinessState>>,
}

impl std::fmt::Debug for RuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeHandle")
            .field("attached", &self.readiness.is_some())
            .finish()
    }
}

impl RuntimeHandle {
    /// Create a handle backed by a live [`ReadinessState`] (`OoP` profiles).
    #[must_use]
    pub fn new(readiness: Arc<ReadinessState>) -> Self {
        Self {
            readiness: Some(readiness),
        }
    }

    /// Create a detached handle (in-process / Profile 1). All operations are
    /// no-ops.
    #[must_use]
    pub fn detached() -> Self {
        Self { readiness: None }
    }

    /// Register a custom [`ReadinessCheck`] that gates the gear's `/readyz`
    /// probe. A no-op on a detached (in-process) handle.
    pub fn register_readiness_check(&self, name: impl Into<String>, check: Arc<dyn ReadinessCheck>) {
        match &self.readiness {
            Some(state) => state.register_readiness_check(name, check),
            None => {
                tracing::debug!(
                    check = %name.into(),
                    "register_readiness_check called on a detached runtime (in-process); ignoring"
                );
            }
        }
    }

    /// The backing [`ReadinessState`], if this handle is attached.
    #[must_use]
    pub fn readiness_state(&self) -> Option<Arc<ReadinessState>> {
        self.readiness.clone()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "readiness_tests.rs"]
mod tests;
