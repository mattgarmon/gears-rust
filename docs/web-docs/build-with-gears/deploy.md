---
title: Deploy Gears
description: Deploy gears across Embedded, Self-Hosted, and K8s Native profiles using the required build and runtime wiring.
sidebar:
  label: Deploy Gears
  order: 14
---

The same business logic can run across three deployment profiles when the required host/worker
binaries, transport support, and runtime configuration are present. For the conceptual model, see
[Deployment shapes](../../concepts/deployment-shapes/).

## Embedded (single-process)

Every gear runs in one process (edge, on-prem, development). Gears talk in-process through `ClientHub`. This is what the quickstart runs — see [Install and run](../).

```yaml
gears:
  my-gear:
    runtime:
      type: local
```

## Self-Hosted (multi-process)

Gears run as independent processes or machines without container orchestration. An OoP gear
self-registers with the DirectoryService, and consumers resolve a remote client behind the same SDK
trait when transport support and deployment wiring are present.

For a worker configuration, see `config/oop-hello.yaml` and [Run a gear out-of-process](../out-of-process/).
The worker uses `oop_http` for its REST
listener and `TOOLKIT_DIRECTORY_ENDPOINT` to reach the platform host's DirectoryService.

## K8s Native

Gears run as containerized services with cluster-native discovery. Each service is an out-of-process
gear plus the system gears it depends on. Cluster-plane coordination primitives (leader election,
distributed locks, distributed cache) are **designed but not yet implemented** — see
[Status and roadmap](../../capabilities/status-and-roadmap/).

## Implementation status

- **Embedded**: implemented.
- **Self-Hosted**: implemented for the currently supported OoP gear set/profile combinations in this repo (see OoP E2E topology and contract-call tests).
- **K8s Native**: supported deployment model, with full gear support still in progress.

## Build considerations

- **FIPS** — build with `--features fips` to route TLS through a validated crypto provider on Linux/macOS/Windows. See [Compliance and FIPS](../../concepts/compliance-and-fips/).
- **Configuration** — deployment differences are expressed in config and environment overrides. See [Configure a Gears application](../configure/).

## See also

- [Deployment shapes](../../concepts/deployment-shapes/) — the model in depth.
- [Runtime and lifecycle](../../concepts/runtime-and-lifecycle/) — startup and shutdown.
