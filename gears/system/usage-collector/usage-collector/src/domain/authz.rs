//! PEP gate and per-resource vocabulary for the usage-collector domain.
//!
//! Per ADR-0001 (`cpt-cf-usage-collector-adr-pdp-centric-authorization`) the
//! collector keeps NO local policy table and NO PDP-decision cache; every
//! decision is delegated to the bound `authz-resolver` client. Catalog
//! resources are platform-global per ADR-0012 / PRD §5.8 (no owning tenant,
//! no resource id, no per-row scoping), so catalog authz is subject-only;
//! the ingestion surface declares per-record attribution attributes
//! (tenant, optional subject, resource type and id) so policy can reason
//! over them. Both surfaces opt out of `require_constraints`, making an
//! unconstrained permit (`allow_all`) a legitimate happy-path outcome.
//!
//! Fail-closed wiring (transport → `AuthorizationUnavailable`, deny /
//! compile-failed → `AuthorizationDenied`) lives here so it cannot drift
//! between call sites.

use authz_resolver_sdk::PolicyEnforcer;
use authz_resolver_sdk::pep::{AccessRequest, ResourceType};
use toolkit_macros::domain_model;
use toolkit_odata::ast;
use toolkit_security::{AccessScope, ScopeFilter, ScopeValue, SecurityContext, pep_properties};
use usage_collector_sdk::UsageRecord;
use uuid::Uuid;

use super::error::DomainError;

/// The full attribution tuple that determines a `UsageRecord` PDP request
/// under a fixed `(SecurityContext, action)` pair.
///
/// **Why this type exists.** The batch ingestion path
/// (`Service::create_usage_records`) deduplicates PDP round-trips by
/// grouping records with byte-identical PDP payloads. Correctness of that
/// dedup hinges on a single invariant: **every field the PDP payload
/// reads MUST be carried by this type, and every field this type carries
/// MUST be read by the payload composer.** If those two field sets ever
/// diverge, records that the PDP would have judged differently could
/// silently share a single decision — a bypass.
///
/// The invariant is enforced **structurally**, not by review: the
/// only PDP-composer entry point ([`authorize_attribution_tuple`]) takes
/// `&AttributionTupleKey` and nothing else, so it physically cannot
/// reference any record field outside this struct. Adding a new PEP
/// attribute therefore requires (a) a new field here, (b) an update to
/// [`AttributionTupleKey::from_record`], and (c) a corresponding
/// `.resource_property(...)` line in `authorize_attribution_tuple` —
/// the type system rejects any edit that touches the composer but not
/// the key.
///
/// `action` participates in the hash/eq contract so a batch carrying
/// records bound to different actions cannot collapse onto a single PDP
/// decision. Today every batch caller passes a constant
/// (`usage_record::actions::CREATE`); promoting `action` into the key
/// makes the safety property hold structurally for any future caller
/// that mixes actions in one batch.
#[domain_model]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AttributionTupleKey {
    tenant_id: Uuid,
    resource_type: String,
    resource_id: String,
    subject_id: Option<String>,
    subject_type: Option<String>,
    action: &'static str,
}

impl AttributionTupleKey {
    /// Extract the tuple key from a record's caller-supplied attribution
    /// fields together with the PEP `action` the batch is authorising.
    /// The set of fields read here MUST match the set of
    /// `resource_property` / action writes in
    /// [`authorize_attribution_tuple`] — the field-by-field structural
    /// mirror is the load-bearing invariant.
    pub(crate) fn from_record(record: &UsageRecord, action: &'static str) -> Self {
        let (subject_id, subject_type) = match record.subject_ref.as_ref() {
            Some(s) => (
                Some(s.subject_id().to_owned()),
                s.subject_type().map(str::to_owned),
            ),
            None => (None, None),
        };
        Self {
            tenant_id: record.tenant_id,
            resource_type: record.resource_ref.resource_type().to_owned(),
            resource_id: record.resource_ref.resource_id().to_owned(),
            subject_id,
            subject_type,
            action,
        }
    }
}

/// PEP vocabulary for the `UsageType` catalog.
///
/// Platform-global resource (ADR-0012 / PRD §5.8): no owning tenant, no
/// resource id, no per-`UsageType` scoping. The PDP authorizes the subject
/// alone and the [`RESOURCE`] declares no attributes.
pub(crate) mod usage_type {
    use authz_resolver_sdk::pep::ResourceType;

    /// PEP resource type for the `UsageType` catalog.
    pub const RESOURCE: ResourceType =
        ResourceType::from_static("gts.cf.core.uc.usage_type.v1~", &[]);

    /// `UsageType` action vocabulary. Renaming any of these is a contract
    /// change against the PDP policy bundle.
    pub mod actions {
        pub const CREATE: &str = "create";
        pub const GET: &str = "get";
        pub const LIST: &str = "list";
        pub const DELETE: &str = "delete";
    }
}

/// PEP vocabulary for the `UsageRecord` ingestion surface.
///
/// The PDP authorizes the subject together with the caller-supplied
/// attribution composites carried on each record: the owning tenant
/// (`UsageRecord::tenant_id` — caller-supplied, never derived from the
/// [`SecurityContext`]), the optional subject reference (`subject_id` plus
/// optional `subject_type` qualifier), and the mandatory resource reference.
/// Property keys are exported as `PROP_*` constants so call sites and policy
/// authors share one vocabulary.
pub(crate) mod usage_record {
    use authz_resolver_sdk::pep::ResourceType;
    use toolkit_security::pep_properties;

    /// PEP attribute key carrying the caller-supplied `resource_type`.
    pub const PROP_RESOURCE_TYPE: &str = "resource_type";

    /// PEP attribute key carrying the caller-supplied `resource_id`.
    pub const PROP_RESOURCE_ID: &str = "resource_id";

    /// PEP attribute key carrying the optional caller-supplied `subject_type`
    /// qualifier (present only when [`usage_collector_sdk::SubjectRef`] is
    /// supplied AND its `subject_type` field is populated).
    pub const PROP_SUBJECT_TYPE: &str = "subject_type";

    /// PEP resource type for the `UsageRecord` ingestion surface. Declares the
    /// attribution-tuple attributes the PDP may key its policy on.
    pub const RESOURCE: ResourceType = ResourceType::from_static(
        "gts.cf.core.uc.usage_record.v1~",
        &[
            pep_properties::OWNER_TENANT_ID,
            pep_properties::OWNER_ID,
            PROP_RESOURCE_TYPE,
            PROP_RESOURCE_ID,
            PROP_SUBJECT_TYPE,
        ],
    );

    /// `UsageRecord` action vocabulary. Renaming any of these is a contract
    /// change against the PDP policy bundle.
    pub mod actions {
        pub const CREATE: &str = "create";
        pub const DEACTIVATE: &str = "deactivate";
        pub const GET: &str = "get";
        pub const LIST: &str = "list";
    }
}

/// Run the PDP check for `(resource_type, action)` and lift the outcome into
/// [`DomainError`].
///
/// Subject-only authz: the request carries no resource attributes and opts
/// out of `require_constraints`, so a permit with no constraints (`allow_all`)
/// is the legitimate happy-path outcome. Deny / transport failure / compile
/// failure fail closed through the existing `From<EnforcerError>` mapping.
///
/// # Errors
///
/// * [`DomainError::AuthorizationDenied`] when the PDP denies or returns an
///   uncompilable constraint shape.
/// * [`DomainError::AuthorizationUnavailable`] when the PDP transport fails.
// @cpt-flow:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1
// @cpt-algo:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2
// @cpt-dod:cpt-cf-usage-collector-dod-foundation-principle-pdp-centric-authorization:p2
// @cpt-dod:cpt-cf-usage-collector-dod-foundation-principle-fail-closed:p2
// @cpt-dod:cpt-cf-usage-collector-dod-foundation-contract-authz-resolver:p1
// @cpt-dod:cpt-cf-usage-collector-dod-foundation-entity-pdp-decision:p1
// @cpt-dod:cpt-cf-usage-collector-dod-foundation-adr-pdp-centric-authorization:p2
pub(crate) async fn authorize(
    enforcer: &PolicyEnforcer,
    // @cpt-begin:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-input
    ctx: &SecurityContext,
    resource_type: &ResourceType,
    action: &str,
    // @cpt-end:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-input
) -> Result<(), DomainError> {
    // @cpt-begin:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-compose-tuple
    // @cpt-begin:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-compose
    // @cpt-begin:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-resolver-call
    // @cpt-begin:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-call
    // @cpt-begin:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-return
    // @cpt-begin:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-return
    enforcer
        .access_scope_with(
            ctx,
            resource_type,
            action,
            None,
            &AccessRequest::new().require_constraints(false),
        )
        .await
        .map(|_| ())
        .map_err(DomainError::from)
    // @cpt-end:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-return
    // @cpt-end:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-return
    // @cpt-end:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-call
    // @cpt-end:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-resolver-call
    // @cpt-end:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-compose
    // @cpt-end:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-compose-tuple
}

/// Run the PDP check for `(usage_record, action)` carrying the caller-supplied
/// (for `CREATE`) or plugin-loaded (for `DEACTIVATE`) attribution-tuple
/// attributes lifted off the [`UsageRecord`]: the owning tenant
/// (`record.tenant_id`), the optional subject reference (its mandatory
/// `subject_id` plus optional `subject_type` qualifier), and the mandatory
/// resource reference. `action` selects the verb the PDP authorizes against
/// (`actions::CREATE` for emission, `actions::DEACTIVATE` for event
/// deactivation); the per-verb PEP vocabulary is identical so policy authors
/// reason over a single attribute set. The `require_constraints(false)`
/// posture mirrors [`authorize`]; an unconstrained permit is the legitimate
/// happy-path outcome.
///
/// # Errors
///
/// * [`DomainError::AuthorizationDenied`] when the PDP denies or returns an
///   uncompilable constraint shape.
/// * [`DomainError::AuthorizationUnavailable`] when the PDP transport fails.
// @cpt-algo:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1
// @cpt-algo:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-fr-tenant-attribution:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-fr-resource-attribution:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-fr-subject-attribution:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-fr-ingestion-authorization:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-entity-resource-ref:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-entity-subject-ref:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-entity-security-context:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-principle-fail-closed:p1
// @cpt-dod:cpt-cf-usage-collector-dod-usage-emission-adr-caller-supplied-attribution:p1
pub(crate) async fn authorize_usage_record(
    enforcer: &PolicyEnforcer,
    ctx: &SecurityContext,
    record: &UsageRecord,
    action: &'static str,
) -> Result<(), DomainError> {
    let key = AttributionTupleKey::from_record(record, action);
    authorize_attribution_tuple(enforcer, ctx, &key).await
}

/// PDP-evaluate an [`AttributionTupleKey`] directly, bypassing
/// [`AttributionTupleKey::from_record`].
///
/// This is the sole composer of the `UsageRecord` PDP request. By taking
/// `&AttributionTupleKey` and no `&UsageRecord`, it makes "the dedup
/// grouping key is a complete description of the PDP payload" a
/// **structural** invariant rather than a coupling between two files —
/// see the type-level docs on [`AttributionTupleKey`].
///
/// # Errors
///
/// Same envelope as [`authorize_usage_record`]:
///
/// * [`DomainError::AuthorizationDenied`] on deny / uncompilable constraint.
/// * [`DomainError::AuthorizationUnavailable`] on PDP transport failure.
pub(crate) async fn authorize_attribution_tuple(
    enforcer: &PolicyEnforcer,
    ctx: &SecurityContext,
    key: &AttributionTupleKey,
) -> Result<(), DomainError> {
    // @cpt-begin:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-compose-tuple
    // @cpt-begin:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-compose
    // @cpt-begin:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1:inst-algo-attrib-compose-tuple
    // @cpt-begin:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-compose-tuple
    let mut request = AccessRequest::new()
        .require_constraints(false)
        .resource_property(pep_properties::OWNER_TENANT_ID, key.tenant_id.to_string())
        .resource_property(usage_record::PROP_RESOURCE_TYPE, key.resource_type.clone())
        .resource_property(usage_record::PROP_RESOURCE_ID, key.resource_id.clone());

    if let Some(subject_id) = key.subject_id.as_ref() {
        request = request.resource_property(pep_properties::OWNER_ID, subject_id.clone());
        if let Some(subject_type) = key.subject_type.as_ref() {
            request =
                request.resource_property(usage_record::PROP_SUBJECT_TYPE, subject_type.clone());
        }
    }
    // @cpt-end:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-compose-tuple
    // @cpt-end:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1:inst-algo-attrib-compose-tuple
    // @cpt-end:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-compose
    // @cpt-end:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-compose-tuple

    // @cpt-begin:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-resolver-call
    // @cpt-begin:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-call
    // @cpt-begin:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1:inst-algo-attrib-pdp-deny
    // @cpt-begin:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1:inst-algo-attrib-pdp-allow
    // @cpt-begin:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-call
    // @cpt-begin:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-deny
    // @cpt-begin:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-fail-closed
    // @cpt-begin:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-allow
    enforcer
        .access_scope_with(ctx, &usage_record::RESOURCE, key.action, None, &request)
        .await
        .map(|_| ())
        .map_err(DomainError::from)
    // @cpt-end:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-allow
    // @cpt-end:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-fail-closed
    // @cpt-end:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-deny
    // @cpt-end:cpt-cf-usage-collector-algo-event-deactivation-operator-pdp-authorization:p1:inst-algo-pdp-call
    // @cpt-end:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1:inst-algo-attrib-pdp-allow
    // @cpt-end:cpt-cf-usage-collector-algo-usage-emission-attribution-and-pdp-authorization:p1:inst-algo-attrib-pdp-deny
    // @cpt-end:cpt-cf-usage-collector-algo-foundation-pdp-authorize:p2:inst-algo-pdp-call
    // @cpt-end:cpt-cf-usage-collector-flow-foundation-pdp-authorize:p1:inst-pdp-resolver-call
}

/// Authorize a `list_usage_records` request and return the compiled
/// [`AccessScope`] for downstream `OData` composition.
///
/// Mirrors [`authorize`] in posture — `require_constraints(false)` so an
/// unconstrained permit (`AccessScope::allow_all`) is a legitimate
/// happy-path outcome and the caller can dispatch the user-supplied
/// filter unchanged. A permit carrying constraints is projected through
/// [`scope_to_odata_filter`] at the call site and AND-merged into the
/// user's filter before plugin dispatch (see
/// [`crate::domain::service::Service::list_usage_records`]).
///
/// The request carries no per-record attribution attributes (LIST is
/// pre-row), so the composed PEP request is action+resource-type only —
/// the PDP returns row-scope narrowing via the `AccessScope`
/// constraints, not via a tuple match.
///
/// # Errors
///
/// * [`DomainError::AuthorizationDenied`] when the PDP denies or returns
///   an uncompilable constraint shape.
/// * [`DomainError::AuthorizationUnavailable`] when the PDP transport
///   fails.
// @cpt-algo:cpt-cf-usage-collector-algo-usage-query-attribution-and-pdp-authorization-on-read:p2
pub(crate) async fn authorize_list_usage_records(
    enforcer: &PolicyEnforcer,
    ctx: &SecurityContext,
) -> Result<AccessScope, DomainError> {
    enforcer
        .access_scope_with(
            ctx,
            &usage_record::RESOURCE,
            usage_record::actions::LIST,
            None,
            &AccessRequest::new().require_constraints(false),
        )
        .await
        .map_err(DomainError::from)
}

/// Project an [`AccessScope`] into an `OData` filter expression over the
/// `UsageRecord` raw-read filter surface.
///
/// Constraints are OR-ed at the [`AccessScope`] level (one constraint per
/// independent access path) and filters within a constraint are AND-ed —
/// see [`AccessScope`] docs. The returned expression mirrors that shape:
/// `(f1 and f2 and ...) or (g1 and g2 and ...)`. PEP property names are
/// translated to the `OData` wire fields declared on
/// [`usage_collector_sdk::UsageRecordFilterField`]:
///
/// | PEP property                          | `OData` field   | Value kind |
/// |---------------------------------------|-----------------|------------|
/// | `pep_properties::OWNER_TENANT_ID`     | `tenant_id`     | UUID       |
/// | `pep_properties::OWNER_ID`            | `subject_id`    | string     |
/// | `usage_record::PROP_RESOURCE_TYPE`    | `resource_type` | string     |
/// | `usage_record::PROP_RESOURCE_ID`      | `resource_id`   | string     |
/// | `usage_record::PROP_SUBJECT_TYPE`     | `subject_type`  | string     |
///
/// Returns `Ok(None)` when the scope is unconstrained ([`AccessScope::is_unconstrained`]):
/// nothing to AND-merge with the caller's filter, equivalent to "no
/// row-scope narrowing requested by PDP" per
/// [`cpt-cf-usage-collector-algo-usage-query-pdp-constraint-composition-v2`].
///
/// # Errors
///
/// * [`DomainError::AuthorizationDenied`] when the scope is deny-all
///   ([`AccessScope::is_deny_all`]) — the PDP explicitly authorized no
///   rows, which is observationally indistinguishable from a deny on
///   this surface.
/// * [`DomainError::AuthorizationDenied`] when a constraint carries a
///   tree predicate ([`ScopeFilter::InGroup`] /
///   [`ScopeFilter::InGroupSubtree`] / [`ScopeFilter::InTenantSubtree`])
///   — `usage_records` is a flat resource without resource-group or
///   tenant-closure membership tables, so a tree predicate cannot be
///   compiled against this plugin's storage. Surfacing a policy
///   shape this gear can't honour as `AuthorizationDenied` is
///   fail-closed by construction.
/// * [`DomainError::AuthorizationDenied`] when a constraint names a
///   PEP property outside the
///   [`usage_record::RESOURCE`] attribute set — same fail-closed
///   rationale.
// @cpt-algo:cpt-cf-usage-collector-algo-usage-query-pdp-constraint-composition-v2:p2
pub(crate) fn scope_to_odata_filter(scope: &AccessScope) -> Result<Option<ast::Expr>, DomainError> {
    if scope.is_unconstrained() {
        return Ok(None);
    }
    if scope.is_deny_all() {
        tracing::warn!(
            target: "authz",
            "PDP returned a deny-all scope on the usage_record query path"
        );
        return Err(DomainError::AuthorizationDenied {
            reason: Some("PDP returned a deny-all scope".to_owned()),
        });
    }

    let mut disjunction: Option<ast::Expr> = None;
    for constraint in scope.constraints() {
        let mut conjunction: Option<ast::Expr> = None;
        for filter in constraint.filters() {
            let predicate = scope_filter_to_expr(filter)?;
            conjunction = Some(match conjunction {
                None => predicate,
                Some(acc) => acc.and(predicate),
            });
        }
        let Some(constraint_expr) = conjunction else {
            // An empty `ScopeConstraint` matches every row — `() AND row =
            // row` — so the outer scope is effectively unconstrained.
            return Ok(None);
        };
        disjunction = Some(match disjunction {
            None => constraint_expr,
            Some(acc) => acc.or(constraint_expr),
        });
    }
    Ok(disjunction)
}

/// Map a single [`ScopeFilter`] to an `OData` [`ast::Expr`].
fn scope_filter_to_expr(filter: &ScopeFilter) -> Result<ast::Expr, DomainError> {
    match filter {
        ScopeFilter::Eq(eq) => {
            let field = pep_property_to_field(eq.property())?;
            let value = scope_value_to_ast(field, eq.value())?;
            Ok(ast::Expr::Compare(
                Box::new(ast::Expr::Identifier(field.name.to_owned())),
                ast::CompareOperator::Eq,
                Box::new(ast::Expr::Value(value)),
            ))
        }
        ScopeFilter::In(in_filter) => {
            let field = pep_property_to_field(in_filter.property())?;
            let values: Vec<ast::Expr> = in_filter
                .values()
                .iter()
                .map(|v| scope_value_to_ast(field, v).map(ast::Expr::Value))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ast::Expr::In(
                Box::new(ast::Expr::Identifier(field.name.to_owned())),
                values,
            ))
        }
        ScopeFilter::InGroup(_)
        | ScopeFilter::InGroupSubtree(_)
        | ScopeFilter::InTenantSubtree(_) => {
            tracing::warn!(
                target: "authz",
                property = %filter.property(),
                "PDP returned an unsupported tree predicate: usage_records is a \
                 flat resource with no resource-group or tenant-closure membership"
            );
            Err(DomainError::AuthorizationDenied {
                reason: Some(format!(
                    "PDP returned an unsupported tree predicate on property `{}`: \
                     usage_records is a flat resource with no resource-group or \
                     tenant-closure membership",
                    filter.property()
                )),
            })
        }
    }
}

/// Wire description of a PDP property's `OData` projection.
#[domain_model]
#[derive(Clone, Copy)]
struct OdataField {
    /// `OData` identifier visible on the [`usage_collector_sdk::UsageRecordFilterField`] surface.
    name: &'static str,
    /// Expected [`ScopeValue`] variant. Anything else lifts to a fail-closed deny.
    kind: OdataFieldKind,
}

#[domain_model]
#[derive(Clone, Copy)]
enum OdataFieldKind {
    Uuid,
    String,
}

fn pep_property_to_field(property: &str) -> Result<OdataField, DomainError> {
    if property == pep_properties::OWNER_TENANT_ID {
        return Ok(OdataField {
            name: "tenant_id",
            kind: OdataFieldKind::Uuid,
        });
    }
    if property == pep_properties::OWNER_ID {
        return Ok(OdataField {
            name: "subject_id",
            kind: OdataFieldKind::String,
        });
    }
    match property {
        usage_record::PROP_RESOURCE_TYPE => Ok(OdataField {
            name: "resource_type",
            kind: OdataFieldKind::String,
        }),
        usage_record::PROP_RESOURCE_ID => Ok(OdataField {
            name: "resource_id",
            kind: OdataFieldKind::String,
        }),
        usage_record::PROP_SUBJECT_TYPE => Ok(OdataField {
            name: "subject_type",
            kind: OdataFieldKind::String,
        }),
        other => {
            tracing::warn!(
                target: "authz",
                property = %other,
                "PDP returned a constraint over an unknown property for the \
                 usage_record resource: refuse to widen scope under an \
                 unrecognised attribute"
            );
            Err(DomainError::AuthorizationDenied {
                reason: Some(format!(
                    "PDP returned a constraint over unknown property `{other}` for the \
                     usage_record resource — refuse to widen scope under an \
                     unrecognised attribute"
                )),
            })
        }
    }
}

fn scope_value_to_ast(field: OdataField, value: &ScopeValue) -> Result<ast::Value, DomainError> {
    match (field.kind, value) {
        (OdataFieldKind::Uuid, ScopeValue::Uuid(u)) => Ok(ast::Value::Uuid(*u)),
        (OdataFieldKind::Uuid, ScopeValue::String(s)) => {
            Uuid::parse_str(s).map(ast::Value::Uuid).map_err(|e| {
                tracing::warn!(
                    target: "authz",
                    field = %field.name,
                    "PDP returned a non-UUID string value for a UUID-typed field"
                );
                DomainError::AuthorizationDenied {
                    reason: Some(format!(
                        "PDP returned a non-UUID value `{s}` for UUID-typed field `{}`: {e}",
                        field.name
                    )),
                }
            })
        }
        (OdataFieldKind::String, ScopeValue::String(s)) => Ok(ast::Value::String(s.clone())),
        (kind, v) => {
            let expected = match kind {
                OdataFieldKind::Uuid => "UUID",
                OdataFieldKind::String => "string",
            };
            let actual = describe_scope_value(v);
            tracing::warn!(
                target: "authz",
                field = %field.name,
                expected = %expected,
                actual = %actual,
                "PDP returned a value of the wrong type for the constraint field"
            );
            Err(DomainError::AuthorizationDenied {
                reason: Some(format!(
                    "PDP returned a {actual} value for field `{}` typed as {expected}",
                    field.name,
                )),
            })
        }
    }
}

fn describe_scope_value(v: &ScopeValue) -> &'static str {
    match v {
        ScopeValue::Uuid(_) => "UUID",
        ScopeValue::String(_) => "string",
        ScopeValue::Int(_) => "integer",
        ScopeValue::Bool(_) => "boolean",
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "authz_tests.rs"]
mod authz_tests;
