---
title: Deployment shapes
description: One codebase, three deployment profiles — Embedded, Self-Hosted, and K8s Native — selected through build and runtime wiring.
sidebar:
  label: Deployment shapes
  order: 14
---

One codebase supports three deployment profiles. Moving gears between profiles requires the
appropriate build outputs, transport support, bootstrap entry point, and deployment wiring for
discovery, endpoints, and authentication; configuration alone does not make a gear remote.

| Profile | Where gears run | How they talk |
|---|---|---|
| **Embedded** (single-node) | one process (edge, on-prem, dev) | in-process via `ClientHub` |
| **Self-Hosted** (multi-process) | across processes/machines, no orchestrator | remote clients via directory-resolved endpoints (REST by default; gRPC where configured) |
| **K8s Native** | containerized services | cluster DNS discovery, external gateways |

## Why one codebase can do this

The [SDK facade + backend pattern](../sdk-and-clienthub/) means a consumer resolves a trait and calls it without knowing
whether the implementation is an in-process adapter or a remote client. With transport support and deployment wiring in
place, the same logical composition of gears carries from a laptop to a cluster without rewriting domain logic. This underpins the
local-first workflow: compose and test gears together in one process, then deploy the same building blocks distributed.

## Status

Embedded and Self-Hosted profiles are implemented for the currently supported OoP gear set and transport wiring in this
repo. The cluster-plane coordination primitives that a large K8s Native deployment relies on — distributed cache,
leader election, distributed locks — are **designed but not yet implemented**. See [Status and roadmap](../../capabilities/status-and-roadmap/) and
the [cluster plane note](../runtime-and-lifecycle/).

## See also

- [Deploy Gears](../../build-with-gears/deploy/) — the how-to for each shape.
- [Run a gear out-of-process](../../build-with-gears/out-of-process/) — the current OoP model and transport options.
- [Configure a Gears application](../../build-with-gears/configure/) — the config knobs.
