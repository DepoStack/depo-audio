import { useState, useRef } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { fmtTime } from '../../utils'
import { Play, Pause } from 'lucide-react'
import { Button } from '../ui/button'
import Waveform from './Waveform'

// ── Before/After comparison player ──────────────────────────────────────────
//
// Plays original and processed audio in sync so users can hear the difference.
// A/B toggle switches between them at the same playback position. Only the
// active source plays; the other waits at the same position.

export default function ComparePlayer({
  originalPath,
  processedPath,
  originalLabel = 'Original',
  processedLabel = 'Processed',
  unavailableMessage = "This file can't be played here.",
  processedUnavailableMessage = '',
  note = '',
  // False when the two files' timelines differ (leading-silence trim), so the
  // hint does not promise the same position.
  aligned = true,
}) {
  const [activeSource, setActiveSource] = useState('processed') // 'original' | 'processed'
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [failed, setFailed] = useState({ original: false, processed: false })
  const originalRef = useRef(null)
  const processedRef = useRef(null)

  const originalSrc = convertFileSrc(originalPath)
  const processedSrc = convertFileSrc(processedPath)

  const activeRef = activeSource === 'original' ? originalRef : processedRef

  const toggle = () => {
    const a = activeRef.current
    if (!a) return
    if (playing) {
      a.pause()
      setPlaying(false)
    } else {
      a.play()
        .then(() => setPlaying(true))
        .catch(() => {})
    }
  }

  const markFailed = source => {
    setFailed(prev => ({ ...prev, [source]: true }))
    if (activeSource === source) setPlaying(false)
  }

  const switchSource = source => {
    if (source === activeSource) return
    const wasPlaying = playing
    const time = currentTime

    // Pause current
    if (activeRef.current) activeRef.current.pause()

    // Both audio elements are always mounted, so we can switch immediately
    const newRef = source === 'original' ? originalRef : processedRef
    if (newRef.current) {
      newRef.current.currentTime = time
      // The two sources can differ in length (trim/fade change duration), so
      // show the duration of the one we're now listening to.
      const d = newRef.current.duration
      if (Number.isFinite(d)) setDuration(d)
      if (wasPlaying) {
        newRef.current
          .play()
          .then(() => setPlaying(true))
          .catch(() => {})
      }
    }

    setActiveSource(source)
  }

  const seek = time => {
    if (originalRef.current) originalRef.current.currentTime = time
    if (processedRef.current) processedRef.current.currentTime = time
    setCurrentTime(time)
  }

  return (
    <div className="flex flex-col gap-2 py-3">
      {/* Hidden audio elements */}
      <audio
        ref={originalRef}
        src={originalSrc}
        preload="metadata"
        onLoadedMetadata={e => {
          if (activeSource === 'original') setDuration(e.target.duration || 0)
        }}
        onTimeUpdate={e => {
          if (activeSource === 'original') setCurrentTime(e.target.currentTime)
        }}
        onEnded={() => {
          if (activeSource === 'original') setPlaying(false)
        }}
        onError={() => markFailed('original')}
      />
      <audio
        ref={processedRef}
        src={processedSrc}
        preload="metadata"
        onLoadedMetadata={e => {
          if (activeSource === 'processed') setDuration(e.target.duration || 0)
        }}
        onTimeUpdate={e => {
          if (activeSource === 'processed') setCurrentTime(e.target.currentTime)
        }}
        onEnded={() => {
          if (activeSource === 'processed') setPlaying(false)
        }}
        onError={() => markFailed('processed')}
      />

      {/* A/B toggle */}
      <div role="group" aria-label="Listen to" className="flex gap-0.5 bg-secondary rounded-md p-0.5 self-center">
        <Button
          variant="ghost"
          size="sm"
          aria-pressed={activeSource === 'original'}
          className={activeSource === 'original' ? 'bg-card text-foreground ring-1 ring-inset ring-foreground' : ''}
          onClick={() => switchSource('original')}
        >
          {originalLabel}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          aria-pressed={activeSource === 'processed'}
          className={activeSource === 'processed' ? 'bg-card text-foreground ring-1 ring-inset ring-foreground' : ''}
          onClick={() => switchSource('processed')}
        >
          {processedLabel}
        </Button>
      </div>

      {/* Waveform */}
      <Waveform
        audioPath={activeSource === 'original' ? originalPath : processedPath}
        color={activeSource === 'original' ? 'hsl(var(--sub))' : 'hsl(var(--success))'}
        currentTime={currentTime}
        duration={duration}
        height={48}
        onSeek={seek}
      />

      {/* Controls */}
      <div className="flex items-center justify-center gap-3">
        <span className="tabular-nums text-[12px] text-[hsl(var(--sub))] min-w-[40px] text-center">
          {fmtTime(currentTime)}
        </span>
        <button
          className="w-9 h-9 bg-secondary border border-foreground/60 text-foreground flex items-center justify-center shrink-0 transition-colors hover:border-foreground"
          aria-label={playing ? 'Pause' : 'Play'}
          onClick={toggle}
        >
          {playing ? <Pause size={14} /> : <Play size={14} />}
        </button>
        <span className="tabular-nums text-[12px] text-[hsl(var(--sub))] min-w-[40px] text-center">
          {fmtTime(duration)}
        </span>
      </div>

      <p className="type-small text-[hsl(var(--sub))] text-center" aria-live="polite">
        {activeSource === 'original'
          ? `Listening to the ${originalLabel.toLowerCase()}`
          : `Listening to the ${processedLabel.toLowerCase()}`}
        {aligned
          ? ' — choose the other button to compare at the same position'
          : ' — choose the other button to switch'}
      </p>
      {note && <p className="type-small text-[hsl(var(--text2))] text-center">{note}</p>}
      {failed[activeSource] && (
        <p role="alert" className="rounded border-l-[3px] border-l-destructive px-2 py-1.5 type-small text-destructive">
          {activeSource === 'original'
            ? unavailableMessage
            : processedUnavailableMessage || `${processedLabel} can't be played here.`}
        </p>
      )}
    </div>
  )
}
