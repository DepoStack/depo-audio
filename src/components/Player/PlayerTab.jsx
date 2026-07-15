import { useState, useRef, useEffect } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Play, Pause, SkipBack, SkipForward, Bookmark, X, Plus, Repeat, Copy, Check } from 'lucide-react'
import { cn } from '../../lib/utils'
import { CH_COLORS } from '../../constants'
import { fmtTime, sortRecordingChunks } from '../../utils'
import { AUDIO_EXTS, SPEED_STEPS, loadSpeed, cycleSpeedStep, loadBookmarks, freshAudioPaths, bookmarksToText } from '../../lib/player'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle } from '../ui/card'
import Waveform from '../common/Waveform'
import { WaveformIcon } from '../common/Icons'
import Transcript from './Transcript'

// ── Global Audio Player ─────────────────────────────────────────────────────
//
// Play any audio file directly — no conversion needed. Supports multi-channel
// files with color-coded speaker tracks. Drop files or browse to start.
// Accepted extensions, speed steps, and bookmark handling live in
// lib/player.js (characterization-tested).
//
// Keyboard transport (when not typing in a field):
//   Space / K  play-pause      ← / →  seek ±5s       J / L  seek ±10s
//   ↑ / ↓      speed up/down    [ / ]  prev/next      B      bookmark

export default function PlayerTab({ dropHandlerRef }) {
  const [tracks, setTracks] = useState([])       // { path, name, size, channels, duration, color }
  const [activeTrack, setActiveTrack] = useState(null)
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [dragOver, setDragOver] = useState(false)
  // Playback speed persists across sessions (transcription workflows live here)
  const [speed, setSpeedState] = useState(() => loadSpeed(localStorage.getItem('player-speed')))
  // A-B loop points (session-only, reset per track)
  const [loopA, setLoopA] = useState(null)
  const [loopB, setLoopB] = useState(null)
  const [copied, setCopied] = useState(false)
  // Bookmarks persist across sessions; validate the shape — corrupt storage
  // must not crash the tab on every launch
  const [bookmarks, setBookmarks] = useState(() => loadBookmarks(localStorage.getItem('player-bookmarks')))
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
  // FTR .trm chunks are ordered chronologically — but only the incoming
  // batch: existing tracks keep any manual drag-reorder the user made.
  const addFiles = (paths) => {
    const newTracks = sortRecordingChunks(freshAudioPaths(paths, tracks)).map((path, i) => ({
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

  // Seek by a relative amount, clamped to the track
  const seekBy = (delta) => {
    const a = audioRef.current
    if (!a) return
    a.currentTime = Math.max(0, Math.min((a.currentTime || 0) + delta, a.duration || 0))
  }

  // Apply and persist playback speed
  const applySpeed = (s) => {
    setSpeedState(s)
    try { localStorage.setItem('player-speed', String(s)) } catch { /* ignore */ }
    if (audioRef.current) audioRef.current.playbackRate = s
  }
  const cycleSpeed = (dir) => applySpeed(cycleSpeedStep(speed, dir))

  // Add a bookmark at the current position
  const addBookmark = () => {
    if (!activeTrack) return
    const t = audioRef.current?.currentTime ?? currentTime
    setBookmarks(prev => [...prev, {
      time: t, label: fmtTime(t), color: '#c44e4e', trackPath: activeTrack.path,
    }])
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

  // Remove track. When removing the active one, advance to the track that took
  // its slot (the next track), falling back to the previous if it was last —
  // rather than always jumping to the first track.
  const removeTrack = (path) => {
    const idx = tracks.findIndex(t => t.path === path)
    const next = tracks.filter(t => t.path !== path)
    setTracks(next)
    if (activeTrack?.path === path) {
      setActiveTrack(next[idx] ?? next[idx - 1] ?? null)
      setPlaying(false)
    }
  }

  // Copy the active track's bookmarks as "MM:SS  label" lines (for transcripts)
  const copyBookmarks = async () => {
    if (!activeTrack) return
    const text = bookmarksToText(bookmarks, activeTrack.path)
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch { /* clipboard unavailable */ }
  }

  // Keyboard transport. Registered once; reads latest actions/state via a ref
  // so it never goes stale and never re-binds. The ref is refreshed after each
  // render (refs must not be written during render).
  const actionsRef = useRef({})
  useEffect(() => {
    actionsRef.current = { hasTrack: !!activeTrack, toggle, seekBy, skip, cycleSpeed, addBookmark }
  })
  useEffect(() => {
    const onKey = (e) => {
      const el = e.target
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return
      if (e.metaKey || e.ctrlKey || e.altKey) return
      const a = actionsRef.current
      if (!a.hasTrack) return
      switch (e.key) {
        case ' ': case 'k': case 'K': e.preventDefault(); a.toggle(); break
        case 'ArrowLeft': e.preventDefault(); a.seekBy(-5); break
        case 'ArrowRight': e.preventDefault(); a.seekBy(5); break
        case 'j': case 'J': e.preventDefault(); a.seekBy(-10); break
        case 'l': case 'L': e.preventDefault(); a.seekBy(10); break
        case 'ArrowUp': e.preventDefault(); a.cycleSpeed(1); break
        case 'ArrowDown': e.preventDefault(); a.cycleSpeed(-1); break
        case '[': e.preventDefault(); a.skip(-1); break
        case ']': e.preventDefault(); a.skip(1); break
        case 'b': case 'B': e.preventDefault(); a.addBookmark(); break
        default: break
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Reload on track change; keep playing if we got here by auto-advance.
  // Reset the A-B loop since its points belong to the previous track.
  useEffect(() => {
    if (activeTrack && audioRef.current) {
      audioRef.current.load()
      audioRef.current.playbackRate = speed
      setCurrentTime(0)
      setDuration(0)
      setLoopA(null)
      setLoopB(null)
      if (autoAdvanceRef.current) {
        autoAdvanceRef.current = false
        audioRef.current.play().then(() => setPlaying(true)).catch(() => {})
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTrack?.path])

  const audioSrc = activeTrack ? convertFileSrc(activeTrack.path) : ''
  const trackBookmarks = activeTrack ? bookmarks.filter(b => b.trackPath === activeTrack.path) : []

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
                  onLoadedMetadata={e => { setDuration(e.target.duration); e.target.playbackRate = speed }}
                  onTimeUpdate={e => {
                    const t = e.target.currentTime
                    setCurrentTime(t)
                    // A-B loop: jump back to A when B is reached
                    if (loopA != null && loopB != null && loopB > loopA && t >= loopB) {
                      e.target.currentTime = loopA
                    }
                  }}
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
                  markers={trackBookmarks}
                />

                <div className="flex items-center justify-between">
                  <span className="font-mono text-[11px] text-[hsl(var(--sub))] min-w-[45px]">{fmtTime(currentTime)}</span>
                  <div className="flex items-center gap-2">
                    <button
                      className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                      onClick={() => skip(-1)}
                      title="Previous track ( [ )"
                      aria-label="Previous track"
                    >
                      <SkipBack size={14} fill="currentColor" />
                    </button>
                    <button
                      className="w-11 h-11 bg-primary text-white rounded-full flex items-center justify-center transition-colors hover:bg-gold-hi focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                      onClick={toggle}
                      title={playing ? 'Pause (Space)' : 'Play (Space)'}
                      aria-label={playing ? 'Pause' : 'Play'}
                    >
                      {playing
                        ? <Pause size={16} fill="currentColor" />
                        : <Play size={16} fill="currentColor" />}
                    </button>
                    <button
                      className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                      onClick={() => skip(1)}
                      title="Next track ( ] )"
                      aria-label="Next track"
                    >
                      <SkipForward size={14} fill="currentColor" />
                    </button>
                  </div>
                  <span className="font-mono text-[11px] text-[hsl(var(--sub))] min-w-[45px] text-right">{fmtTime(duration)}</span>
                  <button
                    className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                    title="Add bookmark at current position (B)"
                    aria-label="Add bookmark"
                    onClick={addBookmark}
                  >
                    <Bookmark size={14} />
                  </button>
                </div>

                {/* Secondary transport: playback speed + A-B loop */}
                <div className="flex items-center justify-between flex-wrap gap-2 pt-0.5">
                  <div className="flex items-center gap-1">
                    <span className="text-[9.5px] font-mono tracking-wider uppercase text-[hsl(var(--sub))] mr-1">Speed</span>
                    {SPEED_STEPS.map(s => (
                      <button
                        key={s}
                        onClick={() => applySpeed(s)}
                        aria-pressed={speed === s}
                        title={`${s}× playback speed`}
                        className={cn(
                          'px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
                          speed === s ? 'bg-[hsl(var(--gold-dim))] text-foreground' : 'text-[hsl(var(--sub))] hover:text-foreground'
                        )}
                      >
                        {s}×
                      </button>
                    ))}
                  </div>
                  <div className="flex items-center gap-1">
                    <Repeat size={11} className={cn('mr-0.5', loopA != null && loopB != null ? 'text-foreground' : 'text-[hsl(var(--sub))]')} />
                    <button
                      onClick={() => setLoopA(currentTime)}
                      title="Set loop start to current position"
                      className={cn(
                        'px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
                        loopA != null ? 'bg-[hsl(var(--gold-dim))] text-foreground' : 'text-[hsl(var(--sub))] hover:text-foreground'
                      )}
                    >
                      {loopA != null ? `A ${fmtTime(loopA)}` : 'Set A'}
                    </button>
                    <button
                      onClick={() => setLoopB(currentTime)}
                      title="Set loop end to current position"
                      className={cn(
                        'px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
                        loopB != null ? 'bg-[hsl(var(--gold-dim))] text-foreground' : 'text-[hsl(var(--sub))] hover:text-foreground'
                      )}
                    >
                      {loopB != null ? `B ${fmtTime(loopB)}` : 'Set B'}
                    </button>
                    {(loopA != null || loopB != null) && (
                      <button
                        onClick={() => { setLoopA(null); setLoopB(null) }}
                        title="Clear A-B loop"
                        className="px-1.5 py-0.5 rounded text-[10px] font-mono text-[hsl(var(--sub))] hover:text-destructive transition-colors"
                      >
                        clear
                      </button>
                    )}
                  </div>
                </div>
              </div>
            ) : (
              <p className="text-[13px] text-[hsl(var(--sub))] text-center py-5">
                Select a track below to start listening.
              </p>
            )}

            {/* Bookmarks for active track — editable labels + export */}
            {activeTrack && trackBookmarks.length > 0 && (
              <div className="px-4 py-2 border-t border-border/60">
                <div className="flex items-center justify-between mb-1.5">
                  <span className="font-mono text-[9.5px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">Bookmarks</span>
                  <button
                    onClick={copyBookmarks}
                    title="Copy bookmarks to clipboard"
                    className="flex items-center gap-1 text-[10px] text-[hsl(var(--sub))] hover:text-foreground transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring rounded px-1"
                  >
                    {copied ? <><Check size={11} /> Copied</> : <><Copy size={11} /> Copy</>}
                  </button>
                </div>
                <div className="flex flex-col gap-1">
                  {trackBookmarks
                    .slice()
                    .sort((a, b) => a.time - b.time)
                    .map((b) => (
                      <div key={`${b.time}-${b.trackPath}`} className="flex items-center gap-2 group">
                        <button
                          className="font-mono text-[11px] text-foreground hover:underline shrink-0 w-12 text-left focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring rounded"
                          title="Jump to bookmark"
                          onClick={() => { if (audioRef.current) audioRef.current.currentTime = b.time }}
                        >
                          {fmtTime(b.time)}
                        </button>
                        <input
                          className="flex-1 min-w-0 bg-transparent border-none p-0 text-[11px] text-foreground focus:text-foreground focus:outline-hidden"
                          value={b.label}
                          placeholder="Add a note…"
                          onChange={e => setBookmarks(prev => prev.map(x => x === b ? { ...x, label: e.target.value } : x))}
                        />
                        <button
                          className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 hover:text-destructive transition-all shrink-0"
                          aria-label="Remove bookmark"
                          onClick={() => setBookmarks(prev => prev.filter(x => x !== b))}
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

          {/* ── Transcript (synced editor, per track) ─────────── */}
          {activeTrack && (
            <Transcript
              key={activeTrack.path}
              trackPath={activeTrack.path}
              currentTime={currentTime}
              playing={playing}
              onSeek={t => { if (audioRef.current) audioRef.current.currentTime = t }}
            />
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
                  'focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring',
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
                        className="text-[10px] text-[hsl(var(--text2))] bg-transparent border-none p-0 font-mono w-[120px] focus:text-foreground focus:outline-hidden"
                        value={t.label}
                        placeholder="Speaker name"
                        onClick={e => e.stopPropagation()}
                        onChange={e => setTracks(prev => prev.map((tr, j) => j === i ? {...tr, label: e.target.value} : tr))}
                      />
                    </div>
                    {activeTrack?.path === t.path && playing && (
                      <span className="text-foreground text-xs shrink-0">
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

          {/* Keyboard hint */}
          {tracks.length > 0 && (
            <p className="text-[10px] text-[hsl(var(--sub))] text-center font-mono">
              Space play · ←/→ ±5s · J/L ±10s · ↑/↓ speed · [ / ] track · B bookmark
            </p>
          )}

        </div>
      </div>
    </>
  )
}
