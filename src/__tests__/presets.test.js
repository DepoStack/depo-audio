import { describe, expect, it } from 'vitest'
import {
  LEGACY_PRESETS,
  NOT_TESTED_NOTE,
  OPUS_CATALYST_NOTE,
  PRESETS,
  STENOGRAPH_ASSOCIATE_AUDIO,
  STENOGRAPH_SINGLE_CHANNEL,
  STENOGRAPH_SOURCES,
  describeSettings,
  settingsMatchPreset,
} from '../presets'

const byId = id => PRESETS.find(p => p.id === id)
const BANNED = /certified|court-ready|admissible|accurate|\bfinal\b|seamless|AI-powered|guaranteed/i

// Build the settings object the Convert tab passes to describeSettings.
const current = (settings, extra = {}) => ({ mp3Bitrate: 192, ...settings, ...extra })

describe('destination-named presets', () => {
  it('names each preset for where the file is going and states its basis', () => {
    expect(PRESETS.map(p => p.name)).toEqual([
      'For CATalyst audio sync (WAV)',
      'For other or older CAT software (WAV)',
      'Send to scopist — small (MP3 192k)',
      'Phone recording cleanup (WAV + high-pass)',
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
    // Every source the presets cite is linked under the picker by its title.
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
    expect(opus[0].basis).toBe(OPUS_CATALYST_NOTE)
    expect(opus[0].source).toBe(STENOGRAPH_SINGLE_CHANNEL)
    expect(OPUS_CATALYST_NOTE).toMatch(/Opus plays only in CATalyst 20 or newer.*Older versions cannot play it/)
    // The playback version is not stretched to cover audio association.
    expect(OPUS_CATALYST_NOTE).not.toMatch(/associat\w* (?:needs|requires) CATalyst 20/)
  })

  it('describes the phone preset as the high-pass and normalization it really turns on', () => {
    const phone = byId('phone-wav')
    expect(phone.settings).toMatchObject({ format: 'wav', hpf: true, normalize: true, declip: false })
    expect(`${phone.name} ${phone.desc} ${phone.basis}`).not.toMatch(/speech-band|band-pass|lowpass|low-pass/i)
    expect(phone.desc).toMatch(/high-pass filter/)
    expect(phone.desc).toMatch(/No other filtering/)
  })

  it('keeps the start of the audio in place for every preset', () => {
    for (const p of PRESETS) expect(p.settings, p.id).toMatchObject({ trim: false, fade: false })
  })

  it('sets the formats the names promise', () => {
    expect(byId('catalyst-wav').settings).toMatchObject({ format: 'wav', mode: 'stereo' })
    expect(byId('other-cat-wav').settings).toMatchObject({
      format: 'wav',
      normalize: false,
      hpf: false,
      autoLevel: false,
    })
    expect(byId('scopist-mp3').settings).toMatchObject({ format: 'mp3', mp3Bitrate: 192 })
    expect(byId('archive-flac').settings).toMatchObject({ format: 'flac', mode: 'keep', normalize: false, hpf: false })
    expect(byId('split-wav').settings).toMatchObject({ format: 'wav', mode: 'split' })
  })
})

describe('matching the current settings', () => {
  it('matches each preset to its own settings and to no other', () => {
    for (const p of PRESETS) {
      const state = describeSettings(current(p.settings))
      expect(state.kind, p.id).toBe('preset')
      expect(state.preset.id).toBe(p.id)
    }
  })

  it('ignores settings that have no effect on the output', () => {
    const wav = byId('catalyst-wav').settings
    expect(settingsMatchPreset(current(wav, { fadeDur: 2 }), wav)).toBe(true)
    expect(settingsMatchPreset(current(wav, { mp3Bitrate: 320 }), wav)).toBe(true)
    const opus = byId('small-opus').settings
    expect(settingsMatchPreset(current(opus, { rate: '22050' }), opus)).toBe(true)
    const mp3 = byId('scopist-mp3').settings
    expect(settingsMatchPreset(current(mp3, { mp3Bitrate: 320 }), mp3)).toBe(false)
  })

  it('shows Custom when one setting differs', () => {
    const state = describeSettings(current(byId('catalyst-wav').settings, { declip: true }))
    expect(state).toEqual({ kind: 'custom', legacy: null, closest: null })
  })
})

describe('settings saved under the earlier presets', () => {
  it('lands Courtroom and Archive users on the matching new preset unchanged', () => {
    const courtroom = LEGACY_PRESETS.find(p => p.id === 'courtroom')
    const archive = LEGACY_PRESETS.find(p => p.id === 'archive')
    expect(describeSettings(current(courtroom.settings)).preset.id).toBe('split-wav')
    expect(describeSettings(current(archive.settings)).preset.id).toBe('archive-flac')
  })

  it('keeps Deposition, Phone Recording and Quick Share settings as Custom and names the closest new preset', () => {
    const expected = { deposition: 'scopist-mp3', phone: 'phone-wav', quick: 'small-opus' }
    for (const [legacyId, closestId] of Object.entries(expected)) {
      const legacy = LEGACY_PRESETS.find(p => p.id === legacyId)
      const saved = current(legacy.settings)
      const before = structuredClone(saved)
      const state = describeSettings(saved)
      expect(state.kind, legacyId).toBe('custom')
      expect(state.legacy.id).toBe(legacyId)
      expect(state.closest.id).toBe(closestId)
      // Describing never changes the saved settings.
      expect(saved).toEqual(before)
    }
  })

  it('points every earlier preset at a preset that exists', () => {
    for (const legacy of LEGACY_PRESETS) expect(byId(legacy.closest), legacy.id).toBeDefined()
  })
})
