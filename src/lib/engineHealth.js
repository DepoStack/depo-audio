// The bundled audio engine check (health_check), read for people. The window
// shows nothing while the engine is healthy; Convert and Player show a warning
// only when something is wrong, and Settings > About always has the details.

// One problem at most: the most serious one wins, since each one below it is
// a consequence (no FFmpeg means no FTR decoder either).
export function engineProblem(health, checkError) {
  if (checkError) {
    return {
      id: 'check-failed',
      title: 'The audio engine could not be checked.',
      action:
        'Conversions may fail until it is. Re-check now; if this keeps happening, reinstall DepoAudio from depostack.com or GitHub Releases.',
      detail: checkError,
    }
  }
  if (!health) return null
  if (!health.ffmpeg) {
    return {
      id: 'ffmpeg-missing',
      title: 'FFmpeg is missing or did not start.',
      action:
        'Conversion and playback of most recordings may fail. Reinstall DepoAudio to restore the bundled FFmpeg, then re-check.',
    }
  }
  if (health.ffprobe === false) {
    return {
      id: 'ffprobe-missing',
      title: 'FFprobe is missing or did not start.',
      action:
        'Recordings cannot be inspected before conversion. Reinstall DepoAudio to restore the bundled FFprobe, then re-check.',
    }
  }
  if (health.ftrDecoder === false) {
    return {
      id: 'ftr-decoder-missing',
      title: 'The FTR decoder check failed.',
      action:
        'For The Record (.ftr / .trm) recordings may not convert or play; other formats should not be affected. Reinstall DepoAudio, then re-check.',
    }
  }
  return null
}

// Settings > About: one row per check.
export function engineChecks(health, checkError) {
  const unknown = checkError ? 'Not checked' : 'Checking…'
  const state = value => (health ? (value ? 'ok' : 'failed') : 'unknown')
  const ffmpegOk = !!health?.ffmpeg
  return [
    {
      id: 'ffmpeg',
      label: 'FFmpeg',
      state: state(ffmpegOk),
      value: health ? (ffmpegOk ? (health.ffmpegVersion ?? 'Ready (version not reported)') : 'Missing') : unknown,
    },
    {
      id: 'ffprobe',
      label: 'FFprobe',
      state: state(health?.ffprobe !== false),
      value: health ? (health.ffprobe !== false ? 'Ready' : 'Missing') : unknown,
    },
    {
      id: 'ftr',
      label: 'FTR decoder',
      state: state(!!health?.ftrDecoder),
      value: health ? (health.ftrDecoder ? 'Available' : ffmpegOk ? 'Check failed' : 'Needs FFmpeg') : unknown,
    },
  ]
}
