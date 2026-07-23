//! Tests for the readiness subsystem.

use super::*;
use std::sync::atomic::AtomicUsize;

struct StaticCheck(CheckResult);

#[async_trait]
impl ReadinessCheck for StaticCheck {
    async fn check(&self) -> CheckResult {
        self.0.clone()
    }
}

/// Counts how many times it is polled; used to assert cache behavior.
struct CountingCheck {
    calls: Arc<AtomicUsize>,
    result: CheckResult,
}

#[async_trait]
impl ReadinessCheck for CountingCheck {
    async fn check(&self) -> CheckResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

#[tokio::test]
async fn no_deps_no_checks_is_ready() {
    let state = ReadinessState::new(Vec::<String>::new());
    let report = state.evaluate().await;
    assert!(report.ready);
    assert!(!report.draining);
    assert!(report.unresolved_deps.is_empty());
    assert!(report.checks.is_empty());
}

#[tokio::test]
async fn unresolved_deps_block_readiness_until_resolved() {
    let state = ReadinessState::new(["billing", "catalog"]);

    let report = state.evaluate().await;
    assert!(!report.ready);
    assert_eq!(report.unresolved_deps, vec!["billing", "catalog"]);

    state.mark_dep_resolved("billing");
    let report = state.evaluate().await;
    assert!(!report.ready);
    assert_eq!(report.unresolved_deps, vec!["catalog"]);

    state.mark_dep_resolved("catalog");
    let report = state.evaluate().await;
    assert!(report.ready);
    assert!(report.unresolved_deps.is_empty());
}

#[tokio::test]
async fn mark_unknown_dep_is_ignored() {
    let state = ReadinessState::new(["billing"]);
    state.mark_dep_resolved("does-not-exist");
    assert!(!state.all_deps_resolved());
    let report = state.evaluate().await;
    assert!(!report.ready);
    assert_eq!(report.unresolved_deps, vec!["billing"]);
}

#[tokio::test]
async fn not_ready_check_forces_not_ready_with_reason() {
    let state = ReadinessState::new(Vec::<String>::new());
    state.register_readiness_check(
        "cache_warm",
        Arc::new(StaticCheck(CheckResult::NotReady {
            reason: "cache cold".to_owned(),
        })),
    );

    let report = state.evaluate().await;
    assert!(!report.ready);
    assert_eq!(
        report.checks.get("cache_warm"),
        Some(&CheckReport::NotReady {
            reason: "cache cold".to_owned()
        })
    );
}

#[tokio::test]
async fn degraded_check_stays_ready_but_reported() {
    let state = ReadinessState::new(Vec::<String>::new());
    state.register_readiness_check(
        "indexer",
        Arc::new(StaticCheck(CheckResult::Degraded {
            reason: "rebuilding index".to_owned(),
        })),
    );

    let report = state.evaluate().await;
    assert!(report.ready, "degraded must still be ready (200)");
    assert_eq!(
        report.checks.get("indexer"),
        Some(&CheckReport::Degraded {
            reason: "rebuilding index".to_owned()
        })
    );
}

#[tokio::test]
async fn draining_flips_to_not_ready() {
    let state = ReadinessState::new(Vec::<String>::new());
    assert!(state.evaluate().await.ready);

    state.set_draining(true);
    let report = state.evaluate().await;
    assert!(!report.ready);
    assert!(report.draining);
}

#[tokio::test]
async fn evaluation_is_cached_within_ttl() {
    let calls = Arc::new(AtomicUsize::new(0));
    let state = ReadinessState::new(Vec::<String>::new());
    state.register_readiness_check(
        "counter",
        Arc::new(CountingCheck {
            calls: Arc::clone(&calls),
            result: CheckResult::Ready,
        }),
    );

    // First evaluate populates the cache (1 poll). Registration invalidated the
    // cache, so the first call after registration recomputes.
    let _ = state.evaluate().await;
    let after_first = calls.load(Ordering::SeqCst);
    assert_eq!(after_first, 1);

    // Subsequent evaluations within the TTL must not re-poll the check.
    let _ = state.evaluate().await;
    let _ = state.evaluate().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "checks should be cached");
}

#[tokio::test]
async fn mutation_invalidates_cache() {
    let calls = Arc::new(AtomicUsize::new(0));
    let state = ReadinessState::new(["dep"]);
    state.register_readiness_check(
        "counter",
        Arc::new(CountingCheck {
            calls: Arc::clone(&calls),
            result: CheckResult::Ready,
        }),
    );

    let _ = state.evaluate().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A state mutation must invalidate the cache and force re-evaluation.
    state.mark_dep_resolved("dep");
    let report = state.evaluate().await;
    assert!(report.ready);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "mutation must bust cache");
}

#[test]
fn detached_runtime_handle_register_is_noop() {
    let handle = RuntimeHandle::detached();
    // Must not panic and must remain detached.
    handle.register_readiness_check(
        "x",
        Arc::new(StaticCheck(CheckResult::Ready)),
    );
    assert!(handle.readiness_state().is_none());
}

#[tokio::test]
async fn attached_runtime_handle_registers_check() {
    let state = ReadinessState::new(Vec::<String>::new());
    let handle = RuntimeHandle::new(Arc::clone(&state));
    handle.register_readiness_check(
        "gate",
        Arc::new(StaticCheck(CheckResult::NotReady {
            reason: "warming".to_owned(),
        })),
    );
    assert!(handle.readiness_state().is_some());
    assert!(!state.evaluate().await.ready);
}
