"""Headless Chromium smoke over Core's native HTTPS; standard library only."""
import base64
import http.client
import json
import os
import pathlib
import re
import shutil
import socket
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import unittest
from relay import BIN, port


def rename_dist_preserving_parent_mtime(source, destination):
    # This smoke runs with no concurrent frontend changes. Preserve only the
    # directory timestamp changed by our temporary asset-availability probe.
    assert source.parent == destination.parent
    parent = source.parent
    before = parent.stat()
    source.rename(destination)
    os.utime(parent, ns=(before.st_atime_ns, before.st_mtime_ns))


class CDP:
    def __init__(self, address):
        from urllib.parse import urlsplit
        url = urlsplit(address)
        self.socket = socket.create_connection((url.hostname, url.port), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        self.socket.sendall(f'GET {url.path} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode())
        headers = b''
        while not headers.endswith(b'\r\n\r\n'):
            headers += self.socket.recv(1)
        assert b'101' in headers, headers
        self.sequence = 0
    def read(self, count):
        value = b''
        while len(value) < count:
            chunk = self.socket.recv(count - len(value))
            if not chunk:
                raise EOFError('browser disconnected')
            value += chunk
        return value
    def call(self, method, params=None):
        self.sequence += 1
        payload = json.dumps({'id': self.sequence, 'method': method, 'params': params or {}}).encode()
        n = len(payload)
        header = bytes([0x81, 0x80 | n]) if n < 126 else bytes([0x81, 0x80 | 126]) + struct.pack('!H', n)
        mask = os.urandom(4)
        self.socket.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))
        while True:
            start = self.read(2)
            length = start[1] & 127
            if length == 126:
                length = struct.unpack('!H', self.read(2))[0]
            elif length == 127:
                length = struct.unpack('!Q', self.read(8))[0]
            assert length < 4 * 1024 * 1024
            result = json.loads(self.read(length))
            if result.get('id') == self.sequence:
                assert 'error' not in result, result
                return result.get('result', {})
    def evaluate(self, source):
        result = self.call('Runtime.evaluate', {'expression': source, 'returnByValue': True, 'awaitPromise': True})
        assert 'exceptionDetails' not in result, result
        return result.get('result', {}).get('value')
    def click(self, expression):
        point=self.evaluate(f'(()=>{{const e=({expression});e.scrollIntoView({{block:"center"}});const r=e.getBoundingClientRect();return {{x:r.x+r.width/2,y:r.y+r.height/2}}}})()')
        for kind in ['mousePressed','mouseReleased']:
            self.call('Input.dispatchMouseEvent',{'type':kind,**point,'button':'left','clickCount':1})


class Browser(unittest.TestCase):
    @unittest.skipUnless(shutil.which('chromium') or shutil.which('google-chrome'), 'Chromium is required')
    def test_native_https_login_navigation_logout(self):
        chrome = shutil.which('chromium') or shutil.which('google-chrome')
        processes = []
        upstream = None
        with tempfile.TemporaryDirectory(prefix='hydra-browser-') as directory:
            tmp = pathlib.Path(directory)
            ap, debug = port(), port()
            sock = str(tmp / 'proxy.sock')
            (tmp / 'proxy.json').write_text(json.dumps({'socket': sock, 'state_file': str(tmp / 'saved.json'), 'flag_pattern':'FLAG[0-9]{6}'}))
            subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(tmp/'tls.key'),'-out',str(tmp/'tls.crt'),'-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1','-days','1'],check=True,capture_output=True)
            (tmp/'operations.json').write_text(json.dumps({'flag_pattern':'FLAG[0-9]{6}','auto_submit_captured':False}))
            (tmp/'operations.json').chmod(0o600)
            args = [str(BIN/'hydra-core'),'--database',str(tmp/'db'),'--key',str(tmp/'jwt.pem'),'--socket',sock]
            log = open(tmp/'process.log','w+')
            cdp=None
            dist = pathlib.Path(__file__).resolve().parents[2] / "frontend" / "dist"
            hidden_dist = None
            try:
                subprocess.run(args+['create','user','alice','test-password-1234'],check=True,stdout=log,stderr=log,timeout=10)
                processes.append(subprocess.Popen([str(BIN/'hydra-proxy'),'--config',str(tmp/'proxy.json')],stdout=log,stderr=log))
                if os.environ.get('HYDRA_VERIFY_EMBEDDED') == '1':
                    self.assertTrue(dist.is_dir(), 'Build frontend/dist before this smoke test')
                    hidden_dist = dist.with_name(f'dist.browser-hidden-{os.getpid()}')
                    rename_dist_preserving_parent_mtime(dist, hidden_dist)
                processes.append(subprocess.Popen(args+['serve','--listen',f'127.0.0.1:{ap}','--origin',f'https://localhost:{ap}','--tls-cert',str(tmp/'tls.crt'),'--tls-key',str(tmp/'tls.key'),'--operations',str(tmp/'operations.json')],stdout=log,stderr=log))
                for _ in range(100):
                    try:
                        c=http.client.HTTPSConnection('127.0.0.1',ap,context=ssl._create_unverified_context(),timeout=2)
                        c.request('GET','/');r=c.getresponse();index=r.read().decode();c.close()
                        if r.status==200:
                            break
                    except (OSError,http.client.HTTPException):
                        pass
                    time.sleep(.05)
                else:
                    log.seek(0);self.fail(log.read())
                scripts = re.findall(r'<script[^>]+src="([^"]+)"', index)
                self.assertTrue(scripts, 'Core must serve the Vite compiled module')
                for script in scripts:
                    self.assertRegex(script, r'^/assets/[^/]+-[A-Za-z0-9_-]+\.js$')
                    c=http.client.HTTPSConnection('127.0.0.1',ap,context=ssl._create_unverified_context(),timeout=2)
                    c.request('GET',script);r=c.getresponse();bundle=r.read();c.close()
                    self.assertEqual(r.status,200)
                    self.assertIn('javascript',r.getheader('Content-Type'))
                    self.assertEqual(r.getheader('X-Content-Type-Options'),'nosniff')
                    self.assertIn('immutable',r.getheader('Cache-Control'))
                    self.assertGreater(len(bundle),1000)
                processes.append(subprocess.Popen([chrome,'--headless','--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--disable-background-networking','--disable-component-update','--no-first-run','--ignore-certificate-errors',f'--user-data-dir={tmp / "chrome"}',f'--remote-debugging-port={debug}','about:blank'],stdout=log,stderr=log))
                # GitHub's shared hosted runners can take several seconds to start Chrome.
                for _ in range(300):
                    try:
                        c=http.client.HTTPConnection('127.0.0.1',debug,timeout=2);c.request('GET','/json/list');targets=json.loads(c.getresponse().read());c.close()
                        target=next(t for t in targets if t['type']=='page');break
                    except (OSError,StopIteration,http.client.HTTPException):
                        time.sleep(.05)
                else:
                    log.seek(0);self.fail(log.read())
                cdp=CDP(target['webSocketDebuggerUrl'])
                cdp.call('Emulation.setDeviceMetricsOverride',{'width':1280,'height':900,'deviceScaleFactor':1,'mobile':False})
                cdp.call('Page.navigate',{'url':f'https://localhost:{ap}/'})
                def wait(expression):
                    for _ in range(100):
                        if cdp.evaluate(expression):
                            return
                        time.sleep(.05)
                    self.fail(cdp.evaluate("'Controls: '+JSON.stringify([...document.querySelectorAll('[role=combobox]')].map(e=>e.outerHTML))+'\\n'+document.body.innerText+'\\nTextarea DOM: '+[...document.querySelectorAll('textarea')].map(e=>e.outerHTML).join('\\n')"))
                wait("document.querySelector('#login') && !document.querySelector('#login').hidden")
                self.assertTrue(cdp.evaluate("document.querySelector('#app')?.hasAttribute('data-v-app')"), "Vue must mount the application")
                cdp.evaluate("for (const [name,value] of [['username','alice'],['password','test-password-1234']]) { const input=document.querySelector(`[name=${name}]`);input.value=value;input.dispatchEvent(new Event('input',{bubbles:true})); }")
                cdp.evaluate("document.querySelector('#login-form').requestSubmit()")
                wait("document.querySelector('#console') && !document.querySelector('#console').hidden")
                wait("Boolean(document.querySelector('#traffic-sessions') && document.querySelector('#traffic-service'))")
                self.assertEqual(cdp.evaluate('location.hash'), '#/traffic')
                original=cdp.evaluate('performance.timeOrigin')
                cdp.call('Page.reload')
                wait(f"performance.timeOrigin!=={original} && document.querySelector('#console') && !document.querySelector('#console').hidden")
                # Real Vue bindings and mutations, with a loopback-only service and no workers.
                cdp.evaluate("[...document.querySelectorAll('nav button')].find(b=>b.textContent==='Services').click()")
                wait("[...document.querySelectorAll('#page button')].some(b=>b.textContent==='Add service' && !b.disabled)")
                cdp.evaluate("[...document.querySelectorAll('#page button')].find(b=>b.textContent==='Add service').click()")
                wait("document.querySelector('#service-form') !== null")
                upstream=socket.socket();upstream.bind(('127.0.0.1',0));upstream.listen();upstream.settimeout(10)
                release_response=threading.Event()
                def reply():
                    try:
                        connection,_=upstream.accept()
                        with connection:
                            connection.settimeout(3);data=b''
                            while b'\r\n\r\n' not in data and len(data)<8192:
                                chunk=connection.recv(4096)
                                if not chunk:return
                                data+=chunk
                            release_response.wait(5)
                            connection.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nX-Reply: visible\r\nConnection: close\r\n\r\nOK')
                    except OSError:
                        pass
                threading.Thread(target=reply,daemon=True).start()
                service = {'transport':'tcp','listen_address':f'127.0.0.1:{port()}','upstream_address':f'127.0.0.1:{upstream.getsockname()[1]}','protocol_id':'http','enabled':True}
                def fill_service(value):
                    for field,key in [('service-listener','listen_address'),('service-backend','upstream_address'),('service-profile','protocol_id')]:
                        cdp.evaluate(f"{{const e=document.querySelector('#{field}');e.value={json.dumps(value[key])};e.dispatchEvent(new Event('input',{{bubbles:true}}));}}")
                    cdp.evaluate("document.querySelector('#service-form').requestSubmit()")
                    wait(f"document.querySelector('#page').innerText.includes({json.dumps(value['listen_address'])}) && !document.querySelector('#service-form')")
                fill_service(service)
                # A second real configured service has no sessions, to verify dropdown filtering.
                cdp.evaluate("[...document.querySelectorAll('#page button')].find(b=>b.textContent==='Add service').click()")
                wait("document.querySelector('#service-form') !== null")
                other={**service,'listen_address':f'127.0.0.1:{port()}'}
                fill_service(other)
                cdp.evaluate("[...document.querySelectorAll('nav button')].find(b=>b.textContent==='Traffic').click()")
                wait("document.querySelector('#traffic-live-status')?.textContent==='Live'")
                for _ in range(100):
                    try:
                        connection=socket.create_connection(('127.0.0.1',int(service['listen_address'].split(':')[1])),timeout=2)
                        break
                    except OSError:time.sleep(.05)
                else:self.fail('Fixture listener did not apply')
                with connection:
                    connection.sendall(b'GET /browser-stream HTTP/1.1\r\nHost: browser-fixture\r\nX-Fixture-Flag: FLAG123456\r\nX-Visible: one\r\nX-Visible: two\r\nConnection: close\r\n\r\n')
                    wait("Boolean(document.querySelector('#traffic-stream [data-direction=\"1\"]'))")
                    release_response.set()
                    self.assertIn(b'200 OK',connection.recv(4096))
                wait("Boolean(document.querySelector('#traffic-sessions [data-flow-id]') && document.querySelector('#traffic-stream [data-message-id]'))")
                self.assertIn('/browser-stream',cdp.evaluate("document.querySelector('#traffic-stream').innerText"))
                wait("Boolean(document.querySelector('#traffic-sessions [data-has-flag=\"true\"]'))")
                self.assertIn('Flag',cdp.evaluate("document.querySelector('#traffic-sessions [data-has-flag=\"true\"]').innerText"))
                self.assertTrue(cdp.evaluate("document.querySelector('#traffic-sessions [data-flow-id]').style.borderLeftColor.length>0"))
                self.assertEqual(cdp.evaluate("document.querySelector('#traffic-show-blocked').getAttribute('aria-checked')"), 'false')
                self.assertIn('X-Visible: one',cdp.evaluate("document.querySelector('#traffic-stream').innerText"))
                self.assertIn('X-Visible: two',cdp.evaluate("document.querySelector('#traffic-stream').innerText"))
                self.assertTrue(cdp.evaluate("document.querySelector('#traffic-stream [data-direction=\"1\"]')!==null"))
                wait("Boolean(document.querySelector('#traffic-stream [data-direction=\"2\"]'))")
                self.assertIn('http.status_code: 200',cdp.evaluate("document.querySelector('#traffic-stream [data-direction=\"2\"]').innerText"))
                self.assertIn('X-Reply: visible',cdp.evaluate("document.querySelector('#traffic-stream').innerText"))
                self.assertIn('http.body_text: OK',cdp.evaluate("document.querySelector('#traffic-stream [data-direction=\"2\"]').innerText"))
                cdp.evaluate("document.querySelector('#traffic-sessions [data-flow-id]').click()")
                wait("document.querySelector('#traffic-sessions [aria-pressed=true]')!==null")
                self.assertTrue(cdp.evaluate("(()=>{const l=document.querySelector('#traffic-sessions').getBoundingClientRect(),r=document.querySelector('#traffic-stream').getBoundingClientRect();return r.left>=l.right-1})()"))
                if os.environ.get('HYDRA_SCREENSHOT'):
                    shot=cdp.call('Page.captureScreenshot',{'format':'png'})
                    pathlib.Path(os.environ['HYDRA_SCREENSHOT']).write_bytes(base64.b64decode(shot['data']))
                service_id=cdp.evaluate("document.querySelector('#traffic-sessions [data-service-id]').dataset.serviceId")
                cdp.click("document.querySelector('#traffic-service')")
                wait("document.querySelectorAll('[role=option]').length===3")
                cdp.click(f"[...document.querySelectorAll('[role=option]')].find(e=>e.textContent.trim()!=='All services'&&e.textContent.trim()!=={json.dumps(service_id)})")
                wait("document.querySelector('#traffic-sessions').innerText.includes('No sessions for this service yet.')")
                self.assertFalse(cdp.evaluate("document.querySelector('#traffic-stream').innerText.includes('/browser-stream')"))
                cdp.click("document.querySelector('#traffic-service')")
                wait("document.querySelectorAll('[role=option]').length===3")
                cdp.click(f"[...document.querySelectorAll('[role=option]')].find(e=>e.textContent.trim()==={json.dumps(service_id)})")
                wait("document.querySelector('#traffic-stream').innerText.includes('/browser-stream')")
                for section in ['Filters','Traffic','Protocols','Farm','Flags','Settings','Users / audit']:
                    cdp.evaluate(f"[...document.querySelectorAll('nav button')].find(b=>b.textContent==={json.dumps(section)}).click()")
                    expected = {'Users / audit':'Users and audit','Farm':'Exploit farm','Flags':'Flag Manager'}.get(section,section)
                    wait(f"document.querySelector('#page h1')?.textContent==={json.dumps(expected)}")
                    wait("!document.querySelector('#page').innerText.includes('Loading')")
                    self.assertFalse(cdp.evaluate("document.querySelector('#error').textContent"))
                    self.assertFalse(cdp.evaluate("[...document.querySelectorAll('#page [role=alert]')].some(e=>e.textContent.trim())"),cdp.evaluate("document.querySelector('#page').innerText"))
                    if section == 'Filters':
                        cdp.evaluate("[...document.querySelectorAll('#page button')].find(b=>b.textContent==='Add filter').click()")
                        wait("Boolean(document.querySelector('[role=dialog]')?.innerText.includes('Match conditions'))")
                        self.assertFalse(cdp.evaluate("Boolean(document.querySelector('[role=dialog] textarea:not(#filter-description)'))"), 'Filter editor must not request an expression JSON textarea')
                        self.assertTrue(cdp.evaluate("[...document.querySelectorAll('[role=dialog] button')].some(b=>b.textContent==='Add group')"))
                        cdp.evaluate("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent==='Add group').click()")
                        wait("document.querySelectorAll('[role=dialog] [aria-label=\"Group match mode\"]').length===2")
                        self.assertIn('No traffic test is required', cdp.evaluate("document.querySelector('[role=dialog]').innerText"))
                        # Remove the root condition, then its only remaining group.
                        # This must edit the expression without submitting the form.
                        cdp.evaluate("document.querySelector('[role=dialog] [aria-label=\"Remove rule\"]').click()")
                        wait("document.querySelectorAll('[role=dialog] [aria-label=\"Remove rule\"]').length===1")
                        cdp.evaluate("document.querySelector('[role=dialog] [aria-label=\"Remove group\"]').click()")
                        wait("document.querySelectorAll('[role=dialog] [aria-label=\"Group match mode\"]').length===1")
                        self.assertEqual(cdp.evaluate("document.querySelectorAll('[role=dialog] [aria-label=\"Remove rule\"]').length"), 1)
                        self.assertFalse(cdp.evaluate("Boolean(document.querySelector('[role=dialog] [aria-label=\"Remove group\"]'))"))
                        cdp.evaluate("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent==='Cancel').click()")
                        wait("!document.querySelector('[role=dialog]')")
                        # Default creation and editing enforce directly: no
                        # staging in Shadow and no test-message selection.
                        cdp.evaluate("[...document.querySelectorAll('#page button')].find(b=>b.textContent==='Add filter').click()")
                        wait("Boolean(document.querySelector('#filter-mode'))")
                        self.assertIn('Enforce', cdp.evaluate("document.querySelector('#filter-mode').textContent"))
                        cdp.click("document.querySelector('[aria-label=\"Filter service\"]')")
                        wait("document.querySelectorAll('[role=option]').length===2")
                        cdp.click(f"[...document.querySelectorAll('[role=option]')].find(e=>e.textContent.trim()==={json.dumps(service_id)})")
                        cdp.click("document.querySelector('[aria-label=\"Filter action\"]')")
                        wait("document.querySelectorAll('[role=option]').length>2")
                        cdp.click("[...document.querySelectorAll('[role=option]')].find(e=>e.textContent.trim()==='Block HTTP request')")
                        cdp.evaluate("document.querySelector('[role=dialog] form').requestSubmit()")
                        wait("!document.querySelector('[role=dialog]') && document.querySelector('#page').innerText.includes('Enforce — applies action to matches')")
                        cdp.evaluate("[...document.querySelectorAll('#page button')].find(b=>b.textContent==='Edit').click()")
                        wait("Boolean(document.querySelector('#filter-status'))")
                        cdp.evaluate("{ const e = document.querySelector('#filter-status'); e.value='402'; e.dispatchEvent(new Event('input',{bubbles:true})); }")
                        cdp.evaluate("document.querySelector('[role=dialog] form').requestSubmit()")
                        wait("!document.querySelector('[role=dialog]') && document.querySelector('#page').innerText.includes('Enforce — applies action to matches')")
                        # Saved-traffic testing is still an optional diagnostic.
                        wait("[...document.querySelectorAll('#page button')].some(b=>b.textContent==='Test saved message'&&!b.disabled)")
                        cdp.evaluate("[...document.querySelectorAll('#page button')].find(b=>b.textContent==='Test saved message').click()")
                        wait("Boolean(document.querySelector('[role=dialog] [aria-label=\"Test message\"]'))")
                        cdp.evaluate("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent==='Test only').click()")
                        wait("document.querySelector('[role=dialog]').innerText.includes('Matched.')")
                        cdp.evaluate("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent==='Close').click()")
                        wait("!document.querySelector('[role=dialog]')")
                        self.assertFalse(cdp.evaluate("[...document.querySelectorAll('#page [role=alert]')].some(e=>e.textContent.trim())"))
                        # A real blocked HTTP flow is absent by default and
                        # appears only when the operator requests blocked traffic.
                        with socket.create_connection(('127.0.0.1',int(service['listen_address'].split(':')[1])),timeout=3) as blocked:
                            blocked.sendall(b'GET /blocked-browser HTTP/1.1\r\nHost: browser-fixture\r\nConnection: close\r\n\r\n')
                            self.assertIn(b'402',blocked.recv(4096))
                        cdp.evaluate("[...document.querySelectorAll('nav button')].find(b=>b.textContent==='Traffic').click()")
                        wait("document.querySelector('#traffic-live-status')?.textContent==='Live' && Boolean(document.querySelector('#traffic-sessions [data-flow-id]'))")
                        self.assertFalse(cdp.evaluate("Boolean(document.querySelector('#traffic-sessions [data-blocked=\"true\"]'))"))
                        cdp.click("document.querySelector('#traffic-show-blocked')")
                        wait("Boolean(document.querySelector('#traffic-sessions [data-blocked=\"true\"]'))")
                        cdp.click("document.querySelector('#traffic-show-blocked')")
                        wait("!document.querySelector('#traffic-sessions [data-blocked=\"true\"]')")
                        wait("Boolean(document.querySelector('#traffic-sessions [data-has-flag=\"true\"]'))")
                self.assertIn('alice',cdp.evaluate("document.querySelector('#page').innerText"))
                cdp.evaluate("document.querySelector('#logout').click()")
                wait("document.querySelector('#login') && !document.querySelector('#login').hidden")
                self.assertNotIn('hydra_csrf=',cdp.evaluate('document.cookie'))
                cdp.socket.close()
                print('Embedded Vue, native HTTPS and real Chromium login, navigation, API calls, logout: OK')
            finally:
                if upstream:upstream.close()
                if cdp:
                    cdp.socket.close()
                try:
                    for p in reversed(processes):
                        if p.poll() is None:
                            p.terminate()
                        p.wait(timeout=10)
                finally:
                    if hidden_dist is not None:
                        rename_dist_preserving_parent_mtime(hidden_dist, dist)
                    log.close()


if __name__ == '__main__':
    unittest.main()
