import { describe, expect, test } from 'bun:test'
import { exploitDto, serviceDto, teamDto } from './farmDto'

describe('farm mutation DTO projection', () => {
  const provenance = { created_at: 100, updated_at: 200, author_id: 'server-user', initiated_by: 'server-user' }
  test('edited exploit and manifest exclude read-only metadata and copy selections', () => {
    const read = {
      ...provenance, id: 'exploit', name: 'Example', version: 'v1', enabled: true, revision: 3,
      service_id: 'svc', target_ids: ['legacy'], team_ids: ['team'], interval_seconds: 30,
      manifest: { runtime: 'python', mode: 'per_target', timeout_seconds: 30, memory_mib: 64, cpu_percent: 50, python_packages: ['requests'], ...provenance },
    }
    const result = exploitDto(read)
    expect(result).toEqual({ id: 'exploit', name: 'Example', version: 'v1', enabled: true, revision: 3, service_id: 'svc', target_ids: ['legacy'], team_ids: ['team'], interval_seconds: 30, manifest: { runtime: 'python', mode: 'per_target', timeout_seconds: 30, memory_mib: 64, cpu_percent: 50, python_packages: ['requests'] } })
    expect(result.target_ids).not.toBe(read.target_ids)
    expect(result.team_ids).not.toBe(read.team_ids)
    expect(result.manifest.python_packages).not.toBe(read.manifest.python_packages)
  })
  test('catalog toggles remove provenance while preserving revision and disabled state', () => {
    const service = { ...provenance, id: 'svc', name: 'HTTP', port: 8080, transport: 'udp', enabled: false, revision: 4 }
    const team = { ...provenance, id: 'team', name: 'Team', host: '10.0.0.2', enabled: false, revision: 8 }
    expect(serviceDto(service)).toEqual({ id: 'svc', name: 'HTTP', port: 8080, transport: 'udp', enabled: false, revision: 4 })
    expect(teamDto(team)).toEqual({ id: 'team', name: 'Team', host: '10.0.0.2', enabled: false, revision: 8 })
  })
  test('omitted compatibility defaults are explicit', () => {
    expect(serviceDto({ id: 'svc', name: 'S', port: 80 })).toEqual({ id: 'svc', name: 'S', port: 80, transport: 'tcp', enabled: true, revision: 0 })
    expect(teamDto({ id: 'team', name: 'T', host: '::1' })).toEqual({ id: 'team', name: 'T', host: '::1', enabled: true, revision: 0 })
  })
})
