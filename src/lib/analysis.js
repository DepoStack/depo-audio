// Scan reads a bounded sample from the start of each file. Must match
// ANALYSIS_SAMPLE_SECS in src-tauri/src/ffmpeg.rs (a test checks the source).
export const SCAN_SAMPLE_SECS = 180

// "the first 3 minutes" / "the first 90 seconds"
export function scanSampleSpan(secs = SCAN_SAMPLE_SECS) {
  if (secs % 60 === 0) {
    const minutes = secs / 60
    return `the first ${minutes === 1 ? 'minute' : `${minutes} minutes`}`
  }
  return `the first ${secs} seconds`
}

export const SCAN_SAMPLE_NOTE = `Scan checks ${scanSampleSpan()} of each file.`

export function aggregateAnalysisResults(results) {
  if (!results.length) return null

  const speechRatios = results
    .map(result => result.speechRatio)
    .filter(value => typeof value === 'number' && Number.isFinite(value))
  const qualityScores = results
    .map(result => result.qualityScore)
    .filter(score => typeof score?.ovr === 'number' && Number.isFinite(score.ovr))
  const worstQuality = qualityScores.reduce((worst, score) => (!worst || score.ovr < worst.ovr ? score : worst), null)
  const speakerCounts = results
    .map(result => result.speakerCount)
    .filter(value => typeof value === 'number' && Number.isFinite(value))
  const narrowbandRates = results
    .filter(result => result.isNarrowband)
    .map(result => result.sampleRate)
    .filter(value => typeof value === 'number' && Number.isFinite(value))

  return {
    ...results[0],
    needsDenoise: results.some(result => result.needsDenoise),
    needsLeveling: results.some(result => result.needsLeveling),
    hasClipping: results.some(result => result.hasClipping),
    isNarrowband: results.some(result => result.isNarrowband),
    recommendations: [...new Set(results.flatMap(result => result.recommendations || []))],
    qualityScore: worstQuality,
    speakerCount: speakerCounts.length ? Math.max(...speakerCounts) : null,
    speechRatio: speechRatios.length ? speechRatios.reduce((sum, value) => sum + value, 0) / speechRatios.length : null,
    turns: results.flatMap(result => result.turns || []),
    sampleRate: narrowbandRates.length ? Math.min(...narrowbandRates) : results[0].sampleRate,
  }
}
