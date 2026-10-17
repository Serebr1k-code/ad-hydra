"""Harmless real-uv submitter acceptance against temporary Core + Proxy state.

Run after building binaries: python3 tests/e2e/python_submitter.py
Only synthetic flags are printed; no checker or target connections are made.
uv downloads idna==3.10 into the temporary runtime cache. SQL updates advance
only the fixture's durable dispatch clock to exercise retry backoff without sleeping.
"""
import gzip
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request
from relay import BIN, port


class PythonSubmitter(unittest.TestCase):
    def test_stdout_queue_window_real_python_redaction_restart_and_retry(self):
        uv = shutil.which('uv')
        self.assertIsNotNone(uv, 'real uv is required (this acceptance does not use a fake interpreter)')
        with tempfile.TemporaryDirectory(prefix='hydra-submitter-') as temporary:
            root = pathlib.Path(temporary)
            address = port()
            origin = f'https://localhost:{address}'
            operations = root / 'operations.json'
            operations.write_text(json.dumps({
                'execution_mode': 'host', 'auto_submit_captured': False,
                'host_runtime': {'uv': uv, 'work_dir': str(root / 'runtime')},
                # The API-selected pattern must replace this operator fallback for Farm.
                'flag_pattern': 'OPERATOR_ONLY_[A-Z]+',
            }))
            operations.chmod(0o600)
            proxy_config = root / 'proxy.json'
            proxy_config.write_text(json.dumps({
                'socket': str(root / 'proxy.sock'), 'state_file': str(root / 'snapshot.json'),
                'snapshot': {'revision': 1, 'services': []},
            }))
            proxy_config.chmod(0o600)
            base = [str(BIN / 'hydra-core'), '--database', str(root / 'db'),
                    '--key', str(root / 'key'), '--socket', str(root / 'proxy.sock')]
            subprocess.run(base + ['create', 'user', 'submitter-test', 'temporary-submitter-password'],
                           check=True, capture_output=True, timeout=10)
            token = None
            token_at = 0
            log = open(root / 'process.log', 'w+')
            processes = []

            def start_core():
                process = subprocess.Popen(base + ['serve', '--listen', f'127.0.0.1:{address}',
                                                  '--origin', origin, '--operations', str(operations)],
                                           stdout=log, stderr=log)
                processes.append(process)
                return process

            def request(method, path, value=None, raw=False):
                headers = {'Origin': origin, 'Idempotency-Key': os.urandom(16).hex()}
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                if value is not None:
                    headers['Content-Type'] = 'application/octet-stream' if raw else 'application/json'
                    value = value if raw else json.dumps(value).encode()
                req = urllib.request.Request(f'http://127.0.0.1:{address}/api/v1{path}', value, headers, method=method)
                with urllib.request.urlopen(req, timeout=5) as response:
                    content = response.read()
                    return json.loads(content) if content else None

            def login():
                nonlocal token, token_at
                token = request('POST', '/auth/login', {
                    'username': 'submitter-test', 'password': 'temporary-submitter-password',
                })['access_token']
                token_at = time.monotonic()

            def api(method, path, value=None, raw=False):
                if token and time.monotonic() - token_at > 45:
                    login()
                return request(method, path, value, raw)

            def ready(core):
                nonlocal token
                token = None
                for _ in range(150):
                    try:
                        login()
                        return
                    except (urllib.error.URLError, ConnectionError):
                        if core.poll() is not None:
                            log.flush()
                            self.fail((root / 'process.log').read_text())
                        time.sleep(.05)
                self.fail('Core did not become ready')

            def wait_for(description, fetch, predicate, seconds=30):
                deadline = time.monotonic() + seconds
                last = None
                while time.monotonic() < deadline:
                    last = fetch()
                    if predicate(last):
                        return last
                    time.sleep(.1)
                self.fail(f'{description} did not finish: {last}')

            def db_query(sql, parameters=()):
                with sqlite3.connect(root / 'db', timeout=10) as db:
                    return db.execute(sql, parameters).fetchall()

            def due():
                with sqlite3.connect(root / 'db', timeout=10) as db:
                    db.execute('UPDATE flag_submitter_clock SET next_batch_at=0 WHERE id=1')
                    db.execute("UPDATE submissions SET next_attempt=0 WHERE state='retry'")

            def configure(enabled, source):
                config = api('GET', '/submitter')['config']
                return api('PUT', '/submitter', {
                    'revision': config['revision'], 'enabled': enabled,
                    'flag_pattern': 'TESTFLAG_[A-Z]+', 'python_packages': ['idna==3.10'],
                    'source': source, 'timeout_seconds': 60,
                })

            def capture(identifier, source):
                api('PUT', f'/farm/exploits/{identifier}/versions/v1', source, raw=True)
                api('PUT', '/farm/exploits', {
                    'id': identifier, 'name': identifier, 'version': 'v1', 'enabled': True, 'revision': 0,
                    'service_id': 'local-service', 'team_ids': ['local-team'], 'target_ids': [],
                    'manifest': {'runtime': 'shell', 'mode': 'per_target', 'timeout_seconds': 10,
                                 'memory_mib': 64, 'cpu_percent': 50, 'python_packages': []},
                })
                run_id = api('POST', '/farm/runs', {
                    'exploit_id': identifier, 'revision': 1, 'team_id': 'local-team',
                })['id']
                run = wait_for('Farm print fixture', lambda: api('GET', '/farm/runs/' + run_id),
                               lambda row: row['status'] not in ('queued', 'running'))
                self.assertEqual(run['status'], 'succeeded', run)

            good_source = ('import idna, sys\n'
                           'print("IDNA", idna.__version__, "ARGC", len(sys.argv[1:]))\n'
                           'print("ARGS", *sys.argv[1:])\n'
                           'print("ERR", *sys.argv[1:], file=sys.stderr)\n')
            bad_source = good_source + 'sys.exit(7)\n'
            try:
                proxy = subprocess.Popen([str(BIN / 'hydra-proxy'), '--config', str(proxy_config)], stdout=log, stderr=log)
                processes.append(proxy)
                core = start_core()
                ready(core)
                view = configure(False, good_source)
                self.assertTrue(view['runtime_available'])
                self.assertFalse(view['config']['enabled'])
                api('PUT', '/farm/services', {'id': 'local-service', 'name': 'Local fixture', 'port': 12345})
                api('PUT', '/farm/teams', {'id': 'local-team', 'name': 'Owned loopback', 'host': '127.0.0.1'})
                capture('first', b'printf "TESTFLAG_ALPHA TESTFLAG_BETA TESTFLAG_ALPHA\\n"; printf "TESTFLAG_GAMMA\\n" >&2')
                view = wait_for('automatic queue capture', lambda: api('GET', '/submitter'),
                                lambda row: row['queued_flags'] == 3)
                self.assertIsNone(view['last_batch'])
                self.assertEqual(db_query('SELECT count(*) FROM submissions')[0][0], 3)
                self.assertEqual(db_query("SELECT count(*) FROM flags WHERE target_id='local-team' AND category='captured'")[0][0], 3)
                due()  # A paused configuration must not dispatch even when its clock is due.
                time.sleep(1.2)
                self.assertEqual(db_query('SELECT count(*) FROM flag_submitter_batches')[0][0], 0)
                view = configure(True, good_source)
                self.assertEqual(view['interval_seconds'], 1)
                self.assertLessEqual(view['next_batch_at'], int(time.time()) + 1)
                view = wait_for('real uv successful whole-batch submission', lambda: api('GET', '/submitter'),
                                lambda row: row['last_batch'] is not None and row['last_batch']['status'] == 'accepted', seconds=70)
                batch = view['last_batch']
                self.assertEqual(batch['flag_count'], 3)
                self.assertEqual(batch['exit_code'], 0)
                self.assertEqual(view['queued_flags'], 0)
                self.assertIn('IDNA 3.10 ARGC 3', batch['stdout'])
                self.assertEqual(db_query("SELECT state,attempts FROM submissions"), [('accepted', 1)] * 3)
                for stream in ('stdout', 'stderr'):
                    self.assertIn('[redacted flag]', batch[stream])
                    for flag in ('TESTFLAG_ALPHA', 'TESTFLAG_BETA', 'TESTFLAG_GAMMA'):
                        self.assertNotIn(flag, batch[stream])
                out, err = db_query('SELECT stdout_gzip,stderr_gzip FROM flag_submitter_batches WHERE id=?', (batch['id'],))[0]
                for compressed in (out, err):
                    self.assertEqual(compressed[:2], b'\x1f\x8b')
                    self.assertIn(b'[redacted flag]', gzip.decompress(compressed))
                    self.assertNotIn(b'TESTFLAG_', gzip.decompress(compressed))
                configure(False, good_source)
                capture('second', b'printf "TESTFLAG_DELTA\\n"')
                self.assertEqual(api('GET', '/submitter')['queued_flags'], 1)
                core.terminate(); core.wait(timeout=10)
                core = start_core(); ready(core)
                persisted = api('GET', '/submitter')
                self.assertEqual(persisted['config']['source'], good_source)
                self.assertEqual(persisted['config']['python_packages'], ['idna==3.10'])
                self.assertFalse(persisted['config']['enabled'])
                self.assertEqual(persisted['queued_flags'], 1)
                previous = persisted['last_batch']['id']
                configure(True, bad_source); due()
                failed = wait_for('failed script retained for retry', lambda: api('GET', '/submitter'),
                                  lambda row: row['last_batch']['id'] != previous and row['last_batch']['status'] == 'failed', seconds=70)
                self.assertEqual(failed['last_batch']['exit_code'], 7)
                self.assertEqual(failed['queued_flags'], 1)
                self.assertEqual(db_query("SELECT state,attempts FROM submissions WHERE state='retry'"), [('retry', 1)])
                previous = failed['last_batch']['id']
                configure(False, good_source); due(); time.sleep(1.2)
                self.assertEqual(api('GET', '/submitter')['last_batch']['id'], previous)
                configure(True, good_source); due()
                final = wait_for('retry accepted', lambda: api('GET', '/submitter'),
                                 lambda row: row['last_batch']['id'] != previous and row['last_batch']['status'] == 'accepted', seconds=70)
                self.assertEqual(final['queued_flags'], 0)
                self.assertEqual(db_query("SELECT count(*) FROM submissions WHERE state='accepted'")[0][0], 4)
                self.assertEqual(db_query('SELECT max(attempts) FROM submissions')[0][0], 2)
                print('PASS: real uv+idna, Farm stdout/stderr regex, durable automatic queue, immediate dispatch, whole-batch acceptance, redacted gzip logs, restart, pause, failure and retry')
            finally:
                for process in reversed(processes):
                    if process.poll() is not None:
                        continue
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
                log.close()


if __name__ == '__main__':
    unittest.main()
