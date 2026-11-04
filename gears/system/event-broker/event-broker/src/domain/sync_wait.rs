//! The synchronous-publish wait registry (`DESIGN.md:1335`, feature 0001 §5.1).
//!
//! A `Prefer: wait=N` publish holds until the backend stores its events. Ingest
//! mints a trace (a UUID, never on the wire), stamps it on every outbox row it
//! enqueues, and registers a waiter here *before* the enqueue commits; the
//! outbox worker ([`crate::infra::workers::IngestOutboxHandler`]) signals the
//! trace once each row's `backend.append` succeeds.
//!
//! Process-local by design: a publish whose rows drain on another ingest
//! instance (cluster mode) never sees its signal and times out - the
//! documented `202`, needing no cross-node coordination.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use toolkit::domain_model;
use uuid::Uuid;

/// A per-process map from a publish's internal trace to the count of its events
/// the backend has stored so far. Shared (by `Arc`) between the ingest service,
/// which registers waiters, and the ingest outbox worker, which signals them.
#[domain_model]
#[derive(Default)]
pub struct SyncWaitRegistry {
    /// Each live sync publish, keyed by its trace. The `watch` channel carries
    /// how many of the trace's events are stored; the worker advances it and a
    /// waiter reads it. A trace absent from the map is one with no waiter (an
    /// async publish, or a waiter that already timed out), and a signal for it
    /// is a no-op.
    waiters: Mutex<HashMap<Uuid, watch::Sender<usize>>>,
}

impl SyncWaitRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Raises `trace`'s stored count by one. A no-op for a trace with no waiter
    /// (an async publish, or one that already timed out).
    pub fn signal(&self, trace: Uuid) {
        if let Some(tx) = self.lock().get(&trace) {
            tx.send_modify(|stored| *stored += 1);
        }
    }

    /// Registers a waiter for `trace` at zero stored. Called before the enqueue
    /// commits, so the worker can never signal a row before its waiter exists.
    fn insert(&self, trace: Uuid) -> watch::Receiver<usize> {
        let (tx, rx) = watch::channel(0usize);
        self.lock().insert(trace, tx);
        rx
    }

    fn remove(&self, trace: Uuid) {
        self.lock().remove(&trace);
    }

    /// The waiter map, recovering a poisoned lock: holders do a single map
    /// operation and cannot panic mid-update, so a lost wait must never take
    /// the broker's publish path down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, watch::Sender<usize>>> {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A live sync-publish wait: registers its trace on construction and
/// deregisters on drop, so a waiter is never leaked however the publish path
/// returns.
#[domain_model]
pub struct SyncWaitGuard {
    registry: Arc<SyncWaitRegistry>,
    trace: Uuid,
    stored: watch::Receiver<usize>,
}

impl SyncWaitGuard {
    /// Registers a waiter for `trace` on `registry`.
    #[must_use]
    pub fn new(registry: Arc<SyncWaitRegistry>, trace: Uuid) -> Self {
        let stored = registry.insert(trace);
        Self {
            registry,
            trace,
            stored,
        }
    }

    /// Holds until `expected` of the trace's events are stored, or `wait`
    /// elapses: `true` if all were stored in time, `false` on timeout (the
    /// events stay enqueued). `expected` is the admitted count; an all-duplicate
    /// publish admits none and answers `200` without reaching here.
    pub async fn wait_stored(&mut self, expected: usize, wait: Duration) -> bool {
        // `wait_for` returns at once if the predicate already holds; the sender
        // outlives the wait, so the only non-success result is a timeout.
        matches!(
            tokio::time::timeout(wait, self.stored.wait_for(|stored| *stored >= expected)).await,
            Ok(Ok(_))
        )
    }
}

impl Drop for SyncWaitGuard {
    fn drop(&mut self) {
        self.registry.remove(self.trace);
    }
}

#[cfg(test)]
#[path = "sync_wait_tests.rs"]
mod sync_wait_tests;
