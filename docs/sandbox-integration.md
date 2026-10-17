# Linux farm sandbox adapter

The adapter in `apps/hydra-core/src/infrastructure/sandbox/` runs **no untrusted child** without a successfully configured cgroup v2 leaf and bubblewrap isolation. Missing prerequisites are execution failures, never permission to run an upload on the host. There is no Docker dependency and no permanent worker daemon.

## Operator prerequisites

* Linux with unprivileged nested user, PID, mount and network namespaces enabled; bubblewrap and iproute2 executable by Core.
* A merged `/usr` runtime containing `/usr/bin/{python3,sh}`, ELF loader and runtime libraries under `/usr/lib`. Other layouts require an explicit adapter change; do not fall back to host execution. `/bin` maps to `/usr/bin`, `/lib` and `/lib64` map to `/usr/lib`.
* A **dedicated operator-delegated cgroup v2 subtree**, accessible at an absolute path below `/sys/fs/cgroup`. Its parent must have enabled `cpu memory pids` controllers; Core never enables controllers, changes global cgroup settings, or migrates operator processes. Each run creates one random `hydra-UUID` child and requires `cpu.max`, `memory.max`, `memory.swap.max`, `memory.oom.group`, `pids.max`, `cgroup.procs` and `cgroup.kill` to be writable. A root-only or read-only cgroup mount does not satisfy this prerequisite.
* A private runtime directory owned by Core (absolute pathname at most 62 bytes for Unix socket limits) and immutable private upload snapshots. The snapshot must contain only exploit files, no symlinks, special files, keys, DB or checker configuration. Validation bounds it to 32 MiB, 1024 entries and eight nested directories. Upload storage must prevent concurrent replacement and mutation throughout a run; this is a caller ownership contract.
* An explicit list of numeric unicast IPv4 TCP destination endpoints. Loopback, multicast, unspecified, broadcast, IPv6 and UDP targets are rejected. This adapter supports TCP only. DNS resolution and arbitrary destination selection are absent from the worker interface.

An operator may provision a delegated systemd subtree through its service manager. Do not point Hydra at the cgroup root or guess a writable global location. Delegation provisioning is intentionally outside the application's authority.

## Execution and network boundary

`Sandbox::new(Config)` validates configuration, and `Sandbox::run(Job, CancellationToken)` acquires the single-worker semaphore. It validates the exact target against the allowlist, configures CPU quota, memory, zero swap, PID and group-OOM controls, and proves `cgroup.kill` is writable. A trusted fresh Core helper clears its environment, joins that leaf, and **execs bubblewrap before starting an upload**. It does not initialize the database, JWT key or checker configuration.

The outer namespace has no host networking or host filesystem mounts beyond read-only `/usr`, the helper/runtime executables, the upload, and a private job broker directory. Its trusted bridge runs as UID/GID zero only inside the new user namespace, and has `CAP_NET_ADMIN` to bring up loopback and add exactly the target's IPv4 address as a local alias, plus `CAP_NET_BIND_SERVICE` to support low target ports and `CAP_SYS_ADMIN` solely to construct the inner mount/PID sandbox. The inner worker receives none of these capabilities. The bridge binds exactly the configured address and TCP port before starting the exploit. It forwards that listener to one pathname Unix socket. Core's parent-side Unix gateway connects only to the **already validated fixed numeric destination**; there is no destination protocol supplied by an exploit.

The exploit runs in a second bubblewrap mount/PID/IPC/UTS namespace sharing the isolated outer user and network namespaces. All worker capabilities and their bounding set are dropped. A worker-created descendant user namespace cannot grant capabilities over the outer network namespace. Keeping the outer user namespace avoids nested UID-map setup failures on the tested kernel. `/broker` and the helper binaries are excluded. It receives read-only `/usr` and `/work`, private tmpfs `/tmp`, private proc/dev and only `PATH`, `LANG`, `TARGET_HOST`, `TARGET_PORT`. Python uses isolated mode, shell uses `/usr/bin/sh`, and ELF files execute directly. The worker has no capabilities to administer the outer network namespace. There is no default route, host interface, host resolver or host Unix socket. Connections outside the one alias/listener have no host egress path.

Both bridge and gateway limit active connections to 16 and use bounded streaming buffers. Connection attempts time out after three seconds; relay lifetime is capped at one hour. Untrusted stdout and stderr are drained continuously, retaining a contiguous 32 KiB tail each, with discarded-byte counters. Flag extraction can scan these bounded tails across chunk boundaries; flags in discarded output cannot be recovered. No stream is presented as a complete log when its loss counter is nonzero.

CPU quota accepts 10–2000 milliseconds per second; memory accepts 16 MiB–2 GiB; PID cap accepts 8–256; wall timeout accepts up to one hour. Defaults and smaller operational policy limits belong to the farm adapter. Concurrency is one. Deadline, cancellation, normal exit and owner-drop kill the whole cgroup, including forked descendants. Gateway tasks are cancelled as well. Group directory removal is best effort after kill; a kernel cleanup race can leave an empty `hydra-UUID` directory for operator cleanup. There is no untrusted execution fallback if cleanup or resource-control setup fails.

## Core hidden CLI integration

Dispatch these before initializing Core state:

* `sandbox-child <cgroup-path> <bwrap-path> <bwrap-arguments...>` calls `sandbox::worker_child(&Path, &Path, &[OsString])`. Successful `exec` does not return.
* `sandbox-bridge <json>` deserializes `sandbox::BridgeConfig` and calls `sandbox::bridge(config).await`; exit using the returned `i32` code. Errors must terminate the helper unsuccessfully.

`BridgeConfig` and command arguments contain no submission credentials. Helper modes are internal, transient subprocess entrypoints and do not expose a network API. The runtime only needs existing serde/serde_json, uuid, Tokio process/net/io/sync/time, and tokio-util CancellationToken dependencies.

## Verification

Normal unit tests are discovered by Cargo:

```sh
cargo test -p hydra-core infrastructure::sandbox --lib
```

The actual Linux integration test requires a real non-loopback TCP echo endpoint in the authorized competition/test allowlist, a built Core helper with the hidden modes wired, and an operator-provisioned writable cgroup delegation:

```sh
cargo build -p hydra-core
HYDRA_SANDBOX_CGROUP=/sys/fs/cgroup/OPERATOR-DELEGATION \
HYDRA_SANDBOX_HELPER="$PWD/target/debug/hydra-core" \
HYDRA_SANDBOX_TARGET=192.0.2.10:19090 \
cargo test -p hydra-core actual_linux_network_limits_and_cancellation --lib -- --ignored --nocapture
```

Replace the documentation target with a reachable authorized endpoint that echoes the 14 bytes `hydra-isolated`. The test executes a real Python upload, verifies target echo, denies another destination, verifies missing host paths/environment, executes shell and ELF uploads, then exercises a forked-child timeout, cancellation, PID exhaustion, measured CPU quota and memory exhaustion. No mock sandbox reports success.

A reproducible operator test fixture can provision **only its own transient user scope**, run an owned local TCP echo server, and execute that same integration test:

```sh
cargo build -p hydra-core
systemd-run --user --scope --quiet -p Delegate=yes \
  /usr/bin/python3 tests/sandbox/run-linux.py --bind-ip YOUR_LOCAL_NON_LOOPBACK_IPV4
```

The helper refuses an existing service scope or a scope containing other processes. Choose an IP assigned to the local test machine. The fixture's local owned echo endpoint is the only allowlisted destination. The application's sandbox code itself still never enables controllers; this fixture performs explicit operator test provisioning.

Verification on the implementation host: the tool sandbox's cgroup mount appeared read-only, but real host execution in a delegated transient user scope exposed writable `cpu memory pids` controllers. The real test passed with the built Core binary (Python, shell, ELF; allowlisted echo and network deny; missing host filesystem/environment; whole-tree timeout/cancel; PID exhaustion; CPU quota; memory exhaustion). A 500 ms/s CPU quota yielded **0.619008 s worker CPU over 1.213302 s wall time**; complete kernel test elapsed **2.92 s**. This is a sandbox control test, not the SPEC two-process RAM benchmark. No baseline RSS claim is made.

The adapter also implements multi-target batch invocation and bounded UDP datagram brokering; see [farm execution](farm-execution.md) for the target JSON, flag attribution, limits and retry contract. The opt-in kernel tests above exercise the earlier TCP path only. After the user restricted exploit-execution testing, no further kernel sandbox, uploaded exploit or outbound target tests were run; UDP framing was checked only in memory. Treat UDP kernel behavior as unverified until a separately authorized integration run.

## Python packages and uv

Python versions run through `uv run --offline --no-project --no-sync` using a prepared,
read-only virtual environment. Install the operator-trusted `uv` executable and the
system Python runtime; `sandbox.uv` is an optional absolute binary pathname (default
`/usr/bin/uv`). A missing binary fails Python execution; there is no fallback to host
Python. The full path may live outside `/usr`: the adapter binds only that executable.

The exploit manifest accepts `python_packages`, an optional list of up to 32 exact
registry pins, for example `["requests==2.32.5"]`. The Add exploit form exposes this
list. Names, numeric versions and pin syntax are validated; URLs, paths, requirements
files, options, markers, extras and duplicate normalized package names are rejected.
Only Python manifests may contain packages. Runtime and package pins become immutable
when the uploaded version is first associated with a saved exploit; use a new version
to change either. Existing manifests default to an empty list.

Before the first Python execution, a separate trusted preparation phase creates a
virtual environment with `uv venv`, then installs packages and transitive dependencies
from the fixed `https://pypi.org/simple` index. Installation uses `--only-binary :all:`
(the uv equivalent of `--no-build`), a fresh empty cache, no inherited environment and no discovered
configuration. Missing compatible wheels fail preparation. Uploaded code is never
mounted or executed during preparation. Only this phase shares host networking and
receives read-only system DNS and CA certificates; it receives no Core data or secrets.
Runtime exploit networking continues through the existing target broker only. Standard
PyPI dependency metadata participates in resolution; packages are operator-selected,
and the preparation phase does not provide a network destination firewall for uv.
See the [uv command reference](https://docs.astral.sh/uv/reference/cli/).

Preparation runs in the same configured cgroup resource limits as its job, with a
separate 120-second deadline and cancellation. Downloads/installs are serialized,
diagnostics retain at most 32 KiB, and the environment is checked every 100 ms against
a 256 MiB quota and 100,000-file traversal limit. Temporary download/cache data live
on namespace tmpfs and count against the cgroup memory limit. The disk quota is
monitored, so a write burst can briefly exceed it; use an operator filesystem quota
for a hard disk bound. New environment preparation is refused once the private runtime
tree exceeds 2 GiB. Preparation uses the farm's single worker slot; it may require
raising the exploit memory setting for large dependencies.

A successful environment is atomically renamed to `python-<digest>` in the private
runtime directory and reused for that exploit version and package list. It is never
updated by workers. Use a **persistent** private runtime path at most 62 bytes long
(e.g. `/var/lib/hydra-core/jobs`) if environments must survive host restarts. Back it up
alongside uploads. Deleting an environment causes fresh transitive resolution on the
next run; do not delete it when preserving the exact resolved environment matters.
There is no automatic garbage collector; stop Core before removing unused environments.

Each prepared environment also contains `.hydra-resolved.txt`, generated by
bounded inspection of installed distribution metadata without executing Python. It records the installed
transitive versions (bounded to 32 KiB); retain it and the environment in backups.
A benign local wheel installation/import was checked with uv 0.10.9 and system
Python 3.14.8. This does not claim kernel sandbox or live PyPI verification.
