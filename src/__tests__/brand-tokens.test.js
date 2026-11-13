import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

// DepoStack brand rule: the ink on gold is plum-black at 7:1 or better, in both
// themes, on the gold fill and on its hover. These checks read the token source
// (design/tokens.json); `npm run tokens` and CI keep src/styles/tokens.css in sync.
const tokens = JSON.parse(readFileSync('design/tokens.json', 'utf8'))

function hslToRgb(value) {
  const match = /^(\d+(?:\.\d+)?) (\d+(?:\.\d+)?)% (\d+(?:\.\d+)?)%$/.exec(value)
  if (!match) throw new Error(`not an opaque "H S% L%" token: ${value}`)
  const [h, s, l] = [Number(match[1]), Number(match[2]) / 100, Number(match[3]) / 100]
  const a = s * Math.min(l, 1 - l)
  const channel = n => {
    const k = (n + h / 30) % 12
    return Math.round((l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))) * 255)
  }
  return [channel(0), channel(8), channel(4)]
}

function hexToRgb(value) {
  const match = /^#([0-9a-f]{6})$/i.exec(value)
  if (!match) throw new Error(`not an opaque hex colour: ${value}`)
  return [0, 2, 4].map(offset => parseInt(match[1].slice(offset, offset + 2), 16))
}

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

const PLUM_BLACK = '#20182A' // depo-stack tokens: ref.color.ink.950, "the ink on gold in both themes"

describe('on-gold ink', () => {
  for (const theme of ['dark', 'light']) {
    const group = tokens.semantic[theme]
    const ink = hslToRgb(group['primary-foreground'].$value)

    it(`${theme}: is the brand plum-black`, () => {
      expect(ink).toEqual(hexToRgb(PLUM_BLACK))
    })

    it(`${theme}: clears 7:1 on the gold fill and on every hover gold`, () => {
      const fills = {
        primary: hslToRgb(group.primary.$value),
        gold: hslToRgb(group.gold.$value),
        'gold-hi': hslToRgb(group['gold-hi'].$value),
        'color-gold': hexToRgb(group['color-gold'].$value),
        'color-gold-hi': hexToRgb(group['color-gold-hi'].$value),
      }
      for (const [name, fill] of Object.entries(fills)) {
        expect(contrast(ink, fill), `${theme} primary-foreground on ${name}`).toBeGreaterThanOrEqual(7)
      }
    })
  }

  it('documents no on-gold contrast exception', () => {
    expect(tokens.$description).not.toMatch(/5\.9|exception/i)
  })

  it('computes WCAG contrast correctly', () => {
    expect(contrast([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5)
    expect(contrast(hexToRgb('#20182A'), hexToRgb('#E8A33A'))).toBeCloseTo(7.94, 2)
  })
})
