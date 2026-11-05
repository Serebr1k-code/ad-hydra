# Service forwarding with reversible flag substitution

Status: proposed for review. No product code has been changed for this feature.

## Intended behavior

An operator enables forwarding for one Hydra service and chooses an explicitly configured competition team. Clients continue connecting to the service's existing Hydra listener. Hydra sends requests to the selected team's service, replacing each recognized original flag with a generated substitute. Returning occurrences of known substitutes become the corresponding original flags. Unrecognized content remains byte-identical. This is an inline application relay, rather than a passive traffic mirror or an IP-transparent proxy; the remote service sees Hydra as its peer.

The user selected generated matching substitutes. A substitution must be stable across connections and Proxy restarts: a flag written in one connection can be retrieved in a later connection. This extends the present product scope because deliberately storing reversible mappings of defensive flags is necessary for this mode. Mapping values remain separate from Flag Manager, submission queues, telemetry and ordinary audit records.

## Approach

Three implementation choices were considered:

1. Regex replacement on individual socket reads is small but incorrect: read boundaries are arbitrary, and flags can span reads.
2. Protocol-specific rewriting covers a single service well but duplicates mapping and relay logic across protocols.
3. A shared reversible substitution engine with explicit framing adapters supports TCP and UDP while preserving existing relay and inspection interfaces. This is the recommended approach.

Rewriting remains disabled by default. Existing services retain their behavior. The shared TCP/UDP relay parent holds optional rewriting context. No new operating-system thread is created for each session; bounded async tasks run on Tokio's existing worker pool.

## Configuration and destination

Service configuration gains optional forwarding settings, separate from its existing local upstream: enabled state, selected team ID, flag pattern and TCP framing. Core resolves an enabled team from the existing Farm team catalog to a numeric competition address. The target uses the configured service port, with an explicit override if the competition's remote port differs from the local backend port. Arbitrary per-request destinations, DNS routing and traffic injection are not introduced.

The Services form exposes a forwarding switch, team selector, target port, flag pattern and framing controls. Show the effective upstream and pending/applied configuration state. Normal service edits retain optimistic revision checks, actor-derived audit and atomic Proxy snapshots. Existing sessions retain their original target and rewriting policy; changes affect new sessions. Prevent known local listener loops, and document that multi-team relay loops cannot be detected from local configuration alone.

OpenAPI, generated frontend types, the canonical protobuf and wire conversions carry the same configuration. Add protobuf fields compatibly; missing fields mean disabled. Proxy validates the complete snapshot before activation, including startup restoration of last-known-good configuration.

## Matching and generation

The first generator supports fixed-width printable ASCII flag patterns built from literals, character classes, capture groups, concatenation and exact repetition. Examples include `[A-Z0-9]{31}=` and `FLAG\{[a-f0-9]{32}\}`. Reject unsupported or ambiguous generation patterns at configuration time with an actionable error; never silently generate values that fail the supplied pattern. General regex matching for existing detection rules is unchanged.

Generated substitutes preserve the original match length and pattern. Literal prefixes and suffixes stay intact. Use operating-system randomness, check uniqueness, reject identity substitutions, and use a bounded collision retry count. Original values and substitutes must occupy disjoint sets within a namespace; close/reject a request that would introduce an ambiguous reverse mapping.

Namespaces include service identity, resolved destination and flag format. Another service or target cannot restore a mapping from the wrong namespace. Repeated appearances use the same mapping. Server responses only reverse known substitutions; they never invent mappings for unrelated flags from the target service. Replacements are simultaneous and never recursively rewrite their own output.

## Framing and byte integrity

UDP rewrites each complete datagram independently and preserves datagram boundaries, including empty datagrams. A flag is not assembled across datagrams.

TCP uses an explicit framing adapter. Initial adapters cover supported HTTP/1.1 messages, delimiter-terminated messages, and bounded length-prefixed messages with configured width, offset, byte order and length semantics. Match only application payload spans where framing bytes must be excluded. Flags split across TCP reads are assembled before substitution. Incomplete messages at EOF, unsupported framing and buffer/deadline exhaustion are reported explicitly; the rewrite mode must not silently downgrade to forwarding original flags.

HTTP must preserve original header spelling, order, whitespace, duplicate headers and body framing. Equal-length substitutions preserve Content-Length. For chunked messages, decode entity-body spans and scatter equal-length replacements back into their original chunk payload positions, preserving all chunk sizes, extensions and trailers. Do not search chunk metadata as body data. Existing parser restrictions, supported request methods and upgrade behavior need explicit validation; unsupported Expect/upgrade/HTTP2 cases must be rejected rather than falsely advertised as transparent. HTTP body framing and buffering limits must be documented in the UI.

The implementation makes no universal claim for TLS ciphertext, compressed/encoded flag values, signatures or checksums covering modified bytes. Those services require an appropriate protocol adapter that can decode and re-encode correctly. Other bytes remain unchanged in supported plaintext messages. Unknown protocols continue passing through unchanged when rewriting is disabled.

## Mapping durability and bounds

Proxy owns a protected, separate mapping journal under its existing private state directory; Core remains the sole SQLite owner. Persist each newly allocated mapping before forwarding its substitute, so a successful upstream write cannot depend on a mapping lost during restart. Use async bounded storage work, never Core/gRPC/SQLite calls in the relay path. Reusing an existing mapping does not require a disk write.

The journal and in-memory indexes have finite byte and entry limits. Do not automatically rotate mappings while the remote service may still hold substituted flags. At capacity or on persistence failure, reject affected new mappings before forwarding and emit a value-free operational error. Existing durable mappings remain usable where safe. Validate file ownership/permissions and reject symlinks. Recover only a provably incomplete final journal record; interior corruption fails closed. Configuration changes do not silently erase earlier namespaces.

Audit configuration changes and explicit mapping maintenance with actor provenance. Automatic allocations have service/system provenance and value-free counts. Never place original/substitute values in audit, general logs, gRPC event telemetry or Flag Manager. Existing authenticated traffic-history access keeps its current audit controls.

## Inspection and UX

Client-facing stream views should show original client requests and restored responses. Internal transformation counters report mappings created/reused, replacements in each direction, persistence failures, capacity failures and framing failures, without flag values. Preserve existing filtering, history, overload protection and per-service metrics. State the effective destination in service configuration so an operator can distinguish local service mode from forwarding mode.

## Validation and deployment

Use synthetic flags and benign local fixture services. Cover all TCP split positions, repeated flags, two-direction interleaving, separate write/read connections, restart restoration, namespace isolation, unknown response flags, collisions, malformed/truncated journal recovery, capacity and disk errors, configuration changes, HTTP Content-Length and chunk boundaries, half-close, timeout, UDP datagram integrity and unchanged disabled-mode relay behavior.

Verify byte-for-byte identity outside replacement spans. Validate public contracts, protobuf compatibility, dependency directions, formatting, Clippy, frontend tests, crate tests and a two-process smoke test. Measure enabled-mode throughput/CPU/RSS on a bounded workload rather than claiming nginx-equivalent performance. Deploy the completed binaries/UI to team-1 using the existing deployment scripts, leaving forwarding disabled until a target and supported service format are configured. Do not alter another team's VM.
