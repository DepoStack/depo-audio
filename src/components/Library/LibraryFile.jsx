import { useState, useRef } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { Play, Pause } from 'lucide-react'
import { cn } from '../../lib/utils'
import { fmtSize, fmtTime, basename } from '../../utils'

const fmtBadgeClass = {
  wav:  'bg-success/10 text-success',
  mp3:  'bg-[hsl(var(--blue)/0.1)] text-[hsl(var(--blue))]',
  flac: 'bg-[hsl(var(--gold-dim))] text-primary',
  opus: 'bg-warning/10 text-warning',
}

export default function LibraryFile({ file }) {
  const audioRef = useRef(null)
  const [playing, setPlaying] = useState(false)
  const [current, setCurrent] = useState(0)
  const [duration, setDuration] = useState(0)
  const src = convertFileSrc(file.path)

  const toggle = () => {
    const a = audioRef.current; if (!a) return
    if (playing) { a.pause(); setPlaying(false) } else { a.play().then(() => setPlaying(true)).catch(() => {}) }
  }

  return (
    <div className="flex items-center gap-2 px-2 py-1 bg-secondary rounded-md">
      <audio ref={audioRef} src={src} preload="metadata"
        onLoadedMetadata={e => setDuration(e.target.duration)}
        onTimeUpdate={e => setCurrent(e.target.currentTime)}
        onEnded={() => { setPlaying(false); setCurrent(0) }} />
      <span className={cn(
        'inline-flex items-center font-mono text-[9.5px] whitespace-nowrap rounded-sm px-1.5 py-0.5',
        fmtBadgeClass[file.format] || 'bg-secondary text-[hsl(var(--sub))]'
      )}>
        {file.format.toUpperCase()}
      </span>
      <span className="text-[11px] text-[hsl(var(--text2))] flex-1 min-w-0 truncate" title={file.path}>
        {basename(file.path)}
      </span>
      <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">{fmtSize(file.size)}</span>
      <button
        className="w-[22px] h-[22px] rounded-full bg-[hsl(var(--gold-dim))] border border-primary/30 text-primary flex items-center justify-center shrink-0 transition-colors hover:bg-primary/20 hover:border-primary"
        onClick={toggle}
      >
        {playing
          ? <Pause size={10} fill="currentColor" />
          : <Play size={10} fill="currentColor" />}
      </button>
      {duration > 0 && (
        <div
          className="relative h-1 bg-border rounded-full cursor-pointer overflow-hidden"
          style={{ width: '80px' }}
          onClick={e => {
            const r = e.currentTarget.getBoundingClientRect()
            if (audioRef.current && duration) audioRef.current.currentTime = ((e.clientX - r.left) / r.width) * duration
          }}
        >
          <div className="absolute inset-y-0 left-0 bg-primary rounded-full" style={{ width: `${(current / duration) * 100}%` }} />
        </div>
      )}
      {duration > 0 && <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">{fmtTime(current)}</span>}
    </div>
  )
}
