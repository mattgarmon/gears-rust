use super::*;
use uuid::Uuid;

/// Stable test tenant id used by every conversion test below. Its
/// concrete value is irrelevant to the redaction / variant-mapping
/// invariants under test — only the `tenant_id` field on the
/// emitted `am.idp` log is shaped by it, and these tests don't
/// assert log payload, just the resulting `DomainError` shape.
fn fixture_tenant_id() -> Uuid {
    Uuid::from_u128(0xA11CE)
}

#[test]
fn clean_failure_maps_to_service_unavailable() {
    let err = IdpProvisionFailure::CleanFailure {
        detail: "conn refused".into(),
    }
    .into_domain_error(fixture_tenant_id());
    assert!(matches!(err, DomainError::ServiceUnavailable { .. }));
}

#[test]
fn provision_unsupported_operation_maps_to_unsupported_operation() {
    let err = IdpProvisionFailure::UnsupportedOperation {
        detail: "not supported by provider".into(),
    }
    .into_domain_error(fixture_tenant_id());
    let DomainError::UnsupportedOperation { detail } = err else {
        panic!("expected UnsupportedOperation");
    };
    // Public detail MUST carry the redaction marker and MUST NOT
    // leak the raw provider string.
    assert!(
        detail.contains("detail redacted"),
        "missing redaction marker in public detail: {detail}"
    );
    assert!(
        !detail.contains("not supported by provider"),
        "raw provider string leaked into public detail: {detail}"
    );
}

#[test]
fn provision_ambiguous_maps_to_internal_with_redacted_diagnostic() {
    let err = IdpProvisionFailure::Ambiguous {
        detail: "vendor stack trace with token=secret-LEAK-9f3a7c2e".into(),
    }
    .into_domain_error(fixture_tenant_id());
    let DomainError::Internal { diagnostic, .. } = err else {
        panic!("expected Internal");
    };
    assert!(diagnostic.contains("provider detail redacted"));
    // Pin the redaction contract for vendor-text leaks: even a
    // sentinel-shaped token in `detail` MUST NOT reach the public
    // `Internal::diagnostic` field (which is forwarded verbatim into
    // `Problem.detail` by the canonical-mapping boundary). The
    // symmetric Deprovision-side coverage previously lived in this
    // file too; with `DeprovisionFailureExt` removed (no production
    // callers — see `domain::idp::mod`), the redaction-helper itself
    // is exercised by the Provision tests since both Provision and
    // Deprovision conversions share `redact_provider_detail`.
    assert!(
        !diagnostic.contains("token="),
        "raw vendor token leaked into Internal diagnostic: {diagnostic}"
    );
    assert!(
        !diagnostic.contains("secret-LEAK-9f3a7c2e"),
        "raw vendor sentinel leaked into Internal diagnostic: {diagnostic}"
    );
}

/// Pins the cause-chain contract on the Ambiguous → Internal
/// mapping: `Error::source` MUST surface a typed wrapper so
/// downstream retry-classification / observability consumers can
/// walk the chain, while `Display` on the wrapper MUST NOT carry
/// raw vendor text (only the redacted digest + length already
/// emitted on the `am.idp` log line). Closes deep-review #5.
#[test]
fn provision_ambiguous_chains_redacted_cause_without_leaking_raw_detail() {
    use std::error::Error as _;

    let raw_detail = "vendor stack trace with token=secret-LEAK-c0ffee99";
    let err = IdpProvisionFailure::Ambiguous {
        detail: raw_detail.to_owned(),
    }
    .into_domain_error(fixture_tenant_id());
    let DomainError::Internal { cause, .. } = &err else {
        panic!("expected Internal");
    };
    let cause = cause
        .as_ref()
        .expect("Ambiguous mapping MUST chain a typed cause for retry-classification");
    let source_chain = cause.to_string();
    assert!(
        source_chain.contains("digest=") && source_chain.contains("len="),
        "redacted cause MUST carry digest+len for operator correlation; got: {source_chain}"
    );
    assert!(
        !source_chain.contains("token=") && !source_chain.contains("secret-LEAK-c0ffee99"),
        "raw vendor text leaked into redacted cause Display: {source_chain}"
    );
    // Walkable: `Error::source` returns None at the wrapper (it is
    // the leaf), but the wrapper itself MUST be reachable via
    // `Error::source` on the DomainError envelope (`#[source]`
    // attribute on the `cause` field).
    assert!(
        err.source().is_some(),
        "DomainError::Internal::cause MUST be reachable via Error::source"
    );
}
