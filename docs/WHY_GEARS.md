# Why Should I Use CF / Gears (Rust)?


<!-- toc -->

- [Executive Summary](#executive-summary)
  - [At a glance: Go vs C# vs Rust vs Rust + Gears](#at-a-glance-go-vs-c-vs-rust-vs-rust--gears)
- [Part A — Where Rust has advantages over Go and C# (for platform code)](#part-a--where-rust-has-advantages-over-go-and-c-for-platform-code)
  - [A.1 Errors are part of the type system](#a1-errors-are-part-of-the-type-system)
  - [A.2 Data races are a compile error, not a `-race` flag](#a2-data-races-are-a-compile-error-not-a--race-flag)
  - [A.3 No `nil` interfaces, no exceptions-from-anywhere](#a3-no-nil-interfaces-no-exceptions-from-anywhere)
  - [A.4 Sum types make illegal states unrepresentable](#a4-sum-types-make-illegal-states-unrepresentable)
  - [A.5 Exhaustive `match` makes state evolution safer](#a5-exhaustive-match-makes-state-evolution-safer)
  - [A.6 Zero-cost abstractions and predictable performance](#a6-zero-cost-abstractions-and-predictable-performance)
  - [A.7 Tooling and static analysis as a first-class citizen](#a7-tooling-and-static-analysis-as-a-first-class-citizen)
- [Part B — Why "just Rust" is not enough: what Gears adds](#part-b--why-just-rust-is-not-enough-what-gears-adds)
  - [B.1 Tenant isolation that you can't forget](#b1-tenant-isolation-that-you-cant-forget)
  - [B.2 Authentication & authorization, built in (NIST SP 800-162 PDP/PEP)](#b2-authentication--authorization-built-in-nist-sp-800-162-pdppep)
  - [B.3 One consistent API dialect: `OperationBuilder` + OpenAPI + OData](#b3-one-consistent-api-dialect-operationbuilder--openapi--odata)
  - [B.4 Architecture enforced at compile time (`dylint`)](#b4-architecture-enforced-at-compile-time-dylint)
  - [B.5 A pre-integrated XaaS backbone (and it's replaceable)](#b5-a-pre-integrated-xaas-backbone-and-its-replaceable)
  - [B.6 Extensible domain model via the Global Type System (GTS)](#b6-extensible-domain-model-via-the-global-type-system-gts)
  - [B.7 Composable gears: one codebase, many deployment shapes](#b7-composable-gears-one-codebase-many-deployment-shapes)
  - [B.8 Canonical errors](#b8-canonical-errors)
  - [B.9 Observability and operational defaults](#b9-observability-and-operational-defaults)
  - [B.10 FIPS 140-3 support](#b10-fips-140-3-support)
  - [B.11 Other useful Gears runtime patterns](#b11-other-useful-gears-runtime-patterns)
  - [B.12 Local-first, shift-left development](#b12-local-first-shift-left-development)
- [When Gears is (and isn't) the right choice](#when-gears-is-and-isnt-the-right-choice)
- [Get started](#get-started)

<!-- /toc -->

> A guide for **Go developers** (and C# developers) evaluating [Constructor Fabric Gears](https://github.com/constructorfabric/gears-rust) — a secure, modular **XaaS development framework & middleware** written in Rust.

**Public links**

- **Gears (Rust) monorepo** — <https://github.com/constructorfabric/gears-rust>
- **Architecture Manifest** — [`docs/ARCHITECTURE_MANIFEST.md`](./ARCHITECTURE_MANIFEST.md)
- **Overview slides** — [`docs/slides/1_OVERVIEW.md`](./slides/1_OVERVIEW.md)
- **Gears inventory** — [`docs/GEARS.md`](./GEARS.md)
- **Toolkit guide** — [`docs/toolkit_unified_system/README.md`](./toolkit_unified_system/README.md)
- **Global Type System (GTS)** — <https://github.com/globaltypesystem/gts-spec>
- **Constructor Fabric Foundation** — <https://www.constructorfabric.org>

---

## Executive Summary

If you build **multi-tenant XaaS / SaaS backends**, you are repeatedly solving the same problems: tenant isolation, authentication/authorization, licensing & quota, usage metering, consistent REST APIs, pagination/filtering, observability, and safe DB access. Go and C# can absolutely solve these problems, but many guarantees are usually enforced through framework conventions, code review, analyzers, and shared team discipline rather than through one integrated platform contract.

Gears takes a different position, in two layers:

1. **Rust as the language** moves whole classes of bugs — data races, use-after-free, nil-dereferences, unhandled errors — from *runtime incidents* to *compile errors*.

2. **Gears as the middleware** goes further: it makes **security and tenancy structural**. Architectural rules are enforced by custom `dylint` lints in CI, and DB access is scoped automatically by the framework, so the preferred path is also the safer path.

The result: you write business logic, and the platform gives you multi-tenancy, defense-in-depth, a uniform API dialect (OpenAPI + OData), canonical errors, an extensible type system (GTS), and one deployment model that runs from a single edge binary up to Kubernetes — **without rewrites**.

### At a glance: Go vs C# vs Rust vs Rust + Gears

| Concern | Go (typical) | C# / .NET | Rust (plain) | **Rust + Gears** |
|---|---|---|---|---|
| **Memory & data-race safety** | GC; data races possible, detected only at runtime (`-race`) | GC; data races possible | Compile-time ownership & `Send`/`Sync` | Compile-time, same as Rust |
| **Error handling** | `if err != nil`, easy to ignore | exceptions (often uncaught) | `Result<T, E>`, `?` — must handle | `Result` + **canonical error taxonomy** (RFC-9457) |
| **Null safety** | `nil` panics | nullable refs (NRT opt-in) | `Option<T>` — no null | `Option<T>` everywhere |
| **Panics / unsafe shortcuts** | runtime panics possible | runtime exceptions possible | possible, but lintable | `unwrap`, `panic`, unsafe patterns **prohibited at build time** |
| **State evolution** | `switch` may miss new constants | `switch` often needs analyzer support | exhaustive `match` over enums | exhaustive `match` + architecture lints |
| **Tenant isolation** | manual `WHERE tenant_id = ?` | manual / EF global filters | manual | **Automatic** via `SecureConn` + `AccessScope` |
| **AuthN / AuthZ** | per-service middleware, bespoke | ASP.NET policies | bespoke | **Built-in** PDP/PEP (NIST SP 800-162) |
| **API consistency** | per-team router conventions | attributes + filters | bespoke | **`OperationBuilder`** → uniform REST + OpenAPI |
| **Pagination / filtering** | hand-rolled | OData libs | hand-rolled | **Built-in OData** `$filter`/`$select`/`$orderby` |
| **Architecture enforcement** | code review + `go vet` | analyzers | Clippy | Excessive Clippy rules + **Custom `dylint` rules fail the build** |
| **Multi-tenancy / licensing / quota / usage** | build it yourself | build it yourself | build it yourself | **Pre-integrated, replaceable gears** |
| **Extensible API domain data types** | manual | manual | manual | **GTS** — versioned, schema-validated, autogenerated json schemas from Rust code |
| **Deployment shapes** | per-service choices | per-service | per-service | **One code → edge / bare-metal / K8s** |
| **Runtime footprint** | small binary, GC pauses | larger runtime, GC | small, no GC | small, no GC |

> **TL;DR for Go devs:** You keep the things you like about Go — a single static
> binary, fast startup, low footprint, great concurrency — and you gain compile-time
> correctness *plus* a batteries-included XaaS backbone you would otherwise hand-roll
> in every service.

---

## Part A — Where Rust has advantages over Go and C# (for platform code)

This section is about the **language**. Gears is built on Rust specifically because it targets *long-lived platform code* where correctness and maintainability matter more than raw time-to-first-prototype.

### A.1 Errors are part of the type system

In Go, error handling is explicit and simple, but it is also easy to accidentally discard an error if the codebase does not enforce stricter rules.

```go
// Go — compiles fine, but the error is silently dropped.
func loadUser(id string) *User {
    u, _ := db.FindUser(id) // ignored error; u may be nil
    return u
}

caller := loadUser("42")
fmt.Println(caller.Name) // nil pointer dereference at runtime
```

In Rust, the error is part of the type. You must deal with it, and there is no `nil`.

```rust
// Rust — you cannot accidentally ignore the error or deref a null.
fn load_user(id: &str) -> Result<User, RepoError> {
    let user = db.find_user(id)?; // `?` propagates the error explicitly
    Ok(user)
}

match load_user("42") {
    Ok(user) => println!("{}", user.name),
    Err(e)   => tracing::warn!(error = %e, "user not found"),
}
```

`Option<T>` replaces `nil`, and `Result<T, E>` makes "this can fail" visible in the signature. Whole categories of `nil` panics and swallowed errors simply cannot occur.

### A.2 Data races are a compile error, not a `-race` flag

Go's race detector is excellent — but it only finds races on code paths you actually execute under instrumentation. Races ship to production all the time.

```go
// Go — compiles, runs, and corrupts the map under load. No compile error.
counts := map[string]int{}
for _, ev := range events {
    go func(e Event) {
        counts[e.Key]++ // concurrent map write -> runtime panic / corruption
    }(ev)
}
```

Rust's ownership model and the `Send`/`Sync` traits make shared-mutable-state-across-threads a **compile error**. You're forced to use a proper synchronization primitive.

```rust
// Rust — won't compile unless the shared state is actually thread-safe.
use std::sync::{Arc, Mutex};

let counts = Arc::new(Mutex::new(HashMap::<String, i64>::new()));
let mut handles = vec![];
for ev in events {
    let counts = Arc::clone(&counts);
    handles.push(std::thread::spawn(move || {
        *counts.lock().unwrap().entry(ev.key).or_insert(0) += 1;
    }));
}
```

"If it compiles, it's free of data races" is not a slogan — it's enforced by the borrow checker. For a platform handling concurrent multi-tenant traffic, that shifts an important class of concurrency defects from runtime testing into compilation.

### A.3 No `nil` interfaces, no exceptions-from-anywhere

C# gives you a rich runtime, but exceptions are invisible in signatures — any call can throw, and `NullReferenceException` remains the most common production failure. Go's `nil` interface trap (`err != nil` being true for a typed-nil) catches even experienced developers.

Rust has neither. Fallibility (`Result`) and absence (`Option`) are explicit in every signature, and exhaustive `match` means adding a new variant forces you to handle it everywhere.

**Gears** goes further than plain Rust style advice: nil-like failure paths and unsafe shortcuts are blocked by the build. Project policy treats `unwrap`, avoidable `panic`, unchecked assumptions, and unsafe patterns as architecture violations, not personal taste. In Go or C#, teams can enforce similar rules with analyzers, linters, and review policy; in Gears, Clippy plus project-specific lints make these checks part of the standard build gate.

### A.4 Sum types make illegal states unrepresentable

Modeling a state machine in Go usually means a struct with a bunch of optional fields and a comment explaining which combinations are "valid."

```go
// Go — nothing stops you from setting ErrorMsg while Status == "running".
type Job struct {
    Status   string // "pending" | "running" | "done" | "failed"  (by convention)
    Result   *Output
    ErrorMsg string
}
```

Rust enums carry data per-variant, so invalid combinations can't be constructed:

```rust
// Rust — the compiler guarantees a failed job has an error and no result.
enum Job {
    Pending,
    Running { started_at: Instant },
    Done { result: Output },
    Failed { error: String },
}
```

### A.5 Exhaustive `match` makes state evolution safer

In Go or C#, if you add a new status, old `switch` statements may keep compiling unless analyzers or strict review rules require every call site to be revisited:

```go
// Go — adding StatusCanceled later does not force every switch to be updated.
switch job.Status {
case StatusPending:
    queue(job)
case StatusRunning:
    observe(job)
case StatusDone:
    archive(job)
}
```

In Rust, matching an enum is exhaustive by default. If you later add `Cancelled`, every `match` that forgot it fails to compile until you decide what the new state means:

```rust
// Rust — adding Job::Cancelled forces this match to be updated.
match job {
    Job::Pending => queue(job),
    Job::Running { started_at } => observe(started_at),
    Job::Done { result } => archive(result),
    Job::Failed { error } => report(error),
}
```

The same pattern appears everywhere: `Option` forces you to handle absence, `Result` forces you to handle failure, and enums force you to handle new states. This is exactly the kind of language support you want when platform APIs and workflows evolve over years.

### A.6 Zero-cost abstractions and predictable performance

No garbage collector means no GC pauses and a small, predictable memory footprint — which is exactly what you want for edge/on-prem appliances and for running the *full* platform locally during development. You get C-like performance with high-level ergonomics, and the abstractions compile away.

### A.7 Tooling and static analysis as a first-class citizen

`cargo`, `rustfmt`, `clippy`, and `cargo-deny` give a consistent, batteries-included experience. Crucially, Rust's lint infrastructure is extensible — which is the hook Gears uses to enforce **architecture** at compile time (see Part B).

> **Honest trade-off:** Rust has a steeper learning curve and longer compile times than Go. For throwaway scripts or a team optimizing purely for time-to-first-demo, Go may win. For a **secure, multi-tenant platform you'll operate for years**, Rust's upfront cost buys stronger compile-time guarantees and a richer static-analysis surface.

---

## Part B — Why "just Rust" is not enough: what Gears adds

Rust gives you a safe language. It does **not** give you multi-tenancy, an authz model, a consistent API dialect, licensing, or a deployment story. In Go or Rust alike, every team re-implements these — slightly differently — in every service.

Gears is the **middleware and framework** that provides them once, correctly, and makes the secure path the path of least resistance.

### B.1 Tenant isolation that you can't forget

One of the highest-risk bugs in a SaaS backend is a missing tenant filter — one missing `WHERE tenant_id = ?` can expose data across tenants.

In many Go or C# services, this is handled through query helpers, ORM conventions, middleware, or code review. Those approaches can work well, but they still depend on every code path using the right abstraction:

```go
// Go — one missing clause = cross-tenant data leak. The compiler is silent.
rows, _ := db.Query("SELECT * FROM documents WHERE owner = ?", userID)
// forgot AND tenant_id = ?  -> leaks every tenant's documents
```

In Gears, entities derive `Scopable`, and `SecureConn` applies the caller's `AccessScope` (tenant, resource, owner, type) as automatic `WHERE` clauses. There is **no unscoped query API** to misuse:

```rust
// Rust + Gears — scoping is applied by the framework from the SecurityContext.
#[derive(Scopable)]
#[secure(tenant_col = "tenant_id", owner_col = "owner_id")]
struct Document { /* ... */ }

// The AccessScope (derived from the authenticated caller) is applied automatically.
let docs = secure_conn
    .scoped::<Document>(&access_scope)
    .filter(documents::Column::Status.eq("active"))
    .all()
    .await?; // emitted SQL always includes the tenant/owner predicates
```

> The architecture makes the **unscoped path harder than the scoped one** — there is
> no "unscoped shortcut" to reach for by accident.

### B.2 Authentication & authorization, built in (NIST SP 800-162 PDP/PEP)

Gears ships a real authorization architecture, not a middleware stub:

- **API Gateway** validates the token and injects a `SecurityContext`.
- The **PDP** (Policy Decision Point — an AuthZ Resolver plugin) evaluates policies
  (RBAC/ABAC/ReBAC — vendor's choice) and returns a decision **plus row-level
  constraints**.
- The **PEP** (Policy Enforcement Point — your domain gear) compiles those
  constraints into SQL `WHERE` clauses via `AccessScope`.
- Returns **predicates, not resource IDs** → O(1) authz overhead, correct pagination,
  accurate counts.
- **Fail-closed**: denied / unreachable PDP / missing constraints → `403`.

In Go you would wire and re-wire this per service; in Gears it's a platform contract.

### B.3 One consistent API dialect: `OperationBuilder` + OpenAPI + OData

In Go, each team picks a router and invents its own conventions for auth, errors, pagination, and OpenAPI generation (if any). Gears has a single authoritative route-registration mechanism. One declaration produces the route, the auth posture, the license posture, the schemas, the registered error responses, **and** the OpenAPI entry:

```rust
// Rust + Gears — one place declares everything; OpenAPI is generated from it.
OperationBuilder::get("/documents/v1/documents")
    .operation_id("documents.list")
    .summary("List documents")
    .authenticated()                       // auth posture is part of the route
    .require_license_features::<License>([])
    .handler(handlers::list_documents)
    .json_response_with_schema::<dto::DocumentPage>(openapi, StatusCode::OK, "OK")
    .error_401(openapi)
    .error_500(openapi)
    .register(router, openapi);
```

This guarantees uniform pagination/filtering (**OData** `$filter`, `$select`, `$orderby`), consistent auth, rate-limiting, timeouts, and observability across every gear — and an always-accurate OpenAPI spec, because it's derived from the same code that runs.

### B.4 Architecture enforced at compile time (`dylint`)

This is where Gears uses Rust's compiler-integrated linting model as a platform feature. Gears ships a suite of custom [`dylint`](https://github.com/constructorfabric/gears-rust/tree/main/tools/dylint_lints) lints that run in CI and **fail the build** on architectural violations:

- **Domain-layer isolation** — no infra imports (`sqlx`, `sea_orm`, `axum`, `reqwest`) inside `domain/`.
- **Direct-SQL restriction** — raw SQL only in migration infrastructure.
- **Versioned REST paths** — endpoints must be `/<gear>/v1/...`.
- **Mandatory `OperationBuilder` metadata** — auth posture, error responses, and schemas must be declared.
- **GTS identifier correctness** — valid IDs; no `schema_for!` on GTS structs.
- **No unsafe shortcuts** — `unwrap`, avoidable `panic`, unsafe code paths, and unchecked invariants are treated as build-time failures where they would undermine platform guarantees.

Why this matters for Gears: the framework is not just a set of helper libraries; it is a **runtime contract** for secure XaaS systems. `dylint` lets the repository encode rules that ordinary Rust tooling cannot know: which layer may import which crate, which paths must be versioned, which API metadata is mandatory, where SQL is allowed, and which GTS identifiers are valid. That turns design documents into compiler-enforced contracts.

Compared with Go/C# alternatives, this is not about one ecosystem being incapable and another being capable. Go has `go vet`, `staticcheck`, and custom analyzers; C# has Roslyn analyzers; both are mature and useful. The difference is that Gears intentionally treats project-specific architecture as a first-class compile-time contract: layer boundaries, route metadata, SQL placement, GTS identifiers, and unsafe shortcuts are checked by the same quality gate as formatting, Clippy, tests, and security checks.

> Documentation in markdown decays. `dylint` makes the architecture **executable** — if you violate it, the code does not compile.

### B.5 A pre-integrated XaaS backbone (and it's replaceable)

Multi-tenancy, permissions & roles, licensing & quota, usage collection, and an event system are all built in — and each is a **regular, replaceable gear** with its own SDK. You can swap Gears' `authn-resolver` / `tenant-resolver` for your existing vendor systems, or integrate an existing product catalog / license engine via plugins, **without modifying core gears**.

### B.6 Extensible domain model via the Global Type System (GTS)

Gears exposes extensible domain objects through [GTS](https://github.com/globaltypesystem/gts-spec): globally unique, human-readable, **versioned** identifiers (e.g. `gts.cf.core.events.event.v1~`) with JSON Schemas generated directly from Rust types and registered in a Types Registry. You can add new event types, settings, model attributes, permissions, or license types **without touching existing gears**. CRUD handlers are customizable via hooks/callbacks implemented as serverless functions or workflows.

### B.7 Composable gears: one codebase, many deployment shapes

A **Gear** is a self-contained unit that owns its API (an SDK crate), owns its data (behind `SecureConn`), is discovered at link time via `inventory`, and composes through a typed `ClientHub` in-process — or the *same* SDK over gRPC out-of-process.

The logical model is identical regardless of the physical boundary. Switching between in-process and out-of-process is a **YAML field** (`runtime.type`), not a code change:

- **Single-node** — all gears in one process → edge, on-prem appliances, dev/test.
- **Multi-node** — gears across processes/machines over gRPC, no orchestrator.
- **Kubernetes** — containerized, full orchestration, cloud-native ops.

> Develop locally single-node → deploy bare-metal → scale to K8s — **no rewrites**.

### B.8 Canonical errors

Gears uses a canonical error taxonomy aligned with the 16 gRPC status categories inspired by the official [gRPC status codes](https://grpc.io/docs/guides/status-codes/) (`NotFound`, `AlreadyExists`, `PermissionDenied`, `InvalidArgument`, `Unauthenticated`, and others). Over HTTP, errors are rendered as **RFC-9457 `Problem`** documents, so REST handlers, SDK boundaries, and future gRPC transports share the same vocabulary. Handlers return typed domain errors; middleware maps them into stable wire responses with trace context.

### B.9 Observability and operational defaults

Gears standardizes operational concerns that are often re-created per service: OpenTelemetry tracing, request IDs, structured logs, health endpoints (`/health`, `/healthz`), timeouts, body limits, CORS/MIME controls, rate limiting, and inflight protection. This gives platform teams a common operational surface across all gears instead of a different observability story per service.

### B.10 FIPS 140-3 support

For regulated deployments, Gears can be built with `--features fips` to route TLS crypto through a FIPS 140-3 validated module per OS: Apple `corecrypto` on macOS, AWS-LC FIPS on Linux, and Windows CNG on Windows. This does not claim that Gears itself is a CMVP-listed module; it means Gears consumes validated cryptographic modules through a controlled TLS provider strategy.

### B.11 Other useful Gears runtime patterns

- **Gear-owned migrations** — gears own their database migrations and run them as part of the runtime lifecycle, so schema ownership follows capability ownership.
- **Cluster primitives** — the cluster system gear provides common cross-instance coordination primitives: distributed cache, leader election, distributed locks, and service discovery. Operators can bind each primitive to the right backend for the deployment (in-process, Postgres, Redis, Kubernetes, NATS, etcd), while consumers keep the same facade-style API and get startup validation when a backend cannot satisfy required guarantees.
- **Transactional outbox** — reliable async message production with per-partition ordering, transactional or leased processing modes, retry/reject semantics, and graceful cancellation.
- **HTTP client** — `toolkit-http` provides a standard outbound HTTP client with rustls TLS, pooling, timeouts, retries with exponential backoff, User-Agent injection, fail-fast concurrency limiting, response size limits, transparent gzip/brotli/deflate decompression, and secure redirect handling with SSRF and credential-leakage protections.
- **SSE streaming** — toolkit support for typed server-sent events gives gears a standard way to expose streaming APIs without inventing one-off protocols.

### B.12 Local-first, shift-left development

Because gears are composable libraries, the **full business logic runs locally** on a developer machine — no Jenkins, Ansible, or K8s required to exercise real logic. Coverage and diff-coverage make it obvious whether tests actually touched changed code. *If it runs locally, it fails locally* — long before CI or a release stage.

---

## When Gears is (and isn't) the right choice

**Choose Gears when you are:**

- A **XaaS / SaaS vendor** building on a governed, multi-tenant backbone.
- A **platform/product team** that wants security and tenancy *for free*.
- A **GenAI builder** needing chat, RAG, model management, agents, tools.
- An **on-prem / edge vendor** shipping single-binary appliances.
- An **enterprise** embedding capabilities into an existing platform via plugins.

**Gears is deliberately *not*:**

- Optimized for minimalism / the absolute lowest barrier to entry — it prioritizes explicit structure, security, governance, and evolvability.
- A ready-to-use catalog of end-user services — it's the *foundation* vendors build on.
- A replacement for cloud infrastructure or PaaS — gears are libraries and building
  blocks.

---

## Get started

```bash
git clone --recurse-submodules https://github.com/constructorfabric/gears-rust
cd gears-rust

make build      # build libs + example server
make example    # run the example server -> http://127.0.0.1:8087/cf/docs

curl http://127.0.0.1:8087/cf/health    # detailed JSON
curl http://127.0.0.1:8087/healthz      # liveness "ok"
```

**Next steps**

- Read the [Architecture Manifest](./ARCHITECTURE_MANIFEST.md) for the full rationale behind the Rust and monorepo choices.
- Browse the [Gears inventory](./GEARS.md) to see what's already built.
- Follow the [Toolkit guide](./toolkit_unified_system/README.md) to build your first
  gear.

---

*Constructor Fabric Gears (Rust) · Apache-2.0 · by the
[Cyber Fabric Foundation](https://www.constructorfabric.org).
Secure · Modular · Composable · GenAI-ready.*
