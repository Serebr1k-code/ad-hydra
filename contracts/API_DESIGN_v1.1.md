# Hydra API v1.1 — draft contract

## Boundary
- **Public:** OpenAPI 3.1 HTTP API on `hydra-core`, `/api/v1`, Swagger UI at `/docs`, specification at `/openapi.yaml`.
- **Internal:** protobuf3/gRPC `hydra.proxy.v1` on Unix socket `/run/hydra/proxy.sock`.
- Two long-lived processes only: `hydra-core` (SQLite, REST, auth, UI, farm, submitter) and `hydra-proxy` (TCP/UDP relay, protocol/WASM, inline filters, telemetry). Farm commands launch bounded sandboxed children.
- The gRPC event stream delivers **parsed events**, not raw packets. Fetch original byte samples only on demand and within a fixed disk/ring buffer budget.
- No Round Manager, Service Monitor or eBPF implementation. Network filtering backend is an abstraction; initially userspace.

## Users and authentication
- CLI **supports** `hydra-core create user <username> <password>`; **recommended** `hydra-core create user <username>` with hidden interactive password, because command-line passwords can be exposed via shell history/process inspection.
- Only CLI can create users. **Every authenticated active user has full, identical privileges. No roles or RBAC.** CLI operations are attributed to the local OS user + UID, not an invented web user ID.
- Any user may disable other user accounts, but safeguards and CLI recovery must prevent permanent operator lockout.
- Passwords: Argon2id with unique salt; initial budget suggestion 32 MiB, time cost 2, parallelism 1, then tune on target 2 GB host; throttle concurrent logins.
- JWT signing: EdDSA (Ed25519), allowlisted algorithm; persistent key file with restrictive permissions. Claims: `iss`, `aud`, `sub`, `jti`, `sid`, `typ`, `iat`, `nbf`, `exp` (roles should be validated against account state for sensitive changes).
- Access JWT: **60 seconds**; returned in JSON and held only in client memory; used in `Authorization: Bearer` header.
- Refresh JWT: **10 hours absolute from login**, delivered as `HttpOnly; Secure; SameSite=Strict; Path=/api/v1/auth` cookie over HTTPS. Rotate on refresh without extending absolute session deadline; store only a digest of its unique identifier in SQLite; revoke token family on detected reuse (and handle multi-tab refresh with frontend single-flight). CSRF header plus Origin check on cookie-based mutations.
- Store session record (`sid`, user_id, created_at, expires_at, revoked_at, refresh_jti_digest) so logout and disabling a user can revoke sessions. For sensitive operations, check session active even if access JWT signature is valid.

## Actor and audit invariant
- Every **mutation** gets `actor_type`, `actor_id`, `initiated_by_user_id` (when background system executes on someone's configured schedule), UTC timestamps and `request_id`. `created_by` and `updated_by` come from verified auth context, never user JSON.
- Central append-only audit table: `id, request_id, at, actor_type, actor_user_id, os_uid, action, resource_type, resource_id, before_redacted, after_redacted, result, source_ip`. CLI and background actions receive system/local-CLI attribution. Service-initiated actions use an explicit service actor. Audit privileged sensitive reads (raw traffic sample access, administrative user/session reads) as well as all mutations. Secrets/passwords/raw flags MUST NOT enter audit payload.
- Filter record: `created_at`, `created_by`, `updated_at`, `updated_by`, `desired_enabled`, `applied_enabled`, `apply_status`, `revision`, `applied_revision`, `hit_count`.
- Core stores desired configuration to SQLite and sends monotonic revision snapshots over gRPC. Proxy validates and swaps atomically, replies with applied revision; UI shows **pending**, **applied** or **error**, not premature success. Proxy reloads last known good snapshot on restart; Core reconciles on reconnect.

## Traffic and delivery
- Normal telemetry is bounded and lossy under overload, with drop counters. Critical events (e.g. leak findings) are replayable from a finite disk spool and acknowledged **after** Core SQLite commit. De-duplicate by `(proxy_epoch_id, sequence)`. If the finite spool is exhausted, raise a critical loss condition; do not promise unlimited reliable delivery.
- Avoid per-packet RPC. Use gRPC streams, small event batches, and backpressure. No full raw payload in streaming protobuf; sample access is a limited exception for diagnostics.
- Explicit RPC deadlines for unary control/diagnostics. For long-lived streams use keepalive, reconnect cursors and cancellation. Socket access controlled by permissions and peer credentials.
- Diagnostics can send bounded application payload only to configured, competition-allowlisted upstreams, not arbitrary destinations or arbitrary injection into established third-party connections.

## REST conventions
- `/api/v1`, UTC RFC3339 timestamps, IDs are UUIDs, cursor-based paging, `problem`-style JSON errors.
- `Idempotency-Key` for mutations causing external effects (run/submit/activate), `expected_revision` for filter state changes, HTTP **202 Accepted** with `apply_status: pending` for asynchronous proxy changes; **409** for concurrent conflicts, **410** for expired traffic samples.
- Frontend live view consumes SSE `/api/v1/traffic/events/stream`, authenticated with short access JWT in Authorization header (not URL). Server closes SSE when access JWT expires (60 seconds); client refreshes and reconnects, resuming via event cursor when available.
- Swagger UI should be behind authentication (or only exposed on trusted management interface). OpenAPI describes REST API; gRPC uses `.proto` and is not shown as REST endpoints.

## Security & resource budgets
- Bind REST API to management interface; HTTPS mandatory when remote. Do not expose Unix gRPC socket publicly.
- All uploads have size limits, SHA-256 digest and provenance; WASM parsers bounded by memory/fuel; exploit farm executed in jailed child processes with cgroup CPU/memory/PID and network allowlist.
- Rate-limit login, cap JSON/body sizes, and redact tokens, passwords, flags and sensitive headers in logs. Maintain minimal technical proxy statistics only; no separate Service Monitor.
- Initial target on a shared 2 vCPU/2 GiB machine: Core and Proxy low hundreds of MiB when idle/moderately loaded; benchmark; no hard performance guarantees.

## Suggested SQL indexes
- `users(username)` unique; `sessions(user_id, revoked_at)`; `audit(at desc, actor_user_id)`; `filters(service_id, created_at desc)`; `flows(service_id, started_at desc, id)`; `messages(flow_id, occurred_at, id)`; `submissions(flag_id, created_at desc)`; `critical_inbox(proxy_epoch_id, sequence)` unique.

## Build plan
1. Authentication CLI, JWT/refresh/session, equal privileges, audit middleware and SQLite migrations.
2. Generate OpenAPI 3.1 from Rust handlers or keep contract-first YAML; CI diff the public specification against the reviewed version.
3. Generate Tonic/Prost code from `.proto`, version package `hydra.proxy.v1`; enforce compatibility checks in CI.
4. Implement rules snapshot and status reconciliation with author provenance; no RBAC middleware.
5. Add bounded parsed telemetry streams and critical ack/spool.
6. Add Traffic Explorer, protocol artifacts, farm, submitter and diagnostics.

Draft design document, not a running service implementation.

## Rules / filters
- HTTP representation: `FilterInput` and `Filter` in `/openapi.yaml`; protobuf transport: `Rule` in `hydra/proxy/v1/proxy.proto`.
- `RuleExpression` recursive AST: `all`, `any`, `not`, or `{field, op, value}` predicate. Namespaced field paths include `network.src_ip`, `transport.kind`, `direction`, `http.path`, `http.user_agent`, `protocol.command`, and `protocol.fields.*`.
- Allowed operators: `eq`, `ne`, `contains`, `regex`, `prefix`, `gt`, `lt`, `in`, `not_in`; comparable types validated against parser metadata.
- `enabled` is desired state. Applied state and revisions are separate; persist creation/update author and UTC time in `audit`.
- `priority` sorted descending; terminal actions stop further decisions, but telemetry-only matches can accumulate. Stable `id` breaks ties; conflicting `ALLOW`/`BLOCK` semantics must be explicit in engine tests.
- `shadow` evaluates and records the action that *would* have been taken, without enforcing. `enforce` executes compatible action on the proxy.
- `action_config.http_status` is valid only for a parsed HTTP `block` action; for generic TCP use `drop_connection`, for UDP `drop_datagram`.
- `action_config.rate_limit` is required for `rate_limit`, counting by `key` with `requests` and `window_ms`; `on_exceed` must be compatible with current transport. Implement bounded cardinality and TTL for counters.
- `expires_at` optional; expired rules are non-enforcing even before Core synchronizes and must be observable in applied state.
- Raw TCP cannot safely remove an arbitrary message without verified framing and protocol semantics. Inspect-before-forward may require buffering; proxy must enforce strict time/byte budgets.
- Rules changes are applied atomically as snapshots; a missing/unknown parser field never silently counts as a match. Expose validation errors, would-block and actual-block counts separately.
- Examples under `examples/rules/`.
