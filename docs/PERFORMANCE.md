# Local resource benchmark

`tests/performance/loopback.py` starts the two actual Hydra binaries and owned HTTP, TCP echo and UDP echo fixtures. Every fixture and client endpoint is explicitly `127.0.0.1`. There are no exploits, uploads, sandbox workers, external destinations or attack payloads.

## Reproduce

Build the current release binaries before recording release results:

```sh
cargo build --locked --release --workspace
systemd-run --user --scope --quiet -p MemoryMax=2G -p CPUQuota=200% \
  /usr/bin/env HYDRA_BUILD_PROFILE=release python tests/performance/loopback.py \
  --duration 30 --threads 16 --output /tmp/hydra-performance-release.json
```

The default binary profile is `release`; `HYDRA_BUILD_PROFILE=debug` explicitly selects a development build. The script does not compile binaries. It samples each application's `/proc` RSS and CPU once per second, collects an idle baseline, verifies every successful echo/HTTP response, records per-thread request/byte/error counters, and obtains before/after normal telemetry losses using the actual Core status CLI. It exits unsuccessfully on payload errors or combined load RSS at or above 500 MiB. The JSON also reports the 300 MiB idle target separately.

The duration is bounded to 15–60 seconds, with 3–32 load threads (defaults 15/16). Hydra processes are pinned to at most two available CPUs. The configured Proxy flow limit is 128. With 16 workers there are six TCP, five UDP and five HTTP clients; UDP sessions persist, while TCP and HTTP reconnect after 64 requests. TCP request payloads are 4096 bytes, UDP 1024 bytes, HTTP uses `GET /benchmark` with a two-byte `OK` body. Received-byte counts include successful application payloads and exclude protocol overhead. Fixture and load-generator allocations are bounded by worker count and bounded socket buffers. Children are terminated and waited on in `finally`, with kill fallback; fixtures and temporary storage are cleaned on failure.

The optional systemd scope limits the complete benchmark, including fixtures and generator, to 2 GiB RAM and two CPUs of quota. Per-process reported RSS/CPU covers Core and Proxy only. Running the Python script directly also works, but the report must then be interpreted using its actual recorded cgroup limits. A constrained scope is **not** evidence that the physical host has only 2 GiB RAM.

## Development-build measurement

Measured 2026-10-16 using `HYDRA_BUILD_PROFILE=debug`, 15 seconds, 16 workers, application affinity `[0, 1]`. The host reports 8 logical CPUs and 31.14 GiB physical RAM. The actual scope files read `memory.max = 2147483648` and `cpu.max = 200000 100000`.

| Metric | Result |
| --- | ---: |
| Combined idle RSS | 51.91 MiB |
| Peak combined load RSS | 60.64 MiB |
| First five RSS samples after two-second warmup | 60.07 MiB mean |
| Last five RSS samples | 60.61 MiB mean |
| Last-five minus first-five RSS | +0.54 MiB |
| Mean Core CPU under load | 66.49% of one CPU |
| Mean Proxy CPU under load | 85.23% of one CPU |
| Successful requests | 233231 |
| Received payload bytes | 545356004 |
| Payload/request errors | 0 |
| Normal telemetry events dropped | 13762 |
| Proxy rejected flows / dropped UDP datagrams | 0 / 0 |

Both RSS targets passed for this workload. Normal telemetry was lossy under load and its loss counter increased; this must not be described as lossless traffic indexing. Payload integrity passed despite telemetry loss. The small positive RSS difference is reported directly: a 15-second run cannot establish a long-term memory plateau or prove absence of all growth. This measures one benign mixed workload, not maximum throughput, malformed-input stress, farm execution, or production behavior.

An earlier exploratory run rotated UDP sockets every 64 packets, exhausted the configured 128 flow mappings and produced connection/time-out errors. It was not accepted as the representative resource result. The published workload uses persistent UDP sessions and reports zero request errors; overload behavior is a separate scenario.

| Second | Core RSS MiB | Proxy RSS MiB | Core CPU % | Proxy CPU % |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 36.40 | 20.53 | 78.33 | 87.49 |
| 2 | 37.53 | 20.66 | 74.00 | 84.00 |
| 3 | 38.46 | 20.69 | 62.00 | 92.00 |
| 4 | 39.14 | 20.73 | 68.00 | 86.00 |
| 5 | 39.14 | 21.28 | 66.00 | 83.00 |
| 6 | 39.17 | 21.29 | 69.00 | 83.00 |
| 7 | 39.17 | 21.29 | 59.00 | 87.00 |
| 8 | 39.19 | 21.36 | 63.00 | 86.00 |
| 9 | 39.21 | 21.31 | 59.00 | 88.00 |
| 10 | 39.23 | 21.34 | 67.00 | 81.00 |
| 11 | 39.25 | 21.32 | 73.00 | 81.00 |
| 12 | 39.26 | 21.32 | 67.00 | 84.00 |
| 13 | 39.27 | 21.35 | 64.00 | 86.00 |
| 14 | 39.27 | 21.38 | 67.00 | 86.00 |
| 15 | 39.27 | 21.38 | 61.00 | 84.00 |

The complete development-run JSON was written to `/tmp/hydra-performance-debug.json` during this session. The final current-release result is recorded below.

## Final release measurement

Measured 2026-10-16 on the same host, with the final optimized binaries, 30seconds/16workers, affinity `[0, 1]`, `memory.max=2147483648` and `cpu.max=200000 100000`. The scope also contains the owned fixtures and generator. A disposable CLI-created fixture account enables actual authenticated Core status reads; a stored parsed HTTP `/benchmark` message is checked after the workload. This verifies Core indexing was active. CLI status alone measures Proxy counters; its default Core-only counters are not interpreted as indexing measurements.

| Metric | Result |
| --- | ---: |
| Combined idle RSS | 24.66 MiB |
| Peak combined load RSS | 28.74 MiB |
| First five RSS samples after two-second warmup | 27.85 MiB mean |
| Last five RSS samples | 28.71 MiB mean |
| Last-five minus first-five RSS | +0.86 MiB |
| Mean Core CPU under load | 23.99% of one CPU |
| Mean Proxy CPU under load | 36.09% of one CPU |
| Successful requests | 221030 |
| Received payload bytes | 651283196 |
| Payload/request errors | 0 |
| Proxy telemetry events dropped | 0 |
| Core events received | 8469 |
| Core indexing events dropped | 0 |
| Proxy rejected flows / dropped UDP datagrams | 0 / 0 |

Successful requests by transport: TCP139649, UDP77415, parsed HTTP3966. Both RSS targets passed. No payload errors or measured telemetry/indexing losses occurred in this particular release workload; this does not override the development run's observed losses or make delivery lossless. RSS increased 0.86MiB between the compared windows. Thirty seconds is insufficient to establish long-term memory stability, maximum throughput or production behavior. These are constrained-host measurements, not results from a physically2GiB machine.

Full per-second samples, per-thread counters and before/after statuses: [release JSON](benchmarks/loopback-release-2026-10-16.json). No sandbox workers or external destinations were used.

## Vue-containing Core release

After the Vue migration, the rebuilt binaries passed the same owned-loopback workload for15seconds/16threads under verified2GiB/two-CPU quota and `[0,1]` affinity. This remains a constrained scope on the larger host described above.

| Metric | Result |
| --- | ---: |
| Combined idle RSS | 24.36MiB |
| Peak combined RSS | 28.06MiB |
| Compared RSS window growth | +0.38MiB |
| Mean Core CPU | 23.70% of one CPU |
| Mean Proxy CPU | 35.97% of one CPU |
| Verified requests / errors | 100664 / 0 |
| Core received events / indexing losses | 4020 / 0 |
| Proxy normal telemetry losses | 0 |

Parsed HTTP storage was verified. A separate real HTTPS Chromium check fetched the Vue assets and exercised login, refresh-on-reload, service creation, all pages and logout while `frontend/dist` was absent. This benchmark measures server traffic, not browser memory or maximum throughput. Fifteen seconds cannot establish long-term stability. [Raw Vue-release samples](benchmarks/vue-console-release-2026-10-16.json).

## Populated history directory regression — 2026-10-18

Reproduce the isolated regression with synthetic data only:

```sh
cargo build --locked --release -p hydra-proxy
python3 tests/performance/history.py --records 10000 --flows 20 \
  --output /tmp/hydra-history.json
```

The script creates valid private gzip/JSON histories, starts the actual release Proxy pinned to two CPUs, then verifies20 completed binary-safe loopback echoes and waits for all histories to persist. Each echo is3,328bytes; the generator pauses50ms between flows. Measurement excludes fixture seeding and startup index recovery. Process CPU/RSS exclude the generator, backend and Core; this fixture does not impose a2GiB cgroup limit. `/proc` CPU resolution is10ms on this host.

| Metric | Before | Indexed writer |
| --- | ---: | ---: |
| Proxy CPU seconds for20 streams |2.04|0.02|
| Proxy CPU milliseconds per stream |102|1|
| Measured elapsed seconds, including pacing/drain |2.12|1.06|
| Average Proxy CPU, percent of one core |96.16|1.88|
| Proxy RSS |9,515,008bytes|10,108,928bytes|
| Relay byte integrity / all histories persisted |yes / yes|yes / yes|

This is approximately99% less CPU for this history-heavy workload, not a claim about maximum network throughput or parity with nginx. Parsing/WASM/filtering/gzip remain enabled as configured. The bounded in-memory index trades modest RAM for avoiding repeated filesystem scans; it caps retained files at65,536 records in addition to the existing24-hour TTL and100MiB compressed-payload quota. Startup still scans existing records once. [Raw before/after results](benchmarks/history-index-release-2026-10-18.json).

## CPU-aware shared pools

Proxy creates one Tokio async worker per CPU reported by Rust `available_parallelism` at startup (respecting the process's available CPU budget), instead of hardcoding two workers. Concurrent sessions are async tasks on the work-stealing pool. Disk operations share a separate blocking pool capped at `min(available CPUs, 4)`; the critical spool has its existing single worker. There is no OS thread per connection or per submitted job. The startup log records the actual `async_workers` and `blocking_workers_limit`. CPU allocation changes take effect after a Proxy restart.

The history catalog remains single-owner to preserve quota/expiry accounting, and each connection's decoder remains ordered. These guarantees do not cap independent connections to one core. CPU use grows with real work; idle cores are not kept busy artificially. On hosts larger than the target two-vCPU machine, systemd `TasksMax` must allow at least async workers + blocking workers + two housekeeping threads (the supplied limit is64).

```sh
HYDRA_BUILD_PROFILE=release python3 tests/e2e/proxy_runtime.py
```

This Linux-only benign loopback check runs Proxy under one-CPU and up-to-three-CPU affinity, opens128 simultaneous TCP sessions, verifies8,192 binary exchanges per case and checks that total OS threads fit the shared-pool budget. It is also part of CI. A debug run observed one async worker/four total OS threads with one CPU, and three async workers/seven total OS threads with three CPUs. This is a concurrency/resource regression check, not a maximum-throughput benchmark.
