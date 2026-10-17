# Retained Stream History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retain a bounded, gzip-compressed tail of configured TCP flows so operators can inspect raw bytes and decoded messages after a connection closes.

**Architecture:** Proxy captures at most 64 KiB per direction in memory and persists completed records with a bounded gzip writer, 24-hour TTL, and 100 MiB compressed-byte quota. Core keeps parsed messages in its existing SQLite store and fetches raw history only on demand through an authenticated, audited REST endpoint and bounded ProxyDiagnostics RPC; continuous telemetry remains parsed events only.

**Tech Stack:** Rust stable, Tokio, tonic/prost, axum, SQLite WAL, gzip via `flate2`, Vue 3/TypeScript/Bun, OpenAPI.

**Spec:** `docs/superpowers/specs/2026-10-17-stream-history-design.md`

## Global Constraints

- Only `hydra-core` owns SQLite and migrations.
- `hydra-proxy` and `hydra-core` are separate permanent binaries and do not depend on each other.
- `ProxyTelemetry.StreamTelemetry` carries bounded parsed events only; it never carries raw packet streams.
- TCP payload forwarding must remain unchanged and must not wait for Core, gRPC, disk, or history backpressure.
- All capture buffers, queues, active history flows, disk records, and decompression output are bounded.
- All sensitive history reads are authenticated and audited with actor and flow provenance.
- Raw bytes are retained only for configured services, in Proxy's private state directory; gzip compression occurs off the relay path.
- Team-1 redeployment is required before reporting completion; smoke checks must not send traffic through competition service listeners or run exploits.

## Review Focus

- Older serialized service snapshots have no `retain_history` field; test that deserialization defaults it to `true`.
- Flow buffers, pending writes, and disk quota are exhausted; test explicit truncation/loss while relay behavior stays independent.
- A corrupt or gzip-bomb history file is read; test rejection before exceeding the 128 KiB decompressed cap.
- The operator disables history while sessions are active; test that active buffers for that service are discarded and no new buffers begin.
- History is requested for active, missing, expired, or unknown flow IDs; test distinct HTTP status behavior and audit actor/target metadata.

---

### Task 1: Add the per-service history setting

**Files:**
- Modify: `crates/hydra-domain/src/lib.rs`
- Modify: `contracts/hydra/proxy/v1/proxy.proto`
- Modify: `crates/hydra-wire/src/lib.rs`
- Modify: `apps/hydra-core/src/modules/services/http.rs`
- Modify: `frontend/src/generated/contracts.d.ts` (regenerate from OpenAPI)
- Modify: `contracts/openapi.yaml` (regenerate from handlers)
- Modify: `frontend/src/features/services/ServiceForm.vue`
- Test: `crates/hydra-domain/src/lib.rs`
- Test: `crates/hydra-wire/src/lib.rs`
- Test: `apps/hydra-core/src/modules/services/tests.rs`
- Test: `frontend/src/features/services/endpoints.test.ts` or a focused `frontend/tests/services.test.ts`

**Interfaces:**
- Add `Service.retain_history: bool` with serde default `true`; existing JSON snapshots therefore remain readable without a SQL migration.
- Add protobuf `ServiceConfig.retain_history = 7` and map it in both `From<&ServiceSnapshot>` and `TryFrom<&ApplyServiceSnapshotRequest>`.
- Add `ServiceInput.retain_history: bool` with HTTP deserialization default `true`; PATCH/POST continue using the same audited service mutation path.
- Service form displays “Retain completed stream history” and submits the boolean.

- [ ] **Step 1: Write failing compatibility and mapping tests.** Assert old service JSON without the field deserializes with `true`, explicit `false` survives JSON and protobuf round trips, and service API input defaults to `true`.
- [ ] **Step 2: Run tests to verify the expected failures.** Run `cargo test -p hydra-domain`, `cargo test -p hydra-wire`, and `cargo test -p hydra-core modules::services::tests`; each new assertion must fail because the field/mapping is absent.
- [ ] **Step 3: Implement the domain, protobuf, Core DTO, and Vue form field.** Use protobuf tag 7; preserve all existing field numbers. Do not add a SQL migration: service snapshots are JSON in `control_state.services` and serde defaulting provides compatibility.
- [ ] **Step 4: Generate and validate contracts.** Run `cargo check -p hydra-wire -p hydra-core`, the focused tests, the repository OpenAPI snapshot check, and `cd frontend && bun test` for the service form tests.
- [ ] **Step 5: Commit the service setting.**

### Task 2: Capture and persist bounded gzip history in Proxy

**Files:**
- Create: `apps/hydra-proxy/src/history/mod.rs`
- Modify: `apps/hydra-proxy/src/lib.rs`
- Modify: `apps/hydra-proxy/src/app.rs`
- Modify: `apps/hydra-proxy/Cargo.toml`
- Modify: `apps/hydra-proxy/src/telemetry/mod.rs`
- Modify: `apps/hydra-proxy/src/state/mod.rs` and service snapshot apply path
- Modify: `apps/hydra-proxy/src/transport/tcp.rs`
- Test: `apps/hydra-proxy/src/history/mod.rs`
- Test: `apps/hydra-proxy/src/state/tests.rs`

**Interfaces:**
- `HistoryStore::open(directory: impl AsRef<Path>) -> anyhow::Result<Arc<HistoryStore>>`
- `HistoryStore::begin(flow_id: String, service_id: String, profile_id: String) -> Option<HistoryWriter>` admits no more than 256 active histories.
- `HistoryWriter::record(&mut self, direction: hydra_protocol_sdk::Direction, bytes: &[u8])` keeps only the last 65,536 bytes per direction and records truncation.
- `HistoryWriter::finish(self, closed_at_ms: i64, bytes_up: u64, bytes_down: u64)` uses `try_send` into a 64-record worker queue; failure increments visible history-loss counters.
- `HistoryStore::get(flow_id: &str) -> anyhow::Result<Option<FlowHistory>>` returns metadata and at most 131,072 uncompressed payload bytes.
- Disk records are gzip-compressed by the worker, use private files, survive Proxy restart, expire after 24 hours, and are evicted oldest-first when compressed files exceed 100 MiB. Decompression must stop at 131,072 bytes and reject invalid gzip data.
- `Telemetry::open` creates a `HistoryWriter` only for TCP services with `retain_history == true`; the relay direction workers call `record` for observed chunks. `Flow::drop` submits the completed record.

- [ ] **Step 1: Write failing store tests.** Cover direction ordering, per-direction tail truncation, active-flow admission, full writer queue loss reporting, gzip round-trip, malformed/oversized gzip rejection, disk quota eviction, TTL cleanup, private file permissions, and completed history surviving reopen.
- [ ] **Step 2: Run the history test module and confirm failures.** Run `cargo test -p hydra-proxy history::tests` before adding implementation.
- [ ] **Step 3: Implement `HistoryStore` and `HistoryWriter`.** Add direct `flate2` dependency; use one bounded Tokio channel and one blocking disk worker. Keep all compression and file I/O off TCP relay tasks. Count quota using compressed file sizes.
- [ ] **Step 4: Add Proxy lifecycle integration.** Open the history store under the Proxy state directory; attach a writer on enabled TCP flow creation; record both relay directions without modifying bytes; finish on drop. Apply service snapshot changes so disabling retention immediately drops active in-memory writers for that service.
- [ ] **Step 5: Run focused verification.** Run `cargo test -p hydra-proxy history::tests`, relevant `cargo test -p hydra-proxy state::tests`, `cargo fmt --all -- --check`, and `cargo clippy -p hydra-proxy --all-targets -- -D warnings`.
- [ ] **Step 6: Commit bounded Proxy history storage.**

### Task 3: Expose completed history through ProxyDiagnostics and Core REST

**Files:**
- Modify: `contracts/hydra/proxy/v1/proxy.proto`
- Modify: `apps/hydra-proxy/src/grpc/diagnostics.rs`
- Modify: `apps/hydra-core/src/modules/traffic/diagnostics.rs`
- Modify: `apps/hydra-core/src/modules/traffic/http.rs`
- Modify: `apps/hydra-core/src/modules/traffic/mod.rs`
- Modify: `apps/hydra-core/src/api/mod.rs` if router state wiring is needed
- Modify: `contracts/openapi.yaml` (regenerate/diff-check)
- Modify: `frontend/src/generated/contracts.d.ts` (regenerate)
- Test: `apps/hydra-proxy/src/grpc/diagnostics.rs`
- Test: `apps/hydra-core/src/modules/traffic/tests.rs`

**Interfaces:**
- Add `ProxyDiagnostics.GetFlowHistory(GetFlowHistoryRequest) returns (GetFlowHistoryReply)`; request contains only `flow_id`; reply includes ordered direction segments, captured and total byte counts, truncation/loss flags, decoder/profile id, and open/close times. Bound total segment bytes to 131,072.
- Proxy checks that the requested flow is known and completed; it never opens a socket or accepts a destination in this method.
- Add authenticated `GET /api/v1/traffic/flows/{id}/history`; it returns a typed history JSON object, `409` while active, `404` when absent/expired/unknown, `503` when Proxy is unavailable, and `401` without valid auth.
- Core records `traffic.history.read` and its result with authenticated actor and flow ID, including failed reads.

- [ ] **Step 1: Write failing RPC/Core endpoint tests.** Assert active/missing/completed status mapping, response byte cap, no destination fields, authentication, and audit rows containing actor plus flow ID.
- [ ] **Step 2: Run focused tests and confirm expected failures.** Run `cargo test -p hydra-proxy grpc::diagnostics` and `cargo test -p hydra-core modules::traffic`.
- [ ] **Step 3: Implement the protobuf RPC and Proxy handler.** Generate Rust through Cargo `OUT_DIR`; do not edit generated code. Return only a stored completed flow, enforce one bounded response.
- [ ] **Step 4: Implement the Core REST handler and audit.** Connect through the existing Unix-socket `proxy_client`, validate flow ID length, map tonic status codes to the specified HTTP statuses, and audit every attempt/result.
- [ ] **Step 5: Regenerate and verify public contracts.** Run protobuf build/check, the focused test suites, and OpenAPI snapshot diff check; ensure generated TypeScript exposes all response fields.
- [ ] **Step 6: Commit the history retrieval API.**

### Task 4: Render retained history in Traffic Explorer

**Files:**
- Modify: `frontend/src/features/traffic/FlowConversation.vue`
- Modify: `frontend/src/features/traffic/FlowCapture.vue` or create `frontend/src/features/traffic/FlowHistory.vue` if a focused component is clearer
- Modify: `frontend/src/features/traffic/traffic.ts`
- Test: `frontend/tests/traffic-history.test.ts`

**Interfaces:**
- `GET /traffic/flows/{id}/history` response becomes a `FlowHistory` TypeScript type containing `segments: { direction: 1 | 2; data: number[]; truncated: boolean }[]`, `protocol_id`, timestamps, byte totals, and `history_truncated`.
- Selecting a closed flow loads messages from the existing `/messages` endpoint and loads raw history once; live SSE continues to update open sessions.
- Raw defaults to hex; UTF-8 view decodes retained segments with replacement characters for invalid sequences. Decoded profiles replay saved parsed messages in original order and direction.
- Empty, expired, unavailable, and truncated history states are distinct and visible.

- [ ] **Step 1: Write failing frontend tests.** Cover closed-flow history fetch, raw hex default, explicit UTF-8 rendering with invalid bytes, decoder message replay, direction filtering, and no-history/truncation states.
- [ ] **Step 2: Run `cd frontend && bun test tests/traffic-history.test.ts` and verify failures.**
- [ ] **Step 3: Implement history API types and rendering.** Avoid polling; refresh through the existing Refresh action. Do not render raw bytes as UTF-8 in Raw mode.
- [ ] **Step 4: Run the focused test and full frontend suite/build.** Run `cd frontend && bun test && bun run build`.
- [ ] **Step 5: Commit the retained Traffic Explorer view.**

### Task 5: Update product docs and verify/redeploy

**Files:**
- Modify: `SPEC.md`
- Modify: `DECISIONS.md`
- Modify: `docs/CUSTOM_DECODERS.md` or `docs/protocol-integration.md` with history behavior and bounds
- Modify: `deploy/README.md` if the history data directory needs an operator note
- Modify: `docs/superpowers/specs/2026-10-17-stream-history-design.md` with final implementation deviations

**Interfaces:**
- Document automatic bounded gzip retention as an explicit change to the previous on-demand-only sample behavior.
- Explain service toggle, 64 KiB/direction tail, 24-hour TTL, 100 MiB compressed quota, loss/truncation semantics, and authenticated audited history retrieval.

- [ ] **Step 1: Add a regression test for old saved service configuration and gzip store reopening.** Keep these in their owning crate test modules from Tasks 1 and 2.
- [ ] **Step 2: Run local verification.** Run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, relevant crate tests, `cd frontend && bun test && bun run build`, contract generation/diff checks, and `git diff --check`.
- [ ] **Step 3: Run the two-process smoke check.** Start Core and Proxy, verify health, toggle retention through the service API, create a local loopback TCP flow (no competition listener), close it, and retrieve its history.
- [ ] **Step 5: Commit docs and final verification record.**

## Self-review

- Spec coverage: limits, defaults, gzip-at-rest, retention/eviction, enabled/disabled behavior, parsed telemetry boundary, history RPC/API, authenticated audit, UI rendering, loss/truncation reporting, tests, and team-1 redeployment each have an owning task.
- Step scan: every task begins with tests, proves RED, implements only its own interfaces, then verifies and commits.
- Type consistency: history direction uses SDK `Direction`; Proto represents it as its enum/integer encoding and REST emits numeric directions matching existing `ParsedMessage`; maximum is consistently 131,072 bytes.
- Review Focus: compatibility is Task 1, active disable is Task 2, storage/gzip limits are Task 2, status/auth/audit are Task 3, and UI states are Task 4.
- Proportion: five implementation tasks map directly to service config, Proxy storage, API retrieval, UI, and ship verification.
