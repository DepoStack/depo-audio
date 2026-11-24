// ── Player listening mix ─────────────────────────────────────────────────────
//
// Master volume and a per-channel mix for what the person hears in the
// Player. Nothing here touches a file: the mix is built in the web view's
// Web Audio graph and goes to the speakers only.
//
//   <audio> → MediaElementSource → ChannelSplitter(N) → Gain per channel
//           → ChannelMerger(2) → master Gain → speakers
//
// A 2-channel file keeps its left/right placement. With 3 or more channels
// every channel is sent to both ears, so a microphone on channel 5 is not
// dropped by a stereo output. Channels sent to the same ear add up, so each
// heard channel is scaled by 1 / (number of heard channels): with every strip
// at 100%, four mics that all pick up the same voice sum to the level of one
// mic, not four times it, and soloing one mic plays it at its own level. The source also feeds a small AudioWorklet that
// reports how many channels the web view actually passes to Web Audio: a web
// view may mix a multichannel file down before Web Audio sees it, and the
// Player says so instead of showing strips that would do nothing.

export const LISTENING_ONLY_NOTE = 'Changes what you hear only. Converted files and exports are not affected.'
export const MASTER_VOLUME_STEP = 0.05
export const DEFAULT_MASTER_VOLUME = 1
export const CHANNEL_VOLUME_MAX = 2
export const DEFAULT_CHANNEL_VOLUME = 1
// Web Audio's own channel limit (AudioContext max channel count in Chromium
// and WebKit).
export const MAX_MIX_CHANNELS = 32
// Channel counts the probe reports in the first moments of playback can be
// the source node's 2-channel placeholder, before the decoder's format
// arrives, so counts are only trusted after this much continuous playback.
export const CHANNEL_COUNT_SETTLE_MS = 400

// Persisted master volume (0–1). Missing or invalid values play at full
// volume, which is what the Player did before it had a volume control.
export function loadVolume(raw) {
  if (raw == null || raw === '') return DEFAULT_MASTER_VOLUME
  const v = Number(raw)
  if (!Number.isFinite(v)) return DEFAULT_MASTER_VOLUME
  return Math.min(1, Math.max(0, Math.round(v / MASTER_VOLUME_STEP) * MASTER_VOLUME_STEP))
}

export function volumePercent(v) {
  return `${Math.round((Number.isFinite(v) ? v : 0) * 100)}%`
}

export function initialStrips(count) {
  return Array.from({ length: Math.max(0, count) }, () => ({
    volume: DEFAULT_CHANNEL_VOLUME,
    muted: false,
    soloed: false,
  }))
}

// Which channels are heard. Any solo means only soloed channels are heard;
// a muted channel is never heard, even when it is soloed.
export function audibleChannels(strips) {
  const anySolo = strips.some(s => s.soloed)
  return strips.map(s => !s.muted && (!anySolo || s.soloed))
}

// Gain for each channel's GainNode: its own volume when heard, else 0. With
// 3 or more channels (all summed into both ears) each heard channel is also
// divided by the number of heard channels so the sum cannot exceed what the
// loudest single channel would play at the same strip volume. The master
// volume is applied once, on the master node.
export function channelGains(strips) {
  const audible = audibleChannels(strips)
  const heard = audible.filter(Boolean).length
  const share = strips.length >= 3 && heard > 1 ? 1 / heard : 1
  return strips.map((s, i) => (audible[i] ? clampChannelVolume(s.volume) * share : 0))
}

// True when a strip differs from its default (Mute, Solo or volume changed).
export function stripChanged(s) {
  return Boolean(s?.muted || s?.soloed || (s && s.volume !== DEFAULT_CHANNEL_VOLUME))
}

export function clampChannelVolume(v) {
  const n = Number(v)
  if (!Number.isFinite(n)) return DEFAULT_CHANNEL_VOLUME
  return Math.min(CHANNEL_VOLUME_MAX, Math.max(0, n))
}

// Merger inputs (0 = left, 1 = right) each channel is sent to.
export function channelRouting(count) {
  if (count === 2) return [[0], [1]]
  return Array.from({ length: Math.max(0, count) }, () => [0, 1])
}

// Labels for the strips. When the web view passes fewer channels than the
// file has, what Web Audio sees is the web view's own down-mix, not the
// file's channels, and the labels say so.
export function stripLabels({ fileChannels, heardChannels }) {
  const downmixed = heardChannels != null && fileChannels != null && heardChannels < fileChannels
  const count = downmixed ? heardChannels : fileChannels || 0
  // One channel has nothing to separate: no strips.
  if (count < 2) return []
  if (!downmixed) return Array.from({ length: count }, (_, i) => `Ch ${i + 1}`)
  if (count === 2) return ['Left mix', 'Right mix']
  return Array.from({ length: count }, (_, i) => `Mix ${i + 1}`)
}

// How many channels the mix splits: the file's channels, or fewer when the
// web view mixes the file down before Web Audio sees it.
export function mixChannelCount({ fileChannels, heardChannels }) {
  if (!fileChannels) return 0
  return heardChannels != null && heardChannels < fileChannels ? heardChannels : fileChannels
}

// What the status line says about the strips: how many channels are heard
// once anything is muted or soloed, or that nothing is heard.
export function hearingSummary(strips) {
  if (!Array.isArray(strips) || strips.length < 2) return ''
  const heard = audibleChannels(strips).filter(Boolean).length
  if (heard === 0) return 'Every channel is silent. Turn off a Mute or a Solo to hear the file.'
  if (!strips.some(s => s.muted || s.soloed)) return ''
  return `Hearing ${heard} of ${strips.length} channels.`
}

// The plain note shown above the strips about how many channels the Player
// can separate here.
export function separationNote({ fileChannels, heardChannels, probeAvailable }) {
  if (!fileChannels || fileChannels < 2) return ''
  if (!probeAvailable) {
    return 'This web view cannot report how many channels it passes to the Player. If a strip makes no difference, use Split Channels in Convert to hear each channel.'
  }
  if (heardChannels == null) return 'Play the file to check how many channels reach the Player.'
  if (heardChannels >= fileChannels) return ''
  const can = heardChannels < 2 ? 'cannot separate them here' : `can separate ${heardChannels} here`
  return `This file has ${fileChannels} channels; the web view mixes them down, so the Player ${can}. Use Split Channels in Convert to hear each channel.`
}

// Number of channels in the file DepoAudio plays (probe_audio_streams_cmd's
// answer). The web view plays the first audio track, which is the one
// convertedIndex names.
export function probedChannelCount(info) {
  const streams = Array.isArray(info?.streams) ? info.streams : []
  const index = Number.isInteger(info?.convertedIndex) ? info.convertedIndex : 0
  const channels = streams[index]?.channels
  return Number.isInteger(channels) && channels > 0 ? Math.min(channels, MAX_MIX_CHANNELS) : null
}

export function audioContextClass(win = globalThis.window) {
  return win?.AudioContext || win?.webkitAudioContext || null
}

const PROBE_NAME = 'depoaudio-channel-count'
// Vite emits the worklet as its own same-origin file, which the app's CSP
// (script-src 'self') allows.
export const CHANNEL_PROBE_URL = new URL('./channelCountProbe.worklet.js', import.meta.url).href

function setGain(context, param, value) {
  if (typeof param.setTargetAtTime === 'function') {
    param.cancelScheduledValues?.(context.currentTime)
    // ~10 ms ramp: no click when a channel is muted mid-word.
    param.setTargetAtTime(value, context.currentTime, 0.01)
  } else {
    param.value = value
  }
}

// Build the graph for one <audio> element. The element must carry
// crossOrigin="anonymous" (Tauri's asset protocol answers with
// Access-Control-Allow-Origin for the window), or WebKit and Chromium output
// silence for it. A MediaElementSource can be made only once per element, so
// turning the mix off means a new element; dispose() closes the context.
export function createChannelMixer({
  audio,
  channels,
  AudioContextCtor = audioContextClass(),
  probeUrl = null,
  now = () => globalThis.performance?.now?.() ?? Date.now(),
  onHeardChannels = () => {},
  onProbeUnavailable = () => {},
}) {
  if (!AudioContextCtor) throw new Error('This web view has no Web Audio support.')
  const context = new AudioContextCtor()
  let source
  try {
    source = context.createMediaElementSource(audio)
  } catch (error) {
    context.close?.()
    throw error
  }
  const master = context.createGain()
  master.connect(context.destination)

  let splitter = null
  let merger = null
  let gains = []
  let layout = 0
  let disposed = false
  let probe = null
  let playingSince = null
  let heard = null
  let lastGains = []

  const teardownLayout = () => {
    try {
      source.disconnect(splitter)
    } catch {
      // already disconnected
    }
    splitter?.disconnect()
    gains.forEach(g => g.disconnect())
    merger?.disconnect()
    splitter = null
    merger = null
    gains = []
  }

  const buildLayout = count => {
    if (splitter) teardownLayout()
    layout = Math.max(1, Math.min(count, MAX_MIX_CHANNELS))
    splitter = context.createChannelSplitter(layout)
    merger = context.createChannelMerger(2)
    const routes = channelRouting(layout)
    // Until the caller sends strips for this layout, every channel is heard
    // at its default share, never at full level summed N times.
    const start = lastGains.length === layout ? lastGains : channelGains(initialStrips(layout))
    gains = routes.map((targets, i) => {
      const gain = context.createGain()
      gain.gain.value = start[i]
      splitter.connect(gain, i)
      targets.forEach(input => gain.connect(merger, 0, input))
      return gain
    })
    source.connect(splitter)
    merger.connect(master)
  }

  buildLayout(channels)

  const startProbe = async () => {
    const worklet = context.audioWorklet
    const Node = globalThis.AudioWorkletNode
    if (!probeUrl || !worklet || typeof Node !== 'function') {
      onProbeUnavailable()
      return
    }
    try {
      await worklet.addModule(probeUrl)
      if (disposed) return
      probe = new Node(context, PROBE_NAME, {
        numberOfInputs: 1,
        numberOfOutputs: 0,
        channelCountMode: 'max',
        channelInterpretation: 'discrete',
      })
      probe.port.onmessage = event => {
        const count = Number(event?.data?.channels)
        if (!Number.isInteger(count) || count < 1) return
        if (playingSince == null || now() - playingSince < CHANNEL_COUNT_SETTLE_MS) return
        if (heard != null && count <= heard) return
        heard = count
        onHeardChannels(count)
      }
      source.connect(probe)
    } catch {
      if (!disposed) onProbeUnavailable()
    }
  }
  const probeReady = startProbe()

  return {
    context,
    probeReady,
    get layout() {
      return layout
    },
    get heardChannels() {
      return heard
    },
    // Re-route for the channel count Web Audio really receives.
    setLayout(count) {
      if (disposed || !count || count === layout) return
      buildLayout(count)
    },
    setMaster(volume) {
      if (disposed) return
      setGain(context, master.gain, Math.min(1, Math.max(0, Number(volume) || 0)))
    },
    // strips must have one entry per channel of the current layout; any other
    // length is ignored (stale strips must never silence or boost a channel
    // the person cannot see).
    setStrips(strips) {
      if (disposed) return
      const list = Array.isArray(strips) && strips.length === layout ? strips : initialStrips(layout)
      lastGains = channelGains(list)
      gains.forEach((g, i) => setGain(context, g.gain, lastGains[i]))
    },
    // A new track: counts heard for the previous file no longer apply.
    resetHeard() {
      heard = null
      playingSince = null
    },
    setPlaying(playing) {
      playingSince = playing ? now() : null
    },
    // Resolves to whether the context is running afterwards. Any state other
    // than 'running' or 'closed' is resumed: 'suspended', and WebKit's
    // 'interrupted' (for example after another app took the audio device).
    resume() {
      if (disposed || context.state === 'closed') return Promise.resolve(false)
      if (context.state === 'running') return Promise.resolve(true)
      return Promise.resolve(context.resume?.())
        .catch(() => {})
        .then(() => !disposed && context.state === 'running')
    },
    dispose() {
      if (disposed) return
      disposed = true
      teardownLayout()
      if (probe) {
        try {
          source.disconnect(probe)
        } catch {
          // already disconnected
        }
        probe.port.onmessage = null
        probe.port.close?.()
        probe = null
      }
      source.disconnect()
      master.disconnect()
      Promise.resolve(context.close?.()).catch(() => {})
    },
  }
}
