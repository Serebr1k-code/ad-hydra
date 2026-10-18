# Traffic view

Select a service above the session list, then select a TCP stream to follow its
conversation. Each card has a left stripe with a stable color derived from its
service ID. The service dropdown uses the same colors. Refreshes and pagination
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

## Saved TCP history limits

Enabled services retain a gzip-compressed tail of64KiB per direction after a stream closes. History expires after24hours, or earlier when the100MiB compressed-payload quota or65,536-stream limit evicts the oldest records. Metadata and filesystem allocation add disk overhead. The nonblocking writer queue holds64 completed streams; overload reports capture loss instead of delaying forwarding.

The writer indexes completed files once at startup; appending and expiry do not rescan the history directory. Its private directory must not be edited while Proxy runs. Restart rebuilds the index; on-demand reads still validate permissions and bounded decompression. See [performance measurements](PERFORMANCE.md).
