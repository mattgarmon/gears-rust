# OoP-7 (Helm Charts) + OoP-10 (Docker Images) — Implementation Plan

Status: PLANNING
Issues: [#4109 OoP-7 K8s Packaging (Helm Charts)](https://github.com/constructorfabric/gears-rust/issues/4109),
[#4255 OoP-10 Gear & Platform-Host Docker Image Build & Publish Pipeline](https://github.com/constructorfabric/gears-rust/issues/4255)

Design refs: `docs/arch/toolkit-oop/DESIGN.md` §3.8 (Profile 3), §3.9 (K8s Packaging), "Platform Host Composition";
ADR-0001 (deployment profiles), ADR-0004 (helm chart organization), ADR-0006 (platform-plane auth), ADR-0007 (edge architecture).

## Locked decisions

| # | Decision | Choice |
|---|----------|--------|
| §10.2 | Binary layout | **Hybrid** — a dedicated `apps/platform-host` binary crate (core + system gears) **plus** per-gear OoP binaries (following the `calculator-oop` / `run_oop_with_options` template) for minimal gear images. |
| §10.1 | Feature composition location | **New `apps/platform-host` crate** (and per-gear OoP bin crates). `cf-gears-example-server` stays as the dev/embedded example, untouched. Gear isolation comes from the **crate dependency graph**, not `#[cfg]`-gating a monolithic `registered_gears.rs`. |
| §10.6 | Build/publish mechanism | **Dedicated CI workflow** (`.github/workflows/docker-publish.yml`), buildx → `ghcr.io/cyberfabric`, on release tags + manual dispatch. Independent of `release-plz` (crates.io only). |
| §7.2 scope | First-pass scope | **Platform-host image + chart first**, then split out the OoP-eligible gears into minimal images + charts. |

> **Note on §10.1:** the issue framed feature-gating around a single generalized binary. With the chosen dedicated-crate
> approach, the "excludes unrelated gears" acceptance criterion is met structurally — `apps/platform-host` depends only on
> the core+system set, and each per-gear OoP crate depends only on its gear + dep SDK/client crates. No `#[cfg]`-gating of
> `cf-gears-example-server` is required. Template: `examples/toolkit/users-info/users-info-server` (host binary) and
> `examples/oop-gears/calculator/calculator` (OoP binary).

## Composition (from DESIGN "Platform Host Composition")

**Corrected split (user-confirmed): infra tier STAYS in platform-host; the business tier goes OoP.**
This supersedes an earlier reading that split system gears out. Per issue #4110 (OoP-8, Platform Host + Infra REST
Surface), the shared infra services stay co-located in the platform-host pod and gain REST surfaces so OoP business
gears can reach them; only the business tier is packaged as separate per-gear images.

Platform-host image/binary bundles (the **infra host**):

- **Trust-coupled core (must stay co-located):** `authz-resolver`, `tenant-resolver`, `resource-group`, `account-management`.
- **System/infra gears:** `gear-orchestrator` (DirectoryService), `types-registry` (central registry OoP gears register
  into via `POST /types-registry/v1/entities`), `credstore`, `api-gateway` (REST edge), `grpc-hub`.
- **Embedded in every pod:** `authn-resolver` (stateless JWT validation; not split out).

OoP-eligible = the **business tier** (own per-gear images/charts): `file-parser`, `simple-user-settings`, `mini-chat`,
`oagw`, `chat-engine`, `bss-*`, etc. Each embeds `authn-resolver` and calls back to the platform-host's REST surfaces
(`RestAuthZResolverClient`, `RestTenantResolverClient`, `RestTypesRegistryClient`, `RestCredStoreClientV1`) resolved via
DirectoryService.

> **Dependency on #4110 (OoP-8):** the `Rest*Client`s + platform-host REST endpoints are the *functional* prerequisite
> for a business OoP gear to work end-to-end. OoP-10 (images) and OoP-7 (charts) can produce the packaging in parallel,
> but a business-gear image only *functions* once #4110 lands. `file-parser` (zero shared-service deps per #4110) is the
> cleanest first template.

## Contract between the two issues

OoP-7 charts default `image.registry/repository:tag`; OoP-10 must produce images at those coordinates. Naming/tag
conventions (§10.5/§10.7) are defined by OoP-10 and consumed by OoP-7 `values.yaml`:

- Gear image: `ghcr.io/cyberfabric/<gear-name>:<tag>`
- Platform host image: `ghcr.io/cyberfabric/platform-host:<tag>`
- Tag aligns with each chart's `AppVersion`; platform-host and gear images may version independently.

Recommended order: land OoP-10 image + naming foundations first (or jointly), then wire chart defaults in OoP-7.

## Runtime facts the packaging targets (already implemented)

- Probes: `/healthz` (liveness), `/readyz` (readiness), `/health` (diagnostics) at `OopHttpConfig.listen_addr`, with
  optional `probe_bind_addr` sidecar port. See `libs/toolkit/src/runtime/oop_serve.rs`.
- Config env: `TOOLKIT_MODULE_CONFIG` (rendered gear config JSON), `TOOLKIT_DIRECTORY_ENDPOINT` (discovery). See
  `libs/toolkit/src/runtime/host_runtime.rs`.
- Platform-plane auth: `oop_http.internal_auth` with `provider: kube` → TokenReview with configurable audiences;
  requires the `k8s-auth` toolkit feature. See `libs/toolkit/src/bootstrap/oop.rs::build_internal_authenticator`.
- SA token projection target: audience `toolkit-internal`, projected volume at
  `/var/run/secrets/tokens/toolkit-internal` (per issue §7.2 + ADR-0006).

## Phase 1 — OoP-10 image foundations (#4255)

### 1a. §10.1 / §10.2 Dedicated crates (isolation via dependency graph)

Rather than `#[cfg]`-gating the monolithic `cf-gears-example-server`, gear isolation is achieved by dedicated crates:

- **`apps/platform-host`** (host binary, template: `examples/toolkit/users-info/users-info-server`): depends on the
  trust-coupled core (`authz-resolver`, `tenant-resolver`, `resource-group`, `account-management`) + system gears
  (`gear-orchestrator`, `types-registry`, `credstore`, `api-gateway`, `grpc-hub`) + the default plugin set needed to
  boot. `main.rs` uses `toolkit::bootstrap::run_server` with `run` / `migrate` / `print-config` subcommands;
  `registered_gears.rs` links exactly the core+system set + plugins.
- **Per-gear OoP binaries** for **business-tier** gears (`file-parser`, `simple-user-settings`, `mini-chat`, `oagw`, …):
  each depends only on its gear + dep SDK/client crates, and runs via `toolkit::bootstrap::oop::run_oop_with_options`
  (template: `examples/oop-gears/calculator/calculator/src/main.rs`). The minimal dependency graph structurally
  satisfies the "excludes unrelated gears" acceptance criterion. Placement: an in-crate `[[bin]]` (e.g.
  `file-parser-oop`) gated by an `oop_module` feature, matching the calculator example — keeps the gear crate publishable
  (the bin is not built unless the feature is enabled). System/infra gears do **not** get OoP binaries (they stay in
  platform-host).

`cf-gears-example-server` remains the dev/embedded (Profile 1) example, untouched, so existing configs/tests keep working.

**Plugin presets (`apps/platform-host`).** Plugin availability is a build-time choice (which plugin crates are linked);
the active vendor is selected at runtime via GTS `vendor` config. Two presets:

- `dev-plugins` (default) — `static-authn`, `static-authz`, `static-tenants`, `static-credstore`, `static-idp`. Zero
  external deps; boots out of the box (mirrors `config/e2e-features.txt`). For CI/demo/single-user on-prem.
- `prod-plugins` — `oidc-authn`, `tr-authz` (tenant-resolver-backed authz), `tenant-resolver-rg` (resource-group-backed
  tenant resolution), `static-credstore`, `static-idp`. The real Profile 3 stack. Build the production image with
  `--no-default-features --features prod-plugins` (both presets may also be linked into one image and disambiguated by
  config). Note: credstore and account-management ship only `static-*` backends in-tree today, so those remain `static`
  in both presets until real backends land.

Risk to validate: plugin wiring for the platform-host boot (authn/authz/tenant/credstore/account-management need plugins,
selected via GTS `vendor` config); credstore + resource-group need DB config; ordering of trust-coupled core init
(authz → tenant-resolver → resource-group chain, `am.system` for account-management).

### 1c. §10.3 Platform-host Dockerfile — DONE

`deploy/docker/platform-host.Dockerfile` — multi-stage build:

- Builder stage (`rust:1.95.0-bookworm`, pinned digest): installs `cmake` + `protobuf-compiler`, copies the full
  workspace (`.dockerignore` trims `target/`/`.git/`/`logs/`), builds `cargo build --bin platform-host --package
  cf-gears-platform-host` with BuildKit cache mounts. Build args: `BUILD_PROFILE` (`release`|`dev`), `CARGO_FEATURES`
  (space-separated, e.g. `"prod-plugins k8s otel"`), `CARGO_NO_DEFAULT_FEATURES` (set to drop the `dev-plugins`
  default — required for the prod-plugins image). Binary copied to `/tmp` because the target dir is a cache mount.
- Runtime stage (`debian:13.3-slim`): `ca-certificates`, non-root uid 1000, copies binary + `config/platform-host.yaml`,
  `EXPOSE 8087`. `ENV APP__SERVER__HOME_DIR=/app/data` (+ `mkdir`/`chown`) so runtime state has a writable path instead
  of the non-root user's absent home dir.
- Config: `config/platform-host.yaml` (dev-plugins preset, SQLite, static plugins) — includes the types-registry
  `entities` seed for `cf.core.am.platform.v1` so account-management's bootstrap preflight resolves `root_tenant_type`
  (fixes the earlier non-strict bootstrap WARN). Boots with zero external deps.
- Probes: `/healthz`, `/readyz`, `/health` are all served by the embedded api-gateway on `:8087` (verified 200).

Output: `ghcr.io/cyberfabric/platform-host:<tag>`.

**Verified:** `docker build` (dev profile) succeeds; `docker run` boots the full stack, bootstrap saga completes
(`classification=fresh`), all three probes return 200.

**Prod-plugins status (build-verified only, per user decision):** The prod image builds via
`--build-arg CARGO_NO_DEFAULT_FEATURES=1 --build-arg CARGO_FEATURES="prod-plugins"`. It is **not** run standalone: the
dev `config/platform-host.yaml` selects the `static` vendors, so `oidc-authn-plugin` has no config section and init
fails (`gear 'oidc-authn-plugin' not found`). A real prod run needs a dedicated config (trusted issuers + audience +
S2S discovery, HTTPS-strict; JWKS is fetched lazily so a live IdP is not required to boot) — deferred to the Helm phase
where the config ships as a ConfigMap with real issuer/audience/DB values.

### 1d. §10.4 Parameterized per-gear Dockerfile

One reusable Dockerfile taking `GEAR_NAME` / `CARGO_FEATURES` build args, producing a minimal binary (target gear + dep
SDKs only). Output: `ghcr.io/cyberfabric/<gear-name>:<tag>`.

### 1e. §10.6 Dedicated CI workflow

`.github/workflows/docker-publish.yml`: docker buildx, matrix over {platform-host, per-gear images}, login + push to
`ghcr.io/cyberfabric`, triggered on release tag + `workflow_dispatch`. Cache mounts for cargo registry/target.

### 1f. §10.8 Image tests

- Excludes-unrelated-gears check: inspect the built gear image's linked crates / binary size, assert absence of unrelated
  gear crates (e.g. via `cargo tree` on the feature set, or binary symbol/size diff vs platform-host).
- `docker run` + `curl /healthz` smoke test for platform-host and one representative gear image.

## Phase 2 — OoP-7 Helm charts (#4109)

### 2a. §7.1 `deploy/helm/toolkit-common/` library chart (`type: library`)

Named templates:

- `_deployment.tpl` — standard Deployment: image (`image.registry/image.repository:image.tag`), `http` port (+ optional
  `grpc`), probes wired to `/healthz` (liveness) + `/readyz` (readiness), env (`TOOLKIT_MODULE_CONFIG`,
  `TOOLKIT_DIRECTORY_ENDPOINT`), resources, labels, image pull secrets, and **SA token projection** (audience
  `toolkit-internal`, projected volume mounted at `/var/run/secrets/tokens/toolkit-internal`).
- `_service.tpl` (ClusterIP), `_configmap.tpl` (gear config → `TOOLKIT_MODULE_CONFIG`), `_helpers.tpl` (labels/names/selectors),
  `_serviceaccount.tpl`, `_hpa.tpl`, `_pdb.tpl`, `_networkpolicy.tpl`, `_ingress.tpl`.

### 2b. §7.2 Per-gear charts (`type: application`, depend on `toolkit-common`)

Scope order: `platform-host` chart first, then `gear-orchestrator`, `api-gateway`, `authn-resolver` (issue minimum),
extending to `types-registry`, `credstore` as split-out proceeds. Each: `Chart.yaml`, `values.yaml` (per DESIGN §3.9
conventions), `values.schema.json`, `templates/` composed of `{{ include "toolkit-common.*" . }}`.

### 2c. §7.3 `deploy/helm/toolkit-platform/` umbrella chart

Conditional deps on all gear charts (`condition: <gear>.enabled`); presets `values-minimal.yaml`
(platform-host/flight-control + api-gateway + authn-resolver), `values-production.yaml` (all + HPA/PDB),
`values-dev.yaml` (all, minimal resources). `templates/NOTES.txt`.

### 2d. §7.4 Helm CI

`helm dependency build` → `helm lint` → `helm template` → `helm package` → push to OCI registry
(`ghcr.io/cyberfabric/charts/<name>`). Published charts embed the resolved `toolkit-common` library.

### 2e. §7.5 Helm tests

Render each preset; `helm lint` passes; assert SA token projection present in the Deployment spec.

## Phase 3 — Wiring & docs

- §10.7: wire OoP-10 image refs (registry/repo/tag) into each chart's default `values.yaml`; ensure `helm install`
  works with no manual image overrides.
- Document the add-a-new-gear flow: feature-gate → OoP bin → Dockerfile args → chart scaffold (`toolkit-common` includes)
  → values defaults.

## Acceptance criteria (combined, from both issues)

- `helm install my-platform toolkit-platform -f values-minimal.yaml` produces working manifests; SA token projection
  present in the Deployment template.
- Pipeline builds+publishes platform-host image + minimal per-gear images to `ghcr.io/cyberfabric` on release.
- Each per-gear image provably contains only that gear (+ dep SDKs).
- Image tags align with chart `AppVersion`/values defaults (no manual image overrides needed).
- Build/tag/push + chart-scaffold process documented for developers adding a new gear.
