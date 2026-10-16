#!/usr/bin/env python3
"""Bounded, benign loopback-only benchmark; starts no uploaded code or farm workers."""
import argparse
import concurrent.futures
import http.client
import http.server
import json
import os
from pathlib import Path
import socket
import socketserver
import statistics
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
HOST = '127.0.0.1'


class TCP(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        try:
            while data := self.request.recv(16384):
                self.request.sendall(data)
        except (OSError, TimeoutError):
            pass


class UDP(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        sock.sendto(data, self.client_address)


class HTTP(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Length', '2')
        self.end_headers()
        self.wfile.write(b'OK')

    def log_message(self, *_):
        pass


class BoundedThreads:
    def __init__(self, *args, **kwargs):
        self.handler_slots = threading.BoundedSemaphore(32)
        super().__init__(*args, **kwargs)

    def process_request(self, request, address):
        if not self.handler_slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, address)
        except Exception:
            self.handler_slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.handler_slots.release()


class TCPServer(BoundedThreads, socketserver.ThreadingTCPServer):
    daemon_threads = True
    block_on_close = False
    request_queue_size = 64


class HTTPServer(BoundedThreads, http.server.ThreadingHTTPServer):
    daemon_threads = True
    block_on_close = False
    request_queue_size = 64


def port(kind=socket.SOCK_STREAM):
    with socket.socket(type=kind) as sock:
        sock.bind((HOST, 0))
        return sock.getsockname()[1]


def proc(process):
    if process.poll() is not None:
        raise RuntimeError(f'{Path(process.args[0]).name} exited {process.returncode}')
    status = Path(f'/proc/{process.pid}/status').read_text().splitlines()
    rss = int(next(row.split()[1] for row in status if row.startswith('VmRSS:'))) * 1024
    fields = Path(f'/proc/{process.pid}/stat').read_text().rsplit(')', 1)[1].split()
    ticks = int(fields[11]) + int(fields[12])
    return rss, ticks


def cgroup_limits():
    try:
        group = next(line.split(':', 2)[2] for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::'))
        base = Path('/sys/fs/cgroup') / group.lstrip('/')
        return {name: (base / name).read_text().strip() for name in ['memory.max', 'cpu.max']}
    except (OSError, StopIteration):
        return {'unavailable': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--duration', type=int, default=15)
    parser.add_argument('--threads', type=int, default=16)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if not 15 <= args.duration <= 60 or not 3 <= args.threads <= 32:
        parser.error('duration must be 15..60 seconds; threads must be 3..32')
    profile = os.environ.get('HYDRA_BUILD_PROFILE', 'release')
    binaries = ROOT / 'target' / profile
    for name in ['hydra-core', 'hydra-proxy']:
        if not (binaries / name).is_file():
            parser.error(f'build {binaries / name} first')
    cpus = sorted(os.sched_getaffinity(0))[:2]
    stop = threading.Event()
    processes, servers, server_threads = [], [], []
    report = {'profile': profile, 'duration_seconds': args.duration, 'threads': args.threads,
              'affinity': cpus, 'cgroup_limits': cgroup_limits(), 'host_cpu_count': os.cpu_count(),
              'host_mem_total_kib': next(row.split()[1] for row in Path('/proc/meminfo').read_text().splitlines() if row.startswith('MemTotal:')),
              'payload_bytes': {'tcp': 4096, 'udp': 1024, 'http': 2}, 'samples': []}
    with tempfile.TemporaryDirectory(prefix='hydra-perf-') as name:
        directory = Path(name)
        with (directory / 'process.log').open('w+') as log:
            try:
                for cls, handler in [(TCPServer, TCP), (socketserver.UDPServer, UDP), (HTTPServer, HTTP)]:
                    server = cls((HOST, 0), handler)
                    servers.append(server)
                    thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': .1}, daemon=True)
                    thread.start()
                    server_threads.append(thread)
                ports = [port(), port(socket.SOCK_DGRAM), port()]
                snapshot = {'revision': 1, 'services': [
                    {'id': kind, 'transport': 'udp' if kind == 'udp' else 'tcp',
                     'listen_address': f'{HOST}:{listen}', 'upstream_address': f'{HOST}:{server.server_address[1]}',
                     'enabled': True, 'protocol_id': 'http' if kind == 'http' else 'raw'}
                    for kind, listen, server in zip(['tcp', 'udp', 'http'], ports, servers)]}
                sock = str(directory / 'proxy.sock')
                api_port = port()
                origin = 'https://hydra-benchmark.test'
                (directory / 'proxy.json').write_text(json.dumps({'socket': sock, 'state_file': str(directory / 'state.json'),
                    'snapshot': snapshot, 'limits': {'max_flows': 128, 'udp_idle_ms': 1000}}))
                (directory / 'snapshot.json').write_text(json.dumps(snapshot))
                core_args = [str(binaries / 'hydra-core'), '--socket', sock,
                             '--database', str(directory / 'core.db'), '--key', str(directory / 'key.pem')]
                # This disposable account belongs only to this private loopback fixture.
                credentials = {'username': 'benchmark', 'password': 'loopback-only-test-password'}
                subprocess.run(core_args + ['create', 'user', credentials['username'], credentials['password']],
                               check=True, stdout=log, stderr=log, timeout=10)

                def core_api(method, path, value=None, token=None):
                    headers = {'Origin': origin, 'Content-Type': 'application/json'}
                    if token:
                        headers['Authorization'] = 'Bearer ' + token
                    connection = http.client.HTTPConnection(HOST, api_port, timeout=5)
                    try:
                        connection.request(method, '/api/v1' + path,
                                           json.dumps(value) if value is not None else None, headers)
                        response = connection.getresponse()
                        data = response.read(1024 * 1024)
                        if response.status != 200:
                            raise RuntimeError(f'Core fixture API {path} returned {response.status}')
                        return json.loads(data)
                    finally:
                        connection.close()

                def core_status():
                    # Fresh login keeps the maximum 60-second workload independent of JWT expiry.
                    token = core_api('POST', '/auth/login', credentials)['access_token']
                    return core_api('GET', '/system/proxy', token=token), token

                def start(command):
                    child = subprocess.Popen(command, stdout=log, stderr=log)
                    processes.append(child)
                    os.sched_setaffinity(child.pid, cpus)
                    return child

                def status():
                    result = subprocess.run([str(binaries / 'hydra-core'), '--socket', sock, 'status'],
                                            capture_output=True, text=True, check=True, timeout=5)
                    return json.loads(result.stdout)

                proxy = start([str(binaries / 'hydra-proxy'), '--config', str(directory / 'proxy.json')])
                for _ in range(100):
                    if proxy.poll() is not None:
                        raise RuntimeError('proxy startup failed')
                    try:
                        status()
                        break
                    except (OSError, subprocess.SubprocessError):
                        time.sleep(.05)
                else:
                    raise RuntimeError('proxy startup timed out')
                core = start(core_args + ['serve', '--listen', f'{HOST}:{api_port}', '--origin', origin,
                              '--snapshot', str(directory / 'snapshot.json')])
                time.sleep(2)
                report['core_status_before'], _ = core_status()
                idle_start = {key: proc(p) for key, p in [('core', core), ('proxy', proxy)]}
                idle_at = time.monotonic()
                time.sleep(1)
                idle_end = {key: proc(p) for key, p in [('core', core), ('proxy', proxy)]}
                report['idle'] = {key: {'rss_bytes': idle_end[key][0], 'cpu_percent_one_core':
                    (idle_end[key][1] - idle_start[key][1]) / os.sysconf('SC_CLK_TCK') / (time.monotonic() - idle_at) * 100}
                    for key in idle_end}
                report['status_before'] = status()
                counters = [{'kind': ['tcp', 'udp', 'http'][i % 3], 'requests': 0, 'bytes_received': 0, 'errors': 0} for i in range(args.threads)]
                deadline = time.monotonic() + args.duration

                def load(counter):
                    kind = counter['kind']
                    address = (HOST, ports[['tcp', 'udp', 'http'].index(kind)])
                    while not stop.is_set() and time.monotonic() < deadline:
                        try:
                            with socket.socket(type=socket.SOCK_DGRAM if kind == 'udp' else socket.SOCK_STREAM) as client:
                                client.settimeout(1)
                                client.connect(address)
                                if kind != 'udp':
                                    client.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                                for _ in range(1_000_000 if kind == 'udp' else 64):
                                    if stop.is_set() or time.monotonic() >= deadline:
                                        break
                                    if kind == 'http':
                                        client.sendall(b'GET /benchmark HTTP/1.1\r\nHost: localhost\r\n\r\n')
                                        response = bytearray()
                                        while b'\r\n\r\n' not in response:
                                            chunk = client.recv(1024)
                                            if not chunk:
                                                raise OSError('HTTP EOF')
                                            response += chunk
                                            if len(response) > 8192:
                                                raise OSError('invalid HTTP response')
                                        headers, body = response.split(b'\r\n\r\n', 1)
                                        while len(body) < 2:
                                            data = client.recv(2-len(body))
                                            if not data:
                                                raise OSError('HTTP EOF')
                                            body += data
                                        if not headers.startswith(b'HTTP/1.1 200') or body != b'OK':
                                            raise OSError('HTTP integrity mismatch')
                                        length = len(body)
                                    else:
                                        payload = b'loopback-benchmark ' + b'x' * ((1024 if kind == 'udp' else 4096)-19)
                                        client.sendall(payload)
                                        response = bytearray()
                                        while len(response) < len(payload):
                                            data = client.recv(len(payload)-len(response))
                                            if not data:
                                                raise OSError('echo EOF')
                                            response += data
                                        if response != payload:
                                            raise OSError('echo integrity mismatch')
                                        length = len(response)
                                    counter['requests'] += 1
                                    counter['bytes_received'] += length
                        except OSError:
                            counter['errors'] += 1

                with concurrent.futures.ThreadPoolExecutor(max_workers=args.threads) as pool:
                    workers = [pool.submit(load, counter) for counter in counters]
                    previous = idle_end
                    previous_at = time.monotonic()
                    for second in range(args.duration):
                        time.sleep(max(0, deadline - args.duration + second + 1 - time.monotonic()))
                        now = time.monotonic()
                        sample = {'second': second+1}
                        for key, child in [('core', core), ('proxy', proxy)]:
                            rss, ticks = proc(child)
                            sample[key] = {'rss_bytes': rss, 'cpu_percent_one_core':
                                (ticks-previous[key][1]) / os.sysconf('SC_CLK_TCK') / (now-previous_at) * 100}
                            previous[key] = (rss, ticks)
                        previous_at = now
                        report['samples'].append(sample)
                    stop.set()
                    for worker in workers:
                        worker.result(timeout=3)
                report['load'] = counters
                time.sleep(.5)
                report['status_after'] = status()
                report['core_status_after'], token = core_status()
                flows = core_api('GET', '/traffic/flows?service_id=http&limit=1', token=token)['items']
                if not flows:
                    raise RuntimeError('Core indexed no HTTP flows during workload')
                messages = core_api('GET', '/traffic/flows/' + flows[0]['flow_id'] + '/messages', token=token)['items']
                if not any(m.get('fields', {}).get('http.path', {}).get('value') == '/benchmark' for m in messages):
                    raise RuntimeError('Core indexed no expected parsed HTTP message')
                rss = [sum(sample[key]['rss_bytes'] for key in ['core', 'proxy']) for sample in report['samples']]
                first = statistics.mean(rss[2:7])
                last = statistics.mean(rss[-5:])
                report['summary'] = {'idle_combined_rss_bytes': sum(v['rss_bytes'] for v in report['idle'].values()),
                    'peak_combined_rss_bytes': max(rss), 'after_warmup_first5_mean_rss_bytes': first,
                    'last5_mean_rss_bytes': last, 'last5_minus_first5_bytes': last-first,
                    'requests': sum(c['requests'] for c in counters), 'errors': sum(c['errors'] for c in counters),
                    'bytes_received': sum(c['bytes_received'] for c in counters),
                    'telemetry_dropped_delta': int(report['status_after']['telemetry_dropped'])-int(report['status_before']['telemetry_dropped']),
                    'core_events_received_delta': int(report['core_status_after']['events_received'])-int(report['core_status_before']['events_received']),
                    'core_indexing_dropped_delta': int(report['core_status_after']['indexing_dropped'])-int(report['core_status_before']['indexing_dropped']),
                    'parsed_http_verified': True,
                    'load_rss_under_500_mib': max(rss) < 500*1024*1024,
                    'idle_rss_under_300_mib': sum(v['rss_bytes'] for v in report['idle'].values()) < 300*1024*1024}
            except Exception:
                log.flush()
                log.seek(0, os.SEEK_END)
                log.seek(max(0, log.tell()-16384))
                print(log.read(16384), file=__import__('sys').stderr)
                raise
            finally:
                stop.set()
                for child in reversed(processes):
                    if child.poll() is None:
                        child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)
                for server in servers:
                    server.shutdown()
                    server.server_close()
                for thread in server_threads:
                    thread.join(timeout=2)
    encoded = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(encoded+'\n')
    print(encoded)
    if report['summary']['errors'] or not report['summary']['load_rss_under_500_mib']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
