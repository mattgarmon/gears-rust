---
title: Run a gear out-of-process
description: Run a gear as its own process, with DirectoryService discovery and configured REST or gRPC transport support.
sidebar:
  label: Run a gear out-of-process
  order: 10
---

A gear can run **in the host process** (resolved through `ClientHub` as a direct call) or
**out-of-process (OoP)** as its own process or pod. In the OoP model, the gear serves its REST
surface locally and registers its instance with the platform host's **DirectoryService**. The
api-gateway edge can discover exposed routes and reverse-proxy external traffic. Consumers can use
the same SDK trait locally or remotely only when the contract has transport support and the
deployment is configured for remote discovery, endpoints, and authentication.

This guide follows the runnable examples `examples/toolkit/hello/` and
`examples/toolkit/api-contracts/`.

In practice, this means:

- Build the required host and worker binaries with the selected transport support (REST by default; gRPC where configured).
- Configure discovery, endpoints, and authentication for your deployment.

Business logic can remain unchanged while deployment shape changes.

## The gear remains an ordinary REST gear

An OoP gear uses the same `RestApiCapability` and `OperationBuilder` patterns as an in-process
gear. Routes intended for edge access are marked `.exposed()`. The `hello` example demonstrates
this without making the gear's handlers depend on its process boundary.

## Prepare transport support in the SDK

The remote transport is provided by SDK wiring generated for the contract. The current
implementation resolves remote endpoints through the directory and registers a remote client in
`ClientHub`.

Evidence in the repo:

- `ClientHub::get` resolves registered clients by contract type (`libs/toolkit/src/client_hub.rs`).
- Consumer wiring is declared via `ConsumerRegistration` and resolves REST endpoints through
    `DirectoryEndpointResolver` (`libs/toolkit/src/discovery.rs`).
- OoP E2E validates host+worker topology and REST contract calls
    (`testing/e2e/suites/oop/conftest.py`, `testing/e2e/suites/oop/test_contract_calls.py`).

## Build and run host/worker binaries

Out-of-process mode requires binaries that include the required gear and transport support.
Configuration alone is not sufficient if the process was not built with the needed features.

## Configure discovery, endpoints, and authentication

Remote calls require deployment configuration for discovery and authn/authz. For a locally run
OoP process, `oop_http` configures the worker's REST listener and advertised URI, while
`TOOLKIT_DIRECTORY_ENDPOINT` identifies the platform host's DirectoryService.

```yaml
oop_http:
  listen_addr: "127.0.0.1:9091"
  advertise_uri: "http://127.0.0.1:9091"
  allow_loopback_advertise: true

gears:
  hello:
    config: {}
```

See `config/oop-hello.yaml` for the complete worker configuration. For Kubernetes, use the
per-gear and platform charts under `deploy/helm/`.

:::note[Transport caveat]
The same SDK contract can be used locally or remotely only when the selected transport support is
present in the built binaries and the deployment configuration provides discovery, endpoints, and
authentication.
:::

## Optional gRPC transport

gRPC remains available for contracts/deployments that explicitly select it. The shared runtime and
SDK pattern are documented in `docs/toolkit_unified_system/09_oop_grpc_sdk_pattern.md`.
The current `api-contracts` example also contains an opt-in gRPC projection; the protobuf example
below illustrates the general build-time step:

```protobuf title="my-gear-sdk/proto/my_gear/v1/service.proto"
syntax = "proto3";
package my_gear.v1;

service MyGearService {
    rpc DoSomething(DoSomethingRequest) returns (DoSomethingResponse);
}
message DoSomethingRequest {}
message DoSomethingResponse {}
```

```rust title="my-gear-sdk/build.rs"
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&["proto/my_gear/v1/service.proto"], &["proto"])?;
    Ok(())
}
```

:::note[Directory & discovery]
Out-of-process gears find each other through a directory service. Current OoP E2E coverage in this
repo runs a Host + Workers loopback topology and validates remote REST contract calls.
:::

## Run locally

Start the platform host and worker in separate terminals:

```bash
# Terminal 1: edge and DirectoryService
cargo run -p cf-gears-flight-control -- --config config/oop-flight-control.yaml run

# Terminal 2: standalone hello worker
TOOLKIT_DIRECTORY_ENDPOINT=http://127.0.0.1:50051 \
    cargo run -p hello --features oop_module --bin hello-oop -- --config config/oop-hello.yaml
```

The edge proxies the exposed route; the worker also serves it directly:

```bash
curl http://127.0.0.1:8087/hello/v1/ping
curl http://127.0.0.1:9091/hello/v1/ping
```

## See also

- [Gears & composition](../../concepts/gears-and-composition/) — in-process vs out-of-process.
- Full code: `examples/toolkit/hello/` and `examples/toolkit/api-contracts/`.
- Container and Kubernetes deployment: `deploy/docker/` and `deploy/helm/`.
