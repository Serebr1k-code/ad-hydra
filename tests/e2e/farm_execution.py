"""Opt-in full Farm -> isolated exploit -> flag -> checker acceptance."""
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

@unittest.skipUnless(os.getenv('HYDRA_SANDBOX_CGROUP') and os.getenv('HYDRA_SANDBOX_TARGET'),'run through tests/sandbox/run-linux.py with delegated controllers')
class FarmExecution(unittest.TestCase):
    def test_isolated_capture_and_submission(self):
        received=[]
        class Checker(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                received.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
                self.send_response(200);self.send_header('Content-Length','0');self.end_headers()
            def log_message(self,*_):pass
        checker=http.server.ThreadingHTTPServer(('127.0.0.1',0),Checker)
        threading.Thread(target=checker.serve_forever,daemon=True).start()
        with tempfile.TemporaryDirectory(prefix='hyjob-') as runtime, tempfile.TemporaryDirectory(prefix='hydra-farm-e2e-') as directory:
            root=pathlib.Path(directory);ap=port();origin='https://hydra.test';log=open(root/'log','w+')
            host,target_port=os.environ['HYDRA_SANDBOX_TARGET'].rsplit(':',1)
            settings={'flag_pattern':'FLAG\\{[a-z0-9]{8}\\}','auto_submit_captured':True,'checker':{'kind':'http','address':f'127.0.0.1:{checker.server_port}','host':'localhost','path':'/submit','bearer':'checker-secret'},'sandbox':{'delegated_cgroup':os.environ['HYDRA_SANDBOX_CGROUP'],'runtime_dir':runtime,'bwrap':'/usr/bin/bwrap','ip':'/usr/bin/ip'}}
            (root/'ops').write_text(json.dumps(settings));(root/'ops').chmod(0o600)
            args=[str(BIN/'hydra-core'),'--database',str(root/'db'),'--key',str(root/'key'),'--socket',str(root/'proxy.sock')]
            subprocess.run(args+['create','user','operator','test-password-1234'],stdout=log,stderr=log,check=True,timeout=10)
            core=subprocess.Popen(args+['serve','--operations',str(root/'ops'),'--listen',f'127.0.0.1:{ap}','--origin',origin],stdout=log,stderr=log)
            def request(method,path,value=None,token=None,raw=False,headers=None):
                h={'Origin':origin,**(headers or {})}
                if token:h['Authorization']='Bearer '+token
                if value is not None:h['Content-Type']='application/octet-stream' if raw else 'application/json'
                c=http.client.HTTPConnection('127.0.0.1',ap,timeout=5);c.request(method,'/api/v1'+path,value if raw else json.dumps(value) if value is not None else None,h);r=c.getresponse();data=r.read();status=r.status;c.close();return status,json.loads(data) if data else None
            def wait(action):
                for _ in range(200):
                    try:
                        result=action()
                        if result:return result
                    except (OSError,http.client.HTTPException):pass
                    time.sleep(.05)
                log.seek(0);self.fail(log.read())
            try:
                def login():
                    status,result=request('POST','/auth/login',{'username':'operator','password':'test-password-1234'})
                    return result['access_token'] if status==200 else None
                token=wait(login)
                self.assertEqual(request('PUT','/farm/targets',{'id':'toy','name':'Owned echo','host':host,'port':int(target_port),'transport':'tcp','enabled':True,'revision':0},token)[0],200)
                artifact=b"import os,socket\nassert 'checker-secret' not in str(os.environ)\ns=socket.create_connection((os.environ['TARGET_HOST'],int(os.environ['TARGET_PORT'])),3)\ns.sendall(b'FLAG{abcd1234}')\nprint(s.recv(256).decode())\n"
                self.assertEqual(request('PUT','/farm/exploits/toy/versions/v1',artifact,token,True)[0],200)
                exploit={'id':'toy','name':'Echo extractor','version':'v1','enabled':True,'revision':0,'manifest':{'runtime':'python','mode':'per_target','timeout_seconds':10,'memory_mib':128,'cpu_percent':50},'target_ids':['toy'],'interval_seconds':None}
                self.assertEqual(request('PUT','/farm/exploits',exploit,token)[0],200)
                status,run=request('POST','/farm/runs',{'exploit_id':'toy','revision':1},token,headers={'Idempotency-Key':'sandbox-e2e'})
                self.assertEqual(status,202);run_id=run['id']
                def finished():
                    status,result=request('GET','/farm/runs/'+run_id,token=token)
                    return result if status==200 and result['status'] not in ['queued','running'] else None
                result=wait(finished);self.assertEqual(result['status'],'succeeded',result)
                submissions=wait(lambda:(items if (items:=request('GET','/submissions',token=token)[1]) and items[0]['state']=='accepted' else None))
                flags=request('GET','/flags',token=token)[1]
                self.assertEqual(flags[0]['category'],'captured');self.assertEqual(flags[0]['target_id'],'toy');self.assertEqual(flags[0]['run_id'],run_id)
                self.assertEqual(received[0]['flag'],'FLAG{abcd1234}');self.assertEqual(received[0]['idempotency_key'],submissions[0]['id'])
                self.assertNotIn('FLAG{',json.dumps(flags));self.assertNotIn('checker-secret',json.dumps(result))
                print('Real isolated Farm execution -> captured flag provenance -> persistent queue -> local checker: OK')
            finally:
                if core.poll() is None:core.terminate()
                core.wait(timeout=10);log.close();checker.shutdown();checker.server_close()

if __name__=='__main__':unittest.main()
