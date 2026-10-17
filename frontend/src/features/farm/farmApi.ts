import { api } from '@/shared/api/client'
import type { FarmPage } from './types'
// Catalogs must include every page so saved team selections never disappear.
export async function catalog<T>(kind: 'services' | 'teams'): Promise<T[]> {
  const items: T[] = []
  let after = ''
  for (let page = 0; page < 100; page++) {
    const result = await api<FarmPage<T>>(`/farm/${kind}${after ? `?after=${encodeURIComponent(after)}` : ''}`)
    items.push(...result.items)
    if (result.items.length < 100 || !result.next_cursor) return items
    if (result.next_cursor === after) throw new Error('Catalog pagination did not advance. Refresh the farm.')
    after = result.next_cursor
  }
  throw new Error('Catalog exceeds the supported 10,000 entries. Reduce it before editing the farm.')
}
