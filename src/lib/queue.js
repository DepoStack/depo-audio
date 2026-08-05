import { sortRecordingChunks } from '../utils'

export function partitionQueuePaths(inputs = []) {
  const uniquePaths = [
    ...new Set(
      inputs.map(input => (typeof input === 'string' ? input : input?.path)).filter(path => typeof path === 'string'),
    ),
  ]

  return {
    queuePaths: uniquePaths.filter(path => !path.toLowerCase().endsWith('.trs')),
    trsCompanions: uniquePaths.filter(path => path.toLowerCase().endsWith('.trs')),
  }
}

export function mergeQueuedFiles(current, incoming, { replace = false } = {}) {
  const combined = replace ? [] : [...current]
  const seen = new Set(combined.map(file => file.path))

  for (const file of incoming) {
    if (!file?.path || seen.has(file.path)) continue
    seen.add(file.path)
    combined.push(file)
  }

  return sortRecordingChunks(combined, file => file.path)
}

export function catJobPaths(job) {
  return (job?.files || []).map(file => file?.path).filter(Boolean)
}
