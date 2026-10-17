# AGENTS.md — persistent instructions for Hydra coding agent

You are implementing Hydra from `SPEC.md` for authorized A/D CTF use. Treat `SPEC.md` as product truth, `IMPLEMENTATION_PLAN.md` as delivery order, and `contracts/` as a **draft** interface starting point. If they disagree, prefer `SPEC.md`. Log meaningful deviations in `DECISIONS.md`; do not invent features to fill ambiguity.


## Mandatory repository layout (do not improvise a monolith)
- **Read `REPOSITORY_STRUCTURE.md` before touching implementation**. It is a required source-tree/dependency contract. Follow its `apps/`, `crates/`, `contracts/`, `frontend/`, `plugins/`, `migrations/`, `tests/`, `deploy/` and docs boundaries.
- `apps/hydra-core` and `apps/hydra-proxy` are the only always-running binaries; they NEVER depend on each other directly. Share types in `hydra-domain`, wire via `hydra-wire`, rule evaluation via `hydra-filter-engine`, plugin interface via `hydra-protocol-sdk`, and WASM host via `hydra-protocol-runtime`.
- Keep `main.rs` as thin bootstrap. Inside Core organize by vertical feature under `src/modules/{auth,users,services,filters,traffic,protocols,farm,flags,audit}`; keep transport/gRPC/telemetry in Proxy-owned modules. Feature handlers must not embed unrelated SQL and authentication or networking code.
- Use single canonical `contracts/hydra/proxy/v1/proxy.proto`; generated Rust in Cargo OUT_DIR, never hand-edit generated code. OpenAPI checked-in snapshot `contracts/openapi.yaml` must be generated/diff-checked against handlers.
- Only Core owns SQLite and migrations. Plugin SDK must be independent of binaries. Frontend uses HTTP OpenAPI only. No frontend access to internal gRPC.
- Do not create empty crates to satisfy a tree; start with needed scaffold and grow justified modules. Avoid giant `main.rs`, `utils.rs`, `handlers.rs` and cross-feature catch-all files. Explain any deviations in `DECISIONS.md`.
- For every PR/phase verify package dependency directions, generated contract compatibility, per-crate tests and 2-process smoke test; absence of unrelated changes is an architectural success criterion.

## Engineering defaults
- Rust stable, Tokio async; `tonic`/`prost` for gRPC; `axum` for REST; SQLite WAL via a mature Rust library; light SPA bundled as static files and served by Core (no Node runtime on deployment machine).
- Two permanent processes only (`hydra-proxy`, `hydra-core`); exploit workers are transient sandboxed children. Data path never blocks on Core, gRPC, SQLite, or frontend.
- All core state bounded: channels, samples, UDP mapping, parser state, connections, regex input, execution worker count, spool and upload sizes. Handle backpressure and report loss explicitly.
- Rust-compiled WASM plugin is the hot-loaded extension format. Native Rust parsing is a compile-time extension. Unknown protocols pass through unchanged.
- Every mutation and sensitive diagnostic read has actor provenance and audit metadata; all authenticated users have identical full rights. Never accept author from user input.
- Access JWT 60 s; refresh JWT 10 h absolute and rotates. User creation via authenticated UI/API and CLI bootstrap/recovery (explicit user amendment 2026-10-18). Keep security secrets out of telemetry, worker environments and logs.
- Domain of authorized test targets is defined by explicit allowlist; never provide open arbitrary-host execution or packet injection API.
- Do not reintroduce Round Manager, Service Monitor, eBPF, Suricata, Redis, PostgreSQL or Docker daemon without explicit product change.

## Execution discipline
- First inspect repo state and existing contracts; do not overwrite user work blindly. Maintain reproducible build and runnable vertical slice after each milestone.
- Implement a phase, run appropriate tests and linters, fix failures, update docs, then continue to the next phase if feasible. Don't end after drafting a plan when implementation is requested.
- Keep OpenAPI/protobuf in sync with real handlers. Validate/generate them in build/CI; any removed or changed field/method needs a migration/compatibility note.
- Avoid placeholder endpoints or mock UI with no backend unless explicitly labeled as an unfinished phase. Report incomplete scope accurately.
- For every phase report: what works, changed files, build/test commands and results, outstanding limitations, and measured RAM/CPU when relevant.
- Treat untrusted WASM and exploit uploads as hostile. No default unsandboxed execution, no trusted arbitrary `.so` loading, no unrestricted filesystem or network access.

## Implementation preferences
- Separate transport, protocol parsing, inspection, rules, telemetry, storage, auth, farm and UI via small explicit interfaces; avoid premature microservices.
- Use clear typed errors and cancellation/shutdown handling; prefer tests around packet byte integrity, rule semantics, retry/idempotency, and auth/session lifetimes.
- For protocol-specific HTTP response blocking, only promise enforcement before forwarding where buffering/framing guarantees it; raw TCP message drop is not generally valid.
- Minimize allocations and copies on the transport fast path but prioritize correctness; document actual benchmark conditions rather than assuming Rust has a specific RSS target.
