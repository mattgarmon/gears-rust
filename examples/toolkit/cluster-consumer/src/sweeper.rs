//! Leader-elected background maintenance: exactly one pod at a time seeds the
//! roster and periodically reclaims expired holds.
//!
//! This is the leader-election primitive earning its place. Every pod runs
//! [`run`], but [`LeaderWatch::run_while_leader`](cluster_sdk::LeaderWatch::run_while_leader)
//! only invokes the maintenance loop on the pod that currently holds the
//! `cluster-consumer/seat-sweeper` claim; the rest observe as followers and do
//! nothing until they win a later election. Leadership is advisory (ADR-002): the
//! sweep's correctness still rests on the per-seat CAS, so a brief two-leader
//! overlap during failover cannot double-reclaim a seat.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::domain::ReservationService;

/// The election every pod campaigns in for the singleton maintenance role.
const ELECTION_NAME: &str = "seat-sweeper";
/// How often the leader reclaims expired holds.
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
/// Grace given to the maintenance loop to stop on leadership loss before it is
/// aborted (passed to `run_while_leader`).
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Backoff between attempts to (re)establish the election when the cluster is
/// unreachable — e.g. on plain loopback, where the DNS endpoint never resolves.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

/// Cross-task snapshot of this pod's coordination role, written by the sweeper
/// and read by the `/status` route. All fields are lock-free so `/status` stays a
/// fast, cluster-free read.
#[derive(Debug, Default)]
pub struct NodeState {
    is_leader: AtomicBool,
    sweeper_running: AtomicBool,
    total_reclaimed: AtomicU64,
    last_reclaimed: AtomicU64,
}

impl NodeState {
    /// A fresh state: follower, sweeper not yet started.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn set_leader(&self, leader: bool) {
        self.is_leader.store(leader, Ordering::Relaxed);
    }

    fn set_sweeper_running(&self, running: bool) {
        self.sweeper_running.store(running, Ordering::Relaxed);
    }

    fn record_sweep(&self, reclaimed: u64) {
        self.last_reclaimed.store(reclaimed, Ordering::Relaxed);
        self.total_reclaimed.fetch_add(reclaimed, Ordering::Relaxed);
    }

    /// Does this pod currently hold the maintenance leadership? Advisory.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::Relaxed)
    }

    /// Is the background sweeper task alive on this pod (campaigning or leading)?
    #[must_use]
    pub fn sweeper_running(&self) -> bool {
        self.sweeper_running.load(Ordering::Relaxed)
    }

    /// Holds reclaimed by this pod on its most recent sweep pass.
    #[must_use]
    pub fn last_reclaimed(&self) -> u64 {
        self.last_reclaimed.load(Ordering::Relaxed)
    }

    /// Total holds reclaimed by this pod across all its sweeps as leader.
    #[must_use]
    pub fn total_reclaimed(&self) -> u64 {
        self.total_reclaimed.load(Ordering::Relaxed)
    }

    /// A short role label for `/status`.
    #[must_use]
    pub fn role(&self) -> &'static str {
        if self.is_leader() {
            "leader"
        } else if self.sweeper_running() {
            "follower"
        } else {
            "idle"
        }
    }
}

/// Run the leader-elected maintenance loop until `cancel` fires (gear shutdown).
///
/// Campaigns for the `seat-sweeper` claim and, while elected, seeds the roster
/// once and sweeps expired holds every [`SWEEP_INTERVAL`]. If the cluster is
/// unreachable it retries with backoff rather than exiting, so a pod on loopback
/// keeps trying quietly instead of crashing the process.
pub async fn run(service: Arc<ReservationService>, cancel: CancellationToken) {
    let node = Arc::clone(service.node());
    node.set_sweeper_running(true);

    while !cancel.is_cancelled() {
        match campaign_once(&service, &cancel).await {
            Campaign::ClusterUnavailable => {
                // No election possible right now (e.g. loopback). Back off, retry.
                if sleep_or_cancel(RECONNECT_BACKOFF, &cancel).await {
                    break;
                }
            }
            Campaign::WatchClosed => {
                // The election ended terminally (cluster shutdown). Re-establish
                // unless we're shutting down too.
                if sleep_or_cancel(RECONNECT_BACKOFF, &cancel).await {
                    break;
                }
            }
            Campaign::Cancelled => break,
        }
    }

    node.set_leader(false);
    node.set_sweeper_running(false);
    tracing::info!("seat-sweeper stopped");
}

/// Outcome of one campaign attempt.
enum Campaign {
    /// The cluster could not be resolved/reached to elect.
    ClusterUnavailable,
    /// The election watch closed terminally (e.g. cluster shutdown).
    WatchClosed,
    /// Gear shutdown was requested while campaigning/leading.
    Cancelled,
}

async fn campaign_once(service: &Arc<ReservationService>, cancel: &CancellationToken) -> Campaign {
    let Some(watch) = try_join(service).await else {
        return Campaign::ClusterUnavailable;
    };
    tracing::info!("seat-sweeper joined election as a candidate");

    let node = Arc::clone(service.node());
    let svc = Arc::clone(service);
    // `run_while_leader` invokes the loop only while this pod is leader, cancels
    // its token on loss, re-invokes on re-election, and returns when the watch
    // closes terminally. Race it against gear shutdown so `stop()` ends it too.
    let lead = watch.run_while_leader(STOP_TIMEOUT, move |leader_token| {
        let svc = Arc::clone(&svc);
        let node = Arc::clone(&node);
        async move { lead_loop(svc, node, leader_token).await }
    });

    tokio::select! {
        () = lead => Campaign::WatchClosed,
        () = cancel.cancelled() => Campaign::Cancelled,
    }
}

/// Resolve the leader facade and join the election, logging (and returning
/// `None`) if the cluster is unreachable.
async fn try_join(service: &Arc<ReservationService>) -> Option<cluster_sdk::LeaderWatch> {
    let leader = match service.leader().await {
        Ok(facade) => facade,
        Err(e) => {
            tracing::debug!(error = ?e, "seat-sweeper: cluster unavailable, will retry");
            return None;
        }
    };
    match leader.elect(ELECTION_NAME).await {
        Ok(watch) => Some(watch),
        Err(e) => {
            tracing::debug!(error = ?e, "seat-sweeper: could not join election, will retry");
            None
        }
    }
}

/// The maintenance work performed while this pod holds leadership. Returns when
/// `leader_token` is cancelled (leadership lost or shutting down).
// The body is six straight-line statements; the cognitive-complexity score is
// inflated by the `tracing` macro and `await` expansions, not real branching.
#[allow(clippy::cognitive_complexity)]
async fn lead_loop(
    service: Arc<ReservationService>,
    node: Arc<NodeState>,
    leader_token: CancellationToken,
) {
    node.set_leader(true);
    tracing::info!("seat-sweeper elected leader; seeding roster and sweeping");

    if let Err(e) = service.seed_roster().await {
        tracing::warn!(error = ?e, "seat-sweeper: roster seed failed (will still sweep)");
    }

    sweep_until_lost(&service, &node, &leader_token).await;

    node.set_leader(false);
    tracing::info!("seat-sweeper stepped down from leadership");
}

/// Sweep on a fixed cadence until leadership is lost or shutdown is requested
/// (`leader_token` cancelled).
async fn sweep_until_lost(
    service: &ReservationService,
    node: &NodeState,
    leader_token: &CancellationToken,
) {
    let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        tokio::select! {
            () = leader_token.cancelled() => break,
            _ = ticker.tick() => sweep_pass(service, node).await,
        }
    }
}

/// One reclaim pass, recording the count and swallowing a transient failure so
/// the leader loop keeps ticking.
async fn sweep_pass(service: &ReservationService, node: &NodeState) {
    match service.sweep_expired().await {
        Ok(reclaimed) => {
            if reclaimed > 0 {
                tracing::info!(reclaimed, "seat-sweeper reclaimed expired holds");
            }
            node.record_sweep(reclaimed);
        }
        Err(e) => tracing::warn!(error = ?e, "seat-sweeper: sweep pass failed"),
    }
}

/// Sleep for `dur`, or return `true` early if `cancel` fires first.
async fn sleep_or_cancel(dur: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(dur) => false,
    }
}
