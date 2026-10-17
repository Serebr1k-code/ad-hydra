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
`http.user_agent`, `network.src_ip`, `network.src_port`, `network.dst_ip`,
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
