# Reversible flag forwarding implementation plan

> Execute inline with superpowers:executing-plans. User approved the written design and requested implementation.

**Goal:** Forward configured services to an allowlisted competition team with durable equal-length flag substitution and response restoration.
**Architecture:** Pure rewrite/framing crate, Proxy-owned durable mapping journal, optional service configuration transferred through existing snapshots. Existing relay inspection receives client-facing bytes; disabled services keep their fast path.
**Tech stack:** Rust/Tokio, serde, regex/regex-syntax, SQLite only in Core, existing Vue components.
**Spec:** docs/superpowers/specs/2026-10-22-flag-substitution-proxy-design.md

## Global constraints
- Two permanent processes, no per-session threads and no raw traffic gRPC.
- Finite framing buffers, durable mappings, no automatic mapping eviction.
- Equal-length replacements; unsupported framing fails closed in enabled mode.
- Never enqueue original/substitute flags for submission.
- Preserve existing uncommitted work; no commit/push requested for this feature.

## Review focus
- Flags split at every TCP boundary must still match.
- Separate connections and restarts must restore the same original.
- Chunk metadata and length prefixes must remain unchanged.
- Capacity/persistence failure must not forward unmapped flags.
- Configuration changes must not restore another service/target's mappings.

### 1. Pure rewrite and framing
Files: new crates/hydra-rewrite/src/{lib,format,framing,http,tests}.rs; domain forwarding types.
- [x] Write format/generation and byte-integrity tests; observe failure.
- [x] Implement fixed-width ASCII format validation, replacement spans, TCP frame adapters and UDP framing. Bound messages at64KiB.
- [x] Verify all split positions, chunked HTTP body scatter, length prefixes, malformed/unsupported framing and EOF.

### 2. Durable mapping and relay integration
Files: new apps/hydra-proxy/src/forwarding/{mod,store,tests}.rs; transport, app and telemetry wiring.
- [x] Write restart, namespace, collision, capacity and request/response fixture tests; observe failure.
- [x] Implement protected bounded journal and async duplex adapter; commit mappings before forwarding. Datagram adapter shares mappings.
- [x] Verify two-process relay integration and disabled byte integrity.

### 3. Configuration and UI
Files: domain/wire/protobuf; Core services forwarding DTO/resolution; frontend services ForwardingForm component.
- [x] Test allowlisted team resolution, invalid patterns, default disabled and wire round trip.
- [x] Implement authenticated audited settings with team/port/regex/framing selectors; generate OpenAPI/frontend types.
- [x] Run frontend test/typecheck/build and inspect service controls.

### 4. Delivery
- [x] Run Rust tests, formatting, Clippy, contract and architecture checks; document measured results/limits.
- [x] Build releases and redeploy to team-1 with feature disabled; verify both processes/API/UI and original service configuration.

Ruling: execute the approved feature inline following the user’s “Do it”; preserve existing uncommitted work and do not introduce another approval checkpoint.
Ruling: namespace keys exclude framing so changing framing retains previous target/format mappings; UDP bindings keep the existing reset-on-edit behavior.
Review: fresh-context reviewer found delimiter mutation, inherited deadlines and own-interface relay loops. Regression tests reproduced the first two; all three were corrected and re-reviewed.
