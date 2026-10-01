//! Standalone `OoP` entrypoint for the cluster-consumer gear.
//!
//! Runs the gear as its own process: it connects to the platform host's
//! `DirectoryService` (via `TOOLKIT_DIRECTORY_ENDPOINT`), registers its REST
//! endpoint (from `oop_http.advertise_uri`), serves `/cluster-consumer/v1/...`,
//! and deregisters on shutdown. The cluster dependency is resolved over gRPC by
//! the framework's proxy-wiring phase — this binary links no cluster code.

mod registered_gears;

use clap::Parser;
use mimalloc::MiMalloc;
use std::path::PathBuf;
use toolkit::bootstrap::oop::{OopRunOptions, run_oop_with_options};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// cluster-consumer `OoP` gear.
#[derive(Parser)]
#[command(name = "cluster-consumer-oop")]
#[command(about = "Cluster-consuming demo gear (Profile 3)")]
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

    let opts = OopRunOptions {
        gear_name: "cluster-consumer".to_owned(),
        config_path: cli.config,
        verbose: cli.verbose,
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        ..Default::default()
    };

    run_oop_with_options(opts).await
}
