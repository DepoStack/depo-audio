use std::fs;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter};

use uuid::Uuid;

use crate::analysis::analyze_audio;
use crate::denoise::{decode_to_wav_48k, denoise_buffer, denoise_deep_filter};
use crate::dereverb;
use crate::enhance::enhance_buffer;
use crate::ffmpeg::{build_proc_filters_with_gain, probe_channels, run_ffmpeg_with_timeout};
use crate::helpers::{basename, detect_format_for_path, output_args, output_ext, safe_label, strip_sgmca_header, unique_path};
use crate::types::{AudioBuffer, ConvertJob, FormatInfo, OutputFile, ProgressEvent};

// ── Conversion orchestration ─────────────────────────────────────────────────

pub(crate) async fn do_convert(app: &AppHandle, job: &ConvertJob) -> Result<Vec<OutputFile>, String> {
    // Safety checks
    let src = Path::new(&job.src_path);
    let max_bytes = (job.max_file_size_gb * 1024.0 * 1024.0 * 1024.0) as u64;
    crate::safety::check_file_safe_with_limit(src, max_bytes)?;
    if job.fade { crate::safety::validate_fade_dur(job.fade_dur)?; }
    crate::safety::validate_rate(&job.rate)?;

    let fmt = detect_format_for_path(&job.src_path)
        .ok_or("Unrecognised file format")?;

    if fmt.handler == "rejected" {
        return Err(fmt.note.unwrap_or_else(|| "This format cannot be converted.".into()));
    }
    let (feed_path, is_temp) = if fmt.handler == "sgmca" {
        strip_sgmca_header(src)?
    } else {
        (src.to_path_buf(), false)
    };

    let result = do_convert_inner(app, job, &feed_path, &fmt).await;

    if is_temp { let _ = fs::remove_file(&feed_path); }
    result
}

async fn do_convert_inner(app: &AppHandle, job: &ConvertJob, feed: &Path, fmt: &FormatInfo) -> Result<Vec<OutputFile>, String> {
    let input_codec: Vec<String> = if fmt.handler == "ftr" {
        vec!["-acodec".into(), "aac".into()]
    } else {
        vec![]
    };

    let base = Path::new(&job.src_path)
        .file_stem().and_then(|s| s.to_str()).unwrap_or("output");

    let out_dir = if job.out_dir.is_empty() {
        Path::new(&job.src_path).parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        PathBuf::from(&job.out_dir)
    };

    let ext = output_ext(&job.format);
    let out_codec = output_args(&job.format, &job.rate);

    // ── AI pre-processing step ──────────────────────────────────────────────
    // Uses in-memory AudioBuffer pipeline to eliminate intermediate temp WAV files.
    // Pattern: decode → temp WAV → read to buffer → delete temp → process in memory
    //        → write single temp WAV → FFmpeg.
    let mut ai_feed = feed.to_path_buf();
    let mut ai_temps: Vec<crate::safety::TempFile> = Vec::new();
    let mut channel_gains: Option<Vec<f64>> = None;

    let has_ai = job.denoise || job.auto_level || job.declip || job.enhance || job.dereverb;

    if has_ai {
        // Phase: Analyzing
        let _ = app.emit("convert:progress", ProgressEvent {
            id: job.id.clone(), seconds: 0.0, phase: Some("analyzing".into()),
        });

        // Run analysis if auto-leveling is requested
        if job.auto_level {
            if let Ok(analysis) = analyze_audio(app, &job.src_path).await {
                channel_gains = Some(analysis.channel_gains);
            }
        }

        // Phase: Cleaning up audio
        let _ = app.emit("convert:progress", ProgressEvent {
            id: job.id.clone(), seconds: 0.0, phase: Some("processing".into()),
        });

        // Decode input to 48kHz WAV, read into AudioBuffer, delete temp immediately
        let decoded_wav = decode_to_wav_48k(app, &ai_feed, &input_codec).await?;
        let mut audio_buf = AudioBuffer::from_wav(&decoded_wav)?;
        let _ = fs::remove_file(&decoded_wav);

        // Denoise in-memory: route based on quality setting
        if job.denoise {
            if job.denoise_quality == "best" {
                // DeepFilterNet3 — best quality, uses ONNX models
                // DFN3 still uses file-based path (complex STFT pipeline)
                // Write buffer to temp, run DFN3, read result back
                match denoise_deep_filter(app, &ai_feed, &input_codec).await {
                    Ok(denoised) => {
                        match AudioBuffer::from_wav(&denoised) {
                            Ok(buf) => { audio_buf = buf; }
                            Err(_) => {} // Keep current buffer on read failure
                        }
                        let _ = fs::remove_file(&denoised);
                    }
                    Err(_) => {
                        // Fall back to RNNoise in-memory
                        let _ = denoise_buffer(&mut audio_buf);
                    }
                }
            } else {
                // RNNoise — fast, lightweight, fully in-memory
                let _ = denoise_buffer(&mut audio_buf);
            }
        }

        // De-reverberation in-memory (if DCCRN+ model available)
        if job.dereverb {
            let _ = dereverb::dereverb_buffer(app, &mut audio_buf);
        }

        // Bandwidth extension in-memory (if FlashSR model available)
        if job.enhance {
            let _ = enhance_buffer(app, &mut audio_buf);
        }

        // Write the processed AudioBuffer to a single temp WAV for FFmpeg encoding
        let tmp_out = std::env::temp_dir().join(format!(
            "depoaudio_ai_{}.wav",
            Uuid::new_v4().to_string().replace('-', "")
        ));
        audio_buf.to_wav(&tmp_out)?;
        ai_temps.push(crate::safety::TempFile::new(tmp_out.clone()));
        ai_feed = tmp_out;
    }

    // Build filter chain — use ai_feed (post-AI) as input to FFmpeg
    let effective_feed = &ai_feed;

    // For stereo mode with auto-level, we inject gains per-channel in the pan filter
    // For other modes, we inject a single gain into the proc filter chain
    let default_gain = channel_gains.as_ref().map(|g| {
        // Average gain across active channels as fallback for non-split modes
        let active: Vec<f64> = g.iter().copied().filter(|&v| v > 0.1).collect();
        if active.is_empty() { 1.0 } else { active.iter().sum::<f64>() / active.len() as f64 }
    });

    let proc = build_proc_filters_with_gain(
        app, job, effective_feed,
        if job.auto_level && job.mode != "stereo" { default_gain } else { None },
    ).await;

    // When AI processing ran, the feed is our own PCM WAV — forcing the
    // original container's codec (e.g. -acodec aac for FTR) would corrupt it
    let mut ffmpeg_args: Vec<String> = if has_ai { Vec::new() } else { input_codec.clone() };
    ffmpeg_args.extend(["-i".into(), effective_feed.to_string_lossy().to_string()]);

    let mut output_paths: Vec<PathBuf> = Vec::new();

    match job.mode.as_str() {
        "stereo" => {
            let dst = unique_path(&out_dir.join(format!("{}{}", base, ext)));
            let num_ch = probe_channels(app, effective_feed).await
                .ok_or("Cannot create stereo mix: unable to determine the input's channel count")?;
            let weight = 1.0 / num_ch as f64;

            // Use auto-level gains if available, otherwise use manual chan_vols
            let vols: Vec<f64> = if job.auto_level {
                if let Some(ref gains) = channel_gains {
                    gains.iter().map(|&g| g * weight).collect()
                } else {
                    (0..num_ch).map(|i| job.chan_vols.get(i as usize).copied().unwrap_or(1.0) * weight).collect()
                }
            } else {
                (0..num_ch).map(|i| job.chan_vols.get(i as usize).copied().unwrap_or(1.0) * weight).collect()
            };

            let scale = num_ch as f64; // compensate for per-channel weight
            let weights: Vec<String> = (0..num_ch).map(|i| {
                let v = vols.get(i as usize).copied().unwrap_or(weight);
                format!("{:.4}*c{}", v, i)
            }).collect();
            let mix = weights.join("+");
            let pan = format!("pan=stereo|c0={}|c1={},volume={:.1}", mix, mix, scale);
            let mut all: Vec<String> = std::iter::once(pan).chain(proc.into_iter()).collect();
            // The weight/scale pair makes this a unity-gain SUM of channels:
            // right for turn-taking speakers, but correlated content (every
            // mic hearing the same voice) can exceed full scale by up to
            // 20*log10(N) dB. loudnorm true-peak-limits when normalize is on;
            // otherwise cap peaks ourselves so the encode doesn't hard-clip.
            if !job.normalize {
                all.push("alimiter=limit=0.97:level=false".into());
            }
            let mut args = ffmpeg_args.clone();
            args.extend(["-af".into(), all.join(",")]);
            args.extend(out_codec.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            run_ffmpeg_with_timeout(app, args, &job.id, job.ffmpeg_timeout as u64).await?;
            output_paths.push(dst);
        }

        "keep" => {
            let dst = unique_path(&out_dir.join(format!("{}_orig{}", base, ext)));
            let mut args = ffmpeg_args.clone();
            if !proc.is_empty() {
                args.extend(["-af".into(), proc.join(",")]);
            }
            args.extend(out_codec.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            run_ffmpeg_with_timeout(app, args, &job.id, job.ffmpeg_timeout as u64).await?;
            output_paths.push(dst);
        }

        "split" => {
            let num_ch = probe_channels(app, effective_feed).await
                .ok_or("Cannot split channels: unable to determine the input's channel count")?;
            let labels: Vec<String> = (0..num_ch as usize).map(|i| {
                let raw = job.labels.get(i).map(|s| s.as_str()).unwrap_or("");
                let sl = safe_label(raw);
                if sl.is_empty() { format!("ch{}", i + 1) } else { sl }
            }).collect();
            let dsts: Vec<PathBuf> = labels.iter()
                .map(|l| unique_path(&out_dir.join(format!("{}_{}{}", base, l, ext))))
                .collect();

            // Use asplit + pan=mono per channel instead of channelsplit:
            // channelsplit requires the actual channel layout (defaulting to
            // stereo), which breaks mono and 4-channel court recordings.
            let mut args = ffmpeg_args.clone();
            let sp_tags: Vec<String> = (0..num_ch as usize).map(|i| format!("sp{}", i)).collect();
            let split_str = format!("[0:a]asplit={}[{}]", num_ch, sp_tags.join("]["));
            let per_ch_proc = if proc.is_empty() { String::new() } else { format!(",{}", proc.join(",")) };
            let chain: Vec<String> = (0..num_ch as usize)
                .map(|i| format!("[sp{}]pan=mono|c0=c{}{}[op{}]", i, i, per_ch_proc, i))
                .collect();
            let fc = std::iter::once(split_str).chain(chain).collect::<Vec<_>>().join(";");
            args.extend(["-filter_complex".into(), fc, "-y".into()]);
            for (i, dst) in dsts.iter().enumerate() {
                args.extend(["-map".into(), format!("[op{}]", i)]);
                args.extend(out_codec.clone());
                args.push(dst.to_string_lossy().to_string());
            }
            run_ffmpeg_with_timeout(app, args, &job.id, job.ffmpeg_timeout as u64).await?;
            output_paths.extend(dsts);
        }

        _ => return Err(format!("Unknown mode: {}", job.mode)),
    }

    let files: Vec<OutputFile> = output_paths.into_iter().map(|p| {
        let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        OutputFile { name: basename(&p.to_string_lossy()), path: p.to_string_lossy().to_string(), size }
    }).collect();

    if let Some(empty) = files.iter().find(|f| f.size == 0) {
        // ai_temps cleaned up automatically via TempFile Drop
        return Err(format!("Output file is empty: {}", empty.name));
    }

    // ai_temps cleaned up automatically via TempFile Drop

    Ok(files)
}
