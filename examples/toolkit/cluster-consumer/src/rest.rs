//! The reservation service's HTTP surface: a small, realistic set of routes over
//! the seat-reservation domain, each anonymous (cluster calls carry a
//! platform-plane token attached by the runtime, not a tenant context) and
//! exposed (reverse-proxied by the api-gateway edge).

use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::{Json, Router};
use toolkit::api::OpenApiRegistry;
use toolkit::api::operation_builder::OperationBuilder;
use toolkit_canonical_errors::CanonicalError;

use crate::domain::{InventorySummary, ReservationService, SeatView, served_by};

const API_TAG: &str = "Cluster Consumer";

// --- DTOs -----------------------------------------------------------------

/// Response for `GET /cluster-consumer/v1/ping` - a cluster-free liveness probe.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct PingResponse {
    /// Always `"pong"`.
    pub message: String,
    /// The serving process (proves the request reached this `OoP` pod).
    pub served_by: String,
}

/// Response for `GET /cluster-consumer/v1/status` - this pod's coordination role.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct StatusResponse {
    /// `"leader"`, `"follower"`, or `"idle"` (sweeper not started).
    pub role: String,
    /// Whether this pod currently holds the maintenance leadership (advisory).
    pub is_leader: bool,
    /// Whether the background sweeper task is alive on this pod.
    pub sweeper_running: bool,
    /// Expired holds reclaimed on this pod's most recent sweep pass.
    pub last_reclaimed: u64,
    /// Expired holds reclaimed by this pod across all its sweeps.
    pub total_reclaimed: u64,
    /// Total seats in the venue roster.
    pub seats_total: u64,
    /// The serving process.
    pub served_by: String,
}

/// Request body for `POST /cluster-consumer/v1/reservations`.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(request)]
pub struct ReserveRequest {
    /// The seat to hold, e.g. `A12` (rows A-C, seats 1-12).
    pub seat: String,
    /// An opaque holder identity (who is reserving).
    pub holder: String,
}

/// Request body for confirm/release - names the holder acting on the seat.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(request)]
pub struct HolderRequest {
    /// The holder that placed the reservation.
    pub holder: String,
}

/// The current state of one seat - returned by reserve, confirm, release, and read.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct SeatResponse {
    /// The seat id, e.g. `A12`.
    pub seat: String,
    /// `"available"`, `"held"`, or `"booked"`.
    pub status: String,
    /// The holder, when held or booked.
    pub holder: Option<String>,
    /// Hold expiry (epoch millis), when held.
    pub expires_at_ms: Option<u64>,
    /// The backing cache version; absent for a seat never written.
    pub version: Option<u64>,
    /// The serving process.
    pub served_by: String,
}

/// Response for `GET /cluster-consumer/v1/inventory` - venue-wide availability.
#[derive(Debug, Clone)]
#[toolkit_macros::api_dto(response)]
pub struct InventoryResponse {
    /// Total seats in the roster.
    pub total: u64,
    /// Seats currently free.
    pub available: u64,
    /// Seats under an unexpired hold.
    pub held: u64,
    /// Seats confirmed (booked).
    pub booked: u64,
    /// Up to 16 free seat ids, in roster order.
    pub available_seats: Vec<String>,
    /// The serving process.
    pub served_by: String,
}

// --- mapping helpers ------------------------------------------------------

impl From<SeatView> for SeatResponse {
    fn from(v: SeatView) -> Self {
        SeatResponse {
            seat: v.seat,
            status: v.status.to_owned(),
            holder: v.holder,
            expires_at_ms: v.expires_at_ms,
            version: v.version,
            served_by: served_by(),
        }
    }
}

impl From<InventorySummary> for InventoryResponse {
    fn from(s: InventorySummary) -> Self {
        InventoryResponse {
            total: s.total as u64,
            available: s.available as u64,
            held: s.held as u64,
            booked: s.booked as u64,
            available_seats: s.available_seats,
            served_by: served_by(),
        }
    }
}

// --- route registration ---------------------------------------------------

/// Register every reservation route on `router`.
pub fn register_routes(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    let router = register_ping(router, openapi);
    let router = register_status(router, openapi, Arc::clone(&service));
    let router = register_reserve(router, openapi, Arc::clone(&service));
    let router = register_confirm(router, openapi, Arc::clone(&service));
    let router = register_release(router, openapi, Arc::clone(&service));
    let router = register_get_seat(router, openapi, Arc::clone(&service));
    register_inventory(router, openapi, service)
}

/// `GET /ping` - cluster-free liveness (fast, no coordination call).
fn register_ping(router: Router, openapi: &dyn OpenApiRegistry) -> Router {
    OperationBuilder::get("/cluster-consumer/v1/ping")
        .operation_id("cluster_consumer.ping")
        .summary("Liveness ping (no cluster call)")
        .description("Returns `pong` and the serving process id. Touches no cluster plane.")
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .handler(|| async {
            Ok::<_, CanonicalError>(Json(PingResponse {
                message: "pong".to_owned(),
                served_by: served_by(),
            }))
        })
        .json_response_with_schema::<PingResponse>(openapi, StatusCode::OK, "Pong response")
        .register(router, openapi)
}

/// `GET /status` - this pod's leader/sweeper role (cluster-free: reads local state).
fn register_status(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    OperationBuilder::get("/cluster-consumer/v1/status")
        .operation_id("cluster_consumer.status")
        .summary("This pod's coordination role")
        .description(
            "Reports whether this pod is the elected maintenance leader, whether its \
             background sweeper is running, and how many expired holds it has \
             reclaimed. Reads local state only - no cluster call.",
        )
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .handler(move || {
            let service = Arc::clone(&service);
            async move {
                let node = service.node();
                Ok::<_, CanonicalError>(Json(StatusResponse {
                    role: node.role().to_owned(),
                    is_leader: node.is_leader(),
                    sweeper_running: node.sweeper_running(),
                    last_reclaimed: node.last_reclaimed(),
                    total_reclaimed: node.total_reclaimed(),
                    seats_total: crate::model::roster_size() as u64,
                    served_by: served_by(),
                }))
            }
        })
        .json_response_with_schema::<StatusResponse>(openapi, StatusCode::OK, "Node status")
        .register(router, openapi)
}

/// `POST /reservations` - place a hold on a seat (CAS, retry on conflict).
fn register_reserve(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    OperationBuilder::post("/cluster-consumer/v1/reservations")
        .operation_id("cluster_consumer.reservation.reserve")
        .summary("Reserve (hold) a seat")
        .description(
            "Places a soft, expiring hold on a seat. Damped by the seat's advisory \
             distributed lock and committed by a versioned compare-and-swap, so two \
             pods cannot both hold the same seat. 409 if already taken or too \
             contended; 503 if the cluster is unreachable.",
        )
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .json_request::<ReserveRequest>(openapi, "Seat and holder")
        .handler(move |Json(req): Json<ReserveRequest>| {
            let service = Arc::clone(&service);
            async move {
                let view = service.reserve(&req.seat, &req.holder).await?;
                Ok::<_, CanonicalError>(Json(SeatResponse::from(view)))
            }
        })
        .json_response_with_schema::<SeatResponse>(openapi, StatusCode::OK, "The held seat")
        .standard_errors(openapi)
        .register(router, openapi)
}

/// `POST /reservations/{seat}/confirm` - book a held seat.
fn register_confirm(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    OperationBuilder::post("/cluster-consumer/v1/reservations/{seat}/confirm")
        .operation_id("cluster_consumer.reservation.confirm")
        .summary("Confirm (book) a held seat")
        .description("Transitions a seat you hold from `held` to `booked` via compare-and-swap.")
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .path_param("seat", "Seat id, e.g. A12")
        .json_request::<HolderRequest>(openapi, "The holder confirming")
        .handler(
            move |Path(seat): Path<String>, Json(req): Json<HolderRequest>| {
                let service = Arc::clone(&service);
                async move {
                    let view = service.confirm(&seat, &req.holder).await?;
                    Ok::<_, CanonicalError>(Json(SeatResponse::from(view)))
                }
            },
        )
        .json_response_with_schema::<SeatResponse>(openapi, StatusCode::OK, "The booked seat")
        .standard_errors(openapi)
        .register(router, openapi)
}

/// `DELETE /reservations/{seat}` - release a hold/booking back to available.
fn register_release(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    OperationBuilder::delete("/cluster-consumer/v1/reservations/{seat}")
        .operation_id("cluster_consumer.reservation.release")
        .summary("Release a held or booked seat")
        .description("Frees a seat you hold or booked, back to `available`, via compare-and-swap.")
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .path_param("seat", "Seat id, e.g. A12")
        .json_request::<HolderRequest>(openapi, "The holder releasing")
        .handler(
            move |Path(seat): Path<String>, Json(req): Json<HolderRequest>| {
                let service = Arc::clone(&service);
                async move {
                    let view = service.release(&seat, &req.holder).await?;
                    Ok::<_, CanonicalError>(Json(SeatResponse::from(view)))
                }
            },
        )
        .json_response_with_schema::<SeatResponse>(openapi, StatusCode::OK, "The freed seat")
        .standard_errors(openapi)
        .register(router, openapi)
}

/// `GET /reservations/{seat}` - read one seat's current state.
fn register_get_seat(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    OperationBuilder::get("/cluster-consumer/v1/reservations/{seat}")
        .operation_id("cluster_consumer.reservation.get")
        .summary("Read a seat's state")
        .description("Returns the seat's current available/held/booked state from the cache.")
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .path_param("seat", "Seat id, e.g. A12")
        .handler(move |Path(seat): Path<String>| {
            let service = Arc::clone(&service);
            async move {
                let view = service.get_seat(&seat).await?;
                Ok::<_, CanonicalError>(Json(SeatResponse::from(view)))
            }
        })
        .json_response_with_schema::<SeatResponse>(openapi, StatusCode::OK, "The seat state")
        .standard_errors(openapi)
        .register(router, openapi)
}

/// `GET /inventory` - venue-wide availability summary.
fn register_inventory(
    router: Router,
    openapi: &dyn OpenApiRegistry,
    service: Arc<ReservationService>,
) -> Router {
    OperationBuilder::get("/cluster-consumer/v1/inventory")
        .operation_id("cluster_consumer.inventory")
        .summary("Venue-wide seat availability")
        .description(
            "Scans every seat record and reports counts of available/held/booked \
             plus a sample of free seat ids. 503 if the cluster is unreachable.",
        )
        .tag(API_TAG)
        .exposed()
        .anonymous()
        .handler(move || {
            let service = Arc::clone(&service);
            async move {
                let summary = service.inventory().await?;
                Ok::<_, CanonicalError>(Json(InventoryResponse::from(summary)))
            }
        })
        .json_response_with_schema::<InventoryResponse>(openapi, StatusCode::OK, "Availability")
        .standard_errors(openapi)
        .register(router, openapi)
}
