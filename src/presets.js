// ── Export presets ───────────────────────────────────────────────────────────
//
// Named configurations for common use cases. Each preset sets processing
// options to sensible defaults. Users can apply a preset and then override
// individual settings.

// Resolve release- and mode-dependent settings before a preset is compared or
// applied. Retired learned-model flags always fail closed.
export function resolvePresetSettings(settings) {
  return {
    ...settings,
    autoLevel: settings.mode === 'keep' ? false : Boolean(settings.autoLevel),
    denoise: false,
    enhance: false,
    dereverb: false,
  }
}

export const PRESETS = [
  {
    id: 'deposition',
    name: 'Deposition',
    desc: 'Court deposition — balanced channels and consistent output level',
    settings: {
      mode: 'stereo',
      format: 'mp3',
      rate: '48000',
      normalize: true,
      trim: true,
      fade: true,
      fadeDur: 0.3,
      hpf: true,
      denoise: false,
      denoiseQuality: 'fast',
      autoLevel: true,
      declip: false,
      enhance: false,
      dereverb: false,
    },
  },
  {
    id: 'phone',
    name: 'Phone Recording',
    desc: 'Phone call or dial-in — focused speech-band filtering and consistent output',
    settings: {
      mode: 'stereo',
      format: 'mp3',
      rate: '48000',
      normalize: true,
      trim: true,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      denoise: false,
      denoiseQuality: 'fast',
      autoLevel: false,
      declip: false,
      enhance: false,
      dereverb: false,
    },
  },
  {
    id: 'courtroom',
    name: 'Courtroom',
    desc: 'Large room recording — separate and balance microphone channels',
    settings: {
      mode: 'split',
      format: 'wav',
      rate: '48000',
      normalize: true,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: true,
      denoise: false,
      denoiseQuality: 'fast',
      autoLevel: true,
      declip: false,
      enhance: false,
      dereverb: false,
    },
  },
  {
    id: 'archive',
    name: 'Archive',
    desc: 'Lossless archival — preserve original quality, minimal processing',
    settings: {
      mode: 'keep',
      format: 'flac',
      rate: '48000',
      normalize: false,
      trim: false,
      fade: false,
      fadeDur: 0.5,
      hpf: false,
      denoise: false,
      denoiseQuality: 'fast',
      autoLevel: false,
      declip: false,
      enhance: false,
      dereverb: false,
    },
  },
  {
    id: 'quick',
    name: 'Quick Share',
    desc: 'Small file for email — voice-optimized Opus with level normalization',
    settings: {
      mode: 'stereo',
      format: 'opus',
      rate: '48000',
      normalize: true,
      trim: true,
      fade: true,
      fadeDur: 0.3,
      hpf: true,
      denoise: false,
      denoiseQuality: 'fast',
      autoLevel: false,
      declip: false,
      enhance: false,
      dereverb: false,
    },
  },
]
