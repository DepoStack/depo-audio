// The DepoStack mark: three stacked isometric plates (depo-stack public/depostack-mark.svg).
// The layers read the artifact.mark tokens, so each theme gets its canonical layer
// colors (violet/plum/gold on the light board, lilac/cream/gold on the dark board).
// Never recolor, rotate, skew or shadow it, and never draw it under 24px.
export function DepoStackMark({ size = 24, className }) {
  const px = Math.max(24, size)
  return (
    <svg
      width={px}
      height={px}
      viewBox="0 0 160 160"
      aria-hidden="true"
      focusable="false"
      className={className}
      data-brand-mark="depostack"
    >
      <polygon points="80,78 152,115 80,152 8,115" fill="var(--color-mark-bottom)" />
      <polygon points="80,46 152,83 80,120 8,83" fill="var(--color-mark-mid)" />
      <polygon points="80,14 152,51 80,88 8,51" fill="var(--color-mark-top)" />
    </svg>
  )
}

// The DepoStack wordmark: Archivo at 90% width, "Depo" 500 and "Stack" 800, both in ink.
// "Stack" is set apart by weight, never by color (BRAND.md §2).
export function DepoStackWordmark({ className }) {
  return (
    <span className={className} style={{ fontStretch: '90%' }}>
      <span className="font-medium">Depo</span>
      <span className="font-extrabold">Stack</span>
    </span>
  )
}
