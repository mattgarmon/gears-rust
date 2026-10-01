//! The reservation service: a realistic consumer of all three cluster primitives.
//!
//! Each seat's lifecycle is one cache key mutated by optimistic
//! [`compare_and_swap`](ClusterCacheV1::compare_and_swap); a per-seat advisory
//! [`try_lock`](DistributedLockV1::try_lock) damps contention on hot seats but is
//! **not** the correctness gate — the CAS is (ADR-002). Leader-only maintenance
//! (roster seeding + expiry sweeping) lives in [`crate::sweeper`] and calls the
//! [`seed_roster`](ReservationService::seed_roster) /
//! [`sweep_expired`](ReservationService::sweep_expired) methods here.
//!
//! Each facade is resolved from the [`ClientHub`] **once, lazily** on first use
//! and then memoized in a [`OnceCell`](tokio::sync::OnceCell): resolution redoes
//! in-process bookkeeping (the requirement registry recording, name/scope
//! validation, `Arc` allocation) on every call, and the descriptor it awaits is
//! cached on the client after the first fetch — so re-resolving per request is
//! pure churn on a hot path. The facade is a cheap cloneable `Arc` handle built
//! for exactly this. Memoizing does not couple the service to a deployment mode:
//! Profile 1 (co-located) vs Profile 3 (remote gRPC proxy) is decided at *client
//! registration*, not per resolve, so the cached handle is as mode-oblivious as a
//! freshly resolved one. Only a successful bind is cached (`get_or_try_init`), so
//! a transient warmup failure retries rather than pinning a broken facade.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use cluster_sdk::cache::{CacheEntry, PutRequest, Ttl};
use cluster_sdk::{ClusterCacheV1, ClusterError, DistributedLockV1, LeaderElectionV1, LockGuard};
use tokio::sync::OnceCell;
use toolkit::ClientHub;

use crate::error::ReservationError;
use crate::gear::DemoProfile;
use crate::model::{
    self, SeatState, is_roster_seat, now_ms, roster, roster_size, seat_id_from_key, seat_key,
};
use crate::sweeper::NodeState;

/// The keyspace this gear carves out of the `demo` profile, so its
/// keys/locks/elections never collide with another consumer's (DESIGN §3.8). All
/// three primitives are `.scoped(SCOPE)`, so wire names are `cluster-consumer/…`.
pub const SCOPE: &str = "cluster-consumer";

/// TTL of the per-seat advisory lock. Short: it only spans the fast, local
/// bookkeeping between `try_lock` and `release` (no remote I/O — ADR-002), so a
/// crashed holder cannot block a seat for long.
const LOCK_TTL: Duration = Duration::from_secs(5);
/// How long a `Held` claim survives before it lapses back to `Available` (whether
/// or not the leader's sweeper has reclaimed it yet).
const HOLD_TTL: Duration = Duration::from_secs(30);
/// Bound on the optimistic-concurrency retry loop before a caller is told the
/// seat is too hot right now (`Contended` → 409, client may retry).
const MAX_CAS_ATTEMPTS: u32 = 5;
/// Cap on how many free seat ids `inventory` echoes, so a large venue's response
/// stays small.
const MAX_LISTED_AVAILABLE: usize = 16;

/// A seat's current state as returned to the API layer.
pub struct SeatView {
    /// The seat id, e.g. `A12`.
    pub seat: String,
    /// `"available"`, `"held"`, or `"booked"`.
    pub status: &'static str,
    /// The holder, when held or booked.
    pub holder: Option<String>,
    /// Hold expiry (epoch millis), when held.
    pub expires_at_ms: Option<u64>,
    /// The cache version backing this state; `None` for a roster seat never
    /// written (logically `Available`).
    pub version: Option<u64>,
}

impl SeatView {
    fn from_state(seat: String, state: &SeatState, version: u64) -> Self {
        let (holder, expires) = match state {
            SeatState::Available => (None, None),
            SeatState::Held {
                holder,
                expires_at_ms,
            } => (Some(holder.clone()), Some(*expires_at_ms)),
            SeatState::Booked { holder } => (Some(holder.clone()), None),
        };
        SeatView {
            seat,
            status: state.label(),
            holder,
            expires_at_ms: expires,
            version: Some(version),
        }
    }

    /// A synthetic `Available` view for a roster seat that has never been written.
    fn available(seat: String) -> Self {
        SeatView {
            seat,
            status: "available",
            holder: None,
            expires_at_ms: None,
            version: None,
        }
    }
}

/// Venue-wide availability, from a `scan_prefix` over the seat records.
pub struct InventorySummary {
    /// Total seats in the roster.
    pub total: usize,
    /// Seats currently free (roster minus held/booked).
    pub available: usize,
    /// Seats under an unexpired hold.
    pub held: usize,
    /// Seats confirmed (booked).
    pub booked: usize,
    /// Up to [`MAX_LISTED_AVAILABLE`] free seat ids, in roster order.
    pub available_seats: Vec<String>,
}

/// Resolves the cluster facades once (memoized) and runs the reservation domain
/// logic. Each `OnceCell` holds the already-[`scoped`](ClusterCacheV1::scoped)
/// facade for its primitive, populated on first use.
pub struct ReservationService {
    hub: Arc<ClientHub>,
    node: Arc<NodeState>,
    cache: OnceCell<ClusterCacheV1>,
    locks: OnceCell<DistributedLockV1>,
    leader: OnceCell<LeaderElectionV1>,
}

impl ReservationService {
    /// Build over a `ClientHub` handle (typically `ctx.client_hub()`) and the
    /// shared node state the sweeper updates and `/status` reports. The facade
    /// cells start empty and fill on first resolve.
    #[must_use]
    pub fn new(hub: Arc<ClientHub>, node: Arc<NodeState>) -> Self {
        Self {
            hub,
            node,
            cache: OnceCell::new(),
            locks: OnceCell::new(),
            leader: OnceCell::new(),
        }
    }

    /// The shared leader/sweeper status this pod reports on `/status`.
    #[must_use]
    pub fn node(&self) -> &Arc<NodeState> {
        &self.node
    }

    // --- facade resolution (memoized, binding-mode-agnostic) ----------------
    //
    // Each helper resolves its scoped facade at most once via `get_or_try_init`
    // and returns a cheap `Arc`-handle clone thereafter. Only a successful bind is
    // stored, so a transient failure during warmup retries on the next call.

    async fn cache(&self) -> Result<ClusterCacheV1, ReservationError> {
        self.cache
            .get_or_try_init(|| async {
                ClusterCacheV1::resolver(&self.hub)
                    .profile(DemoProfile)
                    .resolve()
                    .await
                    .map_err(ReservationError::unavailable)?
                    .scoped(SCOPE)
                    .map_err(ReservationError::unavailable)
            })
            .await
            .cloned()
    }

    async fn locks(&self) -> Result<DistributedLockV1, ReservationError> {
        self.locks
            .get_or_try_init(|| async {
                DistributedLockV1::resolver(&self.hub)
                    .profile(DemoProfile)
                    .resolve()
                    .await
                    .map_err(ReservationError::unavailable)?
                    .scoped(SCOPE)
                    .map_err(ReservationError::unavailable)
            })
            .await
            .cloned()
    }

    /// The scoped leader-election facade — used by [`crate::sweeper`] to campaign
    /// for the singleton maintenance role. Resolved once, then memoized.
    ///
    /// # Errors
    /// [`ReservationError::ClusterUnavailable`] if the cluster cannot be resolved
    /// or reached.
    pub async fn leader(&self) -> Result<LeaderElectionV1, ReservationError> {
        self.leader
            .get_or_try_init(|| async {
                LeaderElectionV1::resolver(&self.hub)
                    .profile(DemoProfile)
                    .resolve()
                    .await
                    .map_err(ReservationError::unavailable)?
                    .scoped(SCOPE)
                    .map_err(ReservationError::unavailable)
            })
            .await
            .cloned()
    }

    // --- request-path operations --------------------------------------------

    /// Reserve `seat_id` for `holder`: place (or refresh) a soft, expiring hold.
    ///
    /// Acquires the seat's advisory lock and releases it immediately (a contention
    /// damper holding no remote I/O), then commits the `Available → Held`
    /// transition with a versioned CAS, retrying on a lost race. Re-reserving a
    /// seat you already hold is idempotent.
    ///
    /// # Errors
    /// - [`ReservationError::UnknownSeat`] / [`InvalidInput`](ReservationError::InvalidInput)
    ///   for bad input,
    /// - [`SeatTaken`](ReservationError::SeatTaken) if another holder has it,
    /// - [`Contended`](ReservationError::Contended) if the CAS lost too many times,
    /// - [`ClusterUnavailable`](ReservationError::ClusterUnavailable) if the plane
    ///   is unreachable.
    pub async fn reserve(&self, seat_id: &str, holder: &str) -> Result<SeatView, ReservationError> {
        let seat = normalize_seat(seat_id)?;
        let holder = validate_holder(holder)?;
        let key = seat_key(&seat);
        let cache = self.cache().await?;

        // Advisory coarse lock: reduce the odds of a CAS storm on a hot seat. Held
        // with NO remote I/O inside (ADR-002) — we take it and release it right
        // away; a concurrent holder means "being reserved right now", a clean
        // fast-fail rather than hammering the CAS below. The CAS is the gate.
        match self.locks().await?.try_lock(&key, LOCK_TTL).await {
            Ok(guard) => release_guard(guard).await?,
            Err(ClusterError::LockContended { .. }) => {
                return Err(ReservationError::Contended { seat });
            }
            Err(e) => return Err(ReservationError::unavailable(e)),
        }

        for _ in 0..MAX_CAS_ATTEMPTS {
            let now = now_ms();
            let held = SeatState::Held {
                holder: holder.clone(),
                expires_at_ms: now.saturating_add(millis(HOLD_TTL)),
            };

            let Some(entry) = self.get(&cache, &key).await? else {
                // Never written: try to create it held in one shot. A racer that
                // beat us makes `put_if_absent` return `None` → re-read and CAS.
                match cache
                    .put_if_absent(PutRequest {
                        key: &key,
                        value: &held.encode(),
                        ttl: Ttl::Indefinite,
                    })
                    .await
                    .map_err(ReservationError::unavailable)?
                {
                    Some(entry) => return Ok(SeatView::from_state(seat, &held, entry.version)),
                    None => continue,
                }
            };

            let state = decode(&entry.value, &seat)?;
            if let Some(current_holder) = state.holder() {
                if current_holder == holder && !state.is_expired_hold(now) {
                    // Idempotent: caller already holds/booked it.
                    return Ok(SeatView::from_state(seat, &state, entry.version));
                }
                if !state.is_reservable(now) {
                    return Err(ReservationError::SeatTaken {
                        seat,
                        by: current_holder.to_owned(),
                        state: state.label(),
                    });
                }
            }

            // Available, or a lapsed hold — take it under the read version.
            match cache
                .compare_and_swap(&key, entry.version, &held.encode(), Ttl::Indefinite)
                .await
            {
                Ok(new_entry) => return Ok(SeatView::from_state(seat, &held, new_entry.version)),
                Err(ClusterError::CasConflict { .. }) => {} // lost the race → retry
                Err(e) => return Err(ReservationError::unavailable(e)),
            }
        }
        Err(ReservationError::Contended { seat })
    }

    /// Confirm a held seat (`Held → Booked`) for `holder`. Idempotent if already
    /// booked by the same holder.
    ///
    /// # Errors
    /// [`NotHeldByYou`](ReservationError::NotHeldByYou) if the seat is not held by
    /// `holder` (or the hold lapsed); the usual contention/availability errors
    /// otherwise.
    pub async fn confirm(&self, seat_id: &str, holder: &str) -> Result<SeatView, ReservationError> {
        let seat = normalize_seat(seat_id)?;
        let holder = validate_holder(holder)?;
        let key = seat_key(&seat);
        let cache = self.cache().await?;

        for _ in 0..MAX_CAS_ATTEMPTS {
            let now = now_ms();
            let Some(entry) = self.get(&cache, &key).await? else {
                return Err(ReservationError::NotHeldByYou { seat, holder });
            };
            let state = decode(&entry.value, &seat)?;
            match &state {
                SeatState::Booked { holder: h } if *h == holder => {
                    return Ok(SeatView::from_state(seat, &state, entry.version)); // already confirmed
                }
                SeatState::Held {
                    holder: h,
                    expires_at_ms,
                } if *h == holder && *expires_at_ms > now => {
                    let booked = SeatState::Booked {
                        holder: holder.clone(),
                    };
                    match cache
                        .compare_and_swap(&key, entry.version, &booked.encode(), Ttl::Indefinite)
                        .await
                    {
                        Ok(new_entry) => {
                            return Ok(SeatView::from_state(seat, &booked, new_entry.version));
                        }
                        Err(ClusterError::CasConflict { .. }) => {} // retry
                        Err(e) => return Err(ReservationError::unavailable(e)),
                    }
                }
                _ => return Err(ReservationError::NotHeldByYou { seat, holder }),
            }
        }
        Err(ReservationError::Contended { seat })
    }

    /// Release a seat `holder` holds or booked, back to `Available`. Idempotent on
    /// an already-free seat.
    ///
    /// # Errors
    /// [`NotHeldByYou`](ReservationError::NotHeldByYou) if a different holder owns
    /// the seat; contention/availability errors otherwise.
    pub async fn release(&self, seat_id: &str, holder: &str) -> Result<SeatView, ReservationError> {
        let seat = normalize_seat(seat_id)?;
        let holder = validate_holder(holder)?;
        let key = seat_key(&seat);
        let cache = self.cache().await?;

        for _ in 0..MAX_CAS_ATTEMPTS {
            let Some(entry) = self.get(&cache, &key).await? else {
                return Ok(SeatView::available(seat)); // nothing to release
            };
            let state = decode(&entry.value, &seat)?;
            match state.holder() {
                None => return Ok(SeatView::from_state(seat, &state, entry.version)), // already free
                Some(h) if h == holder => {
                    let free = SeatState::Available;
                    match cache
                        .compare_and_swap(&key, entry.version, &free.encode(), Ttl::Indefinite)
                        .await
                    {
                        Ok(new_entry) => {
                            return Ok(SeatView::from_state(seat, &free, new_entry.version));
                        }
                        Err(ClusterError::CasConflict { .. }) => {} // retry
                        Err(e) => return Err(ReservationError::unavailable(e)),
                    }
                }
                Some(_) => return Err(ReservationError::NotHeldByYou { seat, holder }),
            }
        }
        Err(ReservationError::Contended { seat })
    }

    /// Read a single seat's current state. A roster seat never written reads back
    /// as a synthetic `Available`.
    ///
    /// # Errors
    /// [`UnknownSeat`](ReservationError::UnknownSeat) for a non-roster id;
    /// [`ClusterUnavailable`](ReservationError::ClusterUnavailable) if unreachable.
    pub async fn get_seat(&self, seat_id: &str) -> Result<SeatView, ReservationError> {
        let seat = normalize_seat(seat_id)?;
        let key = seat_key(&seat);
        let cache = self.cache().await?;
        match self.get(&cache, &key).await? {
            Some(entry) => {
                let state = decode(&entry.value, &seat)?;
                Ok(SeatView::from_state(seat, &state, entry.version))
            }
            None => Ok(SeatView::available(seat)),
        }
    }

    /// Venue-wide availability, from one `scan_prefix` plus a read per written seat.
    ///
    /// # Errors
    /// [`ClusterUnavailable`](ReservationError::ClusterUnavailable) if the cluster
    /// is unreachable.
    pub async fn inventory(&self) -> Result<InventorySummary, ReservationError> {
        let cache = self.cache().await?;
        let now = now_ms();
        let keys = cache
            .scan_prefix(model::SEAT_KEY_PREFIX)
            .await
            .map_err(ReservationError::unavailable)?;

        let mut held = 0usize;
        let mut booked = 0usize;
        let mut taken: HashSet<String> = HashSet::new();
        for key in keys {
            let Some(entry) = self.get(&cache, &key).await? else {
                continue;
            };
            // A record we can't decode is skipped rather than failing the whole
            // listing — it never counts against availability.
            let Ok(state) = SeatState::decode(&entry.value) else {
                continue;
            };
            let Some(seat) = seat_id_from_key(&key) else {
                continue;
            };
            match state {
                SeatState::Booked { .. } => {
                    booked += 1;
                    taken.insert(seat.to_owned());
                }
                SeatState::Held { .. } if !state.is_expired_hold(now) => {
                    held += 1;
                    taken.insert(seat.to_owned());
                }
                _ => {} // available or lapsed hold → still free
            }
        }

        let available_seats: Vec<String> = roster()
            .into_iter()
            .filter(|seat| !taken.contains(seat))
            .take(MAX_LISTED_AVAILABLE)
            .collect();

        Ok(InventorySummary {
            total: roster_size(),
            available: roster_size().saturating_sub(held + booked),
            held,
            booked,
            available_seats,
        })
    }

    // --- leader-only maintenance (called from the sweeper) ------------------

    /// Idempotently create every roster seat as `Available`. Run by the elected
    /// leader on taking office; `put_if_absent` makes re-seeding a no-op.
    ///
    /// # Errors
    /// [`ClusterUnavailable`](ReservationError::ClusterUnavailable) on a cluster
    /// failure.
    pub async fn seed_roster(&self) -> Result<(), ReservationError> {
        let cache = self.cache().await?;
        let available = SeatState::Available.encode();
        for seat in roster() {
            let key = seat_key(&seat);
            cache
                .put_if_absent(PutRequest {
                    key: &key,
                    value: &available,
                    ttl: Ttl::Indefinite,
                })
                .await
                .map_err(ReservationError::unavailable)?;
        }
        Ok(())
    }

    /// Reclaim every expired hold back to `Available`, returning how many were
    /// reclaimed. Run periodically by the elected leader only.
    ///
    /// A lost CAS is ignored (a successor already changed the seat); a transport
    /// error aborts the pass so the caller can back off.
    ///
    /// # Errors
    /// [`ClusterUnavailable`](ReservationError::ClusterUnavailable) if a cache
    /// call fails at the transport level.
    pub async fn sweep_expired(&self) -> Result<u64, ReservationError> {
        let cache = self.cache().await?;
        let now = now_ms();
        let keys = cache
            .scan_prefix(model::SEAT_KEY_PREFIX)
            .await
            .map_err(ReservationError::unavailable)?;

        let available = SeatState::Available.encode();
        let mut reclaimed = 0u64;
        for key in keys {
            let Some(entry) = self.get(&cache, &key).await? else {
                continue;
            };
            let Ok(state) = SeatState::decode(&entry.value) else {
                continue; // leave records this version can't read
            };
            if state.is_expired_hold(now) {
                match cache
                    .compare_and_swap(&key, entry.version, &available, Ttl::Indefinite)
                    .await
                {
                    Ok(_) => reclaimed += 1,
                    Err(ClusterError::CasConflict { .. }) => {} // a successor won; fine
                    Err(e) => return Err(ReservationError::unavailable(e)),
                }
            }
        }
        Ok(reclaimed)
    }

    /// One `get`, mapping a transport failure to the domain's unavailable error.
    async fn get(
        &self,
        cache: &ClusterCacheV1,
        key: &str,
    ) -> Result<Option<CacheEntry>, ReservationError> {
        cache.get(key).await.map_err(ReservationError::unavailable)
    }
}

/// The serving process — proof a request reached this `OoP` pod.
#[must_use]
pub fn served_by() -> String {
    format!("cluster-consumer-oop (pid {})", std::process::id())
}

/// Uppercase-normalize and validate a seat id against the roster.
fn normalize_seat(seat_id: &str) -> Result<String, ReservationError> {
    let seat = seat_id.trim().to_ascii_uppercase();
    if is_roster_seat(&seat) {
        Ok(seat)
    } else {
        Err(ReservationError::UnknownSeat { seat })
    }
}

/// Validate a holder identity: non-blank and bounded.
fn validate_holder(holder: &str) -> Result<String, ReservationError> {
    let holder = holder.trim();
    if holder.is_empty() {
        return Err(ReservationError::InvalidInput {
            detail: "holder must not be empty".to_owned(),
        });
    }
    if holder.len() > 64 {
        return Err(ReservationError::InvalidInput {
            detail: "holder must be at most 64 characters".to_owned(),
        });
    }
    Ok(holder.to_owned())
}

/// Decode a seat record, turning a bad value into a `Corrupt` domain error.
fn decode(bytes: &[u8], seat: &str) -> Result<SeatState, ReservationError> {
    SeatState::decode(bytes).map_err(|e| ReservationError::Corrupt {
        seat: seat.to_owned(),
        detail: e.to_string(),
    })
}

/// Release an advisory lock guard, mapping failure to the unavailable error.
async fn release_guard(guard: LockGuard) -> Result<(), ReservationError> {
    guard.release().await.map_err(ReservationError::unavailable)
}

/// `Duration` → whole milliseconds, saturating.
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
