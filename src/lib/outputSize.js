import { fmtSize } from '../utils'

// WAV outputs are 16-bit PCM (pcm_s16le in the backend's output_args).
const WAV_BYTES_PER_SAMPLE = 2
// Past this size a RIFF header's 32-bit size fields overflow. The backend adds
// `-rf64 auto` to a WAV whose own estimate reaches 3.5 GiB (helpers.rs
// wav_rf64_args), so FFmpeg switches such a file to an RF64 header once it
// passes this size. Smaller WAVs keep the plain RIFF header.
export const RIFF_LIMIT_BYTES = 4 * 1024 ** 3

// Approximate WAV output size for one queued file, from the probed duration
// and channel count of the converted audio stream and the chosen sample rate:
// duration x channels x sample rate x 2 bytes. Headers, and anything trim
// removes, are not counted. Null when the probe gave no duration or channels.
export function estimateWavOutput(info, { mode, rate }) {
  const index = Number.isInteger(info?.convertedIndex) ? info.convertedIndex : 0
  const sourceChannels = info?.streams?.[index]?.channels
  const duration = info?.durationSecs
  const sampleRate = Number(rate)
  if (!(duration > 0) || !(sourceChannels > 0) || !(sampleRate > 0)) return null
  const channelsPerFile = mode === 'stereo' ? 2 : mode === 'split' ? 1 : sourceChannels
  const files = mode === 'split' ? sourceChannels : 1
  const perFileBytes = Math.round(duration * sampleRate * channelsPerFile * WAV_BYTES_PER_SAMPLE)
  return { perFileBytes, totalBytes: perFileBytes * files, files }
}

// The queue-wide estimate shown next to the WAV format choice, or '' when no
// queued file could be measured.
export function wavSizeEstimateText(files, audioStreams, { mode, rate }) {
  const estimates = files.map(file => estimateWavOutput(audioStreams?.[file.path], { mode, rate })).filter(Boolean)
  if (estimates.length === 0) return ''
  const total = estimates.reduce((sum, estimate) => sum + estimate.totalBytes, 0)
  const largest = Math.max(...estimates.map(estimate => estimate.perFileBytes))
  const coverage =
    estimates.length === files.length
      ? files.length === 1
        ? 'for this file'
        : `for these ${files.length} files`
      : `for ${estimates.length} of ${files.length} files; the others could not be measured`
  const rf64 =
    largest > RIFF_LIMIT_BYTES
      ? ' Outputs over 4 GiB are written with an RF64 header, which some older programs cannot open.'
      : ''
  return `Estimated WAV size: about ${fmtSize(total)} ${coverage} (duration × channels × sample rate × 2 bytes).${rf64}`
}
