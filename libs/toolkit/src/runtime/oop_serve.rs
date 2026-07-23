//! Out-of-process (`OoP`) HTTP server: probes, two-plane auth, and graceful
//! drain (`cpt-cf-component-oop-bootstrap`, OoP-4 tasks 4.1/4.2/4.5/4.6).
//!
//! This module owns the edge of an `OoP` gear's HTTP surface:
//!
//! - **Framework probes** — `/healthz` (liveness), `/readyz` (readiness, gated
//!   on dependency resolution + custom checks), and `/.well-known/openapi.json`
//!   (the canonical discovery path; `cpt-cf-binding-constraint-openapi-well-known`).
//! - **Two-plane auth** — installs [`internal_auth_middleware`] (platform plane)
//!   *before* [`security_context_middleware`] (tenant plane) per DESIGN § 3.2,
//!   but only when the corresponding authenticator is injected. Because both
//!   authenticator traits use return-position `impl Trait` (not `dyn`-safe),
//!   callers inject the object-safe [`DynBearerAuthenticator`] /
//!   [`DynInternalAuthenticator`] adapters.
//! - **Graceful drain** — a drain guard rejects new gear-route requests with
//!   `503 + Retry-After` once draining begins, while in-flight requests finish.
//!
//! The server itself is bound and driven by [`serve`], which is invoked from the
//! `HostRuntime` OoP path. Self-registration and dependency resolution live in
//! [`super::oop_registration`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header::RETRY_AFTER},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::get,
};
use tokio_util::sync::CancellationToken;

use toolkit_http_middleware::{internal_auth_middleware, security_context_middleware};
use toolkit_security::{
    AuthNError, BearerAuthenticator, InternalAuthNError, InternalAuthenticator, PlatformIdentity,
    SecurityContext,
};

use super::readiness::ReadinessState;

/// `Retry-After` (seconds) advertised while the gear is draining.
const DRAIN_RETRY_AFTER_SECONDS: u64 = 5;

// ---------------------------------------------------------------------------
// Object-safe authenticator adapters
// ---------------------------------------------------------------------------

type BearerFuture<'a> = Pin<Box<dyn Future<Output = Result<SecurityContext, AuthNError>> + Send + 'a>>;
type InternalFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PlatformIdentity, InternalAuthNError>> + Send + 'a>>;

/// Object-safe erasure of [`BearerAuthenticator`].
///
/// [`BearerAuthenticator::authenticate`] returns `impl Future`, so the trait is
/// not `dyn`-compatible. This trait boxes the future so a concrete authenticator
/// can be stored behind an `Arc` and injected at the bootstrap layer.
trait ErasedBearer: Send + Sync {
    fn authenticate<'a>(&'a self, token: &'a str) -> BearerFuture<'a>;
}

impl<A: BearerAuthenticator> ErasedBearer for A {
    fn authenticate<'a>(&'a self, token: &'a str) -> BearerFuture<'a> {
        Box::pin(BearerAuthenticator::authenticate(self, token))
    }
}

/// Injectable, object-safe tenant-plane authenticator.
///
/// Wrap a concrete [`BearerAuthenticator`] (e.g. an `AuthNResolverClient`
/// adapter supplied by the app/gear binary) with [`DynBearerAuthenticator::new`]
/// and hand it to [`OopServeOptions::bearer_authenticator`].
#[derive(Clone)]
pub struct DynBearerAuthenticator(Arc<dyn ErasedBearer>);

impl DynBearerAuthenticator {
    /// Erase a concrete [`BearerAuthenticator`] into the injectable adapter.
    #[must_use]
    pub fn new<A: BearerAuthenticator + 'static>(authenticator: A) -> Self {
        Self(Arc::new(authenticator))
    }

    /// Erase an already-`Arc`'d [`BearerAuthenticator`].
    #[must_use]
    pub fn from_arc<A: BearerAuthenticator + 'static>(authenticator: Arc<A>) -> Self {
        // Adapt Arc<A> to Arc<dyn ErasedBearer> via a thin wrapper.
        struct W<A>(Arc<A>);
        impl<A: BearerAuthenticator> ErasedBearer for W<A> {
            fn authenticate<'a>(&'a self, token: &'a str) -> BearerFuture<'a> {
                Box::pin(BearerAuthenticator::authenticate(&*self.0, token))
            }
        }
        Self(Arc::new(W(authenticator)))
    }
}

impl BearerAuthenticator for DynBearerAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<SecurityContext, AuthNError> {
        self.0.authenticate(token).await
    }
}

/// Object-safe erasure of [`InternalAuthenticator`] (same rationale as
/// [`ErasedBearer`]).
trait ErasedInternal: Send + Sync {
    fn authenticate<'a>(&'a self, token: &'a str) -> InternalFuture<'a>;
}

impl<A: InternalAuthenticator> ErasedInternal for A {
    fn authenticate<'a>(&'a self, token: &'a str) -> InternalFuture<'a> {
        Box::pin(InternalAuthenticator::authenticate(self, token))
    }
}

/// Injectable, object-safe platform-plane authenticator.
///
/// Wrap a concrete [`InternalAuthenticator`] (e.g. the K8s `TokenReview`
/// validator) with [`DynInternalAuthenticator::new`] and hand it to
/// [`OopServeOptions::internal_authenticator`].
#[derive(Clone)]
pub struct DynInternalAuthenticator(Arc<dyn ErasedInternal>);

impl DynInternalAuthenticator {
    /// Erase a concrete [`InternalAuthenticator`] into the injectable adapter.
    #[must_use]
    pub fn new<A: InternalAuthenticator + 'static>(authenticator: A) -> Self {
        Self(Arc::new(authenticator))
    }

    /// Erase an already-`Arc`'d [`InternalAuthenticator`].
    #[must_use]
    pub fn from_arc<A: InternalAuthenticator + 'static>(authenticator: Arc<A>) -> Self {
        struct W<A>(Arc<A>);
        impl<A: InternalAuthenticator> ErasedInternal for W<A> {
            fn authenticate<'a>(&'a self, token: &'a str) -> InternalFuture<'a> {
                Box::pin(InternalAuthenticator::authenticate(&*self.0, token))
            }
        }
        Self(Arc::new(W(authenticator)))
    }
}

impl InternalAuthenticator for DynInternalAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<PlatformIdentity, InternalAuthNError> {
        self.0.authenticate(token).await
    }
}

// ---------------------------------------------------------------------------
// Serve options
// ---------------------------------------------------------------------------

/// Configuration for the `OoP` HTTP server, assembled by the bootstrap layer.
pub struct OopServeOptions {
    /// Address the main HTTP server binds to (gear routes + probes).
    pub listen_addr: std::net::SocketAddr,
    /// Optional separate address for probe endpoints (sidecar port). When set,
    /// probes are served here *and* on the main listener.
    pub probe_bind_addr: Option<std::net::SocketAddr>,
    /// Maximum time to wait for in-flight requests to drain on shutdown.
    pub drain_timeout: Duration,
    /// Tenant-plane authenticator; when `Some`, `security_context_middleware`
    /// is installed on gear routes.
    pub bearer_authenticator: Option<DynBearerAuthenticator>,
    /// Platform-plane authenticator; when `Some`, `internal_auth_middleware` is
    /// installed on gear routes (runs before the tenant plane).
    pub internal_authenticator: Option<DynInternalAuthenticator>,
}

impl std::fmt::Debug for OopServeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OopServeOptions")
            .field("listen_addr", &self.listen_addr)
            .field("probe_bind_addr", &self.probe_bind_addr)
            .field("drain_timeout", &self.drain_timeout)
            .field("bearer_authenticator", &self.bearer_authenticator.is_some())
            .field(
                "internal_authenticator",
                &self.internal_authenticator.is_some(),
            )
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Probe router
// ---------------------------------------------------------------------------

/// Shared state for the probe endpoints.
#[derive(Clone)]
struct ProbeState {
    readiness: Arc<ReadinessState>,
    openapi_json: Arc<String>,
}

/// Build the framework probe router: `/healthz`, `/readyz`, and the
/// well-known OpenAPI discovery path.
///
/// These routes carry no tenant JWT and are never subject to the drain guard or
/// the auth middlewares (they must respond during startup and drain).
#[must_use]
pub(crate) fn build_probe_router(
    readiness: Arc<ReadinessState>,
    openapi_json: Arc<String>,
) -> Router {
    let state = ProbeState {
        readiness,
        openapi_json,
    };

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/.well-known/openapi.json", get(openapi))
        .route("/openapi.json", get(openapi))
        .with_state(state)
}

/// Liveness: always `200` once the server is listening.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Readiness: `200` when ready, `503` with the unresolved-deps / failing-checks
/// body otherwise. `Degraded` checks keep the response `200`.
async fn readyz(State(state): State<ProbeState>) -> Response {
    let report = state.readiness.evaluate().await;
    let status = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(report)).into_response()
}

/// Serve the gear's generated OpenAPI document.
async fn openapi(State(state): State<ProbeState>) -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json",
        )],
        (*state.openapi_json).clone(),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Drain guard
// ---------------------------------------------------------------------------

/// Tracks in-flight gear-route requests and enforces the drain state.
#[derive(Clone)]
pub(crate) struct DrainGuard {
    readiness: Arc<ReadinessState>,
    in_flight: Arc<AtomicUsize>,
}

impl DrainGuard {
    pub(crate) fn new(readiness: Arc<ReadinessState>) -> Self {
        Self {
            readiness,
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Current number of in-flight gear-route requests.
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }
}

/// Middleware: reject new requests with `503 + Retry-After` while draining;
/// otherwise track the request as in-flight until it completes.
async fn drain_guard_middleware(State(guard): State<DrainGuard>, request: axum::extract::Request, next: Next) -> Response {
    if guard.readiness.is_draining() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(RETRY_AFTER, DRAIN_RETRY_AFTER_SECONDS.to_string())],
            "draining",
        )
            .into_response();
    }

    guard.in_flight.fetch_add(1, Ordering::SeqCst);
    let response = next.run(request).await;
    guard.in_flight.fetch_sub(1, Ordering::SeqCst);
    response
}

// ---------------------------------------------------------------------------
// Router assembly
// ---------------------------------------------------------------------------

/// Compose the final `OoP` router: gear routes (with drain guard + auth) merged
/// with the framework probe router (unguarded).
#[must_use]
pub(crate) fn assemble_router(
    gear_router: Router,
    probe_router: Router,
    drain_guard: DrainGuard,
    options: &OopServeOptions,
) -> Router {
    let mut gear = gear_router;

    // Auth planes (installed only when injected). Add tenant plane first so it
    // is *inner* to the platform plane — `internal_auth_middleware` must run
    // BEFORE `security_context_middleware` (DESIGN § 3.2).
    if let Some(bearer) = options.bearer_authenticator.clone() {
        gear = gear.layer(from_fn_with_state(
            Arc::new(bearer),
            security_context_middleware::<DynBearerAuthenticator>,
        ));
    }
    if let Some(internal) = options.internal_authenticator.clone() {
        gear = gear.layer(from_fn_with_state(
            Arc::new(internal),
            internal_auth_middleware::<DynInternalAuthenticator>,
        ));
    }

    // Drain guard is the outermost gear-route layer so rejected requests never
    // enter auth or handlers.
    gear = gear.layer(from_fn_with_state(drain_guard, drain_guard_middleware));

    // Probes are merged last and are intentionally unguarded/unauthenticated.
    Router::new().merge(gear).merge(probe_router)
}

// ---------------------------------------------------------------------------
// Serve
// ---------------------------------------------------------------------------

/// Bind and serve the assembled `OoP` router until `cancel` fires, then drain.
///
/// Serving order:
/// 1. Bind the main listener (and optional probe listener) — probes are up
///    immediately so kubelet sees liveness/readiness without waiting on
///    registration or dependency resolution.
/// 2. Serve with graceful shutdown wired to `cancel`.
/// 3. On `cancel`: flip readiness to draining (via the caller-owned
///    `ReadinessState`), wait up to `drain_timeout` for in-flight to reach zero,
///    then let `axum` close the listener.
///
/// The readiness flip and DirectoryService deregistration are orchestrated by
/// the caller (`HostRuntime` OoP path) around this call so the full drain
/// sequence (DESIGN § 3.2) is honored.
///
/// # Errors
/// Returns an error if a listener cannot be bound or the server task fails.
pub(crate) async fn serve(
    router: Router,
    drain_guard: DrainGuard,
    probe_router_for_sidecar: Option<Router>,
    options: &OopServeOptions,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(options.listen_addr).await?;
    let bound = listener.local_addr().unwrap_or(options.listen_addr);
    tracing::info!(addr = %bound, "OoP HTTP server bound");

    // Optional sidecar probe listener.
    let sidecar = match (options.probe_bind_addr, probe_router_for_sidecar) {
        (Some(addr), Some(probe_router)) => {
            let probe_listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!(addr = %addr, "OoP probe sidecar bound");
            let shutdown = {
                let cancel = cancel.clone();
                async move { cancel.cancelled().await }
            };
            Some(tokio::spawn(async move {
                if let Err(e) = axum::serve(probe_listener, probe_router)
                    .with_graceful_shutdown(shutdown)
                    .await
                {
                    tracing::warn!(error = %e, "OoP probe sidecar server error");
                }
            }))
        }
        _ => None,
    };

    // Main server: graceful shutdown waits for cancellation, then drains.
    let drain_timeout = options.drain_timeout;
    let shutdown = {
        let cancel = cancel.clone();
        let guard = drain_guard.clone();
        async move {
            cancel.cancelled().await;
            tracing::info!("OoP HTTP server draining (graceful shutdown)");
            drain_in_flight(&guard, drain_timeout).await;
        }
    };

    let result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
    .map_err(anyhow::Error::from);

    if let Some(handle) = sidecar {
        handle.abort();
        let _ = handle.await;
    }

    result
}

/// Wait up to `timeout` for the in-flight counter to reach zero.
async fn drain_in_flight(guard: &DrainGuard, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let in_flight = guard.in_flight();
        if in_flight == 0 {
            tracing::info!("OoP drain complete: no in-flight requests");
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                in_flight,
                timeout_secs = timeout.as_secs(),
                "OoP drain timed out with in-flight requests remaining"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "oop_serve_tests.rs"]
mod tests;
