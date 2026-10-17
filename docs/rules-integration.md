# Rule engine integration

The Vue Filters page uses a visual rule builder instead of an expression JSON
editor. Add conditions or nested groups, choose `all (AND)` or `any (OR)` for
each group, and select the comparison from the field type. String operators
include equals, contains, prefix, regex match and regex non-match. Numeric
comparisons accept plain byte counts or human-readable binary/decimal suffixes
such as `512kb`, `512KiB`, and `2MiB`; values are sent as typed unsigned
integers. The UI serializes groups and negated regexes to the existing API
expression tree, so no contract migration is needed.

Built-in filterable fields include HTTP method, path, user agent, UTF-8 request
body (up to 16,000 bytes), HTTP body length, observed message size, client/server
IP and port, transport and direction. Protocol decoder fields are available by
their `protocol.*` path. Non-UTF-8 or oversized request bodies are omitted from
the text field rather than silently truncated before matching.

For live upload limits select **Request/body size in bytes (live)** (`body.size`),
**is greater than**, a byte limit such as `524288` or `512KiB`, and **Block
transfer**. Start in shadow mode, test a saved message, then enable enforcement.
The Proxy checks this rule before forwarding every read. Crossing a TCP limit
closes the connection and discards the entire offending read; previously
forwarded bytes cannot be recalled. UDP drops the offending datagram.

HTTP/1.x request bodies are counted per request, excluding headers and chunked
transfer framing. The independent streaming counter handles binary data and
large uploads without retaining the body, including after the bounded content
decoder falls back to passthrough. Other TCP profiles (raw, UTF-8, framing and
WASM) count all received bytes per direction, independently of decoder output;
unknown protocols have no inferred request boundaries. UDP `body.size` is the
current datagram length. `transfer.bytes` is the cumulative received byte count
per direction for either transport, including HTTP headers. Both counters use
saturating unsigned 64-bit integers. Text replacement characters and text
preview limits do not affect byte counts. Malformed HTTP framing falls back to
the opaque stream count instead of trusting an ambiguous body length.

Parsed messages retain `body.size`, `body.size_scope`, `transfer.bytes` and
`direction`, so saved-message tests use the same byte quantities. Flows with no
decoded messages emit at most one metadata-only `transfer/byte_summary` per
nonempty direction at closure, allowing raw byte-limit rules to be rehearsed.
No per-read raw byte stream or per-packet SQLite rows are added. HTTP response
status blocking still requires a completely buffered request; use connection
blocking for an upload that must stop before it finishes. Native and WASM
decoders cannot delay or disable the host's live byte counter.

`hydra-domain` owns pure rule types and typed field values (`rules.rs`).
`hydra-filter-engine` reexports these types and owns compilation, evaluation and
`NetworkFilterBackend` / `UserspaceBackend`. It has no application, transport,
SQLite, Tokio or gRPC dependencies.

Construct `RuleSnapshot { revision, rules }` and call
`CompiledSnapshot::compile(snapshot, &services)` before publishing. Compilation
validates the entire snapshot, resolves service transports, and compiles Rust
linear-time regexes. Publish a successful immutable snapshot in an `Arc`; reject
a failed update while retaining the previous Arc and applied revision.

For each inspected event, call:

```rust,ignore
let context = EvaluationContext {
    service_id: &service.id,
    transport: service.transport,
    fields: &fields,
    now_ms: unix_time_ms,
    safe_http: false,
};
let result = compiled.evaluate(&context);
```

Fields can use flat canonical keys (`http.path`, `http.user_agent`,
`network.src_ip`) or nested `FieldValue::Object` values. Flat keys take precedence.
Values remain typed; no numeric/string coercion or lossy integer conversion occurs.
A missing field, type mismatch, nonfinite float or oversized value is unknown,
including under NOT. AND/OR/NOT use three-valued logic; only true matches.

Rules run by descending priority with ascending rule ID as a stable tie breaker.
Disabled or expired rules do not match; expiration is inclusive (`now >= expiry`).
Each hit reports rule ID, mode and intended action. Shadow hits never enforce or
stop evaluation. Enforced allow, block, drop_connection and drop_datagram are
terminal. Alert, tag and log remain nonterminal hit effects. The caller owns
bounded counters/telemetry and applies the returned terminal action.

`drop_connection` requires TCP, `drop_datagram` requires UDP. HTTP `block` requires
a TCP service with protocol ID `http` and an HTTP status from 400 through 599.
Even then it only matches when `safe_http` is true: the caller must guarantee a
complete parsed message buffered before forwarding. Raw TCP byte/message dropping
is never represented. Stateful `rate_limit` uses a scalar key, request quota,
window_ms and transport-valid on_exceed action. It has 1024 snapshot-wide keys,
256-byte key values and 50 ms–one-hour windows. Exhaustion follows on_exceed
and emits loss metadata; shadow evaluation never enforces. Snapshot changes reset windows.
These count inspection decisions, not bytes or bandwidth.

Hard caps are 256 rules/snapshot, 256 nodes/expression, depth 16,
1 KiB regex pattern, 16 KiB field/expected-value inspection budget, and 256 KiB
compiled regex/DFA caches per regex. Snapshot-wide allocation-free preflight limits
total AST nodes to 512, regex predicates to 16, and expected data to 256 KiB. Field paths are at most 256 bytes; empty
segments are invalid. Built-in field registry accepts `http.method`, `http.path`,
`http.user_agent`, `http.body_text`, `http.body_length`, `body.size`,
`body.size_scope`, `transfer.bytes`, `packet.size`, `network.src_ip`, `network.src_port`, `network.dst_ip`,
`network.dst_port`, `transport.kind`, `direction` and extensible `protocol.*`
paths. Unknown HTTP/metadata paths reject the snapshot. Rule IDs are unique and at most 128 bytes. Regex input is
rejected as unknown rather than truncated into a potentially false match.

The serialized AST preserves draft `all`, `any`, `not`, and `{field,op,value}`
shapes. FieldValue uses explicit `{type,value}` tagging to retain bytes and full
64-bit numeric types. Action uses `{type,...}` tagging, e.g.
`{"type":"block","http_status":403}`. Public REST/wire adapters should map
existing action/action_config and typed protobuf fields explicitly; never change
canonical contracts implicitly to match these internal representations.

Validation: `cargo test --offline -p hydra-filter-engine -p hydra-domain` and
`cargo clippy --offline -p hydra-filter-engine --all-targets -- -D warnings`.

In the visual editor, every nested group has a **Remove group** button. Removing
its last parent child inserts a default condition so the parent stays editable;
review that condition before saving. The root group stays in place.

**Enabled** and **Enforce** are separate settings. An enabled **Shadow** filter
counts matches but allows traffic through, including HTTP block actions. Choose
**Test and enforce** in the filter list, select a captured session and message,
then click **Test and enforce** in the dialog. No JSON or copied message IDs are
needed. **Test only** evaluates saved fields without changing the mode. A no-match
result leaves the mode unchanged; choose a matching message or edit the rule.

Saving an edited or new filter with mode **Enforce** first saves it in **Shadow**
and opens this test dialog. While it is in Shadow it does not block traffic;
closing the dialog leaves it in Shadow. This implements the required saved-traffic
rehearsal for the exact new rule version instead of rejecting the save. Check that
desired and applied revisions agree before checking the service's public proxy
port. Requests sent directly to its private upstream bypass Hydra.

The picker loads 25 sessions per page, retains at most 100 session choices, and
shows at most 100 messages from the selected session. Use **Older sessions** or
**Refresh traffic** to select other retained sessions. If no parsed messages are
available, check the decoder profile and send a normal request through the proxy.
