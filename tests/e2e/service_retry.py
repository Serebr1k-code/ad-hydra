"""Retry an enabled service listener after a real loopback EADDRINUSE."""
import http.client
import json
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import unittest

from relay import BIN, port


class ServiceRetry(unittest.TestCase):
    def test_retry_binds_freed_listener_and_keeps_revision(self):
        backend = socket.socket()
        backend.bind(('127.0.0.1', 0))
        backend.listen()
        backend.settimeout(.2)
        stop = threading.Event()

        def echo():
            while not stop.is_set():
                try:
                    client, _ = backend.accept()
                except socket.timeout:
                    continue
                threading.Thread(target=lambda c: (c.sendall(c.recv(4096)), c.close()), args=(client,), daemon=True).start()

        threading.Thread(target=echo, daemon=True).start()
        occupied = socket.socket()
        occupied.bind(('127.0.0.1', 0))
        occupied.listen()
        listen_port = occupied.getsockname()[1]

        with tempfile.TemporaryDirectory(prefix='hydra-service-retry-') as directory:
            tmp = pathlib.Path(directory)
            api_port, proxy_port = port(), port()
            origin = 'https://hydra.test'
            proxy_socket = str(tmp / 'proxy.sock')
            (tmp / 'proxy.json').write_text(json.dumps({
                'socket': proxy_socket,
                'state_file': str(tmp / 'proxy-state.json'),
                'snapshot': {'revision': 0, 'services': []},
            }))
            log = open(tmp / 'log', 'w+')
            core = [str(BIN / 'hydra-core'), '--database', str(tmp / 'core.db'), '--key', str(tmp / 'jwt.pem'), '--socket', proxy_socket]
            processes = []

            def request(method, path, payload=None, token=None):
                headers = {'Origin': origin}
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                if payload is not None:
                    headers['Content-Type'] = 'application/json'
                client = http.client.HTTPConnection('127.0.0.1', api_port, timeout=5)
                client.request(method, '/api/v1' + path, json.dumps(payload) if payload is not None else None, headers)
                response = client.getresponse()
                raw = response.read()
                client.close()
                return response.status, json.loads(raw) if raw else None

            def good(method, path, payload=None, token=None):
                status, body = request(method, path, payload, token)
                self.assertIn(status, (200, 201, 202, 204), (status, body))
                return body

            def wait_for(predicate, message):
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline:
                    try:
                        value = predicate()
                    except (OSError, http.client.HTTPException):
                        value = None
                    if value:
                        return value
                    if any(process.poll() is not None for process in processes):
                        log.flush(); log.seek(0)
                        self.fail(log.read())
                    time.sleep(.05)
                self.fail(message)

            try:
                subprocess.run(core + ['create', 'user', 'operator', 'test-password-1234'], check=True, stdout=log, stderr=log, timeout=10)
                processes.append(subprocess.Popen([str(BIN / 'hydra-proxy'), '--config', str(tmp / 'proxy.json')], stdout=log, stderr=log))
                processes.append(subprocess.Popen(core + ['serve', '--listen', f'127.0.0.1:{api_port}', '--origin', origin], stdout=log, stderr=log))
                wait_for(lambda: request('GET', '/services')[0] == 401, 'Core did not start')
                token = good('POST', '/auth/login', {'username': 'operator', 'password': 'test-password-1234'})['access_token']
                created = good('POST', '/services', {
                    'expected_revision': 0,
                    'transport': 'tcp',
                    'listen_address': f'127.0.0.1:{listen_port}',
                    'upstream_address': f'127.0.0.1:{backend.getsockname()[1]}',
                    'protocol_id': 'raw',
                    'enabled': True,
                }, token)
                service_id = created['id']
                failed = wait_for(
                    lambda: (body if (body := request('GET', '/services', token=token)[1]) and body['apply_error'] else None),
                    'occupied listener did not produce an apply error',
                )
                self.assertIn('98', failed['apply_error'], failed['apply_error'])
                self.assertEqual(failed['items'][0]['applied_enabled'], None)

                occupied.close()
                retried = good('POST', f'/services/{service_id}/retry', {'expected_revision': 1}, token)
                self.assertEqual(retried, {'revision': 1, 'status': 'retry_requested'})
                applied = wait_for(
                    lambda: (body if (body := request('GET', '/services', token=token)[1]) and body['apply_status'] == 'applied' else None),
                    'retry did not apply the freed listener',
                )
                self.assertEqual(applied['revision'], 1)
                self.assertEqual(applied['applied_revision'], 1)
                with socket.create_connection(('127.0.0.1', listen_port), timeout=2) as client:
                    client.sendall(b'retry-bound')
                    self.assertEqual(client.recv(64), b'retry-bound')

                with sqlite3.connect(tmp / 'core.db') as db:
                    actor_id, target, metadata = db.execute(
                        "SELECT actor_id,target,metadata FROM audit WHERE action='service.retry'"
                    ).fetchone()
                self.assertEqual(actor_id, good('GET', '/auth/me', token=token)['id'])
                self.assertEqual(target, service_id)
                self.assertEqual(json.loads(metadata)['expected_revision'], 1)
            finally:
                occupied.close()
                for process in processes:
                    if process.poll() is None:
                        process.terminate()
                for process in processes:
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
                log.close()
        stop.set()
        backend.close()


if __name__ == '__main__':
    unittest.main()
