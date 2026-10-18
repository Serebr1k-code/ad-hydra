export interface StreamChoice { flow_id: string; service_id: string }
export interface StreamSelection { service_id: string; flow_ids: string[] }
export const MAX_SELECTED_STREAMS = 50

export function toggleStream(current: StreamSelection | null, flow: StreamChoice, checked: boolean): StreamSelection | null {
  if (checked) {
    if (current && current.service_id !== flow.service_id) return current
    const flow_ids = current?.flow_ids ?? []
    if (flow_ids.includes(flow.flow_id) || flow_ids.length >= MAX_SELECTED_STREAMS) return current
    return { service_id: flow.service_id, flow_ids: [...flow_ids, flow.flow_id] }
  }
  if (!current || current.service_id !== flow.service_id) return current
  const flow_ids = current.flow_ids.filter(id => id !== flow.flow_id)
  return flow_ids.length ? { service_id: current.service_id, flow_ids } : null
}

export function parseStreamSelection(value: unknown): StreamSelection | null {
  if (!value || typeof value !== 'object') return null
  const input = value as Record<string, unknown>
  if (typeof input.service_id !== 'string' || !input.service_id || input.service_id.length > 128 || !Array.isArray(input.flow_ids)) return null
  const flow_ids = [...new Set(input.flow_ids.filter((id): id is string => typeof id === 'string' && id.length > 0 && id.length <= 128))].slice(0, MAX_SELECTED_STREAMS)
  return flow_ids.length ? { service_id: input.service_id, flow_ids } : null
}
