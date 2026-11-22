import { useState, useRef, useEffect, useId, useMemo } from 'react'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Play, Pause, SkipBack, SkipForward, Bookmark, X, Plus, Repeat, Copy, Check } from 'lucide-react'
import { cn, shouldIgnoreNavigationShortcut } from '../../lib/utils'
import { fmtTime, sortRecordingChunks } from '../../utils'
import { useSpeakerColors, speakerColorAt, SPEAKER_COUNT } from '../../lib/speakerColors'
import {
  AUDIO_EXTS,
  SPEED_STEPS,
  loadSpeed,
  cycleSpeedStep,
  loadBookmarks,
  partitionPlaybackPaths,
  bookmarksToText,
  BACKSTEP_RULE,
  BACKSTEP_STEPS,
  loadBackstep,
  backstepLabel,
  resumePosition,
} from '../../lib/player'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Segmented } from '../ui/segmented'
import Waveform from '../common/Waveform'
import { WaveformIcon } from '../common/Icons'
import WorkspaceHeader from '../common/WorkspaceHeader'
import Transcript from './Transcript'
import ListeningMix from './ListeningMix'
import {
  CHANNEL_PROBE_URL,
  createChannelMixer,
  initialStrips,
  mixChannelCount,
  stripChanged,
  loadVolume,
  probedChannelCount,
} from '../../lib/channelMixer'

const NO_STRIPS = []

// ── Global Audio Player ─────────────────────────────────────────────────────
//
// Play any audio file directly — no conversion needed. Playlist tracks get
// colors from the existing --speaker-N design tokens without inferring who is speaking.
//
// Layout: a Now Playing surface (large seekable waveform) + a playlist/
// bookmarks rail scroll above a PERSISTENT transport bar (controls + a thin
// progress scrubber) that never scrolls away, so play/pause and position
// stay reachable no matter how far you scroll.
//
// Keyboard transport (when not typing in a field):
//   Space / K  play-pause      ← / →  seek ±5s       J / L  seek ±10s
//   ↑ / ↓      speed up/down    [ / ]  prev/next      B      bookmark
//
// Auto-backstep: resuming after a pause first rewinds the chosen number of
// seconds (shown as "Backstep" in the transport). Seeks, track changes and
// A-B loop wraps clear the pause point, so they never rewind.
//
// Listening mix: a master volume (saved like speed) and, for a file with 2 or
// more channels, an opt-in "Separate channels" strip per channel. Turning it
// on swaps in a new <audio> element with crossOrigin set and routes it
// through Web Audio (lib/channelMixer.js); turning it off swaps back to plain
// playback. The position and play state carry over. Strips reset for each
// track and are not saved. Nothing here changes any file.

const PLAYER_STORAGE_WARNING =
  'Player bookmarks, speed, volume and backstep cannot be saved in this session. Export important bookmark notes before closing DepoAudio.'

function loadPlayerStorageState() {
  try {
    return {
      speed: loadSpeed(window.localStorage.getItem('player-speed')),
      backstep: loadBackstep(window.localStorage.getItem('player-backstep')),
      volume: loadVolume(window.localStorage.getItem('player-volume')),
      bookmarks: loadBookmarks(window.localStorage.getItem('player-bookmarks')),
      error: '',
    }
  } catch {
    return {
      speed: 1,
      backstep: loadBackstep(null),
      volume: loadVolume(null),
      bookmarks: [],
      error: PLAYER_STORAGE_WARNING,
    }
  }
}

export default function PlayerTab({ dropHandlerRef, onConvertFiles, openRequest, onOpenRequestHandled, playbackKeys }) {
  const [tracks, setTracks] = useState([]) // { path, name, colorIndex, label }
  const [activeTrack, setActiveTrack] = useState(null)
  const [playing, setPlaying] = useState(false)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [dragOver, setDragOver] = useState(false)
  const [initialStorage] = useState(loadPlayerStorageState)
  const [speed, setSpeedState] = useState(initialStorage.speed)
  const [backstep, setBackstepState] = useState(initialStorage.backstep)
  const [volume, setVolumeState] = useState(initialStorage.volume)
  // Listening mix. audioKey changes swap in a fresh <audio> element: a
  // MediaElementSource can be made once per element, and only an element
  // without crossOrigin plays exactly as before.
  const [mixOn, setMixOn] = useState(false)
  const [audioKey, setAudioKey] = useState(0)
  const [probe, setProbe] = useState({ path: null, channels: null })
  const [heard, setHeard] = useState({ path: null, count: null })
  const [mix, setMix] = useState({ path: null, strips: [] })
  const [probeAvailable, setProbeAvailable] = useState(true)
  const [mixNotice, setMixNotice] = useState('')
  const mixerRef = useRef(null)
  const restoreRef = useRef(null) // { time, play } to apply after a swap
  const backstepRuleId = useId()
  // Position where the person last paused this track. Any seek (including our
  // own backstep and the A-B loop wrap) clears it, so only a plain resume rewinds.
  const pausedAtRef = useRef(null)
  const [loopA, setLoopA] = useState(null)
  const [loopB, setLoopB] = useState(null)
  const [copied, setCopied] = useState(false)
  const [convertFirstPaths, setConvertFirstPaths] = useState([])
  const [playbackError, setPlaybackError] = useState('')
  const [bookmarks, setBookmarks] = useState(initialStorage.bookmarks)
  const [storageError, setStorageError] = useState(initialStorage.error)
  const [transcriptStorageError, setTranscriptStorageError] = useState('')
  useEffect(() => {
    let nextError = ''
    try {
      window.localStorage.setItem('player-bookmarks', JSON.stringify(bookmarks))
    } catch {
      nextError = PLAYER_STORAGE_WARNING
    }
    const statusTimer = window.setTimeout(() => setStorageError(nextError), 0)
    return () => window.clearTimeout(statusTimer)
  }, [bookmarks])
  const [dragIdx, setDragIdx] = useState(null) // playlist drag-reorder
  const audioRef = useRef(null)
  const autoAdvanceRef = useRef(false) // play next track once it loads
  const tracksRef = useRef(tracks)
  const activeTrackRef = useRef(activeTrack)
  useEffect(() => {
    tracksRef.current = tracks
  }, [tracks])
  useEffect(() => {
    activeTrackRef.current = activeTrack
  }, [activeTrack])

  // Theme-aware concrete speaker colors, resolved from the design tokens.
  const speakerColors = useSpeakerColors()
  const colorOf = track => speakerColorAt(speakerColors, track?.colorIndex ?? 0)

  const browseFiles = async () => {
    try {
      const selected = await open({ multiple: true, filters: [{ name: 'Audio', extensions: AUDIO_EXTS }] })
      if (selected) addFiles(Array.isArray(selected) ? selected : [selected])
    } catch (e) {
      setPlaybackError(`Couldn't open the audio picker: ${e}`)
    }
  }

  // Add files. Native drops arrive unfiltered, so skip already-queued and
  // non-audio paths. Court-recorder chunks order chronologically (incoming batch
  // only — existing tracks keep any manual drag-reorder).
  const addFiles = paths => {
    const { playable, conversionRequired } = partitionPlaybackPaths(paths, tracksRef.current)
    if (conversionRequired.length > 0) {
      setConvertFirstPaths(prev => [...new Set([...prev, ...conversionRequired])])
      setPlaybackError(
        'Court-recorder .sgmca/.trm/.ftr files require conversion before playback. Send them to Convert and create a standard audio file first.',
      )
    }

    const newTracks = sortRecordingChunks(playable).map((path, i) => ({
      path,
      name: path.split('/').pop().split('\\').pop(),
      colorIndex: (tracksRef.current.length + i) % SPEAKER_COUNT,
      label: `Track ${tracksRef.current.length + i + 1}`,
    }))
    if (!newTracks.length) return
    const merged = [...tracksRef.current, ...newTracks]
    tracksRef.current = merged
    setTracks(merged)
    if (!activeTrackRef.current) {
      activeTrackRef.current = newTracks[0]
      setActiveTrack(newTracks[0])
    }
  }

  const handleDrop = e => {
    e.preventDefault()
    setDragOver(false)
  }

  // Claim native drops for the playlist while mounted. No dep array: re-register
  // every render so addFiles sees fresh state.
  useEffect(() => {
    if (!dropHandlerRef) return undefined
    dropHandlerRef.current = addFiles
    return () => {
      dropHandlerRef.current = null
    }
  })

  const toggle = () => {
    const a = audioRef.current
    if (!a || !activeTrack) return
    setPlaybackError('')
    if (!a.paused) {
      a.pause()
      return
    }
    const loopStart = loopA != null && loopB != null && loopB > loopA ? loopA : null
    const start = resumePosition({ currentTime: a.currentTime, pausedAt: pausedAtRef.current, backstep, loopStart })
    pausedAtRef.current = null
    if (start !== a.currentTime) a.currentTime = start
    mixerRef.current?.resume()
    a.play().catch(e => setPlaybackError(`Couldn't play ${activeTrack.name}: ${e}`))
  }

  const applyVolume = v => {
    const next = loadVolume(v)
    setVolumeState(next)
    try {
      window.localStorage.setItem('player-volume', String(next))
    } catch {
      setStorageError(PLAYER_STORAGE_WARNING)
    }
  }

  // Swap the <audio> element, keeping position and play state.
  const swapAudio = nextMixOn => {
    const a = audioRef.current
    restoreRef.current = a ? { time: a.currentTime || 0, play: !a.paused } : null
    a?.pause()
    setMixOn(nextMixOn)
    setAudioKey(k => k + 1)
  }
  const toggleMix = () => {
    setMixNotice('')
    swapAudio(!mixOn)
  }
  // Kept until the person tries again: pressing Play must not hide why the
  // channel strips went away.
  const mixFailed = reason => {
    setMixNotice(
      `Separate channels could not start (${reason}), so the Player went back to normal playback. Nothing was changed in the file.`,
    )
    swapAudio(false)
  }

  const applyBackstep = s => {
    setBackstepState(s)
    try {
      window.localStorage.setItem('player-backstep', String(s))
    } catch {
      setStorageError(PLAYER_STORAGE_WARNING)
    }
  }

  const seekBy = delta => {
    const a = audioRef.current
    if (!a) return
    a.currentTime = Math.max(0, Math.min((a.currentTime || 0) + delta, a.duration || 0))
  }
  const seekTo = t => {
    if (audioRef.current) audioRef.current.currentTime = t
  }

  const applySpeed = s => {
    setSpeedState(s)
    try {
      window.localStorage.setItem('player-speed', String(s))
    } catch {
      setStorageError(PLAYER_STORAGE_WARNING)
    }
    if (audioRef.current) audioRef.current.playbackRate = s
  }
  const cycleSpeed = dir => applySpeed(cycleSpeedStep(speed, dir))

  const addBookmark = () => {
    if (!activeTrack) return
    const t = audioRef.current?.currentTime ?? currentTime
    setBookmarks(prev => [...prev, { time: t, label: fmtTime(t), trackPath: activeTrack.path }])
  }

  const skip = dir => {
    if (!activeTrack || tracks.length === 0) return
    const idx = tracks.findIndex(t => t.path === activeTrack.path)
    const next = (idx + dir + tracks.length) % tracks.length
    const target = tracks[next]
    audioRef.current?.pause()
    activeTrackRef.current = target
    setActiveTrack(target)
    setPlaying(false)
    setCurrentTime(0)
    setDuration(0)
  }

  const selectTrack = track => {
    if (track.path === activeTrack?.path) return
    autoAdvanceRef.current = false
    audioRef.current?.pause()
    setPlaying(false)
    setCurrentTime(0)
    setDuration(0)
    setPlaybackError('')
    activeTrackRef.current = track
    setActiveTrack(track)
  }

  // Converted files sent from Convert ("Open in Player"): add any that are
  // not in the playlist yet and select the first one.
  useEffect(() => {
    if (!openRequest?.paths?.length) return
    addFiles(openRequest.paths)
    const target = tracksRef.current.find(track => track.path === openRequest.paths[0])
    if (target) selectTrack(target)
    onOpenRequestHandled?.(openRequest)
    // eslint-disable-next-line react-hooks/exhaustive-deps -- run once per request
  }, [openRequest])

  const removeTrack = path => {
    const idx = tracks.findIndex(t => t.path === path)
    const next = tracks.filter(t => t.path !== path)
    tracksRef.current = next
    setTracks(next)
    if (activeTrack?.path === path) {
      const target = next[idx] ?? next[idx - 1] ?? null
      activeTrackRef.current = target
      setActiveTrack(target)
      setPlaying(false)
      setCurrentTime(0)
      setDuration(0)
    }
  }

  const copyBookmarks = async () => {
    if (!activeTrack) return
    try {
      await navigator.clipboard.writeText(bookmarksToText(bookmarks, activeTrack.path))
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch (error) {
      setPlaybackError(`Could not copy bookmarks: ${String(error)}`)
    }
  }

  // Keyboard transport. Registered once; reads latest actions/state via a ref.
  const actionsRef = useRef({})
  useEffect(() => {
    actionsRef.current = { hasTrack: !!activeTrack, toggle, seekBy, skip, cycleSpeed, addBookmark, backstep }
  })

  // Playback keys while typing elsewhere (Settings): the same actions as the
  // transport and the in-app keys, read through actionsRef so they always
  // act on the current track. Present only while the Player is mounted.
  const setPlaybackKeysTarget = playbackKeys?.setTarget
  useEffect(() => {
    if (!setPlaybackKeysTarget) return undefined
    const isPlaying = () => !!audioRef.current && !audioRef.current.paused
    setPlaybackKeysTarget({
      get hasTrack() {
        return actionsRef.current.hasTrack
      },
      get backstep() {
        return actionsRef.current.backstep
      },
      isPlaying,
      toggle: () => actionsRef.current.toggle(),
      play: () => {
        if (!isPlaying()) actionsRef.current.toggle()
      },
      pause: () => {
        if (isPlaying()) actionsRef.current.toggle()
      },
      seekBy: delta => actionsRef.current.seekBy(delta),
      cycleSpeed: dir => actionsRef.current.cycleSpeed(dir),
    })
    return () => setPlaybackKeysTarget(null)
  }, [setPlaybackKeysTarget])
  useEffect(() => {
    const onKey = e => {
      if (shouldIgnoreNavigationShortcut(e)) return
      const a = actionsRef.current
      if (!a.hasTrack) return
      switch (e.key) {
        case ' ':
        case 'k':
        case 'K':
          e.preventDefault()
          a.toggle()
          break
        case 'ArrowLeft':
          e.preventDefault()
          a.seekBy(-5)
          break
        case 'ArrowRight':
          e.preventDefault()
          a.seekBy(5)
          break
        case 'j':
        case 'J':
          e.preventDefault()
          a.seekBy(-10)
          break
        case 'l':
        case 'L':
          e.preventDefault()
          a.seekBy(10)
          break
        case 'ArrowUp':
          e.preventDefault()
          a.cycleSpeed(1)
          break
        case 'ArrowDown':
          e.preventDefault()
          a.cycleSpeed(-1)
          break
        case '[':
          e.preventDefault()
          a.skip(-1)
          break
        case ']':
          e.preventDefault()
          a.skip(1)
          break
        case 'b':
        case 'B':
          e.preventDefault()
          a.addBookmark()
          break
        default:
          break
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Reload on track change; keep playing if we got here by auto-advance. Reset
  // the A-B loop since its points belong to the previous track.
  useEffect(() => {
    if (activeTrack && audioRef.current) {
      pausedAtRef.current = null
      mixerRef.current?.resetHeard()
      setPlaying(false)
      setPlaybackError('')
      audioRef.current.load()
      audioRef.current.playbackRate = speed
      setCurrentTime(0)
      setDuration(0)
      setLoopA(null)
      setLoopB(null)
      if (autoAdvanceRef.current) {
        autoAdvanceRef.current = false
        audioRef.current.play().catch(e => setPlaybackError(`Couldn't play ${activeTrack.name}: ${e}`))
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTrack?.path])

  // Channel count of the active file, from the same probe Convert uses.
  const activePath = activeTrack?.path ?? null
  useEffect(() => {
    if (!activePath) return undefined
    let current = true
    Promise.resolve()
      .then(() => invoke('probe_audio_streams_cmd', { path: activePath }))
      .then(info => {
        if (current) setProbe({ path: activePath, channels: probedChannelCount(info) })
      })
      .catch(() => {
        if (current) setProbe({ path: activePath, channels: null })
      })
    return () => {
      current = false
    }
  }, [activePath])
  const fileChannels = probe.path === activePath ? probe.channels : null
  const heardChannels = heard.path === activePath ? heard.count : null
  // One strip per channel the mix really splits. When the web view turns out
  // to mix the file down, strips set for the file's channels no longer match
  // anything the person can see, so they start over (and the Player says so)
  // instead of a hidden Solo or Mute silencing the mix.
  const mixLayout = mixChannelCount({ fileChannels, heardChannels })
  const ownStrips = mix.path === activePath ? mix.strips : NO_STRIPS
  const strips = useMemo(
    () => (ownStrips.length === mixLayout ? ownStrips : initialStrips(mixLayout)),
    [ownStrips, mixLayout],
  )
  const stripsCleared = ownStrips.length > mixLayout && ownStrips.some(stripChanged)
  const setStrips = next => setMix({ path: activePath, strips: next })

  const latestRef = useRef({})
  useEffect(() => {
    latestRef.current = { volume, strips, mixLayout, activePath, mixFailed }
  })

  // Build the Web Audio graph for the current element while the mix is on;
  // close it when the mix goes off, the element is swapped or the tab unmounts.
  useEffect(() => {
    const audio = audioRef.current
    if (!mixOn || !audio) return undefined
    const latest = latestRef.current
    let mixer
    try {
      mixer = createChannelMixer({
        audio,
        channels: latest.mixLayout || 2,
        probeUrl: CHANNEL_PROBE_URL,
        onHeardChannels: count => setHeard({ path: latestRef.current.activePath, count }),
        onProbeUnavailable: () => setProbeAvailable(false),
      })
    } catch (error) {
      const timer = window.setTimeout(() => latest.mixFailed(String(error?.message || error)), 0)
      return () => window.clearTimeout(timer)
    }
    mixerRef.current = mixer
    mixer.setMaster(latest.volume)
    mixer.setStrips(latest.strips)
    mixer.setPlaying(!audio.paused)
    mixer.resume()
    return () => {
      mixer.dispose()
      if (mixerRef.current === mixer) mixerRef.current = null
    }
  }, [mixOn, audioKey])

  useEffect(() => {
    const mixer = mixerRef.current
    if (!mixer) return
    if (mixLayout) mixer.setLayout(mixLayout)
    mixer.setStrips(strips)
  }, [strips, mixLayout, mixOn, audioKey])

  // With the mix on, the element plays at full level into Web Audio and the
  // master gain sets the volume; otherwise the element's own volume does.
  useEffect(() => {
    const a = audioRef.current
    if (a) a.volume = mixOn ? 1 : volume
    mixerRef.current?.setMaster(volume)
  }, [volume, mixOn, audioKey, activePath])

  const audioSrc = activeTrack ? convertFileSrc(activeTrack.path) : ''
  const trackBookmarks = activeTrack ? bookmarks.filter(b => b.trackPath === activeTrack.path) : []
  const activeColor = colorOf(activeTrack)
  const loopActive = loopA != null && loopB != null && loopB > loopA

  const speedOptions = SPEED_STEPS.map(s => ({ value: s, label: `${s}×`, title: `${s}× playback speed` }))

  const convertFirstNotice = (convertFirstPaths.length > 0 || playbackError) && (
    <div
      role="alert"
      className="mb-3 flex items-center gap-3 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 type-small text-foreground"
    >
      <span className="flex-1">{playbackError}</span>
      {convertFirstPaths.length > 0 && onConvertFiles && (
        <Button
          size="sm"
          variant="outline"
          onClick={async () => {
            const queued = await onConvertFiles(convertFirstPaths)
            if (queued) {
              setConvertFirstPaths([])
              setPlaybackError('')
            }
          }}
        >
          Send to Convert
        </Button>
      )}
      <button
        type="button"
        aria-label="Dismiss playback message"
        onClick={() => {
          setConvertFirstPaths([])
          setPlaybackError('')
        }}
      >
        <X size={12} />
      </button>
    </div>
  )

  const storageNotice = storageError && (
    <div
      role="alert"
      className="mb-3 flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 type-small text-foreground"
    >
      <span className="flex-1">{storageError}</span>
      <button type="button" aria-label="Dismiss player storage warning" onClick={() => setStorageError('')}>
        <X size={12} />
      </button>
    </div>
  )

  const transcriptStorageNotice = transcriptStorageError && (
    <div
      role="alert"
      className="mb-3 flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 type-small text-foreground"
    >
      <span className="flex-1">{transcriptStorageError}</span>
      <button
        type="button"
        aria-label="Dismiss transcript storage warning"
        onClick={() => setTranscriptStorageError('')}
      >
        <X size={12} />
      </button>
    </div>
  )

  // ── Empty state ───────────────────────────────────────────────────────────
  if (tracks.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-4 sm:px-6 lg:px-8 py-5">
          {storageNotice}
          {transcriptStorageNotice}
          {convertFirstNotice}
          <WorkspaceHeader
            eyebrow="Local review"
            title="Review a recording"
            description="Listen across a multi-file session, mark useful moments, and keep notes beside the audio."
            status="Playback stays on this computer"
            className="mb-4"
          />
          {playbackKeys?.settings.enabled && (
            <p role="status" className="mb-3 type-small text-[hsl(var(--sub))]">
              Playback keys are on, but they do nothing until a recording is loaded here.
            </p>
          )}
          <div
            className={cn(
              'grid items-center gap-3 rounded-xl border border-dashed px-4 py-5 transition-colors sm:grid-cols-[auto_minmax(0,1fr)_auto]',
              dragOver
                ? 'border-foreground bg-secondary'
                : 'border-border bg-[hsl(var(--surface))] hover:border-[hsl(var(--text2)/0.55)]',
            )}
            onDragOver={e => {
              e.preventDefault()
              setDragOver(true)
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={handleDrop}
          >
            <div
              aria-hidden="true"
              className="grid h-11 w-11 place-items-center rounded-lg border border-border/70 bg-card"
            >
              <WaveformIcon />
            </div>
            <div className="min-w-0">
              <p className="text-[13px] font-semibold text-foreground">Add audio to the review playlist</p>
              <p className="mt-0.5 type-small leading-relaxed text-[hsl(var(--sub))]">
                Drop WAV, MP3, FLAC, Opus, M4A, or OGG files here. Multi-file sessions play back in order.
              </p>
            </div>
            <Button
              type="button"
              variant="outline"
              className="w-full sm:w-auto"
              aria-label="Add audio files to the playlist: browse for recordings"
              onClick={browseFiles}
            >
              Browse audio
            </Button>
          </div>
        </div>
      </div>
    )
  }

  // ── Workspace (Now Playing + playlist rail) over a fixed transport bar ────
  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-4 sm:px-6 lg:px-8 py-5 grid gap-4 md:grid-cols-[minmax(0,1fr)_300px]">
          <WorkspaceHeader
            eyebrow="Local review"
            title="Review a recording"
            description="Use the waveform, playlist, bookmarks, and synced transcript to work through the record."
            status={`${tracks.length} track${tracks.length === 1 ? '' : 's'} loaded locally`}
            className="md:col-span-2"
          />
          {storageNotice && <div className="md:col-span-2">{storageNotice}</div>}
          {transcriptStorageNotice && <div className="md:col-span-2">{transcriptStorageNotice}</div>}
          {convertFirstNotice && <div className="md:col-span-2">{convertFirstNotice}</div>}

          {/* Now Playing — the main surface: a large seekable waveform */}
          {activeTrack && (
            <div className="md:order-1 order-2 min-w-0">
              <Card>
                <CardHeader>
                  <CardTitle>Now Playing</CardTitle>
                </CardHeader>
                <CardContent className="p-4 flex flex-col gap-3">
                  <div className="flex items-baseline gap-2 min-w-0">
                    <span className="w-2 h-2 rounded-full shrink-0 self-center" style={{ background: activeColor }} />
                    <span className="text-[15px] font-semibold text-foreground truncate">{activeTrack.name}</span>
                    <span className="text-[12px] tabular-nums text-[hsl(var(--sub))] shrink-0">
                      · {activeTrack.label}
                    </span>
                  </div>
                  <Waveform
                    audioPath={activeTrack.path}
                    color={activeColor}
                    currentTime={currentTime}
                    duration={duration}
                    height={96}
                    onSeek={seekTo}
                    markers={trackBookmarks}
                    loop={loopActive ? { a: loopA, b: loopB } : null}
                  />
                  <div className="flex items-center justify-between tabular-nums text-[12px] text-[hsl(var(--sub))]">
                    <span>{fmtTime(currentTime)}</span>
                    <span>{fmtTime(duration)}</span>
                  </div>
                  <ListeningMix
                    volume={volume}
                    onVolume={applyVolume}
                    fileChannels={fileChannels}
                    mixOn={mixOn}
                    onToggleMix={toggleMix}
                    strips={strips}
                    onStrips={setStrips}
                    heardChannels={heardChannels}
                    probeAvailable={probeAvailable}
                    probed={probe.path === activePath}
                    stripsCleared={stripsCleared}
                    notice={mixNotice}
                  />
                </CardContent>
              </Card>
            </div>
          )}

          {/* Rail: playlist + bookmarks */}
          <aside className="md:order-2 order-1 flex flex-col gap-3.5 min-w-0">
            <Card>
              <CardHeader>
                <CardTitle>Playlist</CardTitle>
                <Button size="sm" onClick={browseFiles}>
                  <Plus size={12} /> Add
                </Button>
              </CardHeader>
              <CardContent className="p-1.5">
                {tracks.map((t, i) => (
                  <div
                    key={t.path}
                    className={cn(
                      'group flex items-center gap-2 px-2 py-1.5 rounded-md transition-colors hover:bg-secondary/50',
                      activeTrack?.path === t.path && 'bg-secondary',
                      dragIdx === i && 'opacity-50',
                    )}
                    draggable
                    onDragStart={() => setDragIdx(i)}
                    onDragOver={e => {
                      e.preventDefault()
                    }}
                    onDrop={e => {
                      e.preventDefault()
                      if (dragIdx !== null && dragIdx !== i) {
                        setTracks(prev => {
                          const next = [...prev]
                          const [moved] = next.splice(dragIdx, 1)
                          next.splice(i, 0, moved)
                          tracksRef.current = next
                          return next
                        })
                      }
                      setDragIdx(null)
                    }}
                    onDragEnd={() => setDragIdx(null)}
                  >
                    <div className="flex flex-col flex-1 min-w-0">
                      <button
                        type="button"
                        className="flex items-center gap-2 min-w-0 text-left rounded"
                        aria-current={activeTrack?.path === t.path ? 'true' : undefined}
                        onClick={() => selectTrack(t)}
                      >
                        <span className="w-2 h-2 rounded-full shrink-0" style={{ background: colorOf(t) }} />
                        <span className="type-small text-foreground truncate" title={t.name}>
                          {t.name}
                        </span>
                      </button>
                      <input
                        className="text-[12px] text-[hsl(var(--text2))] bg-transparent border-none p-0 tabular-nums w-full focus:text-foreground"
                        value={t.label}
                        aria-label={`Track label for ${t.name}`}
                        placeholder="Track or participant name"
                        onClick={e => e.stopPropagation()}
                        onChange={e => {
                          const label = e.target.value
                          setTracks(prev => prev.map((tr, j) => (j === i ? { ...tr, label } : tr)))
                          if (activeTrack?.path === t.path) setActiveTrack(prev => ({ ...prev, label }))
                        }}
                      />
                    </div>
                    {activeTrack?.path === t.path && playing ? (
                      <Pause size={10} fill="currentColor" className="text-foreground shrink-0" />
                    ) : (
                      <span className="tabular-nums text-[12px] text-[hsl(var(--sub))] shrink-0 group-hover:hidden">
                        {i + 1}
                      </span>
                    )}
                    <button
                      className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity hover:text-destructive shrink-0"
                      aria-label={`Remove ${t.name}`}
                      onClick={e => {
                        e.stopPropagation()
                        removeTrack(t.path)
                      }}
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
                    <>
                      <button
                        onClick={copyBookmarks}
                        title="Copy bookmarks to clipboard as timestamped lines"
                        className="flex items-center gap-1 type-small text-[hsl(var(--sub))] hover:text-foreground transition-colors rounded px-1"
                      >
                        {copied ? (
                          <>
                            <Check size={11} /> Copied
                          </>
                        ) : (
                          <>
                            <Copy size={11} /> Copy
                          </>
                        )}
                      </button>
                      <span role="status" aria-live="polite" className="sr-only">
                        {copied ? 'Bookmarks copied to clipboard.' : ''}
                      </span>
                    </>
                  )}
                </CardHeader>
                <CardContent className="p-2">
                  {trackBookmarks.length === 0 ? (
                    <p className="px-1 py-1 type-small text-[hsl(var(--sub))]">
                      Press <kbd className="tabular-nums">B</kbd> or the bookmark button to mark the current spot.
                    </p>
                  ) : (
                    <div className="flex flex-col gap-0.5">
                      {trackBookmarks
                        .slice()
                        .sort((a, b) => a.time - b.time)
                        .map(b => (
                          <div key={`${b.time}-${b.trackPath}`} className="flex items-center gap-2 group px-1">
                            <button
                              className="tabular-nums text-[12px] text-[hsl(var(--text2))] hover:text-foreground shrink-0 w-11 text-left rounded"
                              title="Jump to bookmark"
                              onClick={() => seekTo(b.time)}
                            >
                              {fmtTime(b.time)}
                            </button>
                            <input
                              className="flex-1 min-w-0 bg-transparent border-none p-0 type-small text-foreground"
                              value={b.label}
                              placeholder="Add a note…"
                              onChange={e =>
                                setBookmarks(prev => prev.map(x => (x === b ? { ...x, label: e.target.value } : x)))
                              }
                            />
                            <button
                              className="text-[hsl(var(--sub))] opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 hover:text-destructive transition-all shrink-0 rounded"
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

          {activeTrack && (
            <div className="md:order-3 order-3 md:col-span-2 min-w-0">
              <Transcript
                key={activeTrack.path}
                trackPath={activeTrack.path}
                currentTime={currentTime}
                playing={playing}
                onSeek={seekTo}
                onStorageError={setTranscriptStorageError}
              />
            </div>
          )}
        </div>
      </div>

      {/* ── Persistent transport bar ─────────────────────────────────────── */}
      {activeTrack && (
        <div className="shrink-0 border-t border-border bg-card px-4 md:px-6 py-2.5">
          <audio
            key={audioKey}
            ref={audioRef}
            src={audioSrc}
            // Web Audio outputs silence for a cross-origin element without
            // CORS; Tauri's asset protocol sends Access-Control-Allow-Origin.
            crossOrigin={mixOn ? 'anonymous' : undefined}
            preload="metadata"
            onLoadedMetadata={e => {
              setDuration(e.target.duration)
              e.target.playbackRate = speed
              e.target.volume = mixOn ? 1 : volume
              const restore = restoreRef.current
              restoreRef.current = null
              if (restore) {
                if (restore.time > 0) e.target.currentTime = Math.min(restore.time, e.target.duration || restore.time)
                if (restore.play) {
                  mixerRef.current?.resume()
                  e.target.play().catch(err => setPlaybackError(`Couldn't play ${activeTrack.name}: ${err}`))
                }
              }
            }}
            onPlay={() => {
              setPlaying(true)
              mixerRef.current?.setPlaying(true)
            }}
            onPause={e => {
              setPlaying(false)
              mixerRef.current?.setPlaying(false)
              // A pause at the end of the track is not a resume point
              pausedAtRef.current = e.target.ended ? null : e.target.currentTime
            }}
            onSeeking={() => {
              pausedAtRef.current = null
            }}
            onError={() => {
              setPlaying(false)
              if (mixOn) {
                mixFailed('the web view would not open this file for Web Audio')
                return
              }
              setPlaybackError(
                `Couldn't decode ${activeTrack?.name || 'this file'}. Convert the source to a standard audio format and try again.`,
              )
            }}
            onTimeUpdate={e => {
              const t = e.target.currentTime
              setCurrentTime(t)
              if (loopActive && t >= loopB) e.target.currentTime = loopA
            }}
            onEnded={() => {
              setPlaying(false)
              mixerRef.current?.setPlaying(false)
              const idx = tracks.findIndex(t => t.path === activeTrack.path)
              if (idx >= 0 && idx < tracks.length - 1) {
                autoAdvanceRef.current = true
                skip(1)
              }
            }}
          />

          <div className="player-transport w-full max-w-[1100px] mx-auto flex items-center gap-3 md:gap-4">
            {/* Transport buttons */}
            <div className="flex items-center gap-1.5 shrink-0">
              <button
                className="w-8 h-8 flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground"
                onClick={() => skip(-1)}
                title="Previous track ( [ )"
                aria-label="Previous track"
              >
                <SkipBack size={15} fill="currentColor" />
              </button>
              <button
                className="w-10 h-10 bg-primary text-primary-foreground flex items-center justify-center transition-colors hover:bg-gold-hi"
                onClick={toggle}
                title={playing ? 'Pause (Space)' : 'Play (Space)'}
                aria-label={playing ? 'Pause' : 'Play'}
              >
                {playing ? (
                  <Pause size={17} fill="currentColor" />
                ) : (
                  <Play size={17} fill="currentColor" className="ml-0.5" />
                )}
              </button>
              <button
                className="w-8 h-8 flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground"
                onClick={() => skip(1)}
                title="Next track ( ] )"
                aria-label="Next track"
              >
                <SkipForward size={15} fill="currentColor" />
              </button>
            </div>

            {/* Track identity + scrubber (grows) */}
            <div className="player-transport-track flex-1 min-w-0 flex flex-col gap-1">
              <div className="flex items-center gap-2 min-w-0">
                <span className="w-2 h-2 rounded-full shrink-0" style={{ background: activeColor }} />
                <span className="text-[12px] font-medium text-foreground truncate">{activeTrack.name}</span>
                <span className="text-[12px] tabular-nums text-[hsl(var(--sub))] shrink-0 hidden sm:inline">
                  · {activeTrack.label}
                </span>
                <span className="ml-auto tabular-nums text-[12px] text-[hsl(var(--sub))] shrink-0">
                  {fmtTime(currentTime)} / {fmtTime(duration)}
                </span>
              </div>
              {/* Thin progress scrubber — the detailed waveform lives in Now
                  Playing above, so the pinned bar stays cheap (no second
                  decode) while still showing the loop band + position. */}
              <div
                className="relative h-1.5 bg-border cursor-pointer overflow-hidden"
                role="slider"
                aria-label="Seek"
                aria-valuemin={0}
                aria-valuemax={Math.round(duration)}
                aria-valuenow={Math.round(currentTime)}
                tabIndex={0}
                onKeyDown={e => {
                  if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') {
                    e.preventDefault()
                    seekBy(-5)
                  }
                  if (e.key === 'ArrowRight' || e.key === 'ArrowUp') {
                    e.preventDefault()
                    seekBy(5)
                  }
                  if (e.key === 'Home') {
                    e.preventDefault()
                    seekTo(0)
                  }
                  if (e.key === 'End') {
                    e.preventDefault()
                    seekTo(duration)
                  }
                }}
                onClick={e => {
                  const r = e.currentTarget.getBoundingClientRect()
                  if (duration) seekTo(((e.clientX - r.left) / r.width) * duration)
                }}
              >
                {loopActive && (
                  <div
                    className="absolute inset-y-0 bg-foreground/20"
                    style={{ left: `${(loopA / duration) * 100}%`, width: `${((loopB - loopA) / duration) * 100}%` }}
                  />
                )}
                <div
                  className="absolute inset-y-0 left-0 bg-foreground"
                  style={{ width: `${duration > 0 ? (currentTime / duration) * 100 : 0}%` }}
                />
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
              <label className="flex items-center gap-1 type-small text-[hsl(var(--text2))]" title={BACKSTEP_RULE}>
                Backstep
                <span id={backstepRuleId} className="sr-only">
                  {BACKSTEP_RULE}
                </span>
                <select
                  aria-label="Backstep on resume"
                  aria-describedby={backstepRuleId}
                  className="h-7 rounded-md border border-border bg-card px-1.5 type-small text-foreground tabular-nums transition-colors focus:border-ring cursor-pointer"
                  value={backstep}
                  onChange={e => applyBackstep(loadBackstep(e.target.value))}
                >
                  {BACKSTEP_STEPS.map(s => (
                    <option key={s} value={s}>
                      {backstepLabel(s)}
                    </option>
                  ))}
                </select>
              </label>
              <div className="flex items-center gap-1" title="A-B loop: repeat a passage">
                <Repeat size={13} className={cn(loopActive ? 'text-foreground' : 'text-[hsl(var(--sub))]')} />
                <button
                  onClick={() => setLoopA(currentTime)}
                  className={cn(
                    'px-1.5 py-0.5 rounded text-[12px] tabular-nums transition-colors',
                    loopA != null
                      ? 'bg-secondary text-foreground ring-1 ring-inset ring-foreground'
                      : 'text-[hsl(var(--sub))] hover:text-foreground',
                  )}
                  title="Set loop start"
                >
                  {loopA != null ? fmtTime(loopA) : 'A'}
                </button>
                <button
                  onClick={() => setLoopB(currentTime)}
                  className={cn(
                    'px-1.5 py-0.5 rounded text-[12px] tabular-nums transition-colors',
                    loopB != null
                      ? 'bg-secondary text-foreground ring-1 ring-inset ring-foreground'
                      : 'text-[hsl(var(--sub))] hover:text-foreground',
                  )}
                  title="Set loop end"
                >
                  {loopB != null ? fmtTime(loopB) : 'B'}
                </button>
                {(loopA != null || loopB != null) && (
                  <button
                    onClick={() => {
                      setLoopA(null)
                      setLoopB(null)
                    }}
                    title="Clear A-B loop"
                    className="text-[hsl(var(--sub))] hover:text-destructive transition-colors"
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
              <button
                className="w-8 h-8 flex items-center justify-center text-[hsl(var(--text2))] transition-colors hover:bg-secondary hover:text-foreground"
                title="Add bookmark at current position (B)"
                aria-label="Add bookmark"
                onClick={addBookmark}
              >
                <Bookmark size={15} />
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}
