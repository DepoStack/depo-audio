//! The arithmetic of a merge, with no Tauri, FFmpeg or file system in it.
//!
//! `merge.rs` decodes each source to a temporary 48 kHz mono WAV and hands
//! this module one sequential reader per source. The engine walks the aligned
//! timeline block by block, so working memory depends on the block size and
//! the number of sources, never on how long the recordings are.
//!
//! Both strategies produce exactly the samples the earlier in-memory code
//! produced; the tests keep that code as the oracle.

use std::ops::Range;

/// Each source may run for at most this long. The Merge screen says so and
/// the job refuses a longer source before decoding it.
pub(crate) const MAX_MERGE_DURATION_SECS: u32 = 30 * 60;
/// Sync detection searches this far either side of the reference.
pub(crate) const SYNC_SEARCH_SECS: usize = 60;
pub(crate) const SYNC_WINDOW_SECS: usize = 10;
/// Samples per block for Mix all. Loudest source works in 500 ms segments.
const MIX_BLOCK_SAMPLES: usize = 48_000;

/// Reads consecutive mono samples from one decoded source.
pub(crate) trait SampleReader {
    /// Fill `out` completely with the next samples, or fail.
    fn read_exact_into(&mut self, out: &mut [f32]) -> Result<(), String>;
}

/// One source placed on the merged timeline: silent before `offset`, then
/// `len` samples from its reader, then silent again.
pub(crate) struct AlignedSource<R> {
    reader: R,
    offset: u64,
    len: u64,
    next_pos: u64,
}

impl<R: SampleReader> AlignedSource<R> {
    pub(crate) fn new(reader: R, offset: u64, len: u64) -> Self {
        Self {
            reader,
            offset,
            len,
            next_pos: 0,
        }
    }

    pub(crate) fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }

    /// Fill `out` with timeline samples `pos..pos + out.len()`. Samples the
    /// source does not cover are zero. Returns the covered range within `out`.
    /// Blocks must be requested in order, with no gaps.
    fn read_block(&mut self, pos: u64, out: &mut [f32]) -> Result<Range<usize>, String> {
        if pos != self.next_pos {
            return Err("Merge engine read a source out of order".into());
        }
        let block_end = pos + out.len() as u64;
        self.next_pos = block_end;
        let start = pos.max(self.offset);
        let end = block_end.min(self.end());
        out.fill(0.0);
        if start >= end {
            return Ok(0..0);
        }
        let range = (start - pos) as usize..(end - pos) as usize;
        self.reader.read_exact_into(&mut out[range.clone()])?;
        Ok(range)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeStrategy {
    /// Average every source that covers each sample.
    MixAll,
    /// Take the loudest source for each 500 ms segment, with a 50 ms
    /// crossfade where the choice changes.
    Loudest,
}

/// Length of the merged timeline: where the last source ends.
pub(crate) fn timeline_len<R: SampleReader>(sources: &[AlignedSource<R>]) -> u64 {
    sources.iter().map(AlignedSource::end).max().unwrap_or(0)
}

/// The longest timeline the job will write: a full-length source shifted by
/// the widest sync offset, plus a second of slack.
pub(crate) fn max_timeline_samples(sample_rate: u32) -> u64 {
    (MAX_MERGE_DURATION_SECS as u64 + SYNC_SEARCH_SECS as u64 + 1) * sample_rate as u64
}

/// Refuse a source whose probed length is unknown or over the limit, naming
/// it so the person knows which recording to trim.
pub(crate) fn check_source_duration(name: &str, duration_secs: Option<f64>) -> Result<(), String> {
    let Some(duration) = duration_secs.filter(|d| d.is_finite() && *d > 0.0) else {
        return Err(format!("Can't read how long \"{name}\" is, so it can't be merged"));
    };
    if duration > MAX_MERGE_DURATION_SECS as f64 {
        return Err(format!(
            "\"{name}\" is longer than {} minutes. Each merge source is limited to {} minutes.",
            MAX_MERGE_DURATION_SECS / 60,
            MAX_MERGE_DURATION_SECS / 60
        ));
    }
    Ok(())
}

/// Merge the aligned sources into `sink`, block by block. `tick` is called
/// before each block with (samples done, total samples); returning an error
/// from it (a cancel) stops the merge. Returns the number of samples written.
pub(crate) fn merge_stream<R: SampleReader>(
    strategy: MergeStrategy,
    sources: &mut [AlignedSource<R>],
    sample_rate: usize,
    sink: &mut dyn FnMut(&[f32]) -> Result<(), String>,
    tick: &mut dyn FnMut(u64, u64) -> Result<(), String>,
) -> Result<u64, String> {
    match strategy {
        MergeStrategy::MixAll => mix_all_stream(sources, MIX_BLOCK_SAMPLES, sink, tick),
        MergeStrategy::Loudest => loudest_stream(sources, sample_rate, sink, tick),
    }
}

fn mix_all_stream<R: SampleReader>(
    sources: &mut [AlignedSource<R>],
    block: usize,
    sink: &mut dyn FnMut(&[f32]) -> Result<(), String>,
    tick: &mut dyn FnMut(u64, u64) -> Result<(), String>,
) -> Result<u64, String> {
    let total = timeline_len(sources);
    let block = block.max(1);
    let mut acc = vec![0.0f32; block];
    let mut counts = vec![0u8; block];
    let mut scratch = vec![0.0f32; block];
    let mut pos = 0u64;
    while pos < total {
        tick(pos, total)?;
        let n = (total - pos).min(block as u64) as usize;
        acc[..n].fill(0.0);
        counts[..n].fill(0);
        for source in sources.iter_mut() {
            let range = source.read_block(pos, &mut scratch[..n])?;
            for k in range {
                acc[k] += scratch[k];
                counts[k] += 1;
            }
        }
        for k in 0..n {
            if counts[k] > 1 {
                acc[k] /= counts[k] as f32;
            }
        }
        sink(&acc[..n])?;
        pos += n as u64;
    }
    tick(total, total)?;
    Ok(total)
}

fn loudest_stream<R: SampleReader>(
    sources: &mut [AlignedSource<R>],
    sample_rate: usize,
    sink: &mut dyn FnMut(&[f32]) -> Result<(), String>,
    tick: &mut dyn FnMut(u64, u64) -> Result<(), String>,
) -> Result<u64, String> {
    let total = timeline_len(sources);
    let segment = (sample_rate / 2).max(1);
    let half_fade = (sample_rate / 20) / 2;
    // A fade reaches back into the previous segment only, which holds while
    // a segment is at least half a fade long.
    debug_assert!(segment >= half_fade);

    let count = sources.len();
    let mut prev_blocks = vec![vec![0.0f32; segment]; count];
    let mut cur_blocks = vec![vec![0.0f32; segment]; count];
    let mut prev_out: Vec<f32> = Vec::with_capacity(segment);
    let mut cur_out: Vec<f32> = Vec::with_capacity(segment);
    let mut prev_pos = 0u64;
    let mut prev_best: Option<usize> = None;

    let mut pos = 0u64;
    while pos < total {
        tick(pos, total)?;
        let n = (total - pos).min(segment as u64) as usize;

        let mut best = 0usize;
        let mut best_rms = 0.0f64;
        for (idx, source) in sources.iter_mut().enumerate() {
            let range = source.read_block(pos, &mut cur_blocks[idx][..n])?;
            let covered = range.len();
            let mut sum = 0.0f64;
            for &s in &cur_blocks[idx][range] {
                let s = s as f64;
                sum += s * s;
            }
            let rms = if covered > 0 {
                (sum / covered as f64).sqrt()
            } else {
                0.0
            };
            if rms > best_rms {
                best_rms = rms;
                best = idx;
            }
        }

        cur_out.clear();
        cur_out.extend_from_slice(&cur_blocks[best][..n]);

        if let Some(prev) = prev_best {
            if prev != best && pos > 0 {
                let fade_start = pos.saturating_sub(half_fade as u64);
                let fade_end = (pos + half_fade as u64).min(total);
                let fade_len = fade_end - fade_start;
                for j in 0..fade_len {
                    let t = j as f32 / fade_len as f32;
                    let at = fade_start + j;
                    if at < pos {
                        let k = (at - prev_pos) as usize;
                        let prev_sample = prev_blocks[prev][k];
                        let new_sample = prev_blocks[best][k];
                        prev_out[k] = prev_sample * (1.0 - t) + new_sample * t;
                    } else {
                        let k = (at - pos) as usize;
                        let prev_sample = cur_blocks[prev][k];
                        let new_sample = cur_blocks[best][k];
                        cur_out[k] = prev_sample * (1.0 - t) + new_sample * t;
                    }
                }
            }
        }

        if prev_best.is_some() {
            sink(&prev_out)?;
        }
        std::mem::swap(&mut prev_out, &mut cur_out);
        std::mem::swap(&mut prev_blocks, &mut cur_blocks);
        prev_pos = pos;
        prev_best = Some(best);
        pos += n as u64;
    }
    if prev_best.is_some() {
        sink(&prev_out)?;
    }
    tick(total, total)?;
    Ok(total)
}

// ── Sync detection ──────────────────────────────────────────────────────────

/// How far a second recording is from the reference, by cross-correlation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SyncEstimate {
    /// Positive means source B starts later than source A.
    pub(crate) offset_seconds: f64,
    pub(crate) confidence: f64,
    pub(crate) is_same_event: bool,
}

pub(crate) fn detect_sync_samples(
    samples_a: &[f32],
    samples_b: &[f32],
    sample_rate: usize,
) -> Result<SyncEstimate, String> {
    if samples_a.is_empty() || samples_b.is_empty() {
        return Err("One or both files are empty".into());
    }
    // Look for each recording's opening inside the other, so either one may
    // be the one that started late, by up to the search range.
    let forward = align_opening(samples_a, samples_b, sample_rate);
    let backward = align_opening(samples_b, samples_a, sample_rate);
    let (found, b_starts_later) = match (forward, backward) {
        (Some(f), Some(b)) if b.score > f.score => (b, true),
        (Some(f), _) => (f, false),
        (None, Some(b)) => (b, true),
        (None, None) => {
            return Ok(SyncEstimate {
                offset_seconds: 0.0,
                confidence: 0.0,
                is_same_event: false,
            })
        }
    };
    // align_opening matches opening[i] against other[i + offset]. A's
    // opening found `offset` samples into B means B started earlier, so the
    // sign flips to keep the documented meaning: positive = B starts later.
    let samples = if b_starts_later { found.offset } else { -found.offset };
    let offset_seconds = samples as f64 / sample_rate as f64;

    // Same-event threshold. Confidence is a true normalized cross-correlation
    // of raw waveforms from different microphones, which sits well below 1
    // even for genuine matches; 0.15 separates matched recordings (~0.2+)
    // from unrelated audio (~0.05).
    let is_same_event = found.confidence > 0.15;
    Ok(SyncEstimate {
        offset_seconds,
        confidence: found.confidence,
        is_same_event,
    })
}

struct Alignment {
    /// `opening[i]` lines up with `other[i + offset]`.
    offset: i64,
    /// Envelope correlation, used to pick between the two directions.
    score: f64,
    /// Normalized waveform correlation at `offset`.
    confidence: f64,
}

/// Find the first `SYNC_WINDOW_SECS` of `recording` inside `other`, within
/// `SYNC_SEARCH_SECS` either way.
fn align_opening(recording: &[f32], other: &[f32], sample_rate: usize) -> Option<Alignment> {
    let search_len = (SYNC_WINDOW_SECS * sample_rate).min(recording.len()).min(other.len());
    let opening = &recording[..search_len];
    let max_offset = SYNC_SEARCH_SECS * sample_rate;

    // Coarse search on loudness envelopes (10 ms frames). Raw waveforms from
    // two microphones stop correlating a few milliseconds away from the true
    // lag, so a coarse grid on the waveform itself misses any offset that
    // does not fall on the grid. The envelope keeps its shape across
    // microphones and correlates over tens of milliseconds.
    let frame = (sample_rate / 100).max(1);
    let env_open = envelope(opening, frame);
    let env_other = envelope(other, frame);
    let max_lag = (max_offset.min(other.len()) / frame) as i64;
    let mut best: Option<(i64, f64)> = None;
    for lag in -max_lag..=max_lag {
        if let Some(score) = envelope_score(&env_open, &env_other, lag) {
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((lag, score));
            }
        }
    }
    let (coarse_lag, score) = best?;

    // Resolve the lag at full sample precision within two frames of the
    // envelope's pick.
    let coarse_offset = coarse_lag * frame as i64;
    let reach = 2 * frame as i64;
    let mut best_corr = 0.0f64;
    let mut offset = coarse_offset;
    for candidate in coarse_offset - reach..=coarse_offset + reach {
        let corr = cross_correlate(opening, other, candidate, search_len);
        if corr > best_corr {
            best_corr = corr;
            offset = candidate;
        }
    }

    // Both energies are measured over the same overlapping window as the
    // correlation, so a large offset (small overlap) is not penalized.
    let mut energy_open = 0.0f64;
    let mut energy_other = 0.0f64;
    for (i, &v) in opening.iter().enumerate() {
        let j = i as i64 + offset;
        if j >= 0 && (j as usize) < other.len() {
            energy_open += v as f64 * v as f64;
            let w = other[j as usize] as f64;
            energy_other += w * w;
        }
    }
    let confidence = if energy_open > 0.0 && energy_other > 0.0 {
        (best_corr / (energy_open.sqrt() * energy_other.sqrt())).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some(Alignment {
        offset,
        score,
        confidence,
    })
}

/// Cross-correlate segment_a with samples_b at the given offset
/// (a[i] against b[i + offset]). The magnitude is used, so a microphone
/// wired with inverted polarity still matches.
pub(crate) fn cross_correlate(segment_a: &[f32], samples_b: &[f32], offset: i64, len: usize) -> f64 {
    let mut sum = 0.0f64;
    let len = len.min(segment_a.len());
    for (i, &a) in segment_a[..len].iter().enumerate() {
        let b_idx = i as i64 + offset;
        if b_idx >= 0 && (b_idx as usize) < samples_b.len() {
            sum += a as f64 * samples_b[b_idx as usize] as f64;
        }
    }
    sum.abs()
}

/// Mean absolute level per frame, with the overall mean removed so a long
/// overlap does not win just by being long.
fn envelope(samples: &[f32], frame: usize) -> Vec<f64> {
    let mut env: Vec<f64> = samples
        .chunks_exact(frame)
        .map(|chunk| chunk.iter().map(|s| s.abs() as f64).sum::<f64>() / frame as f64)
        .collect();
    if !env.is_empty() {
        let mean = env.iter().sum::<f64>() / env.len() as f64;
        env.iter_mut().for_each(|v| *v -= mean);
    }
    env
}

/// Normalized correlation of envelope A against envelope B shifted by `lag`
/// frames (a[i] against b[i + lag]). None when they barely overlap.
fn envelope_score(env_a: &[f64], env_b: &[f64], lag: i64) -> Option<f64> {
    let mut dot = 0.0f64;
    let mut energy_a = 0.0f64;
    let mut energy_b = 0.0f64;
    let mut overlap = 0usize;
    for (i, &a) in env_a.iter().enumerate() {
        let j = i as i64 + lag;
        if j < 0 || j as usize >= env_b.len() {
            continue;
        }
        let b = env_b[j as usize];
        dot += a * b;
        energy_a += a * a;
        energy_b += b * b;
        overlap += 1;
    }
    // At least half the reference window must overlap to count.
    if overlap * 2 < env_a.len() || energy_a <= 0.0 || energy_b <= 0.0 {
        return None;
    }
    Some(dot / (energy_a.sqrt() * energy_b.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader over an in-memory slice.
    struct SliceReader<'a> {
        samples: &'a [f32],
        at: usize,
    }

    impl SampleReader for SliceReader<'_> {
        fn read_exact_into(&mut self, out: &mut [f32]) -> Result<(), String> {
            let end = self.at + out.len();
            let slice = self.samples.get(self.at..end).ok_or("read past the end")?;
            out.copy_from_slice(slice);
            self.at = end;
            Ok(())
        }
    }

    /// A reader that makes up its samples, so a long source holds no buffer.
    struct NoiseReader {
        state: u32,
        gain: f32,
    }

    impl SampleReader for NoiseReader {
        fn read_exact_into(&mut self, out: &mut [f32]) -> Result<(), String> {
            for s in out {
                self.state ^= self.state << 13;
                self.state ^= self.state >> 17;
                self.state ^= self.state << 5;
                *s = ((self.state >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * self.gain;
            }
            Ok(())
        }
    }

    fn noise(len: usize, seed: u32, gain: f32) -> Vec<f32> {
        let mut out = vec![0.0; len];
        NoiseReader {
            state: seed.max(1),
            gain,
        }
        .read_exact_into(&mut out)
        .unwrap();
        out
    }

    fn aligned<'a>(sources: &'a [Vec<f32>], offsets: &[i64]) -> Vec<AlignedSource<SliceReader<'a>>> {
        sources
            .iter()
            .zip(offsets)
            .map(|(s, &o)| AlignedSource::new(SliceReader { samples: s, at: 0 }, o as u64, s.len() as u64))
            .collect()
    }

    fn run(
        strategy: MergeStrategy,
        sources: &[Vec<f32>],
        offsets: &[i64],
        rate: usize,
        mix_block: Option<usize>,
    ) -> Vec<f32> {
        let mut readers = aligned(sources, offsets);
        let mut out = Vec::new();
        let mut sink = |block: &[f32]| {
            out.extend_from_slice(block);
            Ok(())
        };
        let mut tick = |_: u64, _: u64| Ok(());
        let written = match (strategy, mix_block) {
            (MergeStrategy::MixAll, Some(block)) => mix_all_stream(&mut readers, block, &mut sink, &mut tick),
            _ => merge_stream(strategy, &mut readers, rate, &mut sink, &mut tick),
        }
        .unwrap();
        assert_eq!(written as usize, out.len());
        out
    }

    fn total_of(sources: &[Vec<f32>], offsets: &[i64]) -> usize {
        sources
            .iter()
            .zip(offsets)
            .map(|(s, &o)| s.len() + o as usize)
            .max()
            .unwrap()
    }

    // ── The earlier in-memory strategies, kept as the oracle ───────────────

    fn reference_mix_all(sources: &[Vec<f32>], offsets: &[i64], total_samples: usize) -> Vec<f32> {
        let mut output = vec![0.0f32; total_samples];
        let mut counts = vec![0u8; total_samples];
        for (src_idx, samples) in sources.iter().enumerate() {
            let offset = offsets[src_idx];
            for (i, &s) in samples.iter().enumerate() {
                let out_idx = i as i64 + offset;
                if out_idx >= 0 && (out_idx as usize) < total_samples {
                    output[out_idx as usize] += s;
                    counts[out_idx as usize] += 1;
                }
            }
        }
        for i in 0..total_samples {
            if counts[i] > 1 {
                output[i] /= counts[i] as f32;
            }
        }
        output
    }

    fn reference_best_quality(
        sources: &[Vec<f32>],
        offsets: &[i64],
        total_samples: usize,
        sample_rate: usize,
    ) -> Vec<f32> {
        let mut output = vec![0.0f32; total_samples];
        let segment_size = sample_rate / 2;
        let crossfade_len = sample_rate / 20;
        let mut pos = 0usize;
        let mut prev_best: Option<usize> = None;
        while pos < total_samples {
            let end = (pos + segment_size).min(total_samples);
            let mut best_src = 0;
            let mut best_rms = 0.0f64;
            for (src_idx, samples) in sources.iter().enumerate() {
                let offset = offsets[src_idx];
                let mut rms_sum = 0.0f64;
                let mut count = 0usize;
                for out_idx in pos..end {
                    let i = out_idx as i64 - offset;
                    if i >= 0 && (i as usize) < samples.len() {
                        let s = samples[i as usize] as f64;
                        rms_sum += s * s;
                        count += 1;
                    }
                }
                let rms = if count > 0 {
                    (rms_sum / count as f64).sqrt()
                } else {
                    0.0
                };
                if rms > best_rms {
                    best_rms = rms;
                    best_src = src_idx;
                }
            }
            let offset = offsets[best_src];
            for (j, out) in output[pos..end].iter_mut().enumerate() {
                let i = (pos + j) as i64 - offset;
                if i >= 0 && (i as usize) < sources[best_src].len() {
                    *out = sources[best_src][i as usize];
                }
            }
            if let Some(prev) = prev_best {
                if prev != best_src && pos > 0 {
                    let fade_start = pos.saturating_sub(crossfade_len / 2);
                    let fade_end = (pos + crossfade_len / 2).min(total_samples);
                    let fade_len = fade_end - fade_start;
                    for j in 0..fade_len {
                        let t = j as f32 / fade_len as f32;
                        let at = |src: usize| {
                            let i = (fade_start + j) as i64 - offsets[src];
                            if i >= 0 && (i as usize) < sources[src].len() {
                                sources[src][i as usize]
                            } else {
                                0.0
                            }
                        };
                        output[fade_start + j] = at(prev) * (1.0 - t) + at(best_src) * t;
                    }
                }
            }
            prev_best = Some(best_src);
            pos += segment_size;
        }
        output
    }

    /// Sources whose loudness swaps every few segments, so Loudest switches
    /// and crossfades often.
    fn alternating_sources(rate: usize, segments: usize) -> Vec<Vec<f32>> {
        let segment = rate / 2;
        (0..3)
            .map(|src| {
                let mut s = noise(segment * segments + 37 * src, 11 + src as u32, 0.2);
                for (seg, chunk) in s.chunks_mut(segment).enumerate() {
                    if seg % 3 == src {
                        chunk.iter_mut().for_each(|v| *v *= 4.0);
                    }
                }
                s
            })
            .collect()
    }

    fn assert_same(label: &str, got: &[f32], want: &[f32]) {
        assert_eq!(got.len(), want.len(), "{label}: length");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!(
                g.to_bits() == w.to_bits(),
                "{label}: sample {i} is {g}, the in-memory code gave {w}"
            );
        }
    }

    #[test]
    fn streaming_mix_all_matches_the_in_memory_mix_for_any_block_size() {
        let sources = vec![
            noise(5_003, 1, 0.5),
            noise(4_200, 2, 0.5),
            noise(6_100, 3, 0.5),
            noise(17, 4, 0.5),
        ];
        for offsets in [vec![0, 0, 0, 0], vec![0, 311, 1_700, 5_000], vec![900, 0, 12, 6_090]] {
            let want = reference_mix_all(&sources, &offsets, total_of(&sources, &offsets));
            for block in [1, 7, 512, 4_096, 48_000] {
                let got = run(MergeStrategy::MixAll, &sources, &offsets, 48_000, Some(block));
                assert_same(&format!("mix offsets {offsets:?} block {block}"), &got, &want);
            }
        }
    }

    #[test]
    fn streaming_mix_all_is_the_average_of_the_sources_present() {
        let a = vec![1.0f32; 10];
        let b = vec![3.0f32; 10];
        let got = run(MergeStrategy::MixAll, &[a, b], &[0, 5], 48_000, Some(4));
        let want: Vec<f32> = (0..15)
            .map(|i| {
                if i < 5 {
                    1.0
                } else if i < 10 {
                    2.0
                } else {
                    3.0
                }
            })
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn streaming_loudest_matches_the_in_memory_pick_crossfades_included() {
        for rate in [800, 8_000, 48_000] {
            let sources = alternating_sources(rate, 9);
            for offsets in [vec![0, 0, 0], vec![0, rate as i64 / 3, 7], vec![rate as i64 * 2, 0, 1]] {
                let total = total_of(&sources, &offsets);
                let want = reference_best_quality(&sources, &offsets, total, rate);
                let got = run(MergeStrategy::Loudest, &sources, &offsets, rate, None);
                assert_same(&format!("loudest rate {rate} offsets {offsets:?}"), &got, &want);
            }
        }
    }

    #[test]
    fn loudest_switches_sources_with_a_crossfade() {
        let rate = 800;
        let sources = alternating_sources(rate, 6);
        let got = run(MergeStrategy::Loudest, &sources, &[0, 0, 0], rate, None);
        let segment = rate / 2;
        // Inside segment 1 the output is source 1's loud stretch, untouched.
        assert_eq!(got[segment + 100], sources[1][segment + 100]);
        // At the boundary it is neither source alone.
        let edge = segment;
        assert_ne!(got[edge], sources[0][edge]);
        assert_ne!(got[edge], sources[1][edge]);
    }

    #[test]
    fn long_sources_stream_with_memory_bounded_by_the_block() {
        // Two full 30-minute sources at 48 kHz, made up as they are read, so
        // the test itself holds no recording. The sink sees only block-sized
        // pieces and nothing timeline-sized is ever allocated.
        let rate = 48_000usize;
        let len = MAX_MERGE_DURATION_SECS as u64 * rate as u64;
        for strategy in [MergeStrategy::MixAll, MergeStrategy::Loudest] {
            let mut sources = vec![
                AlignedSource::new(NoiseReader { state: 5, gain: 0.3 }, 0, len),
                AlignedSource::new(NoiseReader { state: 9, gain: 0.6 }, rate as u64 * 45, len),
            ];
            let total = timeline_len(&sources);
            assert!(total <= max_timeline_samples(rate as u32));
            let mut written = 0u64;
            let mut largest = 0usize;
            let mut sink = |block: &[f32]| {
                written += block.len() as u64;
                largest = largest.max(block.len());
                Ok(())
            };
            let mut last_tick = 0;
            let mut tick = |done: u64, all: u64| {
                assert!(done >= last_tick && all == total);
                last_tick = done;
                Ok(())
            };
            let n = merge_stream(strategy, &mut sources, rate, &mut sink, &mut tick).unwrap();
            assert_eq!(n, total);
            assert_eq!(written, total);
            assert_eq!(last_tick, total);
            assert!(
                largest <= MIX_BLOCK_SAMPLES.max(rate / 2),
                "{strategy:?} sent {largest} samples at once"
            );
        }
    }

    #[test]
    fn a_tick_error_stops_the_merge() {
        let sources = vec![noise(10_000, 1, 0.5)];
        let mut readers = aligned(&sources, &[0]);
        let mut blocks = 0;
        let mut sink = |_: &[f32]| {
            blocks += 1;
            Ok(())
        };
        let mut calls = 0;
        let mut tick = |_: u64, _: u64| {
            calls += 1;
            if calls > 2 {
                Err("Merge cancelled".to_string())
            } else {
                Ok(())
            }
        };
        let err = mix_all_stream(&mut readers, 1_000, &mut sink, &mut tick).unwrap_err();
        assert_eq!(err, "Merge cancelled");
        assert_eq!(blocks, 2);
    }

    #[test]
    fn a_source_over_thirty_minutes_is_refused_by_name() {
        let limit = MAX_MERGE_DURATION_SECS as f64;
        assert!(check_source_duration("a.wav", Some(limit)).is_ok());
        let err = check_source_duration("Backup recorder.mp3", Some(limit + 0.5)).unwrap_err();
        assert!(err.contains("\"Backup recorder.mp3\""), "{err}");
        assert!(err.contains("30 minutes"), "{err}");
        for unknown in [None, Some(f64::NAN), Some(0.0), Some(f64::INFINITY)] {
            let err = check_source_duration("Phone.m4a", unknown).unwrap_err();
            assert!(err.contains("\"Phone.m4a\""), "{err}");
        }
    }

    #[test]
    fn sync_finds_known_offsets_with_the_documented_sign() {
        let rate = 8_000usize;
        let event = noise(rate * 180, 21, 0.5);
        // B starts `d` seconds after A, so B's sample i is the event at i + d.
        for d in [2.5f64, -1.25, 0.0, 37.0, -0.0137, 12.3456, -59.0] {
            let a_start = (rate as f64 * 60.0) as usize;
            let b_start = (a_start as f64 + d * rate as f64) as usize;
            let a = &event[a_start..a_start + rate * 75];
            let b = &event[b_start..b_start + rate * 75];
            let found = detect_sync_samples(a, b, rate).unwrap();
            assert!(
                (found.offset_seconds - d).abs() <= 0.001,
                "expected {d}, found {}",
                found.offset_seconds
            );
            assert!(found.is_same_event, "confidence {}", found.confidence);
        }
    }

    #[test]
    fn sync_matches_a_second_microphone_off_the_grid() {
        // B hears the same event duller, quieter, inverted and with its own
        // hiss, and starts 4.321 s after A.
        let rate = 8_000usize;
        let event = noise(rate * 120, 33, 0.5);
        let hiss = noise(rate * 120, 99, 0.05);
        let second: Vec<f32> = (0..event.len())
            .map(|i| {
                let prev = if i > 0 { event[i - 1] } else { 0.0 };
                -0.6 * (event[i] + prev) / 2.0 + hiss[i]
            })
            .collect();
        let d = 4.321f64;
        let a_start = rate * 20;
        let b_start = a_start + (d * rate as f64) as usize;
        let a = &event[a_start..a_start + rate * 75];
        let b = &second[b_start..b_start + rate * 75];
        let found = detect_sync_samples(a, b, rate).unwrap();
        assert!(
            (found.offset_seconds - d).abs() <= 0.001,
            "found {}",
            found.offset_seconds
        );
        assert!(found.is_same_event, "confidence {}", found.confidence);
    }

    #[test]
    fn sync_does_not_match_unrelated_recordings() {
        let rate = 8_000usize;
        let a = noise(rate * 20, 3, 0.5);
        let b = noise(rate * 20, 77, 0.5);
        assert!(!detect_sync_samples(&a, &b, rate).unwrap().is_same_event);
    }

    #[test]
    fn cross_correlate_offset_sign_convention() {
        // b[i + 100] = a[i]: recorder B started 100 samples EARLIER, so the
        // peak is at +100 and detect_sync reports a negative offset.
        let mut a = vec![0.0f32; 400];
        a[10] = 1.0;
        a[50] = -0.5;
        a[200] = 0.8;
        let mut b = vec![0.0f32; 600];
        for (i, &v) in a.iter().enumerate() {
            b[i + 100] = v;
        }
        let at_plus = cross_correlate(&a, &b, 100, a.len());
        assert!(at_plus > cross_correlate(&a, &b, 0, a.len()));
        assert!(at_plus > cross_correlate(&a, &b, -100, a.len()));
    }
}
