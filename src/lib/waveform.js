export const MAX_WAVEFORM_SECONDS = 10 * 60

let fallbackRequestSequence = 0

export function createWaveformRequestId() {
  if (typeof globalThis.crypto?.randomUUID === 'function') return globalThis.crypto.randomUUID()
  fallbackRequestSequence += 1
  return `waveform-${Date.now().toString(36)}-${fallbackRequestSequence.toString(36)}`
}

export function clampSeekTime(time, duration) {
  if (!Number.isFinite(duration) || duration <= 0 || !Number.isFinite(time)) return null
  return Math.max(0, Math.min(time, duration))
}

export function seekTimeFromPointer({ clientX, left, width, duration }) {
  if (!Number.isFinite(width) || width <= 0) return null
  return clampSeekTime(((clientX - left) / width) * duration, duration)
}

export function seekTimeFromKey({ key, currentTime, duration, step = 5 }) {
  const current = clampSeekTime(currentTime, duration)
  if (current == null) return null
  if (key === 'ArrowLeft' || key === 'ArrowDown') return clampSeekTime(current - step, duration)
  if (key === 'ArrowRight' || key === 'ArrowUp') return clampSeekTime(current + step, duration)
  if (key === 'Home') return 0
  if (key === 'End') return duration
  return null
}

export function waveformDecodeGuard({ duration } = {}) {
  if (Number.isFinite(duration) && duration > MAX_WAVEFORM_SECONDS) {
    return 'Waveform disabled for long recordings over 10 minutes. Playback is still available.'
  }
  return ''
}
