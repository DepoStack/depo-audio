// "Copy diagnostics" (DA-09): the few facts support needs to tell builds
// apart, and nothing else. No file paths, file names, case names, error
// messages or recording data, and nothing is sent anywhere: the person copies
// the text and decides where to paste it.
import { version } from '../../package.json'

// package.json, Cargo.toml and tauri.conf.json carry the same version; the
// release contract fails when they drift.
export const APP_VERSION = version

const SEMVER = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]{1,32})?$/
const PLATFORM_WORD = /^[a-z0-9_]{1,32}$/
// FFmpeg reports "7.1.1", "n7.1.1-20250301" or "N-118000-g1234abcd".
const FFMPEG_VERSION = /^[nN]?-?\d[0-9A-Za-z.+~_-]{0,63}$/
const MEDIA_SUFFIX = /\.(?:wav|mp3|flac|opus|m4a|ogg|trm|ftr|sgmca|dm|aes|dcr|mp4|mov|json|txt|exe|app)$/i
const ERROR_CLASS = /^[A-Za-z_$][A-Za-z0-9_$]{0,63}$/

function allowed(value, pattern) {
  return typeof value === 'string' && pattern.test(value) && !MEDIA_SUFFIX.test(value)
}

function field(value, pattern, missing) {
  if (value === undefined || value === null || value === '') return missing
  return allowed(value, pattern) ? value : 'unknown'
}

// The error's class name only (TypeError, RangeError, ...). Messages can
// quote file names or paths, so they never leave the error object.
export function errorTypeOf(error) {
  const name = error instanceof Error ? error.name : undefined
  return allowed(name, ERROR_CLASS) ? name : 'Error'
}

let engineInfo = null

// App stores the engine check here so the error screen can include the
// FFmpeg version without calling the backend while the interface is broken.
export function rememberEngineInfo(health) {
  engineInfo = health ? { ffmpegVersion: health.ffmpegVersion, os: health.os, arch: health.arch } : null
}

export function rememberedEngineInfo() {
  return engineInfo ?? {}
}

export function buildDiagnostics({ appVersion = APP_VERSION, os, arch, ffmpegVersion, errorType } = {}) {
  return [
    'DepoAudio diagnostics',
    `App version: ${field(appVersion, SEMVER, 'unknown')}`,
    `OS: ${field(os, PLATFORM_WORD, 'unknown')} (${field(arch, PLATFORM_WORD, 'unknown')})`,
    `FFmpeg: ${field(ffmpegVersion, FFMPEG_VERSION, 'not available')}`,
    `Error type: ${field(errorType, ERROR_CLASS, 'none')}`,
  ].join('\n')
}

export async function copyDiagnostics(text) {
  await navigator.clipboard.writeText(text)
}
