import { useState, useRef, useEffect } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Play, Pause, SkipBack, SkipForward, Bookmark, X, Plus, Repeat, Copy, Check } from 'lucide-react'
import { cn } from '../../lib/utils'
import { fmtTime, sortRecordingChunks } from '../../utils'
import { useSpeakerColors, speakerColorAt, SPEAKER_COUNT } from '../../lib/speakerColors'
import { AUDIO_EXTS, SPEED_STEPS, loadSpeed, cycleSpeedStep, loadBookmarks, freshAudioPaths, bookmarksToText } from '../../lib/player'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Segmented } from '../ui/segmented'
import Waveform from '../common/Waveform'
import { WaveformIcon } from '../common/Icons'

// ── Global Audio Player ─────────────────────────────────────────────────────
//
// Play any audio file directly — no conversion needed. Multi-channel files get
// color-coded speaker tracks (colors from the --speaker-N design tokens).
//
// Layout: a Now Playing surface (large seekable waveform) + a playlist/
// bookmarks rail scroll above a PERSISTENT transport bar (controls + a thin
// progress scrubber) that never scrolls away, so play/pause and position
// stay reachable no matter how far you scroll.
//
// Keyboard transport (when not typing in a field):
//   Space / K  play-pause      ← / →  seek ±5s       J / L  seek ±10s
//   ↑ / ↓      speed up/down    [ / ]  prev/next      B      bookmark

export default function PlayerTab({ dropHandlerRef }) {
  const [tracks, setTracks] = useState([])       // { path, name, colorIndex, label }
  const [activeTrack, setActiveTrack] = useState(null)
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [dragOver, setDragOver] = useState(false)
  const [speed, setSpeedState] = useState(() => loadSpeed(localStorage.getItem('player-speed')))
  const [loopA, setLoopA] = useState(null)
  const [loopB, setLoopB] = useState(null)
  const [copied, setCopied] = useState(false)
  const [bookmarks, setBookmarks] = useState(() => loadBookmarks(localStorage.getItem('player-bookmarks')))
  useEffect(() => {
    try { localStorage.setItem('player-bookmarks', JSON.stringify(bookmarks)) } catch { /* storage full or unavailable */ }
  }, [bookmarks])
  const [dragIdx, setDragIdx] = useState(null) // playlist drag-reorder
  const audioRef = useRef(null)
  const autoAdvanceRef = useRef(false) // play next track once it loads

  // Theme-aware concrete speaker colors, resolved from the design tokens.
  const speakerColors = useSpeakerColors()
  const colorOf = (track) => speakerColorAt(speakerColors, track?.colorIndex ?? 0)

  const browseFiles = async () => {
    try {
      const selected = await open({ multiple: true, filters: [{ name: 'Audio', extensions: AUDIO_EXTS }] })
      if (selected) addFiles(Array.isArray(selected) ? selected : [selected])
    } catch { /* dialog dismissed */ }
  }

  // Add files. Native drops arrive unfiltered, so skip already-queued and
  // non-audio paths. FTR .trm chunks order chronologically (incoming batch
  // only — existing tracks keep any manual drag-reorder).
  const addFiles = (paths) => {
    const newTracks = sortRecordingChunks(freshAudioPaths(paths, tracks)).map((path, i) => ({
      path,
      name: path.split('/').pop().split('\\').pop(),
      colorIndex: (tracks.length + i) % SPEAKER_COUNT,
      label: `Speaker ${tracks.length + i + 1}`,
    }))
    if (!newTracks.length) return
    setTracks(prev => [...prev, ...newTracks])
    if (!activeTrack) setActiveTrack(newTracks[0])
  }

  const handleDrop = (e) => { e.preventDefault(); setDragOver(false) }

  // Claim native drops for the playlist while mounted. No dep array: re-register
  // every render so addFiles sees fresh state.
  useEffect(() => {
    if (!dropHandlerRef) return undefined
    dropHandlerRef.current = addFiles
    return () => { dropHandlerRef.current = null }
  })

  const toggle = () => {
    const a = audioRef.current
    if (!a || !activeTrack) return
    if (playing) { a.pause(); setPlaying(false) }
    else a.play().then(() => setPlaying(true)).catch(() => {})
  }

  const seekBy = (delta) => {
    const a = audioRef.current
    if (!a) return
    a.currentTime = Math.max(0, Math.min((a.currentTime || 0) + delta, a.duration || 0))
  }
  const seekTo = (t) => { if (audioRef.current) audioRef.current.currentTime = t }

  const applySpeed = (s) => {
    setSpeedState(s)
    try { localStorage.setItem('player-speed', String(s)) } catch { /* ignore */ }
    if (audioRef.current) audioRef.current.playbackRate = s
  }
  const cycleSpeed = (dir) => applySpeed(cycleSpeedStep(speed, dir))

  const addBookmark = () => {
    if (!activeTrack) return
    const t = audioRef.current?.currentTime ?? currentTime
    setBookmarks(prev => [...prev, { time: t, label: fmtTime(t), trackPath: activeTrack.path }])
  }

  const skip = (dir) => {
    if (!activeTrack || tracks.length === 0) return
    const idx = tracks.findIndex(t => t.path === activeTrack.path)
    const next = (idx + dir + tracks.length) % tracks.length
    setActiveTrack(tracks[next])
    setPlaying(false)
    setCurrentTime(0)
  }

  const removeTrack = (path) => {
    const idx = tracks.findIndex(t => t.path === path)
    const next = tracks.filter(t => t.path !== path)
    setTracks(next)
    if (activeTrack?.path === path) {
      setActiveTrack(next[idx] ?? next[idx - 1] ?? null)
      setPlaying(false)
    }
  }

  const copyBookmarks = async () => {
    if (!activeTrack) return
    try {
      await navigator.clipboard.writeText(bookmarksToText(bookmarks, activeTrack.path))
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch { /* clipboard unavailable */ }
  }

  // Keyboard transport. Registered once; reads latest actions/state via a ref.
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

  // Reload on track change; keep playing if we got here by auto-advance. Reset
  // the A-B loop since its points belong to the previous track.
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
  const activeColor = colorOf(activeTrack)
  const loopActive = loopA != null && loopB != null && loopB > loopA

  const speedOptions = SPEED_STEPS.map(s => ({ value: s, label: `${s}×`, title: `${s}× playback speed` }))

  // ── Empty state ───────────────────────────────────────────────────────────
  if (tracks.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5">
          <div
            role="button"
            tabIndex={0}
            aria-label="Add audio files to the playlist: drop them here or press Enter to browse"
            className={cn(
              'flex flex-col items-center justify-center gap-2 border-2 border-dashed rounded-xl py-16 px-8 text-center cursor-pointer transition-colors',
              'focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring',
              dragOver ? 'border-primary bg-[hsl(var(--gold-dim))]' : 'border-border/60 hover:border-border hover:bg-secondary/30'
            )}
            onDragOver={e => { e.preventDefault(); setDragOver(true) }}
            onDragLeave={() => setDragOver(false)}
            onDrop={handleDrop}
            onClick={browseFiles}
            onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); browseFiles() } }}
          >
            <WaveformIcon />
            <p className="text-[13px] font-semibold text-foreground">Drop audio files here to listen</p>
            <p className="text-[11px] text-[hsl(var(--sub))]">No conversion needed — WAV · MP3 · FLAC · Opus · M4A · OGG and more. Multi-file sessions play back to back.</p>
          </div>
        </div>
      </div>
    )
  }

  // ── Workspace (Now Playing + playlist rail) over a fixed transport bar ────
  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5 grid gap-3.5 md:grid-cols-[minmax(0,1fr)_300px]">

          {/* Now Playing — the main surface: a large seekable waveform */}
          {activeTrack && (
            <div className="md:order-1 order-2 min-w-0">
              <Card>
                <CardHeader><CardTitle>Now Playing</CardTitle></CardHeader>
                <CardContent className="p-4 flex flex-col gap-3">
                  <div className="flex items-baseline gap-2 min-w-0">
                    <span className="w-2 h-2 rounded-full shrink-0 self-center" style={{ background: activeColor }} />
                    <span className="text-[15px] font-semibold text-foreground font-serif truncate">{activeTrack.name}</span>
                    <span className="text-[11px] font-mono text-[hsl(var(--sub))] shrink-0">· {activeTrack.label}</span>
                  </div>
                  <Waveform
                    audioSrc={audioSrc}
                    color={activeColor}
                    currentTime={currentTime}
                    duration={duration}
                    height={96}
                    onSeek={seekTo}
                    markers={trackBookmarks}
                    loop={loopActive ? { a: loopA, b: loopB } : null}
                  />
                  <div className="flex items-center justify-between font-mono text-[11px] text-[hsl(var(--sub))] tabular-nums">
                    <span>{fmtTime(currentTime)}</span>
                    <span>{fmtTime(duration)}</span>
                  </div>
                </CardContent>
              </Card>
            </div>
          )}

          {/* Rail: playlist + bookmarks */}
          <aside className="md:order-2 order-1 flex flex-col gap-3.5 min-w-0">
            <Card>
              <CardHeader>
                <CardTitle>Playlist</CardTitle>
                <Button size="sm" onClick={browseFiles}><Plus size={12} /> Add</Button>
              </CardHeader>
              <CardContent className="p-1.5">
                {tracks.map((t, i) => (
                  <div
                    key={t.path}
                    className={cn(
                      'group flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer transition-colors hover:bg-secondary/50',
                      activeTrack?.path === t.path && 'bg-secondary',
                      dragIdx === i && 'opacity-50'
                    )}
                    draggable
                    onDragStart={() => setDragIdx(i)}
                    onDragOver={e => { e.preventDefault() }}
                    onDrop={e => {
                      e.preventDefault()
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
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: colorOf(t) }} />
                    <div className="flex flex-col flex-1 min-w-0">
                      <span className="text-[11px] text-foreground truncate" title={t.name}>{t.name}</span>
                      <input
                        className="text-[10px] text-[hsl(var(--text2))] bg-transparent border-none p-0 font-mono w-full focus:text-foreground focus:outline-hidden"
                        value={t.label}
                        placeholder="Speaker name"
                        onClick={e => e.stopPropagation()}
                        onChange={e => setTracks(prev => prev.map((tr, j) => j === i ? { ...tr, label: e.target.value } : tr))}
                      />
                    </div>
                    {activeTrack?.path === t.path && playing
                      ? <Play size={10} fill="currentColor" className="text-foreground shrink-0" />
                      : <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0 group-hover:hidden">{i + 1}</span>}
                    <button
                      className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive shrink-0"
                      aria-label={`Remove ${t.name}`}
                      onClick={e => { e.stopPropagation(); removeTrack(t.path) }}
                    >
                      <X size={11} />
                    </button>
                  </div>
                ))}
              </CardContent>
            </Card>

            {/* Bookmarks for the active track */}
            {activeTrack && (
              <Card>
                <CardHeader>
                  <CardTitle>Bookmarks</CardTitle>
                  {trackBookmarks.length > 0 && (
                    <button
                      onClick={copyBookmarks}
                      title="Copy bookmarks to clipboard as timestamped lines"
                      className="flex items-center gap-1 text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring rounded px-1"
                    >
                      {copied ? <><Check size={11} /> Copied</> : <><Copy size={11} /> Copy</>}
                    </button>
                  )}
                </CardHeader>
                <CardContent className="p-2">
                  {trackBookmarks.length === 0 ? (
                    <p className="px-1 py-1 text-[11px] text-[hsl(var(--sub))]">Press <kbd className="font-mono">B</kbd> or the bookmark button to mark the current spot.</p>
                  ) : (
                    <div className="flex flex-col gap-0.5">
                      {trackBookmarks.slice().sort((a, b) => a.time - b.time).map((b) => (
                        <div key={`${b.time}-${b.trackPath}`} className="flex items-center gap-2 group px-1">
                          <button
                            className="font-mono text-[11px] text-[hsl(var(--text2))] hover:text-foreground shrink-0 w-11 text-left focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring rounded"
                            title="Jump to bookmark"
                            onClick={() => seekTo(b.time)}
                          >
                            {fmtTime(b.time)}
                          </button>
                          <input
                            className="flex-1 min-w-0 bg-transparent border-none p-0 text-[11px] text-foreground focus:outline-hidden"
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
                  )}
                </CardContent>
              </Card>
            )}
          </aside>
        </div>
      </div>

      {/* ── Persistent transport bar ─────────────────────────────────────── */}
      {activeTrack && (
        <div className="shrink-0 border-t border-border bg-card px-4 md:px-6 py-2.5">
          <audio ref={audioRef} src={audioSrc} preload="metadata"
            onLoadedMetadata={e => { setDuration(e.target.duration); e.target.playbackRate = speed }}
            onTimeUpdate={e => {
              const t = e.target.currentTime
              setCurrentTime(t)
              if (loopActive && t >= loopB) e.target.currentTime = loopA
            }}
            onEnded={() => {
              setPlaying(false)
              const idx = tracks.findIndex(t => t.path === activeTrack.path)
              if (idx >= 0 && idx < tracks.length - 1) { autoAdvanceRef.current = true; skip(1) }
            }}
          />

          <div className="w-full max-w-[1100px] mx-auto flex items-center gap-3 md:gap-4">
            {/* Transport buttons */}
            <div className="flex items-center gap-1.5 shrink-0">
              <button
                className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                onClick={() => skip(-1)} title="Previous track ( [ )" aria-label="Previous track">
                <SkipBack size={15} fill="currentColor" />
              </button>
              <button
                className="w-10 h-10 bg-primary text-primary-foreground rounded-full flex items-center justify-center transition-colors hover:bg-gold-hi focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                onClick={toggle} title={playing ? 'Pause (Space)' : 'Play (Space)'} aria-label={playing ? 'Pause' : 'Play'}>
                {playing ? <Pause size={17} fill="currentColor" /> : <Play size={17} fill="currentColor" className="ml-0.5" />}
              </button>
              <button
                className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                onClick={() => skip(1)} title="Next track ( ] )" aria-label="Next track">
                <SkipForward size={15} fill="currentColor" />
              </button>
            </div>

            {/* Track identity + scrubber (grows) */}
            <div className="flex-1 min-w-0 flex flex-col gap-1">
              <div className="flex items-center gap-2 min-w-0">
                <span className="w-2 h-2 rounded-full shrink-0" style={{ background: activeColor }} />
                <span className="text-[12px] font-medium text-foreground truncate">{activeTrack.name}</span>
                <span className="text-[10px] font-mono text-[hsl(var(--sub))] shrink-0 hidden sm:inline">· {activeTrack.label}</span>
                <span className="ml-auto font-mono text-[10px] text-[hsl(var(--sub))] shrink-0 tabular-nums">
                  {fmtTime(currentTime)} / {fmtTime(duration)}
                </span>
              </div>
              {/* Thin progress scrubber — the detailed waveform lives in Now
                  Playing above, so the pinned bar stays cheap (no second
                  decode) while still showing the loop band + position. */}
              <div
                className="relative h-1.5 bg-border rounded-full cursor-pointer overflow-hidden"
                role="slider"
                aria-label="Seek"
                aria-valuemin={0}
                aria-valuemax={Math.round(duration)}
                aria-valuenow={Math.round(currentTime)}
                onClick={e => {
                  const r = e.currentTarget.getBoundingClientRect()
                  if (duration) seekTo(((e.clientX - r.left) / r.width) * duration)
                }}
              >
                {loopActive && (
                  <div className="absolute inset-y-0 bg-[hsl(var(--gold-dim))]"
                    style={{ left: `${(loopA / duration) * 100}%`, width: `${((loopB - loopA) / duration) * 100}%` }} />
                )}
                <div className="absolute inset-y-0 left-0 bg-primary rounded-full"
                  style={{ width: `${duration > 0 ? (currentTime / duration) * 100 : 0}%` }} />
              </div>
            </div>

            {/* Speed + loop + bookmark */}
            <div className="shrink-0 flex items-center gap-2">
              <Segmented
                size="sm"
                aria-label="Playback speed"
                options={speedOptions}
                value={speed}
                onChange={applySpeed}
                className="hidden lg:inline-flex"
              />
              <div className="flex items-center gap-1" title="A-B loop: repeat a passage">
                <Repeat size={13} className={cn(loopActive ? 'text-primary' : 'text-[hsl(var(--sub))]')} />
                <button
                  onClick={() => setLoopA(currentTime)}
                  className={cn('px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
                    loopA != null ? 'bg-[hsl(var(--gold-dim))] text-foreground' : 'text-[hsl(var(--sub))] hover:text-foreground')}
                  title="Set loop start">{loopA != null ? fmtTime(loopA) : 'A'}</button>
                <button
                  onClick={() => setLoopB(currentTime)}
                  className={cn('px-1.5 py-0.5 rounded text-[10px] font-mono transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
                    loopB != null ? 'bg-[hsl(var(--gold-dim))] text-foreground' : 'text-[hsl(var(--sub))] hover:text-foreground')}
                  title="Set loop end">{loopB != null ? fmtTime(loopB) : 'B'}</button>
                {(loopA != null || loopB != null) && (
                  <button onClick={() => { setLoopA(null); setLoopB(null) }} title="Clear A-B loop"
                    className="text-[hsl(var(--sub))] hover:text-destructive transition-colors">
                    <X size={12} />
                  </button>
                )}
              </div>
              <button
                className="w-8 h-8 rounded-full flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                title="Add bookmark at current position (B)" aria-label="Add bookmark" onClick={addBookmark}>
                <Bookmark size={15} />
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}
