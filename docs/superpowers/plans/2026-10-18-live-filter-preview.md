# Live Filter Preview Implementation Plan

> **For agentic workers:** This plan is being executed inline under the user's explicit instruction to implement the approved design.

**Goal:** Let operators select same-service streams, build filters from the active decoder schema, and see a non-enforcing live preview against retained stream data.

**Architecture:** Traffic selection hands flow IDs and service ID to the filter editor through bounded session storage. Core exposes active decoder fields and preview evaluation using its compiled filter engine; Proxy remains the owner of raw traffic and evaluates raw byte previews locally through an authenticated diagnostics RPC. Built-in and WASM decoders implement one typed decoder interface; raw payload never enters telemetry.

**Tech Stack:** Rust/Tokio, tonic/prost, hydra-filter-engine, Vue 3/TypeScript/Bun, existing OpenAPI generator.

**Spec:** `SPEC.md`, especially Sections 5–7 and 10.

## Global Constraints

- Traffic forwarding stays independent of Core, gRPC, SQLite, and frontend.
- Raw packet streams are never transferred over telemetry gRPC.
- Preview never changes enforcement; only saving a filter does.
- Decoder fields must be typed, bounded, and scoped to the active service decoder.
- Preserve existing API and protobuf compatibility; update generated contracts for additions.

## Review Focus

- Stream IDs from mixed services cannot silently target the wrong decoder — test selection constrains to one service.
- Raw byte patterns crossing read boundaries match without emitting raw payload telemetry — test split-pattern matching and telemetry omission.
- Disabled/shadow/non-block actions are not mislabeled as blocked — test preview status mapping.
- An unavailable or truncated history yields an explicit preview state instead of a false pass — test missing history.
- WASM nested fields retain declared field types and active-version scope — test catalog construction.

### Task 1: Unified decoder schema and raw byte evaluation

**Files:** `crates/hydra-protocol-runtime/src/*`, `apps/hydra-proxy/src/inspection/*`, `apps/hydra-proxy/src/transport/{tcp,udp}.rs`, filter engine tests.

- [ ] Add failing tests for raw bytes patterns split across reads, built-in/WASM schema agreement, and absence of raw payload in telemetry.
- [ ] Introduce a common typed decoder trait implemented by built-ins and WASM; preserve bounded raw carry and parsed event behavior.
- [ ] Run focused Rust tests.

### Task 2: Dynamic service decoder field catalog

**Files:** `frontend/src/features/filters/{FiltersPage,FilterEditorDialog,RuleGroupEditor,ruleBuilder}.vue/ts`, frontend tests.

- [ ] Test field list composition and scalar kind/operator selection for native and nested WASM schema fields.
- [ ] Load the service's active decoder schema; show built-in HTTP/UTF-8/raw fields plus universal fields, and render raw byte values as hex.
- [ ] Run focused Bun tests, typecheck, and build.

### Task 3: Select streams and hand off to filter creation

**Files:** `frontend/src/features/traffic/{TrafficPage,SessionList}.vue`, traffic/filter workflow tests.

- [ ] Test same-service multi-selection and handoff preservation.
- [ ] Add accessible selection controls and create-filter action; retain one-service constraint and clear action.
- [ ] Open the filter builder with service and flow IDs restored after route navigation.
- [ ] Run focused Bun tests.

### Task 4: Non-enforcing candidate preview

**Files:** Core filter HTTP/repository/preview, Proxy diagnostics, canonical proto, frontend filter editor, generated OpenAPI/client.

- [ ] Add failing API/RPC tests for candidate match outcomes, raw history, action semantics, and unavailable history.
- [ ] Evaluate retained parsed fields in Core and raw history in Proxy; return only outcomes and bounded metadata, never raw bytes.
- [ ] Debounce preview on edits and show per-flow would-block/would-pass/would-match-only states.
- [ ] Regenerate and diff OpenAPI/client, run per-crate and frontend suites, then 2-process smoke test.
