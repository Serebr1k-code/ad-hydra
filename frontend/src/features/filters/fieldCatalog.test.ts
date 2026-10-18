import { expect, test } from 'bun:test'
import { fieldsForService } from './fieldCatalog'

test('lists universal and active decoder scalar fields, including WASM fields, with native types', () => {
  const fields = fieldsForService(
    { id: 'svc', protocol_id: 'http' },
    {
      items: [
        { id: 'custom', version: '2', state: 'installed', metadata: { name: 'custom', fields: [
          { path: 'game.player.name', field_type: 'string' },
          { path: 'game.player.score', field_type: 'unsigned' },
          { path: 'game.valid', field_type: 'bool' },
          { path: 'game.payload', field_type: 'bytes' },
          { path: 'game.player', field_type: 'object' },
        ] } },
        { id: 'unused', version: '1', state: 'installed', metadata: { fields: [{ path: 'unused.value', field_type: 'string' }] } },
      ],
      activations: [{ service_id: 'svc', id: 'custom', version: '2', applied: true }],
      native_schemas: [{ id: 'http', fields: [{ path: 'http.path', field_type: 'string' }] }],
    },
  )
  expect(fields.map(field => field.value)).toContain('game.player.name')
  expect(fields.find(field => field.value === 'game.player.score')?.kind).toBe('number')
  expect(fields.find(field => field.value === 'game.valid')?.kind).toBe('boolean')
  expect(fields.find(field => field.value === 'game.payload')?.kind).toBe('bytes')
  expect(fields.map(field => field.value)).not.toContain('game.player')
  expect(fields.map(field => field.value)).not.toContain('unused.value')
  expect(fields.map(field => field.value)).toContain('body.size')
})

test('uses built-in raw, UTF-8 and HTTP schemas when no custom decoder is active', () => {
  const catalog = { items: [], activations: [], native_schemas: [
    { id: 'raw', fields: [{ path: 'raw.bytes', field_type: 'bytes' }] },
    { id: 'utf8', fields: [{ path: 'utf8.text', field_type: 'string' }] },
    { id: 'http', fields: [{ path: 'http.path', field_type: 'string' }] },
  ] }
  expect(fieldsForService({ id: 'raw-service', protocol_id: 'raw' }, catalog).map(field => field.value)).toContain('raw.bytes')
  expect(fieldsForService({ id: 'text-service', protocol_id: 'utf8' }, catalog).map(field => field.value)).toContain('utf8.text')
  expect(fieldsForService({ id: 'http-service', protocol_id: 'http' }, catalog).map(field => field.value)).toContain('http.path')
})
