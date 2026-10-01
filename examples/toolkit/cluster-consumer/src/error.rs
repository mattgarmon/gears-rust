//! Domain errors for the reservation service and their mapping to the platform
//! [`CanonicalError`] (which the REST layer renders as an RFC-9457 `Problem`).
//!
//! The consumer follows the idiomatic gear pattern: a plain domain-error enum
//! that the service returns, plus one `#[resource_error]` type bound to this
//! domain's GTS id whose generated builders (`not_found`, `already_exists`,
//! `aborted`, …) carry the right canonical category — and therefore the right
//! HTTP status — with no hand-mapped status codes.

use cluster_sdk::ClusterError;
use toolkit_canonical_errors::{CanonicalError, resource_error};

/// Why a reservation operation could not complete.
#[derive(Debug)]
pub enum ReservationError {
    /// The seat id is not one this venue has → 404.
    UnknownSeat {
        /// The offending seat id, echoed back as the resource.
        seat: String,
    },
    /// The caller supplied blank input (empty holder, etc.) → 400.
    InvalidInput {
        /// Human-readable constraint that was violated.
        detail: String,
    },
    /// The seat is currently held or booked by a different holder → 409.
    SeatTaken {
        /// The contested seat.
        seat: String,
        /// The current holder.
        by: String,
        /// `"held"` or `"booked"`.
        state: &'static str,
    },
    /// A confirm/release naming a holder that does not currently hold the seat,
    /// or a hold that has already lapsed → precondition failed.
    NotHeldByYou {
        /// The seat in question.
        seat: String,
        /// The holder the caller claimed to be.
        holder: String,
    },
    /// Optimistic concurrency lost repeatedly, or the advisory lock was contended:
    /// a peer is mutating the same seat right now → 409, safe for the client to
    /// retry.
    Contended {
        /// The seat under contention.
        seat: String,
    },
    /// The coordination plane could not be reached → 503.
    ClusterUnavailable(ClusterError),
    /// A cache value did not decode as a seat record → 500.
    Corrupt {
        /// The seat whose record was unreadable.
        seat: String,
        /// The decode failure.
        detail: String,
    },
}

impl ReservationError {
    /// Convenience: wrap a cluster transport/resolve failure.
    #[must_use]
    pub fn unavailable(err: ClusterError) -> Self {
        ReservationError::ClusterUnavailable(err)
    }
}

// A typed resource error bound to this domain's GTS id. The macro generates the
// `not_found()/already_exists()/aborted()/…` constructors used below, each
// pre-tagged with the canonical category the framework maps to an HTTP status.
#[resource_error(gts_id!("cf.cluster_consumer.reservation.seat.v1~"))]
pub struct SeatError;

impl From<ReservationError> for CanonicalError {
    fn from(err: ReservationError) -> Self {
        match err {
            ReservationError::UnknownSeat { seat } => {
                SeatError::not_found("no such seat in this venue")
                    .with_resource(seat)
                    .create()
            }

            ReservationError::InvalidInput { detail } => SeatError::invalid_argument()
                .with_constraint(detail)
                .create(),

            ReservationError::SeatTaken { seat, by, state } => SeatError::already_exists(format!(
                "seat is already {state} by another holder ({by})"
            ))
            .with_resource(seat)
            .create(),

            ReservationError::NotHeldByYou { seat, holder } => SeatError::failed_precondition()
                .with_precondition_violation(
                    seat,
                    format!("seat is not currently held by {holder}"),
                    "SEAT_NOT_HELD_BY_CALLER",
                )
                .create(),

            ReservationError::Contended { seat } => {
                SeatError::aborted("the seat is being modified concurrently; retry the request")
                    .with_resource(seat)
                    .with_reason("SEAT_CONTENDED")
                    .create()
            }

            ReservationError::ClusterUnavailable(e) => CanonicalError::service_unavailable()
                // The cluster error renders its own message (an endpoint, provider
                // name, or typed kind) with no caller data, so it is safe in the
                // public `Problem` detail per the `with_detail` contract.
                .with_detail(format!("cluster coordination call failed: {e}"))
                .create(),

            ReservationError::Corrupt { seat, detail } => {
                tracing::error!(seat, error = %detail, "seat record did not decode");
                CanonicalError::internal(format!("seat {seat} record is corrupt")).create()
            }
        }
    }
}
