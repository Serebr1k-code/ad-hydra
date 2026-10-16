"""Headless Chromium smoke over Core's native HTTPS; standard library only."""
import base64
import http.client
import json
import os
import pathlib
import shutil
import socket
import ssl
import struct
import subprocess
import tempfile
import time
import unittest
from relay import BIN, port


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


class Browser(unittest.TestCase):
    @unittest.skipUnless(shutil.which('chromium') or shutil.which('google-chrome'), 'Chromium is required')
    def test_native_https_login_navigation_logout(self):
        chrome = shutil.which('chromium') or shutil.which('google-chrome')
        processes = []
        with tempfile.TemporaryDirectory(prefix='hydra-browser-') as directory:
            tmp = pathlib.Path(directory)
            ap, debug = port(), port()
            sock = str(tmp / 'proxy.sock')
            (tmp / 'proxy.json').write_text(json.dumps({'socket': sock, 'state_file': str(tmp / 'saved.json')}))
            subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(tmp/'tls.key'),'-out',str(tmp/'tls.crt'),'-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1','-days','1'],check=True,capture_output=True)
            args = [str(BIN/'hydra-core'),'--database',str(tmp/'db'),'--key',str(tmp/'jwt.pem'),'--socket',sock]
            log = open(tmp/'process.log','w+')
            cdp=None
            try:
                subprocess.run(args+['create','user','alice','test-password-1234'],check=True,stdout=log,stderr=log,timeout=10)
                processes.append(subprocess.Popen([str(BIN/'hydra-proxy'),'--config',str(tmp/'proxy.json')],stdout=log,stderr=log))
                processes.append(subprocess.Popen(args+['serve','--listen',f'127.0.0.1:{ap}','--origin',f'https://localhost:{ap}','--tls-cert',str(tmp/'tls.crt'),'--tls-key',str(tmp/'tls.key')],stdout=log,stderr=log))
                for _ in range(100):
                    try:
                        c=http.client.HTTPSConnection('127.0.0.1',ap,context=ssl._create_unverified_context(),timeout=2)
                        c.request('GET','/');r=c.getresponse();r.read();c.close()
                        if r.status==200:
                            break
                    except (OSError,http.client.HTTPException):
                        pass
                    time.sleep(.05)
                else:
                    log.seek(0);self.fail(log.read())
                processes.append(subprocess.Popen([chrome,'--headless','--no-sandbox','--disable-gpu','--disable-background-networking','--disable-component-update','--no-first-run','--ignore-certificate-errors',f'--user-data-dir={tmp / "chrome"}',f'--remote-debugging-port={debug}','about:blank'],stdout=log,stderr=log))
                for _ in range(100):
                    try:
                        c=http.client.HTTPConnection('127.0.0.1',debug,timeout=2);c.request('GET','/json/list');targets=json.loads(c.getresponse().read());c.close()
                        target=next(t for t in targets if t['type']=='page');break
                    except (OSError,StopIteration,http.client.HTTPException):
                        time.sleep(.05)
                else:
                    log.seek(0);self.fail(log.read())
                cdp=CDP(target['webSocketDebuggerUrl'])
                cdp.call('Page.navigate',{'url':f'https://localhost:{ap}/'})
                def wait(expression):
                    for _ in range(100):
                        if cdp.evaluate(expression):
                            return
                        time.sleep(.05)
                    self.fail(cdp.evaluate('document.body.innerText'))
                wait("document.querySelector('#login') && !document.querySelector('#login').hidden")
                cdp.evaluate("document.querySelector('[name=username]').value='alice';document.querySelector('[name=password]').value='test-password-1234';document.querySelector('#login-form').requestSubmit()")
                wait("!document.querySelector('#console').hidden")
                original=cdp.evaluate('performance.timeOrigin')
                cdp.call('Page.reload')
                wait(f"performance.timeOrigin!=={original} && document.querySelector('#console') && !document.querySelector('#console').hidden")
                for section in ['Filters','Traffic','Protocols','Farm','Flags','Settings','Users / audit']:
                    cdp.evaluate(f"[...document.querySelectorAll('nav button')].find(b=>b.textContent==={json.dumps(section)}).click()")
                    expected = {'Users / audit':'Users and audit','Farm':'Exploit farm','Flags':'Flag Vault'}.get(section,section)
                    wait(f"document.querySelector('#page h1')?.textContent==={json.dumps(expected)}")
                    self.assertFalse(cdp.evaluate("document.querySelector('#error').textContent"))
                self.assertIn('alice',cdp.evaluate("document.querySelector('#page').innerText"))
                cdp.evaluate("document.querySelector('#logout').click()")
                wait("!document.querySelector('#login').hidden")
                self.assertNotIn('hydra_csrf=',cdp.evaluate('document.cookie'))
                cdp.socket.close()
                print('Native HTTPS and real Chromium login, navigation, API calls, logout: OK')
            finally:
                if cdp:
                    cdp.socket.close()
                for p in reversed(processes):
                    if p.poll() is None:
                        p.terminate()
                    p.wait(timeout=10)
                log.close()


if __name__ == '__main__':
    unittest.main()
