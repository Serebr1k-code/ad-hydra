import type { TerminalChunk } from './terminal'

/**
 * A TLS record starts with a content type (0x14-0x17), the 0x03 major version and a
 * minor version up to 0x04 (TLS 1.3 records still carry 0x03 0x03).
 */
export function looksLikeTls(chunks: TerminalChunk[]): boolean {
  const first = chunks.find(chunk => chunk.direction === 1)
  if (!first) return false
  const bytes = firstBytes(first)
  if (bytes.length < 3) return false
  const [type, major, minor] = bytes
  return type >= 0x14 && type <= 0x17 && major === 0x03 && minor <= 0x04
}

/** The terminal renders invalid UTF-8 as a bracketed hex span, so parse that form too. */
function firstBytes(chunk: TerminalChunk): number[] {
  if (chunk.kind === 'hex') {
    return chunk.text
      .replace(/^\[|\]$/g, '')
      .split(/\s+/)
      .slice(0, 3)
      .map(value => Number.parseInt(value, 16))
      .filter(value => Number.isFinite(value))
  }
  return Array.from(chunk.text).slice(0, 3).map(char => char.charCodeAt(0))
}
