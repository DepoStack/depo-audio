import { describe, expect, it } from 'vitest'
import {
  KEEPS_RATE_NOTE,
  LEGACY_PRESETS,
  NOT_TESTED_NOTE,
  OPUS_CATALYST_NOTE,
  OPUS_RATE_NOTE,
  PRESETS,
  PRESET_FIELDS,
  PROCESSING_OPTIONS,
  STENOGRAPH_ASSOCIATE_AUDIO,
  STENOGRAPH_SINGLE_CHANNEL,
  STENOGRAPH_SOURCES,
  describeSettings,
  processingInUse,
  processingSummary,
  settingsMatchPreset,
} from '../presets'

const byId = id => PRESETS.find(p => p.id === id)
const BANNED = /certified|court-ready|admissible|accurate|\bfinal\b|seamless|AI-powered|guaranteed/i

// The out-of-box settings (usePreferences): everything off.
const DEFAULTS = {
  mode: 'stereo',
  format: 'wav',
  rate: '48000',
  mp3Bitrate: 192,
  normalize: false,
  trim: false,
  fade: false,
  fadeDur: 0.5,
  hpf: false,
  autoLevel: false,
  declip: false,
}
const current = (extra = {}) => ({ ...DEFAULTS, ...extra })
const PROCESSING_KEYS = ['normalize', 'trim', 'fade', 'fadeDur', 'hpf', 'autoLevel', 'declip', 'rate']

describe('destination-named presets', () => {
  it('names each preset for where the file is going and states its basis', () => {
    expect(PRESETS.map(p => p.name)).toEqual([
      'For CATalyst audio sync (WAV)',
      'For other or older CAT software (WAV)',
      'Send to scopist — small (MP3 192k)',
      'One file per microphone (WAV)',
      'Archive — lossless (FLAC)',
      'Smallest file (Opus)',
    ])
    for (const p of PRESETS) {
      expect(p.basis, p.id).toMatch(/\S/)
      expect(p.desc, p.id).toMatch(/\S/)
      expect([STENOGRAPH_ASSOCIATE_AUDIO, STENOGRAPH_SINGLE_CHANNEL, null]).toContain(p.source)
      // No version or destination claim in a name: those live in the basis.
      expect(p.name, p.id).not.toMatch(/\d{2} or (?:newer|older)/)
    }
    const cited = new Set(PRESETS.map(p => p.source).filter(Boolean))
    expect(STENOGRAPH_SOURCES.map(source => source.url).sort()).toEqual([...cited].sort())
    expect(STENOGRAPH_SOURCES.map(source => source.title)).toEqual(['Associate Audio File', 'Single Channel Audio'])
    expect(new Set(PRESETS.map(p => p.id)).size).toBe(PRESETS.length)
  })

  it('only states CATalyst facts from the Stenograph pages, and never a tested claim', () => {
    const copy = PRESETS.map(p => `${p.name} ${p.desc} ${p.basis}`).join(' ')
    expect(copy).not.toMatch(BANNED)
    expect(copy).not.toMatch(/\btested\b|\bverified\b|compatible with|works with/i)
    expect(copy).not.toMatch(/Eclipse|ProCAT|StenoCAT/)
    expect(NOT_TESTED_NOTE).toBe('Not tested in CAT software')
    expect(byId('scopist-mp3').basis).toMatch(/MP3 needs CATalyst 22 or newer/)
    expect(byId('catalyst-wav').basis).toMatch(/convert MP3 to WAV for CATalyst 21 or older/)
    expect(byId('other-cat-wav').source).toBeNull()
    expect(byId('other-cat-wav').basis).toMatch(/no vendor source/)
  })

  it('says older CATalyst cannot play Opus wherever Opus is produced', () => {
    const opus = PRESETS.filter(p => p.settings.format === 'opus')
    expect(opus.map(p => p.id)).toEqual(['small-opus'])
    expect(opus[0].name).toBe('Smallest file (Opus)')
    expect(opus[0].basis).toBe(`${OPUS_CATALYST_NOTE} ${OPUS_RATE_NOTE}`)
    expect(opus[0].source).toBe(STENOGRAPH_SINGLE_CHANNEL)
    expect(OPUS_CATALYST_NOTE).toMatch(/Opus plays only in CATalyst 20 or newer.*Older versions cannot play it/)
    expect(OPUS_CATALYST_NOTE).not.toMatch(/associat\w* (?:needs|requires) CATalyst 20/)
  })

  it('sets only the format, channel mode and MP3 bitrate the destination needs', () => {
    for (const p of PRESETS) {
      expect(
        Object.keys(p.settings).every(key => PRESET_FIELDS.includes(key)),
        p.id,
      ).toBe(true)
      for (const key of PROCESSING_KEYS) expect(p.settings, p.id).not.toHaveProperty(key)
    }
    expect(byId('catalyst-wav').settings).toEqual({ mode: 'stereo', format: 'wav' })
    expect(byId('other-cat-wav').settings).toEqual({ mode: 'stereo', format: 'wav' })
    expect(byId('scopist-mp3').settings).toEqual({ mode: 'stereo', format: 'mp3', mp3Bitrate: 192 })
    expect(byId('split-wav').settings).toEqual({ mode: 'split', format: 'wav' })
    expect(byId('archive-flac').settings).toEqual({ mode: 'keep', format: 'flac' })
    expect(byId('small-opus').settings).toEqual({ mode: 'stereo', format: 'opus' })
  })

  it('says in each basis line what it does with the sample rate', () => {
    for (const p of PRESETS) {
      const rateNote = p.settings.format === 'opus' ? OPUS_RATE_NOTE : KEEPS_RATE_NOTE
      expect(p.basis.endsWith(rateNote), p.id).toBe(true)
    }
  })

  it('claims no processing in any preset copy', () => {
    const copy = PRESETS.map(p => `${p.name} ${p.desc} ${p.basis}`).join(' ')
    expect(copy).not.toMatch(/normaliz|high-pass|balanced|loudness|cleanup|filter/i)
  })
})

describe('processing options', () => {
  it('lists each option once with a one-line reason that says what it changes', () => {
    expect(PROCESSING_OPTIONS.map(option => option.id)).toEqual([
      'autoLevel',
      'declip',
      'hpf',
      'normalize',
      'trim',
      'fade',
    ])
    for (const option of PROCESSING_OPTIONS) {
      expect(option.reason, option.id).toMatch(/^[^\n]+;[^\n]+\.$/)
      expect(option.reason, option.id).not.toMatch(BANNED)
    }
    expect(PROCESSING_OPTIONS.find(option => option.id === 'normalize').reason).toBe(
      'Makes quiet recordings louder; changes levels.',
    )
    expect(PROCESSING_OPTIONS.find(option => option.id === 'trim').reason).toMatch(/export record says/)
  })

  it('says "No processing" for the out-of-box settings and lists what the person turned on', () => {
    expect(processingInUse(DEFAULTS)).toEqual([])
    expect(processingSummary(DEFAULTS)).toBe('No processing')
    expect(processingSummary(current({ normalize: true, hpf: true }))).toBe(
      'Processing on: High-Pass Filter, Normalize Volume',
    )
    // Options that do nothing in this mode are not listed.
    expect(processingInUse(current({ mode: 'keep', autoLevel: true }))).toEqual([])
    expect(processingInUse(current({ mode: 'split', trim: true }))).toEqual([])
    expect(processingInUse(current({ mode: 'stereo', trim: true }))).toEqual(['Trim Leading Silence'])
  })
})

describe('matching the current settings', () => {
  it('matches each preset to its own settings', () => {
    for (const p of PRESETS) {
      const state = describeSettings(current(p.settings))
      expect(state.kind, p.id).toBe('preset')
      expect(
        state.matches.map(match => match.id),
        p.id,
      ).toContain(p.id)
    }
    // The two WAV destinations share a format, and both are named.
    expect(describeSettings(DEFAULTS).matches.map(p => p.id)).toEqual(['catalyst-wav', 'other-cat-wav'])
  })

  it('ignores the sample rate and processing, which no preset sets', () => {
    const wav = byId('catalyst-wav').settings
    expect(settingsMatchPreset(current({ ...wav, rate: '22050' }), wav)).toBe(true)
    expect(settingsMatchPreset(current({ ...wav, normalize: true }), wav)).toBe(true)
    expect(settingsMatchPreset(current({ ...wav, mp3Bitrate: 320 }), wav)).toBe(true)
    const mp3 = byId('scopist-mp3').settings
    expect(settingsMatchPreset(current({ ...mp3, mp3Bitrate: 320 }), mp3)).toBe(false)
  })

  it('shows Custom when processing is on, naming the preset whose format it has', () => {
    const state = describeSettings(current({ declip: true }))
    expect(state.kind).toBe('custom')
    expect(state.matches.map(p => p.id)).toEqual(['catalyst-wav', 'other-cat-wav'])
    expect(state.processing).toEqual(['Fix Clipped Audio'])
    expect(state.legacy).toBeNull()
  })

  it('shows Custom when no preset has the format', () => {
    for (const extra of [{ format: 'm4a' }, { mode: 'keep', format: 'wav' }, { format: 'mp3', mp3Bitrate: 128 }]) {
      const state = describeSettings(current(extra))
      expect(state.kind).toBe('custom')
      expect(state.matches).toEqual([])
    }
  })
})

describe('settings saved by earlier versions', () => {
  // Saved settings are what a person had when they last used the app. The
  // picker describes them; nothing here or in the Convert tab rewrites them.
  it('keeps processing that earlier presets turned on, and says so', () => {
    for (const legacy of LEGACY_PRESETS) {
      const saved = current(legacy.settings)
      const before = structuredClone(saved)
      const state = describeSettings(saved)
      expect(saved, legacy.id).toEqual(before)
      expect(state.legacy?.id, legacy.id).toBe(legacy.id)
      expect(state.processing, legacy.id).toEqual(processingInUse(saved))
      if (state.processing.length > 0) {
        expect(state.kind, legacy.id).toBe('custom')
      }
    }
  })

  it('describes the 1.0.x Deposition settings as custom, with every option still on', () => {
    const deposition = LEGACY_PRESETS.find(p => p.id === 'deposition')
    const state = describeSettings(current(deposition.settings))
    expect(state.kind).toBe('custom')
    expect(state.matches.map(p => p.id)).toEqual(['scopist-mp3'])
    expect(state.processing).toEqual([
      'Balance Microphone Levels',
      'High-Pass Filter',
      'Normalize Volume',
      'Trim Leading Silence',
      'Fade In / Out',
    ])
  })

  it('describes saved settings that match no preset or earlier preset as plain custom', () => {
    const state = describeSettings(current({ format: 'm4a', normalize: true, autoLevel: true }))
    expect(state).toMatchObject({ kind: 'custom', matches: [], legacy: null })
    expect(state.processing).toEqual(['Balance Microphone Levels', 'Normalize Volume'])
  })

  it('keeps the 1.0.x Archive settings, which had no processing, as that format', () => {
    const archive = LEGACY_PRESETS.find(p => p.id === 'archive')
    const state = describeSettings(current(archive.settings))
    expect(state.kind).toBe('preset')
    expect(state.preset.id).toBe('archive-flac')
  })
})
