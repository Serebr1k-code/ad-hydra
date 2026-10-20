"""Enforce repository dependency directions using Cargo's actual manifests."""
import json
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=root))
allowed = {
    'hydra-domain': set(),
    'hydra-compression': set(),
    'hydra-wire': {'hydra-domain'},
    'hydra-filter-engine': {'hydra-domain'},
    'hydra-protocol-sdk': set(),
    'hydra-protocol-runtime': {'hydra-domain', 'hydra-protocol-sdk'},
    'hydra-core': {'hydra-compression', 'hydra-domain', 'hydra-wire', 'hydra-filter-engine'},
    'hydra-proxy': {'hydra-compression', 'hydra-domain', 'hydra-wire', 'hydra-filter-engine', 'hydra-protocol-runtime', 'hydra-protocol-sdk'},
}
for package in metadata['packages']:
    name = package['name']
    assert name in allowed, f'unreviewed workspace package: {name}'
    for dep in package['dependencies']:
        target = dep['name']
        if dep['kind'] == 'dev':
            continue
        if target.startswith('hydra-'):
            assert target in allowed[name], (name, target)
        if name == 'hydra-proxy':
            assert not any(word in target for word in ['sql', 'jwt', 'argon', 'axum', 'utoipa']), (name, target)
        if name == 'hydra-compression':
            assert target in {'anyhow', 'flate2', 'zstd'}, (name, target)
        if name == 'hydra-domain':
            assert target in {'serde'}, (name, target)
assert list(root.glob('**/proxy.proto')) == [root / 'contracts/hydra/proxy/v1/proxy.proto']
assert not list((root / 'apps').glob('**/*.proto'))
print('Workspace dependency boundaries and canonical protobuf: OK')
