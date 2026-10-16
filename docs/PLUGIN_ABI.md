# Hydra parser ABI 1

The Rust SDK builds for `wasm32-unknown-unknown`. No imports are accepted (including WASI): filesystem, sockets, clocks and host callbacks are unavailable. The host uses wasmi with 200,000 fuel per initialization/feed, one memory capped at 4 MiB, one table capped at 1024 entries, and one instance per session. Modules are limited to 4 MiB and must match the supplied SHA-256 before compilation. The registry allows at most sixteen live versions. Activation replaces an immutable Arc snapshot; active sessions keep their old instance/version.

Exports:

* `memory`: exported linear memory.
* `hydra_abi_version() -> i32`: returns 1.
* `hydra_alloc(length: i32) -> i32`: returns writable guest pointer.
* `hydra_decode(pointer: i32, length: i32) -> i64`: packed unsigned output pointer in high 32 bits, length in low 32 bits.
* `hydra_dealloc(input_pointer: i32, input_length: i32, output_pointer: i32, output_length: i32)`: frees both allocations after host copies output.

Input/output use SDK serde JSON structs, not internal telemetry wire messages. Host writes `DecodeInput { direction, bytes, state }`, copies the returned `DecodeOutput { messages, state }`, then calls dealloc. Stateful plugin data is returned explicitly: a session instance may also hold bounded guest globals. The host keeps separate explicit state blobs per direction; guest globals remain session-wide.

Raw input <=64 KiB, JSON input <=512 KiB, output/state <=64 KiB, <=32 messages/feed, <=256 typed values total, nesting <=8, keys <=128 bytes, strings/byte values <=8192 bytes. Signed/unsigned integers, finite doubles, bool, string, bytes, object and list are supported. Recursion/deserialization is also bounded by serde_json's recursion limit before semantic validation.

Any trap/panic, invalid pointer/output, exhausted fuel or memory, invalid field bounds disables that flow decoder. Host returns an error and never edits transport bytes. There is no state migration; incompatible revisions require a new ABI version. SHA-256 ensures content integrity, not publisher identity. The host deliberately does not load native shared libraries.

Build the real Rust sample with `cargo build --manifest-path plugins/examples/length-prefixed/Cargo.toml --target wasm32-unknown-unknown --release`. Its u16 big-endian prefix excludes the two prefix bytes; payload limit is 8192 bytes. Loading tests additionally use WAT-compiled real WASM fixtures to exercise traps/fuel and version retention.

Optional ABI v1 metadata extension: export `hydra_metadata() -> i64`, using the same
high-32-bit pointer / low-32-bit length encoding as decode. It returns immutable,
module-owned UTF-8 JSON (no deallocation callback), capped at 8192 bytes under the
same fuel and memory budgets. Shape: `{ "name": "parser", "fields": [{ "path":
"protocol.payload", "field_type": "bytes" }] }`. Field types: string, signed,
unsigned, bool, double, bytes, object, list. At most 64 field paths of 128 bytes.
Missing export is accepted for legacy ABI v1 modules; registry schema is then null.

Declarative profiles are installed via `PUT /api/v1/protocol-profiles/{id}/{version}`
with `{ "framing": { "kind": "delimiter", "delimiter": [10] }, "max_frame": 8192 }`.
Other kinds: raw; fixed_size with size; length_prefix with offset, width (1/2/4/8),
little_endian and includes_header. Offset is measured from frame start. Header
length is offset+width. An excluded-header length counts bytes following that
entire header; included-header length counts the whole frame. Emitted bytes payload
includes delimiter/header. Raw emits no fictional parsed messages. Max frame is
1..8192, matching the SDK bytes-field bound. Each direction buffers independently.
Malformed lengths, overflow, unterminated frames at the cap and >32 messages per
feed disable inspection with explicit decoder-loss reporting; relay bytes remain
unchanged. Profile versions are immutable and durable; activation snapshots affect
new sessions only. Profiles and WASM versions share the existing 16-version quota.

`DELETE /api/v1/protocols/activations/{service_id}` with `Idempotency-Key` restores
that service's configured built-in parser for new flows. Proxy removes and durably
persists only the override; existing sessions keep their prior decoder. Core keeps
an unacknowledged removal visible as pending and removes its activation row only
after Proxy ACK. Repeating a completed key returns the original success without
removing an override installed by a later operation. The UI calls this action
“Use service parser”. Installed immutable plugin/profile versions remain available.
