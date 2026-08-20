# OoP Readiness Inventory

Point-in-time sweep of every gear in the repo against out-of-process (OoP)
readiness requirements, including the gears that intentionally remain in
`platform-host`.

**Legend:** 🟢 ready · 🔴 not ready / blocking · 🟡 ready, but from this
session's uncommitted/staged work · ⚪ N/A (requirement doesn't apply to this
gear)

**Columns:**

- **Provides Contract** — *provider* side: does this gear expose a REST/gRPC
  contract that *other* gears can consume cross-process (vs. only an
  in-process trait nothing outside its own process can reach)? (⚪ if nothing
  downstream needs to reach this gear)
- **Consumes Cleanly** — *consumer* side: does this gear reach *its own*
  dependencies via `#[toolkit::consumes]` (works local or remote), rather
  than a compile-time `deps=[...]` link that forces co-location? (⚪ if the
  gear has no gear-to-gear dependencies at all)
- **OoP Binary** — has an `oop_module`-gated bin (or dedicated OoP crate)
- **DB Isolation** — DB-backed gears get their own database
- **Authn Stack** — embedded tenant-plane authn wired (⚪ if the gear is
  anonymous)
- **k8s-auth** — platform-plane TokenReview wired
- **Helm Chart** — standalone deployable chart exists
- **Real SecCtx** — internal calls use real (not synthetic/anonymous)
  credentials — only meaningful for the trust-coupled core

## Trust-coupled core (in `platform-host`)

| Gear | Provides Contract | Consumes Cleanly | OoP Binary | DB Isolation | Authn Stack | k8s-auth | Helm | Real SecCtx | Notes |
|---|---|---|---|---|---|---|---|---|---|
| `authz-resolver` | 🟡 REST (`rest.rs`, added this session, uncommitted) | 🔴 `deps=[types_registry]` | 🔴 none | ⚪ no DB | ⚪ | 🔴 not wired | ⚪ (bundled in platform-host chart) | 🔴 uses `SecurityContext::anonymous()` calling tenant-resolver→resource-group | Has the *provider* contract now — consumed remotely by 6 OoP gears — but can't itself be extracted: blocked by `types_registry` hard-dep + synthetic SecCtx in its own internal chain. |
| `tenant-resolver` | 🔴 none | 🔴 `deps=[types_registry]` | 🔴 none | ⚪ no DB | ⚪ | 🔴 not wired | ⚪ | 🔴 `rg-tr-plugin` reads `resource-group`'s DB directly (bypasses any API) | Same blockers as authz-resolver, but currently has **no** contract at all — build one only once a real consumer needs it. |
| `resource-group` | 🔴 none | 🔴 `deps=[authz_resolver, types_registry]` | 🔴 none | 🟡 pg (shared Postgres, wired this session) | ⚪ | 🔴 not wired | ⚪ | 🔴 has no external SecCtx concept to violate, but nothing calls it except via direct DB/hard-link | The literal missing piece: **no REST contract exists at all.** Unblocks `authz-resolver` + `tenant-resolver` once added. |
| `account-management` | 🔴 none | 🔴 `deps=[authz_resolver, types_registry, resource_group, tenant_resolver]` | 🔴 none | 🟡 pg (shared Postgres, wired this session) | ⚪ | 🔴 not wired | ⚪ | 🔴 background flows run as synthetic `am.system` actor | Two independent blockers: no contract, and the `am.system` credential needs real S2S migration. |

## System / platform plumbing (in `platform-host`)

| Gear | Provides Contract | Consumes Cleanly | OoP Binary | DB Isolation | Authn Stack | k8s-auth | Helm | Notes |
|---|---|---|---|---|---|---|---|---|
| `gear-orchestrator` | 🟢 gRPC `DirectoryService` — consumed by every gear for discovery | ⚪ (no deps) | 🔴 none | ⚪ no DB | ⚪ | 🔴 not wired | ⚪ | It's the discovery mechanism itself — being "OoP" is a category error. Stays in host by definition. |
| `grpc-hub` | ⚪ transport layer, not an app-level contract | ⚪ (no deps) | 🔴 none | ⚪ no DB | ⚪ | 🟡 inbound TokenReview validator added this session | ⚪ | Plumbing other gears use, not a consumer itself. |
| `api-gateway` | ⚪ it's the edge — external HTTP traffic, not `ClientHub` consumption | 🔴 `deps=[grpc_hub, authn_resolver]` | 🔴 none | ⚪ no DB | ⚪ | 🟡 outbound proxy credential (`gateway_proxy.internal_auth`) added this session | ⚪ | Owns the reverse-proxy to OoP gears — has to stay co-located with `grpc_hub`/`authn_resolver` by design. |
| `types-registry` | 🔴 manual REST endpoints exist but **no `#[toolkit::consumes]`-compatible contract** | ⚪ (no deps) | 🔴 none | ⚪ no DB (link-time inventory) | ⚪ | 🔴 not wired | ⚪ | Biggest cross-cutting blocker: `usage-collector`'s residual hard-dep, `oagw`, `bss-rate-provider`, `mini-chat` all need this. |
| `credstore` | 🔴 none | 🔴 `deps=[authz_resolver, tenant_resolver, types_registry]` | 🔴 none | 🟡 pg (shared Postgres, wired this session) | ⚪ | 🔴 not wired | ⚪ | No REST contract at all; blocks `oagw`. |
| `authn-resolver` | ⚪ embeds-per-pod by design, not consumed via `ClientHub` from other gears | 🔴 `deps=[types_registry]` | ⚪ (rides in every OoP bin's `oop_module`) | ⚪ no DB | ⚪ | ❓ not verified | ⚪ | Intentionally never "extracted" — every OoP pod embeds its own copy for zero-latency JWT validation. |

## Fully OoP — done this session (all uncommitted / staged)

| Gear | Provides Contract | Consumes Cleanly | OoP Binary | DB Isolation | Authn Stack | k8s-auth | Helm | Notes |
|---|---|---|---|---|---|---|---|---|
| `hello` | ⚪ nothing downstream consumes it | ⚪ (no deps) |  | ⚪ | ⚪ | 🟡 | 🟡 | Reference minimal gear — fully verified via cluster smoke test. |
| `users-info` | ⚪ nothing downstream consumes it | 🟢 consumes `AuthZResolverApi` via `#[toolkit::consumes]` | 🟡 | 🟡 pg | 🟡 | 🟡 | 🟡 | Fully verified end-to-end this session (POST/GET through the edge). |
| `simple-user-settings` | ⚪ nothing downstream consumes it | 🟢 consumes `AuthZResolverApi` |  | 🟡 pg | 🟡 | 🟡 | 🟡 | Same shape, verified. |
| `file-storage` | ⚪ nothing downstream consumes it | 🟢 consumes `AuthZResolverApi` |  | 🟡 pg | 🟡 | 🟡 | 🟡 | Same shape, verified. |
| `chat-engine` | ⚪ nothing downstream consumes it | 🟢 consumes `AuthZResolverApi` |  | 🟡 pg | 🟡 | 🟡 | 🟡 | Also has its own separate `k8s` (leader-election) feature — unrelated to `k8s-auth`. |
| `usage-collector` | ⚪ nothing downstream consumes it | 🔴 consumes `AuthZResolverApi` cleanly, but `deps=[types_registry]` is still hard | 🟡 | ⚪ (plugin-owned storage) | 🟡 | 🟡 | 🟡 | The one "done" gear that still has a live hard-dep blocker — hits the `types-registry` cross-cutting issue directly. |
| `api-contracts` | 🟢 REST `PaymentApi`/`PaymentApiV2` — consumed by `api-contracts-consumer` | ⚪ (no deps) | 🟡 | ⚪ | 🟡 | 🟡 | 🟡 | OoP↔OoP REST reference pair — verified. |
| `api-contracts-consumer` | ⚪ nothing downstream consumes it | 🟢 consumes `PaymentApi`/`PaymentApiV2` | 🟡 | ⚪ | 🟡 | 🟡 | 🟡 | Deliberately still calls v1 alongside v2 — migration-window reference. |

## Not started (untouched, out of scope)

| Gear | Provides Contract | Consumes Cleanly | OoP Binary | DB Isolation | Authn Stack | k8s-auth | Helm | Notes |
|---|---|---|---|---|---|---|---|---|
| `oagw` | 🔴 none | 🔴 `deps=[types_registry, authz_resolver, credstore, tenant_resolver]` | 🔴 | ⚪ | 🔴 | 🔴 | 🔴 | Was mid-migration this session, reverted per direction — back to baseline, fully out of scope. |
| `mini-chat` | 🔴 none | 🔴 `deps=[types_registry, authn_resolver, authz_resolver, oagw]` | 🔴 | 🔴 pg-capable, not activated | 🔴 | 🔴 | 🔴 | `oagw` hard-dep is the hardest blocker — oagw itself isn't OoP-capable either. |
| `bss-ledger` | 🔴 none | 🔴 `deps=[types_registry, authz_resolver, account_management]` | 🔴 | 🔴 pg-capable, not activated | 🔴 | 🔴 | 🔴 | Hard-deps directly on trust-coupled-core `account_management` — hardest blocker in the whole inventory. |
| `bss-rate-provider` (+`ecb`/`http-json` plugins) | ⚪ n/a, internal only | 🔴 `deps=[types_registry]` | 🔴 | ⚪ | ⚪ anonymous (no REST surface at all) | ⚪ | 🔴 | Simplest remaining blocker — only `types-registry`. |
| `file-parser` | ⚪ nothing downstream consumes it | 🟢 (no `deps` declared) | 🔴 | ⚪ | 🔴 needs embedded authn stack (routes are `.authenticated()`) | 🔴 | 🔴 | No hard deps at all — otherwise a `hello`-shape candidate, just needs the authn-stack + k8s-auth + bin work. |
| `nodes-registry` | 🔴 exposes plain (non-`consumes`-wireable) REST for external callers only; `NodesRegistryClient` itself is never consumed by another gear | 🟢 (no `deps`) | 🔴 | ⚪ | ⚪ anonymous (routes are `.anonymous()`) | 🔴 | 🔴 | **Simplest gear in the whole repo to convert** — no deps, no auth requirement at all. |
| `event-broker` | ⚪ n/a | 🔴 hard **Rust crate dep** on `cluster` (not just `deps=[]`) | 🔴 | ⚪ | ❓ | 🔴 | 🔴 | Links `cluster`'s library directly — can't split without `cluster` gaining a remote API. |
| `cluster` | 🔴 none, and likely shouldn't have one | ⚪ (no deps) | 🔴 | ⚪ | ⚪ | ⚪ | 🔴 | Distributed-coordination primitive (leader election/locks via `postgres-cluster-plugin`) — latency-sensitive; probably meant to stay linked-in wherever needed, not become its own pod. |

## Plugins — always embedded, inherit host's status

`static-authn-plugin`, `oidc-authn-plugin`, `static-authz-plugin`,
`tr-authz-plugin`, `static-tr-plugin`, `single-tenant-tr-plugin`,
`rg-tr-plugin`, `static-credstore-plugin`, `static-idp-plugin`,
`keycloak-idp-plugin`, `noop-usage-collector-plugin`,
`timescaledb-usage-collector-plugin`, `static-mini-chat-audit-plugin`,
`static-mini-chat-model-policy-plugin` — all hard-dep `types_registry` and
ride inside whatever process hosts their parent gear. Not independently
assessable.

## Key takeaways

1. **Single biggest lever:** give `resource-group` a REST contract →
   unblocks `authz-resolver` and `tenant-resolver` (3 of 4 trust-coupled-core
   gears) at once.
2. **Second biggest lever:** `types-registry` `#[toolkit::consumes]` wiring →
   unblocks `usage-collector`'s residual hard-dep, `bss-rate-provider`, and is
   a prerequisite for `mini-chat`.
3. **Easiest untouched wins:** `nodes-registry` (zero deps, zero auth) and
   `file-parser` (zero deps, just needs the authn-stack pattern already
   proven 6 times) — both far easier than `oagw`/`mini-chat`/`bss-ledger`.
4. **Structurally stuck, not just "not started":** `cluster`, `event-broker`
   hard-link their dependency's Rust library directly — that's an
   architecture change, not a mechanical `deps→consumes` swap.
5. **`account-management`'s credential migration** (`am.system` → real S2S)
   is the only blocker in the whole inventory that isn't solvable by "add a
   REST contract" — it needs an actual identity design decision.
