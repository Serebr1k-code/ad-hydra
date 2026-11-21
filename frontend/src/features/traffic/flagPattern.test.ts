import { expect, test } from 'bun:test'
import { compileFlagPattern } from './flagPattern'

test('safe highlighting accepts the ForcAD pattern and common literal/class flag formats', () => {
  expect(compileFlagPattern('[A-Z0-9]{31}=').regex).not.toBeNull()
  expect(compileFlagPattern('FLAG\\{[A-Z0-9]{1,64}\\}').regex).not.toBeNull()
})

test('safe highlighting disables unsupported backtracking expressions with a clear status', () => {
  for (const pattern of ['^(a+)+$', 'a+b', '.*FLAG', '[a-z]{1,}']) {
    const result = compileFlagPattern(pattern)
    expect(result.regex).toBeNull()
    expect(result.status).toContain('unsupported')
  }
})

test('nullable configuration disables highlighting without an error', () => {
  expect(compileFlagPattern(null)).toEqual({ regex: null, status: 'Flag highlighting off; no flag pattern configured.' })
  expect(compileFlagPattern('')).toEqual({ regex: null, status: 'Flag highlighting off; no flag pattern configured.' })
})
