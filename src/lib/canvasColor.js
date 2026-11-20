export function resolveCanvasColor(value, styles, fallback = '#888') {
  if (typeof value !== 'string' || !value.trim()) return fallback
  const match = value.trim().match(/^hsl\(var\((--[\w-]+)\)(?:\s*\/\s*([\d.]+))?\)$/)
  if (!match) return value

  const computed = styles ?? (typeof window !== 'undefined' ? window.getComputedStyle(document.documentElement) : null)
  const token = computed?.getPropertyValue(match[1])?.trim()
  if (!token) return fallback
  return match[2] ? `hsl(${token} / ${match[2]})` : `hsl(${token})`
}
