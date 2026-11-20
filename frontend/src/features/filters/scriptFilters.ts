/** Script filters ride on the sandboxed request-checker runtime: a program with
 * `check(request: bytes) -> bool` (False blocks) plus a per-service policy. */
export interface ScriptProgram { id: string; runtime: string; managed: boolean; source: string }
export interface ScriptPolicy { id: string; services: string[]; timeout_ms: number; allow_on_error: boolean }
export interface ScriptRow { id: string; name: string; source: string; services: string[]; timeoutMs: number; allowOnError: boolean }

export const SCRIPT_TEMPLATE = `def check(request: bytes) -> bool:
    """Return False to block, True to let the request through."""
    if b"../" in request:
        return False
    return True
`

/** Program ids are limited to 32 chars of [A-Za-z0-9_-]. */
export function programIdFromName(name: string): string {
  const slug = name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
  return (slug || 'script').slice(0, 32).replace(/-+$/, '')
}

export function scriptRows(programs: ScriptProgram[], policies: ScriptPolicy[]): ScriptRow[] {
  const byId = new Map(programs.map(program => [program.id, program]))
  return policies.flatMap(policy => {
    const program = byId.get(policy.id)
    if (!program) return []
    return [{
      id: policy.id,
      name: policy.id,
      source: program.source,
      services: policy.services,
      timeoutMs: policy.timeout_ms,
      allowOnError: policy.allow_on_error,
    }]
  })
}

export function upsertPolicy(policies: ScriptPolicy[], row: ScriptPolicy): ScriptPolicy[] {
  const index = policies.findIndex(policy => policy.id === row.id)
  return index < 0 ? [...policies, row] : policies.map((policy, i) => (i === index ? row : policy))
}

export function withoutPolicy(policies: ScriptPolicy[], id: string): ScriptPolicy[] {
  return policies.filter(policy => policy.id !== id)
}

/** Enabling or disabling a script means adding or dropping it for one service. */
export function setPolicyService(policies: ScriptPolicy[], id: string, serviceId: string, enabled: boolean): ScriptPolicy[] {
  return policies.map(policy => {
    if (policy.id !== id) return policy
    const services = enabled
      ? [...new Set([...policy.services, serviceId])]
      : policy.services.filter(service => service !== serviceId)
    return { ...policy, services }
  })
}

/** A service runs at most one checker, so assigning one takes it from the others. */
export function assignService(policies: ScriptPolicy[], id: string, serviceId: string): ScriptPolicy[] {
  return policies.map(policy => policy.id === id
    ? { ...policy, services: [...new Set([...policy.services, serviceId])] }
    : { ...policy, services: policy.services.filter(service => service !== serviceId) })
}
