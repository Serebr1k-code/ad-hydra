export function normalizeApiBaseUrl(value?: string): string {
  return (value?.trim() || '/api').replace(/\/+$/, '')
}
export const apiBaseUrl = normalizeApiBaseUrl(import.meta.env?.VITE_API_BASE_URL)
