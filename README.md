# Constructor Fabric Gears (Rust)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/constructorfabric/gears-rust/badge)](https://scorecard.dev/viewer/?uri=github.com/constructorfabric/gears-rust) [![OpenSSF Best Practices](https://www.bestpractices.dev/projects/12050/badge)](https://www.bestpractices.dev/projects/12050)

<p align="center">
  <img src="docs/img/gears.png" style='max-width: 800px' alt="Constructor Fabric Gears diagram">
</p>

**Gears** is a secure, modular XaaS framework and middleware, developed in Rust by the [Constructor Fabric Foundation](https://www.constructorfabric.org). It provides composable building blocks, domain components, and APIs with defense-in-depth security, multi-tenancy, and fine-grained access control built into every layer.

Gears is not a ready-to-use service. Instead, it is a set of well-integrated libraries that XaaS vendors can compose into their own products. Vendors decide which gears to include, how to combine them into services, and where to run them—from edge devices to Kubernetes clusters.

Gears span three broad categories:
- **Core** gears for platform foundations such as API gateway, authentication/authorization, account management, etc;
- **Serverless** gears for functions, workflows, and event-driven execution;
- **GenAI** gears for chat, retrieval, prompt orchestration, and related AI capabilities.

See also:
- [WHY_GEARS](docs/WHY_GEARS.md) explaining why to chose Rust/Gears for your XaaS project.
- [OVERVIEW](docs/slides/1_OVERVIEW.md) HTML slides explaining the key Constructor Fabric Gears concepts.
- [GEARS](docs/GEARS.md) for gears overview.

**Five defining Gears characteristics:**

1. **Secure XaaS framework with defense-in-depth** — Every API handler enforces authentication, authorization, tenant isolation, and scoped DB access by default. Security is structural, not opt-in, validated at build time using integrated dynamic lints.

2. **Three-tier gear hierarchy** — *Gears Toolkit* (`libs/` — ToolKit, DB access, error model, API middleware), *System gears* (`gears/system/` — API gateway, authn/authz, tenancy, event system, resource groups, type registry), and *Service gears* (`gears/` — serverless runtime, GenAI subsystems, and domain-specific libraries).

3. **Composable libraries, vendor-controlled deployment** — Each gear owns its API surface and database, communicates via a Rust-native SDK that facades local vs. remote calls, and is fully infrastructure-agnostic. Vendors choose which gears to bundle and whether to deploy single-process (edge/on-prem), multi-node (bare metal), or on Kubernetes.

4. **Pre-integrated XaaS backbone** — Deep integration with multi-tenancy, licensing and quota management, usage collection, and event systems. Gears provides its own backbone capabilities, but each can be replaced or integrated with existing vendor infrastructure via plugins (e.g. subscription management, product catalog, provisioning, or license enforcement).

5. **Extensible domain model via Global Type System** — Gears expose extensible domain objects whose metadata and types are customizable through [GTS](https://github.com/globaltypesystem/gts-spec) — define new event types, user settings, LLM model attributes, etc. CRUD API handlers support customization via hooks and callbacks as serverless functions and workflows.

**Engineering principles:**
- **Spec-Driven Development**: [Specification templates](docs/spec-templates/README.md) (PRD, Design, ADR, Feature) define what gets built *before* code is written. Every gear is well documented.
- **Shift Left**: Custom [dylint](tools/dylint_lints/) architectural lints enforce design rules at compile time, alongside Clippy, [tests](#testing), fuzzing, and security audits in CI
- **Quality First**: 90%+ test coverage target with unit, integration, E2E, performance, and security testing
- **Core in Rust**: Compile-time safety, deep static analysis including project-specific lints, so more issues are prevented before review/runtime
- **Monorepo**: All the core gears and contracts in one place for atomic refactors, consistent tooling/CI, and realistic local build + E2E testing

See the full architecture [MANIFEST](docs/ARCHITECTURE_MANIFEST.md) for more details, including rationales behind Rust and Monorepo choice.

See also [REPO_PLAYBOOK](docs/REPO_PLAYBOOK.md) with the registry of repository-wide artifacts (guidelines, rules, conventions, etc).

## Quick Start

### Prerequisites

- Rust stable with Cargo ([Install via rustup](https://rustup.rs/))
- Protocol Buffers compiler (`protoc`):
  - macOS: `brew install protobuf`
  - Linux: `apt-get install protobuf-compiler`
  - Windows: Download from https://github.com/protocolbuffers/protobuf/releases
- MariaDB/PostgreSQL/SQLite or in-memory database

### CI/Development Commands

```bash
# Clone the repository
git clone --recurse-submodules <repository-url>
cd gears-rust

make build      # Build libraries and example server binary
make test       # Run tests
make example    # Run toolkit example gear
```

### Running the Server

The Gears repository comes with an example server illustrating the gears APIs:

```bash
# Run an example server, see the API docs @ http://127.0.0.1:8087/cf/docs
make exammple

# See API documentation:
# $ make example
# visit: http://127.0.0.1:8087/cf/docs

# Check if server is ready (detailed JSON response)
curl http://127.0.0.1:8087/cf/health

# Kubernetes-style liveness probe (simple "ok" response)
curl http://127.0.0.1:8087/healthz
```

Other quick start examples:

```bash
# Option 1: Run with SQLite database (recommended for development)
cargo run --bin cf-gears-example-server -- --config config/quickstart.yaml run

# Option 2: Run without database (no-db mode)
cargo run --bin cf-gears-example-server -- --config config/no-db.yaml run

# Option 3: Run with mock in-memory database for testing
cargo run --bin cf-gears-example-server -- --config config/quickstart.yaml --mock run
```

### Example Configuration (config/quickstart.yaml)

```yaml
# Constructor Fabric Gears Configuration

# Core server configuration (global section)
server:
  home_dir: "~/.cfgears

# Database configuration (global section)
database:
  url: "sqlite://database/database.db"
  max_conns: 10
  busy_timeout_ms: 5000

# Logging configuration (global section)
logging:
  default:
    console_level: info
    file: "logs/cfgears.log"
    file_level: warn
    max_age_days: 28
    max_backups: 3
    max_size_mb: 1000

# Per-gear configurations moved under gears section
gears:
  api_gateway:
    bind_addr: "127.0.0.1:8087"
    enable_docs: true
    cors_enabled: false
```

### Creating Your First Gear

See [TOOLKIT UNIFIED SYSTEM](docs/toolkit_unified_system/README.md) and [TOOLKIT_PLUGINS.md](docs/TOOLKIT_PLUGINS.md) for details.

## Documentation

- **[Architecture manifest](docs/ARCHITECTURE_MANIFEST.md)** - High-level overview of the architecture
- **[Gears](docs/GEARS.md)** - List of all gears and their roles
- **[TOOLKIT UNIFIED SYSTEM](docs/toolkit_unified_system/README.md) and [TOOLKIT_PLUGINS.md](docs/TOOLKIT_PLUGINS.md)** - how to add new gears.
- **[Contributing](CONTRIBUTING.md)** - Development workflow and coding standards

## Security

Gears apply defense-in-depth security across the entire development lifecycle — from Rust's compile-time safety guarantees and custom architectural lints, through compile-time tenant isolation and PDP/PEP authorization enforcement, to continuous fuzzing, dependency auditing, and automated security scanning in CI.

See **[Security Overview](docs/security/SECURITY.md)** for the full breakdown, including: Secure ORM with compile-time tenant scoping, authentication/authorization architecture (NIST SP 800-162 PDP/PEP model), 90+ Clippy deny-level rules, custom dylint architectural lints, cargo-deny advisory checks, ClusterFuzzLite continuous fuzzing, CodeQL/Scorecard/Snyk/Aikido scanners, and AI-powered PR review bots.

## FIPS 140-3 support

Gears build with `--features fips` route every TLS data-path cryptographic operation through a **FIPS 140-3 validated cryptographic module**, on a single `rustls 0.23` state machine with one of three pluggable backends:

| Target | Validated module | Backend |
|---|---|---|
| macOS (any arch) | Apple `corecrypto` User-Space Gear (per-OS-version CMVP cert) | `cf-gears-rustls-corecrypto-provider` over Security.framework + CommonCrypto |
| Windows (x86_64) | Microsoft Windows CNG (per-OS-version CMVP cert) | `rustls-cng-crypto`'s `fips_provider()` over `bcrypt.dll` + `ncrypt.dll` |

All branches share the same `rustls 0.23` state machine — only the `CryptoProvider` swaps per OS.

### What is enforced on the wire

Built with `--features fips`, the toolkit-http client offers **only** FIPS-Approved algorithms in its `ClientHello`:

| Category | Algorithms |
|---|---|
| TLS versions | TLS 1.2, TLS 1.3 (no TLS 1.0/1.1) |
| TLS 1.3 cipher suites | `TLS_AES_128_GCM_SHA256`, `TLS_AES_256_GCM_SHA384` |
| TLS 1.2 cipher suites | `ECDHE_{ECDSA,RSA}_WITH_AES_{128,256}_GCM_SHA{256,384}` (×4) |
| Key exchange | NIST P-256, P-384 ECDHE |
| Signatures (verify) | ECDSA P-256/P-384, RSA-PSS, RSA PKCS#1 v1.5 (SHA-256/384/512) |
| Hash / HMAC / HKDF | SHA-256, SHA-384 |
| TLS 1.2 Extended Master Secret (RFC 7627) | **required** (`require_ems = true`) per NIST SP 800-52 Rev. 2 §3.5 |

Explicitly **excluded**: ChaCha20-Poly1305, x25519, X25519MLKEM768 / post-quantum hybrids, ED25519, MD5, SHA-1.

### Build & runtime

```sh
cargo build -p cf-gears-example-server --features fips
```

This is *"uses FIPS-validated cryptography"* — Gears itself are not on the CMVP Validated Gears list; the validated modules belong to Apple, AWS Labs, and Microsoft.

For the full per-OS detail (algorithm scope, build prerequisites, verification gates, runtime OE-validation, dep-graph policy, what is and is not covered) see **[Security Overview §9](docs/security/SECURITY.md#9-cryptographic-stack--fips-140-3)**. Architecture, ecosystem constraints, alternatives we rejected, and per-OS rationale live in the **[FIPS PRD](docs/security/fips/PRD.md)** and the ADRs in [`docs/security/fips/adrs/`](docs/security/fips/adrs/).

### How to verify a build is FIPS-conformant

```sh
# Wire-level (offered ClientHello inspected by an external TLS server):
cargo run -p cf-gears-fips-probe --features fips -- --url https://www.howsmyssl.com/a/check

# Expected: given_cipher_suites contains only AES-GCM suites, given_named_groups
# contains only secp256r1/secp384r1, post_quantum_key_agreement: false.
# The probe heuristic prints `[OK] No ChaCha20 in ClientHello cipher_suites`.
```

```sh
# Linkage on macOS+fips — should be Apple frameworks only, no aws-lc-fips dylib:
otool -L target/debug/cf-gears-example-server | grep -E 'aws|crypto|ssl|ring'
# (Expected: only /System/Library/Frameworks/Security.framework)

# Runtime — corecrypto loaded:
vmmap <cf-gears-example-server-pid> | grep -E 'corecrypto|Security\.framework'
```

See [`examples/cf-gears-fips-probe/README.md`](examples/cf-gears-fips-probe/README.md) for the full four-layer verification chain (linkage, runtime, wire-level, cert-validation).

### Build prerequisites

- **macOS + fips**: Xcode Command Line Tools + Rust toolchain. No `cmake` / `perl` / `go` (those used to be required when aws-lc-fips was linked on macOS; the per-target shim eliminates them).
- **Linux + fips**: C toolchain + `cmake` + `perl` + `go` (required by `aws-lc-fips-sys` build script).
- **Windows + fips**: MSVC toolchain + Windows SDK only for native Windows builds. No `cmake` / `perl` / `go` — `rustls-cng-crypto` is pure FFI to system DLLs (`bcrypt.dll`, `ncrypt.dll`) with no `build.rs`. Cross-compiling from a Linux/macOS host with `--target x86_64-pc-windows-msvc` needs no extra tooling beyond the standard MSVC sysroot bundled by `rustup target add` and `cargo-xwin` / `lld-link` if linking the binary; the `make check-windows-fips` target only `cargo check`s, so even that is unnecessary.

### System FIPS-mode requirement (Windows)

The Windows CNG FIPS provider only enforces its FIPS-Approved algorithm subset when the operating system itself is in FIPS mode. Gears bootstrap **fails closed** when this is not the case: `toolkit::bootstrap::init_crypto_provider` returns `CryptoProviderError::SystemFipsModeNotEnabled` and the binary refuses to start.

Enable FIPS mode via Group Policy: *Computer Configuration → Windows Settings → Security Settings → Local Policies → Security Options → "System cryptography: Use FIPS compliant algorithms for encryption, hashing, and signing" → Enabled*. Or via the registry:

```powershell
reg add HKLM\System\CurrentControlSet\Control\Lsa\FipsAlgorithmPolicy /v Enabled /t REG_DWORD /d 1 /f
```

A reboot is required after either change. See <https://learn.microsoft.com/en-us/windows/security/threat-protection/fips-140-validation> for Microsoft's reference documentation on FIPS-mode posture.

### What this does NOT claim

- Gears itself is **not** on the CMVP Validated Gears list. CMVP-listed modules are Apple `corecrypto` (macOS), AWS-LC FIPS Provider (Linux), and Microsoft Windows CNG (Windows); Gears are *consumers*.
- Neither `rustls-cng-crypto` nor `cf-gears-rustls-corecrypto-provider` is itself CMVP-listed — both are thin wrappers over the CMVP-listed system module they consume (CNG and corecrypto respectively). The chain-of-trust comes from the underlying validated module, not the wrapper crate.
- The FIPS claim on macOS / Windows is valid only when the running OS version is covered by the Operational Environment of the current CMVP certificate for the system module (`corecrypto` / CNG). Verify per release against <https://csrc.nist.gov/projects/cryptographic-module-validation-program/validated-modules/search>.
- `rustls-cng-crypto` is a young, single-maintainer crate (first release 2024-12). We pin to `0.1.x` and re-evaluate against `rustls-symcrypt` per release; the choice is documented in [`docs/security/fips/adrs/0003-windows-fips-via-rustls-cng-crypto.md`](docs/security/fips/adrs/0003-windows-fips-via-rustls-cng-crypto.md).
- TLS protocol-level NIST recommendations (SP 800-52 Rev. 2) beyond EMS — minimum protocol version, certificate hygiene, etc. — are the deployment's responsibility.
- Server-side TLS termination (inbound HTTPS) is delegated to the reverse proxy and is not part of this FIPS scope.

### Architecture decisions

- [`docs/adrs/toolkit/0004-macos-fips-via-corecrypto-provider.md`](docs/adrs/toolkit/0004-macos-fips-via-corecrypto-provider.md) — why we built a custom rustls `CryptoProvider` over Apple corecrypto rather than using `native-tls` or declaring FIPS Linux-only.
- [`docs/adrs/toolkit/0005-fips-feature-target-conditional-shim.md`](docs/adrs/toolkit/0005-fips-feature-target-conditional-shim.md) — why a one-`fips`-feature design uses an empty shim crate to encode per-target activation.
- [`docs/security/fips/adrs/0003-windows-fips-via-rustls-cng-crypto.md`](docs/security/fips/adrs/0003-windows-fips-via-rustls-cng-crypto.md) — why Windows+FIPS routes through `rustls-cng-crypto` today rather than waiting for Microsoft's `rustls-symcrypt` to obtain CMVP validation.

## Configuration

### YAML Configuration Structure

```yaml
# config/server.yaml

# Global server configuration
server:
  home_dir: "~/.cfgears"

# Database configuration
database:
  servers:
    sqlite_users:
      params:
        WAL: "true"
        synchronous: "NORMAL"
        busy_timeout: "5000"
      pool:
        max_conns: 5
        acquire_timeout: "30s"

# Logging configuration
logging:
  default:
    console_level: info
    file: "logs/cf-gears.log"
    file_level: warn
    max_age_days: 28
    max_backups: 3
    max_size_mb: 1000

# Per-gear configuration
gears:
  api_gateway:
    config:
      bind_addr: "127.0.0.1:8087"
      enable_docs: true
      cors_enabled: true
  users_info:
    database:
      server: "sqlite_users"
      file: "users_info.db"
    config:
      default_page_size: 5
      max_page_size: 100
```

### Environment Variable Overrides

Configuration supports environment variable overrides with `CF_` prefix:

```bash
export CF_GEARS_DATABASE_URL="postgres://user:pass@localhost/db"
export CF_GEARS_API_GATEWAY_BIND_ADDR="0.0.0.0:8080"
export CF_GEARS_LOGGING_DEFAULT_CONSOLE_LEVEL="debug"
```

## Testing

```bash
make check           # full quality gate (fmt + clippy + test + security)
```

Other tests:

```bash
make test            # unit tests (workspace)
make test-sqlite     # integration tests (SQLite, no external DB required)
make e2e-local       # end-to-end tests (builds + starts server automatically)
make e2e-local E2E_TARGET=testing/e2e/gears/file_parser/  # targeted end-to-end scope
make e2e-docker      # end-to-end tests (builds + starts server in Docker)
make coverage-unit   # unit test code coverage
make fuzz            # fuzz smoke tests (30 s per target)
```

On **Windows** (no `make`), use the cross-platform CI script directly:

```bash
python tools/scripts/ci.py check          # full CI suite
python tools/scripts/ci.py e2e-local      # end-to-end tests
python tools/scripts/ci.py e2e-local -- testing/e2e/gears/file_parser/  # targeted end-to-end scope
python tools/scripts/ci.py fuzz --seconds 60  # fuzz smoke run
```

For the complete test strategy, coverage policy, CI pipeline details, and all
available commands see **[docs/TESTING.md](docs/TESTING.md)**.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed guidelines.

## License

This project is licensed under the Apache 2.0 License - see the [LICENSE](LICENSE) file for details.
