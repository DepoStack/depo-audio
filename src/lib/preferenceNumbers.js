// Keep incomplete or invalid number-field drafts out of persisted preferences.
export function parseNumericPreference(value, { min, max, integer = false }) {
  if (typeof value !== 'number' && typeof value !== 'string') return null
  if (typeof value === 'string' && !value.trim()) return null
  const number = Number(value)
  if (!Number.isFinite(number) || number < min || number > max) return null
  if (integer && !Number.isInteger(number)) return null
  return number
}

export const INTEGER_PREFERENCE_LIMITS = {
  ffmpegTimeout: { min: 60, max: 3600, integer: true },
  maxScanDepth: { min: 1, max: 20, integer: true },
}
