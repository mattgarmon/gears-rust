// Link this gear so its `#[toolkit::gear]` inventory registration is included in
// the OoP binary.
//
// This binary links ONLY the consumer gear (its own library). It deliberately
// does NOT link the cluster gear: the cluster client is resolved over gRPC from
// the SEPARATE cluster pod, wired by the framework's proxy-wiring phase from
// cluster-sdk's `ConsumerRegistration`. Linking cluster here would make the
// wiring take its local (in-process) branch instead of the remote one.
//
// The gear is anonymous, so no tenant-plane authn stack (authn-resolver /
// static-authn-plugin / types-registry) is linked either.
#![allow(unused_imports)]

use cluster_consumer as _;
