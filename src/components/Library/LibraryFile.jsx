import { useState, useRef } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { Play, Pause } from 'lucide-react'
import { cn } from '../../lib/utils'
import { fmtSize, fmtTime, basename } from '../../utils'

const fmtBadgeClass = {
  wav: 'bg-success/10 text-success',
  mp3: 'bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))]',
  flac: 'bg-[hsl(var(--gold-dim))] text-foreground',
  opus: 'bg-warning/10 text-warning',
}

export default function LibraryFile({ file }) {
  const audioRef = useRef(null)
  const [playing, setPlaying] = useState(false)
  const [current, setCurrent] = useState(0)
  const [duration, setDuration] = useState(0)
  const src = convertFileSrc(file.path)

  const toggle = () => {
    const a = audioRef.current
    if (!a) return
    if (!a.paused) a.pause()
    else a.play().catch(() => setPlaying(false))
  }

  return (
    <div className="library-file flex items-center gap-2 px-2 py-1 bg-secondary rounded-md">
      <audio
        ref={audioRef}
        src={src}
        preload="metadata"
        onLoadedMetadata={e => setDuration(e.target.duration)}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onError={() => setPlaying(false)}
        onTimeUpdate={e => setCurrent(e.target.currentTime)}
        onEnded={() => {
          setPlaying(false)
          setCurrent(0)
        }}
      />
      <span
        className={cn(
          'library-file-format inline-flex items-center font-mono text-[9.5px] whitespace-nowrap rounded-sm px-1.5 py-0.5',
          fmtBadgeClass[file.format] || 'bg-secondary text-[hsl(var(--sub))]',
        )}
      >
        {file.format.toUpperCase()}
      </span>
      <span
        className="library-file-name text-[11px] text-[hsl(var(--text2))] flex-1 min-w-0 truncate"
        title={file.path}
      >
        {basename(file.path)}
      </span>
      <span className="library-file-size font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">
        {fmtSize(file.size)}
      </span>
      <button
        type="button"
        className="library-file-play w-7 h-7 rounded-full bg-[hsl(var(--gold-dim))] border border-primary/30 text-foreground flex items-center justify-center shrink-0 transition-colors hover:bg-primary/20 hover:border-primary focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
        aria-label={playing ? `Pause ${basename(file.path)}` : `Play ${basename(file.path)}`}
        onClick={toggle}
      >
        {playing ? (
          <Pause size={11} fill="currentColor" aria-hidden="true" />
        ) : (
          <Play size={11} fill="currentColor" aria-hidden="true" />
        )}
      </button>
      {duration > 0 && (
        <div
          className="library-file-seek relative h-2 bg-border rounded-full cursor-pointer overflow-hidden focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
          style={{ width: '80px' }}
          role="slider"
          aria-label={`Seek ${basename(file.path)}`}
          aria-valuemin={0}
          aria-valuemax={Math.round(duration)}
          aria-valuenow={Math.round(current)}
          aria-valuetext={`${fmtTime(current)} of ${fmtTime(duration)}`}
          tabIndex={0}
          onKeyDown={e => {
            if (!audioRef.current || !duration) return
            if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') {
              e.preventDefault()
              audioRef.current.currentTime = Math.max(0, current - 5)
            }
            if (e.key === 'ArrowRight' || e.key === 'ArrowUp') {
              e.preventDefault()
              audioRef.current.currentTime = Math.min(duration, current + 5)
            }
            if (e.key === 'Home') {
              e.preventDefault()
              audioRef.current.currentTime = 0
            }
            if (e.key === 'End') {
              e.preventDefault()
              audioRef.current.currentTime = duration
            }
          }}
          onClick={e => {
            const r = e.currentTarget.getBoundingClientRect()
            if (audioRef.current && duration) audioRef.current.currentTime = ((e.clientX - r.left) / r.width) * duration
          }}
        >
          <div
            className="absolute inset-y-0 left-0 bg-primary rounded-full"
            style={{ width: `${(current / duration) * 100}%` }}
          />
        </div>
      )}
      {duration > 0 && (
        <span className="library-file-time font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">
          {fmtTime(current)}
        </span>
      )}
    </div>
  )
}
