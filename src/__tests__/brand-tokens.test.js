import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { createResolver, cssValue, hexToRgb, parseChannels } from '../../scripts/lib/tokens.mjs'

// DepoStack brand rules for the app's tokens. These checks read the token
// source (design/tokens.json) through the same resolver that writes
// src/styles/tokens.css; `npm run tokens` and CI keep the CSS in sync.
const tokens = JSON.parse(readFileSync('design/tokens.json', 'utf8'))
const resolver = createResolver(tokens)

// The canonical DepoStack primitives (depo-stack tokens/dist/depostack.tokens.json,
// ref.color.*). The app's tier-1 tokens must carry exactly these values.
const CANONICAL = {
  'vellum.0': '#FFFFFF',
  'vellum.50': '#FBFBF9',
  'vellum.100': '#F1F2EE',
  'vellum.200': '#E4E5E0',
  'ink.800': '#453B51',
  'ink.950': '#20182A',
  'night.800': '#2A2333',
  'night.850': '#241E2C',
  'night.900': '#1D1824',
  'night.950': '#15111A',
  'pale.50': '#EEEAF2',
  'pale.250': '#C9C1D3',
  'gold.300': '#F0B456',
  'gold.400': '#E8A33A',
  'gold.500': '#D6912A',
  'redline.300': '#FF9E91',
  'redline.800': '#8E1C15',
  'plum.700': '#3E2A4E',
  'violet.600': '#5C3F70',
  'cream.100': '#F4F2ED',
  'lilac.300': '#CBBFDB',
}

const THEMES = ['dark', 'light']

/** A semantic token as [r, g, b] plus its alpha. */
function color(theme, name) {
  const value = cssValue(name, tokens.semantic[theme][name], resolver)
  if (name.startsWith('color-')) {
    const rgba = /^rgba\((\d+),(\d+),(\d+),([\d.]+)\)$/.exec(value)
    if (rgba) return { rgb: rgba.slice(1, 4).map(Number), alpha: Number(rgba[4]) }
    return { rgb: hexToRgb(value), alpha: 1 }
  }
  const [channels, alpha] = value.split(' / ')
  return { rgb: parseChannels(channels), alpha: alpha ? Number(alpha) : 1 }
}

const over = (layer, ground) => layer.rgb.map((c, i) => Math.round(c * layer.alpha + ground[i] * (1 - layer.alpha)))

function luminance(rgb) {
  const [r, g, b] = rgb.map(v => {
    const c = v / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

const PLUM_BLACK = '#20182A'
const BRAND_GOLD = '#E8A33A'
const BRAND_GOLD_HOVER = '#F0B456'

describe('canonical DepoStack primitives', () => {
  it('carries the canonical hex for every tier-1 color', () => {
    for (const [path, hex] of Object.entries(CANONICAL)) {
      expect(resolver.raw(`{ref.color.${path}}`), path).toBe(hex)
    }
  })

  it('has no tier-1 color outside the canonical list', () => {
    const listed = []
    for (const [family, steps] of Object.entries(tokens.ref.color)) {
      if (family.startsWith('$')) continue
      for (const step of Object.keys(steps)) listed.push(`${family}.${step}`)
    }
    expect(listed.sort()).toEqual(Object.keys(CANONICAL).sort())
  })

  it('renders every canonical alias exactly (no rounding drift through HSL)', () => {
    for (const theme of THEMES) {
      for (const [name, token] of Object.entries(tokens.semantic[theme])) {
        if (token.$type !== 'color' || name.startsWith('color-')) continue
        const raw = resolver.raw(token.$value)
        if (!/^#[0-9a-f]{6}$/i.test(raw)) continue
        expect(color(theme, name).rgb, `${theme} ${name}`).toEqual(hexToRgb(raw))
      }
    }
  })

  it('sets UI text in Archivo and code in Fragment Mono', () => {
    expect(cssValue('font-sans', tokens.base['font-sans'], resolver)).toMatch(/^Archivo, "Archivo Fallback"/)
    expect(cssValue('font-mono', tokens.base['font-mono'], resolver)).toMatch(
      /^"Fragment Mono", "Fragment Mono Fallback"/,
    )
  })
})

describe('on-gold ink', () => {
  for (const theme of THEMES) {
    const ink = color(theme, 'primary-foreground').rgb

    it(`${theme}: is the brand plum-black`, () => {
      expect(ink).toEqual(hexToRgb(PLUM_BLACK))
    })

    it(`${theme}: clears 7:1 on the gold fill and on every hover gold`, () => {
      for (const name of ['primary', 'gold', 'gold-hi', 'color-gold', 'color-gold-hi']) {
        expect(contrast(ink, color(theme, name).rgb), `${theme} primary-foreground on ${name}`).toBeGreaterThanOrEqual(
          7,
        )
      }
    })

    it(`${theme}: clears the large-text bar on the momentary pressed gold`, () => {
      expect(contrast(ink, color(theme, 'gold-press').rgb)).toBeGreaterThanOrEqual(4.5)
    })

    it(`${theme}: the golds are the brand golds exactly`, () => {
      expect(color(theme, 'gold').rgb).toEqual(hexToRgb(BRAND_GOLD))
      expect(color(theme, 'primary').rgb).toEqual(hexToRgb(BRAND_GOLD))
      expect(color(theme, 'color-gold').rgb).toEqual(hexToRgb(BRAND_GOLD))
      expect(color(theme, 'gold-hi').rgb).toEqual(hexToRgb(BRAND_GOLD_HOVER))
      expect(color(theme, 'color-gold-hi').rgb).toEqual(hexToRgb(BRAND_GOLD_HOVER))
    })
  }

  it('documents no contrast exception', () => {
    expect(tokens.$description).not.toMatch(/5\.9|exception/i)
  })

  it('computes WCAG contrast correctly', () => {
    expect(contrast([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5)
    expect(contrast(hexToRgb('#20182A'), hexToRgb('#E8A33A'))).toBeCloseTo(7.94, 2)
  })
})

describe('WCAG AAA text pairs', () => {
  const SURFACES = ['background', 'card', 'popover', 'secondary', 'muted', 'accent', 'surface', 'panel2']
  const TEXT = ['foreground', 'muted-foreground', 'sub', 'text2', 'destructive', 'success', 'warning', 'blue']
  // Status hues also sit on their own 10% tint (badges, chips); redline does not
  // (error notices use a redline rule, not a tint).
  const TINTED = ['success', 'warning', 'blue']

  for (const theme of THEMES) {
    it(`${theme}: every text color clears 7:1 on every surface`, () => {
      for (const text of TEXT) {
        for (const surface of SURFACES) {
          expect(
            contrast(color(theme, text).rgb, color(theme, surface).rgb),
            `${theme} ${text} on ${surface}`,
          ).toBeGreaterThanOrEqual(7)
        }
      }
    })

    it(`${theme}: status text clears 7:1 on its own tint, and ink clears it on the gold tint`, () => {
      for (const surface of ['background', 'card', 'popover']) {
        const ground = color(theme, surface).rgb
        for (const text of TINTED) {
          const fg = color(theme, text)
          expect(
            contrast(fg.rgb, over({ ...fg, alpha: 0.1 }, ground)),
            `${theme} ${text} on its tint over ${surface}`,
          ).toBeGreaterThanOrEqual(7)
        }
        expect(
          contrast(color(theme, 'foreground').rgb, over(color(theme, 'gold-dim'), ground)),
          `${theme} foreground on gold-dim over ${surface}`,
        ).toBeGreaterThanOrEqual(7)
      }
    })

    it(`${theme}: the destructive fill carries AAA text`, () => {
      expect(
        contrast(color(theme, 'destructive-foreground').rgb, color(theme, 'destructive').rgb),
      ).toBeGreaterThanOrEqual(7)
    })

    it(`${theme}: focus ring and control boundary clear 3:1 on every surface`, () => {
      for (const surface of SURFACES) {
        expect(contrast(color(theme, 'ring').rgb, color(theme, surface).rgb)).toBeGreaterThanOrEqual(3)
        expect(contrast(color(theme, 'input').rgb, color(theme, surface).rgb)).toBeGreaterThanOrEqual(3)
      }
    })
  }
})
