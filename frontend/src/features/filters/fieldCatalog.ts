import type { FieldKind, RuleField } from './ruleBuilder'

export interface DecoderFieldSchema { path: string; field_type: string }
export interface DecoderCatalogItem {
  id: string
  version: string
  state?: string
  profile?: { framing?: { kind?: string } } | null
  metadata?: { name?: string; fields?: DecoderFieldSchema[] } | null
}
export interface DecoderCatalog {
  items: DecoderCatalogItem[]
  activations: Array<{ service_id: string; id: string; version: string; applied: boolean }>
  native_schemas: Array<{ id: string; fields: DecoderFieldSchema[] }>
}
export interface FieldService { id: string; protocol_id: string }

const universal: RuleField[] = [
  { value: 'body.size', label: 'Body/transfer size (live bytes)', kind: 'number' },
  { value: 'transfer.bytes', label: 'Total transferred (this direction)', kind: 'number' },
  { value: 'packet.size', label: 'Read/datagram size (bytes)', kind: 'number' },
  { value: 'body.size_scope', label: 'Body size scope', kind: 'string' },
  { value: 'network.src_ip', label: 'Client IP', kind: 'string' },
  { value: 'network.src_port', label: 'Client port', kind: 'number' },
  { value: 'network.dst_ip', label: 'Server IP', kind: 'string' },
  { value: 'network.dst_port', label: 'Server port', kind: 'number' },
  { value: 'transport.kind', label: 'Transport', kind: 'string' },
  { value: 'direction', label: 'Direction', kind: 'string' },
]
const labels: Record<string, string> = {
  'http.method': 'HTTP method', 'http.path': 'HTTP path', 'http.user_agent': 'HTTP user agent',
  'http.body_text': 'HTTP body (UTF-8)', 'http.body_length': 'HTTP body size (bytes)',
  'utf8.text': 'UTF-8 text', 'raw.bytes': 'Raw bytes (hex)', payload: 'Framed payload (hex)',
}
function kind(fieldType: string): FieldKind | undefined {
  if (fieldType === 'string') return 'string'
  if (fieldType === 'bytes') return 'bytes'
  if (fieldType === 'bool') return 'boolean'
  if (['signed', 'unsigned', 'double', 'int', 'uint', 'number'].includes(fieldType)) return 'number'
  return undefined
}
export function fieldsForService(service: FieldService, catalog: DecoderCatalog): RuleField[] {
  const activation = catalog.activations.find(item => item.service_id === service.id && item.applied)
  const version = activation && catalog.items.find(item => item.id === activation.id && item.version === activation.version)
  let schema: DecoderFieldSchema[] = []
  if (version?.profile) {
    schema = version.profile.framing?.kind === 'raw'
      ? [{ path: 'raw.bytes', field_type: 'bytes' }]
      : [{ path: 'payload', field_type: 'bytes' }]
  } else if (version?.metadata) {
    schema = version.metadata.fields ?? []
  } else {
    schema = catalog.native_schemas.find(item => item.id === service.protocol_id)?.fields ?? []
  }
  const decoded = schema.flatMap(field => {
    const fieldKind = kind(field.field_type)
    return fieldKind ? [{ value: field.path, label: labels[field.path] ?? field.path, kind: fieldKind, schemaType: field.field_type }] : []
  })
  const fields = [...universal, ...decoded]
  return [...new Map(fields.map(field => [field.value, field])).values()]
}
