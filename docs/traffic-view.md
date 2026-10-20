# Traffic view

Select a service above the session list, then select a TCP stream or UDP conversation to follow its
conversation. Each card has a left stripe with its configured service color, with automatic
palette colors for services without an explicit choice. Set colors in Services.
The service dropdown uses the same colors. Refreshes and pagination
keep those colors unchanged.

**Show blocked** is unchecked by default. Streams with an actual enforced HTTP
block, TCP close, UDP drop or exceeded blocking rate limit are hidden. Checking
it includes those streams and labels them **Blocked**. A TCP stream is classified
as blocked when any such action occurred in the connection, including a later
request in an existing connection. Shadow hits, alert/log matches, and ordinary
backend HTTP errors do not classify traffic as blocked. A live connection can
appear before a blocking decision arrives; it disappears when that decision is
received if Show blocked is unchecked.

Connections with a retained flag finding have an amber highlight and **Flag**
label; their conversation header shows **Flag detected**. Findings follow the
configured Proxy flag regex. This classification does not imply submission or
expose flag values in the new notification. Blocked streams can also contain
findings: check Show blocked to inspect them. Defensive findings retain value-free flow/service metadata only; own_exposed flag values and digests are removed from the flag vault. Captured conversation bytes, traffic history and pre-migration backups can still contain observed traffic bytes.

## API and compatibility

Flow list/detail return boolean `blocked` and `has_flag` metadata. Message lists
also return these fields; message blocking uses matching message IDs or flow-wide
hits. `include_blocked=false` filters before pagination on flow/message lists.
Omitting it retains the API's previous behavior (include all). The console sends
it explicitly according to the checkbox. An existing `has_flag` query still works.

A committed critical finding emits redacted SSE `flag_detected` metadata containing
only kind, flow ID, service ID and `has_flag=true`. Flow lifecycle notifications
include retained classification, including findings arriving before normal flow
telemetry. Existing SSE loss/catch-up behavior remains in place.

Protobuf `RuleHit.blocked` (field 5, additive) indicates an actual blocking action,
including an exceeded rate limit. Legacy enforced block/drop hits are recognized
from their action codes. Older rate-limit hits cannot distinguish an exhausted
quota from a permitted request and remain unclassified. Upgrade both binaries to
classify new rate-limit events accurately. No storage migration is needed;
classification reads retained rule events and indexed flag provenance. Indicators
last as long as their underlying bounded event retention.

## UDP support

The default list includes TCP and UDP; cards identify their transport. UDP groups datagrams by client peer until the idle timeout (30 seconds by default). Incoming and outgoing decoded messages arrive through the same live events as TCP. The stream viewer, service selection, flag indicators and blocked checkbox work with these conversations. Closed UDP conversations support the same authenticated history API, raw views and saved-traffic filter preview as TCP. Replay feeds each retained datagram separately, preserving empty datagrams and arrival order; raw-byte matches never span datagram boundaries. A truncated capture reports a partial prediction.

Both transports use the same decoder and rule abstractions, including raw bytes, UTF-8 and compatible WASM fields. UDP `body.size`/`packet.size` count each datagram's bytes; `transfer.bytes` accumulates separately per direction within the UDP session. Enforced UDP blocking drops a datagram. TCP connection closing and HTTP-specific response blocking require their respective transports/protocols; unsupported combinations fail validation.

Completed UDP conversations retain zstd-compressed byte history after idle closure, service replacement or orderly shutdown. The viewer shows each complete retained datagram, direction, capture timestamp and monotonic capture sequence, including empty datagrams. Hex is the default; UTF-8 and ASCII are explicit views. Datagram text is never concatenated across boundaries. Retained parsed messages remain in Explorer under its event-retention policy; live capture still samples the next datagram. Raw UDP has no decoded protocol text by definition: choose UTF-8 or a suitable WASM decoder for parsed fields.

## Saved TCP and UDP history limits

Services with retention enabled retain a compressed tail of up to 64 KiB per direction after a flow closes. UDP evicts complete oldest datagrams to fit this bound, with a separate 1,024-datagram limit per flow (empty datagrams also consume a record). Retained sequence gaps and per-direction truncation flags identify capture loss; observed datagram counts include evicted and blocked datagrams. Datagram timestamps are wall-clock capture times, while sequence numbers define receive order even if the clock changes. TCP retains byte tails without claiming original packet timing. Both transports capture before rule enforcement, so history may include blocked traffic; forwarded byte totals exclude it. Each service has its own disk budget, default **128 MiB**, covering compressed payloads and JSON metadata. Change **Settings → Traffic history → Saved traffic per service (MiB)** to any whole value from 1 to 1024. Saving applies live and persists in the Core configuration file; disconnected Proxy receives the latest value when it reconnects. Lowering the limit rotates each service's oldest records immediately. A busy service never evicts another service's history to meet its quota.

History also expires after 24 hours. A global 65,536-record index ceiling bounds memory/inode growth: when full, only the writing service may rotate its own records; if it has none, the new capture is rejected and loss is reported. Filesystem allocation adds overhead beyond the accounted payload and metadata bytes. The nonblocking writer queue holds 64 completed streams; overload reports capture loss instead of delaying forwarding. Active capture admission is capped at 256 flows; overflow and asynchronous persistence failure increment Proxy `history_dropped`, while closure metadata reports known incomplete captures. A successful closure event cannot guarantee a later disk write will succeed. Existing legacy TCP gzip archives are indexed by their saved service IDs on startup without conversion; missing transport metadata means TCP. UDP payloads carry bounded binary datagram framing and additive sidecar fields. New history responses add transport and datagram metadata without changing existing field numbers. Upgrade both binaries together for UDP history; older readers do not understand the new zstd framing.

The HYDRA header turns red when Core reports Proxy disconnected or the status request fails. It returns to its normal color after a successful reconnect. Connection status and telemetry counters refresh every five seconds; live traffic events do not override the connection state.

The writer indexes completed files once at startup; appending and expiry do not rescan the history directory. Its private directory must not be edited while Proxy runs. Restart rebuilds the index; on-demand reads still validate permissions and bounded decompression. See [performance measurements](PERFORMANCE.md).
