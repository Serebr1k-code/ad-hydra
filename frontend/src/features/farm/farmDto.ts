import type { Exploit, FarmService, FarmTeam } from '@/generated/contracts'

// Read responses include provenance. PUT contracts accept only mutable DTO fields.
export function exploitDto(value: Exploit): Exploit {
  const manifest = value.manifest
  return {
    id: value.id, name: value.name, version: value.version, enabled: value.enabled,
    revision: value.revision, service_id: value.service_id ?? null,
    target_ids: [...(value.target_ids ?? [])], team_ids: [...(value.team_ids ?? [])],
    interval_seconds: value.interval_seconds ?? null,
    manifest: {
      runtime: manifest.runtime, mode: manifest.mode,
      timeout_seconds: manifest.timeout_seconds, memory_mib: manifest.memory_mib,
      cpu_percent: manifest.cpu_percent, python_packages: [...(manifest.python_packages ?? [])],
    },
  }
}
export function serviceDto(value: FarmService): FarmService {
  return { id: value.id, name: value.name, port: value.port, transport: value.transport ?? 'tcp', enabled: value.enabled ?? true, revision: value.revision ?? 0 }
}
export function teamDto(value: FarmTeam): FarmTeam {
  return { id: value.id, name: value.name, host: value.host, enabled: value.enabled ?? true, revision: value.revision ?? 0 }
}
