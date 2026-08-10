//! Out-of-process (OoP) binary for the file-parser business gear.
//!
//! Runs file-parser as a standalone service: it composes its own REST router
//! (host-less), serves it with framework probes (`/healthz`, `/readyz`), and
//! self-registers its REST endpoint + OpenAPI with the platform-host
//! DirectoryService. The platform-host api-gateway then discovers and proxies
//! to this instance.
//!
//! Configuration is loaded from:
//! 1. `--config` CLI argument (or `TOOLKIT_CONFIG_PATH`)
//! 2. `TOOLKIT_MODULE_CONFIG` env var (rendered config from the master host)
//!
//! The directory endpoint comes from `TOOLKIT_DIRECTORY_ENDPOINT` (set by the
//! master host / k8s). The gear config must include an `oop_http` section to
//! enable the HTTP-serving lifecycle.
//!
//! file-parser has no shared-service dependencies (per OoP-8 / #4110), so it is
//! the cleanest first business gear to run out-of-process.

// Linking the gear library registers it via `inventory` so the runtime can
// discover its capabilities. `authn-resolver` is expected to be linked as well
// in a real image build (embedded per-pod JWT validation); for a minimal
// file-parser image it is added as a dependency of the OoP binary crate.
use file_parser as _;

use clap::Parser;
use mimalloc::MiMalloc;
use toolkit::bootstrap::oop::{OopRunOptions, run_oop_with_options};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// OoP file-parser gear.
#[derive(Parser)]
#[command(name = "file-parser-oop")]
#[command(about = "Out-of-process runner for the file-parser gear")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    /// Print effective configuration and exit
    #[arg(long)]
    print_config: bool,

    /// Log verbosity level (-v debug, -vv trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let opts = OopRunOptions {
        gear_name: "file-parser".to_owned(),
        verbose: cli.verbose,
        config_path: cli.config,
        print_config: cli.print_config,
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        ..Default::default()
    };

    run_oop_with_options(opts).await
}
