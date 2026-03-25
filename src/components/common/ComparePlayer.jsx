import { useState, useRef } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { fmtTime } from '../../utils'
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
  const inactiveRef = activeSource === 'original' ? processedRef : originalRef

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

    setActiveSource(source)

    // Sync position and resume
    setTimeout(() => {
      const newRef = source === 'original' ? originalRef : processedRef
      if (newRef.current) {
        newRef.current.currentTime = time
        if (wasPlaying) {
          newRef.current.play().then(() => setPlaying(true)).catch(() => {})
        }
      }
    }, 50)
  }

  const seek = (time) => {
    if (originalRef.current) originalRef.current.currentTime = time
    if (processedRef.current) processedRef.current.currentTime = time
    setCurrentTime(time)
  }

  return (
    <div className="compare-player">
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
      <div className="compare-toggle">
        <button className={`compare-btn${activeSource === 'original' ? ' compare-btn--active' : ''}`}
          onClick={() => switchSource('original')}>
          {originalLabel}
        </button>
        <button className={`compare-btn${activeSource === 'processed' ? ' compare-btn--active compare-btn--processed' : ''}`}
          onClick={() => switchSource('processed')}>
          {processedLabel}
        </button>
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
      <div className="compare-controls">
        <span className="compare-time">{fmtTime(currentTime)}</span>
        <button className="player-btn player-btn--play" onClick={toggle} style={{width:36, height:36}}>
          {playing
            ? <svg width="14" height="14" viewBox="0 0 16 16"><rect x="3" y="2" width="4" height="12" rx="1" fill="currentColor"/><rect x="9" y="2" width="4" height="12" rx="1" fill="currentColor"/></svg>
            : <svg width="14" height="14" viewBox="0 0 16 16"><path d="M4 2.5l10 5.5-10 5.5V2.5z" fill="currentColor"/></svg>}
        </button>
        <span className="compare-time">{fmtTime(duration)}</span>
      </div>

      <p className="compare-hint">
        {activeSource === 'original' ? 'Listening to the original' : 'Listening to the processed version'}
        {' — '}click the other button to compare
      </p>
    </div>
  )
}
