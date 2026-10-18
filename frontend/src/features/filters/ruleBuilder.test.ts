import { describe, expect, test } from 'bun:test'
import { fromExpression, toExpression, type Group } from './ruleBuilder'

describe('visual filter expression builder', () => {
  test('serializes nested AND/OR groups, regex negation and byte units into the API tree', () => {
    const rules: Group = {
      id: 'root', kind: 'group', join: 'all', children: [
        { id: 'path', kind: 'condition', field: 'http.path', op: 'prefix', value: '/users' },
        { id: 'nested', kind: 'group', join: 'any', children: [
          { id: 'body', kind: 'condition', field: 'http.body_text', op: 'not_regex', value: '^Z(?:I[^U]|[^I]).*$' },
          { id: 'size', kind: 'condition', field: 'packet.size', op: 'gt', value: '512kb' },
        ] },
      ],
    }

    expect(toExpression(rules)).toEqual({ all: [
      { field: 'http.path', op: 'prefix', value: { type: 'string', value: '/users' } },
      { any: [
        { not: { field: 'http.body_text', op: 'regex', value: { type: 'string', value: '^Z(?:I[^U]|[^I]).*$' } } },
        { field: 'packet.size', op: 'gt', value: { type: 'uint', value: '512000' } },
      ] },
    ] })
  })

  test('round trips expressions without losing nested group operators', () => {
    const expression = { all: [
      { field: 'http.path', op: 'eq', value: { type: 'string', value: '/users' } },
      { any: [
        { not: { field: 'http.body_text', op: 'regex', value: { type: 'string', value: 'secret' } } },
        { field: 'packet.size', op: 'gt', value: { type: 'uint', value: '512000' } },
      ] },
    ] }
    expect(toExpression(fromExpression(expression))).toEqual(expression)
  })

  test('retains negated comparisons and parenthesized groups when editing saved rules', () => {
    const expression = { not: { any: [
      { field: 'http.path', op: 'eq', value: { type: 'string', value: '/admin' } },
      { field: 'http.method', op: 'eq', value: { type: 'string', value: 'DELETE' } },
    ] } }
    expect(toExpression(fromExpression(expression))).toEqual(expression)
  })

  test('rejects incomplete or invalid numeric values before sending', () => {
    expect(() => toExpression({ id: 'root', kind: 'group', join: 'all', children: [] })).toThrow()
    expect(() => toExpression({ id: 'size', kind: 'condition', field: 'packet.size', op: 'gt', value: '5widgets' })).toThrow()
  })
  test('live byte counts preserve full-width unsigned integers and exact unit conversion', () => {
    const condition = { id: 'limit', kind: 'condition' as const, field: 'body.size', op: 'gt', value: '18446744073709551615' }
    expect(toExpression(condition)).toEqual({ field: 'body.size', op: 'gt', value: { type: 'uint', value: '18446744073709551615' } })
    expect(toExpression({ ...condition, value: '0.5KiB' })).toEqual({ field: 'body.size', op: 'gt', value: { type: 'uint', value: '512' } })
    expect(() => toExpression({ ...condition, value: '18446744073709551616' })).toThrow()
    expect(() => toExpression({ ...condition, value: '0.1B' })).toThrow()
  })
  test('serializes raw byte comparisons from hex without converting payload bytes to text', () => {
    const raw = { id: 'raw', kind: 'condition' as const, field: 'raw.bytes', op: 'contains', value: 'DE AD be ef' }
    expect(toExpression(raw)).toEqual({ field: 'raw.bytes', op: 'contains', value: { type: 'bytes', value: [222, 173, 190, 239] } })
    expect(toExpression(fromExpression(toExpression(raw)))).toEqual({ all: [toExpression(raw)] })
    expect(() => toExpression({ ...raw, value: 'ABC' })).toThrow()
    expect(() => toExpression({ ...raw, value: 'AA'.repeat(1025) })).toThrow()
  })
  test('serializes typed boolean fields as boolean values', () => {
    expect(toExpression({ id: 'bool', kind: 'condition', field: 'plugin.valid', op: 'eq', value: 'true' }, [
      { value: 'plugin.valid', label: 'Valid', kind: 'boolean' },
    ])).toEqual({ field: 'plugin.valid', op: 'eq', value: { type: 'bool', value: true } })
  })
})

test('removes nested groups without changing siblings and keeps an editable last condition', async () => {
  const { createGroup, removeChild } = await import('./ruleBuilder')
  const root = createGroup()
  const sibling = root.children[0]!
  const nested = createGroup()
  root.children.push(nested)
  removeChild(root, nested.id)
  expect(root.children).toEqual([sibling])
  root.children = [createGroup()]
  removeChild(root, root.children[0]!.id)
  expect(root.children).toHaveLength(1)
  expect(root.children[0]!.kind).toBe('condition')
  expect(() => toExpression(root)).not.toThrow()
})
