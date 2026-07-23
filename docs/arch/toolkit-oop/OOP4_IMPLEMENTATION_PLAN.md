# OoP-4 Implementation Plan — OoP Bootstrap: HTTP Server + Probes

> Issue: https://github.com/constructorfabric/gears-rust/issues/4106
> Component: `cpt-cf-component-oop-bootstrap`
> PRD: `cpt-cf-fr-rest-primary`, `cpt-cf-fr-eventual-readiness`, `cpt-cf-fr-oop-lifecycle`
> ADR: `0005-cpt-cf-adr-eventual-readiness`
> Effort: **Large — the biggest phase.**
> Status: **IMPLEMENTED** on branch `feat/oop4-http-server` (Tasks A–H + G). All toolkit
> tests pass; clippy clean (default / `bootstrap` / `k8s-auth`); full `cargo check --workspace` green.
> See commit history on the branch. This doc remains the design reference.

## Branch / repo state (as of planning)

- Repo: `gears-rust` (`/home/matt/code/rust/gears-rust`).
- Detached `HEAD` from `me/oop_impl`, 1 commit ahead of `origin/main`:
  `c0b3bb14 feat(directory): add REST endpoint + OpenAPI discovery to DirectoryService` (this is OoP-3).
- **Create a working branch before implementing** (e.g. `git switch -c feat/oop4-http-server c0b3bb14`).

## Prerequisites — ALL PRESENT (verified)

| Ticket | Provides | Location |
|---|---|---|
| OoP-1 | `security_context_middleware<A: BearerAuthenticator>` (tenant plane), `PublicRoute` marker, header extractors | `libs/toolkit-http-middleware/src/auth.rs`, `security.rs` |
| OoP-1 | `BearerAuthenticator` trait, `AuthNError` | `libs/toolkit-security/src/authenticator.rs` |
| OoP-2 | `internal_auth_middleware<A: InternalAuthenticator>` (platform plane) | `libs/toolkit-http-middleware/src/auth.rs` |
| OoP-2 | `InternalCredential`, `InternalAuthenticator`, `InternalAuthNError`, `PlatformIdentity`, `PlatformSecurityContext`, `PeerAuthenticated` | `libs/toolkit-security/src/internal_auth.rs` |
| OoP-3 | `DirectoryClient::{resolve_rest_service, get_openapi_spec, register_instance, deregister_instance, send_heartbeat}`; `RegisterInstanceInfo.{rest_endpoint, openapi_spec}` | `libs/system-sdks/sdks/directory/src/api.rs`, `grpc/client.rs` |

**No concrete `InternalAuthenticator` (K8s `TokenReview`) exists yet.** We are adding a simple one (see Task G).

## Decisions locked in (from planning discussion)

1. **Auth middleware wiring = pluggable/optional injection points (Option 1).**
   The OoP serve options expose `Option<Arc<dyn BearerAuthenticator>>` and
   `Option<Arc<dyn InternalAuthenticator>>`. Each middleware is installed **only when its
   authenticator is provided**. This keeps `libs/toolkit` free of concrete gear-SDK deps
   (`authn_resolver_sdk`) and matches the OoP-1/OoP-2 design ("concrete authenticator adapters
   injected via Axum state at the gear/bootstrap layer"). The middleware itself already lives in
   `toolkit-http-middleware` and is generic — nothing to move.

2. **Include a simple K8s `TokenReview` `InternalAuthenticator` now, feature-gated.**
   New leaf crate `libs/toolkit-k8s-auth` (pulls in `kube`/`k8s-openapi`). OoP bootstrap
   auto-wires it when `InternalCredential::KubeServiceAccountToken` is configured. Gate behind a
   `k8s-auth` feature so the `kube` dep is opt-in.

3. **Keep `libs/toolkit` SDK-agnostic.** The tenant-plane `BearerAuthenticator` adapter
   (`AuthNResolverClient` → `BearerAuthenticator`) is supplied by the **app/gear binary**, not the
   framework. Provide the adapter as a tiny helper the app can use (or document the pattern).

## Current-state facts that shape the design

- `run_oop_with_options` (`libs/toolkit/src/bootstrap/oop.rs`) loads config, connects to
  DirectoryService, spawns heartbeat, then calls `run(RunOptions)` →
  `HostRuntime::run_gear_phases()`. **It never starts an HTTP server, never registers a REST
  endpoint, and never deregisters** (the doc comment claims deregistration but code doesn't do it).
- `HostRuntime::run_rest_phase` (`libs/toolkit/src/runtime/host_runtime.rs:389`) **builds** a
  `Router` but the caller **discards it**: `let _router = self.run_rest_phase().await?;` (line ~830).
  It also **requires an `ApiGatewayCap` host** — returns `RestRequiresHost` if any `RestApiCap`
  gear exists without a host. OoP gears do **not** embed `api-gateway`, so we need a **host-less**
  REST composition path.
- `OpenApiRegistryImpl` (`libs/toolkit/src/api/openapi_registry.rs`) is standalone and usable to
  compose routes without api-gateway. `RestApiCapability::register_rest(ctx, router, &dyn OpenApiRegistry)`
  registers a gear's routes (`libs/toolkit/src/contracts.rs:52`).
- `GearEntry::deps() -> &'static [&'static str]` (`libs/toolkit/src/registry.rs:229`) exposes the
  dep names declared in `#[toolkit::gear(deps = [...])]`. Registry topo-sorts; in OoP (Profile 3)
  deps are out-of-process and resolved via DirectoryService.
- `GearCtx` (`libs/toolkit/src/context.rs`) exposes `client_hub()` but **no `runtime()` accessor**
  and no readiness API. The `register_readiness_check` API must be added (new runtime handle
  threaded through `GearContextBuilder` → `GearCtx`).
- `ServerConfig` (`libs/toolkit/src/bootstrap/config/mod.rs:145`) has only `name` + `home_dir`.
  New OoP HTTP settings go in a **new config section** (don't overload `ServerConfig`).
- `api-gateway` `serve()` (`gears/system/api-gateway/src/gear.rs:547`) is the reference for
  bind + `axum::serve().with_graceful_shutdown()`. Its `apply_middleware_stack` (line ~238) is the
  reference for a layered stack, but OoP uses a **leaner framework stack** (no gateway-specific
  route policy / rate-limit / license layers).
- `libs/toolkit/Cargo.toml`: `axum`, `http` are deps; `tower` is **dev-only**. `toolkit-security`
  and `toolkit-http-middleware` are **NOT yet deps**. OoP serving is bootstrap-only, so add these
  under the `bootstrap` feature.

## Architecture / integration approach

Serve the OoP HTTP server from **within the runtime**, between the START phase and the STOP phase,
replacing the plain "wait for cancellation" step for OoP gears. Concretely:

- Add an optional `OopServeOptions` carried through `RunOptions` → `HostRuntime`.
- When present, `HostRuntime`:
  1. Composes a **host-less** OoP router from `RestApiCap` gears + a framework-owned
     `OpenApiRegistryImpl` (new `compose_oop_router`).
  2. Merges the framework **probe router** (`/healthz`, `/readyz`, `/.well-known/openapi.json`).
  3. Applies the **lean framework middleware stack** + the two auth middlewares (when authenticators
     provided) + a **drain-guard** middleware (503 + Retry-After once draining).
  4. Binds the listener(s) (main `listen_addr`, optional `probe_bind_addr`).
  5. Spawns **background self-registration** (retry w/ backoff, re-register on loss) and
     **background dependency resolution** (poll `resolve_rest_service`, wire REST clients into
     ClientHub, flip readiness when all critical deps resolved).
  6. `axum::serve(...).with_graceful_shutdown(cancel)` on a task; probes serve immediately.
  7. On cancel: run the **drain sequence** (readiness flip → stop new work → drain in-flight →
     deregister → stop runtime tasks → close listener), then proceed to the existing STOP phase.

This keeps `HostRuntime` as the single owner of phases and of the composed router. `run_oop_with_options`
just constructs `OopServeOptions` from config and passes it in; it also performs the DirectoryService
connection (already does) and hands the client to registration/deps tasks.

## Work breakdown (file-by-file)

### Task A — Readiness subsystem  *(issue task 4.2)*
- **New** `libs/toolkit/src/runtime/readiness.rs`:
  - `pub enum CheckResult { Ready, NotReady { reason }, Degraded { reason } }`.
  - `#[async_trait] pub trait ReadinessCheck: Send + Sync { async fn check(&self) -> CheckResult; }`.
  - `ReadinessState` (Arc-shared): tracks (a) set of unresolved critical deps, (b) registered custom
    checks by name, (c) a `draining` flag, (d) a **1s TTL cache** of the evaluated aggregate.
  - `evaluate()` → `(http_status, json_body)`: `503` + unresolved deps + failing checks until all
    deps resolved AND all checks `Ready`/`Degraded`; `200` otherwise. `Degraded` reported in body but
    still `200` (Spring Boot health groups).
  - `register_readiness_check(name, Arc<dyn ReadinessCheck>)`, `mark_dep_resolved(name)`,
    `set_draining(true)`.
- **Runtime handle**: add `RuntimeHandle` (wraps `Arc<ReadinessState>`) and thread it through
  `GearContextBuilder` → `GearCtx::runtime()` so gears can call
  `ctx.runtime().register_readiness_check(...)` (per DESIGN example).
- Export from `runtime/mod.rs`.

### Task B — Probe + well-known router  *(issue tasks 4.2, 4.6)*
- **New** `libs/toolkit/src/runtime/oop_probes.rs` (or fold into `oop_serve.rs`):
  - `GET /healthz` → `200 "ok"` always (process alive).
  - `GET /readyz` → delegates to `ReadinessState::evaluate()` (`503` w/ unresolved list, else `200`;
    `Degraded` → `200` w/ body).
  - `GET /.well-known/openapi.json` → serves the gear's generated OpenAPI JSON
    (`cpt-cf-binding-constraint-openapi-well-known`). Keep `/openapi.json` too for parity.
  - Mark probe routes with `PublicRoute` extension so `security_context_middleware` lets them through.

### Task C — OoP HTTP server + middleware + drain  *(issue tasks 4.1, 4.5)*
- **New** `libs/toolkit/src/runtime/oop_serve.rs`:
  - `pub struct OopServeOptions { listen_addr, probe_bind_addr: Option<..>, drain_timeout: Duration,
    bearer_authenticator: Option<Arc<dyn BearerAuthenticator>>,
    internal_authenticator: Option<Arc<dyn InternalAuthenticator>>,
    internal_credential: InternalCredential, gear_name, version, directory: Arc<dyn DirectoryClient>,
    heartbeat_interval }`.
  - `compose_oop_router(&self) -> (Router, OpenApiJson)`: iterate `RestApiCap` gears, register into a
    fresh `OpenApiRegistryImpl`, build OpenAPI via `OpenApiInfo`. **No `ApiGatewayCap` required.**
  - `apply_framework_middleware(router)`: lean stack — request-id, trace span, canonical-error
    middleware, catch-panic, timeout, body-limit. Reuse `toolkit::api` middleware where available;
    do **not** pull gateway-specific layers.
  - Install auth middlewares (order: `internal_auth_middleware` **before** `security_context_middleware`
    per DESIGN §3.2) only when the respective authenticator is `Some`.
  - **Drain-guard** middleware: track in-flight via `Arc<AtomicUsize>`; once `draining`, reject new
    requests with `503` + `Retry-After` while letting in-flight finish.
  - `serve()`: bind main + optional probe listener; `axum::serve().with_graceful_shutdown(cancel)`.
  - **Drain sequence** on cancel (DESIGN §3.2 order): (1) `readiness.set_draining(true)`; (2) start
    rejecting new work; (3) wait up to `drain_timeout` for in-flight = 0; (4) deregister from
    DirectoryService; (5) [reverse-dep = operator responsibility — document only]; (6) cancel
    heartbeat + registration + deps tasks; (7) drop listener.
- **`InternalCredential` init**: build from config; when
  `KubeServiceAccountToken`, load projected SA token and attach `X-ToolKit-Internal-Token` to
  outgoing system calls (registration, heartbeat). The Profile-2 `TOOLKIT_INTERNAL_TOKEN` env path is
  deferred to P2 (documented, not wired). Outgoing attach helper: `toolkit_http::attach_internal_token_http`
  (verify exact name in `libs/toolkit-http/src/security.rs`).

### Task D — Self-registration & dependency resolution  *(issue tasks 4.3, 4.4)*
- **New** `libs/toolkit/src/runtime/oop_registration.rs`:
  - `register_loop`: `RegisterInstanceInfo` including `rest_endpoint` (from `listen_addr`) and
    `openapi_spec` (from composed OpenAPI). Exponential backoff **100ms → 30s cap**; re-register on
    error / connection loss. Non-blocking (spawned task); HTTP server + probes are up immediately.
  - `deps_loop`: for each `GearEntry::deps()` entry, poll `directory.resolve_rest_service(dep)`; on
    resolve, wire a REST client into `ClientHub` and `readiness.mark_dep_resolved(dep)`. Gate
    `/readyz` on all critical deps. In K8s, **also** accept k8s DNS
    (`{gear}.{namespace}.svc.cluster.local`) as a resolution source (DirectoryService optional for
    metadata — `cpt-cf-fr-k8s-native`). **Profile 1: no-op** (topo-sort already satisfied).
  - REST client wiring depends on the codegen client shape — if the generated REST client isn't
    available on this branch (PR #4084 dependency per prior memory), wire a **generic resolved
    endpoint** into ClientHub and leave typed-client codegen to its ticket. Confirm at implementation.

### Task E — Runtime/HostRuntime integration
- `libs/toolkit/src/runtime/runner.rs`: add `oop_serve: Option<OopServeOptions>` to `RunOptions`;
  pass into `HostRuntime::new`.
- `libs/toolkit/src/runtime/host_runtime.rs`:
  - Store `oop_serve`. Add `run_oop_serve_phase()` used in Full mode when `oop_serve.is_some()`:
    compose router → merge probes → middleware → bind → spawn registration+deps → serve with graceful
    shutdown → drain. Replaces the plain `self.cancel.cancelled().await` wait for OoP gears.
  - Ensure `run_rest_phase` is not required to have a host in OoP mode (either branch on `oop_serve`
    or skip host-required check when serving via OoP path).
- `libs/toolkit/src/bootstrap/oop.rs`: build `OopServeOptions` from new config; pass through
  `RunOptions`. Reuse the already-connected `DirectoryClient` for registration/deps. Keep existing
  heartbeat OR move it into the serve subsystem (prefer consolidating in `oop_serve` so drain can
  stop it deterministically). Remove the misleading "deregisters on shutdown" claim once real
  deregistration lands in the drain sequence.

### Task F — Config additions
- New section (e.g. `OopHttpConfig`) surfaced via `AppConfig`, with:
  `listen_addr` (default e.g. `0.0.0.0:8080`), `probe_bind_addr: Option<String>` (default: same as
  main), `drain_timeout` (default `30s`), and platform-plane settings
  (`internal_credential` selection, SA `token_path`, `audience`). `deps` come from **gear metadata**
  (registry), not config.
- Wire defaults so a gear with no explicit config still starts, serves probes, and registers.

### Task G — Simple K8s TokenReview authenticator (feature-gated)
- **New crate** `libs/toolkit-k8s-auth` (package `cf-gears-toolkit-k8s-auth`), added to workspace
  `members` + `[workspace.dependencies]` (mirror `toolkit-http-middleware` entry style).
  - `impl InternalAuthenticator` calling the K8s `TokenReview` API (`kube` + `k8s-openapi`),
    validating audience, producing `PlatformIdentity::KubernetesServiceAccount { namespace,
    service_account, pod }`.
  - Map errors: unreachable API → `InternalAuthNError::Unavailable`; rejected → `InvalidToken`.
- Gate in `libs/toolkit` behind a `k8s-auth` feature (`dep:toolkit-k8s-auth`). OoP bootstrap
  auto-constructs it when `InternalCredential::KubeServiceAccountToken` is configured AND feature is
  on; otherwise internal-auth stays unwired (permissive per middleware contract).

### Task H — Tests  *(issue task 4.7)*
- Unit (in-crate, follow `#[path = "..._tests.rs"]` convention like `oop_tests.rs`):
  - Readiness state transitions: deps unresolved → `503`; all resolved → `200`; `NotReady` check →
    `503` w/ name+reason; `Degraded` → `200` w/ body; 1s cache behavior.
  - Drain-guard: in-flight tracking, `503 + Retry-After` when draining.
  - Registration backoff schedule (100ms→30s cap); re-register on error.
  - `compose_oop_router` builds routes + OpenAPI without an api-gateway host.
- Integration (`oop_serve` end-to-end with a stub `DirectoryClient` + a tiny test gear):
  - Startup sequence: server binds, `/healthz` 200 immediately, `/readyz` 503→200 as a stub dep
    resolves, request to a gear route succeeds.
  - Registration retry against a flaky stub directory.
  - Shutdown drain order: readiness flips first, in-flight request completes, deregister called,
    no dropped requests.

## Acceptance criteria mapping

| Criterion | Covered by |
|---|---|
| OoP gear starts HTTP server, registers, resolves deps, serves requests | Tasks C, D, E |
| `/healthz` 200 immediately; `/readyz` 503→200 as deps resolve | Tasks A, B, D |
| Graceful shutdown completes without dropped requests | Task C (drain) + integration tests (Task H) |

## Out of scope for OoP-4 (separate tickets)

- **GatewayProvider / reverse-proxy** (`cpt-cf-component-gateway-provider`, tickets D1–D4).
- **REST client codegen** (`cpt-cf-component-rest-client-gen`, PR #4084 dependency) — if unavailable,
  wire resolved endpoints generically (Task D note).
- **mTLS/SPIFFE** platform identity, **Profile 2** `TOOLKIT_INTERNAL_TOKEN` env path (P2).
- Helm charts / k8s packaging (E-series tickets).

## Open questions

1. ~~Outgoing internal-token attach helper name.~~ **RESOLVED**: `toolkit_http::attach_internal_token_http`
   and `attach_bearer_http` exist (`libs/toolkit-http/src/security.rs`). Internal token uses the
   `X-ToolKit-Internal-Token` header (`INTERNAL_TOKEN_HEADER` const in `toolkit-security`).
2. ~~Generated REST client for ClientHub wiring.~~ **RESOLVED**: no `rest_contract`/`RestClient`
   codegen exists on this branch (PR #4084 unmerged). Task D wires a **generic resolved-endpoint**
   into ClientHub; typed-client codegen is deferred to its own ticket.
3. (Implementation detail) Consolidate the existing heartbeat loop into `oop_serve` (recommended so
   drain can stop it deterministically) vs leave in `run_oop_with_options`.
4. (Implementation detail) Final config field names/defaults for `OopHttpConfig` and the `k8s-auth`
   feature name.
