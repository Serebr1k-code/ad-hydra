/** TLS draft for the service form. The stored private key never reaches the browser. */
export interface TlsDraft {
  terminate: boolean
  upstream: boolean
  insecure: boolean
  certificate_pem: string
  private_key_pem: string
  server_name: string
  /** True when the saved service already holds a key this form did not receive. */
  private_key_configured: boolean
}

export interface StoredTls {
  terminate?: boolean
  upstream?: boolean
  insecure?: boolean
  certificate_pem?: string | null
  server_name?: string | null
  private_key_configured?: boolean
}

export function tlsDraft(stored?: StoredTls | null): TlsDraft | null {
  if (!stored) return null
  return {
    terminate: stored.terminate === true,
    upstream: stored.upstream === true,
    insecure: stored.insecure === true,
    certificate_pem: stored.certificate_pem ?? '',
    private_key_pem: '',
    server_name: stored.server_name ?? '',
    private_key_configured: stored.private_key_configured === true,
  }
}

/** Empty strings mean "unchanged", matching the API contract for an omitted key. */
export function tlsPayload(draft: TlsDraft | null) {
  if (!draft || (!draft.terminate && !draft.upstream)) return null
  return {
    terminate: draft.terminate,
    upstream: draft.upstream,
    insecure: draft.upstream && draft.insecure,
    certificate_pem: draft.terminate ? draft.certificate_pem.trim() : null,
    private_key_pem: draft.terminate ? draft.private_key_pem.trim() || null : null,
    server_name: draft.upstream ? draft.server_name.trim() || null : null,
  }
}

export function validateTls(draft: TlsDraft | null): string {
  if (!draft || (!draft.terminate && !draft.upstream)) return ''
  if (draft.terminate) {
    if (!draft.certificate_pem.trim()) return 'Paste the PEM certificate chain for TLS termination.'
    if (!draft.certificate_pem.includes('-----BEGIN CERTIFICATE-----'))
      return 'The TLS certificate must be a PEM certificate chain.'
    if (!draft.private_key_pem.trim() && !draft.private_key_configured)
      return 'Paste the PEM private key for TLS termination.'
    if (draft.private_key_pem.trim() && !draft.private_key_pem.includes('PRIVATE KEY-----'))
      return 'The TLS private key must be a PEM private key.'
  }
  return ''
}
