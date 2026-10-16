# Flag integration

Merge `flags::router(flags.clone())` and `FlagsApi::openapi()` into Core's authenticated API and spawn one `flags.run_submitter(shutdown.child_token())`. `Flags::open` validates operator regex/checker settings and applies migrations 7 and 11 using the existing SQLite owner. No checker or pattern is invented by default.

Manual ingestion uses `record_captured(value, target_id, None, actor, request_id)`. Farm uses `record_farm_captured(value, target_id, run_id, initiated_by)`, which verifies the persisted run's target and initiator before capture. Targets must remain enabled. Farm capture audit uses system/farm-runner with the initiating user preserved in provenance. `extract(bytes)` scans at most 64 KiB and returns at most 256 flags, each at most 256 bytes. Automatic captured queueing is opt-in.

Critical proxy ingestion persists deduplicated epoch/event identity and `flag_events.flow_id` / `service_id`, allowing traffic queries to find defensive flags without copying values into normal telemetry. Unknown findings are never automatically captured. Defensive values cannot be recaptured or submitted even through a separate captured record of the same digest.

Authenticated REST exposes `GET/POST /api/v1/flags`, `POST /api/v1/flags/{id}/submit`, `POST /api/v1/flags/{id}/triage`, and `GET /api/v1/submissions`. Reads expose digests and provenance. Mutations require the configured Origin. The triage body supplies a resolution and, for captured resolution, an enabled authorized target. The authenticated server supplies the actor.

See [flags-operations.md](flags-operations.md) for pagination, checker transport and custom batch configuration, redaction, retries and external idempotency limitations. HTTP, verified HTTPS, JSON-line TCP and declarative CustomTcp adapters share the `CheckerAdapter` boundary. No checker endpoint is accepted from REST and no adapter launches a subprocess.
