import { useState, useRef, useEffect, useLayoutEffect, useId, useMemo } from 'react'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import {
  Play,
  Pause,
  SkipBack,
  SkipForward,
  Bookmark,
  X,
  Plus,
  Repeat,
  Copy,
  Check,
  ListMusic,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  AudioLines,
  AlertTriangle,
} from 'lucide-react'
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
import { Segmented } from '../ui/segmented'
import Waveform from '../common/Waveform'
import { WaveformIcon } from '../common/Icons'
import WorkspaceHeader from '../common/WorkspaceHeader'
import Transcript from './Transcript'
import ListeningMix from './ListeningMix'
import PaneSplitter from './PaneSplitter'
import { fmtClock } from '../../lib/transcript'
import {
  isNarrow,
  paneRange,
  paneWidths,
  readLayout,
  resizeLayout,
  writeLayout,
  SPLITTER_WIDTH,
} from '../../lib/playerLayout'
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
// Layout: three panes over a PERSISTENT transport bar (controls + a thin
// progress scrubber). Left: Playlist and Bookmarks. Centre, the largest:
// transcript proofing. Right: Now Playing (compact waveform), the listening
// mix and the flags. Each pane scrolls inside itself, so the page does not.
// The side panes resize (drag or keyboard on the splitter) or collapse, and
// their sizes are kept per viewer (lib/playerLayout.js). Below 1100px they
// become drawers opened from a toolbar, and the transcript keeps the width.
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
  'Player bookmarks, speed, volume, backstep and pane sizes cannot be saved in this session. Export important bookmark notes before closing DepoAudio.'

const PANE_NAMES = { left: 'Playlist and bookmarks', right: 'Now playing, listening mix and flags' }

function browserStorage() {
  try {
    return window.localStorage
  } catch {
    return null
  }
}

function viewportWidth() {
  return typeof window === 'undefined' ? Infinity : window.innerWidth
}

function loadPlayerStorageState() {
  try {
    return {
      speed: loadSpeed(window.localStorage.getItem('player-speed')),
      backstep: loadBackstep(window.localStorage.getItem('player-backstep')),
      volume: loadVolume(window.localStorage.getItem('player-volume')),
      bookmarks: loadBookmarks(window.localStorage.getItem('player-bookmarks')),
      ...(() => {
        const { layout, error } = readLayout(window.localStorage)
        return { layout, error: error ? PLAYER_STORAGE_WARNING : '' }
      })(),
    }
  } catch {
    return {
      speed: 1,
      backstep: loadBackstep(null),
      volume: loadVolume(null),
      bookmarks: [],
      layout: readLayout(null).layout,
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
  // The track whose strips started at 100% because the previous track's
  // Mute, Solo or volume changes were dropped (the Player says so).
  const [stripsResetPath, setStripsResetPath] = useState(null)
  const mixerRef = useRef(null)
  // True once an element with the mix on has loaded a file. The asset
  // protocol's CORS then works, so a later error on another file is that
  // file's own decode error, not a reason to turn the mix off.
  const mixLoadedRef = useRef(false)
  // { time, play, pausedAt } to apply once a swapped-in element has loaded.
  // Until then, play/pause (buttons, in-app and playback keys) change `play`
  // instead of acting on an element that has not loaded yet.
  const restoreRef = useRef(null)
  // The pause position to keep through the restore's own seek, so turning
  // Separate channels on or off while paused keeps the Backstep resume point.
  const restoreSeekRef = useRef(null)
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
        'Court-recorder .sgmca/.trm/.ftr/.tll/.dm/.dcr files require conversion before playback. Send them to Convert and create a standard audio file first.',
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
    restoreSeekRef.current = null
    const pending = restoreRef.current
    if (pending) {
      // Paused here: the next resume rewinds by Backstep, as for any pause
      restoreRef.current = pending.play
        ? { ...pending, play: false, pausedAt: pending.time }
        : { ...pending, play: true }
      return
    }
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
    // A swap that has not loaded yet (for example the mix failed to start)
    // still holds the position to go back to.
    if (!restoreRef.current && a) {
      restoreRef.current = {
        time: a.currentTime || 0,
        play: !a.paused,
        pausedAt: a.paused ? pausedAtRef.current : null,
      }
    }
    a?.pause()
    mixLoadedRef.current = false
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

  // While a swapped-in element has not loaded, a seek moves the position the
  // restore will apply (setting currentTime on the new element would be
  // overwritten by the restore). Like any seek, it clears the Backstep point.
  const seekPending = time => {
    const pending = restoreRef.current
    if (!pending) return false
    restoreRef.current = { ...pending, time: Math.max(0, time), pausedAt: null }
    return true
  }
  const seekBy = delta => {
    const a = audioRef.current
    if (!a) return
    restoreSeekRef.current = null
    const pending = restoreRef.current
    if (pending) {
      seekPending(Math.min(pending.time + delta, duration || pending.time + delta))
      return
    }
    a.currentTime = Math.max(0, Math.min((a.currentTime || 0) + delta, a.duration || 0))
  }
  const seekTo = t => {
    restoreSeekRef.current = null
    if (seekPending(t)) return
    if (audioRef.current) audioRef.current.currentTime = t
  }

  // Play from a transcript line: seek there and play, with no backstep.
  const playFrom = t => {
    const a = audioRef.current
    if (!a || !activeTrack) return
    setPlaybackError('')
    pausedAtRef.current = null
    a.currentTime = t
    if (a.paused) a.play().catch(e => setPlaybackError(`Couldn't play ${activeTrack.name}: ${e}`))
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
    const isPlaying = () =>
      restoreRef.current ? restoreRef.current.play : !!audioRef.current && !audioRef.current.paused
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
      restoreRef.current = null
      restoreSeekRef.current = null
      mixerRef.current?.resetHeard()
      // Strips belong to one track. When the previous track's strips had
      // changes and the mix stays on, the new track starts at 100% and the
      // Player says so instead of the muted channel coming back unannounced.
      const dropped = mixOn && mix.path && mix.path !== activeTrack.path && mix.strips.some(stripChanged)
      setStripsResetPath(dropped ? activeTrack.path : null)
      if (mix.path && mix.path !== activeTrack.path) setMix({ path: null, strips: [] })
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
  // Only until the person changes a strip on this track.
  const stripsFromPreviousCleared = stripsResetPath === activePath && mix.path !== activePath
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

  // ── Pane layout ───────────────────────────────────────────────────────────
  const [layout, setLayout] = useState(initialStorage.layout)
  const layoutRef = useRef(layout)
  const [narrow, setNarrow] = useState(() => isNarrow(viewportWidth()))
  const narrowRef = useRef(narrow)
  const [drawer, setDrawer] = useState(null) // 'left' | 'right' in a narrow window
  const [workspaceWidth, setWorkspaceWidth] = useState(() => viewportWidth())
  const [flagsNode, setFlagsNode] = useState(null)
  const [clock, setClockState] = useState({ path: null, start: null })
  const workspaceRef = useRef(null)
  const drawerButtonRefs = useRef({})
  const drawerRefs = useRef({})
  // Where the keyboard goes after the panes change shape: a pane toggle
  // ('toggle:left'), a drawer's first control ('drawer'), or nowhere.
  const pendingFocusRef = useRef(null)
  const paneBaseId = useId()
  const paneIds = { left: `${paneBaseId}-left`, right: `${paneBaseId}-right` }
  const setClock = (path, start) =>
    setClockState(prev => (prev.path === path && prev.start === start ? prev : { path, start }))
  const clockStart = clock.path === activeTrack?.path ? clock.start : null

  // Crossing the breakpoint (a resize, or zooming in) keeps every pane
  // mounted (one tree, below), and the keyboard where it was: focus in a side
  // pane going narrow opens that pane's drawer around it; focus in a drawer
  // going wide stays in the pane, or moves to the pane's Show button when the
  // pane is collapsed.
  useEffect(() => {
    const onResize = () => {
      const next = isNarrow(viewportWidth())
      if (next === narrowRef.current) return
      narrowRef.current = next
      const active = document.activeElement
      const side = ['left', 'right'].find(k => drawerRefs.current[k]?.contains(active))
      setNarrow(next)
      if (next) {
        if (side) setDrawer(side)
      } else {
        setDrawer(null)
        const open = side === 'left' ? layoutRef.current.leftOpen : layoutRef.current.rightOpen
        if (side && !open) pendingFocusRef.current = `toggle:${side}`
      }
    }
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [])

  // The panes' real width, so the centre keeps the largest share.
  const hasTracks = tracks.length > 0
  useLayoutEffect(() => {
    const el = workspaceRef.current
    if (!el) return undefined
    const measure = () => setWorkspaceWidth(el.clientWidth || viewportWidth())
    measure()
    if (typeof ResizeObserver !== 'function') return undefined
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [narrow, hasTracks])

  const saveLayout = next => {
    if (!writeLayout(browserStorage(), next)) setStorageError(PLAYER_STORAGE_WARNING)
  }
  // The next layout is worked out here, not in a state updater, so saving it
  // (and the storage warning) happens once, as a plain side effect.
  const commitLayout = (next, save = true) => {
    layoutRef.current = next
    setLayout(next)
    if (save) saveLayout(next)
  }
  const updateLayout = (patch, save = true) => commitLayout({ ...layoutRef.current, ...patch }, save)
  const resizePane = (side, width, save) =>
    commitLayout(resizeLayout(layoutRef.current, workspaceWidth, side, width), save)

  const widths = paneWidths(layout, workspaceWidth)
  const ranges = { left: paneRange(layout, workspaceWidth, 'left'), right: paneRange(layout, workspaceWidth, 'right') }
  const gridColumns = [
    `${widths.left}px`,
    layout.leftOpen ? `${SPLITTER_WIDTH}px` : null,
    'minmax(0,1fr)',
    layout.rightOpen ? `${SPLITTER_WIDTH}px` : null,
    `${widths.right}px`,
  ]
    .filter(Boolean)
    .join(' ')

  const closeDrawer = () => {
    const open = drawer
    setDrawer(null)
    if (open) drawerButtonRefs.current[open]?.focus()
  }
  const openDrawer = side => {
    pendingFocusRef.current = 'drawer'
    setDrawer(side)
  }
  const closeDrawerRef = useRef(closeDrawer)
  closeDrawerRef.current = closeDrawer

  // Moves the keyboard after the panes change shape: into a drawer the
  // person opened, or to the button that now stands for a pane they hid or
  // showed (the one they pressed is gone).
  useLayoutEffect(() => {
    const target = pendingFocusRef.current
    if (!target) return
    pendingFocusRef.current = null
    if (target === 'drawer') {
      if (drawer) drawerRefs.current[drawer]?.querySelector('button, input, select, textarea, [tabindex="0"]')?.focus()
      return
    }
    const side = target.slice('toggle:'.length)
    document.querySelector(`[data-pane-toggle="${side}"]`)?.focus()
  }, [drawer, layout.leftOpen, layout.rightOpen, narrow])

  // Esc closes the open drawer from anywhere inside it. A native listener on
  // the drawer element, because the flags list is rendered into the drawer
  // through a portal and React would bubble its key events to the Transcript,
  // not to the drawer.
  useEffect(() => {
    if (!narrow || !drawer) return undefined
    const el = drawerRefs.current[drawer]
    if (!el) return undefined
    const onKey = event => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      event.preventDefault()
      event.stopPropagation()
      closeDrawerRef.current()
    }
    el.addEventListener('keydown', onKey)
    return () => el.removeEventListener('keydown', onKey)
  }, [narrow, drawer])

  const iconButton =
    'h-7 w-7 shrink-0 inline-flex items-center justify-center text-ink-2 transition-colors hover:bg-secondary hover:text-foreground'
  // The button that hides or shows a side pane (or closes its drawer).
  const paneToggle = side => {
    const name = PANE_NAMES[side].toLowerCase()
    if (narrow) {
      return (
        <button
          type="button"
          className={iconButton}
          aria-label={`Close ${name}`}
          title="Close (Esc)"
          onClick={closeDrawer}
        >
          <X size={14} />
        </button>
      )
    }
    const open = side === 'left' ? layout.leftOpen : layout.rightOpen
    const Icon = side === 'left' ? (open ? PanelLeftClose : PanelLeftOpen) : open ? PanelRightClose : PanelRightOpen
    return (
      <button
        type="button"
        className={iconButton}
        aria-label={`${open ? 'Hide' : 'Show'} ${name}`}
        title={`${open ? 'Hide' : 'Show'} ${name}`}
        aria-expanded={open}
        aria-controls={paneIds[side]}
        data-pane-toggle={side}
        onClick={() => {
          pendingFocusRef.current = `toggle:${side}`
          updateLayout(side === 'left' ? { leftOpen: !open } : { rightOpen: !open })
        }}
      >
        <Icon size={15} />
      </button>
    )
  }
  const drawerButton = side => {
    const Icon = side === 'left' ? ListMusic : AudioLines
    const open = drawer === side
    return (
      <Button
        ref={el => {
          drawerButtonRefs.current[side] = el
        }}
        size="sm"
        variant="outline"
        className={cn(open && 'bg-secondary text-foreground ring-1 ring-inset ring-foreground')}
        aria-expanded={open}
        aria-controls={paneIds[side]}
        onClick={() => (open ? closeDrawer() : openDrawer(side))}
      >
        <Icon size={13} /> {side === 'left' ? 'Playlist and bookmarks' : 'Now playing and flags'}
      </Button>
    )
  }

  const speedOptions = SPEED_STEPS.map(s => ({ value: s, label: `${s}×`, title: `${s}× playback speed` }))

  const warnGlyph = <AlertTriangle aria-hidden="true" size={14} className="mt-0.5 shrink-0 text-warning" />
  const convertFirstNotice = (convertFirstPaths.length > 0 || playbackError) && (
    <div role="alert" className="notice flex items-center gap-2">
      {warnGlyph}
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
        className={iconButton}
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
    <div role="alert" className="notice flex items-start gap-2">
      {warnGlyph}
      <span className="flex-1">{storageError}</span>
      <button
        type="button"
        className={iconButton}
        aria-label="Dismiss player storage warning"
        onClick={() => setStorageError('')}
      >
        <X size={12} />
      </button>
    </div>
  )

  const transcriptStorageNotice = transcriptStorageError && (
    <div role="alert" className="notice flex items-start gap-2">
      {warnGlyph}
      <span className="flex-1">{transcriptStorageError}</span>
      <button
        type="button"
        aria-label="Dismiss transcript storage warning"
        className={iconButton}
        onClick={() => setTranscriptStorageError('')}
      >
        <X size={12} />
      </button>
    </div>
  )

  const notices = (storageNotice || transcriptStorageNotice || convertFirstNotice) && (
    <div className={cn('flex shrink-0 flex-col gap-2', tracks.length ? 'px-4 pt-3' : 'mb-4')}>
      {storageNotice}
      {transcriptStorageNotice}
      {convertFirstNotice}
    </div>
  )

  // ── Empty state ───────────────────────────────────────────────────────────
  if (tracks.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto overflow-x-hidden">
        <div className="w-full max-w-[1100px] mx-auto px-4 sm:px-6 lg:px-8 py-5">
          {notices}
          <WorkspaceHeader
            eyebrow="Local review"
            title="Review a recording"
            description="Listen across a multi-file session, mark useful moments, and keep notes beside the audio."
            status="Playback stays on this computer"
            className="mb-4"
          />
          {playbackKeys?.settings.enabled && (
            <p role="status" className="mb-3 type-small text-ink-2">
              Playback keys are on, but they do nothing until a recording is loaded here.
            </p>
          )}
          <div
            className={cn(
              'grid items-center gap-4 border border-dashed px-4 py-4 transition-colors sm:grid-cols-[auto_minmax(0,1fr)_auto]',
              dragOver ? 'border-foreground bg-secondary' : 'border-input bg-card',
            )}
            onDragOver={e => {
              e.preventDefault()
              setDragOver(true)
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={handleDrop}
          >
            <span aria-hidden="true" className="grid place-items-center">
              <WaveformIcon />
            </span>
            <div className="min-w-0">
              <p className="font-semibold">Add audio to the review playlist</p>
              <p className="type-small text-ink-2">
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

  // Inside a pane, sections sit flush, split by one rule: no boxes.
  const paneSection = 'border-t border-border px-4 pb-3'
  const paneTitle = 'type-label text-ink-2'

  // The flags are the output of proofing, so they come straight after the
  // track and waveform, above the listening mix (which can grow tall).
  const flagsCard = <div ref={setFlagsNode} className={paneSection} />

  const nowPlaying = activeTrack && (
    <div>
      <div className="pane-head">
        <h2 className={paneTitle}>Now Playing</h2>
        {paneToggle('right')}
      </div>
      <div className="px-4 pb-3 flex flex-col gap-2">
        <div className="flex min-w-0 flex-col">
          <div className="flex items-center gap-2 min-w-0">
            <span className="w-2 h-2 rounded-full shrink-0" style={{ background: activeColor }} />
            <span className="font-semibold truncate" title={activeTrack.name}>
              {activeTrack.name}
            </span>
          </div>
          <span className="pl-4 type-small text-ink-2 truncate">{activeTrack.label}</span>
        </div>
        <Waveform
          audioPath={activeTrack.path}
          color={activeColor}
          currentTime={currentTime}
          duration={duration}
          height={56}
          onSeek={seekTo}
          markers={trackBookmarks}
          loop={loopActive ? { a: loopA, b: loopB } : null}
        />
        <div className="flex items-center justify-between tabular-nums type-small text-ink-2">
          <span>{fmtTime(currentTime)}</span>
          <span>{fmtTime(duration)}</span>
        </div>
        {clockStart != null && (
          <p
            className="flex items-baseline justify-between gap-2 type-small text-ink-2"
            title="From the recording start time you entered for the transcript"
          >
            <span>Time of day</span>
            <span className="tabular-nums font-medium text-foreground">{fmtClock(clockStart + currentTime)}</span>
          </p>
        )}
      </div>
    </div>
  )

  const listeningMix = activeTrack && (
    <div className={paneSection}>
      <div className="flex h-10 items-center">
        <h3 className={paneTitle}>Listening mix</h3>
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
        stripsFromPreviousCleared={stripsFromPreviousCleared}
        notice={mixNotice}
        compact
      />
    </div>
  )

  const leftContent = (
    <div className="flex flex-col">
      <div>
        <div className="pane-head">
          <h3 className={paneTitle}>Playlist</h3>
          <div className="flex items-center gap-1">
            <Button size="sm" onClick={browseFiles}>
              <Plus size={12} /> Add
            </Button>
            {paneToggle('left')}
          </div>
        </div>
        <div className="px-2 pb-2">
          {tracks.map((t, i) => (
            <div
              key={t.path}
              className={cn(
                'group relative flex items-center gap-2 border-l-2 py-1 pl-1.5 pr-1 transition-colors',
                activeTrack?.path === t.path
                  ? 'border-l-foreground bg-secondary'
                  : 'border-l-transparent hover:bg-secondary/50',
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
                  className="inset-focus flex items-center gap-2 min-w-0 text-left"
                  aria-current={activeTrack?.path === t.path ? 'true' : undefined}
                  onClick={() => selectTrack(t)}
                >
                  <span className="w-2 h-2 rounded-full shrink-0" style={{ background: colorOf(t) }} />
                  <span className="type-small text-foreground truncate" title={t.name}>
                    {t.name}
                  </span>
                </button>
                <input
                  className="pl-4 type-small text-ink-2 bg-transparent w-full focus:text-foreground"
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
                <span className="w-6 shrink-0 text-center type-min tabular-nums text-ink-2 group-hover:invisible group-focus-within:invisible">
                  {i + 1}
                </span>
              )}
              <button
                className="absolute right-1 inline-flex h-6 w-6 items-center justify-center text-ink-2 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 hover:bg-secondary hover:text-destructive"
                aria-label={`Remove ${t.name}`}
                onClick={e => {
                  e.stopPropagation()
                  removeTrack(t.path)
                }}
              >
                <X size={13} />
              </button>
            </div>
          ))}
        </div>
      </div>

      {/* Bookmarks for the active track */}
      {activeTrack && (
        <div className="border-t border-border">
          <div className="pane-head">
            <h3 className={paneTitle}>Bookmarks</h3>
            {trackBookmarks.length > 0 && (
              <>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={copyBookmarks}
                  title="Copy bookmarks to clipboard as timestamped lines"
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
                </Button>
                <span role="status" aria-live="polite" className="sr-only">
                  {copied ? 'Bookmarks copied to clipboard.' : ''}
                </span>
              </>
            )}
          </div>
          <div className="px-2 pb-2">
            {trackBookmarks.length === 0 ? (
              <p className="px-2 type-small text-ink-2">
                Press <kbd className="tabular-nums">B</kbd> or the bookmark button to mark the current spot.
              </p>
            ) : (
              <div className="flex flex-col">
                {trackBookmarks
                  .slice()
                  .sort((a, b) => a.time - b.time)
                  .map(b => (
                    <div key={`${b.time}-${b.trackPath}`} className="flex items-center gap-2 group py-0.5 pl-2">
                      <button
                        className="inset-focus type-min tabular-nums text-ink-2 hover:text-foreground shrink-0 w-11 text-left"
                        title="Jump to bookmark"
                        onClick={() => seekTo(b.time)}
                      >
                        {fmtTime(b.time)}
                      </button>
                      <input
                        className="flex-1 min-w-0 bg-transparent type-small placeholder:italic placeholder:text-ink-2"
                        value={b.label}
                        placeholder="Add a note…"
                        onChange={e =>
                          setBookmarks(prev => prev.map(x => (x === b ? { ...x, label: e.target.value } : x)))
                        }
                      />
                      <button
                        className="inline-flex h-6 w-6 shrink-0 items-center justify-center text-ink-2 transition-colors hover:bg-secondary hover:text-destructive"
                        aria-label={`Remove bookmark at ${fmtTime(b.time)}`}
                        title="Remove bookmark"
                        onClick={() => setBookmarks(prev => prev.filter(x => x !== b))}
                      >
                        <X size={13} />
                      </button>
                    </div>
                  ))}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )

  const rightContent = (
    <div className="flex flex-col">
      {nowPlaying}
      {flagsCard}
      {listeningMix}
    </div>
  )

  const centre = activeTrack && (
    <Transcript
      key={activeTrack.path}
      trackPath={activeTrack.path}
      currentTime={currentTime}
      playing={playing}
      duration={duration}
      onSeek={seekTo}
      onPlayFrom={playFrom}
      onStorageError={setTranscriptStorageError}
      onClockChange={setClock}
      flagsContainer={flagsNode}
      flush
    />
  )

  // A side pane. Wide: open (content, scrolling inside itself) or a
  // collapsed rail with the button that opens it again. Narrow: a drawer
  // over the transcript, shown while it is the open drawer. The element and
  // its content stay the same either way, so crossing the breakpoint (or
  // zooming) never remounts the panes or the transcript.
  const sidePane = side => {
    const open = side === 'left' ? layout.leftOpen : layout.rightOpen
    const showContent = narrow || open
    return (
      <aside
        id={paneIds[side]}
        ref={el => {
          drawerRefs.current[side] = el
        }}
        aria-label={PANE_NAMES[side]}
        hidden={narrow && drawer !== side ? true : undefined}
        data-drawer={narrow ? 'true' : undefined}
        className={cn(
          'min-h-0 min-w-0 bg-card',
          // An open pane's edge is drawn by its splitter, so only the rail
          // and the drawer carry a border of their own.
          (narrow || !open) && (side === 'left' ? 'border-r border-border' : 'border-l border-border'),
          narrow
            ? cn(
                'absolute inset-y-0 z-20 w-[min(22rem,88%)] overflow-y-auto overflow-x-hidden',
                side === 'left' ? 'left-0' : 'right-0',
              )
            : open
              ? 'relative overflow-y-auto overflow-x-hidden'
              : 'relative flex flex-col items-center pt-1.5',
        )}
      >
        {showContent ? (side === 'left' ? leftContent : rightContent) : paneToggle(side)}
      </aside>
    )
  }

  const splitter = side => (
    <PaneSplitter
      side={side}
      label={side === 'left' ? 'Resize the playlist and bookmarks pane' : 'Resize the now playing and flags pane'}
      controls={paneIds[side]}
      value={widths[side]}
      min={ranges[side].min}
      max={ranges[side].max}
      onChange={w => resizePane(side, w, false)}
      onCommit={w => resizePane(side, w, true)}
    />
  )

  // ── Workspace: three panes over a fixed transport bar ─────────────────────
  // One element tree for both layouts; only the containers' styles change.
  return (
    <>
      <div
        className="relative flex min-h-0 flex-1 flex-col overflow-hidden"
        data-testid="player-workspace"
        data-layout={narrow ? 'narrow' : 'panes'}
      >
        {notices}
        {narrow && (
          <div
            className="shrink-0 flex h-10 items-center gap-2 border-b border-border px-4"
            role="toolbar"
            aria-label="Player panes"
          >
            {drawerButton('left')}
            <span className="flex-1" />
            {drawerButton('right')}
          </div>
        )}
        <div
          ref={workspaceRef}
          className={cn('relative min-h-0 flex-1', !narrow && 'grid')}
          style={narrow ? undefined : { gridTemplateColumns: gridColumns }}
          data-testid={narrow ? undefined : 'player-panes'}
        >
          {sidePane('left')}
          {!narrow && layout.leftOpen && splitter('left')}
          <section
            aria-label="Transcript"
            // While a drawer covers it, the transcript is out of reach of
            // Tab and the pointer, so nothing behind the drawer takes focus.
            inert={narrow && drawer ? true : undefined}
            className={cn('relative min-h-0 min-w-0 overflow-y-auto', narrow && 'h-full')}
            data-testid="player-centre"
          >
            {centre}
          </section>
          {!narrow && layout.rightOpen && splitter('right')}
          {sidePane('right')}
          {narrow && drawer && (
            // Clicking outside the drawer closes it.
            <div
              aria-hidden="true"
              data-testid="drawer-scrim"
              className="absolute inset-0 z-10 bg-[hsl(var(--background)/0.6)]"
              onClick={closeDrawer}
            />
          )}
        </div>
      </div>
      {/* ── Persistent transport bar ─────────────────────────────────────── */}
      {activeTrack && (
        <div className="shrink-0 border-t border-border bg-card px-4 py-2">
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
              if (mixOn) mixLoadedRef.current = true
              const restore = restoreRef.current
              restoreRef.current = null
              if (restore) {
                if (restore.time > 0) {
                  restoreSeekRef.current = restore.play ? null : restore.pausedAt
                  e.target.currentTime = Math.min(restore.time, e.target.duration || restore.time)
                }
                if (!restore.play) pausedAtRef.current = restore.pausedAt ?? null
                if (restore.play) {
                  mixerRef.current?.resume()
                  e.target.play().catch(err => setPlaybackError(`Couldn't play ${activeTrack.name}: ${err}`))
                }
              }
            }}
            onPlay={e => {
              setPlaying(true)
              const mixer = mixerRef.current
              if (!mixer) return
              mixer.setPlaying(true)
              // A play from a playback key is not a user gesture. It is heard
              // because wry turns on autoplay without a gesture (WKWebView:
              // no media type needs user action; WebView2: the autoplay-policy
              // flag, added only while tauri.conf.json sets no
              // additionalBrowserArgs, which a contract test checks). If the
              // context still does not run, the element would play with no
              // sound, so the Player goes back to normal playback and says why.
              const element = e.target
              mixer.resume().then(running => {
                if (running || mixerRef.current !== mixer || element.paused) return
                latestRef.current.mixFailed('the web view kept Web Audio paused, so nothing would be heard')
              })
            }}
            onPause={e => {
              setPlaying(false)
              mixerRef.current?.setPlaying(false)
              // A pause at the end of the track is not a resume point
              pausedAtRef.current = e.target.ended ? null : e.target.currentTime
            }}
            onSeeking={() => {
              pausedAtRef.current = restoreSeekRef.current
              restoreSeekRef.current = null
            }}
            onError={() => {
              setPlaying(false)
              restoreSeekRef.current = null
              // Once the mix has opened a file, an error on another file (a new
              // or auto-advanced track) is that file's decode error: the mix
              // stays on for the next track.
              if (mixOn && !mixLoadedRef.current) {
                mixFailed('the web view would not open this file for Web Audio')
                return
              }
              restoreRef.current = null
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

          <div className="player-transport w-full max-w-[1100px] mx-auto flex items-center gap-4">
            {/* Transport buttons */}
            <div className="flex items-center gap-1 shrink-0">
              <button
                className="w-8 h-8 flex items-center justify-center text-ink-2 transition-colors hover:bg-secondary hover:text-foreground"
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
                className="w-8 h-8 flex items-center justify-center text-ink-2 transition-colors hover:bg-secondary hover:text-foreground"
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
                <span className="type-small font-medium truncate">{activeTrack.name}</span>
                <span className="type-small text-ink-2 shrink-0 hidden sm:inline">· {activeTrack.label}</span>
                <span className="ml-auto type-min tabular-nums text-ink-2 shrink-0">
                  {fmtTime(currentTime)} / {fmtTime(duration)}
                </span>
              </div>
              {/* Thin progress scrubber — the detailed waveform lives in Now
                  Playing above, so the pinned bar stays cheap (no second
                  decode) while still showing the loop band + position. */}
              <div
                className="relative h-2 bg-border cursor-pointer overflow-hidden"
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
            <div className="shrink-0 flex items-center gap-3">
              <Segmented
                size="sm"
                aria-label="Playback speed"
                options={speedOptions}
                value={speed}
                onChange={applySpeed}
                className="hidden lg:inline-flex"
              />
              <label className="flex items-center gap-1 type-small text-ink-2" title={BACKSTEP_RULE}>
                Backstep
                <span id={backstepRuleId} className="sr-only">
                  {BACKSTEP_RULE}
                </span>
                <select
                  aria-label="Backstep on resume"
                  aria-describedby={backstepRuleId}
                  className="h-7 border border-input bg-card px-1.5 type-small tabular-nums cursor-pointer"
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
                <Repeat size={13} className={cn(loopActive ? 'text-foreground' : 'text-ink-2')} />
                <button
                  onClick={() => setLoopA(currentTime)}
                  className={cn(
                    'h-7 px-2 type-small tabular-nums transition-colors',
                    loopA != null
                      ? 'bg-secondary text-foreground ring-1 ring-inset ring-foreground'
                      : 'text-ink-2 hover:text-foreground',
                  )}
                  title="Set loop start"
                >
                  {loopA != null ? fmtTime(loopA) : 'A'}
                </button>
                <button
                  onClick={() => setLoopB(currentTime)}
                  className={cn(
                    'h-7 px-2 type-small tabular-nums transition-colors',
                    loopB != null
                      ? 'bg-secondary text-foreground ring-1 ring-inset ring-foreground'
                      : 'text-ink-2 hover:text-foreground',
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
                    className={cn(iconButton, 'hover:text-destructive')}
                  >
                    <X size={13} />
                  </button>
                )}
              </div>
              <button
                className="w-8 h-8 flex items-center justify-center text-ink-2 transition-colors hover:bg-secondary hover:text-foreground"
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
