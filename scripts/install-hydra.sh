#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Normal mode prompts for S3 configuration on stdin; no Git or AWS CLI required.
# Optional legacy HTTPS archive/index mode (not used for S3).
DOWNLOAD_URL="${HYDRA_DOWNLOAD_URL:-}"
ARCHIVE_SHA256="${HYDRA_ARCHIVE_SHA256:-}" # Optional trusted checksum of the downloaded archive.
INITIAL_USER="${HYDRA_INITIAL_USER:-hydra-admin}"
INSTALL_UV="${HYDRA_INSTALL_UV:-true}"
UV_URL="https://github.com/astral-sh/uv/releases/download/0.10.9/uv-x86_64-unknown-linux-musl.tar.gz"
UV_SHA256="433e56874739e92c7cfd661ba9e5f287b376ca612c08c8194a41a98a13158aea"
# Actions downloads use HYDRA_GITHUB_TOKEN, GH_TOKEN or GITHUB_TOKEN from the environment.

mode=install
archive=""
output=""
while (($#)); do
  case "$1" in
    --archive) [[ $# -ge 2 ]] || { echo '--archive needs a path' >&2; exit 2; }; archive=$2; shift 2 ;;
    --download-only) [[ $# -ge 2 ]] || { echo '--download-only needs a new directory' >&2; exit 2; }; mode=download; output=$2; shift 2 ;;
    -h|--help)
      cat <<'HELP'
Usage: sudo bash install-hydra.sh [--archive BUILD.tar.gz|BUILD.zip]
       bash install-hydra.sh --download-only NEW_DIRECTORY [--archive BUILD.tar.gz|BUILD.zip]
Default: enter S3 endpoint, region, bucket, access key ID, secret key and optional
session token on stdin (one line each). Credential input is hidden on terminals.
Downloads use curl; missing prerequisites are installed automatically as root.
The S3 key needs GetObject access to hydra-core and hydra-proxy. No Git/AWS CLI.
--archive retains offline installation; HYDRA_DOWNLOAD_URL selects legacy HTTPS mode.
Installs on x86_64 Linux with systemd; no Rust, Node or Docker is needed.
Initial credentials are written to /root/hydra-install-credentials.json (0600).
Existing users, configuration, certificates and database are preserved.
HELP
      exit 0 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ "$mode" == install ]]; then
  [[ $EUID == 0 ]] || { echo 'Run installation as root (sudo bash install-hydra.sh).' >&2; exit 1; }
  [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'This build supports x86_64 Linux.' >&2; exit 1; }
  [[ -d /run/systemd/system ]] || { echo 'A running systemd installation is required.' >&2; exit 1; }
  missing=false
  for command in python3 curl openssl getent groupadd useradd runuser systemctl; do
    command -v "$command" >/dev/null || missing=true
  done
  if [[ "$missing" == true ]]; then
    if command -v apt-get >/dev/null; then
      apt-get update
      DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends python3 curl openssl ca-certificates passwd util-linux
    elif command -v dnf >/dev/null; then
      dnf install -y python3 curl openssl ca-certificates shadow-utils util-linux
    else
      echo 'Install python3, curl, openssl, ca-certificates and shadow/util-linux tools first.' >&2; exit 1
    fi
  fi
fi
command -v python3 >/dev/null || { echo 'python3 is required.' >&2; exit 1; }
python3 - "$mode" "$archive" "$output" "$DOWNLOAD_URL" "$ARCHIVE_SHA256" "$INITIAL_USER" "$INSTALL_UV" "$UV_URL" "$UV_SHA256" 3<&0 <<'PY'
import datetime, hashlib, json, os, pathlib, pwd, re, resource, select, shutil, signal, sqlite3, ssl, subprocess, sys, tarfile, tempfile, time, urllib.error, urllib.parse, urllib.request, uuid, zipfile

mode, archive, output, source_url, expected_sha, initial_user, install_uv, uv_url, uv_sha = sys.argv[1:]
CORE='hydra-core-linux-x86_64-musl'
PROXY='hydra-proxy-linux-x86_64-musl'
ALLOWED={CORE,PROXY,'SHA256SUMS','BUILD.txt'}
MAX_DOWNLOAD=256*1024*1024
ROOT=pathlib.Path('/etc/hydra')
DATA=pathlib.Path('/var/lib/hydra-core')
SOCKET='/run/hydra-proxy/proxy.sock'
CREDENTIALS=pathlib.Path('/root/hydra-install-credentials.json')


def require(condition,message):
    if not condition: raise RuntimeError(message)


def digest(path):
    value=hashlib.sha256()
    with open(path,'rb') as stream:
        for chunk in iter(lambda:stream.read(65536),b''): value.update(chunk)
    return value.hexdigest()


def valid_url(url):
    parsed=urllib.parse.urlsplit(url)
    require(parsed.scheme=='https' and parsed.hostname and not parsed.username and not parsed.password,'Download URLs must use HTTPS without embedded credentials')
    return parsed


def curl_quote(value):
    require(not any(ord(character)<32 or ord(character)==127 for character in value),'Invalid control character in download configuration')
    return '"'+value.replace('\\','\\\\').replace('"','\\"')+'"'


def curl_download(url,target,limit,configuration='',follow=True):
    valid_url(url)
    require(shutil.which('curl'),'curl is required for downloads; install curl or run the installer as root')
    version=run('curl','--version').stdout.splitlines()[0]
    match=re.match(r'curl (\d+)\.(\d+)\.(\d+)',version)
    require(match and tuple(map(int,match.groups()))>=(7,75,0),'curl 7.75 or newer is required for S3 signing; update curl using your package manager')
    arguments=['curl','--disable','--config','-','--silent','--show-error','--fail',
        '--proto','=https','--proto-redir','=https','--connect-timeout','20','--max-time','300',
        '--max-filesize',str(limit),'--dump-header','-','--output',str(target)]
    if follow: arguments+=['--location','--max-redirs','3']
    environment=os.environ.copy()
    if environment.get('SSL_CERT_FILE'): environment['CURL_CA_BUNDLE']=environment['SSL_CERT_FILE']
    def bound_file_size(): resource.setrlimit(resource.RLIMIT_FSIZE,(limit,limit))
    try:
        result=subprocess.run(arguments+['--url',url],input=configuration.encode(),stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment,timeout=305,preexec_fn=bound_file_size)
    except (OSError,subprocess.TimeoutExpired):
        target.unlink(missing_ok=True);raise RuntimeError('curl download failed or timed out') from None
    require(len(result.stdout)<=256*1024,'Download response headers exceed limit')
    blocks=[part for part in result.stdout.replace(b'\r\n',b'\n').split(b'\n\n') if part.startswith(b'HTTP/')]
    lines=blocks[-1].decode('iso-8859-1').splitlines() if blocks else []
    status=int(lines[0].split()[1]) if lines else 0
    if result.returncode or status!=200:
        target.unlink(missing_ok=True)
        if status==403: raise RuntimeError('Download denied (HTTP 403): the S3 key needs GetObject/read access to the release objects')
        if 300<=status<400: raise RuntimeError('S3 redirect refused; enter the correct HTTPS S3 endpoint')
        raise RuntimeError('Download failed (HTTP %s, curl exit %s)'%(status,result.returncode))
    require(target.is_file() and target.stat().st_size<=limit,'Download exceeds size limit')
    headers={}
    for line in lines[1:]:
        name,separator,value=line.partition(':')
        if separator:
            name=name.lower().strip()
            require(name not in headers or not name.startswith('x-amz-meta-'),'Duplicate S3 release metadata')
            headers[name]=value.strip()
    return headers


def download(url,target):
    parsed=valid_url(url);configuration=''
    if parsed.hostname=='api.github.com':
        token=os.environ.get('HYDRA_GITHUB_TOKEN') or os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
        if token: configuration+='header = '+curl_quote('Authorization: Bearer '+token)+'\n'
        configuration+='header = "X-GitHub-Api-Version: 2022-11-28"\n'
    curl_download(url,target,MAX_DOWNLOAD,configuration)


def read_s3_input():
    import termios
    with os.fdopen(3,'r') as stream:
        def field(label,hidden=False,optional=False):
            terminal=stream.isatty();previous=None
            if terminal:
                if hidden:
                    previous=termios.tcgetattr(stream.fileno());changed=previous.copy();changed[3]&=~termios.ECHO
                    termios.tcsetattr(stream.fileno(),termios.TCSANOW,changed)
                print(label+': ',end='',file=sys.stderr,flush=True)
            try: value=stream.readline(4098)
            finally:
                if previous is not None:
                    termios.tcsetattr(stream.fileno(),termios.TCSANOW,previous);print(file=sys.stderr)
            value=value.rstrip('\r\n')
            require(len(value)<=4096,'S3 input field exceeds 4096 characters')
            require(optional or bool(value),'Missing S3 '+label+' on stdin')
            require(not any(ord(character)<32 or ord(character)==127 for character in value),'Invalid control character in S3 input')
            return value
        endpoint=field('endpoint URL').strip();region=field('region').strip();bucket=field('bucket').strip()
        key=field('access key ID',hidden=True);secret=field('secret access key',hidden=True)
        token=field('session token (optional; Enter to skip)',hidden=True,optional=True)
    parsed=valid_url(endpoint)
    require(not parsed.query and not parsed.fragment,'S3 endpoint must not contain a query or fragment')
    require(re.fullmatch(r'[a-z0-9][a-z0-9-]{0,62}',region),'Invalid S3 region')
    require(re.fullmatch(r'[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]',bucket),'Invalid S3 bucket name')
    return endpoint.rstrip('/'),region,bucket,key,secret,token


def verify_binaries(directory,checks):
    for name,checksum in checks.items():
        require(digest(directory/name)==checksum,'Binary checksum mismatch: '+name)
        with open(directory/name,'rb') as executable: header=executable.read(20)
        require(header[:6]==b'\x7fELF\x02\x01' and header[18:20]==b'\x3e\x00','Expected an x86_64 Linux ELF binary: '+name)
        os.chmod(directory/name,0o755)
    print('Both Hydra binary checksums verified',flush=True)


def fetch_s3(directory):
    endpoint,region,bucket,key,secret,token=read_s3_input()
    parsed=valid_url(endpoint)
    path=urllib.parse.quote(urllib.parse.unquote(parsed.path).rstrip('/')+'/'+bucket,safe='/-_.~')
    base=urllib.parse.urlunsplit((parsed.scheme,parsed.netloc,path,'',''))
    configuration='aws-sigv4 = '+curl_quote('aws:amz:'+region+':s3')+'\nuser = '+curl_quote(key+':'+secret)+'\n'
    if token: configuration+='header = '+curl_quote('x-amz-security-token: '+token)+'\n'
    configuration+='header = "Cache-Control: no-cache"\n'
    bundle=directory/'verified';bundle.mkdir(mode=0o700);checks={};commit=None
    for remote,local in [('hydra-core',CORE),('hydra-proxy',PROXY)]:
        headers=curl_download(base+'/'+remote,bundle/local,64*1024*1024,configuration,follow=False)
        sha=headers.get('x-amz-meta-sha256','');current=headers.get('x-amz-meta-commit','');target=headers.get('x-amz-meta-target','')
        require(re.fullmatch(r'[0-9a-f]{64}',sha) and re.fullmatch(r'[0-9a-f]{40}',current) and target=='x86_64-unknown-linux-musl','Missing or invalid S3 release checksum/commit/target metadata')
        require(commit is None or commit==current,'S3 release changed during download: Core and Proxy commits differ; retry after publication finishes')
        commit=current;checks[local]=sha
    verify_binaries(bundle,checks)
    (bundle/'SHA256SUMS').write_text(''.join(sha+'  '+name+'\n' for name,sha in checks.items()))
    (bundle/'BUILD.txt').write_text('commit='+commit+'\ntarget=x86_64-unknown-linux-musl\n')
    print('S3 release commit:',commit,flush=True)
    return bundle


def fetch_bundle(directory):
    source=pathlib.Path(archive) if archive else directory/'download'
    if not archive: download(source_url,source)
    require(source.is_file() and source.stat().st_size<=MAX_DOWNLOAD,'Archive is missing or exceeds 256 MiB')
    with open(source,'rb') as stream: prefix=stream.read(128).lstrip()
    if not archive and prefix.startswith(b'{'):
        require(source.stat().st_size<=2*1024*1024,'Artifact index exceeds 2 MiB')
        catalog=json.loads(source.read_text())
        items=[item for item in catalog.get('artifacts',[]) if re.fullmatch(r'hydra-linux-x86_64-musl-[0-9a-f]{40}',item.get('name','')) and not item.get('expired',True) and item.get('workflow_run',{}).get('head_branch')=='main']
        require(items,'No unexpired main-branch Linux build found at the configured artifact index')
        item=max(items,key=lambda item:item.get('created_at',''))
        print('Build:',item['name'],flush=True)
        source=directory/'artifact.zip';download(item['archive_download_url'],source)
        if item.get('digest'):
            require(item['digest']=='sha256:'+digest(source),'GitHub artifact digest mismatch')
    if expected_sha:
        require(re.fullmatch(r'[0-9a-fA-F]{64}',expected_sha) and digest(source)==expected_sha.lower(),'Archive SHA-256 mismatch')
    if zipfile.is_zipfile(source):
        with zipfile.ZipFile(source) as wrapped:
            members=wrapped.infolist()
            require(len(members)==1 and members[0].filename=='hydra-linux-x86_64-musl.tar.gz','Expected an Actions ZIP containing hydra-linux-x86_64-musl.tar.gz')
            require(members[0].file_size<=MAX_DOWNLOAD and (members[0].external_attr>>16)&0o170000!=0o120000,'Unsafe or oversized artifact ZIP')
            inner=directory/'artifact.tar.gz'
            with wrapped.open(members[0]) as incoming,open(inner,'xb') as outgoing: shutil.copyfileobj(incoming,outgoing,65536)
            source=inner
    unpacked=directory/'verified';unpacked.mkdir(mode=0o700)
    seen=set()
    with tarfile.open(source,'r|*') as package:
        count=0
        for member in package:
            count+=1;require(count<=8,'Too many archive members')
            path=pathlib.PurePosixPath(member.name)
            if member.isdir() and not path.parts: continue
            require(not path.is_absolute() and len(path.parts)==1 and path.name in ALLOWED and member.isfile() and path.name not in seen,'Unsafe or unexpected archive member')
            require(0<=member.size<=(64*1024*1024 if path.name in {CORE,PROXY} else 4096),'Archive member exceeds size bound')
            seen.add(path.name)
            with package.extractfile(member) as incoming,open(unpacked/path.name,'xb') as outgoing: shutil.copyfileobj(incoming,outgoing,65536)
    require({CORE,PROXY,'SHA256SUMS'}<=seen,'Build must contain both binaries and SHA256SUMS')
    checks={}
    for line in (unpacked/'SHA256SUMS').read_text().splitlines():
        match=re.fullmatch(r'([0-9a-fA-F]{64}) [ *](hydra-(?:core|proxy)-linux-x86_64-musl)',line)
        require(match and match[2] not in checks,'Invalid binary checksum manifest')
        checks[match[2]]=match[1].lower()
    require(set(checks)=={CORE,PROXY},'Both binary checksums are required')
    verify_binaries(unpacked,checks)
    return unpacked


def run(*arguments,check=True):
    result=subprocess.run(arguments,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    if check and result.returncode: raise RuntimeError('%s failed: %s'%(arguments[0],result.stderr.strip()[:1024]))
    return result


def owned_dir(path,user,mode=0o700):
    path=pathlib.Path(path);require(not path.is_symlink(),'Refusing symlink directory: '+str(path))
    path.mkdir(parents=True,exist_ok=True,mode=mode)
    account=pwd.getpwnam(user);os.chown(path,account.pw_uid,account.pw_gid);os.chmod(path,mode)


def write_new(path,content,user,mode=0o600):
    path=pathlib.Path(path)
    if path.exists() or path.is_symlink(): return
    descriptor=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,mode)
    with os.fdopen(descriptor,'w') as target:
        target.write(content);os.fchmod(target.fileno(),mode)
    account=pwd.getpwnam(user);os.chown(path,account.pw_uid,account.pw_gid)


def prepare_uv(directory):
    for path in ['/usr/local/bin/uv','/usr/bin/uv']:
        if os.path.isfile(path): return path,None
    if install_uv!='true': return '/usr/local/bin/uv',None
    package=directory/'uv.tar.gz';download(uv_url,package)
    require(digest(package)==uv_sha,'uv archive checksum mismatch')
    target=directory/'uv';found=False
    with tarfile.open(package,'r|gz') as archive_file:
        for member in archive_file:
            if member.name=='uv-x86_64-unknown-linux-musl/uv':
                require(member.isfile() and member.size<=100*1024*1024,'Invalid uv binary')
                with archive_file.extractfile(member) as incoming,open(target,'xb') as outgoing: shutil.copyfileobj(incoming,outgoing,65536)
                found=True
    require(found,'uv binary missing from pinned archive');os.chmod(target,0o755)
    return '/usr/local/bin/uv',target


CORE_UNIT='''[Unit]
Description=Hydra operations control plane
After=network.target hydra-proxy.service

[Service]
Type=simple
User=hydra-core
Group=hydra-core
SupplementaryGroups=hydra-control
UMask=0077
RuntimeDirectory=hydra-core
RuntimeDirectoryMode=0700
StateDirectory=hydra-core
StateDirectoryMode=0700
ExecStart=/usr/local/bin/hydra-core --socket /run/hydra-proxy/proxy.sock --database /var/lib/hydra-core/hydra.sqlite3 --key /var/lib/hydra-core/jwt.pem serve --listen 127.0.0.1:8080 --origin https://localhost:8080 --tls-cert /etc/hydra/tls.crt --tls-key /etc/hydra/tls.key --operations /etc/hydra/core-settings/operations.json
Restart=on-failure
RestartSec=1
TimeoutStopSec=10
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/hydra-core /run/hydra-core /etc/hydra/core-settings
LimitNOFILE=4096
TasksMax=128
MemoryMax=1G

[Install]
WantedBy=multi-user.target
'''
PROXY_UNIT='''[Unit]
Description=Hydra TCP/UDP data plane
After=network.target

[Service]
Type=simple
User=hydra-proxy
Group=hydra-control
UMask=0077
RuntimeDirectory=hydra-proxy
RuntimeDirectoryMode=0750
StateDirectory=hydra-proxy
StateDirectoryMode=0700
ExecStart=/usr/local/bin/hydra-proxy --config /etc/hydra/proxy.json
Restart=on-failure
RestartSec=1
TimeoutStopSec=10
NoNewPrivileges=true
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/hydra-proxy /run/hydra-proxy
LimitNOFILE=16384
TasksMax=64
MemoryMax=512M

[Install]
WantedBy=multi-user.target
'''


def create_initial_user():
    # Feed the existing hidden CLI prompt via a PTY. Passwords never enter argv.
    if CREDENTIALS.exists():
        require(CREDENTIALS.stat().st_mode&0o077==0 and not CREDENTIALS.is_symlink(),'Bootstrap credential file must be private')
        credentials=json.loads(CREDENTIALS.read_text());require(credentials['username']==initial_user,'Existing bootstrap credential belongs to another username')
    else:
        import secrets
        credentials={'username':initial_user,'password':secrets.token_urlsafe(24)}
        write_new(CREDENTIALS,json.dumps(credentials)+'\n','root')
    import pty
    child,terminal=pty.fork()
    if child==0:
        arguments=['runuser','-u','hydra-core','--','/usr/local/bin/hydra-core','--database',str(DATA/'hydra.sqlite3'),'--key',str(DATA/'jwt.pem'),'create','user',initial_user]
        os.execvpe(arguments[0],arguments,{'PATH':'/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin','LANG':'C.UTF-8'})
    sent=False;received=b'';deadline=time.monotonic()+30;status=None
    try:
        while time.monotonic()<deadline:
            if select.select([terminal],[],[],.05)[0]:
                try: block=os.read(terminal,4096)
                except OSError: block=b''
                received=(received+block)[-8192:]
                if b'Password:' in received and not sent:
                    time.sleep(.1);os.write(terminal,credentials['password'].encode()+b'\n');sent=True
            done,code=os.waitpid(child,os.WNOHANG)
            if done: status=code;break
        if status is None:
            os.kill(child,9);os.waitpid(child,0);raise RuntimeError('CLI bootstrap timed out')
        require(os.waitstatus_to_exitcode(status)==0,'CLI bootstrap failed; installation will be rolled back')
    finally: os.close(terminal)


def install(bundle,directory):
    require(re.fullmatch(r'[A-Za-z0-9_.-]{1,64}',initial_user),'Invalid initial username')
    require(install_uv in {'true','false'},'HYDRA_INSTALL_UV must be true or false')
    # Do not back up the wrong database on a customized existing installation.
    existing=run('systemctl','show','hydra-core','-p','ExecStart','--value',check=False).stdout
    for flag,expected in [('database',str(DATA/'hydra.sqlite3')),('key',str(DATA/'jwt.pem')),('operations',str(ROOT/'core-settings/operations.json'))]:
        match=re.search(r'--'+flag+r'[ =]+([^\s;]+)',existing)
        require(not existing.strip() or (match and match[1]==expected),'Existing custom Core paths require the normal redeploy procedure')
    uv_path,uv_binary=prepare_uv(directory)
    stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-'+uuid.uuid4().hex[:8]
    backup=pathlib.Path('/var/backups/hydra')/stamp;backup.mkdir(parents=True,mode=0o700)
    names=['hydra-core','hydra-proxy']
    was_active={name:run('systemctl','is-active','--quiet',name,check=False).returncode==0 for name in names}
    was_enabled={name:run('systemctl','is-enabled','--quiet',name,check=False).returncode==0 for name in names}
    files=[pathlib.Path('/usr/local/bin')/name for name in names]+[pathlib.Path('/etc/systemd/system')/(name+'.service') for name in names]+[ROOT/'proxy.json',ROOT/'core-settings/operations.json',ROOT/'tls.key',ROOT/'tls.crt',DATA/'jwt.pem',CREDENTIALS]
    if uv_binary: files.append(pathlib.Path(uv_path))
    saved={}
    for path in files:
        require(not path.is_symlink() and (not path.exists() or path.is_file()),'Expected a regular installation file: '+str(path))
        if path.exists():
            metadata=path.stat();copy=backup/path.relative_to('/');copy.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(path,copy);saved[path]=(copy,metadata.st_uid,metadata.st_gid)
        else: saved[path]=None
    database=DATA/'hydra.sqlite3';had_database=database.exists();database_backed_up=False;changed=False
    require(not database.is_symlink(),'Refusing symlink database')
    try:
        if was_active['hydra-core']: run('systemctl','stop','hydra-core')
        if had_database:
            with sqlite3.connect('file:'+str(database)+'?mode=ro',uri=True) as source,sqlite3.connect(backup/'database.sqlite3') as target: source.backup(target)
            database_backed_up=True
        changed=True
        if run('getent','group','hydra-control',check=False).returncode: run('groupadd','--system','hydra-control')
        for name in names:
            if run('getent','passwd',name,check=False).returncode: run('useradd','--system','--user-group','--groups','hydra-control','--home-dir','/nonexistent','--shell','/usr/sbin/nologin',name)
            owned_dir('/var/lib/'+name,name)
        if not ROOT.exists():
            ROOT.mkdir(parents=True,mode=0o755);os.chmod(ROOT,0o755)
        owned_dir(ROOT/'core-settings','hydra-core');owned_dir(DATA/'farm-host','hydra-core')
        write_new(ROOT/'proxy.json',json.dumps({'socket':SOCKET,'state_file':'/var/lib/hydra-proxy/last-good.json','allowed_core_uid':pwd.getpwnam('hydra-core').pw_uid,'socket_mode':0o660},indent=2)+'\n','hydra-proxy')
        write_new(ROOT/'core-settings/operations.json',json.dumps({'traffic_history_per_service_mib':128,'flag_pattern':None,'auto_submit_captured':False,'checker':None,'sandbox':None,'execution_mode':'sandbox','host_runtime':{'uv':uv_path,'work_dir':str(DATA/'farm-host')}},indent=2)+'\n','hydra-core')
        require((ROOT/'tls.key').exists()==(ROOT/'tls.crt').exists(),'Incomplete existing TLS certificate pair')
        if not (ROOT/'tls.key').exists():
            run('openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','365','-keyout',str(ROOT/'tls.key'),'-out',str(ROOT/'tls.crt'),'-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,IP:127.0.0.1')
            account=pwd.getpwnam('hydra-core')
            for path in [ROOT/'tls.key',ROOT/'tls.crt']: os.chown(path,account.pw_uid,account.pw_gid);os.chmod(path,0o600)
        pathlib.Path('/usr/local/bin').mkdir(parents=True,exist_ok=True)
        for name,source in [('hydra-core',CORE),('hydra-proxy',PROXY)]:
            target=pathlib.Path('/usr/local/bin')/name;temporary=target.with_name(name+'.install-'+uuid.uuid4().hex)
            try:
                shutil.copyfile(bundle/source,temporary);os.chmod(temporary,0o755);os.replace(temporary,target)
            finally: temporary.unlink(missing_ok=True)
        if uv_binary:
            shutil.copyfile(uv_binary,uv_path);os.chmod(uv_path,0o755)
        write_new('/etc/systemd/system/hydra-core.service',CORE_UNIT,'root',0o644)
        write_new('/etc/systemd/system/hydra-proxy.service',PROXY_UNIT,'root',0o644)
        users=0
        if database.exists():
            with sqlite3.connect('file:'+str(database)+'?mode=ro',uri=True) as connection:
                tables={row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
                require(not tables or 'users' in tables,'Existing database is not a Hydra database')
                if 'users' in tables: users=connection.execute('SELECT count(*) FROM users').fetchone()[0]
        if not users: create_initial_user()
        run('systemctl','daemon-reload');run('systemctl','enable','hydra-proxy','hydra-core')
        run('systemctl','restart','hydra-proxy');run('systemctl','restart','hydra-core')
        healthy=False
        for attempt in range(60):
            status=run('runuser','-u','hydra-core','--','/usr/local/bin/hydra-core','--socket',SOCKET,'status',check=False)
            try:
                with urllib.request.urlopen('https://127.0.0.1:8080/',context=ssl._create_unverified_context(),timeout=2) as response: page_ok=response.status==200
                healthy=page_ok and status.returncode==0 and all(run('systemctl','is-active','--quiet',name,check=False).returncode==0 for name in names)
            except (OSError,urllib.error.URLError): healthy=False
            if healthy: break
            time.sleep(.5)
        require(healthy,'Hydra health checks failed')
        print('Hydra installed. Console: https://localhost:8080 (SSH tunnel).')
        print('Rollback backup:',backup)
        print('Initial credentials: '+str(CREDENTIALS) if not users else 'Existing users and credentials preserved.')
        print('Add your service listeners and authorized Farm teams in the console. Python host execution can be enabled in Settings.')
    except Exception:
        if not changed:
            if was_active['hydra-core']: run('systemctl','start','hydra-core',check=False)
            raise
        stopped=run('systemctl','stop','hydra-core','hydra-proxy',check=False)
        if stopped.returncode and any(run('systemctl','is-active','--quiet',name,check=False).returncode==0 for name in names):
            print('Could not stop Hydra for rollback. Recover manually from:',backup,file=sys.stderr);raise
        for name in names:
            if not was_enabled[name]: run('systemctl','disable',name,check=False)
        for path,copy in saved.items():
            if path.exists() or path.is_symlink(): path.unlink()
            if copy:
                original,uid,gid=copy;shutil.copy2(original,path);os.chown(path,uid,gid)
        if database_backed_up or not had_database:
            for suffix in ['-wal','-shm']: pathlib.Path(str(database)+suffix).unlink(missing_ok=True)
            if database_backed_up:
                shutil.copyfile(backup/'database.sqlite3',database)
                account=pwd.getpwnam('hydra-core');os.chown(database,account.pw_uid,account.pw_gid);os.chmod(database,0o600)
            else: database.unlink(missing_ok=True)
        run('systemctl','daemon-reload',check=False)
        for name in ['hydra-proxy','hydra-core']:
            if was_active[name]: run('systemctl','start',name,check=False)
        print('Installation failed; prior binaries/configuration/database restored. Backup:',backup,file=sys.stderr)
        raise


def interrupted(signum,frame): raise RuntimeError('Installation interrupted')
signal.signal(signal.SIGTERM,interrupted)
signal.signal(signal.SIGINT,interrupted)

try:
    with tempfile.TemporaryDirectory(prefix='hydra-install-') as temporary:
        directory=pathlib.Path(temporary);bundle=fetch_bundle(directory) if archive or source_url else fetch_s3(directory)
        if mode=='download':
            destination=pathlib.Path(output)
            require(not destination.exists(),'Download directory must not exist')
            shutil.copytree(bundle,destination);os.chmod(destination,0o700)
            print('Verified build extracted to:',destination)
        else: install(bundle,directory)
except Exception as error:
    print('Hydra installer:',str(error),file=sys.stderr);sys.exit(1)
PY
