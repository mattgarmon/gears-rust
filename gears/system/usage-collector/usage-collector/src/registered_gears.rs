// Link the gears whose `#[toolkit::gear]` inventory registrations must be
// present in this OoP binary:
//   * the target gear (this crate's own library),
//   * a no-op storage-plugin backend (dev/demo only), and
//   * the tenant-plane authn stack (authn-resolver + static-authn-plugin +
//     types-registry). `types-registry` doubles as the runtime
//     `dyn TypesRegistryClient` usage-collector resolves for lazy storage-plugin
//     discovery (it is a hard dependency of this crate).
//
// Authorization is resolved over REST via `#[toolkit::consumes]`.
#![allow(unused_imports)]

// Target gear (this crate's library target).
use usage_collector as _;

// No-op storage-plugin backend (dev/demo).
use noop_usage_collector_plugin as _;

// Tenant-plane authn stack (embedded per pod).
use authn_resolver as _;
use static_authn_plugin as _;
use types_registry as _;
