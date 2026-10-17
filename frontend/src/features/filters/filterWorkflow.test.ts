import { expect, test } from 'bun:test'
import { testSavedFilter } from './filterWorkflow'

test('rehearses the current saved rule before enforcing and sends only writable fields', async () => {
  const calls: Array<[string, string | undefined, unknown]> = []
  const record = { id: 'r', desired_revision: 7, created_by: 'server', service_id: 'docs', name: 'curl', description: '', enabled: true, mode: 'shadow', priority: 10, expression: { all: [] }, action: { type: 'block', http_status: 402 }, backend: 'userspace', expires_at_ms: null }
  const call = async (path: string, method?: string, value?: unknown) => {
    calls.push([path, method, value])
    return path.endsWith('/test') ? { matched: true } : record
  }
  expect(await testSavedFilter(call, 'r', 'm', true)).toEqual({ matched: true, enforced: true })
  expect(calls.map(([path]) => path)).toEqual(['/filters/r', '/filters/r/test', '/filters/r'])
  expect(calls[2]![2]).toEqual({ service_id: 'docs', name: 'curl', description: '', enabled: true, priority: 10, expression: { all: [] }, action: { type: 'block', http_status: 402 }, backend: 'userspace', expires_at_ms: null, mode: 'enforce', expected_revision: 7 })
})

test('no-match and test-only operations never enforce', async () => {
  for (const [matched, enforce] of [[false, true], [true, false]]) {
    const mutations: string[] = []
    const call = async (path: string, method?: string) => {
      if (method) mutations.push(method)
      return path.endsWith('/test') ? { matched } : { desired_revision: 9 }
    }
    expect(await testSavedFilter(call, 'r', 'm', enforce!)).toEqual({ matched, enforced: false })
    expect(mutations).toEqual(['POST'])
  }
})

test('a failed rehearsal never sends an enforce request', async () => {
  const methods: string[] = []
  const call = async (_path: string, method?: string) => {
    if (method) methods.push(method)
    if (method === 'POST') throw new Error('message expired')
    return { desired_revision: 7 }
  }
  await expect(testSavedFilter(call, 'r', 'missing', true)).rejects.toThrow('message expired')
  expect(methods).toEqual(['POST'])
})

test('direct enforcement needs no saved message or rehearsal', async () => {
  const { enforceSavedFilter } = await import('./filterWorkflow')
  const calls: Array<[string, string | undefined, unknown]> = []
  await enforceSavedFilter(async (path, method, value) => {
    calls.push([path, method, value])
    return { desired_revision: 8, service_id: 'docs', enabled: true, mode: 'shadow', created_by: 'server' }
  }, 'r')
  expect(calls.map(([path, method]) => [path, method])).toEqual([['/filters/r', undefined], ['/filters/r', 'PATCH']])
  expect(calls[1]![2]).toMatchObject({ expected_revision: 8, mode: 'enforce', enabled: true })
  expect(calls[1]![2]).not.toHaveProperty('created_by')
})
