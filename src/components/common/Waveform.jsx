import { useRef, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { resolveCanvasColor } from '../../lib/canvasColor'
import {
  clampSeekTime,
  createWaveformRequestId,
  seekTimeFromKey,
  seekTimeFromPointer,
  waveformDecodeGuard,
} from '../../lib/waveform'

// ── Waveform visualization ──────────────────────────────────────────────────
//
// Canvas-based audio waveform display with:
//   - Click-to-seek
//   - Playback position indicator
//   - Speaker color support
//   - Responsive resizing

export default function Waveform({
  audioPath, // Local path used by the bounded backend envelope decoder
  color = 'hsl(var(--speaker-1))',
  currentTime = 0,
  duration = 0,
  height = 48,
  onSeek, // (time: number) => void
  markers = [], // [{ time: number, label: string, color: string }]
  loop = null, // { a, b } A-B loop region in seconds
}) {
  const canvasRef = useRef(null)
  const containerRef = useRef(null)
  const [waveform, setWaveform] = useState({ source: '', peaks: null, error: '' })
  const peaks = waveform.source === audioPath ? waveform.peaks : null
  const durationKnown = Number.isFinite(duration) && duration > 0
  const seekable = typeof onSeek === 'function' && durationKnown
  const seekValue = clampSeekTime(currentTime, duration) ?? 0
  const durationGuard = waveformDecodeGuard({ duration })
  const error = durationGuard || (waveform.source === audioPath ? waveform.error : '')
  const [width, setWidth] = useState(400)

  // Request a fixed-size envelope; decoding stays bounded in the backend.
  useEffect(() => {
    if (!audioPath || !durationKnown || durationGuard) return
    const requestId = createWaveformRequestId()
    let disposed = false
    let settled = false

    const extractPeaks = async () => {
      try {
        const peakData = await invoke('waveform_peaks_cmd', { path: audioPath, requestId })
        if (disposed) return
        if (!Array.isArray(peakData) || peakData.length === 0) throw new Error('Audio contains no samples')
        setWaveform({ source: audioPath, peaks: peakData, error: '' })
      } catch (e) {
        if (!disposed) {
          setWaveform({ source: audioPath, peaks: null, error: `Waveform unavailable: ${e?.message || e}` })
        }
      } finally {
        settled = true
      }
    }

    extractPeaks()
    return () => {
      disposed = true
      if (!settled) invoke('cancel_waveform_cmd', { requestId }).catch(() => {})
    }
  }, [audioPath, durationKnown, durationGuard])

  // Observe container width for responsiveness
  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const observer = new ResizeObserver(entries => {
      for (const entry of entries) {
        setWidth(Math.floor(entry.contentRect.width))
      }
    })
    observer.observe(container)
    setWidth(Math.floor(container.offsetWidth))

    return () => observer.disconnect()
  }, [])

  // Draw waveform
  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !peaks || width <= 0) return

    const ctx = canvas.getContext('2d')
    const dpr = window.devicePixelRatio || 1
    canvas.width = width * dpr
    canvas.height = height * dpr
    ctx.scale(dpr, dpr)

    const midY = height / 2
    const playedWidth = duration > 0 ? (currentTime / duration) * width : 0
    // Bookmark/marker default resolves to the token, not a raw hex.
    const styles = getComputedStyle(document.documentElement)
    const waveformColor = resolveCanvasColor(color, styles)
    const markerColor = resolveCanvasColor('hsl(var(--destructive))', styles, '#c94e4e')
    const loopColor = resolveCanvasColor('hsl(var(--gold-dim))', styles, 'rgba(196, 154, 54, 0.14)')

    ctx.clearRect(0, 0, width, height)

    // A-B loop region: a translucent gold band behind the bars so the looped
    // span is actually visible (the old control drew nothing).
    if (loop && loop.a != null && loop.b != null && duration > 0 && loop.b > loop.a) {
      const x0 = (loop.a / duration) * width
      const x1 = (loop.b / duration) * width
      ctx.fillStyle = loopColor
      ctx.fillRect(x0, 0, Math.max(x1 - x0, 1), height)
    }

    // Draw waveform bars — played at full strength, unplayed dimmed via alpha
    // (no hue math, so any color format works and stays theme-correct).
    for (let i = 0; i < width; i++) {
      const peakIndex = Math.min(peaks.length - 1, Math.floor((i / width) * peaks.length))
      const { min, max } = peaks[peakIndex]
      const barTop = midY - max * midY * 0.85
      const barBottom = midY - min * midY * 0.85
      const barHeight = Math.max(barBottom - barTop, 1)

      ctx.fillStyle = waveformColor
      ctx.globalAlpha = i < playedWidth ? 1.0 : 0.42
      ctx.fillRect(i, barTop, 1, barHeight)
    }

    ctx.globalAlpha = 1.0

    // Draw playback position line
    if (duration > 0 && playedWidth > 0) {
      ctx.strokeStyle = waveformColor
      ctx.lineWidth = 1.5
      ctx.beginPath()
      ctx.moveTo(playedWidth, 0)
      ctx.lineTo(playedWidth, height)
      ctx.stroke()
    }

    // Draw markers
    for (const marker of markers) {
      if (marker.time <= 0 || marker.time >= duration) continue
      const x = (marker.time / duration) * width
      ctx.strokeStyle = resolveCanvasColor(marker.color, styles, markerColor)
      ctx.lineWidth = 1
      ctx.setLineDash([2, 2])
      ctx.beginPath()
      ctx.moveTo(x, 0)
      ctx.lineTo(x, height)
      ctx.stroke()
      ctx.setLineDash([])

      // Marker label
      if (marker.label) {
        ctx.font = '9px "DM Mono", monospace'
        ctx.fillStyle = resolveCanvasColor(marker.color, styles, markerColor)
        ctx.fillText(marker.label, x + 2, 10)
      }
    }
  }, [peaks, width, height, currentTime, duration, color, markers, loop])

  const handleClick = e => {
    if (!seekable) return
    const rect = e.currentTarget.getBoundingClientRect()
    const time = seekTimeFromPointer({ clientX: e.clientX, left: rect.left, width: rect.width, duration })
    if (time != null) onSeek(time)
  }

  const handleKeyDown = e => {
    if (!seekable) return
    const time = seekTimeFromKey({ key: e.key, currentTime: seekValue, duration })
    if (time == null) return
    e.preventDefault()
    onSeek(time)
  }

  return (
    <div
      ref={containerRef}
      className={`w-full rounded overflow-hidden bg-secondary focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring ${seekable ? 'cursor-pointer' : ''}`}
      style={{ height }}
      role={seekable ? 'slider' : 'img'}
      aria-label={seekable ? 'Audio waveform seek control' : 'Audio waveform'}
      aria-valuemin={seekable ? 0 : undefined}
      aria-valuemax={seekable ? duration : undefined}
      aria-valuenow={seekable ? seekValue : undefined}
      tabIndex={seekable ? 0 : undefined}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
    >
      {peaks ? (
        <canvas ref={canvasRef} className="block" style={{ width: '100%', height }} />
      ) : error ? (
        <div
          className="flex items-center justify-center px-3 text-center text-[10px] text-[hsl(var(--sub))]"
          style={{ height }}
        >
          {error}
        </div>
      ) : (
        <div className="flex items-center justify-center px-2" style={{ height }}>
          <div className="w-full h-0.5 bg-primary/30 rounded-full overflow-hidden">
            <div
              className="h-full w-1/3 bg-primary rounded-full"
              style={{ animation: 'progress-pulse 1.2s ease-in-out infinite' }}
            />
          </div>
        </div>
      )}
    </div>
  )
}
