use super::*;

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use cf_system_sdks::directory::{
    LabelSelector, RegisterAck, ServiceEndpoint, ServiceInstanceInfo,
};

use crate::runtime::DependencyChecker;

/// Records every mutation and models directory membership so a heartbeat for an
/// unknown record returns `NOT_FOUND` exactly as the real directory does.
#[derive(Default)]
struct Calls {
    registers: Vec<(String, String, Option<ReportedState>)>,
    heartbeats: Vec<(String, String, Option<ReportedState>)>,
    deregisters: Vec<(String, String)>,
    known: HashSet<(String, String)>,
    lease_ttl_ms: u64,
}

struct MockDir {
    calls: Mutex<Calls>,
}

impl MockDir {
    fn new(lease_ttl_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Calls {
                lease_ttl_ms,
                ..Calls::default()
            }),
        })
    }

    /// Forget a record so its next heartbeat returns `NOT_FOUND`, emulating a
    /// directory restart or an eviction.
    fn forget(&self, gear: &str, id: &str) {
        self.calls
            .lock()
            .unwrap()
            .known
            .remove(&(gear.to_owned(), id.to_owned()));
    }
}

#[async_trait]
impl DirectoryClient for MockDir {
    async fn resolve_grpc_service(&self, service_name: &str) -> Result<ServiceEndpoint> {
        Err(DirectoryNotFound::new(service_name).into())
    }
    async fn resolve_rest_service(&self, gear_name: &str) -> Result<ServiceEndpoint> {
        Err(DirectoryNotFound::new(gear_name).into())
    }
    async fn get_openapi_spec(&self, gear_name: &str) -> Result<String> {
        Err(DirectoryNotFound::new(gear_name).into())
    }
    async fn list_instances(&self, _gear: &str) -> Result<Vec<ServiceInstanceInfo>> {
        Ok(vec![])
    }
    async fn resolve_by_labels(
        &self,
        _gear: &str,
        _selector: &LabelSelector,
    ) -> Result<Vec<ServiceInstanceInfo>> {
        Ok(vec![])
    }
    async fn list_all_instances(&self) -> Result<Vec<ServiceInstanceInfo>> {
        Ok(vec![])
    }
    async fn register_instance(&self, info: RegisterInstanceInfo) -> Result<()> {
        self.register_with_lease(info).await.map(|_| ())
    }
    async fn register_with_lease(&self, info: RegisterInstanceInfo) -> Result<RegisterAck> {
        let mut calls = self.calls.lock().unwrap();
        calls
            .registers
            .push((info.gear.clone(), info.instance_id.clone(), info.reported_state));
        calls.known.insert((info.gear, info.instance_id));
        let ttl = (calls.lease_ttl_ms > 0).then(|| Duration::from_millis(calls.lease_ttl_ms));
        Ok(RegisterAck { lease_ttl: ttl })
    }
    async fn deregister_instance(&self, gear: &str, instance_id: &str) -> Result<()> {
        let mut calls = self.calls.lock().unwrap();
        calls
            .deregisters
            .push((gear.to_owned(), instance_id.to_owned()));
        calls.known.remove(&(gear.to_owned(), instance_id.to_owned()));
        Ok(())
    }
    async fn send_heartbeat(&self, gear: &str, instance_id: &str) -> Result<()> {
        self.heartbeat(gear, instance_id, None).await
    }
    async fn heartbeat(
        &self,
        gear: &str,
        instance_id: &str,
        reported: Option<ReportedState>,
    ) -> Result<()> {
        let mut calls = self.calls.lock().unwrap();
        calls
            .heartbeats
            .push((gear.to_owned(), instance_id.to_owned(), reported));
        if calls.known.contains(&(gear.to_owned(), instance_id.to_owned())) {
            Ok(())
        } else {
            Err(DirectoryNotFound::new(format!("instance {gear}/{instance_id}")).into())
        }
    }
}

fn info(gear: &str, id: &str) -> RegisterInstanceInfo {
    RegisterInstanceInfo::new(gear, id).with_rest_endpoint(ServiceEndpoint::http(gear, 8080))
}

#[tokio::test]
async fn register_learns_lease_ttl_and_paces_renewal_at_a_third() {
    let dir = MockDir::new(30_000);
    let deps = Arc::new(DependencyChecker::new());
    let pm = PresenceManager::new(dir.clone(), ReportedStateSource::Dependencies(deps), Duration::from_secs(5));

    pm.register(info("billing", "i1")).await.unwrap();

    // 30s lease -> renew every 10s, not the 5s fallback.
    assert_eq!(pm.renewal_interval(), Duration::from_secs(10));
    // The registration carried the process reported state (ready -> Serving).
    let calls = dir.calls.lock().unwrap();
    assert_eq!(calls.registers, vec![(
        "billing".to_owned(),
        "i1".to_owned(),
        Some(ReportedState::Serving),
    )]);
}

#[tokio::test]
async fn renewal_interval_falls_back_without_a_lease_ttl() {
    let dir = MockDir::new(0); // directory returns no TTL
    let deps = Arc::new(DependencyChecker::new());
    let pm = PresenceManager::new(dir, ReportedStateSource::Dependencies(deps), Duration::from_secs(7));
    pm.register(info("billing", "i1")).await.unwrap();
    assert_eq!(pm.renewal_interval(), Duration::from_secs(7));
}

#[tokio::test]
async fn renew_all_heartbeats_reported_state_from_dependency_checker() {
    let dir = MockDir::new(30_000);
    let deps = Arc::new(DependencyChecker::new());
    deps.register_dep("other"); // unresolved -> not ready
    let pm = PresenceManager::new(dir.clone(), ReportedStateSource::Dependencies(deps.clone()), Duration::from_secs(5));
    pm.register(info("billing", "i1")).await.unwrap();

    // Not ready yet -> Starting.
    pm.renew_all().await;
    // Dependencies resolve -> Serving.
    deps.mark_resolved("other");
    pm.renew_all().await;
    // Drain -> Draining.
    deps.set_draining(true);
    pm.renew_all().await;

    let hb = &dir.calls.lock().unwrap().heartbeats;
    assert_eq!(hb[0].2, Some(ReportedState::Starting));
    assert_eq!(hb[1].2, Some(ReportedState::Serving));
    assert_eq!(hb[2].2, Some(ReportedState::Draining));
}

#[tokio::test]
async fn not_found_heartbeat_triggers_reregistration() {
    let dir = MockDir::new(30_000);
    let deps = Arc::new(DependencyChecker::new());
    let pm = PresenceManager::new(dir.clone(), ReportedStateSource::Dependencies(deps), Duration::from_secs(5));
    pm.register(info("billing", "i1")).await.unwrap();

    // The directory forgets the record (restart / eviction).
    dir.forget("billing", "i1");

    pm.renew_all().await;

    let calls = dir.calls.lock().unwrap();
    // One heartbeat (which got NOT_FOUND) and a re-registration.
    assert_eq!(calls.heartbeats.len(), 1);
    assert_eq!(
        calls.registers.len(),
        2,
        "the initial register plus the re-register after NOT_FOUND"
    );
    assert!(calls.known.contains(&("billing".to_owned(), "i1".to_owned())));
}

#[tokio::test]
async fn begin_drain_reports_draining_then_deregister_all_removes_records() {
    let dir = MockDir::new(30_000);
    let deps = Arc::new(DependencyChecker::new());
    let pm = PresenceManager::new(dir.clone(), ReportedStateSource::Dependencies(deps.clone()), Duration::from_secs(5));
    pm.register(info("billing", "i1")).await.unwrap();

    // Drain sequence: flip readiness, push an immediate Draining heartbeat.
    deps.set_draining(true);
    pm.begin_drain();
    pm.renew_all().await; // stands in for the loop's wake-driven pass
    assert_eq!(
        dir.calls.lock().unwrap().heartbeats.last().unwrap().2,
        Some(ReportedState::Draining)
    );

    // End of drain: deregister and stop tracking.
    pm.deregister_all().await;
    assert_eq!(
        dir.calls.lock().unwrap().deregisters,
        vec![("billing".to_owned(), "i1".to_owned())]
    );
}
