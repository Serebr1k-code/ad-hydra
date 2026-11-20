# Python and WASM request checkers

A request checker runs before Proxy forwards client traffic to a local service. It receives the original bytes and returns a Boolean decision. A checker runs in its own Bubblewrap sandbox with persistent storage at **`/state`**. Use this for defensive checks on services you operate.

## Register a Python checker

The Hydra installer installs `bubblewrap`, the system `python3`, and the `libseccomp.so.2` runtime. Bubblewrap must support unprivileged user namespaces. On an existing installation, install those packages with the host distribution's package manager before enabling Python checkers.

Managed Python checkers can be created from **Filters → Request checkers** by pasting source or uploading a `.py` file (up to 256 KiB). The UI stores managed source in Proxy's private checker program store; it does not accept filesystem paths. New programs start paused. Select one or more services and save checker settings to enable it. You can edit or delete managed source from the UI. Deleting a program preserves its `/state` data.

Legacy checkers can still be configured by an administrator in Proxy JSON with an absolute `script` path. Those entries appear read-only in the UI; service assignments, timeout, and error fallback remain editable. Define `check(request: bytes) -> bool`. Returning `True` allows traffic to continue through the existing filters. Returning `False` closes the TCP connection or drops the UDP datagram.

This example reads an optional file containing a maximum request size:

```python
from pathlib import Path

LIMIT = Path("/state/byte-limit")

def check(request: bytes) -> bool:
    if not LIMIT.exists():
        LIMIT.write_text("65536\n", encoding="utf-8")
    return len(request) <= int(LIMIT.read_text(encoding="utf-8"))
```

For legacy path-based checkers, add the following property to the existing Proxy JSON (typically `/etc/hydra/proxy.json`). Replace the service identifier with the ID shown by the Services API. One checker can cover several explicit service IDs. Each service can have one checker.

```json
"request_checkers": [
  {
    "id": "web",
    "runtime": "python",
    "services": ["YOUR-SERVICE-ID"],
    "script": "/etc/hydra/checkers/web.py",
    "timeout_ms": 250,
    "on_error": "block"
  }
]
```

Managed source changes are applied by the Proxy program registry without choosing a host path. `on_error` can be `allow` or `block`; it covers timeout, exceptions, invalid decisions, a worker that cannot start, and concurrent requests when the worker is busy. It also covers HTTP framing failures. Checker errors appear next to Proxy status and in the Proxy journal. The status shows the error count and most recent error for the current Proxy process. Requests and script output are excluded from checker error logs.

## Register a WASM checker

Install the [Wasmtime CLI](https://docs.wasmtime.dev/cli-install.html) on the Proxy host. Proxy checks `/usr/bin/wasmtime` and `/usr/local/bin/wasmtime`. Set the Proxy service environment variable `HYDRA_WASMTIME` to use another absolute path. Bubblewrap and `prlimit` are also required.

Compile the [example checker](../plugins/examples/request-checker/main.rs) for WASI Preview 1:

```bash
rustc --edition 2024 --target wasm32-wasip1 -O -C strip=symbols plugins/examples/request-checker/main.rs -o checker.wasm
```

In the same Proxy configuration, set `"runtime": "wasm"` and give `script` the absolute path to the compiled module. Programs use the same four-byte big-endian length followed by raw request bytes on stdin for each check. On stdout write exactly one byte, `0x01` for allow or `0x00` for block, then flush. Keep diagnostics on stderr. Wasmtime exposes `/state` as a WASI preopen and runs inside the same file and network isolation. Each instance has a 64 MiB linear memory cap and a finite fuel budget. Exhausting fuel counts as a checker error and causes Proxy to replace the instance.

## Change checker settings from the console

Open **Filters → Request checkers**. Select the services for each registered checker, enter a timeout up to 1,000 ms, and choose **Allow traffic** or **Block traffic** for errors. Save to apply immediately. Uncheck every service to pause the checker and preserve its files. Proxy saves edits in `request-checkers/settings.json` next to the checker directories, and reloads them after restart. The saved policy takes precedence over the initial service list, timeout, and fallback in Proxy JSON.

## Where files are stored

| View | Directory |
| --- | --- |
| Inside the worker and its working directory | `/state` |
| Host with the standard installer | `/var/lib/hydra-proxy/request-checkers/<id>/` |
| Host with a custom Proxy state file | `<parent directory of state_file>/request-checkers/<id>/` |

Proxy creates the directories with mode `0700`. Keep the checker ID to preserve its files across script changes and restarts. Back up the checker directory with Proxy's private state. Stop Proxy before restoring or modifying files that a running checker may write.

Python uses ordinary file APIs and WASM uses WASI file APIs. It can write only to its state mount and has no network sockets, host management socket, or ability to start other processes. Individual files are limited to 4 MiB. Use a host filesystem quota if you need an aggregate disk bound. Python has 64 MiB of virtual address space. Each checker has at most one request in flight. Proxy allows up to four configured checkers.

## What is passed to the checker

- HTTP uses complete supported plaintext HTTP requests, including headers and body as received. If the HTTP decoder cannot frame the request, the configured error action applies. Use the native HTTP decoder for an HTTP service.
- UDP uses one whole client datagram.
- Other TCP services use one client read at a time. A read is not a protocol message boundary. Use this mode only for checks that work with arbitrary chunk boundaries.
- Checkers receive at most 1 MiB per input. The error action applies to a larger input.

The UI can create, edit, and delete bounded managed Python source, and assign managed or legacy programs to services. Console users cannot select arbitrary paths or mount directories. Managed source and the registry are retained under Proxy's private state directory across restarts; per-checker mutable files remain under `/state` and survive program deletion.
