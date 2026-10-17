# Retained stream history design

## Goal

Let an operator reopen a finished TCP flow and inspect a bounded tail of what crossed it. Raw sessions should remain legible as hex or UTF-8; built-in and installed decoders should show their parsed messages after disconnect. Live updates continue to work while the flow is open.

## Existing behavior and constraints

- Core already retains parsed `Flow`, `Message`, and `Finding/RuleHit` telemetry in SQLite and serves flow messages over the public traffic API.
- Proxy's existing raw sample ring is explicitly requested, capped at 16 KiB, expires after 30 seconds, and is cleared on Proxy restart.
- `ProxyTelemetry.StreamTelemetry` is lossy and carries parsed events only. It must not become a raw packet stream.
- All sensitive reads require authenticated actor provenance and audit metadata. Core remains the only SQLite owner.
- TCP payload is forwarded unchanged. History capture must not block or affect forwarding; overload can lose history but must report truncation/loss.

## Proposed behavior

1. Add per-service `retain_history`, defaulting on for the existing services and new services. The Services UI exposes a live enable/disable switch. Disabling it discards active in-memory history for that service and prevents new history capture.
2. For enabled TCP services, Proxy keeps a rolling tail of at most 64 KiB in each direction per flow. The buffer is bounded by an active-history flow limit. When a flow closes, Proxy submits the bounded record to a single nonblocking disk-writer queue; queue overflow is visible as `history_truncated`, while relay bytes remain unaffected.
3. Proxy gzip-compresses completed history records on its single disk-writer worker, outside the forwarding path. Files are stored in Proxy's private data directory with a 24-hour TTL, a 100 MiB quota measured on compressed bytes, and oldest-record eviction. The 64 KiB-per-direction / 128 KiB total limits apply before compression; reads must use bounded decompression and reject output above 128 KiB. Records preserve direction and captured byte order, plus decoder/profile id, timestamps, byte counts, and truncation state. A Proxy restart preserves completed records; in-progress flow buffers are lost.
4. Core does not ingest raw payload in the telemetry event stream or SQLite. The existing parsed telemetry path continues to retain decoded messages. Core exposes a bounded authenticated `GET /traffic/flows/{id}/history` that reads completed raw history on demand through a new ProxyDiagnostics RPC. The RPC request names only a known flow; it cannot choose an address or send traffic. The response is at most 128 KiB plus metadata. Missing/expired history returns 404; a flow that is still active returns 409/available=false. Core audits the actor and flow id for every history read.
5. When a saved flow is selected, Traffic Explorer loads its retained parsed messages from Core and its raw history from the bounded endpoint. Raw is rendered as hex by default with a UTF-8 view option. For decoder profiles, parsed messages are replayed in order with direction and time; if no parsed messages exist, the UI says so and offers the captured raw tail. The UI visibly marks per-direction truncation, capture loss, and expiry/unavailability.

## Bounds and failure behavior

- Per flow: 64 KiB per direction (128 KiB total).
- Active flow history buffers: maximum 256 flows (32 MiB total payload, excluding bounded indexing overhead).
- Pending persistence queue: maximum 64 records (8 MiB maximum payload).
- Completed history disk: 100 MiB; oldest completed record evicted first; 24-hour TTL.
- Stored payloads use gzip. Decompression is limited to the known 128 KiB uncompressed maximum to prevent oversized or malformed files from consuming unbounded memory.
- History is best-effort. It is not packet capture or a guaranteed full transcript. Bytes outside the retained tail, flows exceeding the active limit, and records dropped under queue/storage pressure are explicitly marked as incomplete.
- No raw data goes over the continuous telemetry stream. Raw history is retrieved only through an authenticated, audited, size-bounded request for a known completed flow.

## Implementation boundaries

- `hydra-proxy`: bounded per-flow capture, close notification, gzip compression/decompression in the disk worker, durable history store, cleanup and `GetFlowHistory` diagnostics method.
- `hydra-core`: `retain_history` service config validation/audit, public history endpoint, and audit for reads. Keep Core as the only SQLite owner; use existing stored parsed messages for decoder history.
- `hydra-wire`/`contracts/hydra/proxy/v1/proxy.proto`: generated gRPC method and request/reply metadata; do not add payload to `StreamTelemetry`.
- OpenAPI snapshot and Vue Traffic/Services views are updated with the real API and status behavior.
- `retain_history` is part of the JSON service snapshot stored in `control_state`; serde defaults it to `true` so existing snapshots get the approved default without a SQL migration. The HTTP and protobuf service config fields use the same default. The setting is explicit in service configuration, not hidden environment-only behavior.

## Validation

- Proxy tests: direction ordering, ring tail truncation, active and writer queue bounds, gzip round-trip and corrupt/oversized input rejection, compressed disk quota/TTL eviction, restart persistence, permission checks, and capture loss reporting.
- Core tests: per-service toggle and authorization/audit for reads; unavailable/active/expired status mapping; contract bounds.
- Frontend tests: completed-flow selection reloads history, raw defaults to hex, UTF-8 conversion, decoder messages replay, and truncation/empty states.
- Run formatting, workspace check/Clippy, crate and frontend suites, then a two-process smoke test. No tests should send traffic to competition services or run exploits.
- Deploy to team-1 before reporting completion, per the existing deployment instruction; verify health and static assets without probing competition listeners.

## Review notes

- This extends the SPEC's explicitly on-demand raw sample into bounded automatic retention for configured services. It is an intentional product change requested by the operator; the SPEC and `DECISIONS.md` should be updated when implementation begins.
- The chosen limits match the approved in-chat design: 64 KiB each direction per flow, 24-hour TTL, and 100 MiB disk quota. A 256-flow active limit and 64-record writer queue bound transient memory; if a limit is reached, capture degrades visibly and forwarding continues.
- Gzip is at-rest compression only. Compression is performed after a flow closes by the bounded disk worker; decompression uses the fixed uncompressed-size cap. The disk quota is charged against compressed file size.
- Automatic retention stores application bytes from configured competition services. UI/API access is authenticated and audited; filesystem data remains private to Proxy. No encryption-at-rest feature is introduced in this change.

## Implementation result

Implemented the per-service default-on setting, bounded Proxy ring and gzip records, `ProxyDiagnostics.GetFlowHistory`, authenticated/audited Core route, generated OpenAPI/TypeScript contract, and closed-flow hex/UTF-8/ASCII UI. Task 5 added an end-to-end test to `tests/e2e/relay.py`; it runs against temporary loopback TCP/UDP services and validates 409 while active and direction-preserving completed retrieval. The test was run on team-1 against the deployed release binaries because this local execution sandbox denies loopback binds.

The public response represents per-direction truncation as top-level booleans (`truncated_client_to_server` and `truncated_server_to_client`); the Vue view derives the marker for each segment. Missing history uses 404 for absent, expired, evicted, disabled, and unknown records, so the UI explains those causes together rather than distinguishing them. Existing JSON service snapshots use serde defaulting; no SQL migration was needed. No raw payload was added to the continuous telemetry stream.
