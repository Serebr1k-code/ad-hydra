# IMPLEMENTATION_PLAN.md — phased delivery and acceptance

This is a delivery order, not permission to stop after a planning document. Implement an actual running slice at each stage. Record completed steps in a simple `PROGRESS.md` or PR summaries. Prefer successive coherent commits, not one monolithic untestable change.

## Phase 0 — Repository foundation and contract validation
**First read and implement the mandatory `REPOSITORY_STRUCTURE.md` layout and dependency rules.** Create just the necessary real directories; no hundreds of empty placeholder crates.
**Deliver** Cargo workspace, rustfmt/clippy/test setup, `hydra-core` + `hydra-proxy` CLI entry points, shared protobuf build generation, `openapi.yaml` validator, Linux launch README, CI commands. Audit provided draft contracts for field mismatches and create `DECISIONS.md`. Confirm no `hydra-core`↔`hydra-proxy` package dependency; generated protobuf lives only in `hydra-wire`; only Core owns SQLite; separate feature modules from HTTP/router, transport and storage.
**Accept** clean build/test; `protoc`/buf validates `.proto`; OpenAPI schema parses; both processes launch independently on UDS. Repo architecture matches `REPOSITORY_STRUCTURE.md` (or documented deviations), with per-crate tests and shared contracts generated once.

## Phase 1 — Real data plane and control channel
**Structure enforcement:** implement real forwarding under `apps/hydra-proxy/src/transport/`, gRPC under `apps/hydra-proxy/src/grpc/`, protobuf in `crates/hydra-wire/` and Core gRPC client under `apps/hydra-core/src/infrastructure/proxy_client/`; no inlining all logic into `main.rs`.
**Deliver** TCP full-duplex relay with transparent raw bytes and graceful half-close; UDP relay with bounded flow table, correct upstream reply mapping; Core→Proxy config snapshots and Proxy status through gRPC/UDS; independent last-good local proxy config. Flow open/close parsed metadata stream.
**Accept** raw binary TCP echo and UDP echo integration tests; kill Core while relaying traffic; restart Core+Proxy and reapply revision; configuration ACK test. No arbitrary session-byte injection.

## Phase 2 — Users, JWT, audit, filter CRUD
**Structure enforcement:** keep auth/users/audit/filter feature modules separate, SQLite migrations in root `migrations/`, and pure filter evaluation in `hydra-filter-engine`.
**Deliver** SQLite migrations, CLI create user with safe prompt and explicit password argv form, Argon2id, JWT/login/logout/refresh/rotation/sessions, equal privileges, actor audit, REST and Swagger; rules AST, parser field registry, shadow mode, Proxy atomic apply, filter list date/author/hits, enabled toggle with pending/applied/error UI.
**Accept** tests for JWT TTL and absolute refresh session end, no registration, identity attribution; HTTP path/User-Agent rule blocks only intended requests; disabling rule restores passage; concurrent revision conflict returns 409.

## Phase 3 — Parse, plugins and Traffic Explorer
**Structure enforcement:** Rust-to-WASM plugin API in `hydra-protocol-sdk`, plugin host in `hydra-protocol-runtime`, examples in `plugins/`, Core traffic features in `modules/traffic`.
**Deliver** basic HTTP/1.1 parser, Rust protocol SDK, WASM plugin ABI/runtime limits/test/activate path, generic frame profiles, typed gRPC events, SQLite event indexing, flow/message UI, search, on-demand sample disk ring (not raw gRPC stream), SSE. Flag matcher across TCP chunks and plugin-decoded data.
**Accept** upload compiled sample decoder and activate live; malformed WASM doesn't crash proxy; unknown encrypted traffic untouched; gRPC flow records parsed only; same sample expires correctly. Demonstrate normal telemetry drop counter and bounded critical replay/ACK.

## Phase 4 — Farm and flag submission
**Structure enforcement:** farm/flags business modules in Core; sandbox mechanics in `infrastructure/sandbox`; checker adapters only in flags module; no extra permanent worker daemon.
**Deliver** exploit upload/versioning, sandbox subprocess, target allowlist, per-target/batch, interval/manual scheduling, cancel/timeout/limits/logs, regex extraction, captured/own_exposed/unknown separation, queue/dedup/retries, checker HTTP/TCP adapter interface, UI and audit.
**Accept** run sample test script against local allowlisted toy target, extract fake flag, submit to fake checker, review provenance, deny outbound to non-allowlisted host; own_exposed not automatically submitted. Restart Core and preserve queued jobs/submissions.

## Phase 5 — Product polish and resource validation
**Deliver** responsive compact browser UI for services, traffic, filters, protocols, farm, flags, users/audit, settings; API errors and status visibility, secure deployment systemd setup, backup/export, doc examples; performance and negative tests.
**Accept** one documented end-to-end demonstration, full test suite, contract generation, failure/restart handling, bounded queue/connection memory; measure Proxy+Core RSS and CPU on 2 vCPU/2 GiB. Report unmet targets honestly.

## End-to-end demo
1. Start toy HTTP, raw TCP and UDP services. Map ports through Proxy.
2. Create users `alice` and `bob` via CLI. Log in as each and confirm equal permissions.
3. Show parsed HTTP flow, define shadow HTTP filter, enable enforce, confirm proxy revision, attribution and counter, then toggle off.
4. Upload a sample Rust→WASM custom parser, activate with no restart and show parsed typed fields.
5. Launch sandboxed toy exploit against allowlisted test target, match fake flag, submit to mocked checker, review flag source and audit.
6. Kill Core; prove Proxy keeps forwarding TCP/UDP; restart and reconcile.

## Open issues for engineering decision (document choice, don't block entire project)
- Initial service mode is port mapping; original client IP at backend changes. Full transparent forwarding is separate, not implicitly promised.
- TLS data remains opaque without explicit termination/keys; don't invent universal encrypted parser.
- HTTP/2/WebSocket not required for first vertical slice; include extension points.
- Sandbox availability and permissions vary across Linux distributions; document supported kernel/cgroup prerequisites and refuse unsafe fallback for untrusted uploads.
