import { describe, expect, test } from 'bun:test'
import { parseTheme, persistTheme, readTheme } from './theme'

describe('theme preference', () => {
  test('defaults to dark and only accepts the OLED preference', () => {
    expect(parseTheme(null)).toBe('dark')
    expect(parseTheme('unknown')).toBe('dark')
    expect(parseTheme('oled')).toBe('oled')
  })

  test('reads and writes the browser preference', () => {
    const values = new Map<string, string>()
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    }
    expect(readTheme(storage)).toBe('dark')
    persistTheme('oled', storage)
    expect(readTheme(storage)).toBe('oled')
  })

  test('falls back safely when browser storage throws or is absent', () => {
    expect(readTheme(undefined)).toBe('dark')
    expect(readTheme({ getItem: () => { throw new Error('blocked') } })).toBe('dark')
    expect(() => persistTheme('oled', { setItem: () => { throw new Error('blocked') } })).not.toThrow()
  })
})
