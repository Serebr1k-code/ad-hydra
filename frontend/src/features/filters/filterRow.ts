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

export function rowAction(row: FilterRow, transport: string): Record<string, unknown> {
  return transport === 'udp' ? { type: 'drop_datagram' } : { type: 'drop_connection' }
}
