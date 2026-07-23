//! Tests for the `OoP` HTTP serve edge (probes, drain guard, auth wiring).

use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt; // for `oneshot`

fn probe_router() -> Router {
    let readiness = ReadinessState::new(Vec::<String>::new());
    build_probe_router(readiness, Arc::from("{\"openapi\":\"3.1.0\"}"))
}

#[tokio::test]
async fn healthz_is_always_ok() {
    let app = probe_router();
    let resp = app
        .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn readyz_reports_503_until_deps_resolved_then_200() {
    let readiness = ReadinessState::new(["billing"]);
    let app = build_probe_router(Arc::clone(&readiness), Arc::from(""));

    let resp = app
        .clone()
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    readiness.mark_dep_resolved("billing");

    let resp = app
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn readyz_503_when_draining() {
    let readiness = ReadinessState::new(Vec::<String>::new());
    readiness.set_draining(true);
    let app = build_probe_router(Arc::clone(&readiness), Arc::from(""));

    let resp = app
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn well_known_openapi_is_served() {
    let app = probe_router();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
}

#[tokio::test]
async fn drain_guard_rejects_new_requests_while_draining() {
    let readiness = ReadinessState::new(Vec::<String>::new());
    let guard = DrainGuard::new(Arc::clone(&readiness));

    let gear = Router::new().route("/work", get(|| async { "done" }));
    let app = gear.layer(from_fn_with_state(guard.clone(), drain_guard_middleware));

    // Not draining: request succeeds.
    let resp = app
        .clone()
        .oneshot(Request::builder().uri("/work").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Draining: request rejected with 503 + Retry-After.
    readiness.set_draining(true);
    let resp = app
        .oneshot(Request::builder().uri("/work").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(resp.headers().get(RETRY_AFTER).is_some());
}

#[tokio::test]
async fn drain_guard_tracks_in_flight_and_resets() {
    let readiness = ReadinessState::new(Vec::<String>::new());
    let guard = DrainGuard::new(readiness);
    assert_eq!(guard.in_flight(), 0);

    let gear = Router::new().route("/work", get(|| async { "done" }));
    let app = gear.layer(from_fn_with_state(guard.clone(), drain_guard_middleware));

    let resp = app
        .oneshot(Request::builder().uri("/work").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // After completion the counter returns to zero.
    assert_eq!(guard.in_flight(), 0);
}

#[tokio::test]
async fn drain_in_flight_returns_when_zero() {
    let readiness = ReadinessState::new(Vec::<String>::new());
    let guard = DrainGuard::new(readiness);
    // No in-flight requests: must return promptly.
    drain_in_flight(&guard, Duration::from_secs(5)).await;
    assert_eq!(guard.in_flight(), 0);
}

// --- auth adapter smoke tests ---

struct AllowAuthN;
impl BearerAuthenticator for AllowAuthN {
    async fn authenticate(&self, _token: &str) -> Result<SecurityContext, AuthNError> {
        Ok(SecurityContext::anonymous())
    }
}

#[tokio::test]
async fn dyn_bearer_adapter_delegates() {
    let dynamic = DynBearerAuthenticator::new(AllowAuthN);
    let result = BearerAuthenticator::authenticate(&dynamic, "token").await;
    assert!(result.is_ok());
}

struct AllowInternal;
impl InternalAuthenticator for AllowInternal {
    async fn authenticate(&self, _token: &str) -> Result<PlatformIdentity, InternalAuthNError> {
        Ok(PlatformIdentity::KubernetesServiceAccount {
            namespace: "ns".to_owned(),
            service_account: "sa".to_owned(),
            pod: None,
        })
    }
}

#[tokio::test]
async fn dyn_internal_adapter_delegates() {
    let dynamic = DynInternalAuthenticator::new(AllowInternal);
    let result = InternalAuthenticator::authenticate(&dynamic, "token").await;
    assert!(result.is_ok());
}

// --- end-to-end: real bound server, acceptance criteria ---

use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;

use async_trait::async_trait;
use cf_system_sdks::directory::{
    DirectoryClient, RegisterInstanceInfo, ServiceEndpoint, ServiceInstanceInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::runtime::ResolvedRestEndpoints;

/// Stub directory that fails `resolve_rest_service` a fixed number of times
/// before succeeding, so `/readyz` can be observed transitioning 503 → 200.
struct E2eDirectory {
    fail_resolve: AtomicUsize,
    deregistered: AtomicUsize,
}

#[async_trait]
impl DirectoryClient for E2eDirectory {
    async fn resolve_grpc_service(&self, _s: &str) -> anyhow::Result<ServiceEndpoint> {
        Ok(ServiceEndpoint::new("http://grpc"))
    }
    async fn resolve_rest_service(&self, gear: &str) -> anyhow::Result<ServiceEndpoint> {
        if self.fail_resolve.load(Ordering::SeqCst) > 0 {
            self.fail_resolve.fetch_sub(1, Ordering::SeqCst);
            anyhow::bail!("not yet");
        }
        Ok(ServiceEndpoint::new(format!("http://{gear}:8080")))
    }
    async fn get_openapi_spec(&self, _g: &str) -> anyhow::Result<String> {
        Ok(String::new())
    }
    async fn list_instances(&self, _g: &str) -> anyhow::Result<Vec<ServiceInstanceInfo>> {
        Ok(vec![])
    }
    async fn register_instance(&self, _i: RegisterInstanceInfo) -> anyhow::Result<()> {
        Ok(())
    }
    async fn deregister_instance(&self, _g: &str, _i: &str) -> anyhow::Result<()> {
        self.deregistered.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn send_heartbeat(&self, _g: &str, _i: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Reserve an ephemeral port and return it (closing the listener so the server
/// can rebind it).
fn free_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

/// Minimal raw HTTP/1.1 GET returning the status code. Uses `Connection: close`
/// so the read completes on EOF.
async fn http_get(addr: SocketAddr, path: &str) -> Option<u16> {
    let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
}

/// Poll `path` until it returns `want` or the deadline elapses.
async fn poll_status(addr: SocketAddr, path: &str, want: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if http_get(addr, path).await == Some(want) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn e2e_startup_readiness_transition_and_graceful_shutdown() {
    let addr = free_addr();
    let directory: Arc<dyn DirectoryClient> = Arc::new(E2eDirectory {
        fail_resolve: AtomicUsize::new(3),
        deregistered: AtomicUsize::new(0),
    });

    let readiness = ReadinessState::new(["dep"]);
    let resolved = Arc::new(ResolvedRestEndpoints::new());
    let cancel = CancellationToken::new();

    let gear_router = Router::new().route("/ping", get(|| async { "pong" }));

    let options = OopServeOptions {
        gear_name: "test-gear".to_owned(),
        instance_id: "i-1".to_owned(),
        version: Some("1.0.0".to_owned()),
        advertise_uri: format!("http://{addr}"),
        listen_addr: addr,
        probe_bind_addr: None,
        drain_timeout: Duration::from_secs(5),
        directory: Arc::clone(&directory),
        bearer_authenticator: None,
        internal_authenticator: None,
    };

    let server = tokio::spawn(super::run_oop_http(
        gear_router,
        "{\"openapi\":\"3.1.0\"}".to_owned(),
        Arc::clone(&readiness),
        resolved,
        vec!["dep".to_owned()],
        options,
        cancel.clone(),
    ));

    // 1. Liveness is up quickly.
    assert!(
        poll_status(addr, "/healthz", 200, Duration::from_secs(3)).await,
        "/healthz should return 200 once the server is listening"
    );

    // 2. Gear routes serve immediately, before readiness.
    assert_eq!(http_get(addr, "/ping").await, Some(200), "gear route should serve");

    // 3. /readyz is 503 until the dependency resolves.
    assert_eq!(
        http_get(addr, "/readyz").await,
        Some(503),
        "/readyz should be 503 while dep unresolved"
    );

    // 4. Dependency resolves in the background → /readyz flips to 200.
    assert!(
        poll_status(addr, "/readyz", 200, Duration::from_secs(3)).await,
        "/readyz should transition to 200 after dep resolves"
    );
    assert!(readiness.all_deps_resolved());

    // 5. Well-known OpenAPI is served.
    assert_eq!(http_get(addr, "/.well-known/openapi.json").await, Some(200));

    // 6. Graceful shutdown completes.
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server task should finish promptly after cancel")
        .expect("server task should not panic");
    assert!(result.is_ok(), "graceful shutdown should succeed: {result:?}");
}
