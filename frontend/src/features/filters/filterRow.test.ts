import { describe, expect, test } from 'bun:test'
import { defaultActionType, expressionFromRow, rowFromExpression, type FilterRow } from './filterRow'

const row: FilterRow = { pattern: '\\.\\./', direction: 'both' }

describe('flat filter rows', () => {
  test('round-trips a pattern that applies to both directions', () => {
    const expression = expressionFromRow(row)
    expect(expression).toEqual({ field: 'stream.text', op: 'regex', value: { type: 'string', value: '\\.\\./' } })
    expect(rowFromExpression(expression)).toEqual(row)
  })

  test('round-trips a direction-scoped pattern', () => {
    const scoped: FilterRow = { pattern: '[A-Z0-9]{31}=', direction: 'server_to_client' }
    const expression = expressionFromRow(scoped)
    expect(expression).toEqual({
      direction: 'server_to_client',
      scoped: { field: 'stream.text', op: 'regex', value: { type: 'string', value: '[A-Z0-9]{31}=' } },
    })
    expect(rowFromExpression(expression)).toEqual(scoped)
  })

  test('keeps the client_to_server side', () => {
    const scoped: FilterRow = { pattern: 'X-Debug', direction: 'client_to_server' }
    expect(rowFromExpression(expressionFromRow(scoped))).toEqual(scoped)
  })

  test('rejects expressions that are not a stream regex', () => {
    expect(rowFromExpression({ field: 'http.path', op: 'regex', value: { type: 'string', value: 'a' } })).toBeNull()
    expect(rowFromExpression({ field: 'stream.text', op: 'eq', value: { type: 'string', value: 'a' } })).toBeNull()
    expect(rowFromExpression({ field: 'stream.text', op: 'regex', value: { type: 'bytes', value: [1] } })).toBeNull()
  })

  test('rejects grouped, negated and malformed expressions', () => {
    expect(rowFromExpression({ all: [{ field: 'stream.text', op: 'regex', value: { type: 'string', value: 'a' } }] })).toBeNull()
    expect(rowFromExpression({ not: { field: 'stream.text', op: 'regex', value: { type: 'string', value: 'a' } } })).toBeNull()
    expect(rowFromExpression({ direction: 'both', scoped: {} })).toBeNull()
    expect(rowFromExpression(null)).toBeNull()
    expect(rowFromExpression({})).toBeNull()
  })

  test('picks a default action the transport supports', () => {
    expect(defaultActionType('http', 'tcp')).toBe('block')
    expect(defaultActionType('raw', 'tcp')).toBe('drop_connection')
    expect(defaultActionType('raw', 'udp')).toBe('drop_datagram')
  })

  test('an empty pattern is still a row so the editor can open it', () => {
    expect(rowFromExpression(expressionFromRow({ pattern: '', direction: 'both' }))).toEqual({ pattern: '', direction: 'both' })
  })
})
