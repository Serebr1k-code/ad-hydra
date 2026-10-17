import type { SubmitterConfig } from '@/generated/contracts'

export function pythonPackages(text: string): string[] {
  const values = text.split(/[\n,]+/).map(value => value.trim()).filter(Boolean)
  if (values.length > 32 || values.some(value => !/^[A-Za-z0-9][A-Za-z0-9._-]*(?:==[A-Za-z0-9][A-Za-z0-9.!+_-]*)?$/.test(value))) {
    throw new Error('Use up to 32 Python package names or name==version entries.')
  }
  return [...new Set(values)]
}
export function submitterDto(value: SubmitterConfig): SubmitterConfig {
  return {
    revision: value.revision, enabled: value.enabled, flag_pattern: value.flag_pattern,
    python_packages: [...(value.python_packages ?? [])], source: value.source, timeout_seconds: value.timeout_seconds,
  }
}
