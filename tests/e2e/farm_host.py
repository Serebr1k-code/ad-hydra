"""Host Farm acceptance using harmless prints, temporary state and loopback API.
Python dependency coverage downloads idna through uv into a temporary cache.
Run after building binaries: python3 tests/e2e/farm_host.py
"""
import gzip
import json
import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request
from relay import BIN, port


class HostFarm(unittest.TestCase):
    def test_catalog_runtimes_live_gzip_logs_and_rotation(self):
        uv = shutil.which('uv')
        self.assertIsNotNone(uv, 'uv is required for the real Python runtime test')
        with tempfile.TemporaryDirectory(prefix='hydra-farm-host-') as tmp:
            root = pathlib.Path(tmp)
            address = port()
            origin = f'https://localhost:{address}'
            config = {'execution_mode': 'host', 'host_runtime': {'uv': uv, 'work_dir': str(root / 'work')},
                      'flag_pattern': 'TESTFLAG_[A-Z]{4}'}
            ops = root / 'ops.json'
            ops.write_text(json.dumps(config)); ops.chmod(0o600)
            snapshot = {'revision': 1, 'services': []}
            proxy_config = root / 'proxy.json'
            proxy_config.write_text(json.dumps({'socket': str(root / 'p.sock'), 'state_file': str(root / 'state.json'), 'snapshot': snapshot}))
            proxy_config.chmod(0o600)
            base = [str(BIN / 'hydra-core'), '--database', str(root / 'db'), '--key', str(root / 'key'), '--socket', str(root / 'p.sock')]
            subprocess.run(base + ['create', 'user', 'farm-test', 'long-farm-test-password'], check=True, capture_output=True, timeout=10)
            token = None
            log = open(root / 'process.log', 'w+')
            proxy = subprocess.Popen([str(BIN / 'hydra-proxy'), '--config', str(proxy_config)], stdout=log, stderr=log)
            core = subprocess.Popen(base + ['serve', '--listen', f'127.0.0.1:{address}', '--origin', origin, '--operations', str(ops)], stdout=log, stderr=log)

            def api(method, path, value=None, raw=False):
                headers = {'Origin': origin, 'Idempotency-Key': os.urandom(16).hex()}
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                if value is not None:
                    headers['Content-Type'] = 'application/octet-stream' if raw else 'application/json'
                    value = value if raw else json.dumps(value).encode()
                req = urllib.request.Request(f'http://127.0.0.1:{address}/api/v1{path}', value, headers, method=method)
                with urllib.request.urlopen(req, timeout=5) as response:
                    return json.loads(response.read())

            def wait_run(run_id, predicate=lambda run: run['status'] not in ('queued', 'running'), seconds=30):
                until = time.monotonic() + seconds
                while time.monotonic() < until:
                    detail = api('GET', '/farm/runs/' + run_id)
                    if predicate(detail):
                        return detail
                    time.sleep(.1)
                self.fail(f'run {run_id} did not reach expected state: {detail}')

            def exploit(identifier, runtime, source, packages=None):
                api('PUT', f'/farm/exploits/{identifier}/versions/v1', source, raw=True)
                value = {'id': identifier, 'name': identifier, 'version': 'v1', 'enabled': True, 'revision': 0,
                         'service_id': 'service', 'team_ids': [], 'target_ids': [],
                         'manifest': {'runtime': runtime, 'mode': 'per_target', 'timeout_seconds': 60,
                                      'memory_mib': 64, 'cpu_percent': 50, 'python_packages': packages or []}}
                api('PUT', '/farm/exploits', value)
                return value

            def run(identifier, team='one'):
                return api('POST', '/farm/runs', {'exploit_id': identifier, 'revision': 1, **({'team_id': team} if team else {})})['id']

            try:
                for _ in range(100):
                    try:
                        token = api('POST', '/auth/login', {'username': 'farm-test', 'password': 'long-farm-test-password'})['access_token']
                        break
                    except (urllib.error.URLError, ConnectionError):
                        if core.poll() is not None:
                            log.flush(); self.fail((root / 'process.log').read_text())
                        time.sleep(.05)
                self.assertIsNotNone(token)
                api('PUT', '/farm/services', {'id': 'service', 'name': 'Test service', 'port': 12345})
                for identifier, host in [('one', '127.0.0.1'), ('two', '127.0.0.2')]:
                    api('PUT', '/farm/teams', {'id': identifier, 'name': identifier, 'host': host})
                self.assertEqual(len(api('GET', '/farm/teams')['items']), 2)
                exploit('bash', 'shell', b'printf "OUT:%s:%s TESTFLAG_DEMO\\n" "$1" "$2"; printf "ERR:%s\\n" "$TARGET_HOST" >&2')
                bash_id = run('bash')
                detail = wait_run(bash_id)
                self.assertEqual(detail['status'], 'succeeded', detail)
                self.assertIn('OUT:127.0.0.1:12345', detail['stdout'])
                self.assertIn('ERR:127.0.0.1', detail['stderr'])
                all_id = run('bash', None)
                detail = wait_run(all_id)
                self.assertEqual(detail['status'], 'succeeded', detail)
                self.assertIn('OUT:127.0.0.2:12345', detail['stdout'])
                exploit('python', 'python', b'import idna,sys\nprint("PY",sys.argv[1],sys.argv[2],idna.encode("example.org").decode())\n', ['idna==3.10'])
                detail = wait_run(run('python'), seconds=65)
                self.assertEqual(detail['status'], 'succeeded', detail)
                self.assertIn('PY 127.0.0.1 12345 example.org', detail['stdout'])
                exploit('binary', 'elf', pathlib.Path('/bin/echo').read_bytes())
                detail = wait_run(run('binary'))
                self.assertEqual(detail['status'], 'succeeded', detail)
                self.assertIn('127.0.0.1 12345', detail['stdout'])
                exploit('cancel', 'shell', b'printf "started-live\\n"; sleep 20')
                cancel_id = run('cancel')
                detail = wait_run(cancel_id, lambda row: 'started-live' in row['stdout'])
                self.assertEqual(detail['status'], 'running')
                api('POST', f'/farm/runs/{cancel_id}/cancel')
                detail = wait_run(cancel_id)
                self.assertEqual(detail['status'], 'cancelled')
                self.assertIn('started-live', detail['stdout'])
                with sqlite3.connect(f'file:{root / "db"}?mode=ro', uri=True) as db:
                    plain, compressed = db.execute('SELECT stdout,stdout_gzip FROM farm_runs WHERE id=?', (bash_id,)).fetchone()
                    self.assertEqual(plain, '')
                    self.assertEqual(compressed[:2], b'\x1f\x8b')
                    self.assertIn(b'OUT:127.0.0.1:12345', gzip.decompress(compressed))
                    self.assertGreater(db.execute("SELECT count(*) FROM flags WHERE target_id='one' AND category='captured'").fetchone()[0], 0)
                self.assertEqual(api('GET', '/farm/runs')['items'][0]['id'], cancel_id)
                deleted = api('DELETE', '/farm/runs?older_than_days=0')['deleted']
                self.assertGreaterEqual(deleted, 4)
                print('PASS: services/teams, direct/all teams, Bash, Python uv+idna pin, binary, live gzip logs, flag provenance, cancel, newest-first, rotation')
            finally:
                for process in [core, proxy]:
                    process.terminate()
                    try:
                        process.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
                log.close()


if __name__ == '__main__':
    unittest.main()
