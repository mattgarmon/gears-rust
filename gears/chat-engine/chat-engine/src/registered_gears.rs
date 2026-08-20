// Link the gears whose `#[toolkit::gear]` inventory registrations must be
// present in this OoP binary. Only the target gear + the tenant-plane authn
// stack (authn-resolver + static-authn-plugin + types-registry); authorization
// is resolved over REST via `#[toolkit::consumes]`.
#![allow(unused_imports)]

// Target gear (this crate's library target).
use chat_engine as _;

// Tenant-plane authn stack (embedded per pod).
use authn_resolver as _;
use static_authn_plugin as _;
use types_registry as _;
