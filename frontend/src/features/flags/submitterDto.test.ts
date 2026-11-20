import { expect, test } from 'bun:test'
import { pythonPackages, submitterDto } from './submitterDto'

test('submitter updates omit read-only counters and preserve source verbatim', () => {
  const config = { revision: 4, enabled: false, flag_pattern: 'FLAG\\{[^}]+\\}', python_packages: ['requests'], source: 'import sys\nprint(len(sys.argv[1:]))\n', timeout_seconds: 20, author_id: 'server-user', updated_at: 123, queued_flags: 12 }
  const result = submitterDto(config)
  expect(result).toEqual({ mode: 'custom', batch_size: 240, revision: 4, enabled: false, flag_pattern: 'FLAG\\{[^}]+\\}', python_packages: ['requests'], source: config.source, timeout_seconds: 20, forcad_ip: '', forcad_port: 8080, team_token: '', team_token_configured: false })
  expect(result.python_packages).not.toBe(config.python_packages)
})
test('new submitter presets to paused ForcAD and keeps the write-only secret blank', () => {
  const config = { mode: 'forcad', batch_size: 100, revision: 0, enabled: false, flag_pattern: '[A-Z0-9]{31}=', python_packages: [], source: '', timeout_seconds: 20, forcad_ip: '', forcad_port: 8080, team_token_configured: false }
  expect(submitterDto(config)).toEqual({ mode: 'forcad', batch_size: 100, revision: 0, enabled: false, flag_pattern: '[A-Z0-9]{31}=', python_packages: [], source: '', timeout_seconds: 20, forcad_ip: '', forcad_port: 8080, team_token: '', team_token_configured: false })
})
test('submitter accepts optional unpinned and pinned Python dependencies', () => {
  expect(pythonPackages(' requests\nhttpx==0.28.1, requests ')).toEqual(['requests', 'httpx==0.28.1'])
  expect(pythonPackages('')).toEqual([])
  expect(() => pythonPackages('https://example.invalid/payload.whl')).toThrow()
  expect(() => pythonPackages('requests; python_version > "3"')).toThrow()
  expect(() => pythonPackages(Array.from({ length: 33 }, (_, n) => `package${n}`).join('\n'))).toThrow()
})
