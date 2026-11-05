"""Real Core + Proxy forwarding, only synthetic flags and loopback fixture targets.

cargo build --locked -p hydra-core -p hydra-proxy
python3 tests/e2e/forwarding.py
"""
import http.client
import http.server
import json
import pathlib
import re
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from relay import BIN, port


class Forwarding(unittest.TestCase):
    def test_cross_connection_restart_http_chunks_and_udp(self):
        observed = []
        saved = []
        class Target(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def log_message(self, *_):
                pass
            def do_POST(self):
                data = self.rfile.read(int(self.headers['Content-Length']))
                observed.append((self.path, data, self.headers['X-Keep']))
                saved[:] = [data]
                self.send_response(200)
                self.send_header('Content-Length', '2')
                self.end_headers()
                self.wfile.write(b'ok')
            def do_GET(self):
                data = b'prefix '+saved[0]+b' suffix'
                self.send_response(200)
                self.send_header('Transfer-Encoding', 'chunked')
                self.send_header('X-Keep', 'reply')
                self.end_headers()
                for chunk in [data[:10], data[10:14], data[14:]]:
                    self.wfile.write(f'{len(chunk):x};kept=yes\r\n'.encode()+chunk+b'\r\n')
                self.wfile.write(b'0\r\nX-Trailer: unchanged\r\n\r\n')
                self.wfile.flush()
        target = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Target)
        threading.Thread(target=target.serve_forever, daemon=True).start()
        udp = socket.socket(type=socket.SOCK_DGRAM)
        udp.bind(('127.0.0.1', 0))
        udp.settimeout(.1)
        seen_udp = []
        stop = threading.Event()
        def udp_target():
            while not stop.is_set():
                try:
                    data, peer = udp.recvfrom(65536)
                    seen_udp.append(data)
                    udp.sendto(data, peer)
                except socket.timeout:
                    continue
                except OSError:
                    break
        threading.Thread(target=udp_target, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory(prefix='hydra-forward-e2e-') as directory:
                root = pathlib.Path(directory)
                api_port, tcp_port, udp_port = port(), port(), port(socket.SOCK_DGRAM)
                origin = 'https://hydra.test'
                services = [dict(id='http', transport='tcp', listen_address=f'127.0.0.1:{tcp_port}', upstream_address=f'127.0.0.1:{port()}', protocol_id='http', enabled=True), dict(id='udp', transport='udp', listen_address=f'127.0.0.1:{udp_port}', upstream_address=f'127.0.0.1:{port(socket.SOCK_DGRAM)}', protocol_id='raw', enabled=True)]
                snapshot = dict(revision=1, services=services)
                config = dict(socket=str(root/'proxy.sock'), state_file=str(root/'state'), snapshot=snapshot, flag_pattern='FLAG[A-Z0-9]{8}', limits=dict(udp_idle_ms=300))
                (root/'proxy.json').write_text(json.dumps(config))
                (root/'seed').write_text(json.dumps(snapshot))
                args = [str(BIN/'hydra-core'), '--database', str(root/'db'), '--key', str(root/'key'), '--socket', str(root/'proxy.sock')]
                processes = []
                with (root/'log').open('w+') as log:
                    subprocess.run(args+['create', 'user', 'operator', 'test-password-1234'], check=True, stdout=log, stderr=log, timeout=15)
                    def start_proxy():
                        process = subprocess.Popen([str(BIN/'hydra-proxy'), '--config', str(root/'proxy.json')], stdout=log, stderr=log)
                        processes.append(process)
                        return process
                    def request(method, path, value=None, token=None):
                        connection = http.client.HTTPConnection('127.0.0.1', api_port, timeout=5)
                        headers = {'Origin': origin}
                        if token:
                            headers['Authorization'] = 'Bearer '+token
                        if value is not None:
                            headers['Content-Type'] = 'application/json'
                        try:
                            connection.request(method, '/api/v1'+path, None if value is None else json.dumps(value), headers)
                            response = connection.getresponse()
                            return response.status, json.loads(response.read())
                        finally:
                            connection.close()
                    def wait(action):
                        for _ in range(200):
                            try:
                                result = action()
                                if result:
                                    return result
                            except (OSError, http.client.HTTPException):
                                pass
                            time.sleep(.05)
                        log.flush()
                        self.fail((root/'log').read_text())
                    try:
                        proxy = start_proxy()
                        core = subprocess.Popen(args+['serve', '--listen', f'127.0.0.1:{api_port}', '--origin', origin, '--snapshot', str(root/'seed')], stdout=log, stderr=log)
                        processes.append(core)
                        auth = wait(lambda: request('POST', '/auth/login', dict(username='operator', password='test-password-1234')))
                        self.assertEqual(auth[0], 200)
                        token = auth[1]['access_token']
                        self.assertEqual(request('PUT', '/farm/teams', dict(id='fixture', name='Loopback fixture', host='127.0.0.1', enabled=True, revision=0), token)[0], 200)
                        revision = 1
                        for service, target_port, framing in [(services[0], target.server_port, dict(type='http1')), (services[1], udp.getsockname()[1], dict(type='datagram'))]:
                            status, body = request('PATCH', '/services/'+service['id'], {**{k:v for k,v in service.items() if k!='id'}, 'expected_revision':revision, 'forwarding':dict(team_id='fixture', target_port=target_port, flag_pattern='FLAG[A-Z0-9]{8}', framing=framing)}, token)
                            self.assertEqual(status, 202, body)
                            revision += 1
                        wait(lambda: request('GET', '/system/proxy', token=token)[1]['applied_services_revision']==revision)
                        flag = b'FLAG12345678'
                        client = http.client.HTTPConnection('127.0.0.1', tcp_port, timeout=5)
                        client.putrequest('POST', '/store')
                        client.putheader('Content-Length', str(len(flag)))
                        client.putheader('X-Keep', 'unchanged')
                        client.endheaders()
                        client.send(flag[:5])
                        client.send(flag[5:])
                        self.assertEqual(client.getresponse().read(), b'ok')
                        client.close()
                        self.assertEqual(observed[0][0], '/store')
                        self.assertEqual(observed[0][2], 'unchanged')
                        self.assertNotEqual(saved[0], flag)
                        self.assertTrue(re.fullmatch(b'FLAG[A-Z0-9]{8}', saved[0]))
                        def get():
                            client = http.client.HTTPConnection('127.0.0.1', tcp_port, timeout=5)
                            try:
                                client.request('GET', '/get')
                                response = client.getresponse()
                                self.assertEqual(response.getheader('X-Keep'), 'reply')
                                self.assertEqual(response.read(), b'prefix '+flag+b' suffix')
                            finally:
                                client.close()
                        get()
                        for data in [b'', b'\x00'+flag+b'\xff', b'\x00'+flag+b'\xff']:
                            with socket.socket(type=socket.SOCK_DGRAM) as client:
                                client.settimeout(3)
                                client.sendto(data, ('127.0.0.1', udp_port))
                                self.assertEqual(client.recv(65536), data)
                        self.assertEqual(seen_udp[0], b'')
                        self.assertEqual(seen_udp[1], seen_udp[2])
                        self.assertNotEqual(seen_udp[1], b'\x00'+flag+b'\xff')
                        self.assertEqual(seen_udp[1][:1], b'\x00')
                        self.assertEqual(seen_udp[1][-1:], b'\xff')
                        old_epoch=request('GET', '/system/proxy', token=token)[1]['proxy_epoch_id']
                        proxy.terminate()
                        proxy.wait(timeout=10)
                        proxy = start_proxy()
                        wait(lambda: (lambda view:view['connected'] and view['proxy_epoch_id']!=old_epoch)(request('GET', '/system/proxy', token=token)[1]))
                        wait(lambda: request('GET', '/system/proxy', token=token)[1]['applied_services_revision']==revision)
                        get()
                        status = request('GET', '/system/proxy', token=token)[1]
                        self.assertEqual(status['forwarding_stats']['mappings'], 2)
                        self.assertEqual(request('GET', '/flags', token=token), (200, []))
                        ticks = __import__('os').sysconf('SC_CLK_TCK')
                        def cpu(process):
                            fields=pathlib.Path(f'/proc/{process.pid}/stat').read_text().split()
                            return (int(fields[13])+int(fields[14]))/ticks
                        before=cpu(proxy);started=time.monotonic()
                        with socket.socket(type=socket.SOCK_DGRAM) as client:
                            client.settimeout(3)
                            for _ in range(1000):
                                client.sendto(b'\x00'+flag+b'\xff',('127.0.0.1',udp_port))
                                self.assertEqual(client.recv(65536),b'\x00'+flag+b'\xff')
                        elapsed=time.monotonic()-started
                        rss=next(line for line in pathlib.Path(f'/proc/{proxy.pid}/status').read_text().splitlines() if line.startswith('VmRSS:'))
                        print(f'Loopback fixture: 1000 mirrored UDP datagrams in {elapsed:.3f}s; Proxy CPU {cpu(proxy)-before:.3f}s; {rss}')
                        print('Forwarding: HTTP chunk scatter + separate connections/restart + UDP binary/empty datagrams + no flag submission: OK')
                    finally:
                        for process in reversed(processes):
                            if process.poll() is None:
                                process.terminate()
                                process.wait(timeout=10)
        finally:
            stop.set()
            udp.close()
            target.shutdown()
            target.server_close()

if __name__ == '__main__':
    unittest.main()
