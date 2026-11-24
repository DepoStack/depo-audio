// ── Export presets ───────────────────────────────────────────────────────────
//
// Each built-in preset is named for where the file is going and carries a
// one-line `basis`: the reason for its format, with the source it comes from.
// Compatibility statements come only from Stenograph's own pages (`source`;
// the picker links each article by its title from STENOGRAPH_SOURCES). None
// of these presets has been tested in CAT software, and the picker says so.
//
// Presets are not stored anywhere. The picker compares the current settings
// with each preset; when nothing matches, it shows "Custom" and changes
// nothing.

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

// Fields of the learned-model processing removed in 1.2.0. Presets never set
// them, stale settings that still carry them have them dropped here, and the
// backend refuses any job that turns one on.
const RETIRED_LEARNED_FIELDS = ['denoise', 'denoiseQuality', 'enhance', 'dereverb']

// Resolve release- and mode-dependent settings before a preset is compared or
// applied.
export function resolvePresetSettings(settings) {
  const resolved = {
    ...settings,
    autoLevel: settings.mode === 'keep' ? false : Boolean(settings.autoLevel),
  }
  for (const field of RETIRED_LEARNED_FIELDS) delete resolved[field]
  return resolved
}

const OFF = {
  declip: false,
}

export const PRESETS = [
  {
    id: 'catalyst-wav',
    name: 'For CATalyst audio sync (WAV)',
    desc: 'One mixed WAV with balanced microphone levels, normalized loudness and the high-pass filter. No trim or fades, so the audio starts where the recording starts.',
    basis: 'Stenograph lists WAV for audio association and says to convert MP3 to WAV for CATalyst 21 or older.',
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: {
      ...OFF,
      mode: 'stereo',
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
    id: 'other-cat-wav',
    name: 'For other or older CAT software (WAV)',
    desc: 'One mixed 16-bit WAV with no processing.',
    basis: 'Plain WAV. We have no vendor source for other CAT programs, so check what yours accepts.',
    source: null,
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'wav',
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
    id: 'scopist-mp3',
    name: 'Send to scopist — small (MP3 192k)',
    desc: 'One mixed MP3 at 192 kbps with balanced microphone levels, normalized loudness and the high-pass filter. No trim or fades.',
    basis: 'Stenograph says associating MP3 needs CATalyst 22 or newer; for 21 or older, send WAV.',
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'mp3',
      rate: '48000',
      mp3Bitrate: 192,
      normalize: true,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      autoLevel: true,
    },
  },
  {
    id: 'phone-wav',
    name: 'Phone recording cleanup (WAV + high-pass)',
    desc: 'Turns on the high-pass filter (80 Hz unless changed in Settings) and loudness normalization. No other filtering.',
    basis: 'Stenograph lists WAV for audio association. The high-pass only removes sound below its cutoff.',
    source: STENOGRAPH_ASSOCIATE_AUDIO,
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'wav',
      rate: '48000',
      normalize: true,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      autoLevel: false,
    },
  },
  {
    id: 'split-wav',
    name: 'One file per microphone (WAV)',
    desc: 'Splits the microphone channels into separate WAV files, named from the channel labels, with balanced levels and the high-pass filter.',
    basis: 'Separate WAV files for listening to one microphone at a time. Stenograph lists WAV for audio association.',
    source: STENOGRAPH_ASSOCIATE_AUDIO,
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
    id: 'archive-flac',
    name: 'Archive — lossless (FLAC)',
    desc: 'Keeps the original channels in lossless FLAC with no processing.',
    basis: "FLAC is not on Stenograph's list of formats CATalyst can associate. Keep it as the archive copy.",
    source: STENOGRAPH_ASSOCIATE_AUDIO,
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
    id: 'small-opus',
    name: 'Smallest file (Opus)',
    desc: 'One mixed Opus file at 64 kbps with normalized loudness and the high-pass filter. No trim or fades.',
    basis: OPUS_CATALYST_NOTE,
    source: STENOGRAPH_SINGLE_CHANNEL,
    settings: {
      ...OFF,
      mode: 'stereo',
      format: 'opus',
      rate: '48000',
      normalize: true,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      autoLevel: false,
    },
  },
]

// The presets shipped up to 1.0.2 (1.1.0 was never published), kept only so the picker can tell people
// whose saved settings still match one of them which new preset is closest.
// Their settings are never applied automatically.
export const LEGACY_PRESETS = [
  {
    id: 'deposition',
    name: 'Deposition',
    closest: 'scopist-mp3',
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
    closest: 'phone-wav',
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
    closest: 'split-wav',
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
    closest: 'archive-flac',
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
    closest: 'small-opus',
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

// True when the current settings produce the same output as the preset.
// Settings that have no effect are ignored: the fade length when fades are
// off, the sample rate for Opus (always 48 kHz), and the MP3 bitrate for
// other formats.
export function settingsMatchPreset(current, presetSettings) {
  const s = resolvePresetSettings(presetSettings)
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
// - { kind: 'preset', preset } when a built-in preset matches;
// - { kind: 'custom', legacy, closest } otherwise, where `legacy` is the
//   retired preset the settings still match (or null) and `closest` is the
//   new preset that replaced it (or null).
export function describeSettings(current) {
  const preset = PRESETS.find(p => settingsMatchPreset(current, p.settings))
  if (preset) return { kind: 'preset', preset }
  const legacy = LEGACY_PRESETS.find(p => settingsMatchPreset(current, p.settings)) || null
  const closest = legacy ? PRESETS.find(p => p.id === legacy.closest) || null : null
  return { kind: 'custom', legacy, closest }
}
