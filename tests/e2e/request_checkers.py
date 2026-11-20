"""Exercise a real networkless request checker and its /state mount over loopback."""
import contextlib
import http.client
import http.server
import json
import pathlib
import sqlite3
import socket
import subprocess
import tempfile
import threading
import time
import unittest

from relay import BIN, port


class RequestChecker(unittest.TestCase):
    def test_managed_checker_creation_without_static_checker_config(self):
        with tempfile.TemporaryDirectory(prefix='hydra-managed-checker-') as directory:
            root = pathlib.Path(directory)
            config = root / 'proxy.json'
            snapshot = {'revision': 0, 'services': []}
            config.write_text(json.dumps({
                'socket': str(root / 'proxy.sock'),
                'state_file': str(root / 'saved.json'),
                'snapshot': snapshot,
            }))
            seed = root / 'seed.json'
            seed.write_text(json.dumps(snapshot))
            console_port = port()
            origin = f'https://localhost:{console_port}'
            core = [BIN / 'hydra-core', '--database', root / 'db.sqlite3', '--key', root / 'jwt.pem',
                    '--socket', root / 'proxy.sock']

            def stop(process):
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)

            subprocess.run(core + ['create', 'user', 'operator', 'test-password-1234'],
                           check=True, stdout=subprocess.DEVNULL, timeout=5)
            core_process = subprocess.Popen(core + ['serve', '--listen', f'127.0.0.1:{console_port}',
                '--origin', origin, '--snapshot', seed], stdout=subprocess.DEVNULL)
            proxy_process = subprocess.Popen([BIN / 'hydra-proxy', '--config', config], stdout=subprocess.DEVNULL)
            self.addCleanup(stop, proxy_process)
            self.addCleanup(stop, core_process)

            def api(method, path, payload=None, token=None):
                headers = {'Content-Type': 'application/json', 'Origin': origin}
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                with contextlib.closing(http.client.HTTPConnection('127.0.0.1', console_port, timeout=3)) as conn:
                    conn.request(method, '/api/v1' + path, None if payload is None else json.dumps(payload), headers)
                    response = conn.getresponse()
                    return response.status, json.loads(response.read())

            deadline = time.monotonic() + 5
            token = None
            while time.monotonic() < deadline and core_process.poll() is None and proxy_process.poll() is None:
                try:
                    code, answer = api('POST', '/auth/login',
                                       {'username': 'operator', 'password': 'test-password-1234'})
                    if code == 200:
                        token = answer['access_token']
                        break
                except (ConnectionError, OSError):
                    pass
                time.sleep(.05)
            self.assertIsNotNone(token, 'Core/Proxy did not become ready without static checker programs')
            endpoint = '/filters/request-checkers/programs'
            deadline = time.monotonic() + 5
            while True:
                try:
                    code, programs = api('GET', endpoint, token=token)
                    if code == 200:
                        break
                except (ConnectionError, OSError):
                    pass
                if time.monotonic() >= deadline or core_process.poll() is not None or proxy_process.poll() is not None:
                    self.fail('Managed checker API did not become ready without static checker programs')
                time.sleep(.05)
            self.assertEqual(code, 200, programs)
            self.assertEqual(programs['programs'], [])
            checker_id = 'created-from-empty'
            source = 'def check(request: bytes) -> bool:\n    return request == b"allow"\n'
            code, created = api('PUT', endpoint + '/' + checker_id,
                                {'revision': programs['revision'], 'source': source}, token)
            self.assertEqual(code, 200, created)
            self.assertEqual(created['programs'][0]['source'], source)

    def test_state_survives_proxy_restart(self):
        seen = []

        class Backend(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                seen.append(self.path)
                self.send_response(200)
                self.send_header('Content-Length', '0')
                self.end_headers()

            def log_message(self, *args):
                pass

        with http.server.ThreadingHTTPServer(('127.0.0.1', 0), Backend) as backend, \
                tempfile.TemporaryDirectory(prefix='hydra-checker-') as directory:
            thread = threading.Thread(target=backend.serve_forever, daemon=True)
            thread.start()
            self.addCleanup(backend.shutdown)
            udp_backend = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            udp_backend.bind(('127.0.0.1', 0))
            self.addCleanup(udp_backend.close)
            udp_backend_port = udp_backend.getsockname()[1]
            udp_stop = threading.Event()

            def echo_udp():
                while not udp_stop.is_set():
                    try:
                        data, peer = udp_backend.recvfrom(4096)
                        udp_backend.sendto(b'UPSTREAM:' + data, peer)
                    except OSError:
                        return

            udp_thread = threading.Thread(target=echo_udp, daemon=True)
            udp_thread.start()
            self.addCleanup(udp_stop.set)
            root = pathlib.Path(directory)
            listener = port()
            udp_listener_socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            udp_listener_socket.bind(('127.0.0.1', 0))
            udp_listener = udp_listener_socket.getsockname()[1]
            udp_listener_socket.close()
            checker = root / 'check.py'
            checker.write_text("""
from pathlib import Path
import time

state = Path('/state/marker')
def check(request):
    if request.startswith(b'GET /write '):
        state.write_bytes(b'persisted')
        return True
    if request.startswith(b'GET /timeout '):
        time.sleep(3)
    return request.startswith(b'GET /read ') and state.read_bytes() == b'persisted'
""")
            config = root / 'proxy.json'
            configuration = {
                'socket': str(root / 'proxy.sock'),
                'state_file': str(root / 'saved.json'),
                'snapshot': {'revision': 1, 'services': [{
                    'id': 'local',
                    'transport': 'tcp',
                    'protocol_id': 'http',
                    'listen_address': f'127.0.0.1:{listener}',
                    'upstream_address': f'127.0.0.1:{backend.server_port}',
                    'enabled': True,
                }, {
                    'id': 'udp-local',
                    'transport': 'udp',
                    'protocol_id': 'raw',
                    'listen_address': f'127.0.0.1:{udp_listener}',
                    'upstream_address': f'127.0.0.1:{udp_backend_port}',
                    'enabled': True,
                }]},
                'request_checkers': [{
                    'id': 'example',
                    'services': ['local'],
                    'script': str(checker),
                    'timeout_ms': 500,
                    'on_error': 'block',
                }],
            }
            config.write_text(json.dumps(configuration))
            seed = root / 'seed.json'
            seed.write_text(json.dumps(configuration['snapshot']))
            console_port = port()
            origin = f'https://localhost:{console_port}'
            core = [BIN / 'hydra-core', '--database', root / 'db.sqlite3', '--key', root / 'jwt.pem',
                    '--socket', root / 'proxy.sock']

            def api(method, path, payload=None, token=None):
                headers = {'Content-Type': 'application/json', 'Origin': origin}
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                with contextlib.closing(http.client.HTTPConnection('127.0.0.1', console_port, timeout=3)) as conn:
                    conn.request(method, '/api/v1' + path, None if payload is None else json.dumps(payload), headers)
                    response = conn.getresponse()
                    return response.status, json.loads(response.read())

            def start_core():
                subprocess.run(core + ['create', 'user', 'operator', 'test-password-1234'],
                               check=True, stdout=subprocess.DEVNULL, timeout=5)
                process = subprocess.Popen(core + ['serve', '--listen', f'127.0.0.1:{console_port}',
                    '--origin', origin, '--snapshot', seed], stdout=subprocess.DEVNULL)
                self.addCleanup(stop, process)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and process.poll() is None:
                    try:
                        code, answer = api('POST', '/auth/login',
                                           {'username': 'operator', 'password': 'test-password-1234'})
                        if code == 200:
                            return answer['access_token']
                    except ConnectionError:
                        pass
                    time.sleep(.05)
                self.fail('Core did not become ready')

            def status():
                return json.loads(subprocess.check_output([
                    BIN / 'hydra-core', '--socket', root / 'proxy.sock', 'status',
                ], stderr=subprocess.DEVNULL, timeout=2))

            def start():
                process = subprocess.Popen(
                    [BIN / 'hydra-proxy', '--config', config],
                    stdout=subprocess.DEVNULL,
                )
                self.addCleanup(stop, process)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and process.poll() is None:
                    try:
                        if status()['applied_services_revision'] == 1:
                            return process
                    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                        pass
                    time.sleep(.05)
                self.fail('Proxy did not become ready')

            def stop(process):
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)

            def request(path):
                with contextlib.closing(http.client.HTTPConnection('127.0.0.1', listener, timeout=3)) as conn:
                    conn.request('GET', path)
                    try:
                        return conn.getresponse().status
                    except (http.client.RemoteDisconnected, ConnectionResetError):
                        return None

            def udp_request(data):
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as conn:
                    conn.settimeout(1.5)
                    conn.sendto(data, ('127.0.0.1', udp_listener))
                    try:
                        return conn.recvfrom(4096)[0]
                    except socket.timeout:
                        return None

            first = start()
            token = start_core()
            endpoint = '/filters/request-checkers'
            self.assertEqual(api('GET', endpoint)[0], 401)
            code, checker_settings = api('GET', endpoint, token=token)
            self.assertEqual(code, 200)
            self.assertEqual(checker_settings['revision'], 0)
            self.assertEqual(request('/write'), 200)
            self.assertEqual((root / 'request-checkers/example/marker').read_bytes(), b'persisted')
            stop(first)
            second = start()
            self.assertEqual(request('/read'), 200)
            self.assertEqual(request('/deny'), None)
            self.assertEqual(status()['request_checker_errors'], 0)
            self.assertEqual(request('/timeout'), None)
            checked = status()
            self.assertEqual(checked['request_checker_errors'], 1)
            self.assertIn('worker timed out', checked['last_request_checker_error'])
            self.assertEqual(seen, ['/write', '/read'])

            # Managed source editing is a separate API from policy assignment.
            # It shares the same optimistic revision so stale editors cannot
            # silently overwrite either source or policy changes.
            programs_endpoint = '/filters/request-checkers/programs'
            self.assertEqual(api('GET', programs_endpoint)[0], 401)
            self.assertEqual(api('PUT', programs_endpoint + '/managed-loopback',
                                 {'revision': 0, 'source': 'def check(request): return True'}, token=None)[0], 401)
            self.assertEqual(api('DELETE', programs_endpoint + '/managed-loopback',
                                 {'revision': 0}, token=None)[0], 401)
            code, programs = api('GET', programs_endpoint, token=token)
            self.assertEqual(code, 200, programs)
            legacy = next(program for program in programs['programs'] if program['id'] == 'example')
            self.assertFalse(legacy['managed'])
            self.assertEqual(legacy['runtime'], 'python')
            checker_id = 'managed-loopback'
            allow_source = "from pathlib import Path\n\nstate = Path('/state/managed-marker')\ndef check(request: bytes) -> bool:\n    if b'/state-write' in request:\n        state.write_bytes(b'initial managed state')\n        return True\n    if b'/state-read' in request:\n        return state.read_bytes() == b'initial managed state'\n    return b'/allow' in request or request == b'ALLOW'\n"
            invalid_programs = [
                ('bad id', programs_endpoint + '/invalid.id', {'revision': programs['revision'], 'source': allow_source}),
                ('empty source', programs_endpoint + '/managed-loopback', {'revision': programs['revision'], 'source': ''}),
                ('oversized source', programs_endpoint + '/managed-loopback', {'revision': programs['revision'], 'source': 'x' * (256 * 1024 + 1)}),
            ]
            for label, path, payload in invalid_programs:
                with self.subTest(label=label):
                    self.assertEqual(api('PUT', path, payload, token)[0], 400)
            unchanged_code, unchanged = api('GET', programs_endpoint, token=token)
            self.assertEqual(unchanged_code, 200)
            self.assertEqual(unchanged['revision'], programs['revision'])
            self.assertEqual(unchanged['programs'], programs['programs'])
            code, created = api('PUT', programs_endpoint + '/' + checker_id,
                               {'revision': programs['revision'], 'source': allow_source}, token)
            self.assertEqual(code, 200, created)
            programs_revision = created['revision']
            self.assertEqual(next(p for p in created['programs'] if p['id'] == checker_id)['source'], allow_source)
            self.assertEqual(api('PUT', programs_endpoint + '/' + checker_id,
                                 {'revision': programs['revision'], 'source': allow_source}, token)[0], 409)
            self.assertIn(api('PUT', programs_endpoint + '/example',
                              {'revision': programs_revision, 'source': allow_source}, token)[0], (400, 403, 409))

            code, current_settings = api('GET', endpoint, token=token)
            self.assertEqual(code, 200)
            managed_policy = next(p for p in current_settings['policies'] if p['id'] == checker_id)
            managed_policy['services'] = ['local', 'udp-local']
            legacy_policy = next(p for p in current_settings['policies'] if p['id'] == 'example')
            legacy_policy['services'] = []
            code, assigned = api('PUT', endpoint, current_settings, token)
            self.assertEqual(code, 200, assigned)
            self.assertEqual(assigned['revision'], programs_revision + 1)
            self.assertEqual(request('/allow'), 200)
            self.assertIsNone(request('/deny'))
            self.assertEqual(udp_request(b'ALLOW'), b'UPSTREAM:ALLOW')
            self.assertIsNone(udp_request(b'BLOCK'))
            self.assertEqual(request('/state-write'), 200)
            state_file = root / f'request-checkers/{checker_id}/managed-marker'
            self.assertEqual(state_file.read_bytes(), b'initial managed state')
            self.assertEqual(request('/state-read'), 200)

            # Updating the managed file changes enforcement on subsequent
            # requests without restarting Proxy.
            deny_source = "from pathlib import Path\n\nstate = Path('/state/managed-marker')\ndef check(request: bytes) -> bool:\n    if b'/state-write' in request:\n        state.write_bytes(b'edited managed state')\n        return True\n    if b'/state-read' in request:\n        return state.read_bytes() == b'edited managed state'\n    return b'/deny' in request or request == b'BLOCK'\n"
            code, changed = api('PUT', programs_endpoint + '/' + checker_id,
                                {'revision': assigned['revision'], 'source': deny_source}, token)
            self.assertEqual(code, 200, changed)
            self.assertIsNone(request('/allow'))
            self.assertEqual(request('/deny'), 200)
            self.assertIsNone(udp_request(b'ALLOW'))
            self.assertEqual(udp_request(b'BLOCK'), b'UPSTREAM:BLOCK')
            self.assertEqual(request('/state-write'), 200)
            self.assertEqual(state_file.read_bytes(), b'edited managed state')
            stop(second)
            restarted = start()
            self.assertEqual(request('/state-read'), 200)
            self.assertEqual(state_file.read_bytes(), b'edited managed state')

            # Removing the program unassigns it while retaining its private
            # state directory for a possible later recreation.
            code, deleted = api('DELETE', programs_endpoint + '/' + checker_id,
                                {'revision': changed['revision']}, token)
            self.assertEqual(code, 200, deleted)
            self.assertTrue((root / f'request-checkers/{checker_id}').is_dir())
            self.assertEqual(state_file.read_bytes(), b'edited managed state')
            self.assertEqual(api('DELETE', programs_endpoint + '/' + checker_id,
                                 {'revision': changed['revision']}, token)[0], 409)
            with sqlite3.connect(root / 'db.sqlite3') as db:
                operator_id = db.execute("SELECT id FROM users WHERE username='operator'").fetchone()[0]
                audit = db.execute("SELECT action,target,success,actor_kind,actor_id FROM audit WHERE action='filter.checker_programs.write' AND target=? AND success=1 ORDER BY id", [checker_id]).fetchall()
                deleted_audit = db.execute("SELECT action,target,success,actor_kind,actor_id FROM audit WHERE action='filter.checker_programs.delete' AND target=? AND success=1 ORDER BY id", [checker_id]).fetchall()
            self.assertEqual(audit, [
                ('filter.checker_programs.write', checker_id, 1, 'user', operator_id),
                ('filter.checker_programs.write', checker_id, 1, 'user', operator_id),
            ])
            self.assertEqual(deleted_audit, [('filter.checker_programs.delete', checker_id, 1, 'user', operator_id)])

            _, latest_settings = api('GET', endpoint, token=token)
            latest_legacy = next(p for p in latest_settings['policies'] if p['id'] == 'example')
            latest_legacy['services'] = ['local']
            latest_legacy['allow_on_error'] = True
            code, saved = api('PUT', endpoint, latest_settings, token)
            self.assertEqual(code, 200, saved)
            self.assertEqual(saved['revision'], deleted['revision'] + 1)
            self.assertEqual(api('PUT', endpoint, latest_settings, token)[0], 409)
            stop(restarted)
            last = start()
            self.assertEqual(api('GET', endpoint, token=token), (200, saved))
            self.assertEqual(request('/timeout'), 200)
            self.assertEqual(status()['request_checker_errors'], 1)
            self.assertEqual(seen, ['/write', '/read', '/allow', '/state-write', '/state-read',
                                    '/deny', '/state-write', '/state-read', '/timeout'])
            stop(last)


if __name__ == '__main__':
    unittest.main()
