#!/usr/bin/env python3
"""Opt-in real kernel tests inside a dedicated transient systemd user scope.

This fixture provisions controllers only inside the scope that systemd-run created
for this command. It never changes global cgroup settings or an existing service.
"""
import argparse
import ipaddress
import os
from pathlib import Path
import socket
import subprocess
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bind-ip", required=True, help="explicit local non-loopback IPv4 address for the owned TCP echo fixture")
    parser.add_argument("--helper", type=Path, help="built Core executable; defaults to target/debug/hydra-core")
    args = parser.parse_args()
    address = ipaddress.IPv4Address(args.bind_ip)
    if address.is_loopback or address.is_unspecified or address.is_multicast:
        parser.error("--bind-ip must be a local non-loopback unicast IPv4 address")
    repository = Path(__file__).resolve().parents[2]
    helper = (args.helper or repository / "target/debug/hydra-core").resolve(strict=True)
    membership = Path("/proc/self/cgroup").read_text().strip().split(":", 2)
    if membership[:2] != ["0", ""]:
        raise SystemExit("cgroup v2 is required")
    scope = Path("/sys/fs/cgroup") / membership[2].lstrip("/")
    if not (scope.name.startswith("run-") and scope.name.endswith(".scope")):
        raise SystemExit("run this fixture directly via systemd-run --user --scope -p Delegate=yes")
    processes = {int(pid) for pid in (scope / "cgroup.procs").read_text().split()}
    if processes != {os.getpid()}:
        raise SystemExit("refusing to provision a scope containing other operator processes")
    available = set((scope / "cgroup.controllers").read_text().split())
    if not {"cpu", "memory", "pids"}.issubset(available):
        raise SystemExit("scope does not delegate cpu, memory and pids")
    # This is operator test provisioning, deliberately outside application code.
    supervisor = scope / "test-supervisor"
    supervisor.mkdir()
    (supervisor / "cgroup.procs").write_text(str(os.getpid()))
    (scope / "cgroup.subtree_control").write_text("+cpu +memory +pids")

    listener = socket.socket()
    listener.bind((str(address), 0))
    listener.listen(16)
    stop = threading.Event()
    listener.settimeout(0.25)

    def echo():
        while not stop.is_set():
            try:
                connection, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with connection:
                connection.settimeout(3)
                try:
                    data = connection.recv(65536)
                    connection.sendall(data)
                except OSError:
                    pass

    thread = threading.Thread(target=echo, daemon=True)
    thread.start()
    target = f"{address}:{listener.getsockname()[1]}"
    environment = os.environ.copy()
    environment.update(HYDRA_SANDBOX_CGROUP=str(scope), HYDRA_SANDBOX_HELPER=str(helper), HYDRA_SANDBOX_TARGET=target)
    print(f"Delegated test scope: {scope}\nExplicit owned TCP echo target: {target}", flush=True)
    try:
        result = subprocess.run(["cargo", "test", "-p", "hydra-core", "actual_linux_network_limits_and_cancellation", "--lib", "--", "--ignored", "--nocapture"], cwd=repository, env=environment, check=False)
        if result.returncode == 0:
            result = subprocess.run(["python3", "tests/e2e/farm_execution.py"], cwd=repository, env=environment, check=False)
    finally:
        stop.set()
        listener.close()
        thread.join(timeout=4)
    raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
