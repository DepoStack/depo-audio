// Shared token resolution for design/tokens.json (DTCG format), used by
// scripts/build-tokens.mjs and the brand token tests.
//
// Three tiers:
//   ref      canonical DepoStack primitives (exact hex from depo-stack
//            tokens/dist/depostack.tokens.json)
//   base     mode-independent roles (font stacks, type roles) → :root as --ds-*
//   semantic the app's role names per theme (dark → :root, light → .light)
//
// Alias tokens reference other tokens as {dot.path}. Every value is plain DTCG:
// colors are a {reference} or a #RRGGBB hex, dimensions are {value, unit},
// font weights are numbers. App-specific facts live in
// $extensions["com.depoaudio"]:
//   { alpha }           the color painted at that opacity
//   { alpha, over }     the color at that opacity composited onto the `over`
//                       color (the hairline on the board) into one opaque value,
//                       so Tailwind opacity modifiers still work on it
//   { unit: '%' }       a number written with that unit (type widths →
//                       font-stretch), as the canonical file does
//
// Output formats:
//   - semantic colors whose name does not start with `color-` become `H S% L%`
//     channels (consumed as hsl(var(--token))), with ` / alpha` when the token
//     carries one. Each channel triplet is written with just enough precision to
//     round-trip to the canonical hex exactly; the build fails otherwise.
//   - `color-*` tokens stay full paint values: hex, or rgba() with an alpha.
//   - fontFamily arrays become a CSS font-family list.

export const EXT = 'com.depoaudio'

export function createResolver(tokens) {
  function node(path) {
    let n = tokens
    for (const key of path.split('.')) {
      n = n?.[key]
      if (n === undefined) throw new Error(`Unknown token reference: {${path}}`)
    }
    if (n?.$value === undefined) throw new Error(`Reference {${path}} does not resolve to a token`)
    return n
  }

  // Resolve a whole-value {reference} to its raw value (hex stays hex, arrays
  // stay arrays). A reference embedded in a longer string is not DTCG; refuse it.
  function raw(value) {
    if (typeof value !== 'string') return value
    const whole = /^\{([^}]+)\}$/.exec(value)
    if (whole) return raw(node(whole[1]).$value)
    if (value.includes('{')) throw new Error(`Not a plain DTCG value: ${value}`)
    return value
  }

  return { node, raw }
}

export function hexToRgb(hex) {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  if (!m) throw new Error(`not an opaque hex colour: ${hex}`)
  return [0, 2, 4].map(o => parseInt(m[1].slice(o, o + 2), 16))
}

export function hslToRgb(h, s, l) {
  s /= 100
  l /= 100
  const a = s * Math.min(l, 1 - l)
  const f = n => {
    const k = (n + h / 30) % 12
    return Math.round((l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))) * 255)
  }
  return [f(0), f(8), f(4)]
}

export function parseChannels(value) {
  const m = /^(-?\d+(?:\.\d+)?) (\d+(?:\.\d+)?)% (\d+(?:\.\d+)?)%$/.exec(value)
  if (!m) throw new Error(`not an opaque "H S% L%" value: ${value}`)
  return hslToRgb(Number(m[1]), Number(m[2]), Number(m[3]))
}

function rgbToHsl([r, g, b]) {
  r /= 255
  g /= 255
  b /= 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  if (max === min) return [0, 0, l * 100]
  const d = max - min
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  let h
  if (max === r) h = (g - b) / d + (g < b ? 6 : 0)
  else if (max === g) h = (b - r) / d + 2
  else h = (r - g) / d + 4
  return [h * 60, s * 100, l * 100]
}

const trim = n => String(Number(n.toFixed(n === Math.round(n) ? 0 : 3)))

/** `#RRGGBB` → `H S% L%`, precise enough to round-trip to the same hex. */
export function hexToChannels(hex) {
  const rgb = hexToRgb(hex)
  const [h, s, l] = rgbToHsl(rgb)
  for (const digits of [1, 2, 3, 4]) {
    const parts = [h, s, l].map(v => Number(v.toFixed(digits)))
    if (hslToRgb(...parts).every((c, i) => c === rgb[i])) {
      return `${trim(parts[0])} ${trim(parts[1])}% ${trim(parts[2])}%`
    }
  }
  throw new Error(`${hex} does not round-trip through HSL`)
}

const OPAQUE = /^#[0-9a-f]{6}$/i

/** `fg` at `alpha` painted on `bg` (all `#RRGGBB`) → the opaque hex. */
export function composite(fg, alpha, bg) {
  const [f, b] = [hexToRgb(fg), hexToRgb(bg)]
  const rgb = f.map((c, i) => Math.round(c * alpha + b[i] * (1 - alpha)))
  return '#' + rgb.map(c => c.toString(16).padStart(2, '0').toUpperCase()).join('')
}

function dimension(name, value) {
  if (!value || typeof value !== 'object' || typeof value.value !== 'number' || !/^(px|rem)$/.test(value.unit)) {
    throw new Error(`${name}: a dimension must be {value: number, unit: 'px' | 'rem'}`)
  }
  return `${value.value}${value.unit}`
}

/** The CSS value of one token in its output format. */
export function cssValue(name, token, resolver) {
  const ext = token.$extensions?.[EXT] ?? {}
  const value = resolver.raw(token.$value)
  switch (token.$type) {
    case 'fontFamily':
      if (!Array.isArray(value)) throw new Error(`${name}: fontFamily must be an array`)
      return value.map(f => (/^[\w-]+$/.test(f) && !/\s/.test(f) ? f : `"${f}"`)).join(', ')
    case 'dimension':
      return dimension(name, value)
    case 'fontWeight':
    case 'number':
      if (typeof value !== 'number') throw new Error(`${name}: ${token.$type} must be a number`)
      return `${value}${ext.unit ?? ''}`
    case 'color': {
      if (typeof value !== 'string' || !OPAQUE.test(value)) {
        throw new Error(`${name}: a color must resolve to #RRGGBB (got ${JSON.stringify(token.$value)})`)
      }
      let hex = value
      let alpha = ext.alpha
      if (ext.over !== undefined) {
        hex = composite(hex, alpha, resolver.raw(ext.over))
        alpha = undefined
      }
      if (name.startsWith('color-')) return alpha === undefined ? hex : `rgba(${hexToRgb(hex).join(',')},${alpha})`
      return alpha === undefined ? hexToChannels(hex) : `${hexToChannels(hex)} / ${alpha}`
    }
    default:
      throw new Error(`${name}: unsupported $type ${token.$type}`)
  }
}
