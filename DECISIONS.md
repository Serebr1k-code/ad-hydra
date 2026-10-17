# Decisions and contract audit

## 001 — Implement in the supplied workspace
The handoff files are all untracked and there is no existing executable code or commit to isolate. Work stays in the supplied directory so the original specification remains available. No existing documents are removed or committed without instruction.

## 002 — Contracts describe implemented features
The original REST draft advertises the complete future product. Preserve it verbatim as `contracts/openapi.future-draft.yaml`; `contracts/openapi.yaml` is now generated from Core's actual annotated handlers. This intentionally removes unimplemented paths from the active snapshot before public adoption. The draft's login/logout, filter and farm schemas remain reference material, not functionality claims. The legacy notes incorrectly mention roles in JWT validation; SPEC.md's equal account rights wins. REST arbitrary JSON values and protobuf typed values will need explicit numeric/bytes mapping in Phase 2/3.

The canonical protobuf compiles without changes to existing field numbers. Additive status fields 9/10 report rejected flows and dropped UDP datagrams. Diagnostics is not registered; rule/plugin/critical-stream RPCs in the draft return explicit UNIMPLEMENTED until their phases. They are not completed features. Keep the package v1 and preserve existing fields for subsequent phases.

## 003 — Bounded raw relay first
Phase 1 accepts numeric IP:port endpoints and the `raw` profile only. Unknown bytes (including encrypted streams) pass unchanged; no false parsed-message claims. Domain validation rejects duplicate IDs/listeners, zero ports, unspecified/multicast upstreams, and direct relay loops. Bootstrap config is operator-controlled; the authenticated local Core is the service configuration authority. There is no probe or arbitrary packet injection API.

Services <=128, global TCP+UDP flows 1..8192 (default 1024), config/state/gRPC control payload <=256 KiB, TCP buffers 16 KiB per direction, UDP datagrams <=65535 bytes with no per-client send queue, telemetry ring 1024 events, four subscribers with 16 queued batches each, 32 local gRPC connections and 16 streams per connection. UDP overload is lossy and counted; mappings expire on bidirectional inactivity. TCP read/write idle timeout is per direction, including after half-close. Connection rejection and all telemetry/datagram losses are observable through status. Normal flow events are lossy metadata, with no payload capture.

## 004 — Last-good snapshots and local authorization
Prepare every added listener before persistence or live mutation. Reuse existing bound endpoints when upstream changes; accepted TCP connections retain their old upstream. UDP mappings reset when that listener's config changes. Persist a bounded snapshot plus actor/request provenance via 0600 create-new temporary file, fsync, rename, and directory fsync before ACK. Exact same-revision replay is idempotent; older or conflicting same-revision snapshots fail. A restart prefers persisted state over bootstrap configuration. Corrupt state fails startup, never silently resets. Local config directories must be provisioned by the operator.

Unix socket mode is 0600 and peer UID is checked (default Proxy effective UID). Same service UID is supported initially; separate service UIDs require an explicit operator-managed socket ACL plus `allowed_core_uid`. Only an owned stale socket is replaced; an active socket, non-socket or foreign socket is refused. Phase 1 provenance is stored for the last snapshot and logged; it is not the append-only SQLite audit trail required in Phase 2. Management REST is loopback-only until authentication exists.

## 005 — Build and resource evidence
Use tested Rust 1.99.0, edition 2024; Cargo.lock pins dependency resolution. Two build jobs and two runtime worker threads per binary. Swagger assets are embedded from the vendored crate, without a deployment-time Node runtime or CDN. Host measurements are reported with conditions; they are not validation of a 2-vCPU/2-GiB target unless run on that target. SQLite/WASM/filter/frontend crates are added only with their real phase implementations.

## 006 — Implemented control plane supersedes initial phase notes
The Phase 1-only statements above describe that milestone. Core now owns Argon2id/Ed25519 authentication, 60-second access tokens and rotating absolute10-hour refresh sessions; append-only SQLite audit; services/filter CRUD; traffic indexing/SSE; protocol upload/activation; Farm and Flag Manager APIs. The SPA is small native ES modules embedded in Core, with generated OpenAPI endpoint helpers and TypeScript declarations. No deployment Node runtime is needed. Native rustls HTTPS avoids requiring a third always-running process. REST numeric parsed fields represent int64/uint64 as strings to preserve browser precision.

Only Core owns SQLite. The service/filter desired snapshot is one transactional aggregate in the service repository to preserve cross-feature references. Filter HTTP handling remains a separate vertical module. Core serves only loopback, with explicit HTTPS origin and secure refresh cookies. UDS actor metadata is attribution; Unix peer UID is the credential.

## 007 — Contract compatibility and bounded payloads
Active OpenAPI is generated from annotated handlers and diff-checked; the original future draft stays preserved. Canonical protobuf adds atomic `ApplyConfigSnapshot` (service and rule revision/provenance agree), status counters9..13 (connection/datagram/rule/critical/sample losses), explicit `CaptureSample`, and saved-sample `TestProtocol`. Existing field numbers and methods remain. ProxyDiagnostics, plugins and critical streams are now implemented. No generated source is checked in. Saved service/rule state is capped2MiB; bootstrap JSON256KiB; module uploads4MiB, decoded messages64KiB, admission limits and per-method message bounds apply separately.

## 008 — Parsing and enforcement
Raw remains transparent. Native HTTP supports complete strict HTTP/1.1 requests, not responses, chunked, upgrade, HTTP2 or TLS decryption. Buffers cap64KiB, headers16KiB, bodies49152bytes, pending request deadline5seconds. Complete valid prefixes are evaluated before fallback for malformed suffixes. Unsupported framing falls back unchanged, with diagnostic metadata; Expect is passed through to avoid100-continue deadlock. Blocking a later pipelined request closes the connection rather than injecting an out-of-order response.

WASM is import-free, interpreter-hosted, checksum-validated, fuel/memory/output bounded. Existing sessions retain versions; eight guest sessions and sixteen installed versions cap memory. SDK is independent of binaries. ABI1 exchanges bounded JSON in guest memory; internal gRPC transfers typed parsed events. Unknown original message size is0. Native generic declarative framing profiles beyond raw/HTTP are not yet exposed; the real Rust WASM example implements u16 big-endian length framing.

## 009 — Critical replay and explicit samples
Normal metadata is lossy; cumulative rule hits reconcile separately. Defensive flags use a2MiB live durable spool with another2MiB atomic-write budget and bounded64-command admission, separate from normal events. The spool keeps its own persisted epoch across process restarts; status process epoch still identifies restarts. ACK follows durable flag/loss persistence plus Core epoch/sequence highwater. Sixteen historical spool epochs are bounded and fail closed on exhaustion. Admission is not fsync: a crash can lose unflushed queued observations. Exhaustion/loss is visible, never unlimited guaranteed delivery.

Raw samples record the next chunk only after an authenticated audited request for a live flow. Sixteen pending captures, sixteen queued writes,256 disk slots of16KiB and30-second expiry cap storage4MiB. UUID generations expire across restart. REST410 represents pending, expired, evicted or absent samples. Samples are separate on-demand diagnostics, never a raw stream. Configured-service probes cap request/reply4096bytes, concurrency1 and5seconds; no destination address is accepted from HTTP.

## 010 — Farm and submission adapters
Immutable exploit uploads are bounded2MiB; one transient worker executes numeric allowlisted targets. Linux bubblewrap namespaces plus delegated cgroup v2 CPU/memory/PID controls are required; missing setup fails without unsandboxed fallback. Current networking adapter supports IPv4 TCP and per-target execution; multi-target batch and UDP jobs are explicitly rejected. The supervisor owns a fixed-destination Unix bridge; workers have no host network route or submission credentials. Scheduled transitions use system actor with the initiating user preserved.

Flag matching is operator-configured, at most256bytes per flag. Defensive findings are own_exposed and never eligible for submission. Captured/manual findings require an enabled explicit target. Hash/category deduplication, durable queue, stable external idempotency IDs, at most 10 attempts with backoff are real. HTTP JSON and TCP JSON-lines adapters are bounded plaintext protocols for trusted local endpoints/tunnels. Custom/batch/native HTTPS checkers remain unimplemented. Raw checker replies are replaced with redacted outcome summaries to avoid echoing flags/credentials; this is an intentional privacy tradeoff from preserving arbitrary raw response bodies.

## 011 — Fixed-window rate limits
`rate_limit` is implemented with key, requests, window_ms and transport-valid on_exceed. Snapshot-wide cardinality is1024, key values scalar and<=256bytes, window50ms..1hour. Missing keys do not match. Capacity exhaustion follows on_exceed and emits explicit loss metadata. Shadow evaluations update counters without enforcement. A rule snapshot replacement resets rate windows. Units are inspection decisions: complete supported HTTP messages, UDP datagrams and raw TCP connection/read decisions; this is not byte shaping or a bandwidth guarantee. Prefer safe HTTP message predicates for request-rate limits.

## 012 — Offline maintenance audit
The `hydra-core apply` recovery command is a Core-owned CLI mutation, so it opens the configured private Core SQLite database and writes intent before the Proxy call plus result afterward, with matching local_cli UID/request identity. It cannot bypass audit merely because the HTTP daemon is stopped. It remains an offline recovery tool: a direct Proxy snapshot can diverge from Core's desired revision and needs an operator reconciliation through Core; ordinary service changes belong to REST/UI. Read-only CLI status does not require SQLite.

## 013 — Completion of previously deferred surfaces
The earlier phase limitations record historical delivery boundaries. Generic raw/delimiter/fixed-size/length-prefix profiles are now immutable uploaded versions using the same tested activation mechanism as WASM. Additive protobuf InstallProtocol fields7/6 carry framing JSON/schema metadata; optional WASM hydra_metadata exports preserve ABI1 compatibility. Additive DeactivateProtocol restores the configured service parser for new flows while existing sessions retain their decoder. Completed idempotency keys cannot remove a later activation. Native and WASM schema metadata stays bounded and is validated once at install.

Farm now implements per-target/batch IPv4 TCP/UDP fixed endpoints, aggregate deadlines, explicit batch target provenance and immutable retry attempts. Duplicate endpoint aliases in batch are rejected. Runtime directory guards clean up on cancellation after worker group termination. These new worker paths were not executed after the user's restriction on exploit-execution tests; pure framing/validation and SQLite checks are separate evidence from kernel enforcement.

Checker adapters now include verified HTTPS with an operator-pinned DER CA and declarative CustomTcp atomic batches. Endpoint and credentials remain operator-only; no adapter executes code. Bounded redacted response bodies/status preserve outcome evidence. Unknown triage requires explicit authorized target provenance. A defensive observation quarantines pending matching-digest submissions; an already-sent external request cannot be recalled. External idempotency remains a checker responsibility.

## 014 — HTTP provenance, typed values and operator controls
All mutating API dispatches retain a correlated durable intent/result audit, including rejected operations, in addition to transactional feature audit. Unauthenticated attempts use a service observer identity. Timeout/overload errors retain JSON error shape and request IDs. Results cannot turn multi-process operations into atomic transactions; callers must check persisted operation state after timeout. Sensitive settings/configuration export reads are audited and contain no signing/checker secrets.

Users can edit/reset/deactivate/reactivate any account with optimistic revisions; creation remains CLI-only. Password changes and deactivation revoke sessions. Bulk filter changes validate the entire bounded selection and commit one snapshot revision; each selected rule retains before/after audit. Saved-field creation reads the exact typed value from Core storage, creates a shadow rule, and still requires testing before enforcement.

Parsed int64/uint64 REST values now serialize as decimal strings, including rule snapshots; readers accept previous numeric JSON for stored-state compatibility. Protobuf still uses native signed/unsigned64. Existing rule edits may require repeating the saved-traffic test because canonical fingerprint representation changed. No protobuf fields were removed or renumbered. Migrations10–14 are ordered and checksum-verified by Core only.

## 015 — Separate deployment identities and measurements
Production units now use separate hydra-core/hydra-proxy identities with a shared control group, explicit mode0660 UDS and exact configured Core peer UID. Core state remains private to its UID; Proxy does not gain database/key access. Development retains mode0600/same-UID defaults. No host account or systemd installation was performed.

A reproducible loopback benchmark records actual cgroup limits, app CPU affinity, RSS/CPU, verified payloads and telemetry loss. A two-CPU/2GiB scope on a larger machine is documented as constrained-host evidence, not a physical target-machine claim. Short runs cannot establish long-term memory growth guarantees. See docs/PERFORMANCE.md.

## 016 — Vue bundle embedded by Core
The requested Vue 3/TypeScript console replaces the hand-maintained browser JavaScript while retaining the existing HTTP contracts and two-process deployment. Bun/Vite are build tools only. A frontend build produces ignored `frontend/dist`; Core's Cargo build embeds its document and hashed assets through a generated `OUT_DIR` map. Cargo deliberately does not install dependencies or run Vite. Missing or older bundles fail with an explicit frontend build command. Freshness compares input/output modification times rather than introducing an additional hashing dependency; preserved timestamps require the documented clean frontend rebuild discipline.

The asset tree rejects symlinks/nonregular files and enforces 256-file/32-MiB build limits. Runtime serving performs exact embedded URL lookup with MIME/nosniff headers, immutable asset caching and a revalidated root document. Hash routing preserves refresh/navigation without a catch-all HTML response for unknown paths. No Rust application dependency direction, public API field or protobuf method changes for this migration. The opt-in browser smoke hides the dist directory while exercising the actual binary, proving frontend deployment files are unnecessary.

## 018 — Session-first traffic workspace
The requested primary view is now a service dropdown above a left-hand TCP session list, with selection opening a Wireshark-style stream pane on the right. Decoded directional messages and explicit ASCII/hex byte samples use authenticated endpoints. No continuous raw packet telemetry or fabricated wire reconstruction was added. The old table/JSON-first presentation is replaced while field inspection, comparison, capture and saved-field filter creation remain available.

The existing traffic query gains optional `newest_first`, default false. True sorts by descending ingestion sequence and treats `after` as an exclusive older-than sequence. Existing default ordering/cursors remain compatible; callers must preserve query order between pages. The sidebar opts in so current sessions appear first and older pages remain stable when new connections arrive. This is an additive OpenAPI change, with no protobuf or database migration. UI request generations, bounded previews and cleanup prevent stale service responses or polling tasks from crossing session/navigation boundaries.

## 019 — Passive native HTTP responses in the conversation

Native HTTP request filtering previously bypassed server-direction decoding entirely. Add independent bounded response state to the native decoder and explicitly feed it from the TCP return path, including EOF for close-delimited replies. Failure disables only response inspection, preserving pending client bytes and request enforcement. Support HTTP/1.0 and 1.1 response status, Content-Length, chunked transfer, HEAD, informational/no-body statuses and connection-close framing. Response inspection holds at most 64 KiB per flow, with at most 128 outstanding request methods; unsupported/oversize responses keep forwarding unchanged and emit a decoder error. Plain UTF-8 text/JSON/XML bodies expose at most 4096 bytes as a decoded `http.body_text` field, with body length, content type and explicit preview truncation; compressed/binary entities are not rendered as text. No arbitrary headers (especially cookies/auth) or raw packet streams are exported. This is passive inspection, not safe response-block enforcement. Existing parsed-event wire/API contracts need no schema change; the native field registry gains additive typed response fields. Historical unindexed responses cannot be reconstructed.

## 020 — Live parsed traffic, complete bounded HTTP headers, and app port mapping

The Traffic page consumes authenticated Server-Sent Events from Core for committed parsed flow/message/loss events. Core publishes only after durable SQLite commit; the stream carries parsed events, never raw packet bytes. On reconnect, the UI reconciles from the REST message API and keeps the newest 500 conversation messages; events remain bounded by existing parser, queue, and SQLite limits. A `ready` frame flushes the HTTP response immediately. Live flow arrivals appear in the session list without polling.

For supported HTTP/1.x messages, initial headers and chunked trailers are now retained as ordered typed name/value pairs, preserving duplicate fields, original casing, and whitespace, and displayed in Traffic. Each initial header/trailer section remains capped at 16 KiB/128 fields, text preview at 4096 bytes, and a stored event row at 64 KiB to match existing SQLite constraints. This supersedes ADR019's decision to omit headers: full parsed headers are available to every authenticated user because Hydra grants equal full privileges. Headers can contain credentials/cookies and inherit that access policy. Unsupported/malformed messages and HTTP/2/TLS remain outside this parser.

Service forms separate the client-facing proxy listener (default `0.0.0.0:9000`) from the application backend (default `127.0.0.1:19000`); operators move the application first, and Hydra does not reconfigure it. Python exploit manifests may contain up to 32 exact `name==version` package pins. `uv` prepares wheels-only, bounded, cached offline environments before the worker is started; package preparation and exploit execution both remain behind the configured fail-closed Linux sandbox. No exploit was run as part of implementation verification.

## 021 — Bounded gzip history for completed TCP flows

The operator requested inspection after a TCP stream has closed. Extend the previous on-demand-only byte sample into per-service automatic retention, enabled by default and switchable live. Proxy keeps the latest 64 KiB per direction for at most 256 active histories, submits completed records through a 64-entry nonblocking queue, and stores gzip payloads in its private state directory. Records expire after 24 hours; oldest records are evicted above a 100 MiB compressed-payload quota. Decompression is capped at 128 KiB. Queue or storage loss does not delay forwarding and increments the history loss counter; tail truncation and byte totals are returned with each completed history.

Core continues to receive parsed telemetry only. It fetches bytes on demand through `ProxyDiagnostics.GetFlowHistory` and exposes an authenticated, audited flow-ID-only REST route; the response contains two directional segments and cannot select a network destination. Existing saved parser messages provide decoded conversations after closure. Raw views default to hex, with explicit UTF-8/ASCII choices. Existing one-shot next-chunk samples remain for live sessions. Service snapshots are JSON, so the default-on field is serde-compatible without a SQL migration. History and parsed-message retention are separate: Proxy history lasts at most 24 hours while Core's existing message retention policy remains authoritative.

## 2026-10-17 — Farm service/team catalog, explicit host runtime and gzip logs

The user explicitly changed the Farm requirement to ordinary host child processes, with Binary/Python/Bash uploads or pasted code, Python dependencies via uv, separately configured service ports and team IPs, direct-team selection, and gzip logs. This supersedes the earlier requirement that every exploit run must be isolated. Host execution is operator-selected in private operations configuration; the prior sandbox remains the default for other deployments and never silently falls back to host execution. Both modes retain configured-target selection, attribution, timeout/cancellation and bounded output. Host jobs inherit Core's OS privileges, not per-job sandbox resource or network restrictions. See `docs/farm.md` for interfaces and deployment semantics.

Farm catalog/run additions are additive migration0016. Legacy target/exploit/run records remain available, with old plaintext logs converted incrementally. Run pagination changes to newest-first row cursors; callers should restart pagination after upgrading. Gzip streams retain at most1MiB each; retention is7days/1,000completed runs/64MiB compressed output. No new permanent process, external database or public arbitrary-host run endpoint is introduced.

## 2026-10-17 — Durable Farm flag queue and editable Python batch submission

The user requested automatic regex extraction from Farm output, a persistent flag queue, and a Python submit script configured like Python exploits, with flags supplied as command-line arguments every 30 seconds. Migration 0017 adds a singleton revisioned config, durable collection clock and bounded gzip batch log history. Farm captures enqueue regardless of the legacy auto-submit switch; own-exposed digest quarantine and selected-run target provenance remain enforced. The editable Farm regex is independent of Proxy's defensive scanner pattern. No raw flags enter audit or submitter diagnostic output.

A configured uv host child receives up to 240 flags/60 KiB payload, no target arguments, and runs at most 120 seconds. One child runs per submitter, and one Farm worker may execute concurrently. Exit 0 acknowledges the whole batch; failures retry no sooner than 30 seconds, at most 10 attempts. Larger windows split into bounded calls. Config edits/pauses stop later chunks from claiming the old revision; active claims complete. Restart/persistence-error recovery preserves pending attempts. External success before a crash is necessarily at-least-once; script/checker idempotency handles duplicates. Existing operator checker adapters remain compatible when no Python config exists. No daemon, external database, protobuf change, or arbitrary-host run API was added. Details and reproducible acceptance commands are in docs/farm.md.

## 2026-10-17 — Live byte limits independent of decoding

The operator clarified that byte rules must stop a transfer while it is still arriving. Add host-owned `body.size` and `transfer.bytes` unsigned counters and evaluate size-dependent rules before forwarding each read. HTTP/1.x body counts reset per request and exclude headers/chunk framing; an independent bounded framing counter never stores body content and survives native content-decoder fallback. Other TCP profiles count received stream bytes per direction because arbitrary protocol request boundaries are unknown; UDP body size is each datagram and transfer size is cumulative per direction. Malformed HTTP framing becomes opaque stream accounting. A matching TCP limit closes the connection before forwarding the offending read; HTTP status injection remains limited to completely buffered HTTP requests.

Size metadata is carried in existing typed event fields, with closure-only metadata summaries for flows without decoded messages so raw rules can be tested against retained evidence. There is no protobuf/REST shape change, SQLite migration, raw packet stream or extra process. New filters default to transport blocking so selecting a live size field does not accidentally require complete HTTP buffering. Byte input uses exact unsigned 64-bit decimal/unit conversion, independent of text decoding and preview limits. See docs/rules-integration.md for field semantics and limitations.

### Nested group removal and explicit enforcement state

### Captured-traffic picker for filter rehearsal

The required exact-rule saved-traffic test was exposed as a JSON message-ID
prompt; enforcement shortcuts consequently failed for new or edited rules with
an opaque policy error. Keep SPEC's rehearsal requirement, but provide a session
and parsed-message picker scoped to the rule's service. Test and enforce reads
the current saved revision, tests that message, and submits a writable-fields-only
Enforce update when it matches. Revision conflicts and test failures prevent the
update; a no-match leaves the existing mode unchanged. Saving in Enforce mode
first saves Shadow and opens this dialog, with an explicit warning that the rule
will observe without blocking until enforcement is applied. Session choices are
bounded (25/page, 100 retained) and messages are bounded to 100 per session.
Existing REST handlers/contracts and the proxy data path are unchanged.

### Immediate filter activation — user product amendment, 2026-10-17

The user explicitly superseded the original shadow-first/test-before-enforce
policy: adding a rule should make it work directly. SPEC now reflects this.
Remove rehearsal prerequisites from create, update, single enable and bulk enable;
keep validation, optimistic revisions, audit attribution and Proxy ACK unchanged.
New UI rules default to enabled/Enforce; saving never demotes an enforced rule or
opens a required test dialog. Shadow and saved-message evaluation remain optional.
Existing modes are preserved; no mass promotion of existing Shadow rules. Keep
historical rule_tests records and the optional test API without a DB migration.
This supersedes earlier decisions making captured-traffic rehearsal mandatory.
The from-message creation endpoint also defaults to Enforce instead of Shadow;
this changes activation behavior while preserving request/response fields and
status codes. Its Explorer button and confirmation text now reflect activation.

### Traffic visibility, flag markers and service colors

The user requested blocked traffic hidden by default, flagged connections
highlighted, and a service-color line on stream cards. Console Show blocked is
opt-in; API include_blocked defaults to previous include-all behavior and filters
before pagination when false. A connection with any actual enforced block/drop is
classified as blocked, not a Shadow hit or an ordinary backend error. Metadata is
computed from retained events/indexes; no new unbounded state or storage migration.
Add compatible RuleHit.blocked field 5 and a domain hit indicator to distinguish
actual exceeded-rate-limit enforcement from allowed rate-limit matches. Legacy
normal block/drop hits remain recognizable; older rate-limit hits are ambiguous.
After durable critical commit, publish only redacted flag_detected SSE metadata,
never flag values. Lifecycle events hydrate indexes to cover cross-stream ordering.
Service stripes use a stable hashed HSL color, with text labels for service, Flag
and Blocked so color is supplementary. See docs/traffic-view.md for limits.

### Exact captured flags, operator batches and defensive vault removal

The user's flag workflow amendment supersedes the former mandatory 30-second
collection window and own_exposed vault retention. Preserve full UTF-8 regex
matches byte for byte and queue captured values while paused. Dispatch due
captures within about one second when enabled and idle, with configurable
1–240 flags per Python call. Manual durable requests select 1–5,000 queued/retry
flags while paused; the existing single worker splits them into bounded calls.
Retain retry backoff, redacted gzip logs, revision checks and actor auditing.

Migration 18 purges legacy defensive vault/triage/submission records, preserves
value-free detection metadata for Traffic highlights, and prevents reinsertion of
own_exposed vault rows. Move metadata incrementally to avoid duplicating its whole
SQLite table. Proxy omits defensive flag values in new critical spool records;
traffic bytes/history and rollback backups remain governed by their own retention.
Captured values and existing submitter configuration are preserved. Add optional
batch_size with default240, paginated /flags/page alongside legacy /flags, atomic
regex text import, and /submitter/run. Generated OpenAPI/frontend types reflect
these compatible additions. See docs/flags.md for limits and restart behavior.

### Flag text in the authenticated manager

The user explicitly requested original flag text instead of Digest in the table.
Expose optional value on the audited authenticated /flags/page response and render
it as escaped text with preserved whitespace in Vue. Keep SHA-256 digest for
deduplication/API compatibility, legacy /flags arrays redacted, and submission
logs redacted. This supersedes the earlier redacted-manager UI decision.

Explicit Submit on a failed captured flag requeues the same submission with
attempts0, a due-now timestamp and the requesting user. Only explicit actions reset
failed attempts; automatic duplicate captures do not. Accepted/inflight/queued
entries remain idempotent; a full pending queue rejects the retry atomically.

### Original submission diagnostics and streaming Farm logs

The user requested removal of [redacted flag] from Python submission diagnostics,
exact per-attempt input/output, Farm output beyond1 MiB, and new/existing flag
counts. New attempts persist the exact argv array and original stdout/stderr as
gzip; historical redaction cannot be reversed. Authenticated batch list returns
metadata summaries, detail returns bounded diagnostics; audit never copies raw
flags/output. Retention counts inputs as well as outputs (1,000/7days/64MiB).

Host Farm logs use32KiB gzip chunks and absolute byte offsets,64KiB compatibility
previews, and64KiB UI pages. Shared64MiB compressed/512MiB raw chunk quotas rotate
oldest history explicitly, avoiding a per-process1MiB cutoff or unbounded RAM.
Sandbox worker bounds remain, with all captured target output archived. Distinct
run/target/digest observations classify first-committed new flags versus already
captured values; repeats in one process do not inflate counts. Legacy counts are
partial and labeled. Observations contain no extra plaintext, cap at1million,
and cascade with run deletion. Migrations19–21 preserve existing configuration.
