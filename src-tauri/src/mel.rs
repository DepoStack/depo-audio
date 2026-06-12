use rustfft::{num_complex::Complex, FftPlanner};

// ── Whisper-style log-mel spectrogram ───────────────────────────────────────
//
// Smart Turn v3 takes `input_features` of shape [1, 80, 800]: 8 seconds of
// 16 kHz audio as an 80-bin log-mel spectrogram with n_fft=400, hop=160,
// normalized the way Whisper does it (log10, clamp to max-8, (x+4)/4).

pub(crate) const N_MELS: usize = 80;
pub(crate) const N_FRAMES: usize = 800;
const N_FFT: usize = 400;
const HOP: usize = 160;
const SAMPLE_RATE: f64 = 16000.0;
const N_BINS: usize = N_FFT / 2 + 1; // 201

fn hz_to_mel(f: f64) -> f64 {
    2595.0 * (1.0 + f / 700.0).log10()
}

fn mel_to_hz(m: f64) -> f64 {
    700.0 * (10f64.powf(m / 2595.0) - 1.0)
}

/// Triangular mel filterbank, N_MELS x N_BINS.
fn mel_filterbank() -> Vec<Vec<f32>> {
    let mel_max = hz_to_mel(SAMPLE_RATE / 2.0);
    let bin_of = |i: usize| -> usize {
        let mel = mel_max * i as f64 / (N_MELS + 1) as f64;
        (((N_FFT + 1) as f64) * mel_to_hz(mel) / SAMPLE_RATE).floor() as usize
    };
    let mut fb = vec![vec![0f32; N_BINS]; N_MELS];
    for m in 0..N_MELS {
        let (l, c, r) = (bin_of(m), bin_of(m + 1), bin_of(m + 2));
        for k in l..c.min(N_BINS) {
            if c > l {
                fb[m][k] = (k - l) as f32 / (c - l) as f32;
            }
        }
        for k in c..r.min(N_BINS) {
            if r > c {
                fb[m][k] = (r - k) as f32 / (r - c) as f32;
            }
        }
    }
    fb
}

/// Compute log-mel features for one 8-second window of 16 kHz mono audio.
/// Shorter input is zero-padded. Returns N_MELS * N_FRAMES floats laid out
/// row-major (mel bin outermost), matching an ONNX [1, 80, 800] tensor.
pub(crate) fn log_mel_8s(audio: &[f32]) -> Vec<f32> {
    let len = N_FRAMES * HOP; // 128000 samples
    let pad = N_FFT / 2;

    // Center the frames: reflect-pad like torch.stft(center=true)
    let mut padded = vec![0f32; len + 2 * pad];
    for (i, slot) in padded.iter_mut().enumerate() {
        let idx = i as i64 - pad as i64;
        let src = if idx < 0 {
            (-idx) as usize // reflect around 0
        } else if (idx as usize) < len.min(audio.len()) {
            idx as usize
        } else {
            // beyond the input: reflect around the end, else zero-pad region
            let over = idx as usize;
            if over < len && audio.len() >= 2 {
                audio.len().saturating_sub(2 + (over - audio.len().min(over)))
            } else {
                usize::MAX
            }
        };
        *slot = audio.get(src).copied().unwrap_or(0.0);
    }

    // Periodic Hann window
    let window: Vec<f32> = (0..N_FFT)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / N_FFT as f64).cos())
        .map(|v| v as f32)
        .collect();

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let fb = mel_filterbank();

    // Power spectrogram -> mel energies, frame by frame
    let mut mel = vec![0f32; N_MELS * N_FRAMES];
    let mut buf = vec![Complex::default(); N_FFT];
    for t in 0..N_FRAMES {
        let start = t * HOP;
        for n in 0..N_FFT {
            buf[n] = Complex::new(padded[start + n] * window[n], 0.0);
        }
        fft.process(&mut buf);
        let power: Vec<f32> = buf[..N_BINS].iter().map(|c| c.norm_sqr()).collect();
        for m in 0..N_MELS {
            let e: f32 = fb[m].iter().zip(&power).map(|(w, p)| w * p).sum();
            mel[m * N_FRAMES + t] = e;
        }
    }

    // Whisper normalization: log10, clamp to (global max - 8), scale to ~[-1, 1]
    let mut max_log = f32::NEG_INFINITY;
    for v in mel.iter_mut() {
        *v = v.max(1e-10).log10();
        if *v > max_log {
            max_log = *v;
        }
    }
    let floor = max_log - 8.0;
    for v in mel.iter_mut() {
        *v = (v.max(floor) + 4.0) / 4.0;
    }
    mel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_mel_shape_and_range() {
        // 440 Hz tone at 16 kHz for 8 s
        let audio: Vec<f32> = (0..128000)
            .map(|i| 0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16000.0).sin() as f32)
            .collect();
        let feats = log_mel_8s(&audio);
        assert_eq!(feats.len(), N_MELS * N_FRAMES);
        // Whisper normalization bounds: (max-8+4)/4 = -1 .. (max+4)/4
        let lo = feats.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = feats.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(hi > lo, "features are not constant");
        assert!(hi - lo <= 2.0 + 1e-3, "normalized span must be <= 2 (got {})", hi - lo);
    }

    #[test]
    fn log_mel_handles_short_input() {
        let feats = log_mel_8s(&[0.1f32; 1000]);
        assert_eq!(feats.len(), N_MELS * N_FRAMES);
        assert!(feats.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn log_mel_matches_reference_implementation() {
        // Reference values computed with a float64 numpy port of this exact
        // algorithm, validated against the Smart Turn v3 model
        let audio: Vec<f32> = (0..128000)
            .map(|i| 0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16000.0).sin() as f32)
            .collect();
        let feats = log_mel_8s(&audio);

        let mean: f64 = feats.iter().map(|&v| v as f64).sum::<f64>() / feats.len() as f64;
        assert!((mean - -0.077015).abs() < 1e-3, "mean {} != reference -0.077015", mean);

        let max = feats.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!((max - 1.849485).abs() < 1e-3, "max {} != reference 1.849485", max);

        let probe = |m: usize, t: usize| feats[m * N_FRAMES + t];
        assert!((probe(0, 0) - 1.380703).abs() < 1e-3);
        assert!((probe(40, 400) - -0.150515).abs() < 1e-3);
        assert!((probe(79, 799) - -0.138396).abs() < 1e-3);
    }
}
