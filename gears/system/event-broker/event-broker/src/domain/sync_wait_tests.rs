//! Unit tests for [`SyncWaitRegistry`]/[`SyncWaitGuard`].

use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use super::{SyncWaitGuard, SyncWaitRegistry};

#[tokio::test(start_paused = true)]
async fn one_signal_releases_a_single_event_wait() {
    let registry = Arc::new(SyncWaitRegistry::new());
    let trace = Uuid::now_v7();
    let mut guard = SyncWaitGuard::new(Arc::clone(&registry), trace);

    // The worker stores the one event before the wait's bound elapses.
    registry.signal(trace);

    assert!(guard.wait_stored(1, Duration::from_secs(30)).await);
}

#[tokio::test(start_paused = true)]
async fn a_wait_times_out_when_fewer_than_expected_are_stored() {
    let registry = Arc::new(SyncWaitRegistry::new());
    let trace = Uuid::now_v7();
    let mut guard = SyncWaitGuard::new(Arc::clone(&registry), trace);

    // Only one of the batch's two events is stored; the wait elapses.
    registry.signal(trace);

    assert!(!guard.wait_stored(2, Duration::from_secs(30)).await);
}

#[tokio::test(start_paused = true)]
async fn a_batch_wait_releases_once_every_event_is_stored() {
    let registry = Arc::new(SyncWaitRegistry::new());
    let trace = Uuid::now_v7();
    let mut guard = SyncWaitGuard::new(Arc::clone(&registry), trace);

    registry.signal(trace);
    registry.signal(trace);
    registry.signal(trace);

    assert!(guard.wait_stored(3, Duration::from_secs(30)).await);
}

#[tokio::test(start_paused = true)]
async fn a_signal_delivered_during_the_wait_releases_it() {
    let registry = Arc::new(SyncWaitRegistry::new());
    let trace = Uuid::now_v7();
    let mut guard = SyncWaitGuard::new(Arc::clone(&registry), trace);

    let signaller = Arc::clone(&registry);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        signaller.signal(trace);
    });

    assert!(guard.wait_stored(1, Duration::from_secs(30)).await);
}

#[test]
fn a_signal_for_an_unregistered_trace_is_a_no_op() {
    // A signal for an async publish (no waiter) or a timed-out waiter must not
    // panic and must change nothing.
    let registry = SyncWaitRegistry::new();
    registry.signal(Uuid::now_v7());
}

#[test]
fn dropping_a_guard_deregisters_its_trace() {
    let registry = Arc::new(SyncWaitRegistry::new());
    let trace = Uuid::now_v7();
    {
        let _guard = SyncWaitGuard::new(Arc::clone(&registry), trace);
        assert!(registry.waiters.lock().unwrap().contains_key(&trace));
    }
    assert!(!registry.waiters.lock().unwrap().contains_key(&trace));
}
