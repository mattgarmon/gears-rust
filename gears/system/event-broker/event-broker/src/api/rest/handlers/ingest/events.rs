//! `POST /v1/events`, `POST /v1/events:batch` (`DESIGN.md:584`,
//! `docs/schemas/gts.cf.core.events.event.v1~.schema.json`).

use std::time::Duration;

use axum::Extension;
use axum::body::Bytes;
use toolkit::api::canonical_prelude::*;
use toolkit_security::SecurityContext;

use super::dto::{PublishBatchRequest, PublishEventRequest};
use crate::api::rest::state::HandlerState;
use crate::domain::error::{DomainError, ErrorCode};
use crate::domain::ingest::{BatchAck, PublishAck, PublishRequest, SyncPublishAck};

/// The seconds a `Prefer: wait[=n]` preference asks for (RFC 7240), or `None`
/// when none is present (`Prefer: respond-async` and no header both yield
/// `None`). A `wait` with no parseable `=n` asks for the broker's cap.
fn requested_wait(headers: &axum::http::HeaderMap) -> Option<u32> {
    headers
        .get_all("Prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|token| {
            let mut parts = token.trim().splitn(2, '=');
            let name = parts.next().unwrap_or_default().trim();
            if !name.eq_ignore_ascii_case("wait") {
                return None;
            }
            // A parseable `=n` is the requested seconds; anything else (no
            // value, or not a number) asks for the broker's own cap.
            Some(
                parts
                    .next()
                    .and_then(|n| n.trim().parse::<u32>().ok())
                    .unwrap_or(u32::MAX),
            )
        })
}

/// The duration a `Prefer: wait=n` publish holds for: the requested seconds,
/// clamped to the broker's configured ceiling. `None` when no wait was asked.
fn sync_wait(headers: &axum::http::HeaderMap, cap: Duration) -> Option<Duration> {
    requested_wait(headers).map(|secs| Duration::from_secs(u64::from(secs)).min(cap))
}

/// The HTTP status a synchronous publish's outcome answers (DESIGN.md:1335):
/// `201` once stored, `202` when the wait elapsed first (the events are still
/// enqueued), `200` when every event was a duplicate.
fn sync_status(ack: SyncPublishAck) -> StatusCode {
    match ack {
        SyncPublishAck::Stored => StatusCode::CREATED,
        SyncPublishAck::TimedOut => StatusCode::ACCEPTED,
        SyncPublishAck::Duplicate => StatusCode::OK,
    }
}

/// Parse a publish body ourselves rather than through axum's `Json` extractor.
/// The request DTOs are `#[serde(deny_unknown_fields)]`, so a body carrying a
/// read-only field (`partition`/`sequence`/`sequence_time`) or any unknown key
/// is a malformed request the producer must fix - a `400`, not the `422` axum's
/// `JsonRejection` would emit for the same deserialize failure. `422` is
/// reserved for payload-*schema* violations, which are decided later in the
/// domain against the event type's `data_schema`.
fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, DomainError> {
    serde_json::from_slice::<T>(body).map_err(|err| DomainError::Validation {
        code: ErrorCode::InvalidBody,
        message: format!("invalid JSON body: {err}"),
    })
}

/// # Errors
///
/// Returns the mapped `CanonicalError` for any `DomainError`
/// `IngestService::publish_event` produces (an unauthorized event type or
/// tenant, topic/event-type not found, payload validation, sequence
/// violation, unknown producer), or `400` when the body carries a
/// read-only/unknown field.
pub async fn publish_event(
    Extension(ctx): Extension<SecurityContext>,
    Extension(state): Extension<HandlerState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let req: PublishEventRequest = parse_body(&body)?;
    // `Prefer: wait=N` holds the response until the backend stores the event
    // (`201`) or the wait elapses (`202`); a duplicate is `200`. The default is
    // asynchronous: a single enqueue, answered `202`/`200`.
    if let Some(wait) = sync_wait(&headers, state.sync_wait_max) {
        return Ok(sync_status(
            state
                .ingest
                .publish_event_sync(&ctx, req.try_into()?, wait)
                .await?,
        ));
    }
    Ok(
        match state.ingest.publish_event(&ctx, req.try_into()?).await? {
            // A lost-ack retry of the producer chain's current head: the event
            // is already durable, so nothing new is written and the response
            // carries no body - `200 OK`, distinct from the `202 Accepted` a
            // newly admitted event gets.
            PublishAck::Duplicate(_) => StatusCode::OK,
            PublishAck::Accepted(_) => StatusCode::ACCEPTED,
        },
    )
}

/// Every answer is status only (`docs/openapi.yaml`). A batch is
/// all-or-nothing: any one event's rejection rejects the whole request
/// (`IngestService::publish_batch`, feature 0001 §5.2).
///
/// # Errors
///
/// Returns the mapped `CanonicalError` for any `DomainError`
/// `IngestService::publish_batch` produces (an empty batch, a repeated event
/// id, a malformed run, mixed topics, batch too large, an unauthorized event
/// type or tenant, topic/event-type not found, unknown producer, payload
/// validation, a sequence violation, or an unavailable store), or `400` when
/// the body carries a read-only/unknown field.
pub async fn publish_batch(
    Extension(ctx): Extension<SecurityContext>,
    Extension(state): Extension<HandlerState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let req: PublishBatchRequest = parse_body(&body)?;
    let requests = req
        .events
        .into_iter()
        .map(PublishRequest::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    // `Prefer: wait=N` applies to the batch as a whole, with the status codes
    // of a single publish (feature 0001 §5.1).
    if let Some(wait) = sync_wait(&headers, state.sync_wait_max) {
        return Ok(sync_status(
            state.ingest.publish_batch_sync(&ctx, requests, wait).await?,
        ));
    }
    Ok(match state.ingest.publish_batch(&ctx, requests).await? {
        // Every event a duplicate - a lost-ack resend: nothing new is written
        // (feature 0001 §5.2 "Outcome").
        BatchAck::Duplicate => StatusCode::OK,
        BatchAck::Accepted => StatusCode::ACCEPTED,
    })
}
