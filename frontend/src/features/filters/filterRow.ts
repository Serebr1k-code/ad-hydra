/** Flat filter rows: one regex over the raw stream window, with a free-text tag. */
import type { Expression } from './ruleBuilder'

export type RowDirection = 'both' | 'client_to_server' | 'server_to_client'
export const ROW_FIELD = 'stream.text'

export interface FilterRow {
  pattern: string
  direction: RowDirection
}

function isObject(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}

/** Reads a row out of a saved expression; anything richer is not a row. */
export function rowFromExpression(expression: Expression | null | undefined): FilterRow | null {
  if (!isObject(expression)) return null
  let node: Record<string, unknown> = expression
  let direction: RowDirection = 'both'
  if (node.scoped !== undefined) {
    if (node.direction !== 'client_to_server' && node.direction !== 'server_to_client') return null
    if (!isObject(node.scoped)) return null
    direction = node.direction
    node = node.scoped
  }
  if (node.field !== ROW_FIELD || node.op !== 'regex') return null
  const value = node.value
  if (!isObject(value) || value.type !== 'string' || typeof value.value !== 'string') return null
  return { pattern: value.value, direction }
}

/** Builds the stored expression for a row. */
export function expressionFromRow(row: FilterRow): Expression {
  const predicate = { field: ROW_FIELD, op: 'regex', value: { type: 'string', value: row.pattern } }
  return row.direction === 'both' ? predicate : { direction: row.direction, scoped: predicate }
}

/** HTTP services answer a block with a status; other transports drop. */
export function defaultActionType(protocolId: string, transport: string): string {
  if (protocolId === 'http') return 'block'
  return transport === 'udp' ? 'drop_datagram' : 'drop_connection'
}

/** Filters shown for one service; an empty id means every service. */
export function visibleFilters<T extends { service_id: string; additional_service_ids?: string[] }>(rules: T[], serviceId: string): T[] {
  if (!serviceId) return rules
  return rules.filter(rule => rule.service_id === serviceId || (rule.additional_service_ids ?? []).includes(serviceId))
}

/** Total hits per service, counting a shared rule once for each of its services. */
export function hitsByService(rules: Array<{ service_id: string; additional_service_ids?: string[]; hit_count: number }>): Record<string, number> {
  const totals: Record<string, number> = {}
  for (const rule of rules) {
    for (const id of [rule.service_id, ...(rule.additional_service_ids ?? [])]) totals[id] = (totals[id] ?? 0) + rule.hit_count
  }
  return totals
}
