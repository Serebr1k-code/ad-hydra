export type FieldKind = 'string' | 'number'
export type Join = 'all' | 'any'
export interface Condition {
  id: string
  kind: 'condition'
  field: string
  op: string
  value: string
  negate?: boolean
}
export interface Group {
  id: string
  kind: 'group'
  join: Join
  negate?: boolean
  children: RuleNode[]
}
export type RuleNode = Condition | Group
export type Expression = Record<string, unknown>

export const fields: Array<{ value: string; label: string; kind: FieldKind }> = [
  { value: 'http.method', label: 'HTTP method', kind: 'string' },
  { value: 'http.path', label: 'HTTP path', kind: 'string' },
  { value: 'http.user_agent', label: 'HTTP user agent', kind: 'string' },
  { value: 'http.body_text', label: 'HTTP body (UTF-8)', kind: 'string' },
  { value: 'body.size', label: 'Request/body size in bytes (live)', kind: 'number' },
  { value: 'transfer.bytes', label: 'Total transferred bytes (this direction)', kind: 'number' },
  { value: 'http.body_length', label: 'HTTP body size in bytes', kind: 'number' },
  { value: 'packet.size', label: 'Inspected frame/read size in bytes', kind: 'number' },
  { value: 'network.src_ip', label: 'Client IP', kind: 'string' },
  { value: 'network.src_port', label: 'Client port', kind: 'number' },
  { value: 'network.dst_ip', label: 'Server IP', kind: 'string' },
  { value: 'network.dst_port', label: 'Server port', kind: 'number' },
  { value: 'transport.kind', label: 'Transport', kind: 'string' },
  { value: 'direction', label: 'Direction', kind: 'string' },
]

export function createCondition(): Condition {
  return { id: crypto.randomUUID(), kind: 'condition', field: 'http.path', op: 'prefix', value: '/' }
}
export function createGroup(join: Join = 'all'): Group {
  return { id: crypto.randomUUID(), kind: 'group', join, negate: false, children: [createCondition()] }
}
export function fieldKind(field: string): FieldKind {
  return fields.find(item => item.value === field)?.kind ?? 'string'
}

function uintValue(input: string): string {
  if (input.length > 128) throw new Error('Size is too long')
  const match = /^\s*(\d+(?:\.\d+)?)\s*(b|kb|kib|mb|mib|gb|gib)?\s*$/i.exec(input)
  if (!match) throw new Error('Enter a number, optionally with B, KB, KiB, MB, MiB, GB, or GiB')
  const unit = (match[2] ?? '').toLowerCase()
  const multiplier = unit === 'kb' ? 1000 : unit === 'kib' ? 1024 : unit === 'mb' ? 1_000_000 : unit === 'mib' ? 1_048_576 : unit === 'gb' ? 1_000_000_000 : unit === 'gib' ? 1_073_741_824 : 1
  const [whole = '0', fraction = ''] = (match[1] ?? '0').split('.')
  const scale = 10n ** BigInt(fraction.length)
  const scaled = BigInt(whole + fraction) * BigInt(multiplier)
  if (scaled % scale !== 0n) throw new Error('Size must resolve to a whole number of bytes')
  const result = scaled / scale
  if (result > 18446744073709551615n) throw new Error('Size exceeds the 64-bit byte counter')
  return result.toString()
}

function conditionExpression(node: Condition): Expression {
  let op = node.op
  if (op === 'not_regex') op = 'regex'
  const kind = fieldKind(node.field)
  let value: Record<string, unknown>
  if (kind === 'number') {
    const converted = uintValue(node.value)
    value = { type: 'uint', value: converted }
  } else value = { type: 'string', value: node.value }
  const predicate = { field: node.field, op, value }
  return node.op === 'not_regex' || node.negate ? { not: predicate } : predicate
}
export function toExpression(node: RuleNode): Expression {
  if (node.kind === 'condition') return conditionExpression(node)
  if (!node.children.length) throw new Error('A condition group must have at least one rule')
  const children = node.children.map(toExpression)
  const expression = node.join === 'all' ? { all: children } : { any: children }
  return node.negate ? { not: expression } : expression
}

function readValue(input: unknown): string {
  if (!input || typeof input !== 'object') return ''
  const value = (input as { value?: unknown }).value
  return typeof value === 'string' || typeof value === 'number' ? String(value) : ''
}
function parseNode(input: unknown): RuleNode {
  if (!input || typeof input !== 'object') throw new Error('Unsupported saved filter expression')
  const value = input as Record<string, unknown>
  if (Array.isArray(value.all) || Array.isArray(value.any)) {
    const join: Join = Array.isArray(value.all) ? 'all' : 'any'
    const rawChildren = (join === 'all' ? value.all : value.any) as unknown[]
    return { id: crypto.randomUUID(), kind: 'group', join, children: rawChildren.map(parseNode) }
  }
  if (value.not && typeof value.not === 'object') {
    const child = parseNode(value.not)
    if (child.kind === 'condition' && child.op === 'regex') return { ...child, id: crypto.randomUUID(), op: 'not_regex' }
    return { ...child, id: crypto.randomUUID(), negate: true }
  }
  if (typeof value.field === 'string' && typeof value.op === 'string') {
    const expected = value.value && typeof value.value === 'object' ? value.value as Record<string, unknown> : {}
    return {
      id: crypto.randomUUID(), kind: 'condition', field: value.field,
      op: value.op, value: readValue(expected),
    }
  }
  throw new Error('Unsupported saved filter expression')
}
export function fromExpression(expression: Expression): Group {
  const node = parseNode(expression)
  return node.kind === 'group' ? node : { id: crypto.randomUUID(), kind: 'group', join: 'all', negate: false, children: [node] }
}
