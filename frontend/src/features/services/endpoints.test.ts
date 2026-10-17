import { expect, test } from 'bun:test'
import { validateEndpoints } from './endpoints'

test('public default port forwards to relocated private backend', () => {
  expect(validateEndpoints('0.0.0.0:8000', '127.0.0.1:18000')).toBe('')
  expect(validateEndpoints('[::]:8000', '[::1]:18000')).toBe('')
  expect(validateEndpoints('0.0.0.0:8000', '192.0.2.10:8000')).toBe('')
})

test('rejects identical and wildcard-to-loopback self routes', () => {
  expect(validateEndpoints('127.0.0.1:8000', '127.0.0.1:8000')).toContain('different')
  expect(validateEndpoints('0.0.0.0:8000', '127.0.0.1:8000')).toContain('different')
  expect(validateEndpoints('[::]:8000', '[0:0:0:0:0:0:0:1]:8000')).toContain('different')
})

test('requires concrete IP endpoints and nonzero valid ports', () => {
  for (const invalid of ['localhost:8000', '127.0.0.1:0', '127.0.0.1:65536', '127.1:8000', '127.0.0.1:8000/path']) {
    expect(validateEndpoints(invalid, '127.0.0.1:18000')).not.toBe('')
  }
  expect(validateEndpoints('0.0.0.0:8000', '0.0.0.0:18000')).not.toBe('')
})
