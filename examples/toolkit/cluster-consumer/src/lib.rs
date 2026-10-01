//! cluster-consumer — a Profile-3 gear that *consumes* the cluster gear to run a
//! small but realistic **seat-reservation service**.
//!
//! Every seat's lifecycle lives in one cluster-cache key and moves
//! `available → held → booked → available` through optimistic
//! [`compare_and_swap`](cluster_sdk::ClusterCacheV1::compare_and_swap) — the
//! correctness gate (ADR-002). Each of the three cluster primitives earns its
//! place:
//!
//! - **Cache (with CAS)** — [`ClusterCacheV1`](cluster_sdk::ClusterCacheV1) holds
//!   the seat records; every transition is a versioned CAS with a bounded retry
//!   loop, so two pods racing for the same seat cannot both win.
//! - **Distributed lock** — [`DistributedLockV1`](cluster_sdk::DistributedLockV1)
//!   gives each seat an advisory `try_lock` that damps CAS contention on hot
//!   seats; it holds no remote I/O and is *not* the correctness gate (ADR-002).
//! - **Leader election** — [`LeaderElectionV1`](cluster_sdk::LeaderElectionV1)
//!   elects exactly one pod to seed the roster and sweep expired holds in the
//!   background ([`sweeper`]); the rest observe as followers.
//!
//! Routes (all anonymous + exposed):
//!
//! ```text
//! GET    /cluster-consumer/v1/ping                       liveness (no cluster call)
//! GET    /cluster-consumer/v1/status                     this pod's leader/sweeper role
//! POST   /cluster-consumer/v1/reservations               hold a seat        { seat, holder }
//! POST   /cluster-consumer/v1/reservations/{seat}/confirm book a held seat  { holder }
//! DELETE /cluster-consumer/v1/reservations/{seat}        release a seat      { holder }
//! GET    /cluster-consumer/v1/reservations/{seat}        read a seat's state
//! GET    /cluster-consumer/v1/inventory                  venue-wide availability
//! ```
//!
//! Cluster's data plane is gRPC, so the consumer→cluster hop travels over gRPC to
//! the cluster pod, discovered by Kubernetes DNS convention
//! (`cluster.{POD_NAMESPACE}.svc.cluster.local:50051`, see `cluster_sdk::wiring`).
//! The gear links no cluster code (no `deps = [cluster]`): the framework's
//! proxy-wiring phase registers a remote `dyn ClusterClient` before this gear's
//! routes ever run.
//!
//! On plain loopback the DNS name does not resolve, so coordination calls return
//! a typed `Provider{ConnectionLost}` surfaced as a 503, and the sweeper simply
//! campaigns-and-backs-off. That is the seam proof (the gRPC path was exercised
//! to the socket); the full flow runs in Kubernetes, where the DNS convention
//! holds. `/ping` and `/status` are cluster-free and always answer.

mod domain;
mod error;
mod gear;
mod model;
mod rest;
mod sweeper;

pub use gear::{ClusterConsumer, DemoProfile};
