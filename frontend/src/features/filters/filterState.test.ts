import { expect, test } from 'bun:test'
import { filterApplication, setFilterEnabled } from './filterState'

const enabled = { desired_enabled: true, applied_enabled: true, desired_revision: 9, applied_revision: 9 }

test('enabled desired state is pending until the same revision is acknowledged', () => {
  expect(filterApplication({ ...enabled, applied_enabled: null, applied_revision: 8 }, 'pending')).toEqual({ state: 'pending', label: 'Pending proxy acknowledgement' })
  expect(filterApplication({ ...enabled, applied_revision: 8 }, 'applied').state).toBe('pending')
  expect(filterApplication(enabled, 'applied')).toEqual({ state: 'enabled', label: 'Applied: enabled' })
})

test('a proxy error overrides previously acknowledged state without calling it active', () => {
  expect(filterApplication(enabled, 'error')).toEqual({ state: 'error', label: 'Application error' })
  expect(filterApplication({ ...enabled, desired_enabled: false, applied_enabled: false }, 'applied')).toEqual({ state: 'disabled', label: 'Applied: disabled' })
  expect(filterApplication({ ...enabled, applied_enabled: false }, 'applied').state).toBe('pending')
})

test('switch writes the explicit desired value with the displayed optimistic revision', async () => {
  const calls: unknown[] = []
  await setFilterEnabled(async (...args) => { calls.push(args) }, 'rule / one', false, 9)
  expect(calls).toEqual([['/filters/rule%20%2F%20one/state', 'PUT', { enabled: false, expected_revision: 9 }]])
})

test('switch failure is reported without retrying a stale revision or doing optional testing', async () => {
  let calls = 0
  await expect(setFilterEnabled(async () => { calls++; throw new Error('revision conflict') }, 'one', true, 8)).rejects.toThrow('revision conflict')
  expect(calls).toBe(1)
})
