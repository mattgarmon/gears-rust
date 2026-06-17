//! Regression tests pinning the load-bearing invariant of
//! `Service::create_usage_records` PDP dedup:
//!
//! > **For every record, the PDP request composed by
//! > [`authorize_usage_record`] is byte-identical to the one composed by
//! > [`authorize_attribution_tuple`] when fed
//! > [`AttributionTupleKey::from_record(record)`].**
//!
//! If that invariant ever breaks, two records that
//! [`AttributionTupleKey`] groups together could be judged differently
//! by the PDP — a bypass. The structural prevention (the per-tuple
//! composer takes only `&AttributionTupleKey`, so no record field can
//! sneak in) makes the divergence physically impossible at the source
//! level; these tests are the corresponding behavioral pin so any
//! refactor that accidentally re-introduces a `&UsageRecord` dependency
//! in the composer is caught by the suite.

use std::collections::BTreeMap;
use std::sync::Arc;

use authz_resolver_sdk::models::EvaluationRequest;
use rust_decimal::Decimal;
use time::OffsetDateTime;
use toolkit_security::SecurityContext;
use usage_collector_sdk::{
    IdempotencyKey, ResourceRef, SubjectRef, UsageRecord, UsageRecordStatus, UsageTypeGtsId,
};
use uuid::Uuid;

use super::{
    AttributionTupleKey, authorize_attribution_tuple, authorize_usage_record, usage_record,
};
use crate::domain::test_support::{CapturingAllowAllResolver, enforcer_for};

const SAMPLE_GTS_ID: &str = "gts.cf.core.uc.usage_record.v1~cf.mini_chat._.tokens_consumed.v1";

fn ctx() -> SecurityContext {
    SecurityContext::builder()
        .subject_id(Uuid::from_u128(0xA110))
        .subject_tenant_id(Uuid::from_u128(0xB220))
        .subject_type("user")
        .build()
        .expect("authenticated ctx")
}

fn record_with(subject: Option<SubjectRef>) -> UsageRecord {
    UsageRecord {
        uuid: Uuid::from_u128(0x0001),
        gts_id: UsageTypeGtsId::new(SAMPLE_GTS_ID).expect("valid gts_id"),
        tenant_id: Uuid::from_u128(0xC330),
        resource_ref: ResourceRef::new("rsc-eq", "compute.vm").expect("valid resource ref"),
        subject_ref: subject,
        metadata: BTreeMap::new(),
        value: Decimal::from(1),
        idempotency_key: IdempotencyKey::new("idem-eq").expect("valid idempotency key"),
        corrects_id: None,
        status: UsageRecordStatus::Active,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

/// Run both PDP composers against the same record and return the
/// captured `EvaluationRequest`s as JSON values (for stable, transitive
/// equality across `HashMap`-backed property bags).
async fn captured_requests_for(record: &UsageRecord) -> (serde_json::Value, serde_json::Value) {
    let resolver = CapturingAllowAllResolver::new();
    let enforcer =
        enforcer_for(Arc::clone(&resolver) as Arc<dyn authz_resolver_sdk::AuthZResolverClient>);

    authorize_usage_record(&enforcer, &ctx(), record, usage_record::actions::CREATE)
        .await
        .expect("permit");
    let from_record = resolver.take_last_request().expect("first call captured");

    let key = AttributionTupleKey::from_record(record, usage_record::actions::CREATE);
    authorize_attribution_tuple(&enforcer, &ctx(), &key)
        .await
        .expect("permit");
    let from_key = resolver.take_last_request().expect("second call captured");

    (json(&from_record), json(&from_key))
}

fn json(req: &EvaluationRequest) -> serde_json::Value {
    serde_json::to_value(req).expect("EvaluationRequest serializes as JSON")
}

/// Subject-absent: the tuple key carries no subject fields, the request
/// MUST carry no subject `resource_property` either.
#[tokio::test]
async fn key_and_record_compose_byte_identical_pdp_requests_without_subject() {
    let record = record_with(None);
    let (from_record, from_key) = captured_requests_for(&record).await;
    assert_eq!(
        from_record, from_key,
        "PDP request composed from the record MUST equal the one composed from \
         AttributionTupleKey::from_record(record) -- otherwise the dedup grouping in \
         Service::create_usage_records can collapse records the PDP would have \
         judged differently. Drift detected for subject-absent record.",
    );
}

/// Subject present without `subject_type`: only `OWNER_ID` is contributed.
#[tokio::test]
async fn key_and_record_compose_byte_identical_pdp_requests_with_subject_id_only() {
    let record = record_with(Some(
        SubjectRef::new("subject-eq-1", None::<String>).expect("valid subject"),
    ));
    let (from_record, from_key) = captured_requests_for(&record).await;
    assert_eq!(
        from_record, from_key,
        "drift detected for subject-id-only record"
    );
}

/// Subject present WITH `subject_type`: both `OWNER_ID` and
/// `SUBJECT_TYPE` are contributed. This is the maximal-attribute path —
/// drift here would be the worst case.
#[tokio::test]
async fn key_and_record_compose_byte_identical_pdp_requests_with_full_subject() {
    let record = record_with(Some(
        SubjectRef::new("subject-eq-2", Some("service")).expect("valid subject"),
    ));
    let (from_record, from_key) = captured_requests_for(&record).await;
    assert_eq!(
        from_record, from_key,
        "drift detected for subject-with-type record"
    );
}

/// Two records that hash-equal under `AttributionTupleKey` MUST always
/// produce equal PDP requests -- even when their *non*-tuple fields
/// (`uuid`, `gts_id`, `value`, `idempotency_key`, `metadata`,
/// `corrects_id`, `created_at`) differ wildly. This pins the
/// projection-correctness premise of the dedup directly: "share the
/// tuple => share the PDP payload".
#[tokio::test]
async fn equal_tuple_keys_produce_equal_pdp_requests_even_when_non_tuple_fields_differ() {
    let record_a = UsageRecord {
        uuid: Uuid::from_u128(0xAAAA),
        gts_id: UsageTypeGtsId::new(SAMPLE_GTS_ID).expect("valid gts_id"),
        tenant_id: Uuid::from_u128(0xDEAD),
        resource_ref: ResourceRef::new("rsc-shared", "compute.vm").expect("valid resource ref"),
        subject_ref: Some(SubjectRef::new("sub-shared", Some("user")).expect("valid subject")),
        metadata: BTreeMap::new(),
        value: Decimal::from(1),
        idempotency_key: IdempotencyKey::new("idem-A").expect("valid idempotency key"),
        corrects_id: None,
        status: UsageRecordStatus::Active,
        created_at: OffsetDateTime::UNIX_EPOCH,
    };
    let record_b = UsageRecord {
        // Same tuple-key fields …
        tenant_id: record_a.tenant_id,
        resource_ref: record_a.resource_ref.clone(),
        subject_ref: record_a.subject_ref.clone(),
        // … wildly different non-tuple fields:
        uuid: Uuid::from_u128(0xBBBB),
        gts_id: UsageTypeGtsId::new(SAMPLE_GTS_ID).expect("valid gts_id"),
        metadata: BTreeMap::new(),
        value: Decimal::from(-999),
        idempotency_key: IdempotencyKey::new("idem-B-different").expect("valid idempotency key"),
        corrects_id: Some(Uuid::from_u128(0xCCCC)),
        status: UsageRecordStatus::Active,
        created_at: OffsetDateTime::UNIX_EPOCH + time::Duration::hours(24),
    };

    let key_a = AttributionTupleKey::from_record(&record_a, usage_record::actions::CREATE);
    let key_b = AttributionTupleKey::from_record(&record_b, usage_record::actions::CREATE);
    assert_eq!(
        key_a, key_b,
        "test premise: records were constructed to share the attribution tuple",
    );

    let resolver = CapturingAllowAllResolver::new();
    let enforcer =
        enforcer_for(Arc::clone(&resolver) as Arc<dyn authz_resolver_sdk::AuthZResolverClient>);

    authorize_usage_record(&enforcer, &ctx(), &record_a, usage_record::actions::CREATE)
        .await
        .expect("permit");
    let req_a = json(&resolver.take_last_request().expect("captured A"));

    authorize_usage_record(&enforcer, &ctx(), &record_b, usage_record::actions::CREATE)
        .await
        .expect("permit");
    let req_b = json(&resolver.take_last_request().expect("captured B"));

    assert_eq!(
        req_a, req_b,
        "two records that hash-equal under AttributionTupleKey MUST compose \
         identical PDP EvaluationRequests; any per-record field leaking into \
         the PDP payload defeats the dedup's safety property",
    );
}

/// Same attribution attributes, different `action` MUST NOT hash-equal.
#[test]
fn different_actions_yield_distinct_tuple_keys_for_same_attribution() {
    let record = record_with(Some(
        SubjectRef::new("sub-action", Some("user")).expect("valid subject"),
    ));
    let create = AttributionTupleKey::from_record(&record, usage_record::actions::CREATE);
    let deactivate = AttributionTupleKey::from_record(&record, usage_record::actions::DEACTIVATE);
    assert_ne!(
        create, deactivate,
        "action MUST participate in AttributionTupleKey hash/eq; \
         otherwise a batch mixing CREATE and DEACTIVATE for the same \
         tuple would share a single PDP decision and silently bypass \
         per-action policy",
    );
}

// ---------------------------------------------------------------------------
// scope_to_odata_filter — projects AccessScope into ODataQuery filter
// ---------------------------------------------------------------------------

#[cfg(test)]
mod scope_to_odata_tests {
    use toolkit_odata::ast::{CompareOperator, Expr, Value};
    use toolkit_security::{
        AccessScope, InGroupScopeFilter, InTenantSubtreeScopeFilter, ScopeConstraint, ScopeFilter,
        ScopeValue, pep_properties,
    };
    use uuid::Uuid;

    use crate::domain::DomainError;
    use crate::domain::authz::{scope_to_odata_filter, usage_record};

    /// Helper — flatten an Expr to a debuggable s-expression string so
    /// assertions can read at a glance.
    fn fmt_expr(expr: &Expr) -> String {
        match expr {
            Expr::And(a, b) => format!("(and {} {})", fmt_expr(a), fmt_expr(b)),
            Expr::Or(a, b) => format!("(or {} {})", fmt_expr(a), fmt_expr(b)),
            Expr::Not(a) => format!("(not {})", fmt_expr(a)),
            Expr::Compare(lhs, op, rhs) => {
                let op = match op {
                    CompareOperator::Eq => "eq",
                    CompareOperator::Ne => "ne",
                    CompareOperator::Gt => "gt",
                    CompareOperator::Ge => "ge",
                    CompareOperator::Lt => "lt",
                    CompareOperator::Le => "le",
                };
                format!("({} {} {})", op, fmt_expr(lhs), fmt_expr(rhs))
            }
            Expr::In(lhs, vs) => {
                let vs: Vec<_> = vs.iter().map(fmt_expr).collect();
                format!("(in {} [{}])", fmt_expr(lhs), vs.join(" "))
            }
            Expr::Function(name, args) => {
                let args: Vec<_> = args.iter().map(fmt_expr).collect();
                format!("({} {})", name, args.join(" "))
            }
            Expr::Identifier(id) => id.clone(),
            Expr::Value(v) => match v {
                Value::Uuid(u) => format!("uuid:{u}"),
                Value::String(s) => format!("\"{s}\""),
                Value::Bool(b) => format!("{b}"),
                Value::Number(n) => format!("{n}"),
                Value::DateTime(t) => format!("dt:{t}"),
                Value::Date(d) => format!("date:{d}"),
                Value::Time(t) => format!("time:{t}"),
                Value::Null => "null".to_owned(),
            },
        }
    }

    fn uid(seed: u128) -> Uuid {
        Uuid::from_u128(seed)
    }

    #[test]
    fn unconstrained_scope_yields_no_filter() {
        let scope = AccessScope::allow_all();
        let out = scope_to_odata_filter(&scope).expect("allow_all is happy-path");
        assert!(out.is_none(), "allow_all MUST NOT add a filter predicate");
    }

    #[test]
    fn deny_all_scope_lifts_to_authorization_denied() {
        let scope = AccessScope::deny_all();
        let err = scope_to_odata_filter(&scope).expect_err("deny_all -> authz denied");
        assert!(matches!(err, DomainError::AuthorizationDenied { .. }));
    }

    #[test]
    fn single_eq_constraint_projects_to_eq_compare() {
        let tenant = uid(0xAA);
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::eq(
            pep_properties::OWNER_TENANT_ID,
            tenant,
        )]));
        let expr = scope_to_odata_filter(&scope)
            .expect("happy path")
            .expect("non-empty");
        assert_eq!(
            fmt_expr(&expr),
            format!("(eq tenant_id uuid:{tenant})"),
            "OWNER_TENANT_ID must project to `tenant_id eq <uuid>`",
        );
    }

    #[test]
    fn single_in_constraint_projects_to_in_expression() {
        let t1 = uid(1);
        let t2 = uid(2);
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::in_uuids(
            pep_properties::OWNER_TENANT_ID,
            vec![t1, t2],
        )]));
        let expr = scope_to_odata_filter(&scope).unwrap().unwrap();
        assert_eq!(
            fmt_expr(&expr),
            format!("(in tenant_id [uuid:{t1} uuid:{t2}])"),
        );
    }

    #[test]
    fn multi_filter_constraint_ands_within_a_constraint() {
        let tenant = uid(0xA);
        let scope = AccessScope::single(ScopeConstraint::new(vec![
            ScopeFilter::eq(pep_properties::OWNER_TENANT_ID, tenant),
            ScopeFilter::eq(usage_record::PROP_RESOURCE_TYPE, "compute.vm"),
        ]));
        let expr = scope_to_odata_filter(&scope).unwrap().unwrap();
        assert_eq!(
            fmt_expr(&expr),
            format!("(and (eq tenant_id uuid:{tenant}) (eq resource_type \"compute.vm\"))"),
        );
    }

    #[test]
    fn multi_constraint_scope_ors_at_top_level() {
        let t1 = uid(11);
        let t2 = uid(22);
        let scope = AccessScope::from_constraints(vec![
            ScopeConstraint::new(vec![ScopeFilter::eq(pep_properties::OWNER_TENANT_ID, t1)]),
            ScopeConstraint::new(vec![ScopeFilter::eq(pep_properties::OWNER_TENANT_ID, t2)]),
        ]);
        let expr = scope_to_odata_filter(&scope).unwrap().unwrap();
        assert_eq!(
            fmt_expr(&expr),
            format!("(or (eq tenant_id uuid:{t1}) (eq tenant_id uuid:{t2}))"),
        );
    }

    #[test]
    fn tree_predicates_fail_closed() {
        let tenant = uid(0xBEEF);
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::InTenantSubtree(
            InTenantSubtreeScopeFilter::new(pep_properties::OWNER_TENANT_ID, tenant),
        )]));
        let err = scope_to_odata_filter(&scope).expect_err("tree filter -> deny");
        assert!(matches!(err, DomainError::AuthorizationDenied { .. }));

        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::InGroup(
            InGroupScopeFilter::new("owner_id", vec![ScopeValue::Uuid(uid(1))]),
        )]));
        let err = scope_to_odata_filter(&scope).expect_err("InGroup -> deny");
        assert!(matches!(err, DomainError::AuthorizationDenied { .. }));
    }

    #[test]
    fn unknown_pep_property_fails_closed() {
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::eq(
            "unsupported_prop",
            "x",
        )]));
        let err = scope_to_odata_filter(&scope).expect_err("unknown prop -> deny");
        assert!(matches!(err, DomainError::AuthorizationDenied { .. }));
    }

    #[test]
    fn type_mismatch_on_value_fails_closed() {
        // OWNER_TENANT_ID is UUID-typed; a string-typed value is a type
        // mismatch and MUST fail closed rather than silently coercing.
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::eq(
            pep_properties::OWNER_TENANT_ID,
            ScopeValue::String("not-a-uuid".into()),
        )]));
        let err = scope_to_odata_filter(&scope).expect_err("string->uuid mismatch");
        assert!(matches!(err, DomainError::AuthorizationDenied { .. }));
    }

    #[test]
    fn uuid_carried_as_string_is_accepted() {
        // The Compiler may emit a UUID as a string ScopeValue; the
        // projection MUST accept it as long as the string parses as a
        // valid UUID (mirrors `ScopeValue::as_uuid`'s convention).
        let t1 = uid(0x1234);
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::eq(
            pep_properties::OWNER_TENANT_ID,
            ScopeValue::String(t1.to_string()),
        )]));
        let expr = scope_to_odata_filter(&scope).unwrap().unwrap();
        assert_eq!(fmt_expr(&expr), format!("(eq tenant_id uuid:{t1})"));
    }

    #[test]
    fn string_pep_property_projects_to_string_value() {
        let scope = AccessScope::single(ScopeConstraint::new(vec![ScopeFilter::eq(
            usage_record::PROP_SUBJECT_TYPE,
            "user",
        )]));
        let expr = scope_to_odata_filter(&scope).unwrap().unwrap();
        assert_eq!(fmt_expr(&expr), "(eq subject_type \"user\")");
    }
}
