export function date(value: unknown): string {
  if (typeof value !== 'number' && typeof value !== 'string') return '—'
  return new Date(typeof value === 'number' ? value * 1000 : value).toLocaleString()
}
