import type { FilterInput } from '@/generated/contracts'

type Call = (path: string, method?: string, value?: unknown) => Promise<unknown>
type SavedFilter = Omit<FilterInput, 'expected_revision'> & { desired_revision: number }

/** Read the saved revision before testing so concurrent edits cannot be enforced. */
export async function testSavedFilter(call: Call, id: string, messageId: string, enforce: boolean) {
  const path = `/filters/${encodeURIComponent(id)}`
  const saved = await call(path) as SavedFilter
  const tested = await call(`${path}/test`, 'POST', { message_id: messageId }) as { matched: boolean }
  if (!enforce || !tested.matched) return { matched: tested.matched, enforced: false }
  await writeEnforce(call, path, saved)
  return { matched: true, enforced: true }
}

/** Direct activation is independent of optional saved-message diagnostics. */
export async function enforceSavedFilter(call: Call, id: string) {
  const path = `/filters/${encodeURIComponent(id)}`
  await writeEnforce(call, path, await call(path) as SavedFilter)
}

async function writeEnforce(call: Call, path: string, saved: SavedFilter) {
  const { service_id, additional_service_ids, name, description, enabled, priority, expression, action, backend, expires_at_ms } = saved
  await call(path, 'PATCH', {
    service_id, additional_service_ids, name, description, enabled, priority, expression, action, backend, expires_at_ms,
    mode: 'enforce', expected_revision: saved.desired_revision,
  })
}
