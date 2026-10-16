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
