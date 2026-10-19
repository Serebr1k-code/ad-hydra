"""Exercise the standalone installer archive/download path without host mutations."""
import hashlib
import http.server
import io
import json
import os
import pathlib
import ssl
import subprocess
import tarfile
import tempfile
import threading
import unittest
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
    def execute(self,source=None,environment=None,target=None):
        arguments=['bash',str(SCRIPT),'--download-only',str(target or self.directory/'output')]
        if source is not None:
            path=self.directory/'input';path.write_bytes(source);arguments+=['--archive',str(path)]
        env={key:value for key,value in os.environ.items() if key not in {'HYDRA_DOWNLOAD_URL','HYDRA_ARCHIVE_SHA256','HYDRA_GITHUB_TOKEN','GH_TOKEN','GITHUB_TOKEN'}}
        env.update(environment or {})
        return subprocess.run(arguments,env=env,capture_output=True,text=True,timeout=30)
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
    def test_https_index_download_and_redirect_policy(self):
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','1','-keyout',str(self.directory/'key'),'-out',str(self.directory/'cert'),'-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1'],check=True,capture_output=True)
        output=io.BytesIO()
        with zipfile.ZipFile(output,'w') as archive:archive.writestr('hydra-linux-x86_64-musl.tar.gz',tar_bundle())
        package=output.getvalue()
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(inner):
                if inner.path=='/index':
                    payload=json.dumps({'artifacts':[{'name':'hydra-linux-x86_64-musl-'+'a'*40,'expired':False,'created_at':'2026-10-19T00:00:00Z','workflow_run':{'head_branch':'main'},'digest':'sha256:'+hashlib.sha256(package).hexdigest(),'archive_download_url':'https://localhost:%d/archive'%server.server_port}]}).encode()
                elif inner.path=='/downgrade':
                    inner.send_response(302);inner.send_header('Location','http://localhost:%d/archive'%server.server_port);inner.end_headers();return
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
            env['HYDRA_DOWNLOAD_URL']='https://localhost:%d/downgrade'%server.server_port
            result=self.execute(environment=env,target=self.directory/'insecure-output');self.assertNotEqual(result.returncode,0)
            self.assertFalse((self.directory/'insecure-output').exists())
        finally:server.shutdown();server.server_close();thread.join()


if __name__=='__main__':unittest.main()
