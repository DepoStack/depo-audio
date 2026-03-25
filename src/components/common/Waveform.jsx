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
  color = '#c49a36',
  playedColor,     // Color for played portion (defaults to brighter version of color)
  currentTime = 0,
  duration = 0,
  height = 48,
  onSeek,          // (time: number) => void
  markers = [],    // [{ time: number, label: string, color: string }]
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
      try {
        const response = await fetch(audioSrc)
        const buffer = await response.arrayBuffer()
        const audioCtx = new (window.AudioContext || window.webkitAudioContext)()
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
        audioCtx.close()
      } catch (e) {
        // Fallback: empty waveform if decode fails
        setPeaks(null)
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
    const resolvedPlayedColor = playedColor || lightenColor(color, 0.3)

    ctx.clearRect(0, 0, width, height)

    // Draw waveform bars
    for (let i = 0; i < peaks.length && i < width; i++) {
      const { min, max } = peaks[i]
      const barTop = midY - max * midY * 0.85
      const barBottom = midY - min * midY * 0.85
      const barHeight = Math.max(barBottom - barTop, 1)

      ctx.fillStyle = i < playedWidth ? resolvedPlayedColor : color
      ctx.globalAlpha = i < playedWidth ? 1.0 : 0.5
      ctx.fillRect(i, barTop, 1, barHeight)
    }

    ctx.globalAlpha = 1.0

    // Draw playback position line
    if (duration > 0 && playedWidth > 0) {
      ctx.strokeStyle = resolvedPlayedColor
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
      ctx.strokeStyle = marker.color || '#c44e4e'
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
        ctx.fillStyle = marker.color || '#c44e4e'
        ctx.fillText(marker.label, x + 2, 10)
      }
    }
  }, [peaks, width, height, currentTime, duration, color, playedColor, markers])

  const handleClick = (e) => {
    if (!onSeek || !duration) return
    const rect = canvasRef.current.getBoundingClientRect()
    const x = e.clientX - rect.left
    const time = (x / rect.width) * duration
    onSeek(Math.max(0, Math.min(time, duration)))
  }

  return (
    <div ref={containerRef} className="waveform-container" style={{ height }}>
      {peaks ? (
        <canvas
          ref={canvasRef}
          className="waveform-canvas"
          style={{ width: '100%', height }}
          onClick={handleClick}
        />
      ) : (
        <div className="waveform-placeholder" style={{ height }}>
          <div className="waveform-loading" />
        </div>
      )}
    </div>
  )
}

// Lighten a hex color by a factor (0-1)
function lightenColor(hex, factor) {
  const r = parseInt(hex.slice(1, 3), 16)
  const g = parseInt(hex.slice(3, 5), 16)
  const b = parseInt(hex.slice(5, 7), 16)
  const lr = Math.min(255, Math.floor(r + (255 - r) * factor))
  const lg = Math.min(255, Math.floor(g + (255 - g) * factor))
  const lb = Math.min(255, Math.floor(b + (255 - b) * factor))
  return `#${lr.toString(16).padStart(2,'0')}${lg.toString(16).padStart(2,'0')}${lb.toString(16).padStart(2,'0')}`
}
