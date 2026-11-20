"""Exercise the standalone installer archive/download path without host mutations."""
import contextlib
import hashlib
import hmac
import http.server
import io
import json
import os
import re
import select
import pathlib
import pty
import ssl
import subprocess
import tarfile
import tempfile
import threading
import time
import unittest
import urllib.parse
import zipfile

ROOT=pathlib.Path(__file__).resolve().parents[2]
SCRIPT=ROOT/'scripts/install-hydra.sh'
CORE='hydra-core-linux-x86_64-musl'
PROXY='hydra-proxy-linux-x86_64-musl'


def tar_bundle(extra=None,corrupt=False):
    header=b'\x7fELF\x02\x01'+b'\0'*12+b'\x3e\0'
    files={CORE:header+b'core archive fixture',PROXY:header+b'proxy archive fixture'}
    manifest=''.join(hashlib.sha256(value).hexdigest()+'  '+name+'\n' for name,value in files.items()).encode()
    if corrupt:files[CORE]+=b'corrupted'
    files['SHA256SUMS']=manifest
    output=io.BytesIO()
    with tarfile.open(fileobj=output,mode='w:gz') as archive:
        for name,value in files.items():
            entry=tarfile.TarInfo('./'+name);entry.size=len(value);archive.addfile(entry,io.BytesIO(value))
        if extra:
            entry=tarfile.TarInfo(extra[0]);entry.type=extra[1];entry.linkname='/etc/passwd';archive.addfile(entry)
    return output.getvalue()


class Installer(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='hydra-install-test-');self.addCleanup(self.temp.cleanup)
        self.directory=pathlib.Path(self.temp.name)
    def execute(self,source=None,environment=None,target=None,stdin=None,install_source=None):
        arguments=['bash',str(SCRIPT),'--download-only',str(target or self.directory/'output')]
        if install_source is not None: arguments+=['--source',install_source]
        if source is not None:
            path=self.directory/'input';path.write_bytes(source);arguments+=['--archive',str(path)]
        env={key:value for key,value in os.environ.items() if key not in {'HYDRA_DOWNLOAD_URL','HYDRA_ARCHIVE_SHA256','HYDRA_GITHUB_TOKEN','GH_TOKEN','GITHUB_TOKEN'}}
        env.update(environment or {})
        return subprocess.run(arguments,env=env,input=stdin,capture_output=True,text=True,timeout=30)
    def test_tarball_and_outer_checksum(self):
        source=tar_bundle()
        result=self.execute(source,{'HYDRA_ARCHIVE_SHA256':hashlib.sha256(source).hexdigest()})
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((self.directory/'output'/CORE).stat().st_mode&0o777,0o755)
        self.assertEqual((self.directory/'output').stat().st_mode&0o777,0o700)
    def test_actions_zip(self):
        output=io.BytesIO()
        with zipfile.ZipFile(output,'w') as archive:archive.writestr('hydra-linux-x86_64-musl.tar.gz',tar_bundle())
        result=self.execute(output.getvalue());self.assertEqual(result.returncode,0,result.stderr)
    def test_corrupt_binary_and_wrong_archive_hash_are_rejected(self):
        for source,env in [(tar_bundle(corrupt=True),{}),(tar_bundle(),{'HYDRA_ARCHIVE_SHA256':'0'*64})]:
            with self.subTest(environment=bool(env)):
                result=self.execute(source,env);self.assertNotEqual(result.returncode,0)
                self.assertFalse((self.directory/'output').exists())
    def test_traversal_symlink_and_unexpected_files_are_rejected(self):
        for path,kind in [('../escape',tarfile.REGTYPE),('/tmp/hydra-installer-escape',tarfile.REGTYPE),('linked',tarfile.SYMTYPE),('linked',tarfile.LNKTYPE),('extra',tarfile.REGTYPE)]:
            with self.subTest(path=path,kind=kind):
                result=self.execute(tar_bundle((path,kind)));self.assertNotEqual(result.returncode,0)
                self.assertFalse((self.directory/'output').exists())
    def test_existing_output_is_preserved(self):
        target=self.directory/'output';target.mkdir();(target/'keep').write_text('user data')
        result=self.execute(tar_bundle());self.assertNotEqual(result.returncode,0)
        self.assertEqual((target/'keep').read_text(),'user data')
    def test_https_and_explicit_github_artifact_download_and_redirect_policy(self):
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-keyout',str(self.directory/'key'),'-out',str(self.directory/'cert'),'-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1'],check=True,capture_output=True)
        output=io.BytesIO()
        with zipfile.ZipFile(output,'w') as archive:archive.writestr('hydra-linux-x86_64-musl.tar.gz',tar_bundle())
        package=output.getvalue();authorization={}
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(inner):
                authorization[inner.path]=inner.headers.get('Authorization')
                if inner.path=='/index':
                    payload=json.dumps({'artifacts':[{'name':'hydra-linux-x86_64-musl-'+'a'*40,'expired':False,'created_at':'2026-10-19T00:00:00Z','workflow_run':{'head_branch':'main'},'digest':'sha256:'+hashlib.sha256(package).hexdigest(),'archive_download_url':'https://localhost:%d/artifact'%server.server_port}]}).encode()
                elif inner.path=='/downgrade':
                    inner.send_response(302);inner.send_header('Location','http://localhost:%d/archive'%server.server_port);inner.end_headers();return
                elif inner.path=='/artifact':
                    inner.send_response(302);inner.send_header('Location','https://localhost:%d/archive'%server.server_port);inner.end_headers();return
                else:payload=package
                inner.send_response(200);inner.send_header('Content-Length',str(len(payload)));inner.end_headers();inner.wfile.write(payload)
            def log_message(self,*args):pass
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(self.directory/'cert',self.directory/'key')
        server.socket=context.wrap_socket(server.socket,server_side=True)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            env={'HYDRA_DOWNLOAD_URL':'https://localhost:%d/index'%server.server_port,'SSL_CERT_FILE':str(self.directory/'cert')}
            result=self.execute(environment=env);self.assertEqual(result.returncode,0,result.stderr)
            gh_token='fixture-actions-read-token'
            github_env={**env,'HYDRA_GITHUB_ARTIFACTS_URL':env['HYDRA_DOWNLOAD_URL']}
            result=self.execute(environment=github_env,target=self.directory/'github-output',stdin=gh_token+'\n',install_source='github-auth')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(authorization['/index'],'Bearer '+gh_token)
            self.assertEqual(authorization['/artifact'],'Bearer '+gh_token)
            self.assertIsNone(authorization['/archive'])
            self.assertIn('Verified build extracted to:',result.stdout)
            self.assertNotIn(gh_token,result.stdout+result.stderr)
            env['HYDRA_DOWNLOAD_URL']='https://localhost:%d/downgrade'%server.server_port
            result=self.execute(environment=env,target=self.directory/'insecure-output');self.assertNotEqual(result.returncode,0)
            self.assertFalse((self.directory/'insecure-output').exists())
        finally:server.shutdown();server.server_close();thread.join()

    def test_public_release_and_terminal_colors(self):
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1',
                        '-keyout',str(self.directory/'key'),'-out',str(self.directory/'cert'),
                        '-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1'],
                       check=True,capture_output=True)
        package=tar_bundle()
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(inner):
                if inner.path=='/missing':
                    inner.send_error(404);return
                payload=package if inner.path=='/archive' else json.dumps({
                    'tag_name':'hydra-latest',
                    'assets':[{'name':'hydra-linux-x86_64-musl.tar.gz',
                        'browser_download_url':f'https://localhost:{server.server_port}/archive',
                        'digest':'sha256:'+hashlib.sha256(package).hexdigest()}],
                }).encode()
                inner.send_response(200);inner.send_header('Content-Length',str(len(payload)));inner.end_headers();inner.wfile.write(payload)
            def log_message(self,*args):pass
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(self.directory/'cert',self.directory/'key')
        server.socket=context.wrap_socket(server.socket,server_side=True)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            # Run the actual public path against our HTTPS release fixture.
            script=self.directory/'public-installer.sh'
            script.write_text(SCRIPT.read_text().replace(
                "index='https://api.github.com/repos/'+repository+'/releases/tags/hydra-latest'",
                f"index='https://localhost:{server.server_port}/release'"))
            env={**os.environ,'SSL_CERT_FILE':str(self.directory/'cert'),'TERM':'xterm-256color'}
            env.pop('NO_COLOR',None)
            args=['bash',str(script),'--source','github','--download-only',str(self.directory/'public')]
            result=subprocess.run(args,env=env,capture_output=True,text=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertNotIn('\x1b[',result.stdout+result.stderr)
            self.assertIn('Both Hydra binary checksums verified',result.stdout)

            master,slave=pty.openpty()
            try:
                args[-1]=str(self.directory/'terminal')
                process=subprocess.Popen(args,env=env,stdout=slave,stderr=slave,stdin=subprocess.DEVNULL)
                os.close(slave);slave=None
                output=b''
                while process.poll() is None or select.select([master],[],[],.05)[0]:
                    if select.select([master],[],[],.1)[0]:
                        try: output+=os.read(master,65536)
                        except OSError: break
                self.assertEqual(process.wait(timeout=5),0,output)
                self.assertIn(b'\x1b[32m',output)
            finally:
                if slave is not None:os.close(slave)
                os.close(master)
            env['NO_COLOR']='1'
            args[-1]=str(self.directory/'no-color')
            result=subprocess.run(args,env=env,capture_output=True,text=True,timeout=30)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertNotIn('\x1b[',result.stdout+result.stderr)
            script.write_text(script.read_text().replace('/release', '/missing'))
            args[-1]=str(self.directory/'missing')
            result=subprocess.run(args,env=env,capture_output=True,text=True,timeout=30)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('No public Hydra install release',result.stderr)
        finally:
            server.shutdown();server.server_close();thread.join()

    @contextlib.contextmanager
    def s3_fixture(self,behavior='ok',token='temporary-session-token'):
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-keyout',str(self.directory/'s3-key'),'-out',str(self.directory/'s3-cert'),'-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1'],check=True,capture_output=True)
        requests=[];key='INSTALLER_FIXTURE_KEY';secret='installer-fixture-secret';region='ru-test-1';commit='a'*40
        header=b'\x7fELF\x02\x01'+b'\0'*12+b'\x3e\0'
        payloads={'hydra-core':header+b'core over signed HTTPS','hydra-proxy':header+b'proxy over signed HTTPS'}
        prefix='/api%20files' if behavior=='prefix' else ''
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(inner):
                requests.append(inner.path)
                authorization=inner.headers.get('Authorization','')
                match=re.fullmatch(r'AWS4-HMAC-SHA256 Credential=([^/]+)/([0-9]{8})/([^/]+)/s3/aws4_request, ?SignedHeaders=([^,]+), ?Signature=([0-9a-f]{64})',authorization)
                valid=False
                if match:
                    fields=match[4].split(';');canonical=''.join(name+':'+ ' '.join(inner.headers.get(name,'').split())+'\n' for name in fields)
                    request='GET\n'+inner.path+'\n\n'+canonical+'\n'+match[4]+'\n'+hashlib.sha256(b'').hexdigest()
                    scope=match[2]+'/'+region+'/s3/aws4_request'
                    message='AWS4-HMAC-SHA256\n'+inner.headers.get('X-Amz-Date','')+'\n'+scope+'\n'+hashlib.sha256(request.encode()).hexdigest()
                    signing=('AWS4'+secret).encode()
                    for piece in [match[2],region,'s3','aws4_request']:signing=hmac.digest(signing,piece.encode(),'sha256')
                    expected=hmac.new(signing,message.encode(),'sha256').hexdigest()
                    valid=(match[1]==key and match[3]==region and hmac.compare_digest(expected,match[5]) and inner.headers.get('X-Amz-Security-Token','')==token)
                if behavior=='denied' or not valid:
                    inner.send_response(403);inner.end_headers();inner.wfile.write((key+' '+secret+' '+token).encode());return
                if behavior=='redirect':
                    inner.send_response(302);inner.send_header('Location','https://localhost:%d/redirected'%server.server_port);inner.end_headers();return
                name=inner.path.rsplit('/',1)[-1]
                if inner.path!=prefix+'/example-hydra-bucket/'+name or name not in payloads:
                    inner.send_response(404);inner.end_headers();return
                payload=payloads[name]
                if behavior=='invalid-elf':payload=b'not an executable'
                inner.send_response(200)
                inner.send_header('Content-Length',str(65*1024*1024 if behavior=='oversize' else len(payload)))
                if behavior!='missing-metadata':
                    inner.send_header('X-Amz-Meta-Sha256','0'*64 if behavior=='corrupt' else hashlib.sha256(payload).hexdigest())
                    inner.send_header('X-Amz-Meta-Commit','b'*40 if behavior=='mixed' and name=='hydra-proxy' else commit)
                    inner.send_header('X-Amz-Meta-Target','x86_64-unknown-linux-musl')
                inner.end_headers()
                try:inner.wfile.write(payload)
                except (BrokenPipeError,ConnectionResetError):pass
            def log_message(self,*args):pass
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(self.directory/'s3-cert',self.directory/'s3-key')
        server.socket=context.wrap_socket(server.socket,server_side=True)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            endpoint='https://localhost:%d%s'%(server.server_port,prefix)
            yield '\n'.join([endpoint,region,'example-hydra-bucket',key,secret,token])+'\n',{'SSL_CERT_FILE':str(self.directory/'s3-cert')},requests,payloads
        finally:server.shutdown();server.server_close();thread.join()
    def test_s3_signed_download_from_stdin_and_session_token(self):
        for behavior,token in [('ok','temporary-session-token'),('prefix','')]:
            with self.subTest(behavior=behavior),self.s3_fixture(behavior,token) as (stdin,env,requests,payloads):
                target=self.directory/behavior
                result=self.execute(environment=env,target=target,stdin=stdin)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(len(requests),2)
                self.assertEqual((target/CORE).read_bytes(),payloads['hydra-core'])
                self.assertEqual((target/PROXY).read_bytes(),payloads['hydra-proxy'])
                self.assertIn('commit='+'a'*40,(target/'BUILD.txt').read_text())
                self.assertEqual((target/CORE).stat().st_mode&0o777,0o755)
                for value in ['INSTALLER_FIXTURE_KEY','installer-fixture-secret','temporary-session-token']:
                    self.assertNotIn(value,result.stdout+result.stderr)
    def test_s3_terminal_prompts_hide_credentials(self):
        with self.s3_fixture() as (stdin,env,requests,_):
            environment={key:value for key,value in os.environ.items() if key not in {'HYDRA_DOWNLOAD_URL','HYDRA_ARCHIVE_SHA256','HYDRA_GITHUB_TOKEN','GH_TOKEN','GITHUB_TOKEN'}}|env
            master,slave=pty.openpty();transcript=b''
            process=subprocess.Popen(['bash',str(SCRIPT),'--download-only',str(self.directory/'terminal-output')],stdin=slave,stdout=slave,stderr=slave,env=environment)
            os.close(slave)
            try:
                for label,value in zip(['endpoint URL','region','bucket','access key ID','secret access key','session token (optional; Enter to skip)'],stdin.splitlines()):
                    deadline=time.monotonic()+10
                    while (label+': ').encode() not in transcript:
                        self.assertLess(time.monotonic(),deadline,'Prompt did not arrive: '+label)
                        if select.select([master],[],[],.1)[0]:transcript+=os.read(master,65536)
                    os.write(master,value.encode()+b'\n')
                deadline=time.monotonic()+10
                while process.poll() is None:
                    self.assertLess(time.monotonic(),deadline,'Terminal installer did not finish')
                    if select.select([master],[],[],.1)[0]:
                        try:transcript+=os.read(master,65536)
                        except OSError:break
                self.assertEqual(process.wait(timeout=3),0,transcript.decode(errors='replace'))
                for value in stdin.splitlines()[3:]:self.assertNotIn(value.encode(),transcript)
                self.assertEqual(len(requests),2)
            finally:
                if process.poll() is None:process.kill();process.wait()
                os.close(master)

    def test_s3_corrupt_mixed_missing_oversize_and_invalid_binaries_are_rejected(self):
        for behavior in ['corrupt','mixed','missing-metadata','oversize','invalid-elf']:
            with self.subTest(behavior=behavior),self.s3_fixture(behavior) as (stdin,env,requests,_):
                target=self.directory/behavior
                result=self.execute(environment=env,target=target,stdin=stdin)
                self.assertNotEqual(result.returncode,0,result.stdout)
                self.assertFalse(target.exists())
    def test_s3_redirect_and_denied_response_do_not_expose_credentials(self):
        for behavior in ['redirect','denied']:
            with self.subTest(behavior=behavior),self.s3_fixture(behavior) as (stdin,env,requests,_):
                result=self.execute(environment=env,stdin=stdin)
                self.assertNotEqual(result.returncode,0)
                self.assertEqual(len(requests),1)
                for value in ['INSTALLER_FIXTURE_KEY','installer-fixture-secret','temporary-session-token']:
                    self.assertNotIn(value,result.stdout+result.stderr)
    def test_s3_incomplete_stdin_and_insecure_endpoint_fail_before_network(self):
        for stdin in ['','https://localhost/\n','http://localhost/\nru-test-1\nexample-hydra-bucket\nKEY\nsecret\n\n']:
            with self.subTest(stdin_length=len(stdin)):
                result=self.execute(stdin=stdin)
                self.assertNotEqual(result.returncode,0)
                self.assertFalse((self.directory/'output').exists())


if __name__=='__main__':unittest.main()
