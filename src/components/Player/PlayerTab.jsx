import { useState, useRef, useEffect } from 'react'
import { invoke, convertFileSrc } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { CH_COLORS } from '../../constants'
import { fmtTime, fmtSize } from '../../utils'
import Waveform from '../common/Waveform'

// ── Global Audio Player ─────────────────────────────────────────────────────
//
// Play any audio file directly — no conversion needed. Supports multi-channel
// files with color-coded speaker tracks. Drop files or browse to start.

export default function PlayerTab() {
  const [tracks, setTracks] = useState([])       // { path, name, size, channels, duration, color }
  const [activeTrack, setActiveTrack] = useState(null)
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [dragOver, setDragOver] = useState(false)
  const audioRef = useRef(null)

  // Browse for files
  const browseFiles = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [{ name: 'Audio', extensions: ['wav','mp3','flac','opus','ogg','m4a','aac','wma','aif','aiff','sgmca','trm','ftr','bwf'] }],
      })
      if (selected) {
        const paths = Array.isArray(selected) ? selected : [selected]
        addFiles(paths)
      }
    } catch {}
  }

  // Add files to playlist
  const addFiles = (paths) => {
    const newTracks = paths.map((p, i) => {
      const path = typeof p === 'string' ? p : p.path
      const name = path.split('/').pop().split('\\').pop()
      return {
        path,
        name,
        size: 0,
        color: CH_COLORS[(tracks.length + i) % CH_COLORS.length],
        label: `Speaker ${tracks.length + i + 1}`,
      }
    })
    setTracks(prev => [...prev, ...newTracks])
    if (!activeTrack && newTracks.length > 0) {
      setActiveTrack(newTracks[0])
    }
  }

  // Handle drag & drop
  const handleDrop = (e) => {
    e.preventDefault()
    setDragOver(false)
    // Tauri drag-drop handled via event listener in App
  }

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

  // Seek
  const seek = (e) => {
    if (!audioRef.current || !duration) return
    const r = e.currentTarget.getBoundingClientRect()
    audioRef.current.currentTime = ((e.clientX - r.left) / r.width) * duration
  }

  // Remove track
  const removeTrack = (path) => {
    setTracks(prev => prev.filter(t => t.path !== path))
    if (activeTrack?.path === path) {
      setActiveTrack(tracks.find(t => t.path !== path) || null)
      setPlaying(false)
    }
  }

  // Auto-play on track change
  useEffect(() => {
    if (activeTrack && audioRef.current) {
      audioRef.current.load()
      setCurrentTime(0)
      setDuration(0)
    }
  }, [activeTrack?.path])

  const audioSrc = activeTrack ? convertFileSrc(activeTrack.path) : ''

  return (
    <>
      <div className="main-scroll">
        <div className="content">

          {/* ── Now Playing ─────────────────────────────────── */}
          <section className="panel player-panel">
            <div className="panel-head">
              <span className="panel-label">NOW PLAYING</span>
            </div>

            {activeTrack ? (
              <div className="player-main">
                <div className="player-info">
                  <span className="player-dot" style={{background: activeTrack.color}} />
                  <div className="player-meta">
                    <span className="player-track-name">{activeTrack.name}</span>
                    <span className="player-track-label">{activeTrack.label}</span>
                  </div>
                </div>

                <audio ref={audioRef} src={audioSrc} preload="metadata"
                  onLoadedMetadata={e => setDuration(e.target.duration)}
                  onTimeUpdate={e => setCurrentTime(e.target.currentTime)}
                  onEnded={() => { setPlaying(false); skip(1) }}
                />

                {/* Waveform visualization */}
                <Waveform
                  audioSrc={audioSrc}
                  color={activeTrack.color}
                  currentTime={currentTime}
                  duration={duration}
                  height={56}
                  onSeek={t => { if (audioRef.current) audioRef.current.currentTime = t }}
                />

                <div className="player-controls">
                  <span className="player-timestamp">{fmtTime(currentTime)}</span>
                  <div className="player-btns">
                    <button className="player-btn" onClick={() => skip(-1)} title="Previous">
                      <svg width="14" height="14" viewBox="0 0 14 14" fill="none"><path d="M3 3v8M12 3L5 7l7 4V3z" fill="currentColor"/></svg>
                    </button>
                    <button className="player-btn player-btn--play" onClick={toggle} title={playing ? 'Pause' : 'Play'}>
                      {playing
                        ? <svg width="16" height="16" viewBox="0 0 16 16"><rect x="3" y="2" width="4" height="12" rx="1" fill="currentColor"/><rect x="9" y="2" width="4" height="12" rx="1" fill="currentColor"/></svg>
                        : <svg width="16" height="16" viewBox="0 0 16 16"><path d="M4 2.5l10 5.5-10 5.5V2.5z" fill="currentColor"/></svg>}
                    </button>
                    <button className="player-btn" onClick={() => skip(1)} title="Next">
                      <svg width="14" height="14" viewBox="0 0 14 14" fill="none"><path d="M11 3v8M2 3l7 4-7 4V3z" fill="currentColor"/></svg>
                    </button>
                  </div>
                  <span className="player-timestamp">{fmtTime(duration)}</span>
                </div>
              </div>
            ) : (
              <p className="player-empty">No file loaded — drop audio files or click Browse below.</p>
            )}
          </section>

          {/* ── Playlist ────────────────────────────────────── */}
          <section className="panel panel--tight">
            <div className="panel-head">
              <span className="panel-label">PLAYLIST</span>
              <button className="btn btn--sm" onClick={browseFiles}>Browse</button>
            </div>

            {tracks.length === 0 ? (
              <div className={`player-dropzone${dragOver ? ' player-dropzone--over' : ''}`}
                onDragOver={e => { e.preventDefault(); setDragOver(true) }}
                onDragLeave={() => setDragOver(false)}
                onDrop={handleDrop}
                onClick={browseFiles}>
                <p className="player-drop-text">Drop audio files here to listen</p>
                <p className="player-drop-sub">WAV · MP3 · FLAC · Opus · M4A · OGG and more</p>
              </div>
            ) : (
              <div className="player-playlist">
                {tracks.map((t, i) => (
                  <div key={t.path}
                    className={`playlist-row${activeTrack?.path === t.path ? ' playlist-row--active' : ''}`}
                    onClick={() => setActiveTrack(t)}>
                    <span className="playlist-dot" style={{background: t.color}} />
                    <span className="playlist-num">{i + 1}</span>
                    <div className="playlist-info">
                      <span className="playlist-name">{t.name}</span>
                      <input className="playlist-label" value={t.label} placeholder="Speaker name"
                        onClick={e => e.stopPropagation()}
                        onChange={e => setTracks(prev => prev.map((tr, j) => j === i ? {...tr, label: e.target.value} : tr))} />
                    </div>
                    {activeTrack?.path === t.path && playing && (
                      <span className="playlist-playing">▶</span>
                    )}
                    <button className="playlist-remove" onClick={e => { e.stopPropagation(); removeTrack(t.path) }}>
                      <svg width="9" height="9" viewBox="0 0 9 9"><path d="M1 1l7 7M8 1L1 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/></svg>
                    </button>
                  </div>
                ))}
                <button className="btn btn--sm player-add-btn" onClick={browseFiles}>+ Add files</button>
              </div>
            )}
          </section>

        </div>
      </div>
    </>
  )
}
