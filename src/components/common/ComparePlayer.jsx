import { useState, useRef } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { fmtTime } from '../../utils'
import { Play, Pause } from 'lucide-react'
import { Button } from '../ui/button'
import Waveform from './Waveform'

// ── Before/After comparison player ──────────────────────────────────────────
//
// Plays original and processed audio in sync so users can hear the difference.
// A/B toggle switches between them at the same playback position.

export default function ComparePlayer({ originalPath, processedPath, originalLabel = 'Original', processedLabel = 'Processed' }) {
  const [activeSource, setActiveSource] = useState('processed') // 'original' | 'processed'
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const originalRef = useRef(null)
  const processedRef = useRef(null)

  const originalSrc = convertFileSrc(originalPath)
  const processedSrc = convertFileSrc(processedPath)

  const activeRef = activeSource === 'original' ? originalRef : processedRef

  const toggle = () => {
    const a = activeRef.current
    if (!a) return
    if (playing) { a.pause(); setPlaying(false) }
    else { a.play().then(() => setPlaying(true)).catch(() => {}) }
  }

  const switchSource = (source) => {
    const wasPlaying = playing
    const time = currentTime

    // Pause current
    if (activeRef.current) activeRef.current.pause()

    // Both audio elements are always mounted, so we can switch immediately
    const newRef = source === 'original' ? originalRef : processedRef
    if (newRef.current) {
      newRef.current.currentTime = time
      if (wasPlaying) {
        newRef.current.play().then(() => setPlaying(true)).catch(() => {})
      }
    }

    setActiveSource(source)
  }

  const seek = (time) => {
    if (originalRef.current) originalRef.current.currentTime = time
    if (processedRef.current) processedRef.current.currentTime = time
    setCurrentTime(time)
  }

  return (
    <div className="flex flex-col gap-2 py-3">
      {/* Hidden audio elements */}
      <audio ref={originalRef} src={originalSrc} preload="metadata"
        onLoadedMetadata={e => { if (!duration) setDuration(e.target.duration) }}
        onTimeUpdate={e => { if (activeSource === 'original') setCurrentTime(e.target.currentTime) }}
        onEnded={() => setPlaying(false)} />
      <audio ref={processedRef} src={processedSrc} preload="metadata"
        onLoadedMetadata={e => { if (!duration) setDuration(e.target.duration) }}
        onTimeUpdate={e => { if (activeSource === 'processed') setCurrentTime(e.target.currentTime) }}
        onEnded={() => setPlaying(false)} />

      {/* A/B toggle */}
      <div className="flex gap-0.5 bg-secondary rounded-md p-0.5 self-center">
        <Button variant="ghost" size="sm"
          className={activeSource === 'original' ? 'bg-card text-foreground shadow-sm' : ''}
          onClick={() => switchSource('original')}>
          {originalLabel}
        </Button>
        <Button variant="ghost" size="sm"
          className={activeSource === 'processed' ? 'bg-[hsl(var(--success)/0.12)] text-success shadow-sm' : ''}
          onClick={() => switchSource('processed')}>
          {processedLabel}
        </Button>
      </div>

      {/* Waveform */}
      <Waveform
        audioSrc={activeSource === 'original' ? originalSrc : processedSrc}
        color={activeSource === 'original' ? '#8097b4' : '#3a9e6a'}
        currentTime={currentTime}
        duration={duration}
        height={48}
        onSeek={seek}
      />

      {/* Controls */}
      <div className="flex items-center justify-center gap-3">
        <span className="font-mono text-[11px] text-[hsl(var(--sub))] min-w-[40px] text-center">{fmtTime(currentTime)}</span>
        <button className="w-9 h-9 rounded-full bg-[hsl(var(--gold-dim))] border border-primary/30 text-primary flex items-center justify-center shrink-0 transition-colors hover:bg-primary/20 hover:border-primary"
          onClick={toggle}>
          {playing
            ? <Pause size={14} />
            : <Play size={14} />}
        </button>
        <span className="font-mono text-[11px] text-[hsl(var(--sub))] min-w-[40px] text-center">{fmtTime(duration)}</span>
      </div>

      <p className="text-[11px] text-[hsl(var(--sub))] text-center">
        {activeSource === 'original' ? 'Listening to the original' : 'Listening to the processed version'}
        {' — '}click the other button to compare
      </p>
    </div>
  )
}
