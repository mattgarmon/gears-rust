//! Per-process lease renewal for every directory record the process registered
//! (ADR-0011, `cpt-cf-component-oop-bootstrap`).
//!
//! Under the lease model a registration is only kept observable while its lease
//! is renewed. Several registrars in one process write directory records — the
//! `OoP` presence loop, grpc-hub's gRPC registrar, [`RestAdvertiser`], and
//! event-broker's role registration — and none of them, except the `OoP`
//! presence loop, used to heartbeat. The [`PresenceManager`] is the single per-process
//! owner of renewal for *all* of those records: whoever registers a record
//! hands it to the manager, which then renews it on one timer and re-registers
//! it if the directory forgets it.
//!
//! The state each record reports (`Starting` / `Serving` / `Draining`) is taken
//! from the process [`DependencyChecker`], so readiness and drain are reflected
//! in the directory without each registrar tracking them itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use cf_system_sdks::directory::{
    DirectoryClient, DirectoryNotFound, RegisterInstanceInfo, ReportedState,
};

use crate::runtime::readiness::{DependencyChecker, ReadinessLifecycle, ReadinessState};

/// Where a [`PresenceManager`] reads the state to report for this process
/// (ADR-0011), evaluated at each renewal: `Starting` while not ready, `Serving`
/// once ready, `Draining` while shutting down.
///
/// Using the *readiness* signal — not dependency resolution alone — keeps a gear
/// that is failing its readiness check from advertising itself as `Serving`.
/// There are exactly two process topologies, so this is a closed set rather than
/// an open trait.
pub enum ReportedStateSource {
    /// The `OoP` serve path's full readiness: startup completion, dependency
    /// resolution, healthchecks and drain (ADR-0005). `Ready`/`Degraded` are
    /// serving; anything else (including an `Unhealthy` healthcheck) is not.
    Readiness(Arc<ReadinessState>),
    /// The in-process host's dependency/drain signal — it has no aggregate
    /// readiness object, so this is the best available source.
    Dependencies(Arc<DependencyChecker>),
}

impl ReportedStateSource {
    /// The state to report right now.
    pub async fn reported_state(&self) -> ReportedState {
        match self {
            Self::Readiness(readiness) => match readiness.evaluate().await.state {
                ReadinessLifecycle::Draining => ReportedState::Draining,
                ReadinessLifecycle::Ready | ReadinessLifecycle::Degraded => ReportedState::Serving,
                ReadinessLifecycle::Starting => ReportedState::Starting,
            },
            Self::Dependencies(deps) => {
                if deps.is_draining() {
                    ReportedState::Draining
                } else if deps.all_resolved() {
                    ReportedState::Serving
                } else {
                    ReportedState::Starting
                }
            }
        }
    }
}

/// Lower bound on the renewal cadence, so a tiny or zero lease TTL cannot turn
/// the renewal loop into a hot loop.
const MIN_RENEWAL_INTERVAL: Duration = Duration::from_secs(1);

/// Identifies a tracked record within the process.
type RecordKey = (String, String);

/// Mutable state behind the manager's lock.
#[derive(Default)]
struct Inner {
    /// Every `(gear, instance_id)` the process registered, with the latest info
    /// used to register it, so a `NOT_FOUND` heartbeat can re-register it.
    records: HashMap<RecordKey, RegisterInstanceInfo>,
    /// Lease TTL the directory last granted, if any. `None` means fall back to
    /// the configured heartbeat interval (an old directory that returns no TTL).
    lease_ttl: Option<Duration>,
}

/// Owns lease renewal for every directory record registered by this process.
///
/// Create one per process, register each record through
/// [`register`](Self::register), and drive renewal with [`run`](Self::run).
/// During shutdown call [`begin_drain`](Self::begin_drain) (after readiness has
/// flipped to draining) to push an immediate `Draining` heartbeat, then
/// [`deregister_all`](Self::deregister_all) once in-flight requests have
/// drained.
pub struct PresenceManager {
    directory: Arc<dyn DirectoryClient>,
    /// Source of the reported state (ADR-0011): the process readiness signal,
    /// evaluated at each renewal.
    readiness: ReportedStateSource,
    /// Heartbeat interval used when the directory returns no lease TTL.
    fallback_interval: Duration,
    inner: parking_lot::Mutex<Inner>,
    /// Wakes the renewal loop for an out-of-band renewal (e.g. the immediate
    /// `Draining` heartbeat at the start of drain).
    wake: Notify,
}

impl PresenceManager {
    /// Create a manager that renews through `directory`, reporting the state
    /// from `readiness`, and heartbeating at `fallback_interval` until a
    /// registration teaches it the directory's lease TTL.
    #[must_use]
    pub fn new(
        directory: Arc<dyn DirectoryClient>,
        readiness: ReportedStateSource,
        fallback_interval: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            directory,
            readiness,
            fallback_interval,
            inner: parking_lot::Mutex::new(Inner::default()),
            wake: Notify::new(),
        })
    }

    /// The renewal cadence: a third of the granted lease TTL (ADR-0011), or the
    /// configured fallback when the directory returned none, clamped so it can
    /// never hot-loop.
    fn renewal_interval(&self) -> Duration {
        let base = self
            .inner
            .lock()
            .lease_ttl
            .map_or(self.fallback_interval, |ttl| ttl / 3);
        base.max(MIN_RENEWAL_INTERVAL)
    }

    /// Register (or re-register) `info` and track it for renewal.
    ///
    /// The declared state is overwritten with the process's current reported
    /// state so every registrar agrees on one record's liveness. The lease TTL
    /// the directory grants is remembered to pace renewals.
    ///
    /// # Errors
    /// Propagates a registration failure (e.g. a gRPC service-name conflict);
    /// the record is still tracked so renewal retries it.
    pub async fn register(&self, mut info: RegisterInstanceInfo) -> anyhow::Result<()> {
        info.reported_state = Some(self.readiness.reported_state().await);
        let key = (info.gear.clone(), info.instance_id.clone());
        // Track before the await so a record whose first registration fails is
        // still renewed (which re-registers it) rather than lost.
        self.inner.lock().records.insert(key, info.clone());

        let ack = self.directory.register_with_lease(info).await?;
        if let Some(ttl) = ack.lease_ttl {
            self.inner.lock().lease_ttl = Some(ttl);
        }
        Ok(())
    }

    /// Stop renewing a record without deregistering it, e.g. when the registrar
    /// that owns it deregisters it itself. Prevents the renewal loop from
    /// re-registering a record that was intentionally removed.
    pub fn untrack(&self, gear: &str, instance_id: &str) {
        self.inner
            .lock()
            .records
            .remove(&(gear.to_owned(), instance_id.to_owned()));
    }

    /// Request an immediate renewal pass. Used at the start of drain so the
    /// directory (and the edge) sees `Draining` without waiting for the next
    /// tick. Readiness must already be flipped to draining before calling this.
    pub fn begin_drain(&self) {
        self.wake.notify_one();
    }

    /// Run the renewal loop until `cancel` fires. Each pass heartbeats every
    /// tracked record with the current reported state; a record the directory
    /// has forgotten (`NOT_FOUND`) is re-registered, so recovery after a
    /// directory restart or an eviction takes one heartbeat.
    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        loop {
            let interval = self.renewal_interval();
            tokio::select! {
                () = cancel.cancelled() => break,
                () = self.wake.notified() => {}
                () = tokio::time::sleep(interval) => {}
            }
            self.renew_all().await;
        }
        tracing::debug!("presence manager renewal loop stopped");
    }

    /// Heartbeat every tracked record once, re-registering any the directory no
    /// longer knows.
    async fn renew_all(&self) {
        let reported = self.readiness.reported_state().await;
        let records: Vec<(RecordKey, RegisterInstanceInfo)> = self
            .inner
            .lock()
            .records
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for ((gear, instance_id), info) in records {
            match self
                .directory
                .heartbeat(&gear, &instance_id, Some(reported))
                .await
            {
                Ok(()) => {}
                Err(err) if err.is::<DirectoryNotFound>() => {
                    // The directory forgot this record (restart or eviction):
                    // re-register it immediately so it is routable again.
                    let mut info = info;
                    info.reported_state = Some(reported);
                    match self.directory.register_with_lease(info).await {
                        Ok(ack) => {
                            if let Some(ttl) = ack.lease_ttl {
                                self.inner.lock().lease_ttl = Some(ttl);
                            }
                            tracing::info!(
                                gear = %gear,
                                instance = %instance_id,
                                "re-registered with the directory after a NOT_FOUND heartbeat"
                            );
                        }
                        Err(e) => tracing::warn!(
                            gear = %gear,
                            instance = %instance_id,
                            error = %e,
                            "re-registration after NOT_FOUND failed; will retry next pass"
                        ),
                    }
                }
                Err(err) => tracing::warn!(
                    gear = %gear,
                    instance = %instance_id,
                    error = %err,
                    "heartbeat failed; will retry next pass"
                ),
            }
        }
    }

    /// Deregister every tracked record and stop tracking them. Called at the end
    /// of drain, after in-flight requests have finished, so the manager stays the
    /// single writer of each record it owns.
    pub async fn deregister_all(&self) {
        let records: Vec<RecordKey> = {
            let mut inner = self.inner.lock();
            inner.records.drain().map(|(k, _)| k).collect()
        };
        for (gear, instance_id) in records {
            if let Err(err) = self
                .directory
                .deregister_instance(&gear, &instance_id)
                .await
            {
                tracing::warn!(
                    gear = %gear,
                    instance = %instance_id,
                    error = %err,
                    "deregistration during shutdown failed"
                );
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "presence_manager_tests.rs"]
mod tests;
