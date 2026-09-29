// Link the AuthZ chain into the OoP binary. A gear becomes active purely by
// being linked: the `#[toolkit::gear]` macro's `inventory::submit!` registration
// is collected at startup, so a `use <crate> as _;` here is enough — the runtime
// discovers and initializes each linked gear in-process.
//
// This embeds the dependent gears the PDP needs so the chain's
// anonymous-context calls stay in-process. resource-group is intentionally
// absent (it depends on authz-resolver — embedding it here would be a Cargo
// cycle); tenant hierarchy comes from static-tr. See the DESIGN "Flight Control
// Composition" section.
#![allow(unused_imports)]

// The PDP itself (this crate's library).
use authz_resolver as _;

// Dependent gear + wiring plugins, embedded in-process.
use static_tr_plugin as _;
use tenant_resolver as _;
use tr_authz_plugin as _;
