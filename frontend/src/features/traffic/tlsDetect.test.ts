import { expect, test } from 'bun:test'
import { looksLikeTls } from './tlsDetect'

const hex = (text: string) => ({ id: 'a', direction: 1, text, kind: 'hex' as const })

test('a TLS client hello in hex spans is detected', () => {
  expect(looksLikeTls([hex('[16 03 01 00 a5 01 00')])).toBe(true)
  expect(looksLikeTls([hex('[16 03 03 00 7a 02')])).toBe(true)
})

test('plaintext and truncated spans are not TLS', () => {
  expect(looksLikeTls([{ id: 'a', direction: 1, text: 'GET / HTTP/1.1' }])).toBe(false)
  expect(looksLikeTls([hex('[16 03]')])).toBe(false)
  expect(looksLikeTls([hex('[47 45 54]')])).toBe(false)
  expect(looksLikeTls([])).toBe(false)
})

test('a text chunk carrying a raw handshake byte is detected', () => {
  expect(looksLikeTls([{ id: 'a', direction: 1, text: '\u0016\u0003\u0001rest' }])).toBe(true)
})

test('only the client direction counts', () => {
  expect(looksLikeTls([{ id: 's', direction: 2, text: '[16 03 03 00]', kind: 'hex' }])).toBe(false)
})
