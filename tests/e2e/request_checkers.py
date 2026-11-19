"""Exercise a real networkless request checker and its /state mount over loopback."""
import contextlib
import http.client
import http.server
import json
import pathlib
import subprocess
import tempfile
import threading
import time
import unittest

from relay import BIN, port


class RequestChecker(unittest.TestCase):
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
            root = pathlib.Path(directory)
            listener = port()
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

            checker_settings['policies'][0]['allow_on_error'] = True
            code, saved = api('PUT', endpoint, checker_settings, token)
            self.assertEqual(code, 200, saved)
            self.assertEqual(saved['revision'], 1)
            self.assertEqual(api('PUT', endpoint, checker_settings, token)[0], 409)
            stop(second)
            last = start()
            self.assertEqual(api('GET', endpoint, token=token), (200, saved))
            self.assertEqual(request('/timeout'), 200)
            self.assertEqual(status()['request_checker_errors'], 1)
            self.assertEqual(seen, ['/write', '/read', '/timeout'])
            stop(last)


if __name__ == '__main__':
    unittest.main()
