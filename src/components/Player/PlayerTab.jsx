import { useState, useRef, useEffect } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Play, Pause, SkipBack, SkipForward, Bookmark, X, Plus } from 'lucide-react'
import { cn } from '../../lib/utils'
import { CH_COLORS } from '../../constants'
import { fmtTime } from '../../utils'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle } from '../ui/card'
import Waveform from '../common/Waveform'
import { WaveformIcon } from '../common/Icons'

// ── Global Audio Player ─────────────────────────────────────────────────────
//
// Play any audio file directly — no conversion needed. Supports multi-channel
// files with color-coded speaker tracks. Drop files or browse to start.

// Extensions the player accepts — native drops can contain anything
const AUDIO_EXTS = ['wav','mp3','flac','opus','ogg','m4a','aac','wma','aif','aiff','sgmca','trm','ftr','bwf']

export default function PlayerTab({ dropHandlerRef }) {
  const [tracks, setTracks] = useState([])       // { path, name, size, channels, duration, color }
  const [activeTrack, setActiveTrack] = useState(null)
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [dragOver, setDragOver] = useState(false)
  // Bookmarks persist across sessions; validate the shape — corrupt storage
  // must not crash the tab on every launch
  const [bookmarks, setBookmarks] = useState(() => {
    try {
      const v = JSON.parse(localStorage.getItem('player-bookmarks') || '[]')
      return Array.isArray(v) ? v.filter(b => b && typeof b.time === 'number' && typeof b.trackPath === 'string') : []
    } catch { return [] }
  })
  useEffect(() => {
    try { localStorage.setItem('player-bookmarks', JSON.stringify(bookmarks)) } catch { /* storage full or unavailable */ }
  }, [bookmarks])
  const [dragIdx, setDragIdx] = useState(null) // playlist drag-reorder
  const audioRef = useRef(null)
  const autoAdvanceRef = useRef(false) // play next track once it loads

  // Browse for files
  const browseFiles = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [{ name: 'Audio', extensions: AUDIO_EXTS }],
      })
      if (selected) {
        const paths = Array.isArray(selected) ? selected : [selected]
        addFiles(paths)
      }
    } catch {
      // Dialog dismissed or unavailable — nothing to add
    }
  }

  // Add files to playlist. Native drops arrive unfiltered, so skip paths
  // already queued (duplicate keys break selection) and non-audio files.
  const addFiles = (paths) => {
    const fresh = paths
      .map(p => (typeof p === 'string' ? p : p.path))
      .filter(path => AUDIO_EXTS.includes(path.split('.').pop()?.toLowerCase()))
      .filter(path => !tracks.some(t => t.path === path))
    const newTracks = [...new Set(fresh)].map((path, i) => ({
      path,
      name: path.split('/').pop().split('\\').pop(),
      size: 0,
      color: CH_COLORS[(tracks.length + i) % CH_COLORS.length],
      label: `Speaker ${tracks.length + i + 1}`,
    }))
    if (!newTracks.length) return
    setTracks(prev => [...prev, ...newTracks])
    if (!activeTrack) {
      setActiveTrack(newTracks[0])
    }
  }

  // Handle drag & drop
  const handleDrop = (e) => {
    e.preventDefault()
    setDragOver(false)
    // Tauri drag-drop handled via event listener in App
  }

  // Claim native drops for the playlist while this tab is mounted.
  // No dependency array: re-register every render so addFiles sees fresh state.
  useEffect(() => {
    if (!dropHandlerRef) return undefined
    dropHandlerRef.current = addFiles
    return () => { dropHandlerRef.current = null }
  })

  // Play / pause
  const toggle = () => {
    const a = audioRef.current
    if (!a || !activeTrack) return
    if (playing) {
      a.pause()
      setPlaying(false)
    } else {
      a.play().then(() => setPlaying(true)).catch(() => {})
    }
  }

  // Skip to next/prev track
  const skip = (dir) => {
    if (!activeTrack || tracks.length === 0) return
    const idx = tracks.findIndex(t => t.path === activeTrack.path)
    const next = (idx + dir + tracks.length) % tracks.length
    setActiveTrack(tracks[next])
    setPlaying(false)
    setCurrentTime(0)
  }

  // Remove track
  const removeTrack = (path) => {
    setTracks(prev => prev.filter(t => t.path !== path))
    if (activeTrack?.path === path) {
      setActiveTrack(tracks.find(t => t.path !== path) || null)
      setPlaying(false)
    }
  }

  // Reload on track change; keep playing if we got here by auto-advance
  useEffect(() => {
    if (activeTrack && audioRef.current) {
      audioRef.current.load()
      setCurrentTime(0)
      setDuration(0)
      if (autoAdvanceRef.current) {
        autoAdvanceRef.current = false
        audioRef.current.play().then(() => setPlaying(true)).catch(() => {})
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTrack?.path])

  const audioSrc = activeTrack ? convertFileSrc(activeTrack.path) : ''

  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5 flex flex-col gap-3.5">

          {/* ── Now Playing (hidden until something is queued) ─────────── */}
          {tracks.length > 0 && (
          <Card>
            <CardHeader>
              <CardTitle>NOW PLAYING</CardTitle>
            </CardHeader>

            {activeTrack ? (
              <div className="flex flex-col gap-3 px-4 py-3">
                <div className="flex items-center gap-3">
                  <span className="w-2.5 h-2.5 rounded-full shrink-0" style={{ background: activeTrack.color }} />
                  <div className="flex flex-col min-w-0">
                    <span className="text-sm font-semibold text-foreground">{activeTrack.name}</span>
                    <span className="text-[11px] text-[hsl(var(--text2))] font-mono">{activeTrack.label}</span>
                  </div>
                </div>

                <audio ref={audioRef} src={audioSrc} preload="metadata"
                  onLoadedMetadata={e => setDuration(e.target.duration)}
                  onTimeUpdate={e => setCurrentTime(e.target.currentTime)}
                  onEnded={() => {
                    setPlaying(false)
                    // Continue through the playlist; stop after the last track
                    const idx = tracks.findIndex(t => t.path === activeTrack.path)
                    if (idx >= 0 && idx < tracks.length - 1) {
                      autoAdvanceRef.current = true
                      skip(1)
                    }
                  }}
                />

                {/* Waveform visualization with bookmarks */}
                <Waveform
                  audioSrc={audioSrc}
                  color={activeTrack.color}
                  currentTime={currentTime}
                  duration={duration}
                  height={56}
                  onSeek={t => { if (audioRef.current) audioRef.current.currentTime = t }}
                  markers={bookmarks.filter(b => b.trackPath === activeTrack?.path)}
                />

                <div className="flex items-center justify-between">
                  <span className="font-mono text-[11px] text-[hsl(var(--sub))] min-w-[45px]">{fmtTime(currentTime)}</span>
                  <div className="flex items-center gap-2">
                    <button
                      className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground"
                      onClick={() => skip(-1)}
                      title="Previous"
                      aria-label="Previous track"
                    >
                      <SkipBack size={14} fill="currentColor" />
                    </button>
                    <button
                      className="w-11 h-11 bg-primary text-white rounded-full flex items-center justify-center transition-colors hover:bg-gold-hi"
                      onClick={toggle}
                      title={playing ? 'Pause' : 'Play'}
                      aria-label={playing ? 'Pause' : 'Play'}
                    >
                      {playing
                        ? <Pause size={16} fill="currentColor" />
                        : <Play size={16} fill="currentColor" />}
                    </button>
                    <button
                      className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground"
                      onClick={() => skip(1)}
                      title="Next"
                      aria-label="Next track"
                    >
                      <SkipForward size={14} fill="currentColor" />
                    </button>
                  </div>
                  <span className="font-mono text-[11px] text-[hsl(var(--sub))] min-w-[45px] text-right">{fmtTime(duration)}</span>
                  <button
                    className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground"
                    title="Add bookmark at current position"
                    aria-label="Add bookmark"
                    onClick={() => {
                      if (!activeTrack || !currentTime) return
                      setBookmarks(prev => [...prev, {
                        time: currentTime,
                        label: `${fmtTime(currentTime)}`,
                        color: '#c44e4e',
                        trackPath: activeTrack.path,
                      }])
                    }}
                  >
                    <Bookmark size={14} />
                  </button>
                </div>
              </div>
            ) : (
              <p className="text-[13px] text-[hsl(var(--sub))] text-center py-5">
                Select a track below to start listening.
              </p>
            )}

            {/* Bookmarks for active track */}
            {activeTrack && bookmarks.filter(b => b.trackPath === activeTrack.path).length > 0 && (
              <div className="px-4 py-2 border-t border-border/60">
                <span className="font-mono text-[9.5px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))] block mb-1.5">BOOKMARKS</span>
                <div className="flex flex-wrap gap-1.5">
                  {bookmarks.filter(b => b.trackPath === activeTrack.path).map((b, i) => (
                    <div key={i} className="flex items-center gap-1 px-2 py-0.5 bg-secondary rounded-md text-xs">
                      <button
                        className="font-mono text-[11px] text-foreground hover:text-primary transition-colors"
                        onClick={() => { if (audioRef.current) audioRef.current.currentTime = b.time }}
                      >
                        {b.label}
                      </button>
                      <button
                        className="text-[hsl(var(--sub))] hover:text-destructive transition-colors"
                        aria-label="Remove bookmark"
                        onClick={() => {
                          const trackBms = bookmarks.filter(b => b.trackPath === activeTrack.path)
                          const toRemove = trackBms[i]
                          setBookmarks(prev => prev.filter(b => b !== toRemove))
                        }}
                      >
                        <X size={10} />
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </Card>
          )}

          {/* ── Playlist ────────────────────────────────────── */}
          <Card>
            <CardHeader>
              <CardTitle>PLAYLIST</CardTitle>
              <Button size="sm" onClick={browseFiles}>Browse</Button>
            </CardHeader>

            {tracks.length === 0 ? (
              <div
                role="button"
                tabIndex={0}
                aria-label="Add audio files to the playlist: drop them here or press Enter to browse"
                className={cn(
                  'flex flex-col items-center justify-center gap-2 border-2 border-dashed border-border rounded-lg m-3 py-10 px-8 text-center cursor-pointer transition-colors hover:border-primary',
                  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
                  dragOver && 'border-primary bg-primary/5'
                )}
                onDragOver={e => { e.preventDefault(); setDragOver(true) }}
                onDragLeave={() => setDragOver(false)}
                onDrop={handleDrop}
                onClick={browseFiles}
                onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); browseFiles() } }}
              >
                <WaveformIcon />
                <p className="text-[13px] font-semibold text-foreground">Drop audio files here to listen</p>
                <p className="text-[11px] text-[hsl(var(--sub))]">No conversion needed — WAV · MP3 · FLAC · Opus · M4A · OGG and more</p>
              </div>
            ) : (
              <div className="p-2">
                {tracks.map((t, i) => (
                  <div
                    key={t.path}
                    className={cn(
                      'group flex items-center gap-2.5 px-3 py-2 rounded-md cursor-pointer transition-colors hover:bg-secondary/50',
                      activeTrack?.path === t.path && 'bg-secondary border-l-[3px] border-l-primary',
                      dragIdx === i && 'opacity-50'
                    )}
                    draggable
                    onDragStart={() => setDragIdx(i)}
                    onDragOver={e => { e.preventDefault(); e.currentTarget.classList.add('bg-secondary/30') }}
                    onDragLeave={e => e.currentTarget.classList.remove('bg-secondary/30')}
                    onDrop={e => {
                      e.currentTarget.classList.remove('bg-secondary/30')
                      if (dragIdx !== null && dragIdx !== i) {
                        setTracks(prev => {
                          const next = [...prev]
                          const [moved] = next.splice(dragIdx, 1)
                          next.splice(i, 0, moved)
                          return next
                        })
                      }
                      setDragIdx(null)
                    }}
                    onDragEnd={() => setDragIdx(null)}
                    onClick={() => setActiveTrack(t)}
                  >
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: t.color }} />
                    <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">{i + 1}</span>
                    <div className="flex flex-col flex-1 min-w-0">
                      <span className="text-xs text-foreground truncate">{t.name}</span>
                      <input
                        className="text-[10px] text-[hsl(var(--text2))] bg-transparent border-none p-0 font-mono w-[120px] focus:text-primary focus:outline-none"
                        value={t.label}
                        placeholder="Speaker name"
                        onClick={e => e.stopPropagation()}
                        onChange={e => setTracks(prev => prev.map((tr, j) => j === i ? {...tr, label: e.target.value} : tr))}
                      />
                    </div>
                    {activeTrack?.path === t.path && playing && (
                      <span className="text-primary text-xs shrink-0">
                        <Play size={10} fill="currentColor" />
                      </span>
                    )}
                    <button
                      className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive shrink-0"
                      aria-label="Remove track"
                      onClick={e => { e.stopPropagation(); removeTrack(t.path) }}
                    >
                      <X size={9} />
                    </button>
                  </div>
                ))}
                <Button size="sm" className="mt-2 ml-1" onClick={browseFiles}>
                  <Plus size={12} /> Add files
                </Button>
              </div>
            )}
          </Card>

        </div>
      </div>
    </>
  )
}
