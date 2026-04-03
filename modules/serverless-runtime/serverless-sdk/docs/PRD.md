<!--
Created: 2026-03-30 by Constructor Tech
Updated: 2026-03-30 by Constructor Tech
-->

# PRD — CyberFabric Serverless SDK Core


<!-- toc -->

- [1. Overview](#1-overview)
  - [1.1 Purpose](#11-purpose)
  - [1.2 Background / Problem Statement](#12-background--problem-statement)
  - [1.3 Goals (Business Outcomes)](#13-goals-business-outcomes)
  - [1.4 Glossary](#14-glossary)
- [2. Actors](#2-actors)
  - [2.1 Human Actors](#21-human-actors)
  - [2.2 System Actors](#22-system-actors)
- [3. Operational Concept & Environment](#3-operational-concept--environment)
  - [3.1 Module-Specific Environment Constraints](#31-module-specific-environment-constraints)
- [4. Scope](#4-scope)
  - [4.1 In Scope](#41-in-scope)
  - [4.2 Out of Scope](#42-out-of-scope)
- [5. Functional Requirements](#5-functional-requirements)
  - [5.1 FunctionHandler Trait](#51-functionhandler-trait)
  - [5.2 WorkflowHandler Trait](#52-workflowhandler-trait)
  - [5.3 Invocation Context](#53-invocation-context)
  - [5.4 Environment](#54-environment)
  - [5.5 Error Model](#55-error-model)
  - [5.6 Tracing Instrumentation](#56-tracing-instrumentation)
- [6. Non-Functional Requirements](#6-non-functional-requirements)
  - [6.1 Module-Specific NFRs](#61-module-specific-nfrs)
  - [6.2 NFR Exclusions](#62-nfr-exclusions)
- [7. Public Library Interfaces](#7-public-library-interfaces)
  - [7.1 Public API Surface](#71-public-api-surface)
  - [7.2 External Integration Contracts](#72-external-integration-contracts)
- [8. Use Cases](#8-use-cases)
- [9. Acceptance Criteria](#9-acceptance-criteria)
- [10. Dependencies](#10-dependencies)
- [11. Assumptions](#11-assumptions)
- [12. Risks](#12-risks)
- [13. Traceability](#13-traceability)

<!-- /toc -->

<!--
=============================================================================
PRODUCT REQUIREMENTS DOCUMENT (PRD)
=============================================================================
PURPOSE: Define WHAT the system must do and WHY — business requirements,
functional capabilities, and quality attributes.

SCOPE:
  ✓ Business goals and success criteria
  ✓ Actors (users, systems) that interact with this module
  ✓ Functional requirements (WHAT, not HOW)
  ✓ Non-functional requirements (quality attributes, SLOs)
  ✓ Scope boundaries (in/out of scope)
  ✓ Assumptions, dependencies, risks

NOT IN THIS DOCUMENT (see other templates):
  ✗ Technical architecture, design decisions → DESIGN.md
  ✗ Why a specific technical approach was chosen → ADR/
  ✗ Detailed implementation flows, algorithms → features/

STANDARDS ALIGNMENT:
  - IEEE 830 / ISO/IEC/IEEE 29148:2018 (requirements specification)
  - ISO/IEC 15288 / 12207 (requirements definition)
=============================================================================
-->
## 1. Overview

### 1.1 Purpose

`cf-serverless-sdk-core` is the engine-agnostic, stable Rust library at the heart
of the CyberFabric Serverless SDK. It provides the handler contracts that adapter
developers implement to integrate execution engines (Temporal, Starlark, cloud FaaS,
or any future engine) with the Serverless Runtime.

The crate defines a minimal, opinionated set of traits and types that covers the
complete adapter authoring contract: receiving invocation context, accessing
configuration and secrets, returning typed outputs, implementing durable workflow
compensation, and emitting structured observability events. Adapter crates build
on this foundation without ever modifying it.

### 1.2 Background / Problem Statement

The CyberFabric Serverless Runtime
(`cpt-cf-serverless-runtime-principle-impl-agnostic`) is designed to support
multiple execution engines through a pluggable adapter model. Without a shared,
engine-agnostic SDK core, each adapter would define its own handler contract,
preventing the platform from enforcing a consistent authoring contract, error
model, or observability surface across engines.

A stable, engine-agnostic SDK core solves this by defining the contract once.
Adapter developers implement it; the runtime depends on its types. This enables
adapter portability, consistent error classification, uniform observability, and
a single authoring mental model regardless of the underlying engine technology.

### 1.3 Goals (Business Outcomes)

_Baseline: module is new (no prior implementation). All targets apply at first stable release (v0.1.0)._

- **FunctionHandler portability**: Adapter developers can implement handlers against a single
  contract that works unchanged across any CyberFabric execution engine.
  _Target: zero engine-specific changes required to port a conformant adapter between engines, verified by a shared test suite._
- **Error categorisation completeness**: The SDK error model maps unambiguously to runtime
  `RuntimeErrorCategory` values, enabling correct retry and dead-letter routing without
  adapter-specific error handling.
  _Target: 100% of `ServerlessSdkError` variants carry a documented `RuntimeErrorCategory` mapping; no unmapped (`Unknown`) fallback case._
- **Observability zero-overhead for consumers**: Adapters can instrument every invocation
  with structured tracing spans and timeline events without requiring SDK consumers to add
  any observability code.
  _Target: a conformant `FunctionHandler` implementation contains zero `tracing` imports, verified by compile-time import audit in CI._
- **Toolchain stability**: The crate compiles without unsafe code and with zero engine-specific transitive dependencies, both enforced on every CI run.

### 1.4 Glossary

| Term | Definition |
|------|------------|
| **FunctionHandler** | A Rust type that implements the `FunctionHandler<I, O>` trait to service function invocations. |
| **WorkflowHandler** | A `FunctionHandler` that additionally implements compensation for durable workflow rollback. |
| **Context** | Read-only invocation metadata (ID, tenant, attempt, deadline) derived from `InvocationRecord`. |
| **Environment** | Abstraction over configuration and secret access for a handler invocation. |
| **Compensation** | The rollback contract for durable workflows in CyberFabric. Two layers: function-level compensation is implemented by the adapter author via `WorkflowHandler::compensate` and invoked by the platform as a standard invocation with a `CompensationInput` payload; step-level compensation (sub-step rollback within a workflow execution) is owned by the executor, not the SDK. |
| **Adapter** | A CyberFabric module that implements `ServerlessRuntime` and drives handlers via this SDK. |
| **GTS ID** | An opaque Global Type System identifier string; the SDK carries these as `String` without interpretation. |
| **Timeline Event** | A structured tracing event mapping to `InvocationTimelineEvent` in the runtime domain. |

---

## 2. Actors

### 2.1 Human Actors

#### Function Author

**ID**: `cpt-cf-serverless-sdk-core-actor-fn-author`

- **Role**: A developer who writes functions or workflows on CyberFabric using an
  adapter-provided authoring model (e.g., Starlark scripts, Temporal activities). Does
  not implement SDK traits directly — the adapter mediates between the authoring model
  and `FunctionHandler<I, O>` / `WorkflowHandler<I, O>`. An indirect stakeholder: their
  needs inform what adapters must expose, which in turn shapes SDK contracts.
- **Needs**: A predictable invocation contract (typed I/O, context, error semantics)
  that adapter authors can surface in the adapter's authoring model.

#### Adapter Developer

**ID**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

- **Role**: A platform developer building an adapter crate (e.g., Starlark, Temporal).
  Implements the SDK handler contracts (`FunctionHandler`, `WorkflowHandler`) and
  wires the `trace` module for invocation instrumentation. This is a
  *development-time* relationship: the adapter developer reads documentation,
  implements the handler interfaces, and maps engine-specific data into `Context`
  and `Environment`.
- **Needs**: Ergonomic, well-documented handler contracts they can implement from
  the SDK documentation alone; instrumentation utilities that emit consistent
  timeline events without manual wiring; unambiguous contracts for how `Context`
  and `Environment` are populated from engine data.

### 2.2 System Actors

#### Serverless Runtime

**ID**: `cpt-cf-serverless-sdk-core-actor-runtime`

- **Role**: The CyberFabric Serverless Runtime module
  (`cpt-cf-serverless-runtime-principle-impl-agnostic`) that owns `InvocationRecord`,
  manages invocation lifecycle, and routes invocations to adapters. Unlike the
  Adapter Developer (who implements SDK handler contracts), the runtime is a
  *structural integration consumer*: it depends directly on SDK-defined types
  (`Context`, `ServerlessSdkError`) in its own invocation-routing and
  error-categorisation logic — independently of any particular adapter crate.
- **Needs**: SDK-defined types that remain structurally stable across minor versions
  so that the runtime's routing and error-categorisation logic does not require
  changes when adapters are updated; a consistent mapping from `ServerlessSdkError`
  variants to `RuntimeErrorCategory` for retry and dead-letter routing decisions;
  `Context` fields that map unambiguously and exhaustively from `InvocationRecord`.

---

## 3. Operational Concept & Environment

### 3.1 Module-Specific Environment Constraints

- It must contain **no unsafe code** (enforced by workspace `unsafe_code = "forbid"`).
- It must have **no engine-specific dependencies** (no `temporal-sdk`, `starlark`,
  or similar crates) — ever.
- **Developer experience target** (UX-PRD-001): The primary target users are
  Adapter Developers — platform developers with intermediate async experience. An
  Adapter Developer MUST be able to implement a conformant `FunctionHandler`
  using only the SDK documentation and this PRD, without consulting engine documentation.
- **Compatibility policy**: The crate follows Rust semver conventions. At 0.x all
  public interfaces are considered unstable and may change between minor releases.
  The stability target is 1.0, gated on successful validation by at least one
  production adapter.

---

## 4. Scope

### 4.1 In Scope

- `FunctionHandler<I, O>` trait for stateless function invocations.
- `WorkflowHandler<I, O>` trait extending `FunctionHandler` with compensation.
- `Context` type populated from `InvocationRecord` metadata.
- `Environment` trait for synchronous config and secret access.
- `ServerlessSdkError` typed error enum with `RuntimeErrorCategory` mapping.
- `CompensationInput` and `CompensationTrigger` types for saga compensation.
- `trace` module with adapter-facing instrumentation utilities that emit
  `tracing` spans and timeline events.

### 4.2 Out of Scope

- GTS schema validation or GTS chain parsing — schema ownership belongs to the GTS layer.
- `InvocationStatus` state machine — owned by the runtime.
- `TenantRuntimePolicy`, `Schedule`, `Trigger`, `Webhook` — owned by the runtime.
- Retry policy logic — runtime concern; SDK only exposes `attempt_number` to handlers.


---

## 5. Functional Requirements

### 5.1 FunctionHandler Trait

#### Async Typed FunctionHandler

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-handler-trait`

The crate MUST provide an async `FunctionHandler` contract parameterised over typed input `I`
and typed output `O`. It MUST expose a single invocation method that accepts typed input,
invocation context, and environment access, and returns a typed result or a `ServerlessSdkError`.

- **Rationale**: Provides the typed, adapter-neutral authoring contract for all stateless functions.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

#### FunctionHandler Concurrent-Use Guarantee

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-handler-send-sync`

`FunctionHandler` implementations MUST be safe to share across concurrent invocations dispatched
by adapters running on multi-threaded async runtimes.

- **Rationale**: Adapters run on multi-threaded async runtimes; handler instances must be safely shared.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

### 5.2 WorkflowHandler Trait

#### Workflow Compensation

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-workflow-handler-trait`

The crate MUST provide a `WorkflowHandler` contract that extends `FunctionHandler` with a
compensation method accepting invocation context, environment access, and a `CompensationInput`,
returning success or a `ServerlessSdkError`.

- **Rationale**: Implements the function-level compensation layer of the two-layer saga model
  (`cpt-cf-serverless-runtime-fr-advanced-patterns` BR-133).
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

#### CompensationInput

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-compensation-input`

The crate MUST provide a `CompensationInput` struct that carries all fields from the runtime's
`CompensationContext` (`gts.x.core.serverless.compensation_context.v1~`) required for
idempotent rollback: `trigger`, `original_workflow_invocation_id`, `failed_step_id`,
`failed_step_error` (typed: `error_type`, `message`, `error_metadata`),
`workflow_state_snapshot`, `timestamp`, `function_id`,
`original_input`, `tenant_id`, `correlation_id`, `started_at`.

Field names and optionality MUST match the runtime's `CompensationContext` schema:
`correlation_id` and `started_at` are `Option<String>` (optional in the runtime schema).

- **Rationale**: Compensation handlers need the full context to perform idempotent rollback
  without accessing the runtime domain types.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

### 5.3 Invocation Context

#### Context Populated from InvocationRecord

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-context`

The crate MUST provide a `Context` struct with the following fields:

- From `InvocationRecord`: `invocation_id`, `function_id`, `function_version`, `tenant_id`.
- From `InvocationObservability`: `correlation_id: String` (required in
  `InvocationObservability`), `trace_id: Option<String>`, `span_id: Option<String>`.
- Adapter-supplied: `attempt_number: u32` (1-indexed; the adapter tracks retry count
  independently — the runtime's `InvocationRecord` does not expose an attempt counter,
  though retry count is tracked at the persistence layer).
- Computed: `deadline` derived from `FunctionLimits.timeout_seconds` at invocation start.

- **Rationale**: Maps the runtime's invocation record to the minimal SDK surface handlers need.
  Follows `cpt-cf-serverless-runtime-principle-impl-agnostic`: no engine-specific fields.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

#### Deadline Helpers

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-deadline-helpers`

`Context` MUST expose helpers so handlers can check whether the deadline has been exceeded
and query the remaining time before forced termination.

- **Rationale**: Enables handlers to self-terminate cleanly (returning `ServerlessSdkError::Timeout`)
  rather than being killed mid-operation, which could leave partial state.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

### 5.4 Environment

#### Config and Secret Access

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-environment-trait`

The crate MUST provide an `Environment` contract for synchronous key-based access to
configuration values and secrets. Adapters supply the implementation, populated before
each invocation.

- **Rationale**: Provides engine-agnostic access to function configuration and credentials
  without coupling to credstore APIs or async resolution inside handler logic.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

### 5.5 Error Model

#### ServerlessSdkError Variants

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-error-model`

The crate MUST provide a `ServerlessSdkError` enum with at minimum the following variants
and their documented `RuntimeErrorCategory` mappings:

| Variant | `RuntimeErrorCategory` |
|---------|------------------------|
| `UserError` | `NonRetryable` |
| `InvalidInput` | `NonRetryable` |
| `Timeout` | `Timeout` |
| `NotSupported` | `NonRetryable` |
| `Internal` | `Retryable` |

The error type MUST be extensible — future variants MUST NOT break existing consumers.

Variant semantics: `InvalidInput` is for structural or type constraint violations (checked
before side effects); `UserError` is for business-logic rejections (checked after domain
rules are evaluated). Both map to `NonRetryable` — the distinction is for observability and
caller-facing error messaging, not for retry behaviour.

The runtime's `RuntimeErrorCategory` also defines `ResourceLimit` and `Canceled`. These are
**adapter-only categories** — handlers never produce them. `ResourceLimit` is signalled by the
adapter when tenant quotas or resource limits are exceeded before or during invocation.
`Canceled` is applied by the runtime when an invocation is externally canceled. The SDK error
model intentionally excludes both because they originate outside handler code.

- **Rationale**: Unambiguous mapping from handler errors to runtime retry and dead-letter routing.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

### 5.6 Tracing Instrumentation

#### Adapter-Facing Timeline Instrumentation

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-trace-module`

The crate MUST provide a `trace` module with adapter-facing instrumentation wrappers that
wrap handler invocations in structured spans and emit lifecycle events covering: invocation
start, success, failure, and the compensation equivalents (`compensation_started`,
`compensation_completed`, `compensation_failed`), mapping to `InvocationTimelineEvent` variants.

- **Rationale**: Enables adapters to emit consistent timeline events without requiring SDK
  consumers to add any observability code.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

#### No Consumer-Visible Tracing

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-fr-no-consumer-tracing`

Handler invocation and compensation methods MUST NOT emit any observability events or spans
directly. All instrumentation MUST be contained in the `trace` module and invisible to
SDK consumers.

- **Rationale**: FunctionHandler implementations remain clean and free of platform-specific observability
  wiring; adapters control the observability boundary.
- **Actors**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

---

## 6. Non-Functional Requirements

### 6.1 Module-Specific NFRs

#### No Engine-Specific Dependencies

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-nfr-no-engine-deps`

The crate MUST NOT introduce any direct or transitive dependency on engine-specific crates
(`temporal-sdk`, `starlark`, any cloud FaaS SDK, or similar) at any point in its lifecycle.

- **Threshold**: Zero engine-specific crates in the dependency tree.
- **Rationale**: Enforces `cpt-cf-serverless-runtime-principle-impl-agnostic`; prevents
  accidental coupling that would break adapter portability.


#### Zero Unsafe Code

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-nfr-no-unsafe`

The crate MUST contain no `unsafe` blocks (enforced by `unsafe_code = "forbid"` in the
workspace lint configuration).

- **Threshold**: Zero `unsafe` blocks; workspace lint enforces this.
- **Rationale**: Safety guarantee for handler authors; no soundness risk from SDK internals.

#### Low Per-Invocation Overhead

- [ ] `p2` - **ID**: `cpt-cf-serverless-sdk-core-nfr-low-overhead`

The SDK's contribution to per-handler-invocation overhead MUST be minimal and predictable
for production async workloads.

- **Threshold**: The SDK's per-invocation instrumentation path MUST NOT introduce blocking I/O
  or synchronous computation. Overhead is limited to the async dispatch mechanism and one
  structured span emission per call (see DESIGN.md for rationale).
- **Rationale**: The SDK is on the critical invocation path for all adapters; latency overhead
  accumulates across high-frequency workloads.
- **Verification Method**: Manual review of per-invocation overhead before each release; no automated benchmark currently exists.

#### Public API Documentation

- [ ] `p2` - **ID**: `cpt-cf-serverless-sdk-core-nfr-api-docs`

All public types, traits, and functions MUST have `rustdoc` documentation covering
purpose, usage, and any invariants or panics.

- **Threshold**: `cargo doc --no-deps` produces zero missing-documentation warnings;
  enforced by `#![deny(missing_docs)]` in CI.
- **Rationale**: Adapter Developers must be able to understand and implement the SDK
  contract from the documentation alone, without consulting DESIGN.md or engine
  internals. Aligns with UX-PRD-001 developer-experience target.
- **Verification Method**: `cargo doc --no-deps` in CI; zero missing-doc warnings.

#### Authoring Ergonomics

- [ ] `p2` - **ID**: `cpt-cf-serverless-sdk-core-nfr-authoring-ergonomics`

Adapter Developers MUST be able to implement `FunctionHandler` and `WorkflowHandler`
without knowledge of the SDK's internal async dispatch mechanism. The handler contracts
MUST NOT impose async machinery boilerplate on the implementor beyond writing the handler
logic itself.

- **Rationale**: The SDK's value proposition is that Adapter Developers focus on handler
  dispatch logic, not the underlying async infrastructure. Leaking internal dispatch
  concerns into the implementation contract undermines the developer experience target
  (UX-PRD-001).
- **Verification Method**: SDK examples and integration tests compile without requiring
  knowledge of the async dispatch mechanism; reviewed on every PR that touches handler contracts.

### 6.2 NFR Exclusions

The following checklist domains are explicitly not applicable to this artifact. Absence of
requirements in these areas is deliberate, not an omission.

| Domain | Disposition | Reasoning |
|--------|-------------|-----------|
| Database / persistence | N/A | Pure library; no persistence layer. |
| REST API / HTTP | N/A | No HTTP endpoints; all interfaces are Rust traits. |
| Deployment topology (OPS-PRD-001) | N/A | Library crate; no deployment artifact. |
| Scalability / throughput / geographic distribution (PERF-PRD-002, PERF-PRD-003) | N/A | Pure library with no runtime process, network activity, or data storage. Throughput is determined by the adapter, not by this crate. |
| Availability / uptime SLOs (REL-PRD-001) | N/A | Library crate; availability is determined by the consuming process (adapter). |
| Disaster recovery / RPO / RTO (REL-PRD-002) | N/A | No stateful data; pure library with no persistence to recover. |
| Authentication requirements (SEC-PRD-001) | N/A | No user-facing sessions, no HTTP endpoints, no credential management at the SDK layer. Auth to secret stores is the adapter's responsibility. |
| Authorization / RBAC (SEC-PRD-002) | N/A | No permission model; all actors are trusted internal platform developers. Access control is the adapter/runtime concern. |
| PII / data classification (SEC-PRD-003) | N/A | The SDK does not store, transmit, or inspect data. `CompensationInput` fields and `Environment` secrets are opaque blobs; data classification is the adapter/runtime concern. |
| Audit logging as security concern (SEC-PRD-004) | Addressed via §5.6 | Invocation lifecycle tracing events are emitted per §5.6 (trace module). Security audit logging beyond invocation events is the adapter/runtime concern. |
| Privacy by design (SEC-PRD-005) | N/A | Internal developer tooling; no end-user PII collection. |
| Operational safety / fail-safe (SAFE-PRD-001, SAFE-PRD-002) | N/A | Pure library; no safety-critical operations, no physical systems interface, no hazards. |
| Monitoring / alerting / log retention (OPS-PRD-002) | N/A | Pure library; monitoring configuration is the adapter/runtime concern. The `trace` module provides the observability hooks; consumers configure retention. |
| UX accessibility — WCAG (UX-PRD-002) | N/A | No end-user UI; developer SDK only. |
| UX internationalization (UX-PRD-003) | N/A | No UI or user-facing strings. Error message strings are English-only; acceptable for internal developer tooling. |
| UX inclusivity (UX-PRD-005) | N/A | Developer SDK; no diverse end-user population considerations. |
| Support SLA / support tiers (MAINT-PRD-002) | N/A | Internal library; no external support tier. Issues tracked via the CyberFabric project repository. |
| Regulatory compliance — GDPR / HIPAA / PCI DSS (COMPL-PRD-001) | N/A | Internal Rust library; no regulatory obligations apply directly to this crate. |
| Industry certification standards (COMPL-PRD-002) | N/A | No formal certification required for an internal SDK crate. IEEE/ISO standards referenced in preamble are informational only. |
| Legal / ToS / consent requirements (COMPL-PRD-003) | N/A | Internal tool; no ToS, privacy policy, or consent flows. |
| Data ownership / stewardship (DATA-PRD-001) | N/A | The SDK passes data through as opaque blobs; ownership is the adapter/runtime concern. |
| Data quality requirements (DATA-PRD-002) | N/A | Pure library; no data storage or quality management. |
| Data lifecycle / retention / purge (DATA-PRD-003) | N/A | Pure library; no data persistence, retention, or archival. |

---

## 7. Public Library Interfaces

This crate exposes its public surface as Rust traits and types. For the concrete API
surface, stability classifications, and breaking change policies, see [DESIGN.md](./DESIGN.md).

### 7.1 Public API Surface

- Core handler authoring traits (function and workflow)
- Invocation context and environment access
- Typed error model with runtime error category mapping
- Adapter-facing instrumentation utilities

### 7.2 External Integration Contracts

- Invocation record → context mapping (adapter populates from runtime)
- Compensation context → compensation input mapping (adapter populates from runtime)

---

## 8. Use Cases

#### Adapter Developer Implements a FunctionHandler

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-usecase-impl-handler`

**Actor**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

**Preconditions**:
- `cf-serverless-sdk-core` is a dependency of the adapter crate.
- The function's `IOSchema.params` and `IOSchema.returns` are known.

**Main Flow**:
1. Adapter Developer defines typed input and output types for the function.
2. Adapter Developer implements `FunctionHandler` on a handler struct, bridging the
   engine's execution model (e.g., wrapping a Starlark interpreter or Temporal activity).
3. The `FunctionHandler` receives typed input, reads invocation context and environment,
   and returns a typed result or a `ServerlessSdkError`.
4. The adapter deserialises params, dispatches the `FunctionHandler` via the instrumentation
   wrapper, and persists the result.

**Postconditions**:
- `InvocationRecord.result` contains the serialised output.
- An invocation span with `succeeded` event is emitted.

**Alternative Flows**:
- **`FunctionHandler` returns `UserError`**: `InvocationRecord.error` is set with
  `RuntimeErrorCategory::NonRetryable`; no retry is attempted.
- **`FunctionHandler` returns `Internal`**: `RuntimeErrorCategory::Retryable`; runtime
  may retry per the function's `RetryPolicy`.
- **Deadline exceeded**: `FunctionHandler` checks deadline via context, returns `Timeout`;
  mapped to `RuntimeErrorCategory::Timeout`.

#### Adapter Developer Wires SDK into an Adapter Crate

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-usecase-wire-adapter`

**Actor**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

**Preconditions**:
- `cf-serverless-sdk-core` is a dependency of the adapter crate.
- The adapter has access to an `InvocationRecord` and the runtime's `CompensationContext`.

**Main Flow**:
1. Adapter constructs a `Context` by mapping fields from `InvocationRecord` and `InvocationObservability`.
2. Adapter implements `Environment`, pre-fetching config and secret values before invocation.
3. Adapter resolves the handler instance for the requested function.
4. Adapter dispatches the invocation through the instrumentation wrapper for automatic tracing.
5. Adapter receives the result or error, maps the error variant to `RuntimeErrorCategory`, and persists the outcome in `InvocationRecord`.

**Postconditions**:
- `InvocationRecord` is updated with the serialised result or mapped error category.
- Lifecycle span events are emitted without any handler-side code.

**Alternative Flows**:
- **Compensation trigger**: Adapter constructs `CompensationInput` from `CompensationContext` and dispatches
  it through the compensation instrumentation wrapper on a `WorkflowHandler` instance.

---

#### Adapter Developer Implements Workflow Compensation

- [ ] `p1` - **ID**: `cpt-cf-serverless-sdk-core-usecase-impl-compensation`

**Actor**: `cpt-cf-serverless-sdk-core-actor-adapter-dev`

**Preconditions**:
- Adapter Developer has a `WorkflowHandler` implementation.
- Workflow failed or was canceled; `CompensationInput` is available.

**Main Flow**:
1. Adapter dispatches the `WorkflowHandler` via the compensation instrumentation wrapper.
2. The `WorkflowHandler` checks `input.original_workflow_invocation_id` for idempotency.
3. Reads `input.failed_step_id` to determine rollback scope.
4. Reads `input.workflow_state_snapshot` for completed-step outputs needed for reversal.
5. Performs rollback operations (e.g., refund payment, release inventory).
6. Returns `Ok(())`.

**Postconditions**:
- Original invocation transitions to `compensated`.
- A `serverless.handler.compensate` span with `compensation_completed` event is emitted.

**Alternative Flows**:
- **compensation returns `Err(_)`**: Original invocation transitions to `dead_lettered`.

---

## 9. Acceptance Criteria

- [ ] A function handler can be implemented and unit-tested without depending on any adapter crate.
- [ ] A workflow handler can implement compensation logic using all contextual fields defined
      in the compensation model (as specified in DESIGN.md §3.1).
- [ ] The environment abstraction can be satisfied in handler unit tests without any platform
      infrastructure or async runtime.
- [ ] Every SDK error maps to a documented runtime error category; the public API is fully
      documented with no missing-doc warnings.
- [ ] Instrumentation emits lifecycle events for handler invocations without any
      observability code in the handler implementation.
- [ ] Instrumentation emits lifecycle events for compensation invocations without any
      observability code in the workflow handler implementation.

---

## 10. Dependencies

| Dependency | Description | Criticality |
|------------|-------------|-------------|
| Serialisation / deserialisation | Input/output serialisation for `FunctionHandler` type params and `CompensationInput` fields | p1 |
| JSON value type | Opaque JSON fields in `CompensationInput` (`workflow_state_snapshot`, etc.) | p1 |
| Error derivation | Ergonomic `Display + std::error::Error` derivation for `ServerlessSdkError` | p1 |
| Stable async trait mechanism | Async fn in trait definitions for `FunctionHandler` and `WorkflowHandler` (see DESIGN.md for rationale) | p1 |
| Structured tracing | Span emission in `trace` module | p1 |
| Serverless Runtime DESIGN | Defines `InvocationRecord`, `CompensationContext`, `RuntimeErrorCategory` | p1 |

---

## 11. Assumptions

- Adapters are responsible for deserialising `InvocationRecord.params` into `I` before
  calling `FunctionHandler::call`; this crate does not perform that deserialisation.
- Adapters pre-fetch all required config and secret values before calling the handler;
  `Environment` is synchronous by design.
- GTS type ID strings (`function_id`, `error_type_id`, etc.) are treated as opaque
  identifiers; this crate never parses or validates them.
- `attempt_number` is provided by the adapter (which tracks retry count independently)
  and starts at 1 for the initial attempt. The runtime's `InvocationRecord` does not
  expose an attempt counter field; retry count is tracked at the persistence layer.

There are no open questions at this time.

---

## 12. Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Trait signature changes break downstream handlers | High — any `FunctionHandler` impl stops compiling | Keep crate at 0.x; communicate breaking changes via semver major bump |
| Engine-specific dep accidentally introduced via transitive pull | High — violates `cpt-cf-serverless-sdk-core-nfr-no-engine-deps` | Manual PR review; minimal dependency surface |
| `async-trait` boxing overhead at cold-path invocations | Low — per-invocation allocation in already-async context | Acceptable trade-off for stable ergonomics; revisit when RPITIT Send bound stabilises fully |

---

## 13. Traceability

- **Design**: [DESIGN.md](./DESIGN.md)
- **Serverless Runtime PRD**: [modules/serverless-runtime/docs/PRD.md](../../docs/PRD.md)
- **Serverless Runtime DESIGN**: [modules/serverless-runtime/docs/DESIGN.md](../../docs/DESIGN.md)
