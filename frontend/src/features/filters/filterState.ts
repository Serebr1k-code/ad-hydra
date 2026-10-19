import type { FilterRecord } from './FilterEditorDialog.vue'

export interface FilterApplicationState {
  desired_revision: number
  applied_revision: number
  desired_enabled: boolean
  applied_enabled: boolean | null
}
export type FilterRule = FilterRecord & FilterApplicationState & { updated_at: string; updated_by: string }
export type ApplicationMark = { state: 'enabled' | 'disabled' | 'pending' | 'error'; label: string }

/** Desired configuration alone never establishes that a rule was applied. */
export function filterApplication(filter: FilterApplicationState, applyStatus: string): ApplicationMark {
  if (applyStatus === 'error') return { state: 'error', label: 'Application error' }
  if (applyStatus !== 'applied' || filter.applied_enabled === null || filter.applied_revision !== filter.desired_revision || filter.applied_enabled !== filter.desired_enabled) {
    return { state: 'pending', label: 'Pending proxy acknowledgement' }
  }
  return filter.applied_enabled ? { state: 'enabled', label: 'Applied: enabled' } : { state: 'disabled', label: 'Applied: disabled' }
}

/** Send an explicit state so a stale click cannot invert a newer desired value. */
export async function setFilterEnabled(call: (path: string, method: string, value: unknown) => Promise<unknown>, id: string, enabled: boolean, revision: number): Promise<void> {
  await call(`/filters/${encodeURIComponent(id)}/state`, 'PUT', { enabled, expected_revision: revision })
}
