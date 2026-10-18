export const servicePalette = [
  { name: 'Red', value: '#ff6b6b' }, { name: 'Orange', value: '#ffa94d' },
  { name: 'Gold', value: '#ffe066' }, { name: 'Lime', value: '#a9e34b' },
  { name: 'Green', value: '#51cf66' }, { name: 'Teal', value: '#20c997' },
  { name: 'Cyan', value: '#66d9e8' }, { name: 'Blue', value: '#4dabf7' },
  { name: 'Indigo', value: '#748ffc' }, { name: 'Purple', value: '#b197fc' },
  { name: 'Pink', value: '#f783ac' }, { name: 'Silver', value: '#ced4da' },
] as const
export function validServiceColor(value: unknown): value is string { return typeof value === 'string' && /^#[0-9a-f]{6}$/i.test(value) }
export function defaultServiceColor(id: string): string {
  let hash = 2166136261
  for (const character of id) hash = Math.imul(hash ^ character.charCodeAt(0), 16777619) >>> 0
  return servicePalette[hash % servicePalette.length]!.value
}
/** Give unconfigured services distinct presets until the palette is exhausted. */
export function resolveServiceColors(services: { id: string; color?: string | null }[]): Record<string, string> {
  const colors: Record<string, string> = {}, used = new Set<string>()
  for (const service of services) if (validServiceColor(service.color)) { colors[service.id] = service.color.toLowerCase(); used.add(colors[service.id]!) }
  for (const service of [...services].sort((a, b) => a.id.localeCompare(b.id))) {
    if (colors[service.id]) continue
    const preferred = defaultServiceColor(service.id)
    const color = !used.has(preferred) ? preferred : servicePalette.find(item => !used.has(item.value))?.value ?? preferred
    colors[service.id] = color; used.add(color)
  }
  return colors
}
