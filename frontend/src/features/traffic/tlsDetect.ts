import type { TerminalChunk } from './terminal'

/**
 * A TLS record starts with a content type (0x14-0x17), the 0x03 major version and a
 * minor version up to 0x04 (TLS 1.3 records still carry 0x03 0x03).
 */
export function looksLikeTls(chunks: TerminalChunk[]): boolean {
  const bytes: number[] = []
  for (const chunk of chunks) {
    if (chunk.direction !== 1) continue
    bytes.push(...chunkBytes(chunk).slice(0, 3 - bytes.length))
    if (bytes.length >= 3) break
  }
  if (bytes.length < 3) return false
  const [type, major, minor] = bytes
  return type >= 0x14 && type <= 0x17 && major === 0x03 && minor <= 0x04
}

/** The terminal renders invalid UTF-8 as a bracketed hex span, so parse that form too. */
function chunkBytes(chunk: TerminalChunk): number[] {
  if (chunk.kind === 'hex') {
    const span = /^\[([0-9a-fA-F]{2}(?:\s+[0-9a-fA-F]{2})*)\]?$/.exec(chunk.text)
    return span ? span[1]!.split(/\s+/).map(value => Number.parseInt(value, 16)) : []
  }
  return Array.from(chunk.text, char => char.charCodeAt(0)).filter(value => value <= 0xff)
}
