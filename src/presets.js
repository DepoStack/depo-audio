// ── Export presets ───────────────────────────────────────────────────────────
//
// Each built-in preset is named for where the file is going and carries a
// one-line `basis`: the reason for its format, with the source it comes from,
// and what it does with the sample rate. Compatibility statements come only
// from Stenograph's own pages (`source`; the picker links each article by its
// title from STENOGRAPH_SOURCES). None of these presets has been tested in CAT
// software, and the picker says so.
//
// A preset sets only what the destination needs: the channel mode, the
// format and, for MP3, the bitrate. It never turns processing on or off and
// never changes the sample rate (no destination here needs one; Opus is
// always 48 kHz). Processing is the person's own choice, made below the
// presets, and each preset card says "No processing" or lists what they
// turned on.
//
// Presets are not stored anywhere. The picker compares the current settings
// with each preset; when none matches, it shows "Custom" and changes nothing.

export const STENOGRAPH_ASSOCIATE_AUDIO = 'https://solutioncenter.stenograph.com/associateaudio'
export const STENOGRAPH_SINGLE_CHANNEL = 'https://solutioncenter.stenograph.com/singlechannelaudio'

// The article titles as Stenograph's Solution Center shows them (checked
// 2026-11-21), in the order the picker lists them.
export const STENOGRAPH_SOURCES = [
  { url: STENOGRAPH_ASSOCIATE_AUDIO, title: 'Associate Audio File' },
  { url: STENOGRAPH_SINGLE_CHANNEL, title: 'Single Channel Audio' },
]

export const NOT_TESTED_NOTE = 'Not tested in CAT software'

// Shown wherever Opus output is chosen, by preset or by hand. "Single
// Channel Audio" gives the version for playback; "Associate Audio File"
// lists .opus among the files CATalyst can associate but names no version.
export const OPUS_CATALYST_NOTE =
  'Stenograph says Opus plays only in CATalyst 20 or newer, and lists .opus among the files CATalyst can associate. Older versions cannot play it; send WAV instead.'

// What a preset does with the sample rate, ending its basis line.
export const KEEPS_RATE_NOTE = 'Keeps your sample rate setting.'
export const OPUS_RATE_NOTE = 'Opus is always 48 kHz.'

// Resolve release- and mode-dependent settings before they are compared or
// described. Retired learned-model flags always fail closed.
export function resolvePresetSettings(settings) {
  return {
    ...settings,
    autoLevel: settings.mode === 'keep' ? false : Boolean(settings.autoLevel),
    denoise: false,
    enhance: false,
    dereverb: false,
  }
}

// The processing options, in the order the Convert tab lists them, with the
// one-line reason each one gives: what it does, and what it changes. All are
// off until the person turns them on.
export const PROCESSING_OPTIONS = [
  {
    id: 'autoLevel',
    name: 'Balance Microphone Levels',
    reason: 'Evens out microphone levels so quiet microphones are easier to hear; changes each channel’s level.',
  },
  {
    id: 'declip',
    name: 'Fix Clipped Audio',
    reason: 'Rebuilds peaks that were cut off by recording too loud; changes the waveform at those peaks.',
  },
  {
    id: 'hpf',
    name: 'High-Pass Filter',
    reason: 'Removes low rumble and handling noise; removes all sound below the cutoff.',
  },
  {
    id: 'normalize',
    name: 'Normalize Volume',
    reason: 'Makes quiet recordings louder; changes levels.',
  },
  {
    id: 'trim',
    name: 'Trim Leading Silence',
    reason:
      'Removes silence before the first sound, judged on the recording’s own levels before any level change, so very quiet speech at the start can be removed even with Normalize on; the output starts later than the recording, and the export record says by how many samples.',
  },
  {
    id: 'fade',
    name: 'Fade In / Out',
    reason: 'Smooths the start and end; makes the first and last moments quieter.',
  },
]

// The processing options that will really run with `settings`, by name:
// auto-level does nothing in Keep Original and trim nothing in Split
// Channels.
export function processingInUse(settings) {
  return processingOptionsInUse(settings).map(option => option.name)
}

// The same, as the PROCESSING_OPTIONS entries (id, name and reason).
export function processingOptionsInUse(settings) {
  const s = resolvePresetSettings(settings)
  const effective = {
    autoLevel: s.autoLevel,
    declip: Boolean(s.declip),
    hpf: Boolean(s.hpf),
    normalize: Boolean(s.normalize),
    trim: Boolean(s.trim) && s.mode !== 'split',
    fade: Boolean(s.fade),
  }
  return PROCESSING_OPTIONS.filter(option => effective[option.id])
}

// The line each preset card shows: what processing a conversion with this
// preset would run. It says only what is on, not who turned it on: an
// earlier preset or an older version's defaults may have.
export function processingSummary(settings) {
  const names = processingInUse(settings)
  return names.length ? `Processing on: ${names.join(', ')}` : 'No processing'
}

export const PRESETS = [
  {
    id: 'catalyst-wav',
    name: 'For CATalyst audio sync (WAV)',
    desc: 'One mixed WAV. The audio starts where the recording starts unless you turn on Trim.',
    basis: `Stenograph lists WAV for audio association and says to convert MP3 to WAV for CATalyst 21 or older. ${KEEPS_RATE_NOTE}`,
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: { mode: 'stereo', format: 'wav' },
  },
  {
    id: 'other-cat-wav',
    name: 'For other or older CAT software (WAV)',
    desc: 'One mixed 16-bit WAV.',
    basis: `Plain WAV. We have no vendor source for other CAT programs, so check what yours accepts. ${KEEPS_RATE_NOTE}`,
    source: null,
    settings: { mode: 'stereo', format: 'wav' },
  },
  {
    id: 'scopist-mp3',
    name: 'Send to scopist — small (MP3 192k)',
    desc: 'One mixed MP3 at 192 kbps.',
    basis: `Stenograph says associating MP3 needs CATalyst 22 or newer; for 21 or older, send WAV. ${KEEPS_RATE_NOTE}`,
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: { mode: 'stereo', format: 'mp3', mp3Bitrate: 192 },
  },
  {
    id: 'split-wav',
    name: 'One file per microphone (WAV)',
    desc: 'Splits the microphone channels into separate WAV files, named from the channel labels.',
    basis: `Separate WAV files for listening to one microphone at a time. Stenograph lists WAV for audio association. ${KEEPS_RATE_NOTE}`,
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: { mode: 'split', format: 'wav' },
  },
  {
    id: 'archive-flac',
    name: 'Archive — lossless (FLAC)',
    desc: 'Keeps the original channels in lossless FLAC.',
    basis: `FLAC is not on Stenograph's list of formats CATalyst can associate. Keep it as the archive copy. ${KEEPS_RATE_NOTE}`,
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: { mode: 'keep', format: 'flac' },
    // Its name promises the recording unchanged, so choosing it while
    // processing is on asks first (keep processing, or turn it off).
    asksAboutProcessing: true,
  },
  {
    id: 'small-opus',
    name: 'Smallest file (Opus)',
    desc: 'One mixed Opus file at 64 kbps.',
    basis: `${OPUS_CATALYST_NOTE} ${OPUS_RATE_NOTE}`,
    source: STENOGRAPH_SINGLE_CHANNEL,
    settings: { mode: 'stereo', format: 'opus' },
  },
]

// The settings a preset sets, and nothing else: applying one copies these.
export const PRESET_FIELDS = ['mode', 'format', 'mp3Bitrate']

// The presets shipped in 1.0.x (the latest published release is 1.0.2;
// 1.1.0 was never published and kept the same presets), kept only so the
// picker can tell people whose saved settings are still exactly one of them
// that this is where their processing came from. 1.0.x presets also turned
// on Noise Removal and Enhance Clarity, which are retired and always read as
// off (resolvePresetSettings), so they are left out here. Their settings
// are never applied automatically, and saved settings are never changed to
// match a new preset.
const OFF = {
  denoise: false,
  denoiseQuality: 'fast',
  enhance: false,
  dereverb: false,
  declip: false,
}
export const LEGACY_PRESETS = [
  {
    id: 'deposition',
    name: 'Deposition',
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'mp3',
      rate: '48000',
      normalize: true,
      trim: true,
      fade: true,
      fadeDur: 0.3,
      hpf: true,
      autoLevel: true,
    },
  },
  {
    id: 'phone',
    name: 'Phone Recording',
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'mp3',
      rate: '48000',
      normalize: true,
      trim: true,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      autoLevel: false,
    },
  },
  {
    id: 'courtroom',
    name: 'Courtroom',
    settings: {
      ...OFF,
      mode: 'split',
      format: 'wav',
      rate: '48000',
      normalize: true,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      autoLevel: true,
    },
  },
  {
    id: 'archive',
    name: 'Archive',
    settings: {
      ...OFF,
      mode: 'keep',
      format: 'flac',
      rate: '48000',
      normalize: false,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: false,
      autoLevel: false,
    },
  },
  {
    id: 'quick',
    name: 'Quick Share',
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'opus',
      rate: '48000',
      normalize: true,
      trim: true,
      fade: true,
      fadeDur: 0.3,
      hpf: true,
      autoLevel: false,
    },
  },
]

// True when the current settings give the format, channel mode and bitrate
// the preset sets. Processing and the sample rate are not part of a preset,
// so they are not compared; the MP3 bitrate is compared only for MP3.
export function settingsMatchPreset(current, presetSettings) {
  return (
    current.mode === presetSettings.mode &&
    current.format === presetSettings.format &&
    (presetSettings.format !== 'mp3' || current.mp3Bitrate === (presetSettings.mp3Bitrate ?? 192))
  )
}

// True when the current settings are exactly an earlier (1.0.x) preset,
// processing included. Settings that have no effect are ignored: the fade
// length when fades are off, the sample rate for Opus (always 48 kHz), and
// the MP3 bitrate for other formats.
export function settingsMatchLegacyPreset(current, legacySettings) {
  const s = resolvePresetSettings(legacySettings)
  const c = resolvePresetSettings(current)
  const rate = f => (f.format === 'opus' ? '48000' : String(f.rate))
  return (
    c.mode === s.mode &&
    c.format === s.format &&
    rate(c) === rate(s) &&
    Boolean(c.normalize) === Boolean(s.normalize) &&
    Boolean(c.trim) === Boolean(s.trim) &&
    Boolean(c.fade) === Boolean(s.fade) &&
    (!s.fade || c.fadeDur === s.fadeDur) &&
    Boolean(c.hpf) === Boolean(s.hpf) &&
    c.autoLevel === s.autoLevel &&
    Boolean(c.declip) === Boolean(s.declip) &&
    (s.format !== 'mp3' || c.mp3Bitrate === (s.mp3Bitrate ?? 192))
  )
}

// Describe the current settings for the picker without changing them:
// - `matches`: every built-in preset whose format, mode and bitrate the
//   settings have (several destinations can share one format);
// - `processing`: the names of the processing options that will run;
// - `legacy`: the earlier preset the settings are still exactly (or null),
//   so the picker can say where the processing came from.
// `kind` is 'preset' when a preset matches and no processing is on, and
// 'custom' otherwise: processing is never part of a preset, so settings with
// processing on are custom even when their format is a preset's.
export function describeSettings(current) {
  const matches = PRESETS.filter(p => settingsMatchPreset(current, p.settings))
  const processing = processingInUse(current)
  const legacy = LEGACY_PRESETS.find(p => settingsMatchLegacyPreset(current, p.settings)) || null
  const kind = matches.length > 0 && processing.length === 0 ? 'preset' : 'custom'
  return { kind, preset: matches[0] || null, matches, processing, legacy }
}
