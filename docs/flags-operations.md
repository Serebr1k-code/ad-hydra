# Flag vault and checker operations

Proxy critical findings retain flow and service provenance. `own_exposed` values are defensive evidence: they cannot be submitted, recaptured manually, or promoted by triage. Unknown findings may be dismissed, marked defensive, or explicitly captured by an authenticated user against an enabled authorized farm target. Triage is recorded once with actor, request ID, before and after metadata; values remain redacted in REST.

Flag and submission lists accept `offset` (up to 1,000,000), `limit` (1–200, default 100), and `search` (up to 128 characters). Search matches identifiers and category/state; flags also match digest and target. Offset pages use creation time plus ID ordering. Concurrent inserts may shift pages; this is bounded browsing rather than a snapshot export.

Checker endpoints and credentials are operator configuration, never HTTP input. Built-in HTTP and HTTPS use POST JSON with `flag` and `idempotency_key`, plus an Idempotency-Key header. TCP sends JSON lines and accepts exactly `OK`. HTTPS verifies the configured host using the explicitly supplied DER CA certificate and connects only to the configured socket address. It does not follow redirects or resolve a caller-supplied host.

The declarative CustomTcp adapter supplies a request template containing `{items}` and optionally `{secret}`. Items expand to a JSON array of flag/idempotency-key objects. Batch size is 1–32; request size is at most 64 KiB, reply at most 4 KiB, timeout five seconds. `accepted_prefix` applies to the entire batch, so use batch sizes above one only with checkers providing an atomic batch acknowledgement. Per-item checker result protocols need another adapter implementing the extension interface. No subprocess is launched by an adapter.

Submissions persist a stable ID before network effects and reuse it on retries, including restart recovery. Exactly-once acceptance depends on the external checker honoring that key; an acknowledgement can be lost after checker acceptance. Attempts stop after ten. A later defensive observation quarantines pending submissions of that digest, including restart reconciliation, and frees queue quota. A request already sent before the observation cannot be recalled. Recovery, claims and results are audited. Stored replies preserve bounded status/body evidence with known flags and configured credentials redacted; HTTP headers are omitted. Vault retention is bounded to 100,000 flags, critical event receipts to 1,000,000 and pending submission queue to 5,000. No external checker was contacted during validation; adapter tests use owned loopback toy listeners only.

New submitter configurations select the built-in ForcAD adapter paused. Configure a numeric IPv4/IPv6 address and port (default 8080), then enter the team token before enabling or requesting a manual run. The token is write-only in the API; leaving its password field blank retains the saved value. The adapter uses Python's standard library to send `PUT /flags/` with an `X-Team-Token` header and exact JSON flag strings, in uploads of at most 100. Larger manual runs are split across uploads. Accepted and already-stolen verdicts complete the item; own and invalid/expired flags are terminal; service-down, unavailable-game, invalid-token, transport and unknown responses stay queued for retry. Existing saved custom scripts keep their 240-flag maximum and whole-batch exit-code behavior.

Operator `operations.json` HTTPS checker example (the certificate file is bounded to 64 KiB; supply one certificate source):

```json
{"kind":"https","address":"192.0.2.10:443","host":"checker.example","path":"/submit","bearer":null,"ca_certificate_path":"/etc/hydra/checker-ca.der"}
```

A small private DER certificate can instead be supplied as `ca_certificate_der`, a JSON byte array. The hostname must match the certificate and the configured numeric address remains the actual destination. Provision the appropriate CA independently; no insecure certificate-verification bypass exists.

Custom atomic TCP batch example:

```json
{"kind":"custom_tcp","address":"192.0.2.10:9000","request_template":"{items}\n","accepted_prefix":"OK","secret":null,"batch_size":8}
```

Use `batch_size: 1` unless one acknowledgement means the whole batch was accepted. These reserved documentation addresses are examples, not test destinations.
