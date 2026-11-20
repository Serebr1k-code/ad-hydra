"""Real uv, synthetic flag arguments and localhost APIs; no checker or exploit traffic."""
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


class FlagControls(unittest.TestCase):
    def test_verbatim_manual_count_auto_dispatch_and_service_pagination(self):
        uv = shutil.which('uv')
        self.assertIsNotNone(uv, 'real uv is required')
        with tempfile.TemporaryDirectory(prefix='hydra-flag-controls-') as directory:
            root = pathlib.Path(directory)
            origin = 'https://localhost:' + str(port())
            address = origin.rsplit(':', 1)[1]
            operations = root / 'operations.json'
            operations.write_text(json.dumps({'execution_mode':'host', 'host_runtime':{'uv':uv,'work_dir':str(root/'runtime')},'flag_pattern':r'FLAG\{[^}]+\}'}))
            operations.chmod(0o600)
            (root/'proxy.json').write_text(json.dumps({'socket':str(root/'proxy.sock'),'state_file':str(root/'snapshot.json')}))
            args=[str(BIN/'hydra-core'),'--database',str(root/'db'),'--key',str(root/'key'),'--socket',str(root/'proxy.sock')]
            subprocess.run(args+['create','user','test-user','local-test-password'],check=True,capture_output=True)
            # Seed an old defensive entry before Flags opens/migrates the database.
            with sqlite3.connect(root/'db') as db:
                db.execute("INSERT INTO flags VALUES('old-own','digest-own','own_exposed','FLAG{own}',NULL,NULL,'proxy',0)")
                db.execute("INSERT INTO flag_events(event_id,flag_id,flow_id,service_id) VALUES('old-detection','old-own','flow','docs')")
            log=open(root/'process.log','w+')
            processes=[]
            token=None
            def api(path, body=None, method=None):
                headers={'Origin':origin,'Content-Type':'application/json'}
                if token:headers['Authorization']='Bearer '+token
                request=urllib.request.Request('http://127.0.0.1:'+address+'/api/v1'+path, None if body is None else json.dumps(body).encode(), headers, method=method)
                try:
                    with urllib.request.urlopen(request,timeout=10) as response:
                        return json.load(response) if response.status!=204 else None
                except urllib.error.HTTPError as error:
                    detail=error.read(512).decode('utf-8','replace')
                    error.close()
                    raise AssertionError(f'{method or ("POST" if body is not None else "GET")} {path} returned HTTP {error.code}: {detail}') from error
            def wait(fetch, predicate):
                until=time.monotonic()+20
                while time.monotonic()<until:
                    result=fetch()
                    if predicate(result):return result
                    time.sleep(.1)
                self.fail('condition timed out: '+str(result))
            try:
                processes.append(subprocess.Popen([str(BIN/'hydra-proxy'),'--config',str(root/'proxy.json')],stdout=log,stderr=log))
                processes.append(subprocess.Popen(args+['serve','--listen','127.0.0.1:'+address,'--origin',origin,'--operations',str(operations)],stdout=log,stderr=log))
                for _ in range(150):
                    try:
                        token=api('/auth/login',{'username':'test-user','password':'local-test-password'})['access_token'];break
                    except urllib.error.URLError:time.sleep(.05)
                self.assertIsNotNone(token)
                self.assertEqual(api('/flags/page')['total'],0)
                recorded=root/'received.jsonl'
                source='import json,sys\nwith open('+repr(str(recorded))+',"a",encoding="utf-8") as f: f.write(json.dumps(sys.argv[1:],ensure_ascii=False)+"\\n")\n'
                config=api('/submitter')['config']
                config.update(mode='custom',enabled=False,source=source,flag_pattern=r'FLAG\{[^}]+\}',python_packages=[],batch_size=1)
                api('/submitter',config,'PUT')
                for service in ['docs','ark']:
                    api('/farm/services',{'id':service,'name':service.title(),'port':12345},'PUT')
                expected=['FLAG{AbC+/==}','FLAG{ spaced-Å }','FLAG{third}']
                result=api('/flags/import',{'text':'noise '+expected[0]+' '+expected[1]+' '+expected[0], 'service_id':'docs','target_id':None})
                self.assertEqual((result['matched'],result['added'],result['queued']),(3,2,2))
                api('/flags/import',{'text':expected[2],'service_id':'ark','target_id':None})
                with sqlite3.connect(root/'db') as db:
                    self.assertEqual([row[0] for row in db.execute('SELECT value FROM flags ORDER BY rowid')],expected)
                    self.assertEqual(db.execute("SELECT count(*) FROM flags WHERE category='own_exposed'").fetchone()[0],0)
                    self.assertEqual(db.execute("SELECT flag_id FROM flag_events WHERE event_id='old-detection'").fetchone()[0],None)
                first=api('/flags/page?sort=service&order=asc&limit=1')
                self.assertEqual(first['items'][0]['value'],expected[2])
                self.assertNotIn('value',api('/flags')[0])
                self.assertEqual([item['value'] for item in api('/flags/page?sort=value&order=asc')['items']],sorted(expected))
                self.assertEqual(api('/flags/page?search=AbC')['items'][0]['value'],expected[0])
                self.assertEqual(first['total'],3);self.assertEqual(first['items'][0]['service_id'],'ark')
                self.assertEqual(api('/flags/page?sort=service&order=asc&limit=1&offset=1')['items'][0]['service_id'],'docs')
                self.assertEqual(api('/flags/page?service_id=docs')['total'],2)
                self.assertFalse(recorded.exists())
                request=api('/submitter/run',{'count':2})
                done=wait(lambda:api('/submitter'),lambda row:row['manual_request'] and row['manual_request']['id']==request['id'] and row['manual_request']['status']=='finished')
                self.assertEqual(done['manual_request']['flag_count'],2);self.assertEqual(done['queued_flags'],1)
                self.assertFalse(done['config']['enabled'])
                self.assertEqual([flag for line in recorded.read_text().splitlines() for flag in json.loads(line)],expected[:2])
                config=api('/submitter')['config'];config['enabled']=True
                started=time.monotonic();api('/submitter',config,'PUT')
                wait(lambda:api('/submitter'),lambda row:row['queued_flags']==0)
                self.assertLess(time.monotonic()-started,20)
                self.assertEqual([flag for line in recorded.read_text().splitlines() for flag in json.loads(line)],expected)
                with sqlite3.connect(root/'db') as db:
                    self.assertEqual(db.execute('SELECT max(flag_count) FROM flag_submitter_batches').fetchone()[0],1)
                    self.assertEqual(db.execute("SELECT count(*) FROM submissions WHERE state='accepted'").fetchone()[0],3)
                config=api('/submitter')['config'];config['enabled']=False
                api('/submitter',config,'PUT')
                failed=api('/flags/page?search=AbC')['items'][0]
                with sqlite3.connect(root/'db') as db:
                    db.execute("UPDATE submissions SET state='failed',attempts=10,next_attempt=9999999999 WHERE flag_id=?",[failed['id']])
                api('/flags/import',{'text':expected[0]})
                with sqlite3.connect(root/'db') as db:
                    self.assertEqual(db.execute('SELECT state,attempts FROM submissions WHERE flag_id=?',[failed['id']]).fetchone(),('failed',10))
                api('/flags/'+failed['id']+'/submit',{},'POST')
                with sqlite3.connect(root/'db') as db:
                    self.assertEqual(db.execute('SELECT state,attempts FROM submissions WHERE flag_id=?',[failed['id']]).fetchone(),('queued',0))
                config=api('/submitter')['config'];config['enabled']=True
                api('/submitter',config,'PUT')
                wait(lambda:api('/submitter'),lambda row:row['queued_flags']==0)
                self.assertEqual([flag for line in recorded.read_text().splitlines() for flag in json.loads(line)],expected+[expected[0]])
                print('Failed flag resubmission, verbatim uv argv, manual count while paused, per-call limit, immediate auto dispatch, migration purge and sorted service pagination: OK')
            finally:
                for process in reversed(processes):
                    if process.poll() is None:process.terminate()
                    process.wait(timeout=10)
                log.close()


if __name__=='__main__':unittest.main()
