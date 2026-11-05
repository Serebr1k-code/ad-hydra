# Forwarding services with reversible flag substitution

Hydra can route an existing service listener to a configured competition team. This is an inline application proxy; it does not preserve the original client's source IP and is not passive mirroring.

1. Add the target team's numeric IP and name in **Farm → Teams**, and enable that team.
2. Open **Services → Edit** on the service to forward.
3. Enable **Forward to another team with flag substitution**, select the team and enter its public service port.
4. Enter the flag format and select the correct TCP message framing. UDP uses whole datagrams automatically.
5. Save and wait for the configuration to show **applied**. A target pointing back into a local Hydra listener is rejected by Proxy; check the displayed apply error.

The local application backend setting is retained so disabling forwarding returns new connections to it. Saving a team IP change does not silently reroute an existing service: edit/save forwarding to resolve the new IP. TCP connections retain their original target and format; existing UDP bindings reset on service configuration changes, following Hydra's existing UDP behavior.

## Flag formats

Use a fixed-width printable ASCII regex, for example `[A-Z0-9]{31}=`, `FLAG[A-Z0-9]{8}`, or `FLAG\{[a-f0-9]{32}\}`. Literals, character classes, capture groups, anchors and exact repetitions are supported. Variable repetitions, general alternation and non-ASCII classes are rejected. Equivalent simple alternatives may be normalized by the regex parser into a supported character class.

Hydra generates a unique substitute satisfying that pattern, with exactly the same length. Literal prefixes/suffixes stay intact. Repeated originals use the same substitute across connections and Proxy restarts. Known substitutes in replies are restored; unrelated remote flags remain unchanged. Mappings are isolated by service identity, resolved target endpoint and flag pattern. Changing framing alone retains the namespace; changing target or pattern selects another namespace. Values never enter Flag Manager or automatic submission.

## TCP framing

A socket read is not a request boundary. Select the service's real application framing:

- **HTTP/1.1:** preserve start lines, original headers and chunk metadata. Rewrite request targets, eligible header values, entity bodies and eligible trailer values. Chunked bodies are assembled logically and replacements are scattered back into the same chunk payload positions, preserving sizes, extensions and trailers. HEAD, interim responses and request/response pipelining are tracked. Content-Length remains unchanged because substitutions have equal length.
- **Delimited:** choose LF or CRLF in the UI; the API also accepts another delimiter of1–16 bytes. Rewrite message payloads while leaving the delimiter unchanged.
- **Length-prefixed:** configure a1-,2- or4-byte length field, its byte offset0–16, endian order and whether the length includes the header. Payload bytes after the length field are rewritten; the header stays unchanged.

Each message is limited to64KiB, including framing, and an incomplete frame has a5-second deadline. A newly started partial message receives its own deadline. HTTP headers are limited to16KiB/128 fields and outstanding HTTP requests to128. UDP flags cannot span datagrams. Partial messages at EOF and invalid/unsupported framing fail closed instead of silently sending original flags. Normal disabled-mode TCP/UDP relay behavior remains unchanged.

TLS ciphertext, compressed or encoded flag values, HTTP upgrades/CONNECT/Expect and signatures/checksums covering modified data need a protocol-specific adapter. The current mode does not decode these. Recognized HTTP encoding/integrity headers cause rejection; arbitrary application-level signatures cannot be detected automatically. Close-delimited HTTP responses are buffered until EOF within the same size/deadline limits. No universal transparent rewriting claim is made for unknown or encrypted protocols.

## Durable mapping storage

Proxy stores mappings in `forwarding-mappings.jsonl` beside its last-known-good state file. The directory/file must be privately owned (0700/0600); the file is exclusively locked, checksum validated and bounded. New mappings are synced before any substitute is forwarded. Reusing a mapping does not write to disk. Core still exclusively owns SQLite; Proxy never sends flag values over gRPC.

The mapping journal contains original and substitute values: protect it as service state, separately from submission credentials. Back it up together with the Proxy state and any remote application data you depend on. Losing or manually deleting it prevents restoration of previously stored remote substitutes. The file is intentionally not part of rotating request history.

Limits are8192 mappings and16MiB for the entire Proxy, across all namespaces. Mappings are never automatically evicted while remote data could reference them. Capacity exhaustion rejects new mappings; existing mappings remain usable. A failed append disables new allocations until recovery. A provably incomplete last record is truncated on restart; corrupt completed records stop startup without discarding mapping history. A too-small flag space or an original colliding with an existing substitute is rejected rather than made ambiguous.

Services shows value-free global counts for durable mappings, reuse, restoration and failures alongside live traffic metrics. The Traffic viewer sees original client requests and restored server replies. Existing filtering/history and authenticated diagnostic access remain in place. No substitute is exposed as a submission candidate.

## API and build

`POST /api/v1/services` and `PATCH /api/v1/services/{id}` accept optional `forwarding` (omit/null disables):

```json
{
  "team_id": "configured-team-id",
  "target_port": 9000,
  "flag_pattern": "[A-Z0-9]{31}=",
  "framing": {"type": "http1"}
}
```

This is the nested object, alongside the existing service fields and expected_revision. Other framing objects are `{"type":"datagram"}`, `{"type":"delimited","delimiter":"\n"}`, and `{"type":"length_prefixed","offset":0,"width":4,"little_endian":false,"length_includes_header":false}`. Service reads contain the resolved `target_address`; writes select a catalog team and port, never an arbitrary destination. Mutations retain server-derived actor attribution and revision conflict handling. Swagger documents the actual API; protobuf adds optional typed forwarding configuration and value-free statistics compatibly.

```sh
bun run --cwd frontend build
cargo build --locked -p hydra-core -p hydra-proxy
cargo test --locked --workspace
python3 tests/e2e/forwarding.py
scripts/check-contracts.sh
```

All fixture traffic uses synthetic flags and loopback services. CI runs the two-process forwarding test. Deployment does not enable forwarding for an existing game service automatically.
