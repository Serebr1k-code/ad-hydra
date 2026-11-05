# Service protection and saved favourites

## Configure protection

Open **Services → Edit → Enable service overload protection**. Protection is optional for existing services. The form offers a starting preset: 128 active sessions, 50 new TCP connections/second, 1,000 UDP datagrams/second, 8 MiB/second in both directions combined and a 30-second idle timeout. These are starting settings, not measured capacity or a guarantee that a checker will always get a slot. Use the live charts to establish normal traffic and adjust each service independently.

Limits aggregate by service, not source IP. This is intentional for A/D networks where legitimate clients and hostile traffic may share a gateway address. Hydra cannot distinguish those clients using their IP alone. A service budget prevents one listener from using every available flow slot when its configured cap is below the global cap. The global Proxy `limits.max_flows` ceiling remains effective; budgets do not reserve a guaranteed number of slots for each service.

The configurable fields are:

| Field | Meaning |
| --- | --- |
| `max_active_flows` | Maximum simultaneous TCP connections or UDP client-endpoint mappings for this service. |
| `tcp_connections_per_second` | TCP admission tokens per second. Zero disables this rate. |
| `udp_datagrams_per_second` | UDP datagrams in both directions combined per second. Zero disables this rate. |
| `bytes_per_second` | Incoming transport-read bytes in both directions combined per second, before decoding/capture/forwarding. Zero disables this rate. |
| `idle_timeout_ms` | Expire inactive sessions. TCP activity in either direction keeps the connection active. |

Rate buckets allow one second of burst traffic. If a single transport read/datagram exceeds the configured byte burst, it cannot be admitted. Do not configure a tiny byte budget when the service needs large UDP datagrams or TCP reads. Rates are per service, not per connection. Changing configuration does not refill exhausted buckets. Rate/cap changes use shared live state; TCP timeout changes affect newly opened connections.

Enforcement happens before expensive decoder/scanner/capture work and before opening an upstream for a newly admitted session. Excess TCP admissions are closed; if a live TCP read exceeds the byte budget, the whole connection is closed. UDP drops whole datagrams. TCP byte order and UDP packet boundaries are preserved for admitted traffic. There is no growing admission wait queue and no denial event written for every rejected packet. Existing filter rules still apply after transport admission.

These are userspace resource controls. A saturated network link or kernel SYN queue needs protection upstream of Hydra; the relay cannot recover bandwidth already consumed before the VM receives traffic. No firewall rules or IP bans are installed automatically.

## Read live charts

**Services → Live service traffic** shows each service's active sessions, new/rejected sessions per second, client/server bytes per second and UDP drops per second. These use Proxy counters through the authenticated Core API, independently of lossy parsed telemetry. UDP admission counts represent new client-endpoint mappings, not individual requests.

The page polls every two seconds while visible and keeps at most 120 samples per service in browser memory. Charts begin when the page opens. Each line has its own vertical scale and a numeric latest value; these plots do not imply that a connection count and a byte rate share units. Missing samples, reconnects and Proxy counter resets create breaks instead of invented rates. Charts do not create a new monitoring daemon or historical metrics database.

## Save a stream

In **Traffic**, click the star on a stream card or **Favourite** in its detail viewer. Enable **Favourites only** to retrieve saved streams after they disappear from the ordinary rolling list. **Show blocked** still controls visibility of blocked streams. All authenticated users share favourites and may remove them; changes are authenticated and audited.

A favourite protects the retained flow metadata, decoded messages and saved raw TCP/UDP history from ordinary traffic count, byte and age rotation. Raw favourites are checksummed zstd archives in a separate Proxy directory. Restarting Core or Proxy preserves favourites. Removing a favourite makes its data eligible for ordinary rotation again.

Favouriting cannot recover data that was already evicted or was never captured. Capture remains a bounded tail: up to 64 KiB per direction, with at most 1,024 whole datagrams for UDP. The viewer reports unavailable/partial raw history and incomplete decoded history explicitly. An active stream is saved with a snapshot and updated when it completes; after an interrupted Proxy run, its saved snapshot is readable but marked partial. These snapshots have `closed_at_ms=0`; final forwarded counters are unavailable (represented by zero in the history API), and the viewer labels them unknown rather than treating observed tail lengths as forwarded totals.

The separate archive has 128 favourite slots and never evicts an older favourite automatically. Core also bounds each favourite's decoded data; saving a retained snapshot that exceeds the admission bounds (128 decoded messages or 900,000 stored bytes) is refused. Further live decoded data stops being retained when its bound is reached and marks the favourite incomplete. Raw archive records are bounded to 2 MiB each. When capacity is exhausted, remove an unwanted favourite before saving another. A temporary Proxy failure leaves a durable pending request and a visible retry state; Core reconciles it after reconnecting.

Both binaries must be upgraded together for the new protection configuration, metrics and favourite control RPC. Old service records default to protection disabled. Existing ordinary gzip histories remain readable.
