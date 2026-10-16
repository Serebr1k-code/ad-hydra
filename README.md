# Hydra

Hydra is a two-process operations platform for authorized Attack/Defense CTF infrastructure. `hydra-proxy` forwards TCP/UDP and hosts bounded protocol decoders; `hydra-core` owns SQLite, accounts, the HTTPS console, filters, traffic, exploit scheduling, and flag submission. Unknown protocols retain their original bytes. Ordinary reverse proxying changes the source address seen by upstream services.

Read [PROGRESS.md](PROGRESS.md) for verified scope and limitations, [SPEC.md](SPEC.md) for product requirements, and [DECISIONS.md](DECISIONS.md) for implementation choices. The original handoff README is preserved in `docs/ORIGINAL_HANDOFF_README.md`.

## Build and verify

Linux, Rust 1.99.0, Bun 1.4.3, Node 22.18+, a C/C++ linker and `protoc` are required to build. CI pins Node22.23.3 for the frontend tooling. SQLite and Swagger assets are bundled. Vue assets are embedded into the Core executable; deployment needs no Bun, Node, frontend directory, Redis, PostgreSQL, containers, or Docker daemon.

```sh
(cd frontend && bun install --frozen-lockfile && bun run typecheck && bun run test && bun run build)
cargo build --locked --workspace
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
scripts/check-contracts.sh
python3 -m pip install -r scripts/requirements.txt
python3 scripts/generate-frontend.py --check
python3 tests/e2e/relay.py
python3 tests/e2e/http_filters.py
python3 tests/e2e/operations.py
python3 tests/e2e/diagnostics.py
HYDRA_VERIFY_EMBEDDED=1 python3 tests/e2e/browser.py
cargo build --release --locked --workspace
```

Build the frontend before every Cargo build after changing frontend source, configuration, package manifest or lockfile. Cargo rejects missing or older bundles. See [docs/vue-frontend.md](docs/vue-frontend.md) for the Vue layout and development workflow.

The Rust checker-adapter tests need OpenSSL and owned loopback sockets. The browser test also needs Chromium. Network integration tests bind local ephemeral ports. Kernel sandbox integration has additional prerequisites in [docs/sandbox-integration.md](docs/sandbox-integration.md); an ignored kernel test is not evidence that isolation passed.

Build the actual Rust WASM sample with a matching upstream Rust compiler and target standard library:

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.99.0
cargo build --locked --release --manifest-path plugins/examples/length-prefixed/Cargo.toml --target wasm32-unknown-unknown
HYDRA_WASM_SAMPLE="$PWD/plugins/examples/length-prefixed/target/wasm32-unknown-unknown/release/hydra_length_prefixed_example.wasm" python3 tests/e2e/http_filters.py
```

Distribution-patched compilers may require their matching target standard library; mixing different compiler builds produces E0514. The WASM sample is an independent workspace. Guest ABI v1 uses bounded JSON in guest memory; parsed events use typed protobuf fields over gRPC.

## Run locally

Use a private state directory. Start upstream toy services separately; the example maps TCP 9000 to 9001 and UDP 9002 to 9003. Substitute competition endpoints explicitly.

```sh
mkdir -p /tmp/hydra-demo
chmod 700 /tmp/hydra-demo
python3 - <<'PY'
import json,pathlib
root=pathlib.Path('/tmp/hydra-demo')
v=json.loads(pathlib.Path('config/proxy.example.json').read_text())
v['socket']=str(root/'proxy.sock')
v['state_file']=str(root/'last-good.json')
(root/'proxy.json').write_text(json.dumps(v))
(root/'seed.json').write_text(json.dumps(v['snapshot']))
PY
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -keyout /tmp/hydra-demo/tls.key -out /tmp/hydra-demo/tls.crt -subj /CN=localhost -addext subjectAltName=DNS:localhost,IP:127.0.0.1
target/debug/hydra-core --database /tmp/hydra-demo/core.sqlite3 --key /tmp/hydra-demo/jwt.pem create user alice
```

In separate terminals:

```sh
target/debug/hydra-proxy --config /tmp/hydra-demo/proxy.json
```

```sh
target/debug/hydra-core --socket /tmp/hydra-demo/proxy.sock --database /tmp/hydra-demo/core.sqlite3 --key /tmp/hydra-demo/jwt.pem serve --snapshot /tmp/hydra-demo/seed.json --listen 127.0.0.1:8080 --origin https://localhost:8080 --tls-cert /tmp/hydra-demo/tls.crt --tls-key /tmp/hydra-demo/tls.key
```

Trust the development certificate and open `https://localhost:8080`. Swagger is `/docs`, generated OpenAPI is `/openapi.yaml`. Bootstrap snapshots initialize an empty Core; subsequent changes belong to the authenticated UI/API. `hydra-core apply` is an offline maintenance tool for use while Core is stopped; pass the same `--database` path so its intent/result are audited. Direct maintenance can diverge from Core desired revisions and needs deliberate reconciliation. Proxy restores its last accepted snapshot independently when Core is unavailable.

Accounts are created through CLI only. Every authenticated account has full equal privileges. Access tokens expire after 60 seconds; rotating refresh cookies retain the original absolute ten-hour session end. The console keeps access tokens in memory. Secure cookies require HTTPS. Omit the optional CLI password argument to use the hidden prompt; passwords supplied in argv may appear in process listings or shell history. Users can edit accounts, reset passwords, deactivate and reactivate through the console; account creation remains CLI-only.

## Operations configuration

Pass `serve --operations /private/operations.json`. This file must be private (0600), regular and <=64 KiB. Defaults disable pattern matching, automatic submission and exploit execution; they do not invent competition settings.

```json
{
  "flag_pattern": "FLAG\\{[A-Za-z0-9_]{1,64}\\}",
  "auto_submit_captured": false,
  "checker": {"kind":"tcp", "address":"127.0.0.1:19000", "secret":null},
  "sandbox": {
    "delegated_cgroup":"/sys/fs/cgroup/OPERATOR-DELEGATION",
    "runtime_dir":"/run/hydra/jobs",
    "bwrap":"/usr/bin/bwrap",
    "ip":"/usr/bin/ip"
  }
}
```

Configure the same optional `flag_pattern` in Proxy JSON for defensive findings. Proxy findings are always `own_exposed` and never eligible for submission. Captured values require an enabled explicit Farm target. Checker endpoints and credentials are operator configuration, never worker environment variables or public request parameters. Built-in adapters support bounded HTTP JSON, verified HTTPS with an operator-pinned CA, TCP JSON lines, and declarative CustomTcp atomic batches. Checker replies retain redacted status/body evidence. See [docs/flags-integration.md](docs/flags-integration.md) and [docs/farm-integration.md](docs/farm-integration.md).

Exploit execution requires bubblewrap, iproute2, user namespaces and a delegated writable cgroup v2 subtree. The adapter supports numeric IPv4 TCP/UDP targets, per-target and batch jobs, and one worker. Unsupported networking and missing isolation fail visibly; no unsandboxed fallback exists. See [docs/sandbox-integration.md](docs/sandbox-integration.md) and [docs/farm-execution.md](docs/farm-execution.md). New batch/UDP worker paths have safe validation tests, but were not executed after the instruction to avoid exploit-execution tests.

## Data and bounds

Only Core opens SQLite; WAL, private files, a bounded database size and traffic retention are enforced. Proxy has bounded connections, parser sessions, message buffers, telemetry queues, plugin versions and a finite durable critical spool. Loss counters are visible through status. No raw packet stream crosses gRPC. HTTP enforcement is limited to completely framed supported HTTP/1.1 requests before forwarding; unsupported framing and opaque encrypted traffic pass through, with diagnostic loss metadata for parser fallback. See [docs/protocol-integration.md](docs/protocol-integration.md).

For backups, stop Core and copy its entire private state directory, including SQLite sidecars, signing key, immutable protocol/exploit files; preserve ownership/modes. Copy Proxy state separately. Treat both backups as secrets. Do not copy a live SQLite database file alone.

The console includes bulk filter toggles, saved-field rule creation, protocol framing profiles/schema inspection, traffic field/rule/flag search and comparison, paginated flags and submissions, triage, run retry, and an audited settings/configuration export. Filter changes display desired and applied revisions separately. Configuration export covers services and rules; use the backup procedure for full recovery.

See [docs/PERFORMANCE.md](docs/PERFORMANCE.md) for the reproducible bounded loopback load measurement and its limits. Production systemd setup uses separate Core/Proxy users: [deploy/README.md](deploy/README.md).
