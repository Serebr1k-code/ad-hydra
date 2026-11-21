import { expect, test } from 'bun:test'
import { tlsDraft, tlsPayload, validateTls } from './tls'

test('a stored service without TLS stays plaintext', () => {
  expect(tlsDraft(null)).toBeNull()
  expect(tlsPayload(null)).toBeNull()
  expect(validateTls(null)).toBe('')
})

test('the stored key is never sent back and an empty key keeps it', () => {
  const draft = tlsDraft({ terminate: true, certificate_pem: '-----BEGIN CERTIFICATE-----x', private_key_configured: true })!
  expect(draft.private_key_pem).toBe('')
  expect(validateTls(draft)).toBe('')
  expect(tlsPayload(draft)).toEqual({
    terminate: true, upstream: false, insecure: false,
    certificate_pem: '-----BEGIN CERTIFICATE-----x', private_key_pem: null, server_name: null,
  })
})

test('termination needs a certificate chain and a key', () => {
  const draft = tlsDraft({ terminate: true })!
  expect(validateTls(draft)).toContain('certificate chain')
  draft.certificate_pem = '-----BEGIN CERTIFICATE-----x'
  expect(validateTls(draft)).toContain('private key')
  draft.private_key_pem = '-----BEGIN PRIVATE KEY-----x'
  expect(validateTls(draft)).toBe('')
})

test('upstream-only TLS sends the verification flag and server name', () => {
  const draft = tlsDraft({ upstream: true, insecure: true, server_name: 'svc.local' })!
  expect(tlsPayload(draft)).toEqual({
    terminate: false, upstream: true, insecure: true,
    certificate_pem: null, private_key_pem: null, server_name: 'svc.local',
  })
})
