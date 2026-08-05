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
