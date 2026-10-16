"""Generate browser endpoint helpers and TypeScript declarations from active OpenAPI."""
import argparse
import json
import pathlib
import yaml

root = pathlib.Path(__file__).resolve().parents[1]
spec = yaml.safe_load((root / 'contracts/openapi.yaml').read_text())
operations = {}
for path, item in spec['paths'].items():
    for method, operation in item.items():
        if method in {'get','put','post','delete','patch','options','head'}:
            identifier = operation['operationId']
            assert identifier not in operations, identifier
            operations[identifier] = {'path':path,'method':method.upper()}
js = '// Generated from contracts/openapi.yaml; do not edit.\nexport const operations = ' + json.dumps(operations,sort_keys=True,indent=2) + ' as const;\n'
js += '''export function path(id: keyof typeof operations, values: Record<string, string | number> = {}): string {
  const operation=operations[id];if(!operation)throw Error('Unknown API operation '+id);
  return operation.path.replace(/\\{([^}]+)\\}/g,(_,name)=>{
    if(values[name]===undefined)throw Error('Missing path parameter '+name);
    return encodeURIComponent(values[name]);
  });
}
'''
def declaration(s):
    if '$ref' in s:
        return s['$ref'].rsplit('/',1)[-1]
    if 'enum' in s:
        return ' | '.join(json.dumps(x) for x in s['enum']) or 'never'
    for key in ['oneOf','anyOf','allOf']:
        if key in s:
            return '(' + (' & ' if key=='allOf' else ' | ').join(declaration(x) for x in s[key]) + ')'
    kind = s.get('type')
    if isinstance(kind,list):
        return ' | '.join(declaration(dict(s,type=t)) for t in kind)
    if kind == 'array':
        return 'Array<' + declaration(s.get('items',{})) + '>'
    if kind == 'object':
        required = s.get('required',[])
        properties = s.get('properties',{})
        if properties:
            return '{ ' + '; '.join(json.dumps(k)+('' if k in required else '?')+': '+declaration(v) for k,v in properties.items())+' }'
        return 'Record<string, unknown>'
    return {'string':'string','integer':'number','number':'number','boolean':'boolean','null':'null'}.get(kind,'unknown')
types = '// Generated from contracts/openapi.yaml; do not edit.\n' + '\n'.join('export type '+name+' = '+declaration(value)+';' for name,value in sorted(spec.get('components',{}).get('schemas',{}).items()))+'\n'
parser=argparse.ArgumentParser();parser.add_argument('--check',action='store_true');args=parser.parse_args()
for name,content in [('operations.ts',js),('contracts.d.ts',types)]:
    target=root/'frontend/src/generated'/name
    if args.check:
        assert target.read_text()==content, f'{target} is stale; run python3 scripts/generate-frontend.py'
    else:
        target.write_text(content)
print('Frontend OpenAPI helpers/types: OK')
