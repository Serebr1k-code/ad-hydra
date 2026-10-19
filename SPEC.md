# Hydra — Product & Architecture Specification (authoritative handoff)

Status: agreed product decisions + implementation guidance. Farm host-execution amendment 2026-10-17: the user explicitly authorized ordinary host child execution for Binary, Python (uv with optional versioned dependencies), and Bash. Farm has separate service(name/port) and team(name/IP) catalogs, per-exploit service selection, direct/all-team runs, and gzip-compressed rotating logs. Host log capture streams beyond1 MiB into paginated chunks with bounded shared disk/raw retention; run and target summaries show distinct new/existing flag counts. The previous mandatory exploit-isolation requirement is superseded for explicitly configured host mode; retain sandbox mode as an alternative. See docs/farm.md and DECISIONS.md. Python submission amendment 2026-10-17: Farm output regex matches enter a persistent flag queue; an editable uploaded/pasted Python script with optional uv dependencies receives exact full regex matches as positional arguments as captures arrive. The operator configures a bounded per-call batch size and can manually submit a chosen count while automatic dispatch is paused. Exit0 accepts the whole call; failures retry up to10times. See docs/farm.md for bounded chunks and gzip history. Version: handoff 1.0. Audience: coding agent (GPT-6.1 Sol) and human maintainer.

## 1. Goal and environment

Build a lightweight Attack/Defense CTF operations console for a team's **authorized competition infrastructure**. Hydra accepts/proxies traffic to the team's defended services, analyzes it via hot-swappable parsers, supports defensive rules, presents traffic in a web UI, runs team-supplied exploits against explicitly allowlisted competition targets, extracts flags, and submits eligible flags to the competition's checker.

Target **Linux, 2 vCPU, 2 GiB RAM**; game services might run on the same host. Keep CPU/RAM predictable, bounded buffers/queues, no required external databases, brokers, container daemon, Suricata, or eBPF. For v1, native Linux on x86_64; other platforms optional. Instrument and benchmark; never claim an unmeasured RAM throughput guarantee.

## 2. Firm scope decisions

- **Two always-on independent Rust processes**: `hydra-proxy` (data plane) and `hydra-core` (control plane). Spawn bounded, transient, sandboxed exploit subprocesses when needed. Source can be one Cargo workspace and optionally built as two binaries.
- **Proxy**: universal bidirectional TCP relay and UDP relay. Unknown protocols **must be passed through unchanged**. HTTP is a parser atop the general relay, not a mandatory intermediary. Protocol detection is optional; user can force a profile per service.
- **Plugin SDK**: built-in/native Rust parsers and **hot-loaded Rust-compiled WASM** parsers. Optional declarative framing profiles (raw, delimiter, fixed-length, length-prefix). No Lua requirement. Plugins expose metadata/schema for typed protocol fields used in UI and rules; support stateful per-direction/per-session decode, optional decrypt when appropriate keys are available, and bounded compute/memory. No universal decryption claim.
- **Core↔Proxy**: Protobuf3/gRPC on **Unix domain socket** `/run/hydra/proxy.sock`. `hydra-core` connects to proxy. Proxy streams **parsed events, not raw network packets**. Raw byte samples may be requested separately on demand from a bounded proxy-side disk ring. Core sends commands/config snapshots and bounded diagnostic probe requests to configured upstreams; no general packet injection into arbitrary sessions.
- **Persistence**: single-owner SQLite WAL in Core for users, sessions, service configs, rules, audit, indexed parsed events, farm and flags. Core serves static embedded SPA and REST/OpenAPI API. Proxy holds the last-known-good config independently and applies compiled rules locally; **no RPC per network packet**.
- **No Round Manager, no Service Monitor**. Minimal technical health/status counters for Hydra itself are permitted and required. Scheduling for farm is interval/manual; not tied to rounds.
- **No eBPF implementation**; preserve pluggable `NetworkFilterBackend` abstraction, with initial userspace implementation. Additional backend is future work only.
- **All accounts have identical full rights**. No roles, no RBAC, no public HTTP registration. Authenticated users can create accounts through the console or `POST /api/v1/users` (explicit user amendment 2026-10-18), with actor-derived audit. CLI bootstrap/recovery remains available through `hydra-core create user <username> <password>`; additionally support interactive no-password-in-argv mode and document argv leakage.

## 3. Identity, auth, audit

- JWT access: **60 seconds**. JWT refresh: **10 hours absolute from login**, refresh rotation without extending that deadline.
- Use Argon2id salted password hashes; Ed25519/EdDSA signing, persistent protected key. Sessions and revoked refresh token identifiers tracked in SQLite; logout, deactivation and detected refresh token reuse revoke as defined in the contract.
- Web: refresh JWT in HttpOnly Secure SameSite=Strict cookie; access token in frontend memory, Authorization bearer; same-origin/CSRF protection. Rate-limit auth and cap work/memory spent hashing on shared host. Swagger behind authentication or on trusted management interface.
- Every **mutation** has real actor identity (`user`, `system`, `local_cli`, `service`), timestamps and append-only audit event, including before/after redacted metadata, source/request ID, success/failure. `created_by`/`updated_by` are derived server-side from auth, never client supplied. Scheduled runs record system actor **and** `initiated_by` user. Do not invent user authors for automatically observed network traffic. Audit access to raw diagnostic samples. All users may manage any accounts, but ensure a safe CLI recovery path.

## 4. Services and relay

A service has id, TCP/UDP transport, listen endpoint, backend endpoint, protocol profile id and enabled flag. Initial deployment mode: port mapping; backend often moves to loopback/new port. Clearly document that ordinary reverse proxy **changes source IP observed by backend** and that transparent proxy preserving backend-visible original source is outside the initial MVP unless tested explicitly.

TCP: correct full-duplex bytes, ordering, backpressure, EOF/half-close, fragmented messages, TLS passthrough when not terminated, connection timeouts. UDP: preserve datagram boundaries and payload bytes, correct bidirectional client-to-upstream session mapping and idle TTL, bounded session table. Transport fast path must be independent from gRPC and SQLite. Avoid plugin failure crashing the relay.

## 5. Protocol system and inspection

- Built-in HTTP/1.1 parser with headers, method, path, body metadata (bounded inspection), message direction; HTTP/2, WebSocket and TLS termination are future integrations unless implemented and tested. Unknown/encrypted raw traffic remains pass-through.
- Protocol SDK standard events: flow_open, flow_close, parsed_message, rule_hit, flag_finding, counters/event_loss. `ParsedMessage` exposes typed fields (`string`, signed/unsigned int, bool, double, bytes, nested object/list), direction, protocol id, message type, optional sample reference. No mandatory JSON on wire.
- Frame formats: raw, delimiter, fixed size, length prefix (endianness, offset, length semantics, maximum frame length). Allow user to upload signed/hashed or at least checksum-validated WASM plugins, test against saved samples, activate without proxy restart. Atomic profile version swap; existing stateful sessions retain old plugin version unless safely migrated.
- Plugin error, panic or fuel/time/memory exhaustion ⇒ disable that decoder for affected flow or degrade to raw passthrough, emit an error event; never corrupt forwarded bytes.
- Crypto plugins may decode known custom formats using available keys/session state, but without keys cannot decrypt; inspect-after-decrypt only when available. Logging decrypted content must be opt-in/redacted.
- Flag scanner supports configurable per-game regex on decoded traffic and farm stdout/stderr. For TCP search across read chunk boundaries with bounded overlap/known max flag size; separate per direction. Passive detection **does not imply prevention of an already forwarded leak**. To block message/response content, inspect-before-forward with safe framing, explicit buffering and limits.

## 6. Filter Manager / Rule Engine

Each rule: UUID, name, description, service id, protocol field paths, expression AST (`all`/`any`/`not`/`predicate`), operators (`eq`, `ne`, `contains`, `prefix`, `regex`, `gt`, `lt`, `in`, `not_in`), priority, `shadow|enforce`, action, optional TTL, backend, owner and edit attribution, `created_at`, `updated_at`, `hit_count`, desired/applied enabled and revisions, apply status/errors.

Available actions include `allow`, `alert`, `tag`, `log`, `block` for safe parsed HTTP, `drop_connection` for TCP, `drop_datagram` for UDP, and bounded `rate_limit` keyed on fields such as source IP. Only apply actions that match the transport's semantics; cannot arbitrarily drop bytes from unknown TCP stream. Missing parser field must not be treated as matching. Use linear-time regex engine and cap input size, nesting, rules, state cardinality and evaluation time.

UI: searchable sortable filter list with **creation date, author, last editor, hits, priority, enabled ON/OFF**, edit, delete, bulk toggle, and pending/applied/error state. Click toggle Core updates desired revision in SQLite, records actor, pushes snapshot over gRPC; Proxy validates and atomically swaps compiled rules then ACKs the applied revision. UI must not claim an unacknowledged rule is active. Filter activation amendment 2026-10-17 (explicit user instruction): enabled rules default to Enforce and apply directly on creation/edit/enable after Proxy ACK. Shadow mode and saved-traffic testing are optional diagnostics, never enforcement prerequisites.

Keep network filtering backend trait independent of userspace implementation; do **not** implement eBPF or require nftables in MVP.

## 7. Traffic Explorer

Traffic UX amendment 2026-10-17: hide streams with actual enforced blocks by default, with a Show blocked checkbox to include them. Highlight connections containing detected flag findings. Give each service a consistent color stripe on its stream cards. Shadow hits and backend HTTP errors do not count as blocked. See docs/traffic-view.md.

Core receives only parsed telemetry batches and indexes `Flow`, `Message`, `Finding/RuleHit`. Browse/filter by service, time, transport, source/destination, direction, parser fields, matched rule/flag. Flow detail shows bidirectional conversation, timestamps and structured fields. For configured TCP and UDP services, Proxy retains a rolling tail of up to 64 KiB per direction and gzip-compresses each completed flow for up to 24 hours, subject to a configurable per-service disk quota (default 128 MiB for gzip payloads and sidecar metadata) and oldest-first eviction within that service. Settings saves apply the history quota live through Core-to-Proxy control, retry after reconnection, and persist across restart. Retention is enabled by default per service and can be disabled live. Captures exceeding the tail, active-history limit, write queue or storage quota are marked incomplete or counted as lost; relay forwarding continues unchanged. Core fetches a completed history only on demand through authenticated, audited `GET /traffic/flows/{id}/history`; the RPC identifies a known flow and never accepts a destination. Raw bytes default to hex with explicit UTF-8 and ASCII views. Decoded profiles replay saved parsed messages; unknown protocols show metadata and retained bytes without inventing decoded content. UDP history retains whole datagrams in capture order, including empty datagrams, with direction, timestamp and sequence metadata; at most 1,024 datagrams are retained per flow. Both transports support saved-history filter preview with explicit partial/unavailable states. Existing one-shot `GetSample` remains available for the next live chunk.

Proxy raw sample disk ring with hard quota, generation/version reference to avoid stale offsets; no unlimited packet capture. Core SQLite WAL with one batched writer and bounded queues. Normal telemetry may drop on overload but must count/notify losses. Critical events have finite persisted spool, monotonically sequenced `(proxy_epoch_id, seq)`, Core durable commit before ACK, replay/dedup on reconnect. If critical spool is exhausted, make it observable; cannot promise unbounded guaranteed delivery.

## 8. Exploit Farm

Core manages exploit uploads and versions, manual and interval schedules, explicit competition **target allowlist**, per-target and batch-mode execution, logs/status, resource/time/concurrency caps, enable/disable, cancel, retry and result history. Support Python, ELF and shell entrypoints subject to sandbox and on-host runtimes; Rust executables prebuilt elsewhere. Run exploit scripts in isolated subprocess/sandbox (Linux namespaces via bubblewrap or equivalent + cgroups v2; if dependency unavailable, fail closed for untrusted uploads, never silently run unsandboxed). Separate maximum memory and CPU budgets; default concurrency 1 on 2 vCPU/2 GiB. No Docker daemon, Celery, Redis, or rounds.

Output parser extracts flags from stdout/stderr with bounded logs, records originating exploit/run/target. Worker must not receive checker submission secret or Core DB access; network limited to competition allowlist. Background actions carry original enabling user's attribution.

## 9. Flag Vault and Submission

Captured flags come from explicitly authorized farm/manual sources; unknown findings require triage. Defensive proxy findings retain only value-free stream detection metadata: do not store own_exposed values or digests in the flag vault or dedicated finding spool. Migration removes legacy defensive vault entries. **Never automatically submit proxy-observed/own flags.** For captured values: validate regex, hash-based dedup, save securely and show original flag text to authenticated users in Flag Manager, persist queue and status, batch if checker supports, handle retries/backoff, submit with HTTP/TCP/custom adapter against configured checker; preserve response and provenance. External exactly-once delivery cannot be assumed. Submission credentials stay out of inherited worker environments and telemetry. Authenticated Python submission diagnostics retain exact input flags and original stdout/stderr, with no flag redaction; gzip history is paginated and audited. Preserve each captured full regex match exactly, without trimming, case conversion, decoding or capture-group substitution. Support pasted text import via the configured regex, paginated flag browsing with service/state filters and sorting, configurable flags per Python call, and a manual run count independent of automatic enablement.

## 10. HTTP and gRPC contracts

Public REST: OpenAPI 3.1, `GET /openapi.yaml`, Swagger UI `/docs`, version prefix `/api/v1`; use axum + utoipa or contract-first equivalent. Internal protobuf package `hydra.proxy.v1`, Rust tonic/prost codegen. Use provided **draft** contracts as starting point in `contracts/`; update contracts with implementation; validate by OpenAPI parser, `buf lint` or `protoc`, generate code and integration tests.

HTTP surface: `/auth`, `/users` (no registration), `/services`, `/filters` (+ toggles), `/traffic`, `/protocols`, `/farm/targets`, `/farm/exploits`, `/farm/runs`, `/flags`, `/submissions`, `/audit`, `/system/proxy`, `/diagnostics/probe`. Pagination cursor, UTC times, consistent errors, optimistic revisions, idempotency keys for external effects. Frontend live traffic via SSE; 60s access token expiry requires renewal and reconnect.

Internal gRPC service groups: `ProxyControl` (status and atomic service/rule snapshots, plugins), `ProxyTelemetry` (parsed normal and critical streams plus ACK), `ProxyDiagnostics` (on-demand sample and bounded test connection to configured competition service). Authenticate local core using Unix socket owner/mode and peer credentials where supported. Proxy should store last-known-good config and restore it after restart; Core reconciles state/revisions on reconnect.

## 11. Resource strategy and delivery

- Single Rust workspace: proxy binary, core binary, shared types/SDK/proto, embedded static web UI. Systemd unit(s) and minimal config with example safe deployment.
- No resource-unbounded allocations, no per-packet gRPC, no full PCAP by default. Run critical network operations independently from UI/core. Prioritize backend game services and proxy over farm. Avoid writing individual packets as SQLite rows.
- Proposed **measurable engineering targets (not claims)**: combined Core+Proxy idle RSS <=300 MiB on baseline machine without active farm, with documented test command; under a representative bounded traffic load target <=500 MiB and no unbounded growth. Real limits and throughput must be recorded after benchmarking.
- No Round Manager, game health checks, service checker integration, Suricata/eBPF, Kubernetes, Redis, PostgreSQL, Docker daemon or native dynamic `.so` plugin support in MVP.

## 12. Delivery requirements and acceptance

Build a **runnable end-to-end vertical slice** before broadening modules. Final minimum working product includes real binaries, migrations, OpenAPI + generated documentation, compiled proto, browser UI, tests and Linux setup/README. Stubs, fake data and unimplemented UI actions do not count as completed features.

Required acceptance tests:
1. TCP unknown protocol: exact bidirectional bytes including fragmentation and half-close; Core disconnect does not interrupt proxy.
2. UDP: two distinct clients correctly receive matching upstream replies; bounded idle eviction.
3. HTTP: parse User-Agent/path, apply shadow vs enforce rule, toggle ON/OFF, persist actor/timestamps and verify applied revision after ACK.
4. Plugins: load a Rust-compiled WASM parser without proxy restart; restart/reconnect and decode sample; parser failure does not drop relay.
5. Telemetry: protobuf contains parsed events only; on-demand sample by id, expiry handled; important events deduped and ACKed after durable commit. Completed TCP and UDP history is gzip-compressed on Proxy's private disk and fetched by flow id through bounded diagnostics.
6. Auth: create user CLI, login, access expires at 60s, rotating refresh never exceeds 10h absolute, logout/reuse revocation; any two users have same permissions and audit identifies which user acted.
7. Farm: sandboxed upload→run on allowlisted test target→flags extracted→queue→mock checker; timeout, network deny and resource limit enforced.
8. Performance: document RSS/CPU and queue loss metrics on 2 vCPU/2 GiB test machine; confirm no runaway memory for traffic or malformed inputs.

## 13. Clarification/precedence rules

1. This `SPEC.md` overrides earlier chat ideas and any stale wording in `contracts/API_DESIGN_v1.1.md` where they disagree.
2. Provided OpenAPI and Proto are **draft baseline**, not immutable or proven compilable; fix gaps, regenerate, and document breaking changes. Make REST and proto representations mutually consistent.
3. Prefer working, testable implementation over premature optimization or speculative subsystems.
4. Do not block on non-critical ambiguity: choose a documented safe default, then proceed. Escalate only choices changing exposed protocol compatibility or legitimate competition scope.

## 14. Mandatory source tree and scalability

`REPOSITORY_STRUCTURE.md` is the **normative repo layout and dependency-boundary contract**, not an optional style guide. Use its virtual Cargo workspace, two application crates, only genuinely needed reusable crates, feature-organized Core modules, layered Proxy, one canonical Proto path, generated OpenAPI snapshot, per-crate tests and explicit end-to-end harness. Maintain no Core↔Proxy package dependency, no SQLite or auth inside Proxy, no transport socket code inside Core, and no direct UI↔Proxy connectivity. Include structure-compliance checks in Phase 0 and every milestone. If a necessary deviation arises, record the reason and tests in `DECISIONS.md`.
