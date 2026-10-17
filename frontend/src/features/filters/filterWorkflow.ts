import type { FilterInput } from '@/generated/contracts'

type Call = (path: string, method?: string, value?: unknown) => Promise<unknown>
type SavedFilter = Omit<FilterInput, 'expected_revision'> & { desired_revision: number }

/** Read the saved revision before testing so concurrent edits cannot be enforced. */
export async function testSavedFilter(call: Call, id: string, messageId: string, enforce: boolean) {
  const path = `/filters/${encodeURIComponent(id)}`
  const saved = await call(path) as SavedFilter
  const tested = await call(`${path}/test`, 'POST', { message_id: messageId }) as { matched: boolean }
  if (!enforce || !tested.matched) return { matched: tested.matched, enforced: false }
  const { service_id, name, description, enabled, priority, expression, action, backend, expires_at_ms } = saved
  await call(path, 'PATCH', {
    service_id, name, description, enabled, priority, expression, action, backend, expires_at_ms,
    mode: 'enforce', expected_revision: saved.desired_revision,
  })
  return { matched: true, enforced: true }
}
