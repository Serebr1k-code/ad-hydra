# Hydra — mandatory repository structure & dependency architecture

**Status:** normative implementation specification, handoff v1.1.  This file is part of the acceptance criteria for GPT-6.1 Sol. Read together with `SPEC.md`, `AGENTS.md` and `IMPLEMENTATION_PLAN.md`. If a naming detail needs adjustment to make the code build, keep the architectural boundaries and document the change in `DECISIONS.md`.

## 1. Design objective

Develop a **scalable modular monorepo**, *not* a single enormous crate, *not* a collection of networked microservices, and *not* a crate for every file. Runtime remains exactly **two permanently running Rust processes** (`hydra-core`, `hydra-proxy`) and bounded transient sandboxed exploit processes. The repo must support adding a new protocol/parser, a rule operator, a UI page, a checker adapter, and eventually a third network-filter backend without moving unrelated files or introducing cyclic dependencies.

All file paths below are **relative to repository root**. This handoff bundle already contains `contracts/openapi.yaml` and `contracts/hydra/proxy/v1/proxy.proto`; **keep these canonical locations** (do not silently create a second copy).

## 2. Required target tree

The tree below is a **layout blueprint**. In Phase 0, create the directories/files needed by the current milestone. Do not generate hundreds of empty placeholder modules just to mirror the example.

```text
hydra/
├── Cargo.toml                          # Virtual Cargo workspace, shared dependencies/lints
├── Cargo.lock                          # Commit for reproducible binary builds
├── rust-toolchain.toml                 # Pinned tested stable toolchain
├── .cargo/config.toml                  # Optional local aliases/build settings
├── .gitignore
├── README.md                           # Build/run/test entry point
├── AGENTS.md                           # Binding coding-agent engineering rules
├── SPEC.md                             # Product source of truth
├── REPOSITORY_STRUCTURE.md             # This architecture contract
├── IMPLEMENTATION_PLAN.md
├── START_HERE.md
├── DECISIONS.md                        # Architecture decision log (ADR index)
│
├── apps/                               # Deployable Rust executables; NOT shared libraries
│   ├── hydra-proxy/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── main.rs                 # CLI args, logger, graceful shutdown only
│   │   │   ├── lib.rs                  # App composition/testable entry points
│   │   │   ├── app.rs                  # Initialize transport, gRPC, rules, plugins
│   │   │   ├── config/                 # Parse/validate local proxy config
│   │   │   │   ├── mod.rs
│   │   │   │   └── validation.rs
│   │   │   ├── transport/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── tcp/                 # Listener, relay, half-close/backpressure
│   │   │   │   └── udp/                 # Datagram relay, session map, eviction
│   │   │   ├── flows/                  # Connection ids, lifecycle, session tracking
│   │   │   ├── inspection/             # Parser wiring, bounded flag scan, event assembly
│   │   │   ├── filters/                # Apply compiled rule snapshots to live flows
│   │   │   ├── telemetry/              # Parsed batches, bounded queue, critical spool
│   │   │   ├── samples/                # Disk ring of *optional* raw samples
│   │   │   ├── grpc/                   # tonic server: control, telemetry, diagnostics
│   │   │   ├── state/                  # Last-known-good config, revision, shutdown
│   │   │   └── security/               # Unix socket permissions, peer authorization
│   │   └── tests/                      # Cargo integration tests for proxy executable
│   │
│   └── hydra-core/
│       ├── Cargo.toml
│       ├── src/
│       │   ├── main.rs                 # CLI dispatch/server entry point only
│       │   ├── lib.rs                  # Testable app assembly
│       │   ├── app.rs                  # Dependency wiring and lifecycle
│       │   ├── cli/                    # create user, safe password prompt, maintenance
│       │   ├── api/                    # axum router, shared errors, auth extractor, SSE
│       │   │   ├── mod.rs
│       │   │   ├── router.rs
│       │   │   └── error.rs
│       │   ├── modules/                # Vertical features; keep changes local
│       │   │   ├── auth/               # Login, JWT, refresh rotation, sessions
│       │   │   ├── users/              # CLI-managed accounts, account edits
│       │   │   ├── services/           # Proxy listeners/upstream configuration
│       │   │   ├── filters/            # Filter CRUD, enable/disable, audit, revisions
│       │   │   ├── traffic/            # Flow/message queries and sample references
│       │   │   ├── protocols/          # Plugin upload/test/activate/versioning
│       │   │   ├── farm/               # Targets, exploits, jobs, sandbox scheduler
│       │   │   ├── flags/              # Flag extraction, provenance, submit queue
│       │   │   └── audit/              # Actor attribution, immutable audit events
│       │   ├── infrastructure/
│       │   │   ├── db/                 # SQLite pool/writer, transactions, migrations
│       │   │   ├── proxy_client/       # tonic client, reconnect, stream ACK/replay
│       │   │   ├── crypto/             # Password hashing/JWT key management
│       │   │   ├── sandbox/            # bwrap/cgroups integration, allowlist enforcement
│       │   │   └── storage/            # Uploads/worker logs and quotas
│       │   └── background/             # Bounded tasks: submissions, event consumer
│       └── tests/                      # Cargo integration tests for HTTP and CLI
│
├── crates/                             # Small, reusable in-process Rust libraries
│   ├── hydra-domain/
│   │   ├── Cargo.toml
│   │   └── src/                         # Shared IDs, typed Flow/Event/Rule/Actor etc.
│   ├── hydra-wire/
│   │   ├── Cargo.toml
│   │   ├── build.rs                     # tonic/prost compile from contracts/
│   │   └── src/                         # Generated code include + domain conversions
│   ├── hydra-filter-engine/
│   │   ├── Cargo.toml
│   │   └── src/                         # Rule AST compile/evaluate, backend *trait*
│   ├── hydra-protocol-sdk/
│   │   ├── Cargo.toml
│   │   └── src/                         # Versioned Rust-to-WASM parser API/helpers
│   ├── hydra-protocol-runtime/
│   │   ├── Cargo.toml
│   │   └── src/                         # WASM host, resource limits, built-in parsers
│   └── hydra-testkit/
│       ├── Cargo.toml
│       └── src/                         # Dev-only fixtures/toy TCP-UDP/HTTP servers
│
├── contracts/                          # Reviewed transport/API contracts, versioned
│   ├── openapi.yaml                    # Checked-in OpenAPI 3.1 generated snapshot
│   ├── buf.yaml                        # Add during Phase 0
│   └── hydra/proxy/v1/proxy.proto      # Canonical protobuf source, v1 package
│
├── frontend/                           # Build-time SPA, NO production Node server
│   ├── package.json
│   ├── src/
│   │   ├── app/                        # Routing, app root, auth refresh handling
│   │   ├── features/
│   │   │   ├── filters/
│   │   │   ├── traffic/
│   │   │   ├── protocols/
│   │   │   ├── farm/
│   │   │   ├── flags/
│   │   │   └── settings/
│   │   ├── shared/                     # Reusable UI, API client, common hooks
│   │   └── generated/                  # REST types/client generated from OpenAPI
│   └── dist/                           # Build output, GITIGNORED; embedded in Core
│
├── plugins/                            # Independently buildable sample plugins
│   ├── templates/rust-wasm/
│   ├── examples/length-prefixed/
│   └── fixtures/                       # Binary samples + expected parsed events
├── migrations/                         # Ordered SQLite migrations, CORE owns schema
│   ├── 0001_identity.sql
│   ├── 0002_services_filters.sql
│   ├── 0003_traffic.sql
│   └── 0004_farm_flags.sql
├── config/                              # Checked-in examples ONLY; no secrets
│   ├── core.example.toml
│   └── proxy.example.toml
├── deploy/
│   ├── systemd/                         # Separate core/proxy systemd units
│   └── tmpfiles.d/                      # Socket/runtime directory ownership
├── tests/
│   ├── e2e/                             # Explicitly invoked system-level harness
│   ├── compatibility/                   # REST/protobuf/plugin ABI compatibility
│   ├── resilience/                      # Core crash, Proxy restart, spool failures
│   └── performance/                     # 2 vCPU / 2 GiB bounded-load measurements
├── scripts/                             # Verify contracts, release bundle, smoke test
├── docs/
│   ├── ARCHITECTURE.md
│   ├── DEVELOPMENT.md
│   ├── SECURITY.md
│   └── adr/                              # Short trade-off decision records
└── .github/workflows/ci.yml
```

**Distinguish blueprint from deliverables:** example migration filenames describe ownership; do not write fake migrations ahead of actual schema. An initially empty `plugins/templates` directory is not a completed plugin SDK. Top-level `tests/e2e` will **not** be discovered automatically by `cargo test`: wire its harness explicitly into CI. Unit/integration tests for each crate belong under its `src`/`tests` and are discovered by Cargo.

## 3. Dependency directions (MANDATORY)

```text
hydra-domain ─────────────────────┐
                                 ├──> hydra-wire ────────┐
                                 ├──> hydra-filter-engine├──> hydra-core
                                 │                      └──> hydra-proxy
hydra-protocol-sdk ──> hydra-protocol-runtime ──────────> hydra-proxy
                           ^
                           └── hydra-domain (events/types only)

frontend ──HTTP/OpenAPI──> hydra-core
hydra-core ──gRPC/protobuf──> hydra-proxy   (RUNTIME only, no Cargo import)
```

Arrows from crates to consumers show library imports. The Core→Proxy gRPC arrow is a **network interaction**, not a Rust package dependency. **Neither application crate may depend on the other.** `hydra-domain` must not depend on database, JWT, axum, tonic, Tokio or the UI. `hydra-filter-engine` must not reach into Proxy's gRPC/transport or Core's SQLite. `hydra-protocol-sdk` must compile independently for its documented WASM target and must not embed the host runtime. Heavy WASM dependencies appear only in `hydra-protocol-runtime`/Proxy, never Core.

For full clarity, allowed Cargo dependencies:

| Package | May depend on | Must not depend on |
|---|---|---|
| `hydra-domain` | serde, lightweight value/ID types | `apps/*`, db, networking, axum, tonic |
| `hydra-wire` | `hydra-domain`, prost, tonic | `apps/*`, SQLite, WASM host |
| `hydra-filter-engine` | `hydra-domain`, regex/aho-corasick | `apps/*`, web framework, databases |
| `hydra-protocol-sdk` | small WASM-compatible type libraries | `apps/*`, Tokio, wasmi host |
| `hydra-protocol-runtime` | `hydra-protocol-sdk`, `hydra-domain`, bounded WASM runtime | `apps/*`, SQLite, UI |
| `hydra-proxy` | domain, wire, filter-engine, protocol-runtime | hydra-core, SQLite, UI/auth |
| `hydra-core` | domain, wire, filter-engine for validation, SQLite/axum/auth | hydra-proxy, WASM host, raw socket transport |
| `hydra-testkit` | test fixtures and minimal domain types | production runtime dependency of Core/Proxy |

**Code review check:** if shared business logic is duplicated in Core and Proxy, put the truly shared pure part in an existing appropriate crate. Never solve sharing by adding `hydra-core` as a dependency of `hydra-proxy`, or vice versa. Prevent dependency cycles using `cargo metadata` and CI boundary tests.

## 4. Vertical feature modules in Core

Each `apps/hydra-core/src/modules/<feature>/` grows by responsibility (create files when justified):

```text
modules/filters/
├── mod.rs              # Exports and feature wiring
├── http.rs             # Axum HTTP endpoints, JSON DTO validation and status codes
├── service.rs          # Filter business operations; actor and revision invariants
├── repository.rs       # SQL reads/writes; no HTTP or gRPC
├── dto.rs              # REST request/response schemas (not domain entities)
└── tests.rs            # Pure/service unit tests; external integration in tests/
```

The HTTP handler extracts authenticated actor and request ID → calls service → service performs validated/audited transactional state change through repository → `proxy_client` applies versioned config → records the asynchronous applied state. Do not block an HTTP request indefinitely on Proxy; represent `pending/applied/error` accurately. **Only `infrastructure/db` manages SQLite connections, migrations and writer policy.** Feature repositories own their SQL statements; they cannot allocate their own independent write pools.

`modules/auth/` owns session state, JWT issuance/rotation and tokens. `modules/users/` manages account records and CLI use cases; no public registration. `modules/audit/` owns the shared audit write API. `modules/farm/` schedules jobs; `infrastructure/sandbox/` alone knows bubblewrap/cgroups mechanics. `modules/flags/` owns submission business policy; transport-specific checker adapters live underneath that feature's `adapters/` directory, not in the proxy.

Do not create a giant `services.rs`, `models.rs`, `handlers.rs`, `utils.rs`, or `common.rs` holding unrelated features. Small local helper modules are fine when accurately named.

## 5. Proxy layering

The **data path** must remain simple:

```text
transport/{tcp,udp} → flow tracking → optional protocol runtime/inspector
                   → local filter-engine → forward-or-reject decision
                                    └──→ bounded telemetry/sample queues
```

Transport owns sockets, TCP half-close/backpressure, UDP session mapping and correct raw-byte forwarding. No transport function calls a Core API, writes SQLite or blocks on gRPC. The `inspection/` module produces typed events, and **only** `telemetry/` batches/ships them. `filters/` applies already-compiled snapshots without calling Core. `grpc/` handles management calls, plugin upgrades and snapshot validation; it does not contain forwarding rules. `state/` persists last-known-good config and runtime versions.

For HTTP blocking, inspect before forwarding only when full/partial framing and bounded buffering make blocking possible. For unknown TCP, do not simulate `drop_message`; use pass-through or close-connection according to policy. Plugin failure must never crash Proxy or silently modify forwarded bytes.

## 6. Contracts as single source of truth

**Protobuf:** `contracts/hydra/proxy/v1/proxy.proto` is canonical and versioned as package `hydra.proxy.v1`. `crates/hydra-wire/build.rs` compiles it into Cargo `OUT_DIR`; `src/lib.rs` exposes generated modules and explicit conversion functions. **Never hand-edit generated Rust** or maintain a copied `.proto` in each app. Preserve removed protobuf field numbers with `reserved`. Run `buf lint` and `buf breaking` in CI when baseline is available. The legacy handoff protobuf is a draft: compile and correct before public adoption.

**OpenAPI:** Public HTTP API is declared/annotated in Core (`utoipa` or equivalent). The checked-in `contracts/openapi.yaml` is the **generated version-controlled contract snapshot**, not a second manually maintained source. During Phase 0 reconcile existing draft with actual routes, generate deterministic YAML/JSON, and make CI fail if generated contract differs from committed snapshot. Frontend API types/client are generated from this contract under `frontend/src/generated/`; do not manually duplicate public API types. API version remains `/api/v1`. Keep `/docs` Swagger UI.

**Plugin SDK / WASM ABI:** `hydra-protocol-sdk` defines a versioned, bounded plugin interface independent of Proxy internals, including typed parsed message/event output, directional context, stateful session lifecycle, resource limits and ability to report decoder failure. Add `PLUGIN_ABI.md` (in `docs/` or SDK) specifying exact exported functions, memory ownership, version handshake, accepted WASM target, compatibility and errors. Do not promise hot load until a sample plugin loads, parses bytes and unloads safely. Native built-in parsers may implement the host's internal trait directly, sharing event semantics.

## 7. DB and runtime data ownership

**Only `hydra-core` writes SQLite.** Keep migrations under root `migrations/`, run transactionally on startup, with checksums/version tracking and backup instructions. Feature-specific repositories contain SQL, and the infrastructure DB module owns pool/writer configuration and busy/timeout policy. Use SQLite WAL, one bounded writer strategy, indexes based on actual Explorer filters, and explicit retention/quotas. Avoid placing large raw payloads as rows.

No runtime data in Git or the workspace build tree. Recommended Linux paths (adjust via config and documented systemd settings):

```text
/etc/hydra/core.toml                   # Non-secret Core config
/etc/hydra/proxy.toml                  # Non-secret Proxy config
/run/hydra/proxy.sock                 # UDS with restrictive ownership/mode
/var/lib/hydra/core/hydra.sqlite3      # Persistent SQLite owned by Core
/var/lib/hydra/core/uploads/           # Authorized uploads + versions
/var/lib/hydra/core/keys/              # JWT key material; restrictive mode
/var/lib/hydra/proxy/last-good.json    # Last applied rule/service config
/var/lib/hydra/proxy/samples/          # Bounded raw ring files
/var/lib/hydra/proxy/spool/            # Bounded critical-event replay log
/var/log/hydra/                        # Optional logs (journald preferred)
```

Use distinct service users and correct Unix permissions for the socket and files. Only Core accesses password hashes/refresh sessions and checker credentials. Proxy has neither SQLite access nor user passwords. No secrets in `config/*.example.toml`, audit payloads, saved network samples by default or stdout.

## 8. Adding features without architectural drift

**Add a protocol:** `plugins/templates/rust-wasm/` or `plugins/examples/<name>/` → code against `hydra-protocol-sdk` → fixtures/tests → Core `modules/protocols` upload/versioning → Proxy `hydra-protocol-runtime` loads plugin → typed fields can be referenced by `hydra-filter-engine`. No changes to TCP/UDP relay unless genuinely required.

**Add a filter operator:** pure AST/operator in `hydra-domain` → compile/evaluation in `hydra-filter-engine` → API schema/snapshot → tests for missing fields, limits, precedence and modes → frontend rule builder. Do not change the transport relay for field-level rules.

**Add a Flag Submitter adapter:** implement a trait under `apps/hydra-core/src/modules/flags/adapters/<name>.rs`; extend config/contract and tests using a toy checker. Never move submission/network credentials into Proxy.

**Add a farm runtime:** add one adapter under `modules/farm/runtimes/` while preserving the one sandbox orchestration boundary; don't add a permanent worker daemon unless explicitly approved.

**Add a network filter backend later:** implement the existing `NetworkFilterBackend` trait, isolate backend-specific OS/platform code, and test the same semantics. eBPF must NOT be implemented now; the trait is the forward-compatibility boundary.

**Add a REST page:** feature HTTP endpoints in Core → shared schema generation → generated frontend client → `frontend/src/features/<feature>/` page(s). No direct frontend-to-proxy gRPC or browser access to SQLite.

## 9. Cargo workspace and CI rules

Initialize with a root **virtual workspace** (no root `[package]`). The following is the **target manifest after all justified crates exist**, not a demand to create empty placeholder packages in Phase 0; add workspace members as their actual modules are implemented:

```toml
[workspace]
resolver = "3"
members = [
  "apps/hydra-core",
  "apps/hydra-proxy",
  "crates/hydra-domain",
  "crates/hydra-wire",
  "crates/hydra-filter-engine",
  "crates/hydra-protocol-sdk",
  "crates/hydra-protocol-runtime",
  "crates/hydra-testkit",
]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "time", "sync"] }
# Add/centralize actual dependencies ONLY when required by implemented code.
```

`hydra-testkit` must be used only as `dev-dependency`; do not accidentally ship fixture/test servers. Keep heavy/proxy-only dependencies off Core, and vice versa. Each package has a short `lib.rs` public API and private modules as appropriate. Commit `Cargo.lock` for binaries. Pin a tested toolchain via `rust-toolchain.toml`; do not silently adopt toolchain upgrades.

Minimum CI checks (in addition to end-to-end smoke tests):

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# buf lint (and buf breaking against baseline once public)
# deterministic OpenAPI generate-and-diff check
# build frontend + frontend unit tests
# launch two real processes and run TCP/UDP/gRPC smoke tests
```

CI should include code boundary checks for prohibited cross-dependencies and ensure no generated source is hand-maintained in multiple locations. New schema change requires regenerated contracts/types, migration as needed and tests. For production release, use `cargo build --release -p hydra-core -p hydra-proxy` and bundle the frontend into Core at build time.

## 10. Scalability means containment, not unlimited features

- **Codebase scalability:** new protocol, operator, submit adapter or UI page is implemented in a bounded set of directories.
- **Runtime scalability:** bounded queue sizes, UDP/session caps, per-flow parser budget, disk ring/spool quotas, per-job CPU/memory limits. Explicit drop/loss metrics for low-priority telemetry.
- **Change safety:** versioned API, protobuf package, WASM ABI, SQLite schema and filter revisions, graceful reconcile on process reconnect.
- **Testing:** unit at module/crate, Cargo integration in each app, explicit full-system fixtures in root tests, benchmarks tied to 2 vCPU/2 GiB.
- **Low overhead:** two long-lived binaries, no database or event-broker process, no full raw traffic over gRPC, no mandatory Docker/eBPF/Suricata.

## 11. Acceptance criteria for the coding agent

Phase 0 is **not accepted** until it contains:

1. Valid virtual Cargo workspace with both independently runnable binary crates and only justified library crates (no fake placeholder features).
2. `REPOSITORY_STRUCTURE.md` and `AGENTS.md` at repo root, with any deviations documented in `DECISIONS.md`.
3. Real `.proto` generation in `hydra-wire` from **one** canonical path and validated OpenAPI strategy.
4. Successful `cargo check --workspace`, `cargo test --workspace` (to the extent code exists), lint/format pass, and a documented minimal two-process startup.
5. An explicit dependency-boundary review documenting why Core cannot reach into Proxy modules and why Proxy cannot access Core persistence/auth.

Every later phase must respect this structure. Refactors that cross ownership boundaries without strong justification and updated docs/tests are not acceptable. Prefer gradual extraction of modules as responsibilities grow over speculative empty directories.
