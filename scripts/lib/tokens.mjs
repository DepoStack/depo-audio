// Shared token resolution for design/tokens.json (DTCG format), used by
// scripts/build-tokens.mjs and the brand token tests.
//
// Three tiers:
//   ref      canonical DepoStack primitives (exact hex from depo-stack
//            tokens/dist/depostack.tokens.json)
//   base     mode-independent roles (font stacks, type roles) → :root as --ds-*
//   semantic the app's role names per theme (dark → :root, light → .light)
//
// Alias tokens reference other tokens as {dot.path}; a reference may be
// embedded in a longer string ("{ref.color.gold.400} / 0.14").
//
// Output formats:
//   - semantic colors whose name does not start with `color-` become `H S% L%`
//     channels (consumed as hsl(var(--token))). Each channel triplet is written
//     with just enough precision to round-trip to the canonical hex exactly; the
//     build fails otherwise.
//   - `color-*` tokens stay full paint values: hex, or rgba() with an alpha.
//   - `{a} / 0.22 over {b}` composites a translucent canonical color onto a
//     canonical ground (the hairline on the board) into one opaque value, so
//     Tailwind opacity modifiers still work on it.
//   - fontFamily arrays become a CSS font-family list.

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

  // Resolve references to their raw values (hex stays hex, arrays stay arrays).
  function raw(value) {
    if (Array.isArray(value)) return value
    if (typeof value !== 'string') return value
    const whole = /^\{([^}]+)\}$/.exec(value)
    if (whole) return raw(node(whole[1]).$value)
    return value.replace(/\{([^}]+)\}/g, (_, path) => {
      const v = raw(node(path).$value)
      if (typeof v !== 'string') throw new Error(`Reference {${path}} cannot be embedded in a string`)
      return v
    })
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

const HEX = /#[0-9a-f]{6}\b/gi
const OVER = /^(#[0-9a-f]{6}) \/ (\d*\.?\d+) over (#[0-9a-f]{6})$/i

/** `#fg / alpha over #bg` → the opaque hex of that layer painted on that ground. */
export function composite(value) {
  const m = OVER.exec(value)
  if (!m) return value
  const [fg, alpha, bg] = [hexToRgb(m[1]), Number(m[2]), hexToRgb(m[3])]
  const rgb = fg.map((c, i) => Math.round(c * alpha + bg[i] * (1 - alpha)))
  return '#' + rgb.map(c => c.toString(16).padStart(2, '0').toUpperCase()).join('')
}

/** The CSS value of one token in its output format. */
export function cssValue(name, token, resolver) {
  let value = resolver.raw(token.$value)
  if (token.$type === 'color' && typeof value === 'string') value = composite(value)
  if (token.$type === 'fontFamily') {
    if (!Array.isArray(value)) throw new Error(`${name}: fontFamily must be an array`)
    return value.map(f => (/^[\w-]+$/.test(f) && !/\s/.test(f) ? f : `"${f}"`)).join(', ')
  }
  if (typeof value !== 'string') return String(value)
  if (token.$type !== 'color') return value
  if (name.startsWith('color-')) {
    const alpha = /^(#[0-9a-f]{6}) \/ (\d*\.?\d+)$/i.exec(value)
    if (alpha) return `rgba(${hexToRgb(alpha[1]).join(',')},${alpha[2]})`
    return value
  }
  return value.replace(HEX, hexToChannels)
}
