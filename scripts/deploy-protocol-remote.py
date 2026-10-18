#!/usr/bin/env python3
"""Run on a Hydra VM: validate/upload an immutable WASM version and activate it.

Credentials stay in a private file on that VM. A failed activation restores the
previous confirmed decoder; new unused immutable versions remain registered.
"""
import argparse
import hashlib
import json
import pathlib
import re
import ssl
import stat
import sys
import urllib.error
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('module', type=pathlib.Path)
    parser.add_argument('--id', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--service', required=True)
    parser.add_argument('--origin', default='https://localhost:8080')
    parser.add_argument('--ca', default='/etc/hydra/tls.crt')
    parser.add_argument('--credentials', type=pathlib.Path,
                        default=pathlib.Path('/root/hydra-staging/operator-credentials.json'))
    args = parser.parse_args()
    for value in (args.id, args.version, args.service):
        if not re.fullmatch(r'[A-Za-z0-9_.-]{1,64}', value):
            parser.error('Protocol, version and service identifiers must use 1..64 ASCII letters, digits, _, . or -')
    if not args.origin.startswith('https://'):
        parser.error('An HTTPS management origin is required')
    metadata = args.credentials.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077 or metadata.st_size > 4096:
        parser.error('Credentials must be a private regular file <=4096 bytes')
    with args.credentials.open('rb') as source:
        credential_bytes = source.read(4097)
    if len(credential_bytes) > 4096:
        parser.error('Credentials exceed4096 bytes')
    credentials = json.loads(credential_bytes)
    module = args.module.lstat()
    if not stat.S_ISREG(module.st_mode) or module.st_size > 4*1024*1024:
        parser.error('Module must be a regular WASM file <=4 MiB')
    with args.module.open('rb') as source:
        wasm = source.read(4*1024*1024+1)
    if len(wasm) > 4*1024*1024 or not wasm.startswith(b'\x00asm'):
        parser.error('Invalid or oversized WASM module')
    context = ssl.create_default_context(cafile=args.ca)
    origin = args.origin.rstrip('/')
    token = None

    def api(path, method='GET', value=None, binary=False):
        headers = {'Origin': origin, 'Content-Type': 'application/wasm' if binary else 'application/json'}
        if token:
            headers['Authorization'] = 'Bearer '+token
        if binary:
            headers['X-SHA256'] = hashlib.sha256(value).hexdigest()
        if path.endswith('/activate') or method == 'DELETE':
            headers['Idempotency-Key'] = str(uuid.uuid4())
        body = value if binary else None if value is None else json.dumps(value).encode()
        request = urllib.request.Request(origin+'/api/v1'+path, body, headers, method=method)
        try:
            with urllib.request.urlopen(request, context=context, timeout=30) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'Management API returned HTTP {error.code}') from None

    token = api('/auth/login', 'POST', credentials)['access_token']
    before = api('/protocols')
    old = next((a for a in before['activations'] if a['service_id'] == args.service), None)
    if old and not old['applied']:
        raise RuntimeError('Resolve the pending decoder activation before deploying another version')
    prefix = f'/protocols/{args.id}/{args.version}'
    result = api(prefix, 'PUT', wasm, binary=True)
    if result.get('state') != 'installed' or result.get('error'):
        raise RuntimeError('WASM installation failed validation; activation was not changed')
    try:
        result = api(prefix+'/activate', 'POST', {'service_id': args.service})
        if not result.get('applied'):
            raise RuntimeError('Proxy did not confirm the new decoder')
    except Exception:
        observed = next((a for a in api('/protocols')['activations'] if a['service_id'] == args.service), None)
        expected = {(args.id, args.version)}
        if old:
            expected.add((old['id'], old['version']))
        if (observed and (observed['id'], observed['version']) not in expected) or (old and observed is None):
            raise RuntimeError('Activation failed and another operation changed this service; left its decoder unchanged') from None
        if old:
            restored = api(f"/protocols/{old['id']}/{old['version']}/activate", 'POST', {'service_id': args.service})
        else:
            restored = api('/protocols/activations/'+args.service, 'DELETE')
        if not restored.get('applied'):
            raise RuntimeError('Activation failed; previous decoder restoration is pending') from None
        raise RuntimeError('Activation failed; previous decoder restored') from None
    current = api('/protocols')
    active = next((a for a in current['activations'] if a['service_id'] == args.service), None)
    if not active or not active['applied'] or (active['id'], active['version']) != (args.id, args.version):
        raise RuntimeError('Decoder activation changed concurrently; refresh its current state')
    installed = next(v for v in current['items'] if v['id'] == args.id and v['version'] == args.version)
    fields = (installed.get('metadata') or {}).get('fields', [])
    print(json.dumps({'id': args.id, 'version': args.version, 'service': args.service,
                      'applied': True, 'sha256': hashlib.sha256(wasm).hexdigest(),
                      'schema_fields': len(fields)}, sort_keys=True))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print('Protocol deployment failed: '+str(error), file=sys.stderr)
        sys.exit(1)
