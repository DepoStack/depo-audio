use std::fs;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter};

use crate::analysis::analyze_audio;
use crate::denoise::{decode_to_wav_48k, denoise_deep_filter, denoise_wav};
use crate::enhance::enhance_bandwidth;
use crate::ffmpeg::{build_proc_filters_with_gain, probe_channels, run_ffmpeg};
use crate::helpers::{basename, detect_format_for_path, output_args, output_ext, safe_label, strip_sgmca_header, unique_path};
use crate::types::{ConvertJob, FormatInfo, OutputFile, ProgressEvent};

// ── Conversion orchestration ─────────────────────────────────────────────────

pub(crate) async fn do_convert(app: &AppHandle, job: &ConvertJob) -> Result<Vec<OutputFile>, String> {
    let fmt = detect_format_for_path(&job.src_path)
        .ok_or("Unrecognised file format")?;

    if fmt.handler == "rejected" {
        return Err(fmt.note.unwrap_or_else(|| "This format cannot be converted.".into()));
    }

    let src = Path::new(&job.src_path);
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
    // Runs Rust-native AI processing before the FFmpeg pipeline.
    let mut ai_feed = feed.to_path_buf();
    let mut ai_temps: Vec<PathBuf> = Vec::new();
    let mut channel_gains: Option<Vec<f64>> = None;

    let has_ai = job.denoise || job.auto_level || job.declip || job.enhance;

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

        // Denoise: route based on quality setting
        if job.denoise {
            if job.denoise_quality == "best" {
                // DeepFilterNet3 — best quality, uses ONNX models
                match denoise_deep_filter(app, &ai_feed).await {
                    Ok(denoised) => {
                        ai_temps.push(denoised.clone());
                        ai_feed = denoised;
                    }
                    Err(_) => {
                        // Fall back to RNNoise if DFN3 fails
                        let decoded = decode_to_wav_48k(app, &ai_feed).await?;
                        ai_temps.push(decoded.clone());
                        match denoise_wav(&decoded) {
                            Ok(d) => { ai_temps.push(d.clone()); ai_feed = d; }
                            Err(_) => { ai_feed = decoded; }
                        }
                    }
                }
            } else {
                // RNNoise — fast, lightweight
                let decoded = decode_to_wav_48k(app, &ai_feed).await?;
                ai_temps.push(decoded.clone());
                match denoise_wav(&decoded) {
                    Ok(denoised) => {
                        ai_temps.push(denoised.clone());
                        ai_feed = denoised;
                    }
                    Err(_) => { ai_feed = decoded; }
                }
            }
        }

        // Bandwidth extension: upscale narrow-band audio to 48 kHz
        if job.enhance {
            match enhance_bandwidth(app, &ai_feed).await {
                Ok(enhanced) => {
                    ai_temps.push(enhanced.clone());
                    ai_feed = enhanced;
                }
                Err(_) => {} // Fall through on failure
            }
        }
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
        app, job, effective_feed, &input_codec,
        if job.auto_level && job.mode != "stereo" { default_gain } else { None },
    ).await;

    let mut ffmpeg_args: Vec<String> = input_codec.clone();
    ffmpeg_args.extend(["-i".into(), effective_feed.to_string_lossy().to_string()]);

    let mut output_paths: Vec<PathBuf> = Vec::new();

    match job.mode.as_str() {
        "stereo" => {
            let dst = unique_path(&out_dir.join(format!("{}{}", base, ext)));
            let num_ch = probe_channels(app, effective_feed, &input_codec).await;
            let weight = if num_ch > 0 { 1.0 / num_ch as f64 } else { 0.25 };

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
            let all: Vec<String> = std::iter::once(pan).chain(proc.into_iter()).collect();
            let mut args = ffmpeg_args.clone();
            args.extend(["-af".into(), all.join(",")]);
            args.extend(out_codec.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            run_ffmpeg(app, args, &job.id).await?;
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
            run_ffmpeg(app, args, &job.id).await?;
            output_paths.push(dst);
        }

        "split" => {
            let num_ch = probe_channels(app, feed, &input_codec).await;
            let labels: Vec<String> = (0..num_ch as usize).map(|i| {
                let raw = job.labels.get(i).map(|s| s.as_str()).unwrap_or("");
                let sl = safe_label(raw);
                if sl.is_empty() { format!("ch{}", i + 1) } else { sl }
            }).collect();
            let dsts: Vec<PathBuf> = labels.iter()
                .map(|l| unique_path(&out_dir.join(format!("{}_{}{}", base, l, ext))))
                .collect();

            let mut args = ffmpeg_args.clone();
            if !proc.is_empty() {
                let sp_tags: Vec<String> = (0..num_ch as usize).map(|i| format!("sp{}", i)).collect();
                let _op_tags: Vec<String> = (0..num_ch as usize).map(|i| format!("op{}", i)).collect();
                let split_str = format!("[0:a]channelsplit[{}]", sp_tags.join("]["));
                let chain: Vec<String> = (0..num_ch as usize)
                    .map(|i| format!("[sp{}]{}[op{}]", i, proc.join(","), i))
                    .collect();
                let fc = std::iter::once(split_str).chain(chain).collect::<Vec<_>>().join(";");
                args.extend(["-filter_complex".into(), fc]);
                for (i, dst) in dsts.iter().enumerate() {
                    args.extend(["-map".into(), format!("[op{}]", i)]);
                    args.extend(out_codec.clone());
                    args.push(dst.to_string_lossy().to_string());
                }
            } else {
                let tags: Vec<String> = (0..num_ch as usize).map(|i| format!("ch{}", i)).collect();
                let fc = format!("[0:a]channelsplit[{}]", tags.join("]["));
                args.extend(["-filter_complex".into(), fc]);
                for (i, dst) in dsts.iter().enumerate() {
                    args.extend(["-map".into(), format!("[ch{}]", i)]);
                    args.extend(out_codec.clone());
                    args.push(dst.to_string_lossy().to_string());
                }
            }
            args.push("-y".into());
            run_ffmpeg(app, args, &job.id).await?;
            output_paths.extend(dsts);
        }

        _ => return Err(format!("Unknown mode: {}", job.mode)),
    }

    let files: Vec<OutputFile> = output_paths.into_iter().map(|p| {
        let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        OutputFile { name: basename(&p.to_string_lossy()), path: p.to_string_lossy().to_string(), size }
    }).collect();

    if let Some(empty) = files.iter().find(|f| f.size == 0) {
        // Clean up AI temp files before returning error
        for tmp in &ai_temps { let _ = fs::remove_file(tmp); }
        return Err(format!("Output file is empty: {}", empty.name));
    }

    // Clean up AI temp files
    for tmp in &ai_temps { let _ = fs::remove_file(tmp); }

    Ok(files)
}
