//! Standalone `OoP` entrypoint for the authz-resolver — the policy decision
//! point (PDP).
//!
//! Runs the PDP as its own process, embedding the dependent gears it needs to
//! resolve authorization in-process: `tenant-resolver` plus the plugins that
//! wire the chain (`tr-authz` -> `tenant-resolver`, `static-tr` for
//! config-driven tenants). Keeping them in one process means the chain's
//! `SecurityContext::anonymous()` calls never cross a network boundary. It
//! connects to Flight Control's `DirectoryService` (via
//! `TOOLKIT_DIRECTORY_ENDPOINT`), registers its REST endpoint (from
//! `oop_http.advertise_uri`), serves `/authz-resolver/v1/evaluate` for
//! out-of-process PEPs, and deregisters on shutdown.
//!
//! Tenant hierarchy here is config-driven (`static-tr`). A
//! resource-group-backed hierarchy is a separate OoP unit — `resource-group`
//! depends on `authz-resolver`, so it cannot be embedded in this crate without
//! a Cargo dependency cycle. See `docs/arch/toolkit-oop/DESIGN.md` § Flight
//! Control Composition.

mod registered_gears;

use clap::Parser;
use mimalloc::MiMalloc;
use std::path::PathBuf;
use toolkit::bootstrap::oop::{OopRunOptions, run_oop_with_options};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// AuthZ resolver `OoP` unit (PDP + embedded tenant-resolver + tenant plugins).
#[derive(Parser)]
#[command(name = "authz-resolver-oop")]
#[command(about = "AuthZ resolver (PDP) OoP unit — embeds tenant-resolver + tenant plugins")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Log verbosity level (-v debug, -vv trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // `gear_name` is the served/registered endpoint (the PDP). The embedded
    // dependent gears are discovered from the linked inventory and initialized
    // in-process by the runtime; they are not advertised separately.
    let opts = OopRunOptions {
        gear_name: "authz-resolver".to_owned(),
        config_path: cli.config,
        verbose: cli.verbose,
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        ..Default::default()
    };

    run_oop_with_options(opts).await
}
