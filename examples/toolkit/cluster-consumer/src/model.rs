//! The reservation domain model: the per-seat records the consumer stores in the
//! cluster cache, and the fixed seat roster the demo manages.
//!
//! A seat's whole lifecycle lives in **one cache key** (`seat-{id}`) as a JSON
//! [`SeatState`]. Every state transition is an optimistic
//! [`compare_and_swap`](cluster_sdk::ClusterCacheV1::compare_and_swap) against the
//! version the caller last read — the CAS is the correctness gate (ADR-002), so
//! two pods racing to hold the same seat cannot both win: the loser sees
//! [`ClusterError::CasConflict`](cluster_sdk::ClusterError::CasConflict) and retries.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Key prefix for every seat record under the (already `.scoped`) cache. Also the
/// prefix the leader's sweeper enumerates with
/// [`scan_prefix`](cluster_sdk::ClusterCacheV1::scan_prefix).
pub const SEAT_KEY_PREFIX: &str = "seat-";

/// The cache key for a seat id — `seat-A1`. The id is validated against the
/// roster ([`is_roster_seat`]) before it ever reaches here, so it is always a
/// roster seat (`A1`..`C12`).
#[must_use]
pub fn seat_key(seat_id: &str) -> String {
    format!("{SEAT_KEY_PREFIX}{seat_id}")
}

/// Recover the seat id from a `seat-{id}` cache key (as returned by `scan_prefix`).
#[must_use]
pub fn seat_id_from_key(key: &str) -> Option<&str> {
    key.strip_prefix(SEAT_KEY_PREFIX)
}

/// The rows this venue has. A real service would load its catalog from a store;
/// a fixed roster keeps the demo deterministic and lets the leader seed it
/// idempotently with [`put_if_absent`](cluster_sdk::ClusterCacheV1::put_if_absent).
pub const ROWS: &[char] = &['A', 'B', 'C'];
/// Seats per row (`1..=SEATS_PER_ROW`), so the roster runs `A1`..`C12`.
pub const SEATS_PER_ROW: u32 = 12;

/// Every seat id in the roster, in stable order (`A1`..`A8`, `B1`.., `C8`).
#[must_use]
pub fn roster() -> Vec<String> {
    ROWS.iter()
        .flat_map(|row| (1..=SEATS_PER_ROW).map(move |n| format!("{row}{n}")))
        .collect()
}

/// The number of seats in the roster.
#[must_use]
pub fn roster_size() -> usize {
    ROWS.len() * SEATS_PER_ROW as usize
}

/// Is `seat_id` a seat this venue actually has? Guards request input before it
/// becomes a cache key, so an unknown seat is a clean `404` rather than a stray
/// record written under an attacker-chosen key.
#[must_use]
pub fn is_roster_seat(seat_id: &str) -> bool {
    let mut chars = seat_id.chars();
    let Some(row) = chars.next() else {
        return false;
    };
    let rest = chars.as_str();
    let Ok(n) = rest.parse::<u32>() else {
        return false;
    };
    ROWS.contains(&row) && (1..=SEATS_PER_ROW).contains(&n)
}

/// Wall-clock now, in epoch milliseconds — the unit hold expiries are stamped in.
///
/// A hold's deadline is compared against this across pods, so it must be absolute
/// wall time, not a process-local `Instant`. Clocks between pods are assumed
/// loosely synchronized (the same assumption the lock/lease TTLs already make).
#[must_use]
pub fn now_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    // Saturating: a value past u64::MAX ms (year 584 million) is not a real clock.
    u64::try_from(millis).unwrap_or(u64::MAX)
}

/// The state of a single seat, stored as the seat key's cache value.
///
/// `Held` is a soft, expiring claim (the reservation step); `Booked` is the
/// confirmed, durable outcome. A `Held` past its `expires_at_ms` is logically
/// [`Available`](SeatState::Available) again — a racing reserver may take it, and
/// the leader's sweeper reclaims it proactively so `inventory` reads clean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SeatState {
    /// Free to reserve.
    Available,
    /// Claimed by `holder` until `expires_at_ms`; auto-lapses after that.
    Held {
        /// Opaque holder identity supplied by the reserving caller.
        holder: String,
        /// Wall-clock deadline (epoch millis) after which the hold is void.
        expires_at_ms: u64,
    },
    /// Confirmed by `holder`; durable until explicitly released.
    Booked {
        /// Opaque holder identity that confirmed the seat.
        holder: String,
    },
}

impl SeatState {
    /// A short, stable label for API responses and inventory summaries.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            SeatState::Available => "available",
            SeatState::Held { .. } => "held",
            SeatState::Booked { .. } => "booked",
        }
    }

    /// The holder, if the seat is claimed or booked.
    #[must_use]
    pub fn holder(&self) -> Option<&str> {
        match self {
            SeatState::Available => None,
            SeatState::Held { holder, .. } | SeatState::Booked { holder } => Some(holder),
        }
    }

    /// Is this a `Held` whose deadline has passed as of `now_ms`? Such a seat is
    /// treated as free by both the reserve path and the sweeper.
    #[must_use]
    pub fn is_expired_hold(&self, now_ms: u64) -> bool {
        matches!(self, SeatState::Held { expires_at_ms, .. } if *expires_at_ms <= now_ms)
    }

    /// Whether the seat can be freshly reserved as of `now_ms`: free, or a hold
    /// that has lapsed.
    #[must_use]
    pub fn is_reservable(&self, now_ms: u64) -> bool {
        matches!(self, SeatState::Available) || self.is_expired_hold(now_ms)
    }

    /// Encode to the bytes stored as the cache value. Serialization of this closed
    /// enum cannot fail; the impossible error falls back to a valid `Available`
    /// document rather than empty bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_else(|_| br#"{"status":"available"}"#.to_vec())
    }

    /// Decode a cache value written by [`encode`](Self::encode). A decode failure
    /// means the key holds something this version did not write.
    ///
    /// # Errors
    /// [`serde_json::Error`] if the bytes are not a valid `SeatState`.
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_the_full_grid_and_ids_validate() {
        let seats = roster();
        assert_eq!(seats.len(), roster_size());
        assert_eq!(seats.len(), ROWS.len() * SEATS_PER_ROW as usize);
        assert_eq!(seats.first().map(String::as_str), Some("A1"));
        assert_eq!(seats.last().map(String::as_str), Some("C12"));
        assert!(seats.iter().all(|s| is_roster_seat(s)));
        // The example seat used throughout the docs/tests is in the roster.
        assert!(is_roster_seat("A12"));
    }

    #[test]
    fn non_roster_ids_are_rejected() {
        for bad in ["", "A", "A0", "A13", "D1", "a1", "A1B", "AA", "1A", " A1"] {
            assert!(!is_roster_seat(bad), "{bad} must not be a roster seat");
        }
    }

    #[test]
    fn seat_key_round_trips() {
        let key = seat_key("B7");
        assert_eq!(key, "seat-B7");
        assert_eq!(seat_id_from_key(&key), Some("B7"));
        assert_eq!(seat_id_from_key("not-a-seat-key"), None);
    }

    #[test]
    fn each_state_round_trips_through_json() {
        let states = [
            SeatState::Available,
            SeatState::Held {
                holder: "alice".to_owned(),
                expires_at_ms: 1_700_000_000_000,
            },
            SeatState::Booked {
                holder: "bob".to_owned(),
            },
        ];
        for state in states {
            let decoded = SeatState::decode(&state.encode()).expect("round-trips");
            assert_eq!(decoded, state);
        }
    }

    #[test]
    fn expiry_and_reservability_track_the_clock() {
        let now = 1_000_000u64;
        let fresh = SeatState::Held {
            holder: "alice".to_owned(),
            expires_at_ms: now + 1,
        };
        let stale = SeatState::Held {
            holder: "alice".to_owned(),
            expires_at_ms: now - 1,
        };
        let booked = SeatState::Booked {
            holder: "alice".to_owned(),
        };

        assert!(!fresh.is_expired_hold(now));
        assert!(stale.is_expired_hold(now));
        assert!(!booked.is_expired_hold(now));
        assert!(!SeatState::Available.is_expired_hold(now));

        // Reservable iff free or a lapsed hold.
        assert!(SeatState::Available.is_reservable(now));
        assert!(stale.is_reservable(now));
        assert!(!fresh.is_reservable(now));
        assert!(!booked.is_reservable(now));
    }

    #[test]
    fn holder_and_label_reflect_state() {
        assert_eq!(SeatState::Available.holder(), None);
        assert_eq!(SeatState::Available.label(), "available");
        let held = SeatState::Held {
            holder: "carol".to_owned(),
            expires_at_ms: 1,
        };
        assert_eq!(held.holder(), Some("carol"));
        assert_eq!(held.label(), "held");
    }
}
