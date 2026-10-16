# Reproducible verification

Use Rust1.99.0 and protoc; see README. Run from repository root:

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace
scripts/check-contracts.sh
python3 -m pip install -r scripts/requirements.txt
python3 scripts/generate-frontend.py --check
find frontend/src -name '*.js' -exec node --check {} \;
python3 tests/e2e/relay.py
python3 tests/e2e/browser.py
cargo build --release --locked --workspace
```

Ordinary Rust checks use local temporary files and local echo sockets. The Linux exploit-execution kernel test is ignored by default. No tests contact competition services, deploy infrastructure or require a Docker daemon. The explicit diagnostics/HTTP/operations fixtures use owned local toy listeners/checkers; operations records an execution failure unless its sandbox is configured.

Resource smoke command, if systemd user scopes are available:

```sh
systemd-run --user --scope --quiet -p MemoryMax=2G -p CPUQuota=200% /usr/bin/python3 tests/e2e/relay.py
```

Both binaries set two Tokio runtime workers. The relay test additionally sets two CPU affinity slots. Results and limitations belong in PROGRESS.md. Do not infer sustained performance from this short check.

`tests/sandbox/run-linux.py` and `tests/e2e/farm_execution.py` are explicit opt-in execution fixtures. They are excluded from ordinary CI. The former performs provisioning only within its own transient delegated test scope. They are not run when the operator requests no exploit-execution testing.
