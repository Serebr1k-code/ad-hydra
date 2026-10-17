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
})
