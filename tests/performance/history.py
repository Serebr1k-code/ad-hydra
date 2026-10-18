"""Linux-only, benign loopback benchmark of completed TCP history persistence.

Run the same command before/after optimization; --records seeds valid synthetic
history, never competition captures. CPU comes from the Proxy's /proc counters.
"""
import argparse
import gzip
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import uuid


def stats(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK'), int(fields[21]) * os.sysconf('SC_PAGE_SIZE')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/release/hydra-proxy'))
    parser.add_argument('--records', type=int, default=20000)
    parser.add_argument('--flows', type=int, default=20)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    assert 0 <= args.records <= 50000 and 1 <= args.flows <= 100
    cpus = sorted(os.sched_getaffinity(0))[:2]
    with tempfile.TemporaryDirectory(prefix='hydra-history-bench-') as temporary:
        root = Path(temporary)
        history = root / 'history'; history.mkdir(mode=0o700)
        payload = b'benign history benchmark\x00\xff' * 128
        compressed = gzip.compress(payload)
        now = int(time.time() * 1000)
        for index in range(args.records):
            flow_id = str(uuid.UUID(int=index + 1))
            stem = history / f'hydra-history-{flow_id}'
            stem.with_suffix('.gz').write_bytes(compressed); stem.with_suffix('.gz').chmod(0o600)
            metadata = dict(flow_id=flow_id, service_id='benchmark', protocol_id='raw',
                            opened_at_ms=now, closed_at_ms=now, bytes_client_to_server=len(payload),
                            bytes_server_to_client=0, client_len=len(payload), server_len=0,
                            truncated_client_to_server=False, truncated_server_to_client=False, history_truncated=False)
            stem.with_suffix('.json').write_text(json.dumps(metadata)); stem.with_suffix('.json').chmod(0o600)
        with socket.socket() as backend, socket.socket() as reserved:
            backend.bind(('127.0.0.1', 0)); backend.listen(); backend.settimeout(.1)
            reserved.bind(('127.0.0.1', 0)); listen = reserved.getsockname(); reserved.close()
            stop = threading.Event()

            def echo():
                while not stop.is_set():
                    try: peer, _ = backend.accept()
                    except socket.timeout: continue
                    with peer:
                        while data := peer.recv(16384): peer.sendall(data)

            worker = threading.Thread(target=echo, daemon=True); worker.start()
            config = dict(socket=str(root / 'proxy.sock'), state_file=str(root / 'state.json'),
                          snapshot=dict(revision=1, services=[dict(id='benchmark', transport='tcp',
                          listen_address=f'{listen[0]}:{listen[1]}', upstream_address=f'127.0.0.1:{backend.getsockname()[1]}',
                          protocol_id='raw', enabled=True, retain_history=True)]))
            (root / 'config.json').write_text(json.dumps(config))
            with (root / 'proxy.log').open('w+') as log:
                command = ['taskset', '-c', ','.join(map(str, cpus)), str(args.binary.resolve()), '--config', str(root / 'config.json')]
                proxy = subprocess.Popen(command, stdout=log, stderr=log)
                try:
                    deadline = time.monotonic() + 30
                    while True:
                        try:
                            with socket.create_connection(listen, timeout=1): pass
                            break
                        except OSError:
                            if proxy.poll() is not None or time.monotonic() > deadline:
                                log.seek(0); raise RuntimeError(log.read())
                            time.sleep(.05)
                    # Allow the zero-byte readiness flow to persist before measuring.
                    deadline = time.monotonic() + 30
                    while len(list(history.glob('*.json'))) < args.records + 1:
                        if time.monotonic() > deadline: raise RuntimeError('readiness history did not persist')
                        time.sleep(.05)
                    before, _ = stats(proxy.pid); started = time.monotonic()
                    for _ in range(args.flows):
                        with socket.create_connection(listen, timeout=3) as peer:
                            peer.sendall(payload); received = b''
                            while len(received) < len(payload):
                                chunk = peer.recv(len(payload) - len(received))
                                if not chunk: raise RuntimeError('unexpected EOF')
                                received += chunk
                            assert received == payload, 'relay changed bytes'
                        time.sleep(.05)
                    deadline = time.monotonic() + 60
                    while len(list(history.glob('*.json'))) < args.records + args.flows + 1:
                        if time.monotonic() > deadline: raise RuntimeError('history queue did not drain')
                        time.sleep(.05)
                    cpu, rss = stats(proxy.pid); elapsed = time.monotonic() - started
                    report = dict(binary=str(args.binary), seeded_records=args.records, completed_flows=args.flows,
                                  affinity=cpus, elapsed_seconds=elapsed, proxy_cpu_seconds=cpu-before,
                                  proxy_cpu_percent_one_core=(cpu-before)/elapsed*100,
                                  proxy_cpu_ms_per_flow=(cpu-before)*1000/args.flows, proxy_rss_bytes=rss,
                                  byte_integrity_verified=True, all_histories_persisted=True)
                    encoded = json.dumps(report, indent=2)
                    print(encoded)
                    if args.output: args.output.write_text(encoded + '\n')
                finally:
                    proxy.terminate(); proxy.wait(timeout=10)
                    stop.set(); worker.join(timeout=2)


if __name__ == '__main__':
    main()
