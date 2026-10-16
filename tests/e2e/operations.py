"""Real authenticated Farm and checker integration; no sandbox success without prerequisites."""
import http.client
import http.server
import json
import os
import pathlib
import subprocess
import tempfile
import threading
import time
import unittest
from relay import BIN, port

class Operations(unittest.TestCase):
    def test_management_fail_closed_and_persistent_checker(self):
        received=[]
        class Checker(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body=self.rfile.read(int(self.headers.get('Content-Length',0)))
                received.append(body)
                payload=b'{"status":"accepted"}'
                self.send_response(200);self.send_header('Content-Length',str(len(payload)));self.end_headers();self.wfile.write(payload)
            def log_message(self,*_): pass
        checker=http.server.ThreadingHTTPServer(('127.0.0.1',0),Checker)
        threading.Thread(target=checker.serve_forever,daemon=True).start()
        with tempfile.TemporaryDirectory(prefix='hydra-operations-') as directory:
            tmp=pathlib.Path(directory);ap=port();origin='https://hydra.test';log=open(tmp/'log','w+')
            cfg=tmp/'operations.json';cfg.write_text(json.dumps({'flag_pattern':'FLAG\\{[a-z0-9]{8}\\}','auto_submit_captured':False,'checker':{'kind':'http','address':f'127.0.0.1:{checker.server_port}','host':'localhost','path':'/submit','bearer':None}}));cfg.chmod(0o600)
            args=[str(BIN/'hydra-core'),'--database',str(tmp/'db'),'--key',str(tmp/'key'),'--socket',str(tmp/'proxy.sock')]
            subprocess.run(args+['create','user','operator','test-password-1234'],check=True,stdout=log,stderr=log,timeout=10)
            processes=[]
            def request(method,path,value=None,token=None,headers=None,raw=False):
                h={'Origin':origin,**(headers or {})}
                if token:h['Authorization']='Bearer '+token
                if value is not None:h['Content-Type']='application/octet-stream' if raw else 'application/json'
                c=http.client.HTTPConnection('127.0.0.1',ap,timeout=5);c.request(method,'/api/v1'+path,value if raw else json.dumps(value) if value is not None else None,h);r=c.getresponse();body=r.read();status=r.status;c.close();return status,json.loads(body) if body else None
            def good(method,path,value=None,token=None,headers=None,raw=False):
                status,body=request(method,path,value,token,headers,raw);self.assertIn(status,[200,201,202,204],(status,body));return body
            def start():
                p=subprocess.Popen(args+['serve','--listen',f'127.0.0.1:{ap}','--origin',origin,'--operations',str(cfg)],stdout=log,stderr=log);processes.append(p)
                for _ in range(100):
                    try:
                        if request('GET','/farm/targets')[0]==401:return p
                    except (OSError,http.client.HTTPException):pass
                    time.sleep(.05)
                log.seek(0);self.fail(log.read())
            def login():return good('POST','/auth/login',{'username':'operator','password':'test-password-1234'})['access_token']
            try:
                proxy_config=tmp/'proxy.json';proxy_config.write_text(json.dumps({'socket':str(tmp/'proxy.sock'),'state_file':str(tmp/'saved.json'),'snapshot':{'revision':0,'services':[]}}))
                processes.append(subprocess.Popen([str(BIN/'hydra-proxy'),'--config',str(proxy_config)],stdout=log,stderr=log))
                p=start();token=login()
                target={'id':'toy','name':'competition toy','host':'127.0.0.1','port':checker.server_port,'transport':'tcp','enabled':True,'revision':0}
                self.assertEqual(request('PUT','/farm/targets',dict(target,host='example.org'),token)[0],400)
                good('PUT','/farm/targets',target,token)
                exploit={'id':'toy','name':'toy','version':'v1','enabled':True,'revision':0,'manifest':{'runtime':'shell','mode':'per_target','timeout_seconds':1,'memory_mib':32,'cpu_percent':10},'target_ids':['toy'],'interval_seconds':None}
                good('PUT','/farm/exploits/toy/versions/v1',b'#!/bin/sh\necho FLAG{abcdefgh}\n',token,raw=True)
                self.assertEqual(request('PUT','/farm/exploits/toy/versions/v1',b'changed',token,raw=True)[0],400)
                good('PUT','/farm/exploits',exploit,token)
                run=good('POST','/farm/runs',{'exploit_id':'toy','revision':1},token,{'Idempotency-Key':'e2e-one'})['id']
                self.assertEqual(run,good('POST','/farm/runs',{'exploit_id':'toy','revision':1},token,{'Idempotency-Key':'e2e-one'})['id'])
                for _ in range(100):
                    result=good('GET','/farm/runs/'+run,token=token)
                    if result['status'] not in ['queued','running']:break
                    time.sleep(.05)
                self.assertEqual(result['status'],'failed');self.assertIn('sandbox',result['stderr'])
                self.assertEqual(received,[])
                captured=good('POST','/flags',{'value':'FLAG{abcdefgh}','target_id':'toy'},token)['id']
                self.assertNotIn('FLAG{abcdefgh}',json.dumps(good('GET','/flags',token=token)))
                good('POST',f'/flags/{captured}/submit',token=token)
                for _ in range(100):
                    submissions=good('GET','/submissions',token=token)
                    if submissions and submissions[0]['state']=='accepted':break
                    time.sleep(.05)
                self.assertEqual(submissions[0]['state'],'accepted')
                self.assertEqual(json.loads(received[0])['flag'],'FLAG{abcdefgh}')
                self.assertNotIn('FLAG{abcdefgh}',json.dumps(submissions))
                # Restart preserves immutable version, target, failed status and initiating actor.
                p.terminate();p.wait(timeout=10);start();token=login()
                self.assertEqual(good('GET','/farm/runs/'+run,token=token)['status'],'failed')
                self.assertEqual(good('GET','/farm/targets',token=token)['items'][0]['id'],'toy')
                self.assertEqual(good('GET','/submissions',token=token)[0]['state'],'accepted')
                flags=good('GET','/flags',token=token)
                self.assertNotIn('FLAG{abcdefgh}',json.dumps(flags))
                self.assertEqual(flags[0]['id'],captured)
            finally:
                for p in processes:
                    if p.poll() is None:p.terminate()
                for p in processes:
                    try:p.wait(timeout=10)
                    except subprocess.TimeoutExpired:p.kill();p.wait()
                log.close()
        checker.shutdown();checker.server_close()

if __name__=='__main__':unittest.main()
