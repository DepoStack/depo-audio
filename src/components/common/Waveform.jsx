import { useRef, useEffect, useState } from 'react'

// ── Waveform visualization ──────────────────────────────────────────────────
//
// Canvas-based audio waveform display with:
//   - Click-to-seek
//   - Playback position indicator
//   - Speaker color support
//   - Responsive resizing

export default function Waveform({
  audioSrc,        // URL to audio file (from convertFileSrc)
  color = 'hsl(var(--speaker-1))',
  currentTime = 0,
  duration = 0,
  height = 48,
  onSeek,          // (time: number) => void
  markers = [],    // [{ time: number, label: string, color: string }]
  loop = null,     // { a, b } A-B loop region in seconds
}) {
  const canvasRef = useRef(null)
  const containerRef = useRef(null)
  const [peaks, setPeaks] = useState(null)
  const [width, setWidth] = useState(400)

  // Decode audio and extract peaks
  useEffect(() => {
    if (!audioSrc) return
    let cancelled = false

    const extractPeaks = async () => {
      let audioCtx = null
      try {
        const response = await fetch(audioSrc)
        const buffer = await response.arrayBuffer()
        audioCtx = new (window.AudioContext || window.webkitAudioContext)()
        const decoded = await audioCtx.decodeAudioData(buffer)

        if (cancelled) return

        // Downsample to target width
        const channel = decoded.getChannelData(0)
        const samplesPerPixel = Math.floor(channel.length / width)
        const peakData = []

        for (let i = 0; i < width; i++) {
          let min = 1.0, max = -1.0
          const start = i * samplesPerPixel
          const end = Math.min(start + samplesPerPixel, channel.length)
          for (let j = start; j < end; j++) {
            if (channel[j] < min) min = channel[j]
            if (channel[j] > max) max = channel[j]
          }
          peakData.push({ min, max })
        }

        setPeaks(peakData)
      } catch {
        // Fallback: empty waveform if decode fails
        setPeaks(null)
      } finally {
        // Close on every path — leaked contexts hit the browser's cap and
        // permanently break waveform rendering
        if (audioCtx) audioCtx.close().catch(() => {})
      }
    }

    extractPeaks()
    return () => { cancelled = true }
  }, [audioSrc, width])

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
    if (!canvas || !peaks) return

    const ctx = canvas.getContext('2d')
    const dpr = window.devicePixelRatio || 1
    canvas.width = width * dpr
    canvas.height = height * dpr
    ctx.scale(dpr, dpr)

    const midY = height / 2
    const playedWidth = duration > 0 ? (currentTime / duration) * width : 0
    // Bookmark/marker default resolves to the token, not a raw hex.
    const markerColor = 'hsl(var(--destructive))'

    ctx.clearRect(0, 0, width, height)

    // A-B loop region: a translucent gold band behind the bars so the looped
    // span is actually visible (the old control drew nothing).
    if (loop && loop.a != null && loop.b != null && duration > 0 && loop.b > loop.a) {
      const x0 = (loop.a / duration) * width
      const x1 = (loop.b / duration) * width
      ctx.fillStyle = 'hsl(var(--gold-dim))'
      ctx.fillRect(x0, 0, Math.max(x1 - x0, 1), height)
    }

    // Draw waveform bars — played at full strength, unplayed dimmed via alpha
    // (no hue math, so any color format works and stays theme-correct).
    for (let i = 0; i < peaks.length && i < width; i++) {
      const { min, max } = peaks[i]
      const barTop = midY - max * midY * 0.85
      const barBottom = midY - min * midY * 0.85
      const barHeight = Math.max(barBottom - barTop, 1)

      ctx.fillStyle = color
      ctx.globalAlpha = i < playedWidth ? 1.0 : 0.42
      ctx.fillRect(i, barTop, 1, barHeight)
    }

    ctx.globalAlpha = 1.0

    // Draw playback position line
    if (duration > 0 && playedWidth > 0) {
      ctx.strokeStyle = color
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
      ctx.strokeStyle = marker.color || markerColor
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
        ctx.fillStyle = marker.color || markerColor
        ctx.fillText(marker.label, x + 2, 10)
      }
    }
  }, [peaks, width, height, currentTime, duration, color, markers, loop])

  const handleClick = (e) => {
    if (!onSeek || !duration) return
    const rect = canvasRef.current.getBoundingClientRect()
    const x = e.clientX - rect.left
    const time = (x / rect.width) * duration
    onSeek(Math.max(0, Math.min(time, duration)))
  }

  return (
    <div ref={containerRef} className="w-full rounded overflow-hidden bg-secondary cursor-pointer" style={{ height }}>
      {peaks ? (
        <canvas
          ref={canvasRef}
          className="block"
          style={{ width: '100%', height }}
          onClick={handleClick}
        />
      ) : (
        <div className="flex items-center justify-center px-2" style={{ height }}>
          <div className="w-full h-0.5 bg-primary/30 rounded-full overflow-hidden">
            <div className="h-full w-1/3 bg-primary rounded-full"
              style={{ animation: 'progress-pulse 1.2s ease-in-out infinite' }} />
          </div>
        </div>
      )}
    </div>
  )
}
