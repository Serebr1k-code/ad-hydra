"""Two-process UDP saved history, replay, enforcement, quota tail and restart.

Run after a coordinated cargo build: python3 tests/e2e/udp_history.py
Only benign binary loopback echo traffic is generated.
"""
import http.client
import json
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from relay import BIN, port


class UdpHistory(unittest.TestCase):
    def test_saved_datagrams_replay_and_restart(self):
        backend = socket.socket(type=socket.SOCK_DGRAM)
        backend.bind(('127.0.0.1', 0))
        backend.settimeout(.1)
        stop = threading.Event()

        def echo():
            while not stop.is_set():
                try:
                    data, peer = backend.recvfrom(65536)
                    backend.sendto(data, peer)
                except socket.timeout:
                    pass
                except OSError:
                    break

        threading.Thread(target=echo, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory(prefix='hydra-udp-history-') as directory:
                root = pathlib.Path(directory)
                listen_port, api_port = port(socket.SOCK_DGRAM), port()
                origin = 'https://hydra.test'
                service = dict(id='udp', transport='udp', listen_address=f'127.0.0.1:{listen_port}',
                               upstream_address=f'127.0.0.1:{backend.getsockname()[1]}',
                               protocol_id='raw', enabled=True, retain_history=True)
                snapshot = dict(revision=1, services=[service])
                config = dict(socket=str(root/'proxy.sock'), state_file=str(root/'state'),
                              snapshot=snapshot, limits=dict(udp_idle_ms=400))
                (root/'proxy.json').write_text(json.dumps(config))
                (root/'seed').write_text(json.dumps(snapshot))
                args = [str(BIN/'hydra-core'), '--database', str(root/'db'), '--key', str(root/'key'),
                        '--socket', str(root/'proxy.sock')]
                processes, clients = [], []
                with (root/'log').open('w+') as log:
                    subprocess.run(args+['create', 'user', 'operator', 'test-password-1234'],
                                   check=True, stdout=log, stderr=log, timeout=15)

                    def proxy():
                        proc = subprocess.Popen([str(BIN/'hydra-proxy'), '--config', str(root/'proxy.json')],
                                                stdout=log, stderr=log)
                        processes.append(proc)
                        return proc

                    def request(method, path, value=None, token=None):
                        headers = {'Origin': origin}
                        if token:
                            headers['Authorization'] = 'Bearer '+token
                        body = None
                        if value is not None:
                            body = json.dumps(value)
                            headers['Content-Type'] = 'application/json'
                        connection = http.client.HTTPConnection('127.0.0.1', api_port, timeout=4)
                        try:
                            connection.request(method, '/api/v1'+path, body, headers)
                            response = connection.getresponse()
                            data = response.read()
                            return response.status, json.loads(data) if data else None
                        finally:
                            connection.close()

                    def wait(action):
                        last = None
                        for _ in range(200):
                            try:
                                last = action()
                                if last:
                                    return last
                            except (OSError, http.client.HTTPException):
                                pass
                            time.sleep(.05)
                        log.flush()
                        self.fail(f'wait expired: {last}\n{(root/"log").read_text()}')

                    def history(flow_id):
                        response = request('GET', f'/traffic/flows/{flow_id}/history', token=token)
                        return response[1] if response[0] == 200 else None

                    def exchange(client, data):
                        client.send(data)
                        self.assertEqual(client.recv(65536), data)

                    try:
                        pp = proxy()
                        processes.append(subprocess.Popen(args+['serve', '--snapshot', str(root/'seed'),
                            '--listen', f'127.0.0.1:{api_port}', '--origin', origin], stdout=log, stderr=log))
                        token = wait(lambda: (r[1]['access_token'] if (r := request('POST', '/auth/login',
                            dict(username='operator', password='test-password-1234')))[0] == 200 else None))
                        for marker in (1, 2):
                            client = socket.socket(type=socket.SOCK_DGRAM)
                            clients.append(client)
                            client.bind(('127.0.0.1', 0))
                            client.connect(('127.0.0.1', listen_port))
                            client.settimeout(2)
                            # Startup readiness via Proxy status avoids generating retry datagrams.
                            wait(lambda: (r[0] == 200 and r[1].get('connected'))
                                 if (r := request('GET', '/system/proxy', token=token)) else False)
                            exchange(client, bytes([marker, 0, 255]))
                            exchange(client, b'')
                            exchange(client, b'tail')
                        flows = wait(lambda: (rows if len(rows := request('GET', '/traffic/flows?include_blocked=true', token=token)[1]['items']) == 2 else None))
                        expected = {}
                        for flow in flows:
                            saved = wait(lambda: history(flow['flow_id']))
                            expected[flow['flow_id']] = saved
                            self.assertEqual(saved['transport'], 2)
                            self.assertFalse(saved['history_truncated'])
                            self.assertEqual([s['direction'] for s in saved['segments']], [1, 2, 1, 2, 1, 2])
                            self.assertEqual([s['sequence'] for s in saved['segments']], [1, 2, 3, 4, 5, 6])
                            self.assertTrue(all(s['datagram'] for s in saved['segments']))
                            self.assertEqual([s['data'] for s in saved['segments'][2:4]], [[], []])
                            self.assertEqual(saved['datagrams_client_to_server'], 3)
                            self.assertEqual(saved['datagrams_server_to_client'], 3)
                            self.assertEqual(request('GET', f'/traffic/flows/{flow["flow_id"]}/history')[0], 401)
                        self.assertEqual({tuple(h['segments'][0]['data']) for h in expected.values()}, {(1, 0, 255), (2, 0, 255)})
                        first_id = flows[0]['flow_id']
                        candidate = dict(service_id='udp', flow_ids=[first_id], name='UDP preview', description='',
                            enabled=True, mode='enforce', priority=10, backend='userspace', expires_at_ms=None,
                            expression=dict(field='packet.size', op='eq', value=dict(type='uint', value=0)),
                            action=dict(type='drop_datagram'))
                        status, preview = request('POST', '/filters/preview', candidate, token)
                        self.assertEqual(status, 200, preview)
                        self.assertEqual(preview['results'][0]['state'], 'ready')
                        self.assertTrue(preview['results'][0]['matched'])
                        self.assertTrue(preview['results'][0]['would_block'])
                        # Whole-filter direction guards also work on retained UDP datagrams.
                        original_expression=candidate['expression']
                        for side in ['client_to_server','server_to_client']:
                            candidate['expression']={'direction':side,'scoped':original_expression}
                            status, directional_preview=request('POST','/filters/preview',candidate,token)
                            self.assertEqual(status,200,directional_preview)
                            self.assertTrue(directional_preview['results'][0]['would_block'],side)
                        candidate['expression']=original_expression
                        # Datagram-size preview must not see the concatenated six-byte stream.
                        candidate['expression']['value']['value'] = 6
                        status, preview = request('POST', '/filters/preview', candidate, token)
                        self.assertEqual(status, 200, preview)
                        self.assertFalse(preview['results'][0]['matched'])
                        candidate['expression'] = dict(field='raw.bytes', op='contains', value=dict(type='bytes', value=[255, 116]))
                        status, preview = request('POST', '/filters/preview', candidate, token)
                        self.assertEqual(status, 200, preview)
                        self.assertFalse(preview['results'][0]['matched'])
                        candidate['expression']['value']['value'] = [0, 255]
                        status, preview = request('POST', '/filters/preview', candidate, token)
                        self.assertEqual(status, 200, preview)
                        self.assertTrue(preview['results'][0]['matched'])
                        # A later flow overflows 64 KiB in each direction by one whole datagram.
                        client = clients[0]
                        for marker in (3, 4, 5):
                            exchange(client, bytes([marker])*32768)
                        latest = wait(lambda: next((f for f in request('GET', '/traffic/flows?include_blocked=true', token=token)[1]['items'] if f['flow_id'] not in expected), None))
                        tail = wait(lambda: history(latest['flow_id']))
                        self.assertTrue(tail['history_truncated'])
                        self.assertTrue(tail['truncated_client_to_server'])
                        self.assertTrue(tail['truncated_server_to_client'])
                        self.assertTrue(all(len(s['data']) == 32768 for s in tail['segments']))
                        self.assertEqual([s['sequence'] for s in tail['segments']], [3, 4, 5, 6])
                        # Packet sizes remain available for large raw datagrams;
                        # raw.bytes follows the engine's existing 16 KiB field bound.
                        candidate['flow_ids'] = [latest['flow_id']]
                        candidate['expression'] = dict(field='packet.size', op='eq', value=dict(type='uint', value=32768))
                        status, preview = request('POST', '/filters/preview', candidate, token)
                        self.assertEqual(status, 200, preview)
                        self.assertEqual(preview['results'][0]['state'], 'partial')
                        self.assertTrue(preview['results'][0]['matched'])
                        # Enforced drops retain observed bytes but no forwarded-byte count.
                        status, listing = request('GET', '/filters', token=token)
                        self.assertEqual(status, 200, listing)
                        dropped_rule = dict(candidate)
                        dropped_rule.pop('flow_ids')
                        dropped_rule['expression'] = dict(field='packet.size', op='eq', value=dict(type='uint', value=0))
                        dropped_rule['expected_revision'] = listing['revision']
                        status, created = request('POST', '/filters', dropped_rule, token)
                        self.assertEqual(status, 202, created)
                        revision = created['rules']['revision']
                        wait(lambda: request('GET', '/system/proxy', token=token)[1]['applied_rules_revision'] == revision)
                        client.send(b'')
                        client.settimeout(.15)
                        with self.assertRaises(socket.timeout):
                            client.recv(65536)
                        known_ids = set(expected) | {latest['flow_id']}
                        dropped = wait(lambda: next((f for f in request('GET', '/traffic/flows?include_blocked=true', token=token)[1]['items'] if f['flow_id'] not in known_ids and f.get('blocked')), None))
                        blocked_history = wait(lambda: history(dropped['flow_id']))
                        self.assertEqual(blocked_history['segments'][0]['data'], [])
                        self.assertEqual(len(blocked_history['segments']), 1)
                        self.assertEqual(blocked_history['datagrams_client_to_server'], 1)
                        self.assertEqual(blocked_history['bytes_client_to_server'], 0)
                        self.assertFalse(blocked_history['history_truncated'])
                        visible = request('GET', '/traffic/flows?include_blocked=false', token=token)[1]['items']
                        self.assertNotIn(dropped['flow_id'], [flow['flow_id'] for flow in visible])
                        # Decoder-empty UDP fallback must support mixed byte predicates
                        # identically during live enforcement and retained replay.
                        utf8_service = dict(service)
                        utf8_service.pop('id')
                        utf8_service.update(protocol_id='utf8', expected_revision=revision)
                        status, updated = request('PATCH', '/services/udp', utf8_service, token)
                        self.assertEqual(status, 202, updated)
                        revision = updated['services']['revision']
                        wait(lambda: request('GET', '/system/proxy', token=token)[1]['applied_services_revision'] == revision)
                        mixed = dict(candidate)
                        mixed.pop('flow_ids')
                        mixed.update(name='mixed byte fields', expected_revision=revision,
                            expression=dict(all=[
                                dict(field='body.size', op='eq', value=dict(type='uint', value=1)),
                                dict(field='packet.size', op='eq', value=dict(type='uint', value=1)),
                            ]))
                        status, created = request('POST', '/filters', mixed, token)
                        self.assertEqual(status, 202, created)
                        revision = created['rules']['revision']
                        wait(lambda: request('GET', '/system/proxy', token=token)[1]['applied_rules_revision'] == revision)
                        client.send(bytes([0xc3]))  # incomplete UTF-8 emits no decoded message
                        with self.assertRaises(socket.timeout):
                            client.recv(65536)
                        known_ids.add(dropped['flow_id'])
                        mixed_flow = wait(lambda: next((f for f in request('GET', '/traffic/flows?include_blocked=true', token=token)[1]['items'] if f['flow_id'] not in known_ids and f.get('blocked')), None))
                        mixed_history = wait(lambda: history(mixed_flow['flow_id']))
                        self.assertEqual(mixed_history['segments'][0]['data'], [0xc3])
                        self.assertEqual(mixed_history['bytes_client_to_server'], 0)
                        mixed.pop('expected_revision')
                        mixed['flow_ids'] = [mixed_flow['flow_id']]
                        status, preview = request('POST', '/filters/preview', mixed, token)
                        self.assertEqual(status, 200, preview)
                        self.assertEqual(preview['results'][0]['state'], 'ready')
                        self.assertTrue(preview['results'][0]['matched'])
                        self.assertTrue(preview['results'][0]['would_block'])
                        # A real favourite keeps its raw zstd archive even after
                        # the ordinary sidecar expires during restart recovery.
                        favorite_path = f'/traffic/flows/{first_id}/favorite'
                        for _ in range(2):
                            status, favorite = request('PUT', favorite_path, {'favorite': True}, token)
                            self.assertEqual(status, 200, favorite)
                            self.assertTrue(favorite['favorite'])
                            self.assertEqual(favorite['raw_history_state'], 'ready')
                        saved_rows = request('GET', '/traffic/flows?favorite=true&include_blocked=true', token=token)[1]['items']
                        self.assertEqual([row['flow_id'] for row in saved_rows], [first_id])
                        self.assertEqual(saved_rows[0]['raw_history_state'], 'ready')
                        archive = root/'history'/'favorites'/f'{first_id}.zst'
                        self.assertEqual(archive.read_bytes()[:4], b'\x28\xb5\x2f\xfd')
                        metrics = request('GET', '/system/proxy', token=token)[1]['service_metrics']
                        self.assertGreaterEqual(next(row for row in metrics if row['service_id'] == 'udp')['accepted_flows'], 2)
                        pp.terminate()
                        pp.wait(timeout=8)
                        metadata_path = root/'history'/f'hydra-history-{first_id}.json'
                        metadata = json.loads(metadata_path.read_text())
                        metadata['closed_at_ms'] = 1
                        metadata_path.write_text(json.dumps(metadata))
                        proxy()
                        wait(lambda: history(first_id))
                        for flow_id, saved in expected.items():
                            self.assertEqual(history(flow_id), saved)
                        self.assertFalse(metadata_path.exists())
                        self.assertTrue(archive.exists())
                        status, favorite = request('PUT', favorite_path, {'favorite': False}, token)
                        self.assertEqual(status, 200, favorite)
                        self.assertFalse(favorite['favorite'])
                        self.assertEqual(request('GET', '/traffic/flows?favorite=true', token=token)[1]['items'], [])
                        self.assertEqual(request('GET', f'/traffic/flows/{first_id}/history', token=token)[0], 404)
                        print('UDP saved datagrams: binary/empty, preview, enforcement, metrics, favourite survives TTL rotation/restart, unpin: OK')
                    finally:
                        for client in clients:
                            client.close()
                        for proc in reversed(processes):
                            if proc.poll() is None:
                                proc.terminate()
                            proc.wait(timeout=10)
        finally:
            stop.set()
            backend.close()


if __name__ == '__main__':
    unittest.main()
