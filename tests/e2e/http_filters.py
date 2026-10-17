"""Authenticated filter lifecycle through real Core REST and Proxy TCP."""
import base64
import hashlib
import http.client
import http.server
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from relay import BIN, ROOT, port


class Filters(unittest.TestCase):
    def test_real_auth_shadow_enforce_toggle_and_http_framing(self):
        seen = []
        class Backend(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def do_GET(self):
                seen.append(self.path)
                data = b'x' * 100_000 if self.path == '/large-response' else ('backend:' + self.path).encode()
                self.send_response(200)
                self.send_header('Content-Length', str(len(data)))
                self.send_header('Content-Type', 'text/plain')
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass
            def do_POST(self):
                seen.append(self.path)
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                self.send_response(200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            def log_message(self, *_):
                pass
        backend = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Backend)
        threading.Thread(target=backend.serve_forever, daemon=True).start()
        processes = []
        with tempfile.TemporaryDirectory(prefix='hydra-http-') as directory:
            tmp = pathlib.Path(directory)
            ap, pp = port(), port()
            sock = str(tmp / 'proxy.sock')
            origin = 'https://hydra.test'
            config = {'socket': sock, 'state_file': str(tmp / 'saved.json'), 'snapshot': {'revision': 0, 'services': []}}
            (tmp / 'proxy.json').write_text(json.dumps(config))
            log = open(tmp / 'logs', 'w+')
            args = [str(BIN / 'hydra-core'), '--database', str(tmp / 'core.db'), '--key', str(tmp / 'jwt.pem'), '--socket', sock]
            def request(method, path, payload=None, token=None, key=None):
                c = http.client.HTTPConnection('127.0.0.1', ap, timeout=5)
                headers = {'Origin': origin}
                if key:
                    headers['Idempotency-Key'] = key
                if token:
                    headers['Authorization'] = 'Bearer ' + token
                if payload is not None:
                    headers['Content-Type'] = 'application/json'
                c.request(method, '/api/v1' + path, json.dumps(payload) if payload is not None else None, headers)
                r = c.getresponse()
                raw = r.read()
                status, hs = r.status, r.getheaders()
                c.close()
                return status, json.loads(raw) if raw else None, hs
            def good(method, path, payload=None, token=None, key=None):
                status, body, _ = request(method, path, payload, token, key)
                self.assertIn(status, [200, 201, 202, 204], (status, body))
                return body
            def wait_revision(rev, token):
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    body = good('GET', '/system/proxy', token=token)
                    if body['applied_services_revision'] == rev and body['applied_rules_revision'] == rev:
                        return
                    time.sleep(.05)
                self.fail(f'revision {rev} not applied: {body}')
            def proxied(path, ua='test-agent'):
                c = http.client.HTTPConnection('127.0.0.1', pp, timeout=3)
                c.request('GET', path, headers={'User-Agent': ua})
                r = c.getresponse()
                status, body = r.status, r.read()
                c.close()
                return status, body
            try:
                for name in ['alice', 'bob']:
                    subprocess.run(args + ['create', 'user', name, 'test-password-1234'], check=True, stdout=log, stderr=log, timeout=10)
                processes.append(subprocess.Popen([str(BIN / 'hydra-proxy'), '--config', str(tmp / 'proxy.json')], stdout=log, stderr=log))
                processes.append(subprocess.Popen(args + ['serve', '--listen', f'127.0.0.1:{ap}', '--origin', origin], stdout=log, stderr=log))
                for _ in range(100):
                    try:
                        status, _, _ = request('GET', '/services')
                        if status == 401:
                            break
                    except (OSError, http.client.HTTPException):
                        pass
                    time.sleep(.05)
                else:
                    log.seek(0)
                    self.fail(log.read())
                self.assertEqual(request('POST', '/auth/register', {})[0], 404)
                login = good('POST', '/auth/login', {'username': 'alice', 'password': 'test-password-1234'})
                alice = login['access_token']
                bob = good('POST', '/auth/login', {'username': 'bob', 'password': 'test-password-1234'})['access_token']
                claims = json.loads(base64.urlsafe_b64decode(alice.split('.')[1] + '=='))
                self.assertEqual(claims['exp'] - claims['iat'], 60)
                service = {'expected_revision': 0, 'transport': 'tcp', 'listen_address': f'127.0.0.1:{pp}', 'upstream_address': f'127.0.0.1:{backend.server_port}', 'protocol_id': 'http', 'enabled': True}
                created = good('POST', '/services', service, alice)
                sid = created['services']['services'][0]['id']
                wait_revision(1, alice)
                self.assertEqual(proxied('/normal'), (200, b'backend:/normal'))
                # Oversized passive response inspection must never truncate the relay.
                self.assertEqual(proxied('/large-response'), (200, b'x' * 100_000))
                for _ in range(100):
                    response_found = False
                    for flow in good('GET', '/traffic/flows', token=alice)['items']:
                        messages = good('GET', f"/traffic/flows/{flow['flow_id']}/messages", token=alice)['items']
                        if any(m['direction'] == 2 and m['fields'].get('http.body_text', {}).get('value') == 'backend:/normal' for m in messages):
                            response_found = True
                            break
                    if response_found: break
                    time.sleep(.05)
                self.assertTrue(response_found, 'HTTP server response was not indexed')
                rule = {'expected_revision': 1, 'service_id': sid, 'name': 'path-and-agent', 'description': 'real test', 'enabled': True, 'mode': 'shadow', 'priority': 10, 'expression': {'all': [{'field': 'http.path', 'op': 'eq', 'value': {'type': 'string', 'value': '/blocked'}}, {'field': 'http.user_agent', 'op': 'contains', 'value': {'type': 'string', 'value': 'test-agent'}}]}, 'action': {'type': 'block', 'http_status': 403}, 'backend': 'userspace', 'expires_at_ms': None}
                made = good('POST', '/filters', rule, alice)
                rid = made['rules']['rules'][0]['id']
                wait_revision(2, bob)
                self.assertEqual(proxied('/blocked')[0], 200)  # shadow
                self.assertEqual(request('PATCH', f'/filters/{rid}', dict(rule, expected_revision=1), bob)[0], 409)
                saved_id = None
                for _ in range(100):
                    flows = good('GET', '/traffic/flows', token=bob)['items']
                    for f in flows:
                        messages = good('GET', f"/traffic/flows/{f['flow_id']}/messages", token=bob)['items']
                        for m in messages:
                            if m['fields'].get('http.path', {}).get('value') == '/blocked':
                                saved_id = m['message_id']
                    if saved_id:
                        break
                    time.sleep(.05)
                self.assertIsNotNone(saved_id)
                tested = good('POST', f'/filters/{rid}/test', {'message_id': saved_id}, bob)
                self.assertTrue(tested['matched'])
                rule.update(expected_revision=2, mode='enforce')
                good('PATCH', f'/filters/{rid}', rule, bob)
                wait_revision(3, alice)
                prior = len(seen)
                self.assertEqual(proxied('/blocked')[0], 403)
                self.assertEqual(len(seen), prior)
                self.assertEqual(proxied('/allowed')[0], 200)
                self.assertEqual(proxied('/blocked', 'other-agent')[0], 200)
                # Complete blocked prefix must be evaluated before malformed fallback.
                with socket.create_connection(('127.0.0.1', pp), timeout=3) as c:
                    c.sendall(b'GET /blocked HTTP/1.1\r\nHost: test\r\nUser-Agent: test-agent\r\n\r\nBAD\r\n\r\n')
                    self.assertIn(b'403', c.recv(1024))
                # A second blocked pipelined request must not inject into response1.
                with socket.create_connection(('127.0.0.1', pp), timeout=3) as c:
                    c.sendall(b'GET /allowed HTTP/1.1\r\nHost: test\r\n\r\nGET /blocked HTTP/1.1\r\nHost: test\r\nUser-Agent: test-agent\r\n\r\n')
                    got = b''
                    try:
                        while part := c.recv(1024):
                            got += part
                    except ConnectionResetError:
                        pass
                    self.assertNotIn(b'403', got)
                # Expect/continue must downgrade transparently instead of deadlocking.
                with socket.create_connection(('127.0.0.1', pp), timeout=3) as c:
                    c.sendall(b'POST /expect HTTP/1.1\r\nHost: test\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n')
                    self.assertIn(b'100 Continue', c.recv(1024))
                    c.sendall(b'data')
                    reply = b''
                    while b'data' not in reply:
                        reply += c.recv(1024)
                    self.assertIn(b'200 OK', reply)
                good('PUT', f'/filters/{rid}/state', {'expected_revision': 3, 'enabled': False}, alice)
                wait_revision(4, bob)
                self.assertEqual(proxied('/blocked')[0], 200)
                listed = good('GET', '/filters', token=bob)['items'][0]
                aid = good('GET', '/auth/me', token=alice)['id']
                bid = good('GET', '/auth/me', token=bob)['id']
                self.assertEqual(listed['created_by'], aid)
                self.assertEqual(listed['updated_by'], aid)
                self.assertTrue(listed['created_at'].endswith('Z'))
                self.assertNotEqual(aid, bid)
                # Parsed flow persistence is part of the same actual traffic path.
                for _ in range(50):
                    flows = good('GET', '/traffic/flows', token=alice)['items']
                    if flows:
                        break
                    time.sleep(.05)
                self.assertTrue(flows)
                sample = os.environ.get('HYDRA_WASM_SAMPLE')
                if sample:
                    raw = socket.socket()
                    raw.bind(('127.0.0.1', 0))
                    raw.listen()
                    raw.settimeout(.2)
                    stop_raw = threading.Event()
                    def echo_raw():
                        while not stop_raw.is_set():
                            try:
                                client, _ = raw.accept()
                            except socket.timeout:
                                continue
                            def handle(client):
                                with client:
                                    while chunk := client.recv(16384):
                                        client.sendall(chunk)
                            threading.Thread(target=handle,args=(client,),daemon=True).start()
                    threading.Thread(target=echo_raw,daemon=True).start()
                    try:
                        rp = port()
                        raw_service = good('POST','/services',dict(service,expected_revision=4,protocol_id='raw',listen_address=f'127.0.0.1:{rp}',upstream_address=f'127.0.0.1:{raw.getsockname()[1]}'),alice)
                        raw_id = next(item['id'] for item in raw_service['services']['services'] if item['id'] != sid)
                        wait_revision(5,alice)
                        def upload(version,body):
                            c=http.client.HTTPConnection('127.0.0.1',ap,timeout=10)
                            c.request('PUT',f'/api/v1/protocols/length-prefixed/{version}',body,{'Authorization':'Bearer '+alice,'X-SHA256':hashlib.sha256(body).hexdigest(),'Content-Type':'application/wasm'})
                            r=c.getresponse();result=json.loads(r.read());self.assertEqual(r.status,202,result);c.close();return result
                        module=pathlib.Path(sample).read_bytes()
                        self.assertEqual(upload('1',module)['state'],'installed')
                        self.assertEqual(upload('1',module)['state'],'installed')
                        c=http.client.HTTPConnection('127.0.0.1',ap,timeout=10)
                        c.request('POST','/api/v1/protocols/length-prefixed/1/activate',json.dumps({'service_id':raw_id}),{'Authorization':'Bearer '+bob,'Content-Type':'application/json','Idempotency-Key':'length-prefixed-activation-test'})
                        r=c.getresponse();result=json.loads(r.read());c.close();self.assertEqual(r.status,202,result);self.assertTrue(result['applied'],result)
                        def frame():
                            with socket.create_connection(('127.0.0.1',rp),timeout=3) as c:
                                c.sendall(b'\x00')
                                self.assertEqual(c.recv(1),b'\x00')
                                c.sendall(b'\x04PING')
                                got=b''
                                while len(got)<5:
                                    got+=c.recv(5-len(got))
                                self.assertEqual(got,b'\x04PING')
                        frame()
                        with socket.create_connection(('127.0.0.1',rp),timeout=3) as sampled:
                            sampled.sendall(b'\x00\x01X');self.assertEqual(sampled.recv(3),b'\x00\x01X')
                            for _ in range(100):
                                candidates=good('GET','/traffic/flows?service_id='+raw_id,token=alice)['items']
                                captured=None
                                for f in reversed(candidates):
                                    status,result,_=request('POST',f"/traffic/flows/{f['flow_id']}/capture",token=alice)
                                    if status==202:captured=result['sample_id'];break
                                if captured:break
                                time.sleep(.02)
                            self.assertTrue(captured)
                            sampled.sendall(b'\x00\x04PING');self.assertEqual(sampled.recv(6),b'\x00\x04PING')
                            for _ in range(100):
                                status,result,_=request('POST','/protocols/length-prefixed/1/test',{'sample_id':captured,'direction':'client_to_server'},alice)
                                if status==200:break
                                self.assertEqual(status,410);time.sleep(.02)
                            self.assertEqual(status,200,result)
                            self.assertEqual(result['messages'][0]['fields']['protocol.payload']['value'],list(b'PING'))
                        decoded=[]
                        for _ in range(100):
                            for f in good('GET','/traffic/flows?service_id='+raw_id,token=alice)['items']:
                                decoded.extend(good('GET',f"/traffic/flows/{f['flow_id']}/messages",token=alice)['items'])
                            if any(m['fields'].get('protocol.payload',{}).get('value')==list(b'PING') for m in decoded):
                                break
                            time.sleep(.05)
                        self.assertTrue(any(m['fields'].get('protocol.payload',{}).get('value')==list(b'PING') for m in decoded),decoded)
                        self.assertEqual(upload('bad',b'invalid wasm')['state'],'error')
                        frame()
                        processes[0].kill();processes[0].wait(timeout=5)
                        processes[0]=subprocess.Popen([str(BIN/'hydra-proxy'),'--config',str(tmp/'proxy.json')],stdout=log,stderr=log)
                        for _ in range(100):
                            result=subprocess.run(args+['status'],capture_output=True,timeout=5)
                            if result.returncode==0:
                                break
                            time.sleep(.05)
                        else:
                            log.seek(0);self.fail(log.read())
                        frame()
                        print('Rust-compiled WASM upload, live activation, typed decoding, malformed module isolation and restart: OK')
                    finally:
                        stop_raw.set();time.sleep(.25);raw.close()
                settings = good('GET', '/system/settings', token=alice)
                self.assertEqual(settings['access_lifetime_seconds'], 60)
                self.assertEqual(settings['refresh_absolute_lifetime_seconds'], 36000)
                exported = good('GET', '/config/export', token=alice)
                self.assertEqual(exported['schema_version'], 1)
                rev = exported['services']['revision']
                good('PUT', '/filters/state', {'expected_revision': rev, 'ids': [rid], 'enabled': False}, bob)
                wait_revision(rev + 1, alice)
                created = good('POST', '/filters/from-message', {'expected_revision': rev + 1, 'message_id': saved_id, 'field': 'http.path', 'name': 'Saved field', 'action': {'type': 'alert'}}, bob)
                self.assertTrue(any(r['name'] == 'Saved field' and r['mode'] == 'enforce' for r in created['rules']['rules']))
                wait_revision(rev + 2, alice)
                profile = good('PUT', '/protocol-profiles/http-headers/1', {'framing': {'kind': 'delimiter', 'delimiter': [13, 10, 13, 10]}, 'max_frame': 8192}, alice)
                self.assertEqual(profile['state'], 'installed')
                activated = good('POST', '/protocols/http-headers/1/activate', {'service_id': sid}, bob, 'native-profile-activation')
                self.assertTrue(activated['applied'])
                self.assertEqual(proxied('/native-profile')[0], 200)
                removed = good('DELETE', '/protocols/activations/' + sid, token=alice, key='native-profile-deactivation')
                self.assertTrue(removed['applied'])
                self.assertTrue(good('DELETE', '/protocols/activations/' + sid, token=alice, key='native-profile-deactivation')['applied'])
                self.assertEqual(proxied('/restored-http')[0], 200)
                # New and edited Enforce rules block immediately, without a
                # saved-message test, including re-enable via the bulk API.
                current = good('GET', '/filters', token=alice)
                instant = dict(rule, expected_revision=current['revision'], name='Immediate enforcement', mode='enforce', enabled=True,
                    expression={'field':'http.path','op':'eq','value':{'type':'string','value':'/instant'}}, action={'type':'block','http_status':403})
                made = good('POST', '/filters', instant, alice)
                instant_id = made['rules']['rules'][-1]['id']
                rev = made['rules']['revision']; wait_revision(rev, alice)
                self.assertEqual(proxied('/instant')[0], 403)
                instant.update(expected_revision=rev, action={'type':'block','http_status':402})
                updated = good('PATCH', '/filters/' + instant_id, instant, alice)
                rev = updated['rules']['revision']; wait_revision(rev, alice)
                self.assertEqual(proxied('/instant')[0], 402)
                good('PUT', '/filters/' + instant_id + '/state', {'expected_revision':rev,'enabled':False}, alice)
                wait_revision(rev + 1, alice)
                self.assertEqual(proxied('/instant')[0], 200)
                good('PUT', '/filters/state', {'expected_revision':rev + 1,'ids':[instant_id],'enabled':True}, alice)
                wait_revision(rev + 2, alice)
                self.assertEqual(proxied('/instant')[0], 402)
                print('Auth, optional tests, immediate enforcement/create/edit/bulk-enable, framing/restore and traffic persistence: OK')
            finally:
                for p in processes:
                    if p.poll() is None:
                        p.terminate()
                    p.wait(timeout=5)
                log.close()
                backend.shutdown()
                backend.server_close()


if __name__ == '__main__':
    unittest.main()
