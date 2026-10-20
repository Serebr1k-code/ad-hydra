# Creating and using capture decoders

This guide covers the complete path from choosing a parser to writing a Rust/WASM decoder, uploading it, testing it against a saved capture, and enabling it for a service. Hydra's parsers only inspect bytes that the TCP/UDP relay has already received. They never change forwarded bytes. Use this only for services you are authorized to operate.

## Choose the right decoder

In **Protocols → Capture decoder**, choose a decoder and service, then select **Apply decoder**. Search the decoder field by name. Built-in choices are:

| Choice | What it emits | Use it for |
| --- | --- | --- |
| `raw` | No parsed events. Captured bytes remain available from Traffic → Captured bytes. Hex is the default view; UTF-8 and ASCII are optional views. | Unknown/binary protocols where exact bytes matter. |
| `utf8` | `utf8.text` string events, split into bounded chunks. Invalid sequences appear as U+FFFD. | Line-oriented or otherwise unframed text where best-effort text is useful. |
| `http` | Bounded HTTP/1.1 request/response fields supported by Hydra's parser. | Supported HTTP/1.1 services. |
| `<id>/<version>` | Typed events from an installed WASM decoder or framing profile. | A proprietary/binary protocol or a supported frame boundary. |

For a fixed-size, delimiter, or length-prefixed protocol, first try **Create framing profile** on the Protocols page. This avoids writing code and emits a typed `protocol.payload` bytes field. Use WASM when a frame needs semantic parsing, validation, nested fields, or stateful interpretation. `raw` is deliberately not a fictional parser: it creates no empty or fabricated message.

Completed TCP flows have a separate retained-history view. For each service, **Retain completed stream history** is on by default and can be changed while Hydra is running. Proxy retains the latest 64 KiB in each direction for each of at most 256 active flows. Completed payloads are zstd-compressed in the Proxy private state directory, expire after 24 hours, and use a configurable disk quota per service (128 MiB by default, including compressed payloads and sidecar metadata) with oldest-first eviction within that service. Change the limit live in Settings; a global 65,536-record index ceiling also bounds memory/inode use. Decompression is capped at 128 KiB per flow. Settings exposes a live zstd level from 1 through 19 (default 1); legacy gzip archives remain readable. Proxy restart preserves completed records; active, unfinished histories are lost.

Open a closed session in **Traffic → Captured bytes** to view its retained history. Raw display defaults to a directional hex dump; UTF-8 uses replacement characters for invalid sequences, and ASCII substitutes non-printing bytes. The direction selector keeps client-to-server and server-to-client streams separate. Tail truncation is shown per direction, and history loss is marked when active-flow or write/storage limits prevent a complete record. No raw payload is sent on the continuous telemetry stream: the authenticated Core API requests a bounded completed record from Proxy on demand, and Core audits the read. The saved parsed messages for a selected decoder are still replayed in **Decoded conversation**.

This history is best-effort, not packet capture. Each flow stores only its latest 64 KiB per direction; flows beyond active limits may have no record, and records can expire or be evicted. The explicit one-shot capture button remains available on live sessions: it captures the *next* chunk only, is capped at 16 KiB and expires after 30 seconds. That sample is distinct from completed history and does not stream raw packet data over gRPC.

## A Rust/WASM decoder project

Start from the working example at [`plugins/examples/length-prefixed`](../plugins/examples/length-prefixed/). It provides the ABI exports, a real parser, metadata, and a reproducible build. Copy it into a new independently built crate, then change the parser and metadata. Keep the example's `Cargo.toml` path to `crates/hydra-protocol-sdk` correct relative to the new directory.

Install the Rust WebAssembly target once on the build machine:

```sh
rustup target add wasm32-unknown-unknown
```

The example parser implementation illustrates the core loop:

```rust
let input: DecodeInput = serde_json::from_slice(input_bytes)?;
let mut pending = input.state;
pending.extend_from_slice(&input.bytes);
let mut messages = Vec::new();

// Parse only complete protocol records. Keep an incomplete suffix in `state`.
while let Some((record, consumed)) = next_complete_record(&pending)? {
    let fields = parse_record(record)?;
    messages.push(ParsedMessage {
        protocol: "my-protocol".into(),
        message_type: "record".into(),
        direction: input.direction,
        fields,
    });
    pending.drain(..consumed);
}

let output = DecodeOutput { messages, state: pending };
```

`next_complete_record` and `parse_record` above are protocol-specific functions to implement. A real decoder must cap its own pending buffer before extending it, reject impossible lengths without allocating from them, and return an error/trap for malformed state. A trap disables inspection for that flow; it never alters the transport bytes. Keep each feed efficient: Hydra calls the parser for each bounded transport read.

The plugin's ABI exports and JSON types are exact and versioned in [`PLUGIN_ABI.md`](PLUGIN_ABI.md). In summary, export `memory`, `hydra_abi_version() -> i32`, `hydra_alloc(i32) -> i32`, `hydra_decode(i32, i32) -> i64`, and `hydra_dealloc(i32, i32, i32, i32)`. Return the output pointer in the high 32 bits and output length in the low 32 bits. The input and output are SDK JSON structures (`DecodeInput` and `DecodeOutput`), not protobuf telemetry structures. Compile against `hydra-protocol-sdk` so your field values serialize with the expected tagged representation.

### Typed fields and metadata

Use a `BTreeMap<String, Value>` for each message's fields. Supported values are `String`, `Signed`, `Unsigned`, finite `Double`, `Bool`, `Bytes`, `Object`, and `List`. Give each message a stable protocol name, message type, and the input direction. Fields are searchable in Traffic and available to filters through their dotted field paths.

Export `hydra_metadata() -> i64` to declare fields for the Protocols UI. Return module-owned UTF-8 JSON using the same packed pointer/length representation:

```json
{
  "name": "my-protocol",
  "fields": [
    { "path": "my_protocol.command", "field_type": "string" },
    { "path": "my_protocol.sequence", "field_type": "unsigned" },
    { "path": "my_protocol.payload", "field_type": "bytes" }
  ]
}
```

The metadata schema is informational and validated at install time. It does not make the parser correct automatically; emitted paths and types must match it.

### Build and validate locally

From the repository root, build a release module:

```sh
cargo build --locked --manifest-path path/to/my-decoder/Cargo.toml \
  --target wasm32-unknown-unknown --release
```

The artifact is `path/to/my-decoder/target/wasm32-unknown-unknown/release/<crate_name>.wasm` (hyphens in crate names become underscores). Before upload, test boundary cases in the decoder crate or with Hydra's sample-test screen: fragmented records, multiple records in one input, both directions, invalid lengths, empty payloads, and the maximum supported frame.

Hydra enforces these ABI/resource bounds: module 4 MiB; no imports (including WASI); 200,000 fuel per initialization/metadata call; decode fuel is 200,000 + 1024 × (input bytes + direction state bytes), capped at 4,000,000 per feed; guest memory 4 MiB; input 64 KiB; serialized input/output and explicit state 64 KiB; 32 messages per feed; 256 typed values total; nesting depth 8; keys 128 bytes; strings/bytes 8192 bytes; metadata 8192 bytes and 64 declared fields. These checks are in addition to the parser's own protocol-specific bounds. See [`PLUGIN_ABI.md`](PLUGIN_ABI.md) for pointer ownership, traps, compatibility, and exact validation rules.

## Upload, test, and activate

1. In **Protocols**, enter a stable decoder ID and immutable version, select the `.wasm` artifact, and upload it. The console computes the SHA-256; the API requires the matching `X-SHA256` header. The upload is checksum-validated and compiled before it is listed.
2. Open **Schema / profile** to inspect the decoder's metadata. Missing metadata is allowed for older ABI v1 decoders, but declaring it makes fields discoverable.
3. Capture representative bytes from a live authorized session in **Traffic → Captured bytes**. Capture is opt-in and bounded. The resulting sample ID is used for deterministic decoder testing; the saved sample expires.
4. Use **Test saved sample** for the uploaded version. This evaluates the sample without activating the parser or forwarding any traffic. Inspect emitted fields and fix the decoder/version if the result is incorrect. Versions are immutable: upload changes under a new version.
5. In **Services**, add or edit a service, search **Capture decoder** for `custom:<id>/<version>`, then **Save service**. This activates the decoder for newly created flows. Existing sessions retain the version they started with. The Protocols page also retains its activation shortcut.
6. Choose **Use service parser** in the service's activation row to remove the custom override. New flows then use the service's configured built-in profile (`raw`, `utf8`, or `http`). Existing sessions keep their original decoder until they close.

The equivalent API operations use the public REST API documented by Swagger at `/docs`:

```sh
sha256=$(sha256sum my-decoder.wasm | cut -d' ' -f1)
curl -X PUT "https://hydra.example/api/v1/protocols/my-protocol/1.0.0" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/wasm" -H "X-SHA256: $sha256" \
  --data-binary @my-decoder.wasm

curl -X POST "https://hydra.example/api/v1/protocols/my-protocol/1.0.0/activate" \
  -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" -H "Idempotency-Key: $(uuidgen)" \
  -d '{"service_id":"my-service"}'
```

Use a fresh access token: access JWTs expire after 60 seconds. The browser handles refresh automatically. The sample-test route and request shape are exposed in Swagger and the Protocols page. Do not put credentials or captures in source control.

## Important limits

Hydra does not decrypt TLS or infer application plaintext from encryption. HTTP/2, WebSocket, arbitrary compression, and protocol-specific authentication are not automatically decoded. The UTF-8 view simply decodes the captured bytes with replacement characters for invalid sequences. WASM checksums detect content changes but do not attest to who authored a module. The WASM sandbox has no filesystem, network, clock, or host callbacks. New versions do not migrate parser state from existing sessions.

### Select an uploaded module for a service

1. In **Protocols**, enter a protocol ID and version, choose the compiled `.wasm`, then **Upload**. Wait for `installed`; validation failures cannot be selected.
2. In **Services**, add or edit a service. Search **Capture decoder** and select the uploaded `custom:ID/version` entry.
3. **Save service** applies the listener and activates that module. The services table shows the selected module/version. Reopen the editor to change it; choosing raw, UTF-8 or HTTP removes the custom override.

New connections use the selected decoder. Existing connections keep their decoder version. If saving reports a partial failure, the service is already saved: retry **Save service**, or refresh to inspect the pending activation. The module upload and activation shortcuts on Protocols remain available.

The browser acceptance test exercises real upload, service selection, activation,
switching to a built-in and retries after a lost activation response. Build an
example first, then run it against local binaries:

```sh
cargo build --locked --release --manifest-path plugins/examples/length-prefixed/Cargo.toml --target wasm32-unknown-unknown
(cd frontend && bun run build && bun test)
cargo build --locked -p hydra-core -p hydra-proxy
HYDRA_WASM_SAMPLE="$PWD/plugins/examples/length-prefixed/target/wasm32-unknown-unknown/release/hydra_length_prefixed_example.wasm" python3 tests/e2e/browser.py
```

