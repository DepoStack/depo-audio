import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  CHANNEL_COUNT_SETTLE_MS,
  audibleChannels,
  channelGains,
  channelRouting,
  createChannelMixer,
  initialStrips,
  loadVolume,
  probedChannelCount,
  separationNote,
  stripLabels,
  volumePercent,
} from '../lib/channelMixer'
import { installFakeAudio } from './helpers/fakeAudioContext'

const strip = (patch = {}) => ({ volume: 1, muted: false, soloed: false, ...patch })

describe('listening mix gain math', () => {
  it('plays every channel at its own volume when nothing is muted or soloed', () => {
    expect(channelGains([strip(), strip({ volume: 0.5 }), strip({ volume: 2 })])).toEqual([1, 0.5, 2])
  })

  it('any solo leaves only soloed channels audible', () => {
    const strips = [strip(), strip({ soloed: true }), strip(), strip({ soloed: true, volume: 0.25 })]
    expect(audibleChannels(strips)).toEqual([false, true, false, true])
    expect(channelGains(strips)).toEqual([0, 1, 0, 0.25])
  })

  it('mute wins: a muted channel is silent with or without solo', () => {
    expect(channelGains([strip({ muted: true }), strip()])).toEqual([0, 1])
    expect(channelGains([strip({ muted: true, soloed: true }), strip()])).toEqual([0, 0])
    expect(channelGains([strip({ muted: true, soloed: true }), strip({ soloed: true })])).toEqual([0, 1])
  })

  it('clamps channel volume to 0–200% and ignores junk', () => {
    expect(channelGains([strip({ volume: 5 }), strip({ volume: -1 }), strip({ volume: 'x' })])).toEqual([2, 0, 1])
  })

  it('keeps left/right for 2 channels and sends 3+ channels to both ears', () => {
    expect(channelRouting(2)).toEqual([[0], [1]])
    expect(channelRouting(1)).toEqual([[0, 1]])
    expect(channelRouting(4)).toEqual([
      [0, 1],
      [0, 1],
      [0, 1],
      [0, 1],
    ])
  })

  it('loads the saved master volume, defaulting to full volume', () => {
    expect(loadVolume(null)).toBe(1)
    expect(loadVolume('')).toBe(1)
    expect(loadVolume('abc')).toBe(1)
    expect(loadVolume('0.42')).toBeCloseTo(0.4)
    expect(loadVolume('0')).toBe(0)
    expect(loadVolume('3')).toBe(1)
    expect(volumePercent(0.55)).toBe('55%')
  })

  it('reads the played track channel count from the stream probe', () => {
    expect(probedChannelCount({ convertedIndex: 0, streams: [{ channels: 4 }, { channels: 2 }] })).toBe(4)
    expect(probedChannelCount({ convertedIndex: 1, streams: [{ channels: 4 }, { channels: 2 }] })).toBe(2)
    expect(probedChannelCount(null)).toBeNull()
    expect(probedChannelCount({ streams: [{ channels: 0 }] })).toBeNull()
    expect(probedChannelCount({ streams: [{ channels: 64 }] })).toBe(32)
  })

  it('labels strips by channel, or as the web view mix when it mixed the file down', () => {
    expect(stripLabels({ fileChannels: 3, heardChannels: null })).toEqual(['Ch 1', 'Ch 2', 'Ch 3'])
    expect(stripLabels({ fileChannels: 3, heardChannels: 3 })).toEqual(['Ch 1', 'Ch 2', 'Ch 3'])
    expect(stripLabels({ fileChannels: 8, heardChannels: 2 })).toEqual(['Left mix', 'Right mix'])
    expect(stripLabels({ fileChannels: 8, heardChannels: 6 })).toHaveLength(6)
    expect(stripLabels({ fileChannels: 8, heardChannels: 1 })).toEqual([])
  })

  it('says how many channels the Player can separate', () => {
    expect(separationNote({ fileChannels: 4, heardChannels: 4, probeAvailable: true })).toBe('')
    expect(separationNote({ fileChannels: 4, heardChannels: null, probeAvailable: true })).toMatch(/Play the file/)
    expect(separationNote({ fileChannels: 8, heardChannels: 2, probeAvailable: true })).toBe(
      'This file has 8 channels; the web view mixes them down, so the Player can separate 2 here. Use Split Channels in Convert to hear each channel.',
    )
    expect(separationNote({ fileChannels: 8, heardChannels: 1, probeAvailable: true })).toMatch(/cannot separate/)
    expect(separationNote({ fileChannels: 4, heardChannels: null, probeAvailable: false })).toMatch(/cannot report/)
  })
})

describe('createChannelMixer', () => {
  let fake
  let clock
  beforeEach(() => {
    fake = installFakeAudio()
    clock = 0
  })
  afterEach(() => fake.restore())

  const build = (channels, extra = {}) => {
    const audio = document.createElement('audio')
    const heard = []
    let unavailable = false
    const mixer = createChannelMixer({
      audio,
      channels,
      AudioContextCtor: fake.FakeAudioContext,
      probeUrl: 'probe.js',
      now: () => clock,
      onHeardChannels: c => heard.push(c),
      onProbeUnavailable: () => {
        unavailable = true
      },
      ...extra,
    })
    return { audio, mixer, heard, isUnavailable: () => unavailable, context: fake.contexts.at(-1) }
  }

  it('routes source → splitter → per-channel gain → merger → master → speakers', () => {
    const { mixer, context } = build(4)
    const source = context.nodes.find(n => n.kind === 'source')
    const splitter = context.nodes.find(n => n.kind === 'splitter')
    const merger = context.nodes.find(n => n.kind === 'merger')
    expect(splitter.numberOfOutputs).toBe(4)
    expect(source.connections.map(c => c.target)).toContain(splitter)
    const gains = fake.channelGains(context)
    expect(gains).toHaveLength(4)
    gains.forEach(g => {
      expect(g.connections.filter(c => c.target === merger).map(c => c.input)).toEqual([0, 1])
    })
    expect(merger.connections[0].target).toBe(fake.master(context))
    expect(mixer.layout).toBe(4)
  })

  it('keeps a stereo file left and right', () => {
    const { context } = build(2)
    const merger = context.nodes.find(n => n.kind === 'merger')
    const [left, right] = fake.channelGains(context)
    expect(left.connections.filter(c => c.target === merger).map(c => c.input)).toEqual([0])
    expect(right.connections.filter(c => c.target === merger).map(c => c.input)).toEqual([1])
  })

  it('applies solo, mute and volume to the channel gains and master to the master gain', () => {
    const { mixer, context } = build(3)
    mixer.setStrips([strip(), strip({ soloed: true, volume: 1.5 }), strip({ muted: true })])
    expect(fake.channelGains(context).map(g => g.gain.value)).toEqual([0, 1.5, 0])
    mixer.setMaster(0.3)
    expect(fake.master(context).gain.value).toBe(0.3)
    mixer.setMaster(4)
    expect(fake.master(context).gain.value).toBe(1)
  })

  it('re-routes for the channel count the web view passes and keeps the gains', () => {
    const { mixer, context } = build(8)
    mixer.setStrips([strip({ muted: true }), strip()])
    mixer.setLayout(2)
    expect(mixer.layout).toBe(2)
    const live = fake.channelGains(context)
    expect(live).toHaveLength(2)
    expect(live.map(g => g.gain.value)).toEqual([0, 1])
    // The 8-way splitter is no longer fed.
    const source = context.nodes.find(n => n.kind === 'source')
    const fed = source.connections.filter(c => c.target.kind === 'splitter').map(c => c.target.numberOfOutputs)
    expect(fed).toEqual([2])
  })

  it('trusts channel counts only after playback has settled, and only upward', async () => {
    const { mixer, heard, context } = build(4)
    await mixer.probeReady
    expect(context.audioWorklet.addModule).toHaveBeenCalledWith('probe.js')
    const probe = fake.workletNodes.at(-1)
    expect(probe.options).toMatchObject({
      numberOfOutputs: 0,
      channelCountMode: 'max',
      channelInterpretation: 'discrete',
    })
    probe.port.onmessage({ data: { channels: 2 } })
    expect(heard).toEqual([]) // not playing
    mixer.setPlaying(true)
    probe.port.onmessage({ data: { channels: 2 } })
    expect(heard).toEqual([]) // placeholder window
    clock = CHANNEL_COUNT_SETTLE_MS + 1
    probe.port.onmessage({ data: { channels: 4 } })
    probe.port.onmessage({ data: { channels: 2 } })
    expect(heard).toEqual([4])
    mixer.resetHeard()
    mixer.setPlaying(true)
    clock += CHANNEL_COUNT_SETTLE_MS + 1
    probe.port.onmessage({ data: { channels: 2 } })
    expect(heard).toEqual([4, 2])
  })

  it('reports when the web view has no AudioWorklet to count channels', async () => {
    delete globalThis.AudioWorkletNode
    const { mixer, isUnavailable } = build(4)
    await mixer.probeReady
    expect(isUnavailable()).toBe(true)
  })

  it('resumes a suspended context and closes everything on dispose', async () => {
    const { mixer, context } = build(2)
    await mixer.probeReady
    await mixer.resume()
    expect(context.resume).toHaveBeenCalledTimes(1)
    mixer.dispose()
    mixer.dispose()
    expect(context.close).toHaveBeenCalledTimes(1)
    expect(context.nodes.filter(n => n.kind !== 'destination').every(n => n.connections.length === 0)).toBe(true)
    expect(fake.workletNodes.at(-1).port.close).toHaveBeenCalled()
    // After dispose nothing is touched.
    mixer.setStrips([strip({ muted: true })])
    mixer.setMaster(0)
  })

  it('closes the context when the element cannot be used as a source', () => {
    const audio = document.createElement('audio')
    audio.__hasSource = true
    expect(() =>
      createChannelMixer({ audio, channels: 2, AudioContextCtor: fake.FakeAudioContext, probeUrl: null }),
    ).toThrow(/already has a source/)
    expect(fake.contexts.at(-1).close).toHaveBeenCalled()
  })

  it('starts every channel at 100%', () => {
    expect(initialStrips(2)).toEqual([strip(), strip()])
  })
})
