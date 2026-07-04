import { useState, useRef } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { fmtSize, fmtTime } from '../../utils'
import { Play, Pause } from 'lucide-react'

export default function MiniPlayer({ out, color, multi }) {
  const audioRef = useRef(null)
  const [playing, setPlaying] = useState(false)
  const [current, setCurrent] = useState(0)
  const [duration, setDuration] = useState(0)
  const src = convertFileSrc(out.path)

  const toggle = () => {
    const a = audioRef.current; if (!a) return
    if (playing) { a.pause(); setPlaying(false) } else { a.play().then(() => setPlaying(true)).catch(() => {}) }
  }

  return (
    <div className="flex items-center gap-2 py-0.5">
      {multi && <span className="text-[10px] shrink-0" style={{color}}>▮</span>}
      <span className="text-[11px] text-[hsl(var(--text2))] flex-1 min-w-0 truncate" title={out.path}>{out.name}</span>
      <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">{fmtSize(out.size)}</span>
      <audio ref={audioRef} src={src} preload="metadata"
        onLoadedMetadata={e => setDuration(e.target.duration)}
        onTimeUpdate={e => setCurrent(e.target.currentTime)}
        onEnded={() => { setPlaying(false); setCurrent(0) }} />
      <div className="flex items-center gap-1.5 shrink-0">
        <button className="w-[22px] h-[22px] rounded-full bg-[hsl(var(--gold-dim))] border border-primary/30 text-foreground flex items-center justify-center shrink-0 transition-colors hover:bg-primary/20 hover:border-primary"
          onClick={toggle}>
          {playing
            ? <Pause size={10} fill="currentColor" />
            : <Play size={10} fill="currentColor" />}
        </button>
        <div className="w-[90px] h-1 bg-border rounded-sm cursor-pointer overflow-hidden shrink-0" onClick={e => {
          if (!audioRef.current || !duration) return
          const r = e.currentTarget.getBoundingClientRect()
          audioRef.current.currentTime = ((e.clientX - r.left) / r.width) * duration
        }}>
          <div className="h-full bg-primary rounded-sm transition-[width_0.1s]" style={{width: duration ? `${(current/duration)*100}%` : '0%'}} />
        </div>
        {duration > 0 && <span className="font-mono text-[9px] text-[hsl(var(--sub))] shrink-0 text-right whitespace-nowrap">{fmtTime(current)}/{fmtTime(duration)}</span>}
      </div>
    </div>
  )
}
