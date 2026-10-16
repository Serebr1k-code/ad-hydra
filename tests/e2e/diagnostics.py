"""Real parsed/critical telemetry and explicitly requested sample across two processes."""
import hashlib
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

class Diagnostics(unittest.TestCase):
    def test_samples_and_defensive_replay(self):
        listener=socket.socket();listener.bind(('127.0.0.1',0));listener.listen()
        stop=threading.Event()
        def serve():
            listener.settimeout(.1)
            while not stop.is_set():
                try: stream,_=listener.accept()
                except socket.timeout: continue
                def echo(s):
                    with s:
                        while True:
                            data=s.recv(16384)
                            if not data: break
                            s.sendall(data)
                threading.Thread(target=echo,args=(stream,),daemon=True).start()
        threading.Thread(target=serve,daemon=True).start()
        with tempfile.TemporaryDirectory(prefix='hydra-diagnostics-') as directory:
            root=pathlib.Path(directory);lp=port();ap=port();origin='https://hydra.test';log=open(root/'log','w+')
            snapshot={'revision':1,'services':[{'id':'raw','transport':'tcp','listen_address':f'127.0.0.1:{lp}','upstream_address':f'127.0.0.1:{listener.getsockname()[1]}','protocol_id':'raw','enabled':True}]}
            (root/'proxy.json').write_text(json.dumps({'socket':str(root/'proxy.sock'),'state_file':str(root/'state'),'flag_pattern':'FLAG\\{[a-z0-9]{8}\\}','snapshot':snapshot}))
            (root/'seed').write_text(json.dumps(snapshot));(root/'operations').write_text(json.dumps({'flag_pattern':'FLAG\\{[a-z0-9]{8}\\}'}));(root/'operations').chmod(0o600)
            args=[str(BIN/'hydra-core'),'--database',str(root/'db'),'--key',str(root/'key'),'--socket',str(root/'proxy.sock')]
            subprocess.run(args+['create','user','operator','test-password-1234'],check=True,stdout=log,stderr=log,timeout=10)
            processes=[];client=None
            def core():
                p=subprocess.Popen(args+['serve','--operations',str(root/'operations'),'--snapshot',str(root/'seed'),'--listen',f'127.0.0.1:{ap}','--origin',origin],stdout=log,stderr=log);processes.append(p);return p
            def proxy():
                p=subprocess.Popen([str(BIN/'hydra-proxy'),'--config',str(root/'proxy.json')],stdout=log,stderr=log);processes.append(p);return p
            def request(method,path,value=None,token=None):
                headers={'Origin':origin}
                if token:headers['Authorization']='Bearer '+token
                body=None
                if value is not None:body=json.dumps(value);headers['Content-Type']='application/json'
                c=http.client.HTTPConnection('127.0.0.1',ap,timeout=4);c.request(method,'/api/v1'+path,body,headers);r=c.getresponse();data=r.read();status=r.status;c.close();return status,json.loads(data) if data else None
            def wait(action):
                last=None
                for _ in range(150):
                    try:
                        last=action()
                        if last:return last
                    except (OSError,http.client.HTTPException):pass
                    time.sleep(.05)
                log.seek(0);self.fail(f'wait expired: {last}\n{log.read()}')
            def login():
                r=request('POST','/auth/login',{'username':'operator','password':'test-password-1234'})
                return r[1]['access_token'] if r[0]==200 else None
            try:
                pp=proxy();cp=core();token=wait(login)
                client=wait(lambda:socket.create_connection(('127.0.0.1',lp),timeout=2))
                client.sendall(b'initial');self.assertEqual(client.recv(7),b'initial')
                flows=wait(lambda:request('GET','/traffic/flows',token=token)[1]['items'])
                flow=flows[0]['flow_id']
                status,capture=request('POST',f'/traffic/flows/{flow}/capture',token=token);self.assertEqual(status,202)
                payload=bytes(range(256))*2
                client.sendall(payload);received=b''
                while len(received)<len(payload):received+=client.recv(len(payload)-len(received))
                self.assertEqual(received,payload)
                result=wait(lambda:(r if (r:=request('GET','/traffic/samples/'+capture['sample_id'],token=token))[0]==200 else None))
                self.assertEqual(bytes(result[1]['data']),payload)
                self.assertEqual(request('GET','/traffic/samples/00000000-0000-0000-0000-000000000000',token=token)[0],410)
                probe=request('POST','/services/raw/probe',{'transport':'tcp','payload':[0,255,3],'timeout_ms':1000},token)
                self.assertEqual(probe[0],200);self.assertEqual(probe[1]['response'],[0,255,3])
                alias=request('POST','/diagnostics/probe',{'service_id':'raw','transport':'tcp','payload':[3,255,0],'timeout_ms':1000},token)
                self.assertEqual(alias[0],200);self.assertEqual(alias[1]['response'],[3,255,0])
                self.assertEqual(request('POST','/diagnostics/probe',{'service_id':'raw','host':'example.org','transport':'tcp','payload':[],'timeout_ms':1000},token)[0],422)
                self.assertEqual(request('POST','/services/missing/probe',{'transport':'tcp','payload':[],'timeout_ms':1000},token)[0],404)
                # Kill Core first. Transport and the disk spool must remain independent.
                cp.kill();cp.wait(timeout=5)
                for chunk in [b'FLAG{abcd',b'1234}']:
                    client.sendall(chunk);self.assertEqual(client.recv(len(chunk)),chunk);time.sleep(.05)
                def spooled():
                    r=subprocess.run(args+['status'],capture_output=True,text=True,timeout=4)
                    return json.loads(r.stdout)['critical_spool_bytes']>60 if r.returncode==0 else False
                wait(spooled);time.sleep(.2)
                pp.terminate();pp.wait(timeout=5);client.close();client=None
                proxy();core();token=wait(login)
                flags=wait(lambda:request('GET','/flags',token=token)[1])
                self.assertEqual(len(flags),1);self.assertEqual(flags[0]['category'],'own_exposed')
                self.assertEqual(flags[0]['digest'],hashlib.sha256(b'FLAG{abcd1234}').hexdigest())
                self.assertNotIn('FLAG{',json.dumps(flags))
                self.assertEqual(request('GET','/submissions',token=token)[1],[])
                self.assertEqual(request('POST','/flags/'+flags[0]['id']+'/submit',token=token)[0],400)
                self.assertEqual(request('GET','/traffic/samples/'+capture['sample_id'],token=token)[0],410)
                print('On-demand raw sample, configured probe, cross-chunk own_exposed spool replay and restart: OK')
            finally:
                if client:client.close()
                for p in reversed(processes):
                    if p.poll() is None:p.terminate()
                    p.wait(timeout=8)
                log.close();stop.set();listener.close()

if __name__=='__main__':unittest.main()
