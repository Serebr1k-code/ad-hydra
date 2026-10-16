import type { Page } from '@/generated/contracts'
export function record(value: unknown): Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {} }
export function text(value: unknown) { return typeof value === 'string' || typeof value === 'number' ? String(value) : '' }
export function direction(value: unknown) { return value === 1 ? 'Client → server' : value === 2 ? 'Server → client' : 'Unknown direction' }
export function timestamp(value: unknown) { return typeof value === 'number' && Number.isFinite(value) ? new Date(value).toISOString() : 'Unknown time' }
export function flowRows(page: Page) { return page.items.map(item => ({ ...item, flow_id: text(item.flow_id), service_id: text(item.service_id), client: `${text(record(item.client).ip)}:${text(record(item.client).port)}`, started: timestamp(item.occurred_ms) })) }
export function messageRows(page: Page) { return page.items.map(item => ({ ...item, message_id: text(item.message_id), occurred_ms: item.occurred_ms, direction: item.direction, message_type: text(item.message_type), protocol_id: text(item.protocol_id), fields: record(item.fields) })) }
export type Message = ReturnType<typeof messageRows>[number]
export function fields(message: Pick<Message, 'fields'>) { return Object.entries(message.fields).map(([key, value]) => ({ key, type: text(record(value).type), value: JSON.stringify(record(value).value) ?? '' })) }
export function differences(a: Pick<Message, 'fields'>, b: Pick<Message, 'fields'>) {
  return [...new Set([...Object.keys(a.fields), ...Object.keys(b.fields)])].sort().map(key => {
    const left = JSON.stringify(a.fields[key]), right = JSON.stringify(b.fields[key])
    return { key, left: left ?? 'Absent', right: right ?? 'Absent', change: left === right ? 'Unchanged' : left === undefined ? 'Added' : right === undefined ? 'Removed' : 'Changed' }
  })
}
