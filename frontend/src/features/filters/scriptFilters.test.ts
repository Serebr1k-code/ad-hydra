import { describe, expect, test } from 'bun:test'
import { assignService, programIdFromName, scriptRows, setPolicyService, upsertPolicy, withoutPolicy, type ScriptPolicy } from './scriptFilters'

const policies: ScriptPolicy[] = [
  { id: 'header-audit', services: ['web'], timeout_ms: 50, allow_on_error: true },
  { id: 'orphan', services: ['web'], timeout_ms: 50, allow_on_error: false },
]
const programs = [{ id: 'header-audit', runtime: 'python', managed: false, source: 'def check(request):\n    return True\n' }]

describe('script filters', () => {
  test('slugs a name into a valid program id', () => {
    expect(programIdFromName('Header audit')).toBe('header-audit')
    expect(programIdFromName('  Slow  POST!! ')).toBe('slow-post')
    expect(programIdFromName('***')).toBe('script')
    expect(programIdFromName('x'.repeat(60))).toHaveLength(32)
    expect(programIdFromName('a'.repeat(31) + ' b')).toBe('a'.repeat(31))
  })

  test('builds rows only for policies that have a program', () => {
    expect(scriptRows(programs, policies)).toEqual([
      { id: 'header-audit', name: 'header-audit', source: programs[0]!.source, services: ['web'], timeoutMs: 50, allowOnError: true },
    ])
    expect(scriptRows([], policies)).toEqual([])
  })

  test('upserts a policy without duplicating ids', () => {
    const added = upsertPolicy(policies, { id: 'new', services: ['ssh'], timeout_ms: 25, allow_on_error: false })
    expect(added.map(policy => policy.id)).toEqual(['header-audit', 'orphan', 'new'])
    const replaced = upsertPolicy(policies, { id: 'orphan', services: ['dns'], timeout_ms: 25, allow_on_error: true })
    expect(replaced).toHaveLength(2)
    expect(replaced[1]).toEqual({ id: 'orphan', services: ['dns'], timeout_ms: 25, allow_on_error: true })
  })

  test('drops a policy', () => {
    expect(withoutPolicy(policies, 'orphan').map(policy => policy.id)).toEqual(['header-audit'])
  })

  test('enables and disables a script per service', () => {
    const on = setPolicyService(policies, 'header-audit', 'ssh', true)
    expect(on[0]!.services).toEqual(['web', 'ssh'])
    const off = setPolicyService(on, 'header-audit', 'web', false)
    expect(off[0]!.services).toEqual(['ssh'])
    expect(setPolicyService(policies, 'header-audit', 'web', true)[0]!.services).toEqual(['web'])
  })
  test('moving a service to another script takes it from the previous one', () => {
    const moved = assignService(policies, 'orphan', 'web')
    expect(moved.find(policy => policy.id === 'header-audit')!.services).toEqual([])
    expect(moved.find(policy => policy.id === 'orphan')!.services).toEqual(['web'])
  })
})
