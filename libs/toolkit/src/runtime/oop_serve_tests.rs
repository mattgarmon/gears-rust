//! Tests for the `OoP` HTTP serve edge (probes, drain guard, auth wiring).

use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt; // for `oneshot`

fn probe_router() -> Router {
    let readiness = ReadinessState::new(Vec::<String>::new());
    build_probe_router(readiness, Arc::new("{\"openapi\":\"3.1.0\"}".to_owned()))
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
    let app = build_probe_router(Arc::clone(&readiness), Arc::new(String::new()));

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
    let app = build_probe_router(Arc::clone(&readiness), Arc::new(String::new()));

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
