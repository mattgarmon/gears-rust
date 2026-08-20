//! Standalone out-of-process (Profile 3) entrypoint for the
//! `simple-user-settings` gear.
//!
//! Built only with `--features oop_module` (see `Cargo.toml`). Runs the gear as
//! its own process/pod: connects to the platform host's `DirectoryService`,
//! registers its REST endpoint, authenticates tenant-plane requests locally
//! (embedded authn stack), serves its routes, and resolves `authz-resolver`
//! remotely for PEP checks.

mod registered_gears;

use clap::Parser;
use std::path::PathBuf;
use toolkit::bootstrap::oop::{OopRunOptions, run_oop_with_options};

/// `simple-user-settings` OoP gear.
#[derive(Parser)]
#[command(name = "simple-user-settings-oop")]
#[command(about = "simple-user-settings - authenticated + DB OoP gear (Profile 3)")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// Path to configuration file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Log verbosity level (-v debug, -vv trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let opts = OopRunOptions {
        gear_name: "simple-user-settings".to_owned(),
        config_path: cli.config,
        verbose: cli.verbose,
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        ..Default::default()
    };

    run_oop_with_options(opts).await
}
