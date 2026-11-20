import { ref } from 'vue'

export type Theme = 'dark' | 'oled'

const STORAGE_KEY = 'hydra-theme'
const theme = ref<Theme>('dark')

export function parseTheme(value: string | null | undefined): Theme {
  return value === 'oled' ? 'oled' : 'dark'
}

export function readTheme(storage: Pick<Storage, 'getItem'> | undefined): Theme {
  try {
    return parseTheme(storage?.getItem(STORAGE_KEY))
  } catch {
    return 'dark'
  }
}

export function persistTheme(value: Theme, storage: Pick<Storage, 'setItem'> | undefined): void {
  try {
    storage?.setItem(STORAGE_KEY, value)
  } catch {
    // The active theme still works when browser storage is unavailable.
  }
}

function applyTheme(value: Theme): void {
  theme.value = value
  if (typeof document !== 'undefined') document.documentElement.dataset.theme = value
}

function browserStorage(): Storage | undefined {
  try {
    return typeof window === 'undefined' ? undefined : window.localStorage
  } catch {
    return undefined
  }
}

// Called by main.ts before mounting, so the saved palette is applied before the first render.
export function initializeTheme(): void {
  applyTheme(readTheme(browserStorage()))
}

export function useTheme() {
  function toggleTheme() {
    const next = theme.value === 'dark' ? 'oled' : 'dark'
    applyTheme(next)
    persistTheme(next, browserStorage())
  }

  return { theme, toggleTheme }
}
