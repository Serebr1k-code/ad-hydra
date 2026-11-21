export interface FlagPattern {
  regex: RegExp | null
  status: string
}

const OFF = 'Flag highlighting off; no flag pattern configured.'
const UNSUPPORTED = 'Flag pattern is unsupported for safe highlighting; backend detection is unchanged.'

/**
 * JavaScript regexes can backtrack exponentially, unlike the Rust regex used by Core.
 * Highlight only a deliberately small, linear-safe subset: concatenated literals,
 * escapes, character classes, anchors, and at most one finite quantifier.
 */
export function compileFlagPattern(pattern?: string | null): FlagPattern {
  const source = pattern?.trim() ?? ''
  if (!source) return { regex: null, status: OFF }
  if (!isSafePattern(source)) return { regex: null, status: UNSUPPORTED }
  try {
    return { regex: new RegExp(source, 'g'), status: 'Flags highlighted in the recent bounded stream.' }
  } catch {
    return { regex: null, status: UNSUPPORTED }
  }
}

function isSafePattern(source: string): boolean {
  if (source.length > 256) return false
  let quantifiers = 0
  let previousAtom = false
  for (let i = 0; i < source.length; i++) {
    const char = source[i]!
    if (char === '[') {
      let closed = false
      for (i++; i < source.length; i++) {
        if (source[i] === '\\') { i++; continue }
        if (source[i] === ']') { closed = true; break }
      }
      if (!closed) return false
      previousAtom = true
      continue
    }
    if (char === '\\') {
      const escaped = source[++i]
      if (!escaped || /[1-9kPp]/.test(escaped)) return false
      if (escaped === 'x') {
        if (!/^[0-9a-fA-F]{2}$/.test(source.slice(i + 1, i + 3))) return false
        i += 2
      } else if (escaped === 'u') {
        if (!/^[0-9a-fA-F]{4}$/.test(source.slice(i + 1, i + 5))) return false
        i += 4
      }
      previousAtom = true
      continue
    }
    if (char === '(' || char === ')' || char === '|') return false
    if (char === '^') {
      if (i !== 0) return false
      previousAtom = false
      continue
    }
    if (char === '$') {
      if (i !== source.length - 1) return false
      previousAtom = false
      continue
    }
    if (char === '*' || char === '+') return false
    if (char === '?') {
      if (!previousAtom || ++quantifiers > 1) return false
      previousAtom = false
      continue
    }
    if (char === '{') {
      const quantifier = /^\{(\d+)(?:,(\d+))?\}/.exec(source.slice(i))
      if (!previousAtom || !quantifier || ++quantifiers > 1) return false
      const minimum = Number(quantifier[1])
      const maximum = Number(quantifier[2] ?? quantifier[1])
      if (minimum > maximum || maximum > 256) return false
      i += quantifier[0].length - 1
      previousAtom = false
      continue
    }
    if (char === '}') return false
    previousAtom = true
  }
  return quantifiers <= 1
}
