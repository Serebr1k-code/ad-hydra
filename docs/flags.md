# Flags and submission

Open **Flags**. Captures from Farm and manual imports enter a persistent queue even
when automatic submission is paused. Set the flag regex under Automatic flag
submitter. Hydra keeps the complete regex match exactly as captured: no trimming,
case conversion, URL/base64 decoding, or replacement by a capture group. Values
must be valid UTF-8, at most 256 bytes. Python receives each flag as one positional
argument in `sys.argv[1:]`, without shell expansion.

## Paste flags

Paste text into **Import flags from text** and optionally select its service and
target. Import extracts matches using the saved regex and reports matched, added,
queued and duplicate counts. A request accepts at most 64 KiB of text and 256
matches. Existing values are deduplicated; accepted duplicates are not resubmitted.
Invalid imports fail atomically. The authenticated Flag Manager displays exact flag text instead of its digest.
The paginated response includes `value`; the legacy array stays redacted.
Python submission logs show original flag text. Digests are internal SHA-256 identifiers used for deduplication.

The table has independent pagination, page size, search, service and state filters,
and ascending/descending sorting by creation time, service, target, state,
category or flag text (the API also retains digest sorting). Service sorting uses Farm catalog names when available.
Submission history has its own pagination.

## Send automatically or manually

Upload or paste the Python submitter and list any uv packages. Set **Flags per
Python call** from 1 to 240 and save. Enabled automatic sending checks for new due
captures within about one second while idle. A running child or dependency
installation can delay dispatch. Each cycle takes up to the configured batch size.

For manual sending, choose **Flags to submit now** (1–5,000) and click **Run saved
submitter now**. Save edits first: the button uses the saved configuration. This
works while automatic sending is paused and leaves it paused. The request takes
the oldest queued/retry entries, capped by the chosen count, and splits them into
calls of the configured size. Retry entries can be explicitly retried before their
scheduled backoff. Only one manual request can be pending/running. Its status and
actual attempted count appear beside the button. A new request does not bypass
the single-child limit.

Exit zero accepts the whole Python call. Failures retry after at least 30 seconds,
up to ten attempts. A checker may accept a request before a process fails; external
exactly-once delivery is not guaranteed. Editing or pausing can stop subsequent
calls; an already claimed child may finish. A restarted Core marks an interrupted
manual request and recovers its in-flight flags to retry. Recent manual request
metadata is retained for up to 500 completed requests; audit records retain actor
provenance. See [Farm runtime and zstd log retention](farm.md).

Click **Resubmit** on a failed flag to requeue it with a fresh attempt budget.
This explicit action preserves the exact value and records the requesting user;
accepted or currently in-flight flags are not restarted. Automatic submission
must be enabled to send queued flags, or use Run saved submitter now while paused.

## Defensive findings and migration

Proxy defensive findings no longer store own_exposed values or digests in the
flag vault or new dedicated finding spool records. Migration 18 removes legacy
own_exposed entries, their submission/triage rows, and leaves nullable value-free
flow/service detection metadata so Traffic's flag highlights continue working.
SQLite secure deletion and a WAL checkpoint clear deleted vault records in the
active database. Traffic capture bytes/history and historical backups can still
contain previously observed bytes. Deployment retains a rollback database backup.

## HTTP compatibility

Generated Swagger/OpenAPI is authoritative for payload schemas:

- `GET /api/v1/flags` retains the existing array response.
- `GET /api/v1/flags/page` returns `items`, `total`, `offset`, `limit`; it supports
  `service_id`, `state`, `search`, `sort` and `order` query parameters.
- `POST /api/v1/flags/import` accepts `text`, optional `service_id` and `target_id`.
- `PUT /api/v1/submitter` adds `batch_size`, defaulting to 240 for old clients.
- `POST /api/v1/submitter/run` accepts `{ "count": 100 }`, returns HTTP202 with a
  durable request; `GET /api/v1/submitter` includes its latest status.

Authenticated mutations and sensitive browsing are audited. The existing service,
target and runtime restrictions apply. No new raw-traffic or internal gRPC method
is introduced; the existing finding value field is empty for defensive findings.

## Reproducible checks

```sh
bun install --frozen-lockfile --cwd frontend
(cd frontend && bun test && bun run build)
CARGO_BUILD_JOBS=2 cargo build --locked --release -p hydra-core -p hydra-proxy
HYDRA_BUILD_PROFILE=release python3 tests/e2e/flags_controls.py
```

The acceptance test requires real `uv` (CI pins 0.10.9). It starts an isolated local
Core/Proxy, migrates synthetic legacy data, and verifies exact Python argv,
manual counts, automatic dispatch, paste import, pagination and sorting. It never
contacts a competition checker or service.

## Submission input and output

Open **Submission attempts**, then **Input / output**. Each new attempt retains
its exact input flag array and original stdout/stderr, including echoed flags.
Hydra no longer substitutes [redacted flag] in these Python logs. Inputs/output
use zstd compression; old gzip batches remain readable and are accessible only through authenticated, audited
reads. The paginated list returns metadata; detail loads only the selected
attempt. Retention is7 days/1,000 attempts/64 MiB of combined compressed input and
output; each output stream remains bounded to64 KiB. Historical redacted output
cannot be restored and pre-update inputs display as unavailable.
