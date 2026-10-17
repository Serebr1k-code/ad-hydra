"""Real two-process acceptance harness. Run after cargo build --workspace."""
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / 'target' / os.environ.get('HYDRA_BUILD_PROFILE', 'debug')


def port(kind=socket.SOCK_STREAM):
    with socket.socket(type=kind) as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


class Relay(unittest.TestCase):
    def test_binary_tcp_udp_core_failure_and_restart(self):
        with tempfile.TemporaryDirectory(prefix='hydra-') as tmp:
            tmp = pathlib.Path(tmp)
            tcp = socket.socket()
            tcp.bind(('127.0.0.1', 0))
            tcp.listen()
            tcp.settimeout(0.2)
            udp = socket.socket(type=socket.SOCK_DGRAM)
            udp.bind(('127.0.0.1', 0))
            udp.settimeout(0.2)
            stop = threading.Event()

            def echo_tcp():
                while not stop.is_set():
                    try:
                        c, _ = tcp.accept()
                    except socket.timeout:
                        continue
                    def reply(c):
                        with c:
                            while data := c.recv(4096):
                                c.sendall(data)
                            c.sendall(b'after-half-close')
                    threading.Thread(target=reply, args=(c,), daemon=True).start()

            def echo_udp():
                while not stop.is_set():
                    try:
                        data, addr = udp.recvfrom(65535)
                        udp.sendto(data, addr)
                    except socket.timeout:
                        pass

            threading.Thread(target=echo_tcp, daemon=True).start()
            threading.Thread(target=echo_udp, daemon=True).start()
            tp, up = port(), port(socket.SOCK_DGRAM)
            snapshot = {'revision': 1, 'services': [
                {'id': 'tcp', 'transport': 'tcp', 'listen_address': f'127.0.0.1:{tp}',
                 'upstream_address': f'127.0.0.1:{tcp.getsockname()[1]}', 'enabled': True, 'protocol_id': 'raw'},
                {'id': 'udp', 'transport': 'udp', 'listen_address': f'127.0.0.1:{up}',
                 'upstream_address': f'127.0.0.1:{udp.getsockname()[1]}', 'enabled': True, 'protocol_id': 'raw'}]}
            sock = str(tmp / 'proxy.sock')
            cfg = {'socket': sock, 'state_file': str(tmp / 'last-good.json'),
                   'snapshot': snapshot, 'limits': {'max_flows': 4, 'udp_idle_ms': 200}}
            (tmp / 'proxy.json').write_text(json.dumps(cfg))
            (tmp / 'snapshot.json').write_text(json.dumps(snapshot))
            processes = []
            log = open(tmp / 'process.log', 'w+')

            def start_proxy():
                p = subprocess.Popen([str(BIN / 'hydra-proxy'), '--config', str(tmp / 'proxy.json')], stdout=log, stderr=log)
                processes.append(p)
                os.sched_setaffinity(p.pid, sorted(os.sched_getaffinity(0))[:2])
                for _ in range(100):
                    if p.poll() is not None:
                        log.seek(0)
                        self.fail(log.read())
                    try:
                        return status()
                    except (subprocess.CalledProcessError, FileNotFoundError):
                        time.sleep(.05)
                self.fail('proxy startup timeout')

            def cli(*args, ok=True):
                p = subprocess.run([str(BIN / 'hydra-core'), '--socket', sock, '--database', str(tmp / 'core.db'), *args], capture_output=True, text=True, timeout=5)
                if ok:
                    self.assertEqual(p.returncode, 0, p.stderr)
                    return json.loads(p.stdout)
                self.assertNotEqual(p.returncode, 0)

            def status():
                p = subprocess.run([str(BIN / 'hydra-core'), '--socket', sock, 'status'], capture_output=True, text=True, check=True, timeout=5)
                return json.loads(p.stdout)

            def api_request(method, path, token=None, payload=None):
                headers = {'Origin': origin}
                data = None
                if payload is not None:
                    headers['Content-Type'] = 'application/json'
                    data = json.dumps(payload).encode()
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                request = urllib.request.Request(f'http://127.0.0.1:{hp}{path}', data=data, headers=headers, method=method)
                with urllib.request.urlopen(request, timeout=3) as response:
                    return response.status, json.loads(response.read())

            def auth_token():
                _, result = api_request('POST', '/api/v1/auth/login', payload={
                    'username': 'history-test', 'password': 'test-password-12345'})
                return result['access_token']

            def find_flow(token, client_port):
                query = urllib.parse.urlencode({'service_id': 'tcp', 'limit': 50, 'newest_first': 'true'})
                _, result = api_request('GET', '/api/v1/traffic/flows?' + query, token)
                return next((flow for flow in result['items']
                             if flow.get('client', {}).get('port') == client_port), None)

            def check_saved_history(token):
                live = socket.create_connection(('127.0.0.1', tp), timeout=2)
                live.settimeout(3)
                client_port = live.getsockname()[1]
                sent = b'hydra-gzip-history-loopback'
                live.sendall(sent)
                self.assertEqual(live.recv(len(sent)), sent)
                flow = None
                for _ in range(60):
                    flow = find_flow(token, client_port)
                    if flow:
                        break
                    time.sleep(.05)
                self.assertIsNotNone(flow, 'Core did not index the local TCP session')
                flow_id = flow['flow_id']
                with self.assertRaises(urllib.error.HTTPError) as active:
                    api_request('GET', f'/api/v1/traffic/flows/{flow_id}/history', token)
                self.assertEqual(active.exception.code, 409)
                live.shutdown(socket.SHUT_WR)
                received = b''
                while data := live.recv(8192):
                    received += data
                live.close()
                self.assertEqual(received, b'after-half-close')
                history = None
                for _ in range(60):
                    try:
                        _, history = api_request('GET', f'/api/v1/traffic/flows/{flow_id}/history', token)
                        break
                    except urllib.error.HTTPError as error:
                        if error.code not in (404, 409):
                            raise
                        time.sleep(.05)
                self.assertIsNotNone(history, 'Proxy did not persist the closed TCP stream history')
                by_direction = {segment['direction']: bytes(segment['data']) for segment in history['segments']}
                self.assertIn(sent, by_direction[1])
                self.assertIn(b'after-half-close', by_direction[2])
                self.assertFalse(history['history_truncated'])

            def check_tcp():
                with socket.create_connection(('127.0.0.1', tp), timeout=2) as c:
                    payload = bytes(range(256)) * 257
                    for off in range(0, len(payload), 997):
                        c.sendall(payload[off:off+997])
                    c.shutdown(socket.SHUT_WR)
                    got = b''
                    while data := c.recv(8192):
                        got += data
                    self.assertEqual(got, payload + b'after-half-close')

            def check_udp():
                clients = [socket.socket(type=socket.SOCK_DGRAM) for _ in range(2)]
                try:
                    for i, c in enumerate(clients):
                        c.settimeout(2)
                        c.sendto(bytes([i, 0, 255]) * 1000, ('127.0.0.1', up))
                    for i, c in enumerate(clients):
                        self.assertEqual(c.recv(65535), bytes([i, 0, 255]) * 1000)
                    # Empty datagrams are valid and must survive.
                    clients[0].sendto(b'', ('127.0.0.1', up))
                    self.assertEqual(clients[0].recv(65535), b'')
                finally:
                    for c in clients:
                        c.close()

            try:
                first = start_proxy()
                self.assertEqual(first['applied_services_revision'], 1)
                self.assertEqual(os.stat(sock).st_mode & 0o777, 0o600)
                hp = port()
                origin = f'https://localhost:{hp}'
                created = subprocess.run([str(BIN / 'hydra-core'), '--database', str(tmp / 'core.db'), '--key', str(tmp / 'jwt.pem'), 'create', 'user', 'history-test', 'test-password-12345'], capture_output=True, text=True, timeout=10)
                self.assertEqual(created.returncode, 0, created.stderr)
                core = subprocess.Popen([str(BIN / 'hydra-core'), '--socket', sock, '--database', str(tmp / 'core.db'), '--key', str(tmp / 'jwt.pem'), 'serve', '--origin', origin, '--listen', f'127.0.0.1:{hp}', '--snapshot', str(tmp / 'snapshot.json')], stdout=log, stderr=log)
                processes.append(core)
                os.sched_setaffinity(core.pid, sorted(os.sched_getaffinity(0))[:2])
                for _ in range(100):
                    try:
                        token = auth_token()
                        break
                    except (urllib.error.URLError, ConnectionError):
                        time.sleep(.05)
                else:
                    self.fail('Core HTTP did not become ready for login')
                check_saved_history(token)
                check_tcp()
                check_udp()
                live = socket.create_connection(('127.0.0.1', tp), timeout=2)
                live.sendall(b'before-core-kill')
                self.assertEqual(live.recv(64), b'before-core-kill')
                core.kill()
                core.wait(timeout=5)
                live.sendall(b'after-core-kill')
                self.assertEqual(live.recv(64), b'after-core-kill')
                live.shutdown(socket.SHUT_WR)
                self.assertEqual(live.recv(64), b'after-half-close')
                live.close()
                check_tcp()
                check_udp()
                time.sleep(.5)
                held = [socket.create_connection(('127.0.0.1', tp), timeout=2) for _ in range(4)]
                for c in held:
                    c.sendall(b'held')
                    self.assertEqual(c.recv(4), b'held')
                with socket.create_connection(('127.0.0.1', tp), timeout=2) as denied:
                    self.assertEqual(denied.recv(16), b'')
                self.assertEqual(status()['active_flows'], 4)
                self.assertGreater(status()['rejected_flows'], 0)
                for c in held:
                    c.close()
                time.sleep(.5)
                self.assertEqual(status()['active_flows'], 0)
                snapshot['revision'] = 2
                (tmp / 'snapshot.json').write_text(json.dumps(snapshot))
                self.assertTrue(cli('apply', str(tmp / 'snapshot.json'))['applied'])
                self.assertTrue(cli('apply', str(tmp / 'snapshot.json'))['applied'])
                stale = dict(snapshot, revision=1)
                (tmp / 'stale.json').write_text(json.dumps(stale))
                cli('apply', str(tmp / 'stale.json'), ok=False)
                import sqlite3
                with sqlite3.connect(tmp / 'core.db') as audit_db:
                    rows=audit_db.execute("SELECT actor_kind,success FROM audit WHERE action='proxy.snapshot.result' ORDER BY id").fetchall()
                    self.assertEqual(rows,[('local_cli',1),('local_cli',1),('local_cli',0)])
                # Bind failure must leave the old snapshot and live listeners intact.
                occupied = socket.socket()
                occupied.bind(('127.0.0.1', 0))
                occupied.listen()
                bad = json.loads(json.dumps(snapshot))
                bad['revision'] = 3
                bad['services'][0]['listen_address'] = f'127.0.0.1:{occupied.getsockname()[1]}'
                (tmp / 'bad.json').write_text(json.dumps(bad))
                cli('apply', str(tmp / 'bad.json'), ok=False)
                occupied.close()
                self.assertEqual(status()['applied_services_revision'], 2)
                check_tcp()
                processes[0].kill()
                processes[0].wait(timeout=5)
                self.assertEqual(start_proxy()['applied_services_revision'], 2)
                check_tcp()
                check_udp()
                core = subprocess.Popen([str(BIN / 'hydra-core'), '--socket', sock, '--database', str(tmp / 'core.db'), '--key', str(tmp / 'jwt.pem'), 'serve', '--listen', f'127.0.0.1:{hp}', '--snapshot', str(tmp / 'snapshot.json')], stdout=log, stderr=log)
                processes.append(core)
                os.sched_setaffinity(core.pid, sorted(os.sched_getaffinity(0))[:2])
                time.sleep(.25)
                self.assertIsNone(core.poll())
                self.assertEqual(status()['applied_services_revision'], 2)
                measured=[processes[-2],core]
                def ticks(p):
                    fields=pathlib.Path(f'/proc/{p.pid}/stat').read_text().split()
                    return int(fields[13])+int(fields[14])
                before=[ticks(p) for p in measured]
                began=time.monotonic()
                time.sleep(1)
                elapsed=time.monotonic()-began
                for p,initial in zip(measured,before):
                    cpu=(ticks(p)-initial)/os.sysconf('SC_CLK_TCK')/elapsed*100
                    print(f'{p.args[0]} idle CPU={cpu:.2f}% of one core; affinity={sorted(os.sched_getaffinity(p.pid))}')
                for p in measured:
                    data = pathlib.Path(f'/proc/{p.pid}/status').read_text()
                    print(f'{p.args[0]} ' + next(line for line in data.splitlines() if line.startswith('VmRSS:')))
            finally:
                for p in processes:
                    if p.poll() is None:
                        p.terminate()
                    p.wait(timeout=5)
                stop.set()
                time.sleep(.25)
                tcp.close()
                udp.close()
                log.close()


if __name__ == '__main__':
    unittest.main()
