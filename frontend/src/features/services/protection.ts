export interface ServiceProtection {
  max_active_flows: number
  tcp_connections_per_second: number
  udp_datagrams_per_second: number
  bytes_per_second: number
  idle_timeout_ms: number
}
export type ProtectionService = { protection?: ServiceProtection | null }
export const recommendedProtection = (): ServiceProtection => ({max_active_flows:128,tcp_connections_per_second:50,udp_datagrams_per_second:1000,bytes_per_second:8388608,idle_timeout_ms:30000})
export function validateProtection(value: ServiceProtection | null): string {
  if (!value) return ''
  for (const [field, number] of Object.entries(value)) {
    if (!Number.isSafeInteger(number) || number < (field === 'max_active_flows' || field === 'idle_timeout_ms' ? 1 : 0)) return `${field.replaceAll('_',' ')} must be a ${field === 'max_active_flows' || field === 'idle_timeout_ms' ? 'positive' : 'non-negative'} safe integer.`
  }
  return ''
}
