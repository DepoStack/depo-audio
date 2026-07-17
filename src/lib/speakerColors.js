import { useEffect, useState } from 'react'

// Speaker/track color palette lives in the design tokens (--speaker-1..4),
// tuned per theme. Canvas fillStyle can't read CSS variables, so this hook
// resolves them to concrete `hsl(...)` strings off the document root and
// recomputes when the theme flips (the root's class/data-theme changes).

export const SPEAKER_COUNT = 4

function readSpeakerColors() {
  if (typeof window === 'undefined') return []
  const s = getComputedStyle(document.documentElement)
  return Array.from({ length: SPEAKER_COUNT }, (_, i) => {
    const triplet = s.getPropertyValue(`--speaker-${i + 1}`).trim()
    return triplet ? `hsl(${triplet})` : '#888'
  })
}

/** Concrete per-theme speaker colors, index 0..3. Recomputes on theme change. */
export function useSpeakerColors() {
  const [colors, setColors] = useState(readSpeakerColors)
  useEffect(() => {
    const update = () => setColors(readSpeakerColors())
    update() // in case the first paint preceded token CSS
    const obs = new MutationObserver(update)
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-theme', 'style'] })
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    mq.addEventListener?.('change', update)
    return () => { obs.disconnect(); mq.removeEventListener?.('change', update) }
  }, [])
  return colors
}

/** Concrete color for a speaker index, wrapping every SPEAKER_COUNT. */
export function speakerColorAt(colors, index) {
  if (!colors.length) return '#888'
  return colors[((index % SPEAKER_COUNT) + SPEAKER_COUNT) % SPEAKER_COUNT]
}
