type Endpoint = { host: string; port: number; wildcard: boolean; loopback: boolean }

function endpoint(value: string): Endpoint | null {
  const match = /^(\[[0-9a-fA-F:.]+\]|\d+\.\d+\.\d+\.\d+):(\d+)$/.exec(value.trim())
  if (!match) return null
  const port = Number(match[2])
  if (port < 1 || port > 65535) return null
  const host = match[1]
  if (!host.startsWith('[') && host.split('.').some(part => Number(part) > 255 || (part.length > 1 && part.startsWith('0')))) return null
  try {
    const normalized = new URL(`http://${host}:${port}`).hostname
    return { host: normalized, port, wildcard: normalized === '0.0.0.0' || normalized === '[::]', loopback: normalized.startsWith('127.') || normalized === '[::1]' }
  } catch { return null }
}

export function validateEndpoints(listenAddress: string, upstreamAddress: string): string {
  const listener = endpoint(listenAddress)
  const backend = endpoint(upstreamAddress)
  if (!listener || !backend) return 'Enter an IP address and port from 1 to 65535, such as 0.0.0.0:8000 or [::1]:18000.'
  if (backend.wildcard) return 'The backend needs a concrete address, usually 127.0.0.1 with its relocated port.'
  if (listener.port === backend.port && (listener.host === backend.host || (listener.wildcard && backend.loopback))) {
    return 'Listener and backend must use different endpoints. Move the local app to a separate private port first.'
  }
  return ''
}
