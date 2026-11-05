import {splitDirection,type FilterDirection} from './filterDirection'
export type FieldKind = 'string' | 'number' | 'boolean' | 'bytes'
export interface RuleField { value: string; label: string; kind: FieldKind }
export type Join = 'all' | 'any'
export interface Condition {
  id: string
  kind: 'condition'
  field: string
  op: string
  value: string
  negate?: boolean
  direction?: FilterDirection
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

export const fields: RuleField[] = [
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
  { value: 'raw.bytes', label: 'Raw bytes (hex)', kind: 'bytes' },
  { value: 'utf8.text', label: 'UTF-8 text', kind: 'string' },
]

export function createCondition(field = 'http.path', available: RuleField[] = fields): Condition {
  const kind = fieldKind(field, available)
  return { id: crypto.randomUUID(), kind: 'condition', field, op: kind === 'number' ? 'gt' : 'eq', value: kind === 'number' ? '512' : '' }
}
export function createGroup(join: Join = 'all', field = 'http.path', available: RuleField[] = fields): Group {
  return { id: crypto.randomUUID(), kind: 'group', join, negate: false, children: [createCondition(field, available)] }
}
export function fieldKind(field: string, available: RuleField[] = fields): FieldKind {
  return available.find(item => item.value === field)?.kind ?? fields.find(item => item.value === field)?.kind ?? 'string'
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

function bytesValue(input: string): number[] {
  const hex = input.replace(/(?:0x|[\s:_-])/gi, '')
  if (!hex || hex.length > 2048 || hex.length % 2 || !/^[\da-f]+$/i.test(hex)) throw new Error('Enter an even-length hex byte pattern (up to 1024 bytes)')
  return hex.match(/.{2}/g)!.map(byte => Number.parseInt(byte, 16))
}
function conditionExpression(node: Condition, available: RuleField[]): Expression {
  let op = node.op
  if (op === 'not_regex') op = 'regex'
  const kind = fieldKind(node.field, available)
  let value: Record<string, unknown>
  if (kind === 'number') {
    const converted = uintValue(node.value)
    value = { type: 'uint', value: converted }
  } else if (kind === 'boolean') {
    if (node.value !== 'true' && node.value !== 'false') throw new Error('Choose true or false')
    value = { type: 'bool', value: node.value === 'true' }
  } else if (kind === 'bytes') {
    value = { type: 'bytes', value: bytesValue(node.value) }
  } else value = { type: 'string', value: node.value }
  const predicate = { field: node.field, op, value }
  const expression=node.op === 'not_regex' || node.negate ? { not: predicate } : predicate
  return node.direction&&node.direction!=='both'?{direction:node.direction,scoped:expression}:expression
}
export function toExpression(node: RuleNode, available: RuleField[] = fields): Expression {
  if (node.kind === 'condition') return conditionExpression(node, available)
  if (!node.children.length) throw new Error('A condition group must have at least one rule')
  const children = node.children.map(child => toExpression(child, available))
  const expression = node.join === 'all' ? { all: children } : { any: children }
  return node.negate ? { not: expression } : expression
}

function readValue(input: unknown, type?: string): string {
  if (!input || typeof input !== 'object') return ''
  const value = (input as { value?: unknown }).value
  if (type === 'bytes' && Array.isArray(value) && value.every(item => Number.isInteger(item) && item >= 0 && item <= 255)) return value.map(item => Number(item).toString(16).padStart(2, '0')).join(' ')
  if (type === 'bool' && typeof value === 'boolean') return String(value)
  return typeof value === 'string' || typeof value === 'number' ? String(value) : ''
}
function parseNode(input: unknown): RuleNode {
  if (!input || typeof input !== 'object') throw new Error('Unsupported saved filter expression')
  const value = input as Record<string, unknown>
  if(value.scoped&&typeof value.scoped==='object'&&(value.direction==='client_to_server'||value.direction==='server_to_client')){
    const child=parseNode(value.scoped);applyDirection(child,value.direction);return child
  }

  if (Array.isArray(value.all) || Array.isArray(value.any)) {
    const join: Join = Array.isArray(value.all) ? 'all' : 'any'
    const rawChildren = (join === 'all' ? value.all : value.any) as unknown[]
    return { id: crypto.randomUUID(), kind: 'group', join, children: rawChildren.map(parseNode) }
  }
  if (value.not && typeof value.not === 'object') {
    const child = parseNode(value.not)
    if(child.kind==='condition'&&(child.op==='regex'||child.op==='not_regex'))return {...child,id:crypto.randomUUID(),op:child.op==='regex'?'not_regex':'regex'}
    return { ...child, id: crypto.randomUUID(), negate: !child.negate }
  }
  if (typeof value.field === 'string' && typeof value.op === 'string') {
    const expected = value.value && typeof value.value === 'object' ? value.value as Record<string, unknown> : {}
    return {
      id: crypto.randomUUID(), kind: 'condition', field: value.field,
      op: value.op, value: readValue(expected, typeof expected.type === 'string' ? expected.type : undefined),
    }
  }
  throw new Error('Unsupported saved filter expression')
}
export function fromExpression(expression: Expression): Group {
  const legacy=splitDirection(expression)
  const node = parseNode(legacy.expression)
  if(legacy.side!=='both')applyDirection(node,legacy.side)
  return node.kind === 'group' ? node : { id: crypto.randomUUID(), kind: 'group', join: 'all', negate: false, children: [node] }
}

/** Keep the editor valid when its only nested group is removed. */
export function removeChild(group: Group, id: string) {
  const index = group.children.findIndex(child => child.id === id)
  if (index < 0) return
  group.children.splice(index, 1)
  if (!group.children.length) group.children.push(createCondition())
}

function applyDirection(node:RuleNode,side:Exclude<FilterDirection,'both'>){
 if(node.kind==='condition'){
  if(node.direction&&node.direction!=='both'&&node.direction!==side)throw new Error('Conflicting nested direction scopes cannot be edited visually')
  node.direction=side
 }else for(const child of node.children)applyDirection(child,side)
}
export function conditionDirections(group:RuleNode):FilterDirection[]{
 const sides=new Set<FilterDirection>();const visit=(node:RuleNode)=>{if(node.kind==='condition')sides.add(node.direction??'both');else node.children.forEach(visit)};visit(group);return [...sides]
}
