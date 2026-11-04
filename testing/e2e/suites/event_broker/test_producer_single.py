"""Single-event publish scenarios (producer/single/1.01-1.05).

Each test exercises one publish path:
- 1.01: publish → 202, no body (async by default)
- 1.02: sync publish (Prefer: wait=N): 201 once the backend stores the event,
  202 when the wait elapses first, 200 when the event is a duplicate
- 1.03: schema validation failure → 400 (requires EVENT_TYPE_STRICT)
- 1.04: rate limiting → 429 (xfail: rate limiting not configured in standalone)
- 1.05: read-only ``partition`` field rejected → 400
"""

from __future__ import annotations

import uuid
from datetime import datetime, timezone

import pytest

from .conftest import (
    SKIP_LIMITS,
    EVENT_TYPE_STREAM,
    EVENT_TYPE_STRICT,
    SUBJECT_TYPE,
    TOPIC_STREAM,
    TOPIC_STRICT,
    TRACE_ID,
)


def _base_event(tenant_id: str, event_type: str = EVENT_TYPE_STREAM) -> dict:
    return {
        "id": str(uuid.uuid4()),
        "type": event_type,
        "tenant_id": tenant_id,
        "source": "e2e-test",
        "subject": "s1",
        "subject_type": SUBJECT_TYPE,
        "occurred_at": datetime.now(timezone.utc).isoformat(),
    }


async def test_publish_single_async_returns_202(api):
    """scenario: producer/single/1.01-positive-publish-single-async.md"""
    tenant_id = str(uuid.uuid4())
    async with api() as client:
        resp = await client.post("/events", json=_base_event(tenant_id))
    assert resp.status_code == 202
    assert resp.text == "", "202 Accepted must carry no body"


@pytest.mark.backend_sensitive
async def test_publish_prefer_wait_returns_201_once_stored(api):
    """scenario: producer/single/1.02-positive-publish-sync-wait-persisted.md

    Publish is asynchronous by default (202). A producer that wants to block
    until the backend confirms persistence requests it with the standard
    ``Prefer: wait=N`` header (RFC 7240, N capped by configuration). The broker
    holds the response until the backend stores the event and answers
    ``201 Created`` - the backend persists the event well within the wait.
    """
    tenant_id = str(uuid.uuid4())
    async with api() as client:
        resp = await client.post(
            "/events",
            headers={"Prefer": "wait=10"},
            json=_base_event(tenant_id),
        )
    assert resp.status_code == 201
    assert resp.text == "", "201 Created must carry no body"


@pytest.mark.backend_sensitive
async def test_publish_prefer_wait_zero_times_out_returns_202(api):
    """scenario: producer/single/1.02-positive-publish-sync-wait-persisted.md

    ``Prefer: wait=0`` asks the broker to hold for zero seconds: the event is
    durably enqueued but the backend has not stored it when the (zero) wait
    elapses, so the broker answers ``202 Accepted`` - the event stays enqueued
    and is stored out of band. Exercises the timed-out branch deterministically
    without depending on a slow backend.
    """
    tenant_id = str(uuid.uuid4())
    async with api() as client:
        resp = await client.post(
            "/events",
            headers={"Prefer": "wait=0"},
            json=_base_event(tenant_id),
        )
    assert resp.status_code == 202
    assert resp.text == "", "202 Accepted must carry no body"


@pytest.mark.backend_sensitive
async def test_publish_prefer_wait_duplicate_returns_200(api):
    """scenario: producer/single/1.02-positive-publish-sync-wait-persisted.md

    A synchronous publish of an idempotent duplicate (a lost-ack resend at the
    producer chain's head) is answered ``200 OK`` at once: nothing is written,
    so there is nothing to wait on.
    """
    tenant_id = str(uuid.uuid4())
    event_id = str(uuid.uuid4())
    meta = {"version": 1, "producer_id": None, "previous": 0, "sequence": 1}
    async with api() as client:
        reg = await client.post(
            "/producers",
            json={"mode": "chained", "client_agent": "e2e-test/1.0"},
        )
        assert reg.status_code == 201
        meta["producer_id"] = reg.json()["id"]

        first = await client.post(
            "/events",
            headers={"Prefer": "wait=10"},
            json={**_base_event(tenant_id), "id": event_id, "meta": meta},
        )
        assert first.status_code == 201, "the first publish is stored within the wait"

        # Lost-ack resend of the chain head: a duplicate, answered 200 without waiting.
        resend = await client.post(
            "/events",
            headers={"Prefer": "wait=10"},
            json={**_base_event(tenant_id), "id": event_id, "meta": meta},
        )
    assert resend.status_code == 200
    assert resend.text == "", "200 OK must carry no body"


async def test_schema_validation_failure_returns_422(api):
    """scenario: producer/single/1.03-negative-schema-validation-failure.md

    EVENT_TYPE_STRICT is registered with a data schema that requires
    ``strict_field`` (string).  Publishing without it fails validation: the
    body parsed, its ``data`` does not satisfy the type's schema, so the
    answer is 422 ``SchemaViolation`` (``docs/openapi.yaml``).
    """
    tenant_id = str(uuid.uuid4())
    async with api() as client:
        resp = await client.post(
            "/events",
            json={
                **_base_event(tenant_id, event_type=EVENT_TYPE_STRICT),
                "data": {"missing_the_required_field": True},
            },
        )
    assert resp.status_code == 422
    assert resp.json() == {
        "type": "gts://gts.cf.core.errors.err.v1~cf.core.err.invalid_argument.v1~",
        "title": "Invalid Argument",
        "status": 422,
        "detail": "Request validation failed",
        "instance": "/event-broker/v1/events",
        "trace_id": TRACE_ID,
        "context": {
            "field_violations": [
                {
                    "description": "event data does not satisfy the event type's data schema",
                    "field": "(payload)",
                    "reason": "schema_validation",
                },
            ],
            "resource_name": TOPIC_STRICT,
            "resource_type": "gts.cf.core.events.topic.v1~",
        },
    }


@SKIP_LIMITS
async def test_rate_limited_returns_429(api):
    """scenario: producer/single/1.04-negative-rate-limited.md"""
    tenant_id = str(uuid.uuid4())
    async with api() as client:
        resp = None
        for _ in range(500):
            resp = await client.post("/events", json=_base_event(tenant_id))
            if resp.status_code == 429:
                break
    assert resp is not None and resp.status_code == 429
    body = resp.json()
    assert body == {
        "type": "gts://gts.cf.core.errors.err.v1~cf.core.err.resource_exhausted.v1~",
        "title": "Resource Exhausted",
        "status": 429,
        "detail": body["detail"],
        "instance": body["instance"],
        "trace_id": TRACE_ID,
        "context": body["context"],
    }
    assert "retry-after" in {k.lower() for k in resp.headers}


async def test_readonly_partition_rejected_returns_400(api):
    """scenario: producer/single/1.05-negative-readonly-partition-rejected.md

    ``partition`` is a consumer-facing read-side field; a producer supplying it
    on publish gets a 400 Bad Request.
    """
    tenant_id = str(uuid.uuid4())
    async with api() as client:
        resp = await client.post(
            "/events",
            json={
                **_base_event(tenant_id),
                "partition": 0,  # read-only field; must be rejected
            },
        )
    assert resp.status_code == 400
    body = resp.json()
    assert body == {
        "type": "gts://gts.cf.core.errors.err.v1~cf.core.err.invalid_argument.v1~",
        "title": "Invalid Argument",
        "status": 400,
        "detail": body["detail"],
        "instance": body["instance"],
        "trace_id": TRACE_ID,
        "context": body["context"],
    }
