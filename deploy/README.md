# CF/Gears Profile 3 (Kubernetes) Demo

A thin, end-to-end demo of the out-of-process (OoP) gear architecture:

- **platform-host** — one pod running the trust-coupled core (authz-resolver,
  tenant-resolver, resource-group, account-management) + system gears
  (gear-orchestrator, types-registry, credstore, **api-gateway** edge,
  **grpc-hub** DirectoryService) + embedded authn-resolver.
- **hello** — a minimal, anonymous REST gear running as its **own pod**. It
  registers with the platform host's DirectoryService; the api-gateway edge
  reverse-proxies external requests to it.
- **users-info** — an **authenticated, database-backed** REST gear running as
  its **own pod**. It authenticates tenant-plane bearer tokens *locally*
  (embedded authn stack), then resolves the central **authz-resolver over REST**
  (via the DirectoryService) for every PEP check, and persists to Postgres. This
  is the full OoP path: authn + remote authz + DB.
- **shared-postgres** — one **shared PostgreSQL** pod serving a **database per
  gear** (`postgres` chart). Each gear owns its own logical database (e.g.
  `users-info` → `usersinfo`); gears never share tables — cross-gear reads go
  through SDK contracts, not SQL. This is the recommended shape as more OoP gears
  are added (vs. one Postgres per gear).

```
curl :8087/hello/v1/ping ──▶ platform-host (api-gateway edge)
                              │  discovers via grpc-hub DirectoryService
                              ▼
                            hello pod (:9091)  ──▶ {"message":"pong","served_by":"hello-oop (pid 1)"}

curl -H 'Authorization: Bearer <jwt>' :8087/users-info/v1/cities
                              │  edge reverse-proxies (forwards the bearer)
                              ▼
                            users-info pod (:9092)
                              │  1. authenticates the bearer locally
                              │  2. PEP → authz-resolver REST (back to platform-host) ──▶ allow + tenant scope
                              │  3. query ──▶ shared-postgres pod (usersinfo db)
                              ▼
                            {"items":[ ... ]}
```

> `users-info` demonstrates the two framework mechanisms that make an
> authenticated OoP gear work: `#[toolkit::consumes(contract =
> AuthZResolverApi, from = "authz-resolver")]` (the blessed remote-dependency
> path — the PEP resolves the PDP lazily from the ClientHub), and the api-gateway
> advertising its in-process REST providers (like authz-resolver) in the
> DirectoryService so other pods can call them.

## Layout

| Path | What |
|------|------|
| `apps/platform-host` | Platform-host binary crate (host mode). |
| `examples/oop-gears/hello/hello` | `hello` gear + `hello-oop` OoP binary (`--features oop_module`). |
| `deploy/docker/platform-host.Dockerfile` | Platform-host image. |
| `deploy/docker/oop-gear.Dockerfile` | Generic per-gear OoP image (parameterized by build args). |
| `deploy/helm/toolkit-common` | Helm **library** chart (Deployment/Service/ConfigMap/SA + SA-token projection). |
| `deploy/helm/platform-host` | Platform-host chart. |
| `deploy/helm/hello` | `hello` OoP-gear chart. |
| `examples/toolkit/users-info/users-info` | `users-info` gear; OoP binary `users-info-oop` is a feature-gated `[[bin]]` (`--features oop_module`) in the gear crate. |
| `deploy/helm/users-info` | `users-info` OoP-gear chart (connects to the shared Postgres; can also bundle its own via `postgres.enabled`). |
| `gears/simple-user-settings/simple-user-settings` | `simple-user-settings` gear; OoP binary `simple-user-settings-oop` is a feature-gated `[[bin]]` in the gear crate. |
| `deploy/helm/simple-user-settings` | `simple-user-settings` OoP-gear chart (connects to the shared Postgres). |
| `gears/file-storage/file-storage` | `file-storage` gear; OoP binary `file-storage-oop` is a feature-gated `[[bin]]` in the gear crate. |
| `deploy/helm/file-storage` | `file-storage` OoP-gear chart (connects to the shared Postgres). |
| `gears/chat-engine/chat-engine` | `chat-engine` gear; OoP binary `chat-engine-oop` is a feature-gated `[[bin]]` in the gear crate. |
| `deploy/helm/chat-engine` | `chat-engine` OoP-gear chart (connects to the shared Postgres). |
| `gears/system/usage-collector/usage-collector` | `usage-collector` gear; OoP binary `usage-collector-oop` is a feature-gated `[[bin]]` in the gear crate. |
| `deploy/helm/usage-collector` | `usage-collector` OoP-gear chart (no database — plugin-owned storage per ADR-0012). |
| `examples/toolkit/api-contracts/api-contracts` | `api-contracts` PaymentApi REST **provider**; its OoP binary (`api-contracts-oop`) is a feature-gated `[[bin]]` (`--features oop_module`) in the gear crate. |
| `examples/toolkit/api-contracts/api-contracts-consumer` | `api-contracts-consumer`; its OoP binary (`api-contracts-consumer-oop`, feature `oop_module`) resolves `PaymentApi` from the provider **pod** over REST (OoP gear-to-gear). |
| `deploy/helm/api-contracts` | `api-contracts` provider OoP-gear chart. |
| `deploy/helm/api-contracts-consumer` | `api-contracts-consumer` OoP-gear chart. |
| `deploy/helm/postgres` | Shared PostgreSQL chart — one server, a database per gear (created by an init script). |
| `deploy/helm/toolkit-platform` | Umbrella chart (platform-host + gears) with `values-{dev,minimal,production}.yaml`. |

## Prerequisites

- Docker
- `minikube` + `kubectl`
- `helm`

## 1. Build images

> **Platform-plane auth (Kubernetes `TokenReview`).** The charts enable
> platform-plane enforcement: grpc-hub validates every DirectoryService RPC's
> `x-toolkit-internal-token` via the K8s `TokenReview` API, and callers attach a
> projected SA token (audience `toolkit-internal`). This requires the `k8s-auth`
> code path, so the images below are built with the `k8s` / `k8s-auth` cargo
> features. See [Platform-plane auth](#platform-plane-auth-tokenreview).

```bash
# Platform-host (dev profile = faster build; drop --build-arg for optimized release).
# CARGO_FEATURES="k8s" compiles the grpc-hub inbound TokenReview validator.
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/platform-host.Dockerfile \
  --build-arg BUILD_PROFILE=dev \
  --build-arg CARGO_FEATURES="k8s" \
  -t ghcr.io/cyberfabric/platform-host:dev .

# hello OoP gear (generic per-gear Dockerfile)
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=hello \
  --build-arg GEAR_BIN=hello-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-hello.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/hello:dev .

# users-info OoP gear (authenticated + DB). The OoP binary is a feature-gated
# [[bin]] in the gear crate, so GEAR_PACKAGE is the gear crate and GEAR_FEATURES
# enables oop_module (+ k8s-auth for the platform plane).
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=users-info \
  --build-arg GEAR_BIN=users-info-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-users-info.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/users-info:dev .

# simple-user-settings OoP gear (authenticated + DB), same shape as users-info.
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=cf-gears-simple-user-settings \
  --build-arg GEAR_BIN=simple-user-settings-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-simple-user-settings.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/simple-user-settings:dev .

# file-storage OoP gear (authenticated + DB), same shape as users-info.
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=cf-gears-file-storage \
  --build-arg GEAR_BIN=file-storage-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-file-storage.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/file-storage:dev .

# chat-engine OoP gear (authenticated + DB), same shape as users-info.
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=cf-chat-engine \
  --build-arg GEAR_BIN=chat-engine-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-chat-engine.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/chat-engine:dev .

# usage-collector OoP gear (authenticated, no DB - plugin-owned storage).
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=cf-gears-usage-collector \
  --build-arg GEAR_BIN=usage-collector-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-usage-collector.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/usage-collector:dev .

# api-contracts PaymentApi REST PROVIDER (authenticated, no DB). The OoP binary
# is a feature-gated [[bin]] in the gear crate (no separate -oop crate), so
# GEAR_PACKAGE is the gear crate and GEAR_FEATURES enables oop_module.
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=cf-api-contracts \
  --build-arg GEAR_BIN=api-contracts-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-api-contracts.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/api-contracts:dev .

# api-contracts-consumer (authenticated, no DB) — calls the provider POD over REST.
# Also a feature-gated [[bin]] in the gear crate.
DOCKER_BUILDKIT=1 docker build \
  -f deploy/docker/oop-gear.Dockerfile \
  --build-arg GEAR_PACKAGE=cf-api-contracts-consumer \
  --build-arg GEAR_BIN=api-contracts-consumer-oop \
  --build-arg GEAR_FEATURES="oop_module,k8s-auth" \
  --build-arg GEAR_CONFIG=config/demo-api-contracts-consumer.yaml \
  --build-arg BUILD_PROFILE=dev \
  -t ghcr.io/cyberfabric/api-contracts-consumer:dev .
```

## 2. Load images into the cluster

```bash
minikube start                    # if not already running
minikube image load ghcr.io/cyberfabric/platform-host:dev
minikube image load ghcr.io/cyberfabric/hello:dev
minikube image load ghcr.io/cyberfabric/users-info:dev
minikube image load ghcr.io/cyberfabric/simple-user-settings:dev
minikube image load ghcr.io/cyberfabric/file-storage:dev
minikube image load ghcr.io/cyberfabric/chat-engine:dev
minikube image load ghcr.io/cyberfabric/usage-collector:dev
minikube image load ghcr.io/cyberfabric/api-contracts:dev
minikube image load ghcr.io/cyberfabric/api-contracts-consumer:dev
```

> **Docker-driver gotcha:** `minikube image load <tag>` may **not** overwrite an
> existing tag if a running container still references the old image (you'll see
> the gear boot the *previous* build). If you rebuild an image, either delete the
> gear's pod first (`kubectl -n cf-demo delete pod -l
> app.kubernetes.io/name=<gear>`) or load via a tarball:
> `docker save <tag> -o /tmp/img.tar && minikube image load /tmp/img.tar`, then
> `kubectl -n cf-demo rollout restart deploy/<gear>`.

## 3. Deploy

```bash
kubectl create namespace cf-demo
helm dependency build deploy/helm/hello
helm dependency build deploy/helm/users-info
helm dependency build deploy/helm/simple-user-settings
helm dependency build deploy/helm/file-storage
helm dependency build deploy/helm/chat-engine
helm dependency build deploy/helm/usage-collector
helm dependency build deploy/helm/api-contracts
helm dependency build deploy/helm/api-contracts-consumer
helm dependency update deploy/helm/toolkit-platform    # packages the sub-charts

helm upgrade --install demo deploy/helm/toolkit-platform \
  -n cf-demo \
  -f deploy/helm/toolkit-platform/values-dev.yaml \
  --timeout 240s

kubectl -n cf-demo get pods
```

> OoP gear pods (`hello`, `users-info`) may restart a few times at first boot:
> they start before the platform-host's grpc-hub is accepting connections, fail
> fast, and Kubernetes restarts them. They become Ready as soon as the
> DirectoryService is up (and, for `users-info`, once its Postgres pod and the
> remote `authz-resolver` dependency resolve). If they stay in `CrashLoopBackOff`
> after the platform-host is `Running`, clear the backoff with
> `kubectl -n cf-demo delete pod -l app.kubernetes.io/name=users-info`.

## 4. Smoke test (edge → OoP)

```bash
kubectl -n cf-demo port-forward svc/platform-host 8087:8087 &

curl -s http://127.0.0.1:8087/healthz            # platform-host edge -> 200
curl -s http://127.0.0.1:8087/hello/v1/ping      # reverse-proxied to the hello pod
# => {"message":"pong","served_by":"hello-oop (pid 1)"}

# users-info: authenticated + remote-authz + Postgres, all through the edge.
# static-authn accept_all maps any non-empty bearer to the platform-root tenant.
TID=00000000-df51-5b42-9538-d2b56b7ee953

curl -s -o /dev/null -w '%{http_code}\n' \
  http://127.0.0.1:8087/users-info/v1/cities                 # no token  -> 401

curl -s -X POST -H 'Authorization: Bearer demo' -H 'Content-Type: application/json' \
  -d "{\"name\":\"Tokyo\",\"country\":\"JP\",\"tenant_id\":\"$TID\"}" \
  http://127.0.0.1:8087/users-info/v1/cities                 # -> 201 Created

curl -s -H 'Authorization: Bearer demo' \
  http://127.0.0.1:8087/users-info/v1/cities                 # -> {"items":[{"name":"Tokyo",...}]}
```

For `hello`, `served_by` is the serving process — proof the request was proxied
across pods. For `users-info`, the `201`/`200` responses prove the full OoP
path: the edge forwarded the bearer to the `users-info` pod, which authenticated
it locally, called the central `authz-resolver` **over REST** for the PEP
decision (visible as `POST /authz-resolver/v1/evaluate` in the platform-host
logs, sourced from the users-info pod IP), and persisted to its own Postgres.

## Platform-plane auth (TokenReview)

The two-plane model separates **tenant-plane** auth (end-user `Authorization:
Bearer` → `SecurityContext`, authenticated at each gear) from **platform-plane**
auth (service-to-service, `x-toolkit-internal-token`). This deployment enforces
the platform plane end-to-end using Kubernetes `TokenReview`.

**How it works**

- Each pod (platform-host + every OoP gear) mounts a **projected ServiceAccount
  token** with audience `toolkit-internal` at
  `/var/run/secrets/tokens/toolkit-internal/token` (`saToken.enabled` in the
  charts).
- Every gRPC **caller** of the DirectoryService attaches that token:
  - OoP gears via `oop_http.internal_auth: { provider: kube, token_path: ... }`.
  - the edge api-gateway proxy via `gateway_proxy.internal_auth`.
- The DirectoryService **receiver** (grpc-hub, in platform-host) validates every
  non-exempt RPC via the K8s `TokenReview` API:
  `grpc-hub.internal_auth: { provider: kube, audiences: [toolkit-internal] }`
  with `internal_auth_enforcement: required`. Health + reflection RPCs are exempt.
- `templates/rbac.yaml` in the platform-host chart binds its ServiceAccount to
  the built-in `system:auth-delegator` ClusterRole so it may submit
  TokenReviews. The `k8s` / `k8s-auth` cargo features compile the TokenReview
  code path (see [Build images](#1-build-images)).

**Verify enforcement**

```bash
# Positive: platform-host logs enforcement + accepted registrations.
kubectl -n cf-demo logs deploy/platform-host | grep "platform-plane enforcement enabled"
kubectl -n cf-demo logs deploy/platform-host | grep "registering gear proxy routes"

# Negative: a caller without a valid token is rejected. Temporarily remove a
# gear's oop_http.internal_auth (e.g. usage-collector) and redeploy; the gear
# cannot register and stays NotReady:
kubectl -n cf-demo logs deploy/usage-collector | grep "Unauthenticated"
# => "directory register_instance failed: gRPC Unauthenticated: missing internal token"
```

> The local loopback path below runs Profile-1-style (no projected tokens); it
> leaves `internal_auth` unset, so grpc-hub runs the pass-through layer. Use the
> `shared_secret` provider to exercise the platform plane without Kubernetes.

## OoP gear-to-gear (REST)

The `hello` and `users-info` paths above show OoP→**host** calls (users-info
resolves the in-host `authz-resolver` over REST). The `api-contracts` pair shows
OoP→**OoP** — one gear pod calling another gear pod over REST, discovered via the
DirectoryService:

- **`api-contracts`** (provider pod) serves the `PaymentApi` REST contract at
  `/api-contracts/v1/...` and registers its endpoint in the DirectoryService.
- **`api-contracts-consumer`** (consumer pod) exposes
  `POST /api-contracts-consumer/v1/charge`. Its handler resolves `dyn PaymentApi`
  from the ClientHub — wired by `#[toolkit::consumes(contract = PaymentApi, from
  = "api-contracts")]` to a **directory-resolving REST client** — and forwards
  the charge. The consumer binary does **not** link the provider, so the call can
  only travel over REST to the provider pod.

```bash
# The consumer's /charge route is authenticated but not .exposed(), so call the
# consumer pod directly (not through the edge).
kubectl -n cf-demo port-forward svc/api-contracts-consumer 9098:9098 &

curl -s -o /dev/null -w '%{http_code}\n' \
  -X POST -H 'Content-Type: application/json' \
  -d '{"amount_cents":1000,"currency":"USD","description":"demo"}' \
  http://127.0.0.1:9098/api-contracts-consumer/v1/charge          # no token -> 401

curl -s -X POST -H 'Authorization: Bearer demo' -H 'Content-Type: application/json' \
  -d '{"amount_cents":1000,"currency":"USD","description":"demo charge"}' \
  http://127.0.0.1:9098/api-contracts-consumer/v1/charge          # -> {"payment_id":"...","status":"pending"}
```

Confirm the hop crossed pods (the provider actually executed the charge):

```bash
kubectl -n cf-demo logs deploy/api-contracts | grep '"method":"charge"'
# => "contract call started" / "contract call succeeded" service=PaymentApi method=charge
kubectl -n cf-demo logs deploy/api-contracts-consumer | grep 'dependency resolved'
# => readiness: dependency resolved dep=api-contracts   (the resolving REST client)
```

## Local (no Kubernetes) end-to-end

Two processes on loopback, same software path:

```bash
# Terminal 1 — platform-host (edge + DirectoryService on TCP :50051)
cargo run -p cf-gears-platform-host -- --config config/demo-host.yaml run

# Terminal 2 — hello OoP gear
TOOLKIT_DIRECTORY_ENDPOINT=http://127.0.0.1:50051 \
  cargo run -p hello --features oop_module --bin hello-oop -- --config config/demo-hello.yaml

# Terminal 2b (optional) — users-info OoP gear (authenticated + DB).
# The local demo config uses per-pod SQLite (no Postgres needed on loopback);
# in Kubernetes the chart points it at a Postgres pod instead.
TOOLKIT_DIRECTORY_ENDPOINT=http://127.0.0.1:50051 \
  cargo run -p users-info --features oop_module --bin users-info-oop -- --config config/demo-users-info.yaml

# Terminal 3
curl http://127.0.0.1:8087/hello/v1/ping   # via edge
curl http://127.0.0.1:9091/hello/v1/ping   # direct to the gear

curl -H 'Authorization: Bearer demo' http://127.0.0.1:8087/users-info/v1/cities   # via edge
curl -H 'Authorization: Bearer demo' http://127.0.0.1:9092/users-info/v1/cities   # direct
```

## Helm presets

| Values file | Contents |
|-------------|----------|
| `values-dev.yaml` | platform-host + hello + users-info + shared Postgres, `pullPolicy: Never` for locally-built images (Postgres uses the public image). |
| `values-minimal.yaml` | platform-host only (no OoP gears). |
| `values-production.yaml` | Scaffold for a registry-pulled prod stack (hardening TODO). |

## Adding another OoP gear

1. Give the gear an OoP binary — a feature-gated `[[bin]]` in the gear crate
   (`src/main.rs` + `src/registered_gears.rs`) behind an `oop_module` feature
   that enables `toolkit/bootstrap`, calling `run_oop_with_options`. This is how
   every OoP gear here is shaped (`hello`, `users-info`, `api-contracts`, ...):
   - **anonymous, no deps** (like `hello`): `oop_module = ["dep:tokio",
     "toolkit/bootstrap"]`.
   - **authenticated / with dependencies** (like `users-info`): make the
     embedded tenant-plane authn stack (`authn-resolver` + `static-authn-plugin`
     + `types-registry`) **optional** deps and gate them behind `oop_module`, so
     the library build stays slim. `registered_gears.rs` links them with
     `use <crate> as _;`.
2. Build its image via `deploy/docker/oop-gear.Dockerfile` (`GEAR_PACKAGE` = the
   gear crate, `GEAR_BIN` = the OoP bin, `GEAR_FEATURES="oop_module,k8s-auth"`).
3. Copy `deploy/helm/hello` (or `deploy/helm/users-info` if it needs a DB) as a
   template, adjust `service.port`, `config.content` (`oop_http.advertise_uri` +
   `gears.<name>` + any `database` block), and the image.
4. Add it to the `toolkit-platform` umbrella `Chart.yaml` dependencies + values.

**If the gear needs a database**, don't bundle its own Postgres — reuse the
shared server: add the gear's database name to `postgres.databases` in the
umbrella values, and set the gear's `postgres` block to `enabled: false`,
`host: shared-postgres`, `database: <its-db>`, `user/password: platform/...`.
The gear owns that database exclusively; it must not read another gear's tables
(use SDK contracts for cross-gear data). See the database-topology guidance in
`docs/arch/database/ADR/0001-cpt-cf-database-adr-object-namespacing.md`.

**Authenticated gears that call the central authz-resolver** additionally need:
the gear must `#[toolkit::consumes(contract = ..., from = "authz-resolver")]` and
resolve the client lazily (e.g. `PolicyEnforcer::from_hub`), and the
platform-host's api-gateway must set `advertise_uri` to its Service DNS so its
in-process REST providers (authz-resolver) are resolvable cross-pod.

## Cleanup

```bash
helm -n cf-demo uninstall demo
kubectl delete namespace cf-demo
# minikube stop   # or: minikube delete
```
