//! FTR (For The Record) `.trm` / `.tll` recordings: inspection and
//! per-stream extraction.
//!
//! Source of the layout below: the owner's latest interoperability notes on
//! FTR's reader, writer and splitter, described here in our own words. They
//! have been checked against a file made by the owner's rebuild tool (not by
//! FTR, see `scripts/fixtures/trm/vendor-notes-samples/`), never against a
//! real FTR recording. Everything is little-endian.
//!
//! - The file is a RIFF `AVI `. Top-level chunks before `LIST movi` can
//!   include `TUTC` and `TLOC` (u64 FILETIMEs: the file's start in UTC and in
//!   local time), `USER` and `UDAT` (UTF-16LE user name and free text, which
//!   DepoAudio only notes as present, never reads into text), `FILE` (the
//!   file's own name, UTF-16LE, kept only when it is a name in the
//!   recorder's format, otherwise only noted as present) and the 20-byte
//!   `FTRA` (five u32 values;
//!   informational for `.trm`). After `idx1` the recorder may append `peak`,
//!   `SIGN` and `CERT`; they are skipped (a signature is reported as
//!   "signed (not verified)", never checked or produced).
//! - `avih` `dwFlags` carries two boundary flags: `0x20000000` (no earlier
//!   file may be joined to this one) and `0x40000000` (no later file may be
//!   joined). `dwStreams` can count an input that has no `strl`, so the
//!   number of `strl` lists is what counts.
//! - The audio stream header (`strh` type `auds`) has `dwScale` 1024 and
//!   `dwRate` = sample rate for AAC; `dwLength` is the number of audio
//!   chunks, and `0x7FFFFFFF` while recording (the file was never closed).
//! - The audio format (`strf`) is a WAVEFORMATEX. The tag's low 12 bits
//!   `0x180` mean FTR's AAC; bit `0x4000` means each interleaved stream is
//!   mono (N = `nChannels` streams), bit `0x8000` each stream is stereo
//!   (N = `nChannels` / 2). `0x180` with neither bit is refused, as FTR's own
//!   splitter refuses it. MP2, MP3 and WMA tags are ordinary audio for FFmpeg.
//! - Audio chunks (`00wb`) are found through `idx1` (offsets relative to the
//!   `movi` list or absolute) or, without a usable index or when the index
//!   misses chunks `movi` holds, by walking `movi` (which looks past a
//!   damaged stretch for the next audio chunk).
//!   Level-meter chunks (`NNal`) and video chunks are ignored.
//! - The chunk payloads, joined in file order, are one run of ADTS AAC-LC
//!   frames that may cross chunk boundaries. The first frame whose ADTS
//!   private bit is clear belongs to stream 0; from there frame `k` belongs
//!   to stream `k mod N`. A frame with the original/copy bit set has five
//!   payload bytes scrambled by a self-inverse swap-and-invert, undone here.
//!   FTR reads an 11-bit frame length and ignores the two header bits above
//!   it; those bits are cleared in the stream files so a standard ADTS
//!   reader sees the same length. Each stream is then a plain mono (or
//!   stereo) ADTS stream; stream `k` of a stereo-stream file holds channels
//!   `2k` and `2k + 1`.
//!
//! The private bit is checked on every frame, not only the first: a round
//! must open with a stream-0 frame and, once the file has shown that it
//! marks the other streams, continue with frames that are not. When that
//! breaks (a lost or extra frame), the partial round is dropped, dealing
//! restarts at the next stream-0 frame, and the restart is counted.
//!
//! Reading streams through the file in fixed-size pieces: memory stays
//! bounded by one piece, one partial frame and one round of frames, whatever
//! the file size. Nothing here writes to the original file.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The codec in the low 12 bits of the WAVEFORMATEX tag: FTR's AAC.
pub(crate) const FTR_AAC_TAG: u16 = 0x0180;
/// Tag bit: each interleaved stream is mono (N = nChannels streams).
pub(crate) const MONO_STREAMS_FLAG: u16 = 0x4000;
/// Tag bit: each interleaved stream is stereo (N = nChannels / 2 streams).
pub(crate) const STEREO_STREAMS_FLAG: u16 = 0x8000;
/// The tag FTR's notes recommend for new files: AAC, one mono stream per
/// microphone.
#[cfg(test)]
pub(crate) const MONO_STREAMS_TAG: u16 = FTR_AAC_TAG | MONO_STREAMS_FLAG;
const CODEC_MASK: u16 = 0x0FFF;
const OPEN_ENDED_LENGTH: u32 = 0x7FFF_FFFF;
const START_BOUNDARY_FLAG: u32 = 0x2000_0000;
const END_BOUNDARY_FLAG: u32 = 0x4000_0000;
/// The registry and every conversion path cap court recordings at 16 channels.
pub(crate) const MAX_TRM_CHANNELS: u16 = 16;
const MAX_HEADER_LIST_BYTES: u64 = 1024 * 1024;
const READ_PIECE_BYTES: usize = 64 * 1024;
const ADTS_HEADER_BYTES: usize = 7;
/// Scrambled bytes reach index 12, so a frame must be at least this long.
const SCRAMBLE_MIN_FRAME_BYTES: usize = 13;
const SAMPLES_PER_FRAME: u64 = 1024;
/// How many index entries or chunks are read between cancellation checks.
const CANCEL_CHECK_EVERY: u64 = 4096;
const ADTS_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350,
];
/// `FILE` holds 256 UTF-16 units; `USER` 0x200 bytes and `UDAT` 0x400.
const FILE_NAME_CHUNK_BYTES: u64 = 0x200;
/// The recorder cuts the name part of a file name to this many characters.
const FILE_NAME_PART_MAX_CHARS: usize = 28;
/// Characters the recorder keeps in the name part besides letters and
/// digits (every other one becomes `-`).
const FILE_NAME_PART_EXTRA_CHARS: &str = "!$`() +,-.=[]^'{}";
/// 100 ns FILETIME ticks between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
const FILETIME_TICKS_PER_DAY: u64 = 24 * 60 * 60 * 10_000_000;
/// What the export record and the queue say about a `SIGN`/`CERT` chunk.
pub(crate) const SIGNED_NOT_VERIFIED: &str = "signed (not verified)";

pub(crate) const TRM_CANCELLED_MESSAGE: &str = crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE;
const TRM_CHANGED_MESSAGE: &str = "The FTR recording changed while it was being read.";
/// FTR's own splitter refuses an AAC tag without a stream-layout bit; so
/// does DepoAudio (FFmpeg's native decoder would read it without a check).
pub(crate) const TRM_UNSPLITTABLE_MESSAGE: &str =
    "This FTR recording's format isn't one FTR's own player can split, so DepoAudio does not convert it.";
/// The reason recorded when DepoAudio read the layout but found no complete
/// round to split, and FFmpeg's native decoder read the file instead.
const NO_COMPLETE_ROUND_ERROR: &str =
    "No complete round of FTR audio frames was found in this recording, so there is nothing to convert.";
pub(crate) const NO_COMPLETE_ROUND_MESSAGE: &str = "DepoAudio found no complete round of audio frames it could split (no frame it could take as stream 0), so FFmpeg's native ftr decoder read the file as it is, without DepoAudio's checks.";
const TRM_FRAME_CHANNELS_MESSAGE: &str = "This FTR recording's audio frames carry a different number of channels than its header declares, so DepoAudio cannot tell which microphone is which and does not split it.";

/// A refusal that no other route may override: the file is not handed to
/// FFmpeg's native decoder either.
pub(crate) fn is_refusal(error: &str) -> bool {
    error == TRM_UNSPLITTABLE_MESSAGE
}

/// The optional `FTRA` chunk, as read. FTR's reader uses it only for `.avi`
/// files with a non-zero `c` (MP3 saved as `.avi`); for a `.trm` it is
/// informational ({1, 0, 0, chunks, start or 0xFFFFFFFF}), so DepoAudio
/// reports it and does not act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FtraInfo {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
    pub e: u32,
}

/// How the audio chunks were found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub(crate) enum IndexKind {
    /// `idx1` offsets are absolute file positions.
    #[serde(rename = "idx1-absolute")]
    Idx1Absolute,
    /// `idx1` offsets count from the `movi` list.
    #[serde(rename = "idx1-relative")]
    Idx1Relative,
    /// No usable `idx1`: the `movi` list was walked chunk by chunk.
    #[default]
    #[serde(rename = "walked")]
    Walked,
    /// The audio is not FTR AAC: FFmpeg reads the file as it is and
    /// DepoAudio does not read its chunks.
    #[serde(rename = "not-read")]
    NotRead,
}

/// The audio codec the format tag names (low 12 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub(crate) enum AudioCodec {
    /// 0x180: FTR's interleaved AAC.
    #[default]
    #[serde(rename = "ftr-aac")]
    FtrAac,
    /// 0x50.
    #[serde(rename = "mp2")]
    Mp2,
    /// 0x55.
    #[serde(rename = "mp3")]
    Mp3,
    /// 0x160 to 0x163.
    #[serde(rename = "wma")]
    Wma,
}

impl AudioCodec {
    fn from_tag(tag: u16) -> Option<Self> {
        match tag & CODEC_MASK {
            FTR_AAC_TAG => Some(Self::FtrAac),
            0x050 => Some(Self::Mp2),
            0x055 => Some(Self::Mp3),
            0x160..=0x163 => Some(Self::Wma),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FtrAac => "FTR AAC",
            Self::Mp2 => "MPEG audio layer 2",
            Self::Mp3 => "MP3",
            Self::Wma => "Windows Media Audio",
        }
    }
}

/// How FTR AAC interleaves its streams, from the tag's high bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum StreamLayout {
    /// Tag bit 0x4000: one mono stream per microphone.
    #[serde(rename = "mono-streams")]
    Mono,
    /// Tag bit 0x8000: stereo streams, two microphones each.
    #[serde(rename = "stereo-streams")]
    Stereo,
}

/// Which chunk gave the start time that feeds the BWF start (`timing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum StartBasis {
    /// `TLOC`: local wall-clock time, as a BWF time reference counts it.
    #[serde(rename = "tloc-local")]
    Local,
    /// `TUTC` only (no `TLOC`): UTC wall-clock time.
    #[serde(rename = "tutc-utc")]
    Utc,
    /// Neither chunk: the FILETIME in the `FILE` chunk's name, whose time
    /// zone the name does not state.
    #[serde(rename = "file-name-unknown-zone")]
    FileName,
}

/// What one full read of a `.trm` found. Counts are of what was read, not of
/// what the header claims. Private text (`USER`, `UDAT`) is never held here:
/// only whether it is present.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrmInfo {
    /// The WAVEFORMATEX tag as stored.
    pub tag: u16,
    pub audio_codec: AudioCodec,
    /// FTR AAC only; None for other codecs.
    pub stream_layout: Option<StreamLayout>,
    /// `nChannels`: the number of microphone channels.
    pub channels: u16,
    /// Interleaved streams N (FTR AAC): `channels` / `channels_per_stream`.
    pub streams: u16,
    /// 1 for mono streams, 2 for stereo streams (FTR AAC).
    pub channels_per_stream: u16,
    /// From the first stream-0 ADTS header; None when no frame was found.
    pub sample_rate: Option<u32>,
    /// `strf` nSamplesPerSec.
    pub format_sample_rate: u32,
    /// `strh` dwScale and dwRate as stored (AAC: 1024 and the sample rate).
    pub header_scale: Option<u32>,
    pub header_rate: Option<u32>,
    /// `strh` dwLength: the number of audio chunks the recorder counted;
    /// None while it holds 0x7FFFFFFF (never closed).
    pub header_chunk_count: Option<u32>,
    /// `avih` dwStreams as stored; it can count an input with no `strl`.
    pub declared_streams: Option<u32>,
    /// `strl` lists actually present.
    pub stream_lists: u32,
    /// `avih` flag 0x20000000: no earlier file may be joined to this one.
    pub start_boundary: bool,
    /// `avih` flag 0x40000000: no later file may be joined to this one.
    pub end_boundary: bool,
    /// The audio length field says the recorder never closed the file.
    pub open_ended: bool,
    /// `TUTC` as RFC 3339 UTC (`…Z`); None without the chunk.
    pub start_utc: Option<String>,
    /// `TLOC` as a local date and time with no offset; None without it.
    pub start_local: Option<String>,
    /// Which time gives the start in `startTime` of the export record.
    pub start_basis: Option<StartBasis>,
    /// The `FILE` chunk: the name the recorder gave this file, only when
    /// it is a name in the recorder's own format (see
    /// [`recorder_file_name`]); any other text is never copied here.
    pub recorded_file_name: Option<String>,
    /// A `FILE` chunk holds text, whether or not it is kept above.
    pub recorded_file_name_present: bool,
    /// A `USER` chunk with a non-empty user name (the name is never read
    /// into DepoAudio's records, logs or messages).
    pub user_name_present: bool,
    /// A `UDAT` chunk with non-empty text (never read into DepoAudio's
    /// records, logs or messages).
    pub user_data_present: bool,
    /// `SIGN`/`CERT` chunks after the index: "signed (not verified)".
    pub signature: Option<&'static str>,
    pub index_kind: IndexKind,
    /// Audio chunks read.
    pub packets: u64,
    /// Frames delivered to the streams (whole rounds only).
    pub frames: u64,
    /// Frames each stream received (one per round).
    pub frames_per_channel: u64,
    /// Delivered frames that needed descrambling.
    pub scrambled_frames: u64,
    /// Frames that start in one audio chunk and end in a later one.
    pub straddling_frames: u64,
    /// Audio chunks that do not start and end on a stream-0 round boundary.
    pub misaligned_packets: u64,
    /// Audio chunks that hold more than one whole round of frames.
    pub multi_round_packets: u64,
    /// Bytes skipped before the first stream-0 frame.
    pub resync_skipped_bytes: u64,
    /// Times the stream order broke after the first stream-0 frame (a
    /// stream-0 frame where another stream's was due, or the reverse) and
    /// dealing restarted at the next stream-0 frame. The round delivered
    /// just before a restart can hold a frame in the wrong slot.
    pub channel_resyncs: u64,
    /// Unreadable bytes skipped after the first stream-0 frame, before the
    /// last frame delivered.
    pub mid_stream_skipped_bytes: u64,
    /// Zero bytes skipped between whole rounds or after the last one.
    pub padding_bytes: u64,
    /// Non-zero bytes after the last frame delivered that are not a frame.
    pub trailing_skipped_bytes: u64,
    /// The last round was missing streams; its frames were not delivered.
    pub incomplete_final_round: bool,
    /// Frames read but not delivered (incomplete rounds, or frames out of
    /// stream order).
    pub dropped_frames: u64,
    /// Partial rounds discarded before the last one delivered. They are not
    /// filled with silence, so each moves all later audio one round (1024
    /// samples) earlier than its time from the start.
    pub dropped_rounds: u64,
    /// `dropped_rounds` as seconds at the sample rate: the least that later
    /// audio is earlier than its time from the start. None without a rate.
    pub timeline_shift_secs: Option<f64>,
    /// Damaged stretches of `movi` (an unreadable chunk header, a zero gap,
    /// a chunk size past the end) that the walk skipped to reach the next
    /// audio chunk, and the bytes skipped. The audio they held is lost and
    /// its length is not known.
    pub walk_gaps: u64,
    pub walk_gap_bytes: u64,
    /// `idx1` does not list every audio chunk: `movi` holds audio chunks
    /// after the last one listed, or the count listed differs from the
    /// header's chunk count (`strh` dwLength, `FTRA` d). The chunks were
    /// then found by walking `movi` if that found more.
    pub index_mismatch: bool,
    /// Delivered frames with either of the two ADTS length bits FTR ignores
    /// (`b3 & 3`) set; they are cleared in the stream files.
    pub length_high_bit_frames: u64,
    /// Delivered frames whose ADTS channel configuration is not the
    /// channels per stream the tag declares.
    pub frame_channel_mismatches: u64,
    /// The file ends inside a chunk or a frame, or its chunk list stops at
    /// unreadable data.
    pub truncated: bool,
    /// RIFF `AVIX` (OpenDML) segments after the first RIFF. DepoAudio does
    /// not read their audio: every count here covers the first segment only,
    /// and the duration is not given.
    pub continuation_segments: u32,
    /// The file grew between the inspection and the stream split; only the
    /// part inspected was read.
    pub grew_while_read: bool,
    pub ftra: Option<FtraInfo>,
    pub video_streams: u32,
    /// Delivered frames per stream × 1024 samples ÷ sample rate.
    pub duration_secs: Option<f64>,
    /// The file length this read covered; a later stream split reads no
    /// further. Not part of the export record.
    #[serde(skip)]
    pub file_bytes: u64,
    /// The start FILETIME behind `start_basis`. Not part of the export
    /// record (`startUtc`/`startLocal` are).
    #[serde(skip)]
    pub start_filetime: Option<u64>,
}

impl TrmInfo {
    /// Whether FFmpeg's native `ftr` decoder can be expected to read this
    /// file as it is: the reasons it cannot, empty when it can. The native
    /// decoder takes each audio chunk as exactly one round starting at
    /// stream 0, does not join frames across chunks, look for the stream-0
    /// marker or skip padding, and reads the 13-bit ADTS length. It decoded
    /// both the mono-stream (0x4180) and the stereo-stream (0x8180) layouts
    /// in channel order in DepoAudio's tests (FFmpeg 6.1.1).
    pub(crate) fn native_blockers(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.audio_codec != AudioCodec::FtrAac {
            reasons.push("not-ftr-aac");
            return reasons;
        }
        if self.index_kind == IndexKind::Walked {
            reasons.push("no-usable-index");
        }
        if self.index_mismatch {
            reasons.push("index-misses-audio-chunks");
        }
        if self.open_ended {
            reasons.push("open-ended");
        }
        if self.truncated {
            reasons.push("truncated");
        }
        if self.straddling_frames > 0 {
            reasons.push("frames-cross-chunks");
        }
        if self.misaligned_packets > 0 {
            reasons.push("chunks-not-on-round-boundaries");
        }
        if self.multi_round_packets > 0 {
            reasons.push("several-rounds-per-chunk");
        }
        if self.resync_skipped_bytes > 0 {
            reasons.push("data-before-channel-0");
        }
        if self.channel_resyncs > 0 {
            reasons.push("channel-order-resynced");
        }
        if self.mid_stream_skipped_bytes > 0 || self.trailing_skipped_bytes > 0 {
            reasons.push("unreadable-data");
        }
        if self.padding_bytes > 0 {
            reasons.push("zero-padding");
        }
        if self.dropped_frames > 0 {
            reasons.push("incomplete-rounds");
        }
        if self.length_high_bit_frames > 0 {
            reasons.push("frame-length-high-bits");
        }
        if self.frame_channel_mismatches > 0 {
            reasons.push("frame-channels-differ-from-header");
        }
        reasons
    }

    /// The file's start as samples since midnight at the source sample
    /// rate, from `TLOC` (local, as a BWF time reference counts it), else
    /// `TUTC`, else the `FILE` name. Only a `TLOC` start is written into an
    /// output's bext chunk (`SourceTiming::is_local_time_of_day`); the
    /// others stay in the export record. None without a start or a rate.
    pub(crate) fn start_timing(&self) -> Option<crate::manifest::SourceTiming> {
        let filetime = self.start_filetime?;
        let basis = self.start_basis?;
        let rate = Some(self.sample_rate.unwrap_or(self.format_sample_rate)).filter(|rate| *rate > 0)?;
        let ticks = filetime % FILETIME_TICKS_PER_DAY;
        let samples = (u128::from(ticks) * u128::from(rate) + 5_000_000) / 10_000_000;
        Some(crate::manifest::SourceTiming {
            time_reference_samples: u64::try_from(samples).ok()?,
            sample_rate: rate,
            source: match basis {
                StartBasis::Local => "ftr-tloc",
                StartBasis::Utc => "ftr-tutc",
                StartBasis::FileName => "ftr-file-name",
            },
        })
    }
}

/// How FFmpeg reads a TRM recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum DecodePath {
    /// The file as it is, with FFmpeg's native `ftr` decoder.
    #[serde(rename = "native-ftr")]
    NativeFtr,
    /// One mono (or stereo) ADTS file per stream, written by
    /// [`deinterleave_trm`] and joined in channel order with `amerge`.
    #[serde(rename = "deinterleaved")]
    Deinterleaved,
    /// Not FTR AAC (MP2, MP3, WMA): the file as it is, with the decoder
    /// FFmpeg picks for that codec.
    #[serde(rename = "ffmpeg-plain")]
    Plain,
}

/// The inspection and the route chosen from it, as the export record and the
/// Convert queue report them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrmRoute {
    #[serde(flatten)]
    pub info: TrmInfo,
    pub decode_path: DecodePath,
    /// Why the native decoder was not used; empty when it was.
    pub route_reasons: Vec<String>,
    /// The options FFmpeg is given before each `-i`: `-c:a ftr` before the
    /// recording itself, `-f aac` before each temporary stream file, or
    /// nothing for a codec FFmpeg reads as it is.
    pub input_options: Vec<String>,
    /// How many `-i` inputs FFmpeg reads: 1, or one per stream (joined in
    /// channel order by the `amerge` step that opens the filter graph).
    pub input_count: u32,
}

impl TrmRoute {
    pub(crate) fn new(info: TrmInfo, decode_path: DecodePath, route_reasons: Vec<String>) -> Self {
        let (input_options, input_count) = match decode_path {
            DecodePath::NativeFtr => (vec!["-c:a".to_string(), "ftr".to_string()], 1),
            DecodePath::Deinterleaved => (vec!["-f".to_string(), "aac".to_string()], u32::from(info.streams)),
            DecodePath::Plain => (Vec::new(), 1),
        };
        Self {
            info,
            decode_path,
            route_reasons,
            input_options,
            input_count,
        }
    }
}

/// Native `ftr` when the layout allows it and the decoder is present and has
/// not already failed on this file; otherwise the streams are split first.
/// A codec other than FTR AAC goes to FFmpeg as it is.
pub(crate) fn choose_route(info: &TrmInfo, options: crate::helpers::FeedOptions) -> (DecodePath, Vec<String>) {
    let mut reasons: Vec<String> = info.native_blockers().into_iter().map(String::from).collect();
    if info.audio_codec != AudioCodec::FtrAac {
        return (DecodePath::Plain, reasons);
    }
    if !options.native_ftr {
        reasons.push("native-decoder-unavailable".into());
    }
    if options.native_failed {
        reasons.push("native-decode-failed".into());
    }
    if reasons.is_empty() {
        (DecodePath::NativeFtr, reasons)
    } else {
        (DecodePath::Deinterleaved, reasons)
    }
}

/// The message for an FTR recording holding MP2, MP3 or WMA audio that
/// FFmpeg could not decode as it is.
pub(crate) fn plain_decode_error(info: &TrmInfo, error: &str) -> String {
    format!(
        "This FTR recording holds {} audio (format tag 0x{:04X}), which DepoAudio hands to FFmpeg as ordinary audio, and FFmpeg could not decode it: {error}",
        info.audio_codec.label(),
        info.tag
    )
}

/// A FILETIME (100 ns since 1601-01-01) as a date and time: `…Z` when `utc`,
/// with no offset otherwise. Whole seconds, plus seven fraction digits when
/// the time has any. None outside what `time` can hold.
fn filetime_text(filetime: u64, utc: bool) -> Option<String> {
    let since_unix = i128::from(filetime) - i128::from(FILETIME_UNIX_EPOCH);
    let time = time::OffsetDateTime::from_unix_timestamp_nanos(since_unix * 100).ok()?;
    let ticks = filetime % 10_000_000;
    let fraction = if ticks == 0 {
        String::new()
    } else {
        format!(".{ticks:07}")
    };
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{fraction}{}",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        if utc { "Z" } else { "" }
    ))
}

/// A `FILE` chunk's text when it is a file name in the recorder's format,
/// `<name>_YYYYMMDD-HHMM_<16 hex>.<ext>`: a name part of at most 28
/// characters, each a letter, a digit or one of ``!$`() +,-.=[]^'{}``, with no
/// space at either end; the FILETIME as 16 lowercase hex digits (high then
/// low dword); the date and time written from that same FILETIME; and an
/// extension of 1 to 8 ASCII letters and digits. Returns the name and the
/// FILETIME, or None for any other text.
fn recorder_file_name(text: &str) -> Option<(String, u64)> {
    let (stem, extension) = text.rsplit_once('.')?;
    if extension.is_empty() || extension.len() > 8 || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let (rest, hex) = stem.rsplit_once('_')?;
    let (name, stamp) = rest.rsplit_once('_')?;
    if name.chars().count() > FILE_NAME_PART_MAX_CHARS
        || name.trim() != name
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || FILE_NAME_PART_EXTRA_CHARS.contains(c))
    {
        return None;
    }
    if hex.len() != 16 || !hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let filetime = u64::from_str_radix(hex, 16).ok().filter(|value| *value > 0)?;
    // The recorder writes the date and time from the same FILETIME, with no
    // zone conversion: YYYYMMDD-HHMM.
    let written = filetime_text(filetime, false)?;
    let expected = format!(
        "{}{}{}-{}{}",
        &written[0..4],
        &written[5..7],
        &written[8..10],
        &written[11..13],
        &written[14..16]
    );
    (stamp == expected).then(|| (text.to_string(), filetime))
}

/// UTF-16LE text up to its first NUL.
fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

// ── Little-endian helpers ────────────────────────────────────────────────────

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset.checked_add(2)?)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset.checked_add(4)?)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn fourcc_at(bytes: &[u8], offset: usize) -> Option<[u8; 4]> {
    bytes
        .get(offset..offset.checked_add(4)?)
        .map(|b| [b[0], b[1], b[2], b[3]])
}

fn io_error(error: std::io::Error) -> String {
    format!("Could not read the FTR recording: {error}")
}

/// Sub-chunks of an in-memory RIFF list body. A chunk that claims more bytes
/// than remain is cut at the end of the buffer.
fn sub_chunks(body: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut chunks = Vec::new();
    let mut pos = 0usize;
    while let (Some(id), Some(size)) = (fourcc_at(body, pos), u32_at(body, pos + 4)) {
        let start = pos + 8;
        let end = start.saturating_add(size as usize).min(body.len());
        chunks.push((id, &body[start..end]));
        let padded = (size as usize).saturating_add(size as usize & 1);
        pos = match start.checked_add(padded) {
            Some(next) if next > pos => next,
            _ => break,
        };
    }
    chunks
}

// ── Reading the file ─────────────────────────────────────────────────────────

#[cfg(test)]
thread_local! {
    /// Bytes this thread has read from recordings through [`SourceReader`].
    static SOURCE_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The recording, read through a counter in tests.
struct Source(File);

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.0.read(buf)?;
        #[cfg(test)]
        SOURCE_BYTES_READ.with(|count| count.set(count.get() + read as u64));
        Ok(read)
    }
}

impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

/// A buffered reader that keeps track of its own position, so moving to a
/// position inside the buffer (the next chunk header, the payload right after
/// it) reuses the buffer instead of discarding it.
struct SourceReader {
    inner: BufReader<Source>,
    pos: u64,
}

impl SourceReader {
    fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            inner: BufReader::with_capacity(READ_PIECE_BYTES, Source(File::open(path).map_err(io_error)?)),
            pos: 0,
        })
    }

    fn seek_to(&mut self, target: u64) -> Result<(), String> {
        if target != self.pos {
            // Positions stay far below i64::MAX: they are offsets in a file.
            let delta = target.wrapping_sub(self.pos) as i64;
            self.inner.seek_relative(delta).map_err(io_error)?;
            self.pos = target;
        }
        Ok(())
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), String> {
        self.inner.read_exact(buf).map_err(io_error)?;
        self.pos += buf.len() as u64;
        Ok(())
    }
}

// ── Container header ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct Layout {
    tag: u16,
    codec: AudioCodec,
    stream_layout: Option<StreamLayout>,
    channels: u16,
    streams: u16,
    channels_per_stream: u16,
    format_sample_rate: u32,
    header_scale: Option<u32>,
    header_rate: Option<u32>,
    header_length: Option<u32>,
    declared_streams: Option<u32>,
    stream_lists: u32,
    start_boundary: bool,
    end_boundary: bool,
    open_ended: bool,
    tutc: Option<u64>,
    tloc: Option<u64>,
    recorded_file_name: Option<String>,
    recorded_file_name_present: bool,
    file_name_filetime: Option<u64>,
    user_name_present: bool,
    user_data_present: bool,
    signed: bool,
    ftra: Option<FtraInfo>,
    video_streams: u32,
    /// File offset of the `movi` list type, which idx1 offsets count from.
    movi_pos: u64,
    /// End of the `movi` list data (EOF when its size is unusable).
    movi_end: u64,
    /// `idx1` data start and usable byte count.
    idx1: Option<(u64, u64)>,
    continuation_segments: u32,
    truncated: bool,
}

struct StreamHeader {
    kind: [u8; 4],
    scale: Option<u32>,
    rate: Option<u32>,
    length: Option<u32>,
    format: Vec<u8>,
}

fn parse_header_list(body: &[u8], layout: &mut Layout) -> Result<(), String> {
    let mut flags = None;
    let mut streams: Vec<StreamHeader> = Vec::new();
    for (id, data) in sub_chunks(body) {
        match &id {
            b"avih" => {
                flags = u32_at(data, 12);
                layout.declared_streams = u32_at(data, 24);
            }
            b"LIST" if data.get(..4) == Some(b"strl") => {
                let mut stream = StreamHeader {
                    kind: [0; 4],
                    scale: None,
                    rate: None,
                    length: None,
                    format: Vec::new(),
                };
                for (inner, inner_data) in sub_chunks(&data[4..]) {
                    match &inner {
                        b"strh" => {
                            stream.kind = fourcc_at(inner_data, 0).unwrap_or([0; 4]);
                            stream.scale = u32_at(inner_data, 20);
                            stream.rate = u32_at(inner_data, 24);
                            stream.length = u32_at(inner_data, 32);
                        }
                        b"strf" => stream.format = inner_data.iter().take(64).copied().collect(),
                        _ => {}
                    }
                }
                streams.push(stream);
            }
            _ => {}
        }
    }
    let flags = flags.ok_or("This FTR recording has no AVI main header (avih), so it cannot be read.")?;
    layout.start_boundary = flags & START_BOUNDARY_FLAG != 0;
    layout.end_boundary = flags & END_BOUNDARY_FLAG != 0;
    // The strl lists are what counts: avih dwStreams can include an input
    // (the level meter) that has none.
    layout.stream_lists = streams.len() as u32;
    layout.video_streams = streams.iter().filter(|stream| &stream.kind == b"vids").count() as u32;
    let audio = streams
        .iter()
        .find(|stream| &stream.kind == b"auds")
        .ok_or("This FTR recording has no audio stream header.")?;
    let (Some(tag), Some(channels)) = (u16_at(&audio.format, 0), u16_at(&audio.format, 2)) else {
        return Err("This FTR recording's audio format header is too short to read.".into());
    };
    layout.tag = tag;
    layout.channels = channels;
    layout.format_sample_rate = u32_at(&audio.format, 4).unwrap_or(0);
    layout.header_scale = audio.scale;
    layout.header_rate = audio.rate;
    layout.header_length = audio.length.filter(|length| *length != OPEN_ENDED_LENGTH);
    layout.open_ended = audio.length == Some(OPEN_ENDED_LENGTH);
    Ok(())
}

/// Read up to `limit` bytes of a top-level chunk's data.
fn read_chunk_data(file: &mut File, data: u64, size: u64, remaining: u64, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; size.min(remaining).min(limit) as usize];
    file.seek(SeekFrom::Start(data)).map_err(io_error)?;
    file.read_exact(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

/// Whether a chunk holds any text (any non-zero byte), wherever in the
/// chunk it is: the whole chunk is read in pieces and the bytes discarded.
/// Only this is kept.
fn holds_text(file: &mut File, data: u64, size: u64, remaining: u64) -> Result<bool, String> {
    file.seek(SeekFrom::Start(data)).map_err(io_error)?;
    let mut left = size.min(remaining);
    let mut piece = vec![0u8; READ_PIECE_BYTES];
    while left > 0 {
        let take = left.min(piece.len() as u64) as usize;
        file.read_exact(&mut piece[..take]).map_err(io_error)?;
        if piece[..take].iter().any(|byte| *byte != 0) {
            return Ok(true);
        }
        left -= take as u64;
    }
    Ok(false)
}

/// The streams the tag's layout bits make of `nChannels`, as FTR's splitter
/// counts them, or the refusal.
fn stream_layout(tag: u16, channels: u16) -> Result<(StreamLayout, u16, u16), String> {
    let mono = tag & MONO_STREAMS_FLAG != 0;
    let stereo = tag & STEREO_STREAMS_FLAG != 0;
    match (mono, stereo) {
        (true, false) => Ok((StreamLayout::Mono, channels, 1)),
        (false, true) => {
            let per_stream = channels.min(2);
            if per_stream == 0 || !channels.is_multiple_of(per_stream) {
                return Err(TRM_UNSPLITTABLE_MESSAGE.into());
            }
            Ok((StreamLayout::Stereo, channels / per_stream, per_stream))
        }
        _ => Err(TRM_UNSPLITTABLE_MESSAGE.into()),
    }
}

fn read_layout(file: &mut File, file_len: u64) -> Result<Layout, String> {
    let mut riff = [0u8; 12];
    if file_len < 12 {
        return Err("This file is too short to be an FTR recording.".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    file.read_exact(&mut riff).map_err(io_error)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"AVI " {
        return Err("This file does not start like an FTR recording (RIFF AVI), so DepoAudio cannot read it.".into());
    }
    let mut layout = Layout::default();
    let mut have_header = false;
    let mut have_movi = false;
    let mut pos = 12u64;
    while pos + 8 <= file_len {
        let mut head = [0u8; 12];
        file.seek(SeekFrom::Start(pos)).map_err(io_error)?;
        let available = (file_len - pos).min(12) as usize;
        file.read_exact(&mut head[..available]).map_err(io_error)?;
        let id = [head[0], head[1], head[2], head[3]];
        let size = u64::from(u32::from_le_bytes([head[4], head[5], head[6], head[7]]));
        let data = pos + 8;
        let remaining = file_len - data;
        // Past idx1 the recorder appends only `peak`, `SIGN` and `CERT`
        // (and an OpenDML file its AVIX segments): nothing there is audio
        // DepoAudio reads, so nothing there counts as truncation either.
        let after_index = layout.idx1.is_some();
        match &id {
            _ if after_index && (&id == b"SIGN" || &id == b"CERT") => layout.signed = true,
            b"RIFF" if available == 12 && &head[8..12] == b"AVIX" => {
                layout.continuation_segments += 1;
            }
            _ if after_index => {}
            b"LIST" if available == 12 && &head[8..12] == b"hdrl" => {
                if size > MAX_HEADER_LIST_BYTES {
                    return Err(
                        "This FTR recording's header list is implausibly large; the file may be damaged.".into(),
                    );
                }
                let body = read_chunk_data(file, data, size, remaining, MAX_HEADER_LIST_BYTES)?;
                parse_header_list(body.get(4..).unwrap_or_default(), &mut layout)?;
                have_header = true;
            }
            b"LIST" if available == 12 && &head[8..12] == b"movi" && !have_movi => {
                layout.movi_pos = data;
                have_movi = true;
                if size < 4 || size > remaining {
                    // Never closed or cut short: the written audio runs to EOF.
                    if size > remaining {
                        layout.truncated = true;
                    }
                    layout.movi_end = file_len;
                    break;
                }
                layout.movi_end = data + size;
            }
            b"FTRA" if size >= 20 && remaining >= 20 => {
                let values = read_chunk_data(file, data, 20, remaining, 20)?;
                let value = |index: usize| u32_at(&values, index * 4).unwrap_or(0);
                layout.ftra = Some(FtraInfo {
                    a: value(0),
                    b: value(1),
                    c: value(2),
                    d: value(3),
                    e: value(4),
                });
            }
            b"TUTC" | b"TLOC" if size >= 8 && remaining >= 8 => {
                let bytes = read_chunk_data(file, data, 8, remaining, 8)?;
                let value = u64::from_le_bytes(bytes.try_into().unwrap_or_default());
                let value = Some(value).filter(|value| *value > 0);
                if &id == b"TUTC" {
                    layout.tutc = value;
                } else {
                    layout.tloc = value;
                }
            }
            b"FILE" => {
                // Kept only when it is a name in the recorder's own format;
                // any other text (which could be anything) only as present.
                let bytes = read_chunk_data(file, data, size, remaining, FILE_NAME_CHUNK_BYTES)?;
                if let Some((name, filetime)) = recorder_file_name(&utf16_text(&bytes)) {
                    layout.recorded_file_name = Some(name);
                    layout.file_name_filetime = Some(filetime);
                }
                layout.recorded_file_name_present |= holds_text(file, data, size, remaining)?;
            }
            // Any chunk of either kind that holds text counts, wherever the
            // text is: a later empty duplicate does not hide an earlier one.
            b"USER" => layout.user_name_present |= holds_text(file, data, size, remaining)?,
            b"UDAT" => layout.user_data_present |= holds_text(file, data, size, remaining)?,
            b"idx1" => {
                if size == 0 {
                    // Never closed: the index runs to EOF, and nothing after
                    // it can be read as a chunk.
                    layout.idx1 = Some((data, remaining));
                    break;
                }
                layout.idx1 = Some((data, size.min(remaining)));
                if size > remaining {
                    layout.truncated = true;
                }
            }
            _ => {}
        }
        let next = data + size + (size & 1);
        if next > file_len {
            if !after_index && !matches!(&id, b"idx1") {
                layout.truncated = true;
            }
            break;
        }
        pos = next;
    }
    if !have_header {
        return Err("This FTR recording has no AVI header list (hdrl), so it cannot be read.".into());
    }
    if !have_movi {
        return Err("This FTR recording has no audio data list (movi); the file may be empty or damaged.".into());
    }
    let Some(codec) = AudioCodec::from_tag(layout.tag) else {
        return Err(format!(
            "This recording's audio is not FTR AAC, MP2, MP3 or WMA (format tag 0x{:04X}), so DepoAudio cannot read it as an FTR recording.",
            layout.tag
        ));
    };
    layout.codec = codec;
    if layout.channels == 0 || layout.channels > MAX_TRM_CHANNELS {
        return Err(format!(
            "This FTR recording reports {} channels; DepoAudio reads 1 to {MAX_TRM_CHANNELS}. The header may be damaged.",
            layout.channels
        ));
    }
    if codec == AudioCodec::FtrAac {
        let (kind, streams, per_stream) = stream_layout(layout.tag, layout.channels)?;
        layout.stream_layout = Some(kind);
        layout.streams = streams;
        layout.channels_per_stream = per_stream;
    }
    Ok(layout)
}

// ── ADTS frames ──────────────────────────────────────────────────────────────

/// FTR's frame test: ADTS sync, layer 0, AAC-LC, no home/copyright bits.
fn is_ftr_frame_header(header: &[u8]) -> bool {
    header.len() >= ADTS_HEADER_BYTES
        && header[0] == 0xFF
        && header[1] & 0xF6 == 0xF0
        && header[2] & 0xC0 == 0x40
        && header[3] & 0x1C == 0
}

/// Frame length from the low 11 bits FTR reads (frames stay under 2048).
fn ftr_frame_length(header: &[u8]) -> usize {
    ((usize::from(header[4]) << 8) | usize::from(header[5])) >> 5
}

fn adts_sample_rate(header: &[u8]) -> Option<u32> {
    ADTS_SAMPLE_RATES.get(usize::from((header[2] >> 2) & 0x0F)).copied()
}

/// Undo (or apply: it is its own inverse) FTR's scrambling of a frame whose
/// original/copy bit (`b3 & 0x20`) is set. Returns whether it changed bytes.
pub(crate) fn descramble_frame(frame: &mut [u8]) -> bool {
    if frame.len() < SCRAMBLE_MIN_FRAME_BYTES || frame[3] & 0x20 == 0 {
        return false;
    }
    let t = frame[8];
    frame[9] = !frame[9];
    frame[11] = !frame[11];
    frame[12] = !frame[12];
    frame[8] = !frame[10];
    frame[10] = !t;
    true
}

/// Receives each delivered frame with its channel index.
type FrameSink<'a> = dyn FnMut(usize, &[u8]) -> Result<(), String> + 'a;

/// ADTS channel configuration (1 = mono, 2 = stereo).
fn adts_channel_config(header: &[u8]) -> u8 {
    ((header[2] & 0x01) << 2) | (header[3] >> 6)
}

/// Deals frames from the joined chunk payloads to streams in rounds.
struct Dealer<'a> {
    channels: usize,
    /// The ADTS channel configuration each frame should carry.
    expected_config: u8,
    buffer: Vec<u8>,
    /// A channel-0 frame has been seen: skipped data is no longer leading.
    synced: bool,
    /// The channel order broke and no channel-0 frame has restarted it yet.
    resyncing: bool,
    /// A frame after channel 0 in a round has carried the private bit, so
    /// this file marks the other channels and a clear bit there means a new
    /// round. Until then only the round's first frame is checked.
    marks_others: bool,
    round: Vec<u8>,
    round_ends: Vec<usize>,
    round_scrambled: u64,
    round_high_bits: u64,
    round_config_mismatches: u64,
    /// Skipped bytes not yet attributed (leading, padding, mid-stream or
    /// trailing): that depends on what follows them.
    pending_skip: u64,
    pending_nonzero: bool,
    /// The pending skip began inside a round (and cost that round).
    pending_mid_round: bool,
    /// The bytes left at the last chunk boundary start a frame begun there.
    carry_pending: bool,
    packet_start_ok: bool,
    packet_rounds_start: u64,
    /// Stop once this many rounds are delivered.
    round_limit: Option<u64>,
    limit_reached: bool,
    sample_rate: Option<u32>,
    stats: DealStats,
    sink: Option<&'a mut FrameSink<'a>>,
}

#[derive(Debug, Default, Clone)]
struct DealStats {
    packets: u64,
    rounds: u64,
    scrambled: u64,
    straddling: u64,
    misaligned: u64,
    multi_round: u64,
    resync_skipped: u64,
    channel_resyncs: u64,
    mid_stream_skipped: u64,
    padding: u64,
    trailing_skipped: u64,
    dropped: u64,
    /// Partial rounds discarded before the end: each is at least one
    /// 1024-sample period missing on every stream.
    lost_rounds: u64,
    /// Gaps in the `movi` walk that were skipped to the next audio chunk.
    walk_gaps: u64,
    walk_gap_bytes: u64,
    high_bits: u64,
    config_mismatches: u64,
    incomplete_final_round: bool,
    cut_final_frame: bool,
}

impl<'a> Dealer<'a> {
    fn new(layout: &Layout, sink: Option<&'a mut FrameSink<'a>>, round_limit: Option<u64>) -> Self {
        let channels = usize::from(layout.streams);
        Self {
            channels,
            expected_config: layout.channels_per_stream as u8,
            buffer: Vec::with_capacity(READ_PIECE_BYTES + 4096),
            synced: false,
            resyncing: false,
            marks_others: false,
            round: Vec::new(),
            round_ends: Vec::with_capacity(channels),
            round_scrambled: 0,
            round_high_bits: 0,
            round_config_mismatches: 0,
            pending_skip: 0,
            pending_nonzero: false,
            pending_mid_round: false,
            carry_pending: false,
            packet_start_ok: true,
            packet_rounds_start: 0,
            round_limit,
            limit_reached: false,
            sample_rate: None,
            stats: DealStats::default(),
            sink,
        }
    }

    fn begin_packet(&mut self) {
        self.packet_start_ok = self.buffer.is_empty() && self.round_ends.is_empty() && self.pending_skip == 0;
        self.packet_rounds_start = self.stats.rounds;
    }

    fn end_packet(&mut self) -> Result<(), String> {
        self.process()?;
        if self.limit_reached {
            return Ok(());
        }
        self.stats.packets += 1;
        if self.stats.rounds - self.packet_rounds_start > 1 {
            self.stats.multi_round += 1;
        }
        // Zero bytes after a whole round, to the end of the chunk: padding.
        if self.synced
            && self.round_ends.is_empty()
            && !self.pending_nonzero
            && !self.pending_mid_round
            && self.buffer.iter().all(|byte| *byte == 0)
        {
            self.stats.padding += self.pending_skip + self.buffer.len() as u64;
            self.pending_skip = 0;
            self.buffer.clear();
        }
        let end_ok = self.buffer.is_empty() && self.round_ends.is_empty() && self.pending_skip == 0 && self.synced;
        if !(self.packet_start_ok && end_ok) {
            self.stats.misaligned += 1;
        }
        self.carry_pending = !self.buffer.is_empty();
        Ok(())
    }

    fn feed(&mut self, data: &[u8]) -> Result<(), String> {
        self.buffer.extend_from_slice(data);
        self.process()
    }

    /// Discard a partial round mid-recording: its period is missing from
    /// every stream, so later audio moves earlier by one round.
    fn drop_round(&mut self) {
        if !self.round_ends.is_empty() {
            self.stats.lost_rounds += 1;
        }
        self.clear_round();
    }

    fn clear_round(&mut self) {
        self.stats.dropped += self.round_ends.len() as u64;
        self.round.clear();
        self.round_ends.clear();
        self.round_scrambled = 0;
        self.round_high_bits = 0;
        self.round_config_mismatches = 0;
    }

    /// The walk skipped `bytes` of unreadable data to reach the next audio
    /// chunk: a frame begun before the gap cannot be completed after it, so
    /// its bytes and the round in progress are given up.
    fn gap(&mut self, bytes: u64) {
        self.stats.walk_gaps += 1;
        self.stats.walk_gap_bytes += bytes;
        for byte in std::mem::take(&mut self.buffer) {
            self.skip_byte(byte);
        }
        self.carry_pending = false;
        self.drop_round();
    }

    fn skip_byte(&mut self, byte: u8) {
        if self.pending_skip == 0 {
            self.pending_nonzero = false;
            self.pending_mid_round = !self.round_ends.is_empty();
        }
        self.pending_skip += 1;
        self.pending_nonzero |= byte != 0;
    }

    /// Attribute skipped bytes once a frame follows them.
    fn settle_pending(&mut self) {
        if self.pending_skip == 0 {
            return;
        }
        if !self.synced {
            self.stats.resync_skipped += self.pending_skip;
        } else if !self.pending_nonzero && !self.pending_mid_round {
            self.stats.padding += self.pending_skip;
        } else {
            self.stats.mid_stream_skipped += self.pending_skip;
        }
        self.pending_skip = 0;
        self.pending_nonzero = false;
        self.pending_mid_round = false;
    }

    fn process(&mut self) -> Result<(), String> {
        let mut index = 0usize;
        while !self.limit_reached && self.buffer.len() - index >= ADTS_HEADER_BYTES {
            let at_carry = index == 0 && self.carry_pending;
            let header = &self.buffer[index..index + ADTS_HEADER_BYTES];
            let length = ftr_frame_length(header);
            if !is_ftr_frame_header(header) || length < ADTS_HEADER_BYTES {
                // Unreadable data: give up the round in progress and look for
                // the next channel-0 frame, byte by byte.
                let byte = self.buffer[index];
                self.skip_byte(byte);
                self.drop_round();
                index += 1;
                if at_carry {
                    self.carry_pending = false;
                }
                continue;
            }
            if self.buffer.len() - index < length {
                break;
            }
            if at_carry {
                self.carry_pending = false;
            }
            let channel_zero = self.buffer[index + 2] & 0x02 == 0;
            let round_start = self.round_ends.is_empty();
            let out_of_order = if round_start {
                !channel_zero
            } else {
                channel_zero && self.marks_others
            };
            if out_of_order {
                if !self.synced {
                    // Frames of other channels before the first channel 0.
                    self.pending_skip += length as u64;
                    self.pending_nonzero = true;
                    index += length;
                    continue;
                }
                if !self.resyncing {
                    self.stats.channel_resyncs += 1;
                    self.resyncing = true;
                }
                if channel_zero {
                    // A new round began before this one was complete.
                    self.drop_round();
                } else {
                    // Another channel's frame where channel 0's was due.
                    self.stats.dropped += 1;
                    index += length;
                    continue;
                }
            }
            self.settle_pending();
            self.synced = true;
            if !round_start && !channel_zero {
                self.marks_others = true;
            }
            if channel_zero && round_start {
                self.resyncing = false;
                if self.sample_rate.is_none() {
                    self.sample_rate = adts_sample_rate(&self.buffer[index..]);
                }
            }
            if at_carry {
                self.stats.straddling += 1;
            }
            let start = self.round.len();
            self.round.extend_from_slice(&self.buffer[index..index + length]);
            let frame = &mut self.round[start..];
            if descramble_frame(frame) {
                self.round_scrambled += 1;
            }
            if frame[3] & 0x03 != 0 {
                frame[3] &= !0x03;
                self.round_high_bits += 1;
            }
            if adts_channel_config(frame) != self.expected_config {
                self.round_config_mismatches += 1;
            }
            self.round_ends.push(self.round.len());
            index += length;
            if self.round_ends.len() == self.channels {
                self.emit_round()?;
            }
        }
        self.buffer.drain(..index);
        Ok(())
    }

    fn emit_round(&mut self) -> Result<(), String> {
        if let Some(sink) = self.sink.as_mut() {
            let mut start = 0usize;
            for (channel, &end) in self.round_ends.iter().enumerate() {
                sink(channel, &self.round[start..end])?;
                start = end;
            }
        }
        self.stats.rounds += 1;
        self.stats.scrambled += self.round_scrambled;
        self.stats.high_bits += self.round_high_bits;
        self.stats.config_mismatches += self.round_config_mismatches;
        self.round.clear();
        self.round_ends.clear();
        self.round_scrambled = 0;
        self.round_high_bits = 0;
        self.round_config_mismatches = 0;
        if self.round_limit.is_some_and(|limit| self.stats.rounds >= limit) {
            self.limit_reached = true;
        }
        Ok(())
    }

    fn finish(mut self) -> (DealStats, Option<u32>) {
        if !self.round_ends.is_empty() {
            self.stats.incomplete_final_round = true;
            self.clear_round();
        }
        // What is left is either the start of a frame the file cuts off, or
        // bytes after the last frame.
        let leftover = std::mem::take(&mut self.buffer);
        let cut_frame = leftover.first() == Some(&0xFF)
            && (leftover.len() < ADTS_HEADER_BYTES
                || (is_ftr_frame_header(&leftover) && ftr_frame_length(&leftover) > leftover.len()));
        if cut_frame {
            self.stats.cut_final_frame = true;
        } else {
            for byte in leftover {
                self.skip_byte(byte);
            }
        }
        if self.pending_skip > 0 {
            if !self.synced {
                self.stats.resync_skipped += self.pending_skip;
            } else if self.pending_nonzero {
                self.stats.trailing_skipped += self.pending_skip;
            } else {
                self.stats.padding += self.pending_skip;
            }
        }
        (self.stats, self.sample_rate)
    }
}

// ── Reading the audio chunks ─────────────────────────────────────────────────

type Cancel<'a> = Option<&'a (dyn Fn() -> bool + Send + Sync)>;

fn check_cancel(cancelled: Cancel<'_>) -> Result<(), String> {
    if cancelled.is_some_and(|check| check()) {
        Err(TRM_CANCELLED_MESSAGE.into())
    } else {
        Ok(())
    }
}

/// Feed `size` payload bytes at `data` to the dealer as one chunk, in pieces.
fn feed_payload(
    reader: &mut SourceReader,
    data: u64,
    size: u64,
    piece: &mut [u8],
    dealer: &mut Dealer<'_>,
    cancelled: Cancel<'_>,
) -> Result<(), String> {
    reader.seek_to(data)?;
    dealer.begin_packet();
    let mut left = size;
    while left > 0 && !dealer.limit_reached {
        check_cancel(cancelled)?;
        let take = left.min(piece.len() as u64) as usize;
        reader.read_exact(&mut piece[..take])?;
        dealer.feed(&piece[..take])?;
        left -= take as u64;
    }
    dealer.end_packet()
}

/// The index kinds worth trying, most likely first. As FFmpeg's AVI reader
/// does, the first entry of any type decides: an offset at or before the
/// `movi` list can only count from it. The other kind is tried next, in case
/// that entry misleads (a large video chunk first, say).
fn index_candidates(path: &Path, layout: &Layout) -> Result<Vec<IndexKind>, String> {
    let Some((idx_start, idx_len)) = layout.idx1 else {
        return Ok(Vec::new());
    };
    if idx_len < 16 {
        return Ok(Vec::new());
    }
    let mut reader = SourceReader::open(path)?;
    reader.seek_to(idx_start)?;
    let mut entry = [0u8; 16];
    reader.read_exact(&mut entry)?;
    let offset = u64::from(u32_at(&entry, 8).unwrap_or(0));
    Ok(if offset <= layout.movi_pos {
        vec![IndexKind::Idx1Relative, IndexKind::Idx1Absolute]
    } else {
        vec![IndexKind::Idx1Absolute, IndexKind::Idx1Relative]
    })
}

/// What reading the chunks `idx1` lists found.
struct IndexRead {
    /// The last chunk was cut off by EOF.
    truncated: bool,
    /// Audio chunks listed (and read).
    audio_chunks: u64,
    /// Where the chunk after the last audio chunk listed would start.
    after_last: u64,
}

/// Read every `00wb` chunk `idx1` lists, reading offsets as `kind` says and
/// checking each listed chunk header against the file before using it.
/// Returns None when the index does not match the file.
fn read_by_index(
    path: &Path,
    layout: &Layout,
    file_len: u64,
    kind: IndexKind,
    dealer: &mut Dealer<'_>,
    cancelled: Cancel<'_>,
) -> Result<Option<IndexRead>, String> {
    let Some((idx_start, idx_len)) = layout.idx1 else {
        return Ok(None);
    };
    let mut index = SourceReader::open(path)?;
    index.seek_to(idx_start)?;
    let mut payload = SourceReader::open(path)?;
    let mut piece = vec![0u8; READ_PIECE_BYTES];
    let mut last_end = layout.movi_pos;
    let mut after_last = layout.movi_pos + 4;
    let mut truncated = false;
    let mut entry = [0u8; 16];
    let mut audio_entries = 0u64;
    for entry_number in 0..idx_len / 16 {
        if entry_number.is_multiple_of(CANCEL_CHECK_EVERY) {
            check_cancel(cancelled)?;
        }
        if dealer.limit_reached {
            break;
        }
        index.read_exact(&mut entry)?;
        if &entry[0..4] != b"00wb" {
            continue;
        }
        let offset = u64::from(u32_at(&entry, 8).unwrap_or(0));
        let size = u64::from(u32_at(&entry, 12).unwrap_or(0));
        let header = match kind {
            IndexKind::Idx1Relative => layout.movi_pos.saturating_add(offset),
            _ => offset,
        };
        if header < last_end || header + 8 > file_len {
            return Ok(None);
        }
        let mut chunk = [0u8; 8];
        payload.seek_to(header)?;
        payload.read_exact(&mut chunk)?;
        if &chunk[0..4] != b"00wb" || u64::from(u32_at(&chunk, 4).unwrap_or(0)) != size {
            return Ok(None);
        }
        let data = header + 8;
        let available = size.min(file_len - data);
        if available < size {
            truncated = true;
        }
        audio_entries += 1;
        last_end = data + available;
        after_last = data + size + (size & 1);
        feed_payload(&mut payload, data, available, &mut piece, dealer, cancelled)?;
    }
    Ok((audio_entries > 0).then_some(IndexRead {
        truncated,
        audio_chunks: audio_entries,
        after_last,
    }))
}

/// Whether `movi` holds an audio chunk after `from` (the end of the last
/// one the index lists): chunk headers are stepped over to the end of
/// `movi`, payloads are not read, and the look stops at the first header
/// that is not a chunk.
fn audio_chunk_after(
    path: &Path,
    layout: &Layout,
    file_len: u64,
    from: u64,
    cancelled: Cancel<'_>,
) -> Result<bool, String> {
    let mut reader = SourceReader::open(path)?;
    let end = layout.movi_end.min(file_len);
    let mut pos = from;
    let mut steps = 0u64;
    while pos + 8 <= end {
        steps += 1;
        if steps.is_multiple_of(CANCEL_CHECK_EVERY) {
            check_cancel(cancelled)?;
        }
        let mut head = [0u8; 8];
        reader.seek_to(pos)?;
        reader.read_exact(&mut head)?;
        if &head[0..4] == b"00wb" {
            return Ok(true);
        }
        if !is_chunk_id(&head[0..4]) {
            return Ok(false);
        }
        if &head[0..4] == b"LIST" {
            pos += 12;
            continue;
        }
        let size = u64::from(u32_at(&head, 4).unwrap_or(0));
        pos = pos + 8 + size + (size & 1);
    }
    Ok(false)
}

fn is_chunk_id(id: &[u8]) -> bool {
    id != [0, 0, 0, 0] && id.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b' ')
}

/// How far past a damaged stretch of `movi` the walk looks for the next
/// audio chunk before it gives up.
const WALK_RESYNC_SCAN_BYTES: u64 = 4 * 1024 * 1024;

/// Look for the next audio chunk from `from` (inclusive) to `limit`: a
/// `00wb` header whose payload fits before `end` and opens with an FTR frame
/// header. Reads in pieces. Returns its position, and whether every byte
/// before it (or before `limit`, when none was found) was zero.
fn find_audio_chunk(
    reader: &mut SourceReader,
    from: u64,
    limit: u64,
    end: u64,
    cancelled: Cancel<'_>,
) -> Result<(Option<u64>, bool), String> {
    let mut all_zero = true;
    let mut base = from;
    let mut window: Vec<u8> = Vec::new();
    // 8 header bytes plus the 7 of the first frame header.
    const NEED: usize = 8 + ADTS_HEADER_BYTES;
    while base < limit {
        check_cancel(cancelled)?;
        let want = (end - (base + window.len() as u64)).min(READ_PIECE_BYTES as u64) as usize;
        if want == 0 && window.len() < NEED {
            break;
        }
        if want > 0 {
            let start = window.len();
            window.resize(start + want, 0);
            reader.seek_to(base + start as u64)?;
            reader.read_exact(&mut window[start..])?;
        }
        let mut index = 0usize;
        while index + NEED <= window.len() && base + (index as u64) < limit {
            let at = &window[index..];
            if &at[0..4] == b"00wb" {
                let size = u64::from(u32_at(at, 4).unwrap_or(0));
                let data = base + index as u64 + 8;
                if size >= ADTS_HEADER_BYTES as u64 && data + size <= end && is_ftr_frame_header(&at[8..NEED]) {
                    return Ok((Some(base + index as u64), all_zero));
                }
            }
            all_zero &= at[0] == 0;
            index += 1;
        }
        window.drain(..index);
        base += index as u64;
        if want == 0 {
            break;
        }
    }
    Ok((None, all_zero && window.iter().all(|byte| *byte == 0)))
}

/// Walk `movi` from its first chunk: skip each chunk (padded to even), read
/// `00wb` payloads and step into `LIST` sub-lists. At a damaged stretch (a
/// zero or unprintable chunk id, or a chunk claiming more than `movi`
/// holds) it looks up to [`WALK_RESYNC_SCAN_BYTES`] ahead, from the end of
/// the previous chunk's data (in case its pad byte is missing), for the
/// next audio chunk, and continues there, counting the gap. Zeros up to the
/// end of the look are the end of the written data. Returns whether the
/// walk ended early (cut-off chunk or unreadable data with no audio after).
fn read_by_walking(
    path: &Path,
    layout: &Layout,
    file_len: u64,
    dealer: &mut Dealer<'_>,
    cancelled: Cancel<'_>,
) -> Result<bool, String> {
    let mut reader = SourceReader::open(path)?;
    let mut piece = vec![0u8; READ_PIECE_BYTES];
    let end = layout.movi_end.min(file_len);
    let mut pos = layout.movi_pos + 4;
    // End of the previous chunk's data, before any pad byte.
    let mut data_end = pos;
    let mut steps = 0u64;
    while pos + 8 <= end && !dealer.limit_reached {
        steps += 1;
        if steps.is_multiple_of(CANCEL_CHECK_EVERY) {
            check_cancel(cancelled)?;
        }
        let mut head = [0u8; 8];
        reader.seek_to(pos)?;
        reader.read_exact(&mut head)?;
        let id = &head[0..4];
        let size = u64::from(u32_at(&head, 4).unwrap_or(0));
        let readable = is_chunk_id(id);
        if readable && id == b"LIST" {
            pos += 12;
            data_end = pos;
            continue;
        }
        let data = pos + 8;
        if !readable || data + size > end {
            let from = data_end.min(pos);
            let limit = from.saturating_add(WALK_RESYNC_SCAN_BYTES).min(end);
            let (next, all_zero) = find_audio_chunk(&mut reader, from, limit, end, cancelled)?;
            match next {
                Some(next) if next != pos => {
                    // Nothing skipped when the next chunk starts before
                    // `pos`: the previous chunk lacked its pad byte, and no
                    // audio was lost.
                    let skipped = next.saturating_sub(pos);
                    if skipped > 0 {
                        dealer.gap(skipped);
                    } else {
                        dealer.stats.walk_gaps += 1;
                    }
                    pos = next;
                    data_end = next;
                    continue;
                }
                // Zeros to the end of movi: the end of the written data.
                _ if !readable => return Ok(!(id == [0, 0, 0, 0] && all_zero && limit == end)),
                // A chunk cut off by the end: read what there is of it.
                _ => {}
            }
        }
        let available = size.min(end - data);
        if id == b"00wb" {
            feed_payload(&mut reader, data, available, &mut piece, dealer, cancelled)?;
        }
        if available < size {
            return Ok(true);
        }
        data_end = data + size;
        pos = data + size + (size & 1);
    }
    // Fewer than 8 bytes left: a chunk header was cut off.
    Ok(!dealer.limit_reached && pos < end)
}

/// What the header chunks say, before any audio is read.
fn header_info(layout: &Layout, file_len: u64, kind: IndexKind) -> TrmInfo {
    let (start_filetime, start_basis) = match (layout.tloc, layout.tutc) {
        (Some(local), _) => (Some(local), Some(StartBasis::Local)),
        (None, Some(utc)) => (Some(utc), Some(StartBasis::Utc)),
        (None, None) => match layout.file_name_filetime {
            Some(named) => (Some(named), Some(StartBasis::FileName)),
            None => (None, None),
        },
    };
    TrmInfo {
        tag: layout.tag,
        audio_codec: layout.codec,
        stream_layout: layout.stream_layout,
        channels: layout.channels,
        streams: layout.streams,
        channels_per_stream: layout.channels_per_stream,
        format_sample_rate: layout.format_sample_rate,
        header_scale: layout.header_scale,
        header_rate: layout.header_rate,
        header_chunk_count: layout.header_length,
        declared_streams: layout.declared_streams,
        stream_lists: layout.stream_lists,
        start_boundary: layout.start_boundary,
        end_boundary: layout.end_boundary,
        open_ended: layout.open_ended,
        start_utc: layout.tutc.and_then(|time| filetime_text(time, true)),
        start_local: layout.tloc.and_then(|time| filetime_text(time, false)),
        start_basis,
        start_filetime,
        recorded_file_name: layout.recorded_file_name.clone(),
        recorded_file_name_present: layout.recorded_file_name_present,
        user_name_present: layout.user_name_present,
        user_data_present: layout.user_data_present,
        signature: layout.signed.then_some(SIGNED_NOT_VERIFIED),
        index_kind: kind,
        truncated: layout.truncated,
        continuation_segments: layout.continuation_segments,
        ftra: layout.ftra,
        video_streams: layout.video_streams,
        file_bytes: file_len,
        ..TrmInfo::default()
    }
}

fn build_info(
    layout: &Layout,
    file_len: u64,
    kind: IndexKind,
    truncated: bool,
    stats: DealStats,
    sample_rate: Option<u32>,
) -> TrmInfo {
    let streams = u64::from(layout.streams);
    let duration_secs = sample_rate
        .filter(|rate| *rate > 0 && layout.continuation_segments == 0)
        .map(|rate| (stats.rounds * SAMPLES_PER_FRAME) as f64 / f64::from(rate));
    TrmInfo {
        sample_rate,
        packets: stats.packets,
        frames: stats.rounds * streams,
        frames_per_channel: stats.rounds,
        scrambled_frames: stats.scrambled,
        straddling_frames: stats.straddling,
        misaligned_packets: stats.misaligned,
        multi_round_packets: stats.multi_round,
        resync_skipped_bytes: stats.resync_skipped,
        channel_resyncs: stats.channel_resyncs,
        mid_stream_skipped_bytes: stats.mid_stream_skipped,
        padding_bytes: stats.padding,
        trailing_skipped_bytes: stats.trailing_skipped,
        incomplete_final_round: stats.incomplete_final_round,
        dropped_frames: stats.dropped,
        dropped_rounds: stats.lost_rounds,
        timeline_shift_secs: sample_rate
            .filter(|rate| *rate > 0 && stats.lost_rounds > 0)
            .map(|rate| (stats.lost_rounds * SAMPLES_PER_FRAME) as f64 / f64::from(rate)),
        walk_gaps: stats.walk_gaps,
        walk_gap_bytes: stats.walk_gap_bytes,
        length_high_bit_frames: stats.high_bits,
        frame_channel_mismatches: stats.config_mismatches,
        truncated: layout.truncated || truncated || stats.cut_final_frame,
        duration_secs,
        ..header_info(layout, file_len, kind)
    }
}

fn open_layout(path: &Path, bound: Option<u64>) -> Result<(Layout, u64, bool), String> {
    let mut file = File::open(path).map_err(io_error)?;
    let actual_len = file.metadata().map_err(io_error)?.len();
    let file_len = bound.map_or(actual_len, |bound| bound.min(actual_len));
    let layout = read_layout(&mut file, file_len)?;
    Ok((layout, file_len, actual_len > file_len))
}

/// Read a `.trm` through once and report its layout. Never modifies it.
pub(crate) fn inspect_trm(path: &Path) -> Result<TrmInfo, String> {
    inspect_trm_with_cancel(path, None)
}

pub(crate) fn inspect_trm_with_cancel(path: &Path, cancelled: Cancel<'_>) -> Result<TrmInfo, String> {
    check_cancel(cancelled)?;
    let (layout, file_len, _) = open_layout(path, None)?;
    if layout.codec != AudioCodec::FtrAac {
        // MP2, MP3 or WMA: FFmpeg reads these as ordinary audio.
        return Ok(header_info(&layout, file_len, IndexKind::NotRead));
    }
    let walk = || -> Result<TrmInfo, String> {
        let mut dealer = Dealer::new(&layout, None, None);
        let truncated = read_by_walking(path, &layout, file_len, &mut dealer, cancelled)?;
        let (stats, rate) = dealer.finish();
        Ok(build_info(&layout, file_len, IndexKind::Walked, truncated, stats, rate))
    };
    for kind in index_candidates(path, &layout)? {
        let mut dealer = Dealer::new(&layout, None, None);
        if let Some(read) = read_by_index(path, &layout, file_len, kind, &mut dealer, cancelled)? {
            let (stats, rate) = dealer.finish();
            let indexed = build_info(&layout, file_len, kind, read.truncated, stats, rate);
            if !index_misses_chunks(path, &layout, file_len, &read, cancelled)? {
                return Ok(indexed);
            }
            // FFmpeg's AVI reader reads an interleaved file straight through
            // movi, not through idx1: so does DepoAudio, if that finds more.
            let walked = walk()?;
            let chosen = if walked.packets > indexed.packets {
                walked
            } else {
                indexed
            };
            return Ok(TrmInfo {
                index_mismatch: true,
                ..chosen
            });
        }
    }
    walk()
}

/// The index does not describe every audio chunk: `movi` holds one after the
/// last chunk listed, or (for a closed recording) the number listed is not
/// the header's chunk count (`strh` dwLength, `FTRA` d).
fn index_misses_chunks(
    path: &Path,
    layout: &Layout,
    file_len: u64,
    read: &IndexRead,
    cancelled: Cancel<'_>,
) -> Result<bool, String> {
    if !layout.open_ended {
        let differs = |count: u32| u64::from(count) != read.audio_chunks;
        if layout.header_length.is_some_and(differs) || layout.ftra.is_some_and(|ftra| differs(ftra.d)) {
            return Ok(true);
        }
    }
    audio_chunk_after(path, layout, file_len, read.after_last, cancelled)
}

/// Read the recording again exactly as `info` describes it (same index kind,
/// same stream layout, no further than the length inspected and no more
/// rounds than inspected), delivering every frame to `sink`.
fn extract<'s>(
    path: &Path,
    info: &TrmInfo,
    sink: &'s mut FrameSink<'s>,
    cancelled: Cancel<'_>,
) -> Result<TrmInfo, String> {
    check_cancel(cancelled)?;
    let (layout, file_len, grew) = open_layout(path, Some(info.file_bytes))?;
    if layout.channels != info.channels || layout.streams != info.streams || layout.codec != info.audio_codec {
        return Err(TRM_CHANGED_MESSAGE.into());
    }
    let mut dealer = Dealer::new(&layout, Some(sink), Some(info.frames_per_channel));
    let truncated = match info.index_kind {
        IndexKind::Walked => read_by_walking(path, &layout, file_len, &mut dealer, cancelled)?,
        IndexKind::NotRead => return Err(TRM_CHANGED_MESSAGE.into()),
        kind => {
            read_by_index(path, &layout, file_len, kind, &mut dealer, cancelled)?
                .ok_or_else(|| TRM_CHANGED_MESSAGE.to_string())?
                .truncated
        }
    };
    if dealer.limit_reached {
        // Every inspected round was delivered, so the inspection describes
        // what was read. If the recording is still being written, what
        // follows was not inspected and is not read.
        return Ok(TrmInfo {
            grew_while_read: grew,
            ..info.clone()
        });
    }
    let (stats, rate) = dealer.finish();
    let mut written = build_info(&layout, file_len, info.index_kind, truncated, stats, rate);
    written.grew_while_read = grew;
    written.index_mismatch = info.index_mismatch;
    Ok(written)
}

/// Removes the per-channel files unless the extraction completed.
struct ChannelFiles {
    paths: Vec<PathBuf>,
    keep: bool,
}

impl Drop for ChannelFiles {
    fn drop(&mut self) {
        if !self.keep {
            for path in &self.paths {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// The name prefix of every temporary channel file, for the startup sweep.
pub(crate) const CHANNEL_FILE_PREFIX: &str = "depoaudio_trm_";

/// Write each stream of a `.trm` to its own new ADTS `.aac` file in
/// `out_dir` (descrambled, whole rounds only, so every stream has the same
/// frame count): one mono file per microphone, or for a stereo-stream file
/// one stereo file per pair, in channel order. Streams: no stream is held in
/// memory. The original is only read. On any error, no stream file is left
/// behind.
#[cfg(test)]
pub(crate) fn deinterleave_trm(path: &Path, out_dir: &Path) -> Result<(TrmInfo, Vec<PathBuf>), String> {
    deinterleave_trm_with_cancel(path, out_dir, None, None)
}

/// [`deinterleave_trm`] with cancellation, reusing an earlier inspection's
/// index choice when given. A recording that grew since that inspection
/// (one still being written) is read only as far as it was inspected.
pub(crate) fn deinterleave_trm_with_cancel(
    path: &Path,
    out_dir: &Path,
    inspected: Option<&TrmInfo>,
    cancelled: Cancel<'_>,
) -> Result<(TrmInfo, Vec<PathBuf>), String> {
    let info = match inspected {
        Some(info) => info.clone(),
        None => inspect_trm_with_cancel(path, cancelled)?,
    };
    if info.audio_codec != AudioCodec::FtrAac {
        return Err(format!(
            "This FTR recording holds {} audio, which is not split into channels; FFmpeg reads it as it is.",
            info.audio_codec.label()
        ));
    }
    if info.continuation_segments > 0 {
        return Err(
            "This FTR recording continues in further RIFF (AVIX) segments, which DepoAudio's channel split does not read yet, so it cannot be split without losing audio.".into(),
        );
    }
    if info.frames_per_channel == 0 {
        return Err(NO_COMPLETE_ROUND_ERROR.into());
    }
    if info.frame_channel_mismatches > 0 {
        return Err(TRM_FRAME_CHANNELS_MESSAGE.into());
    }
    let stem = format!("{CHANNEL_FILE_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let mut files = ChannelFiles {
        paths: Vec::new(),
        keep: false,
    };
    let mut writers = Vec::with_capacity(usize::from(info.streams));
    for stream in 0..info.streams {
        let stream_path = out_dir.join(format!("{stem}_ch{stream}.aac"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Owner only: the decoded audio of a court recording.
            options.mode(0o600);
        }
        let file = options
            .open(&stream_path)
            .map_err(|error| format!("Could not create a temporary channel file: {error}"))?;
        files.paths.push(stream_path);
        writers.push(BufWriter::new(file));
    }
    let mut sink = |stream: usize, frame: &[u8]| -> Result<(), String> {
        writers
            .get_mut(stream)
            .ok_or_else(|| TRM_CHANGED_MESSAGE.to_string())?
            .write_all(frame)
            .map_err(|error| format!("Could not write a temporary channel file: {error}"))
    };
    let written = extract(path, &info, &mut sink, cancelled)?;
    for writer in writers {
        let file = writer
            .into_inner()
            .map_err(|error| format!("Could not write a temporary channel file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not write a temporary channel file: {error}"))?;
    }
    check_cancel(cancelled)?;
    if written.frames_per_channel != info.frames_per_channel {
        return Err(TRM_CHANGED_MESSAGE.into());
    }
    files.keep = true;
    Ok((written, std::mem::take(&mut files.paths)))
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test directory removed on drop.
    pub(crate) struct TrmTestDir(pub PathBuf);

    impl TrmTestDir {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("depoaudio_trm_test_{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TrmTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // ── Synthetic TRM builder ───────────────────────────────────────────────
    //
    // Everything here is made up for tests: tones encoded by FFmpeg's AAC
    // encoder, marked and scrambled the way the interoperability notes
    // describe, wrapped in a hand-built AVI. No byte comes from a real FTR
    // recording or from FTR software.

    pub(crate) const TONE_FREQUENCIES: [u32; 4] = [300, 700, 1100, 1700];
    pub(crate) const TONE_RATE: u32 = 16_000;

    pub(crate) fn aac_tools_available() -> bool {
        let ok = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(" aac "))
            && std::process::Command::new("ffprobe").arg("-version").output().is_ok();
        if !ok {
            assert!(
                std::env::var_os("DEPOAUDIO_REQUIRE_FFMPEG_TESTS").is_none_or(|value| value != "1"),
                "DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1, but ffmpeg with the aac encoder is not on PATH"
            );
            eprintln!("skipping: ffmpeg/ffprobe with the aac encoder not on PATH");
        }
        ok
    }

    /// ADTS frames of one mono tone encoded by FFmpeg.
    pub(crate) fn tone_frames(dir: &Path, frequency: u32, seconds: f64) -> Vec<Vec<u8>> {
        let out = dir.join(format!("tone_{frequency}.aac"));
        let status = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={frequency}:duration={seconds}:sample_rate={TONE_RATE}"
            ))
            .args(["-c:a", "aac", "-b:a", "48k", "-f", "adts", "-y"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&out).unwrap();
        fs::remove_file(&out).unwrap();
        let mut frames = Vec::new();
        let mut pos = 0;
        while pos + 7 <= bytes.len() {
            let h = &bytes[pos..];
            assert_eq!(h[0], 0xFF);
            let len = ((usize::from(h[3]) & 3) << 11) | (usize::from(h[4]) << 3) | (usize::from(h[5]) >> 5);
            assert!((7..2048).contains(&len));
            frames.push(bytes[pos..pos + len].to_vec());
            pos += len;
        }
        frames
    }

    /// Every second frame is scrambled; a frame too short to hold the
    /// scrambled bytes (an encoder's near-empty first frame) is left plain.
    fn scrambles(round: usize, channel: usize, length: usize) -> bool {
        (round + channel) % 2 == 1 && length >= SCRAMBLE_MIN_FRAME_BYTES
    }

    /// N channels' frames, marked (private bit 0 on channel 0, 1 on the
    /// others) and with every second round scrambled, interleaved round by
    /// round into one stream. Returns the stream and each frame's length.
    pub(crate) fn ftr_frame_stream(channels: &[Vec<Vec<u8>>]) -> (Vec<u8>, Vec<usize>) {
        let rounds = channels.iter().map(Vec::len).min().unwrap();
        let mut stream = Vec::new();
        let mut lengths = Vec::new();
        for round in 0..rounds {
            for (channel, frames) in channels.iter().enumerate() {
                let mut frame = frames[round].clone();
                if channel == 0 {
                    frame[2] &= !0x02;
                } else {
                    frame[2] |= 0x02;
                }
                if scrambles(round, channel, frame.len()) {
                    frame[3] |= 0x20;
                    assert!(descramble_frame(&mut frame));
                }
                lengths.push(frame.len());
                stream.extend_from_slice(&frame);
            }
        }
        (stream, lengths)
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    pub(crate) enum Index {
        Absolute,
        Relative,
        None,
        /// An idx1 chunk header whose size field says 0 (never closed).
        AbsoluteZeroSize,
    }

    pub(crate) struct AviSpec {
        pub tag: u16,
        pub channels: u16,
        pub flags: u32,
        pub length: u32,
        pub index: Index,
        pub ftra: Option<[u32; 5]>,
        pub video_streams: u32,
        /// Leave the movi list size at 0 (never closed).
        pub open_movi: bool,
        /// Size of a video (`01dc`) chunk placed before the audio, listed
        /// first in idx1; 0 for none.
        pub lead_video: usize,
        /// Top-level chunks placed between `LIST hdrl` and `LIST movi`.
        pub header_extra: Vec<u8>,
        /// Bytes appended after `idx1` (`peak`, `SIGN`, `CERT`).
        pub trailer: Vec<u8>,
        /// A level-meter (`01al`) chunk after every audio chunk, indexed.
        pub level_meter: bool,
        /// avih dwStreams; None writes the number of strl lists.
        pub declared_streams: Option<u32>,
    }

    impl AviSpec {
        pub(crate) fn closed(channels: u16) -> Self {
            Self {
                tag: MONO_STREAMS_TAG,
                channels,
                flags: 0,
                length: 0,
                index: Index::Absolute,
                ftra: None,
                video_streams: 0,
                open_movi: false,
                lead_video: 0,
                header_extra: Vec::new(),
                trailer: Vec::new(),
                level_meter: false,
                declared_streams: None,
            }
        }
    }

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut data = kind.to_vec();
        data.extend_from_slice(body);
        chunk(b"LIST", &data)
    }

    fn le(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    /// A RIFF AVI holding `packets` as `00wb` chunks.
    pub(crate) fn build_avi(spec: &AviSpec, packets: &[Vec<u8>]) -> Vec<u8> {
        let streams = spec.declared_streams.unwrap_or(1 + spec.video_streams);
        let mut avih = le(&[0, 0, 0, spec.flags, packets.len() as u32, 0, streams, 0, 0, 0]);
        avih.resize(56, 0);
        let mut strh = b"auds".to_vec();
        // 0 writes the audio chunk count, as a recorder does at close.
        let length = if spec.length == 0 {
            packets.len() as u32
        } else {
            spec.length
        };
        strh.extend(le(&[0, 0, 0, 0, 1024, TONE_RATE, 0, length, 4096, u32::MAX, 0]));
        strh.resize(56, 0);
        let mut strf = Vec::new();
        strf.extend(spec.tag.to_le_bytes());
        strf.extend(spec.channels.to_le_bytes());
        strf.extend(TONE_RATE.to_le_bytes());
        strf.extend(6000u32.to_le_bytes());
        strf.extend(1u16.to_le_bytes());
        strf.extend(16u16.to_le_bytes());
        strf.extend(0u16.to_le_bytes());
        let mut hdrl = chunk(b"avih", &avih);
        let mut strl = chunk(b"strh", &strh);
        strl.extend(chunk(b"strf", &strf));
        hdrl.extend(list(b"strl", &strl));
        for _ in 0..spec.video_streams {
            let mut vstrh = b"vids".to_vec();
            vstrh.resize(56, 0);
            let mut vstrl = chunk(b"strh", &vstrh);
            vstrl.extend(chunk(b"strf", &[0u8; 40]));
            hdrl.extend(list(b"strl", &vstrl));
        }

        let mut out = b"RIFF\0\0\0\0AVI ".to_vec();
        if let Some(values) = spec.ftra {
            out.extend(chunk(b"FTRA", &le(&values)));
        }
        out.extend(list(b"hdrl", &hdrl));
        out.extend(&spec.header_extra);
        let movi_header = out.len();
        let movi_pos = movi_header + 8;
        let mut movi = Vec::new();
        let mut entries = Vec::new();
        if spec.lead_video > 0 {
            entries.push((*b"01dc", movi_pos + 4, spec.lead_video));
            movi.extend(chunk(b"01dc", &vec![0x5A; spec.lead_video]));
        }
        for packet in packets {
            let header = movi_pos + 4 + movi.len();
            entries.push((*b"00wb", header, packet.len()));
            movi.extend(chunk(b"00wb", packet));
            if spec.level_meter {
                let header = movi_pos + 4 + movi.len();
                entries.push((*b"01al", header, 6));
                movi.extend(chunk(b"01al", &[0xFF, 0xF1, 0x60, 0x40, 0x00, 0x00]));
            }
        }
        out.extend(list(b"movi", &movi));
        if spec.open_movi {
            out[movi_header + 4..movi_header + 8].copy_from_slice(&0u32.to_le_bytes());
        }
        if spec.index != Index::None {
            let mut idx = Vec::new();
            for (id, header, size) in entries {
                let offset = match spec.index {
                    Index::Relative => header - movi_pos,
                    _ => header,
                };
                idx.extend(id);
                idx.extend(le(&[0x10, offset as u32, size as u32]));
            }
            let mut idx1 = chunk(b"idx1", &idx);
            if spec.index == Index::AbsoluteZeroSize {
                idx1[4..8].copy_from_slice(&0u32.to_le_bytes());
            }
            out.extend(idx1);
        }
        out.extend(&spec.trailer);
        let riff_size = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&riff_size.to_le_bytes());
        out
    }

    /// Split a frame stream into packets of whole rounds.
    pub(crate) fn round_packets(
        stream: &[u8],
        lengths: &[usize],
        channels: usize,
        rounds_per_packet: usize,
    ) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        let mut pos = 0;
        for group in lengths.chunks(channels * rounds_per_packet) {
            let size: usize = group.iter().sum();
            packets.push(stream[pos..pos + size].to_vec());
            pos += size;
        }
        packets
    }

    /// Fixed-size packets that cut frames anywhere.
    pub(crate) fn sized_packets(stream: &[u8], size: usize) -> Vec<Vec<u8>> {
        stream.chunks(size).map(<[u8]>::to_vec).collect()
    }

    /// Small synthetic frames (not decodable audio) for layout-only tests.
    pub(crate) fn fake_frames(channels: usize, rounds: usize) -> Vec<Vec<Vec<u8>>> {
        (0..channels)
            .map(|channel| {
                (0..rounds)
                    .map(|round| {
                        let len = 20 + (round * 7 + channel * 3) % 40;
                        let mut frame = vec![0u8; len];
                        frame[0] = 0xFF;
                        frame[1] = 0xF1;
                        frame[2] = 0x60; // LC, 16 kHz (index 8)
                        frame[3] = 0x40;
                        frame[4] = ((len << 5) >> 8) as u8;
                        frame[5] = ((len << 5) & 0xFF) as u8 | 0x1F;
                        frame[6] = 0xFC;
                        for (i, byte) in frame.iter_mut().enumerate().skip(7) {
                            *byte = (channel * 31 + round * 7 + i) as u8;
                        }
                        frame
                    })
                    .collect()
            })
            .collect()
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Frames of a channel file, as written.
    pub(crate) fn split_adts(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let mut pos = 0;
        while pos + 7 <= bytes.len() {
            let len = ftr_frame_length(&bytes[pos..]);
            frames.push(bytes[pos..pos + len].to_vec());
            pos += len;
        }
        assert_eq!(pos, bytes.len());
        frames
    }

    /// The frames each channel should get: the unscrambled originals with the
    /// channel marker and (for scrambled ones) the original/copy bit set.
    fn expected_channel(channels: &[Vec<Vec<u8>>], channel: usize, rounds: usize) -> Vec<Vec<u8>> {
        (0..rounds)
            .map(|round| {
                let mut frame = channels[channel][round].clone();
                if channel == 0 {
                    frame[2] &= !0x02;
                } else {
                    frame[2] |= 0x02;
                }
                if scrambles(round, channel, frame.len()) {
                    frame[3] |= 0x20;
                }
                frame
            })
            .collect()
    }

    #[test]
    fn routing_keeps_native_ftr_only_for_the_layout_it_handles() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 4);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let aligned = write(
            &dir.0,
            "aligned.trm",
            &build_avi(&AviSpec::closed(2), &round_packets(&stream, &lengths, 2, 1)),
        );
        let doubled = write(
            &dir.0,
            "doubled.trm",
            &build_avi(&AviSpec::closed(2), &round_packets(&stream, &lengths, 2, 2)),
        );
        let straddled = write(
            &dir.0,
            "straddled.trm",
            &build_avi(&AviSpec::closed(2), &sized_packets(&stream, 50)),
        );
        let native = crate::helpers::FeedOptions {
            native_ftr: true,
            native_failed: false,
        };
        let info = inspect_trm(&aligned).unwrap();
        assert_eq!(choose_route(&info, native), (DecodePath::NativeFtr, Vec::new()));
        assert_eq!(
            choose_route(&info, crate::helpers::FeedOptions::default()),
            (
                DecodePath::Deinterleaved,
                vec!["native-decoder-unavailable".to_string()]
            )
        );
        let failed = crate::helpers::FeedOptions {
            native_failed: true,
            ..native
        };
        assert_eq!(
            choose_route(&info, failed),
            (DecodePath::Deinterleaved, vec!["native-decode-failed".to_string()])
        );
        let (path, reasons) = choose_route(&inspect_trm(&straddled).unwrap(), native);
        assert_eq!(path, DecodePath::Deinterleaved);
        assert!(reasons.contains(&"frames-cross-chunks".to_string()), "{reasons:?}");
        // Two whole rounds per chunk: FFmpeg's native decoder can lose audio
        // in some outputs, so it is split instead.
        let info = inspect_trm(&doubled).unwrap();
        assert_eq!(info.multi_round_packets, 2);
        assert_eq!(info.misaligned_packets, 0);
        assert_eq!(
            choose_route(&info, native),
            (DecodePath::Deinterleaved, vec!["several-rounds-per-chunk".to_string()])
        );
    }

    #[test]
    fn descrambling_is_its_own_inverse_and_needs_the_flag() {
        let mut frame: Vec<u8> = (0u8..20).collect();
        frame[3] = 0x20;
        let original = frame.clone();
        assert!(descramble_frame(&mut frame));
        assert_eq!(frame[8], !10);
        assert_eq!(frame[9], !9);
        assert_eq!(frame[10], !8);
        assert_eq!(frame[11], !11);
        assert_eq!(frame[12], !12);
        assert!(descramble_frame(&mut frame));
        assert_eq!(frame, original);

        let mut plain = original.clone();
        plain[3] = 0;
        assert!(!descramble_frame(&mut plain));
        let mut short = vec![0u8; 12];
        short[3] = 0x20;
        assert!(!descramble_frame(&mut short));
    }

    #[test]
    fn aligned_layout_is_native_compatible_and_deinterleaves_exactly() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(4, 9);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 4, 1);
        let path = write(&dir.0, "aligned.trm", &build_avi(&AviSpec::closed(4), &packets));
        let before = fs::read(&path).unwrap();

        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.index_kind, IndexKind::Idx1Absolute);
        assert_eq!(info.channels, 4);
        assert_eq!(info.tag, 0x4180);
        assert_eq!(info.sample_rate, Some(16_000));
        assert_eq!(info.packets, 9);
        assert_eq!(info.frames, 36);
        assert_eq!(info.frames_per_channel, 9);
        assert_eq!(info.scrambled_frames, 18);
        assert_eq!(info.straddling_frames, 0);
        assert_eq!(info.misaligned_packets, 0);
        assert_eq!(info.resync_skipped_bytes, 0);
        assert!(!info.incomplete_final_round && !info.open_ended && !info.truncated);
        assert!(info.native_blockers().is_empty(), "{:?}", info.native_blockers());
        assert_eq!(info.duration_secs, Some(9.0 * 1024.0 / 16_000.0));

        let (written, files) = deinterleave_trm(&path, &dir.0).unwrap();
        assert_eq!(written, info);
        assert_eq!(files.len(), 4);
        for (channel, file) in files.iter().enumerate() {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&channels, channel, 9)
            );
        }
        assert_eq!(fs::read(&path).unwrap(), before, "the original is never modified");
    }

    #[test]
    fn straddling_frames_and_a_late_start_are_reported_and_still_dealt() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(4, 12);
        let (stream, _) = ftr_frame_stream(&channels);
        // Two channel-2/3 frames from an earlier round before channel 0.
        let mut lead = channels[2][0].clone();
        lead[2] |= 0x02;
        let mut lead3 = channels[3][0].clone();
        lead3[2] |= 0x02;
        let skipped = lead.len() + lead3.len();
        let mut joined = lead;
        joined.extend(lead3);
        joined.extend(&stream);
        // And an extra channel-0 frame at the end (an incomplete round).
        let mut tail = channels[0][3].clone();
        tail[2] &= !0x02;
        joined.extend(&tail);
        let packets = sized_packets(&joined, 77);
        let path = write(&dir.0, "straddle.trm", &build_avi(&AviSpec::closed(4), &packets));

        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.frames_per_channel, 12);
        assert_eq!(info.resync_skipped_bytes, skipped as u64);
        assert!(info.straddling_frames > 0);
        assert!(info.misaligned_packets > 0);
        assert!(info.incomplete_final_round);
        assert_eq!(info.dropped_frames, 1);
        let blockers = info.native_blockers();
        assert!(blockers.contains(&"frames-cross-chunks"));
        assert!(blockers.contains(&"data-before-channel-0"));

        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate() {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&channels, channel, 12)
            );
        }
    }

    #[test]
    fn an_unindexed_open_ended_file_is_walked() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 10);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut spec = AviSpec::closed(2);
        spec.index = Index::None;
        spec.length = 0x7FFF_FFFF;
        spec.open_movi = true;
        let mut bytes = build_avi(&spec, &packets);
        // The recorder pre-allocated space it never wrote.
        bytes.extend(vec![0u8; 300]);
        let path = write(&dir.0, "open.trm", &bytes);

        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.index_kind, IndexKind::Walked);
        assert!(info.open_ended);
        assert!(!info.truncated);
        assert_eq!(info.frames_per_channel, 10);
        assert_eq!(info.duration_secs, Some(10.0 * 1024.0 / 16_000.0));
        assert!(info.native_blockers().contains(&"open-ended"));
        assert!(info.native_blockers().contains(&"no-usable-index"));
    }

    #[test]
    fn relative_and_zero_size_indexes_are_read() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(3, 6);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 3, 1);
        for (index, kind) in [
            (Index::Relative, IndexKind::Idx1Relative),
            (Index::AbsoluteZeroSize, IndexKind::Idx1Absolute),
        ] {
            let mut spec = AviSpec::closed(3);
            spec.index = index;
            let path = write(&dir.0, "indexed.trm", &build_avi(&spec, &packets));
            let info = inspect_trm(&path).unwrap();
            assert_eq!(info.index_kind, kind);
            assert_eq!(info.packets, 6);
            assert_eq!(info.frames_per_channel, 6);
            assert_eq!(info.misaligned_packets, 0);
            // A never-closed (zero-size) index runs to EOF: it is not a sign
            // of a cut-off file.
            assert!(!info.truncated, "{index:?}");
            assert!(info.native_blockers().is_empty(), "{:?}", info.native_blockers());
        }
    }

    #[test]
    fn an_index_that_does_not_match_the_chunks_falls_back_to_walking() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 4);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut bytes = build_avi(&AviSpec::closed(2), &packets);
        // Corrupt the first idx1 entry's size.
        let idx = bytes.windows(4).rposition(|w| w == b"idx1").unwrap();
        bytes[idx + 8 + 12] ^= 0x01;
        let path = write(&dir.0, "badindex.trm", &bytes);
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.index_kind, IndexKind::Walked);
        assert_eq!(info.frames_per_channel, 4);
    }

    #[test]
    fn boundary_flags_ftra_and_video_streams_are_reported() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(1, 3);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 1, 1);
        for (flags, first, last) in [
            (0x2000_0000, true, false),
            (0x4000_0000, false, true),
            (0x6000_0010, true, true),
            (0x10, false, false),
        ] {
            let mut spec = AviSpec::closed(1);
            spec.flags = flags;
            spec.ftra = Some([1, 2, 3, 4, 5]);
            spec.video_streams = 2;
            let path = write(&dir.0, "session.tll", &build_avi(&spec, &packets));
            let info = inspect_trm(&path).unwrap();
            assert_eq!(
                (info.start_boundary, info.end_boundary),
                (first, last),
                "flags {flags:#x}"
            );
            assert_eq!(
                info.ftra,
                Some(FtraInfo {
                    a: 1,
                    b: 2,
                    c: 3,
                    d: 4,
                    e: 5
                })
            );
            assert_eq!(info.video_streams, 2);
        }
    }

    #[test]
    fn the_tag_bits_decide_the_stream_layout_and_aac_without_them_is_refused() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 3);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut spec = AviSpec::closed(2);
        let info = inspect_trm(&write(&dir.0, "mono.trm", &build_avi(&spec, &packets))).unwrap();
        assert_eq!(
            (info.stream_layout, info.streams, info.channels_per_stream),
            (Some(StreamLayout::Mono), 2, 1)
        );
        assert_eq!(info.audio_codec, AudioCodec::FtrAac);

        // FTR's splitter refuses 0x180 with neither layout bit, and so does
        // DepoAudio, by every route (FFmpeg's native decoder would read it).
        for tag in [0x0180u16, 0xC180] {
            spec.tag = tag;
            let path = write(&dir.0, "nobits.trm", &build_avi(&spec, &packets));
            let error = inspect_trm(&path).unwrap_err();
            assert_eq!(error, TRM_UNSPLITTABLE_MESSAGE, "tag {tag:#06x}");
            assert!(error.starts_with("This FTR recording's format isn't one FTR's own player can split"));
            assert!(is_refusal(&error));
        }
        // Stereo streams need an even microphone count above one.
        let mut odd = AviSpec::closed(3);
        odd.tag = 0x8180;
        let path = write(&dir.0, "odd.trm", &build_avi(&odd, &packets));
        assert_eq!(inspect_trm(&path).unwrap_err(), TRM_UNSPLITTABLE_MESSAGE);

        // A tag for no codec FTR writes is still an error that names it.
        spec.tag = 0x0001;
        let path = write(&dir.0, "pcm.trm", &build_avi(&spec, &packets));
        let error = inspect_trm(&path).unwrap_err();
        assert!(error.contains("0x0001") && !is_refusal(&error), "{error}");
    }

    #[test]
    fn mp2_mp3_and_wma_tags_are_left_to_ffmpeg_as_plain_audio() {
        let dir = TrmTestDir::new();
        for (tag, codec) in [
            (0x0050u16, AudioCodec::Mp2),
            (0x0055, AudioCodec::Mp3),
            (0x0160, AudioCodec::Wma),
            (0x0163, AudioCodec::Wma),
        ] {
            let mut spec = AviSpec::closed(1);
            spec.tag = tag;
            let path = write(&dir.0, "plain.trm", &build_avi(&spec, &[vec![0x11; 40]]));
            let info = inspect_trm(&path).unwrap();
            assert_eq!(info.audio_codec, codec);
            assert_eq!(info.index_kind, IndexKind::NotRead);
            assert_eq!((info.stream_layout, info.streams, info.frames), (None, 0, 0));
            assert_eq!(info.duration_secs, None);
            let native = crate::helpers::FeedOptions {
                native_ftr: true,
                native_failed: false,
            };
            assert_eq!(
                choose_route(&info, native),
                (DecodePath::Plain, vec!["not-ftr-aac".to_string()])
            );
            assert!(deinterleave_trm(&path, &dir.0).unwrap_err().contains("not split"));
            assert!(plain_decode_error(&info, "boom").contains(codec.label()));
        }
    }

    #[test]
    fn garbage_and_truncated_files_give_clear_errors_without_panicking() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(4, 6);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let good = build_avi(&AviSpec::closed(4), &round_packets(&stream, &lengths, 4, 1));

        for (name, bytes) in [
            ("empty", Vec::new()),
            ("short", b"RIFF".to_vec()),
            ("wav", b"RIFF\x10\0\0\0WAVEfmt \0\0\0\0".to_vec()),
            (
                "noise",
                (0..5000u32)
                    .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
                    .collect(),
            ),
        ] {
            let path = write(&dir.0, &format!("{name}.trm"), &bytes);
            let error = inspect_trm(&path).unwrap_err();
            assert!(
                error.contains("FTR recording") || error.contains("too short"),
                "{name}: {error}"
            );
            assert!(deinterleave_trm(&path, &dir.0).is_err());
        }

        // Every prefix of a good file: an error or a (possibly empty) result,
        // never a panic, and no channel file left behind by a failure.
        for cut in (0..good.len()).step_by(7) {
            let path = write(&dir.0, "cut.trm", &good[..cut]);
            if let Ok(info) = inspect_trm(&path) {
                assert!(info.frames_per_channel <= 6);
                if cut < good.len() - 200 {
                    assert!(info.truncated || info.index_kind == IndexKind::Walked, "cut {cut}");
                }
            }
            if let Ok((_, files)) = deinterleave_trm(&path, &dir.0) {
                files.iter().for_each(|file| fs::remove_file(file).unwrap());
            }
        }

        // A header list claiming a huge size, a zero-channel and a 17-channel
        // header, and a hostile idx1 pointing past EOF.
        let mut huge = good.clone();
        let hdrl = huge.windows(4).position(|w| w == b"hdrl").unwrap() - 8;
        huge[hdrl + 4..hdrl + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(inspect_trm(&write(&dir.0, "huge.trm", &huge))
            .unwrap_err()
            .contains("large"));
        for count in [0u16, 17] {
            let mut spec = AviSpec::closed(count);
            spec.index = Index::Absolute;
            let bytes = build_avi(&spec, &round_packets(&stream, &lengths, 4, 1));
            let error = inspect_trm(&write(&dir.0, "count.trm", &bytes)).unwrap_err();
            assert!(error.contains(&format!("{count} channels")), "{error}");
        }
        let mut hostile = good.clone();
        let idx = hostile.windows(4).rposition(|w| w == b"idx1").unwrap();
        hostile[idx + 8 + 8..idx + 8 + 12].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
        let info = inspect_trm(&write(&dir.0, "hostile.trm", &hostile)).unwrap();
        assert_eq!(info.index_kind, IndexKind::Walked);
        assert_eq!(info.frames_per_channel, 6);

        // Audio chunks with no ADTS frames at all.
        let junk = build_avi(&AviSpec::closed(2), &[vec![0x11; 500], vec![0x22; 300]]);
        let path = write(&dir.0, "nojunk.trm", &junk);
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.frames, 0);
        assert_eq!(info.sample_rate, None);
        assert_eq!(info.duration_secs, None);
        assert!(deinterleave_trm(&path, &dir.0)
            .unwrap_err()
            .contains("No complete round"));

        let leftovers: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".aac"))
            .collect();
        assert!(leftovers.is_empty(), "failed extractions left channel files behind");
    }

    #[test]
    fn cancellation_stops_the_read_and_removes_channel_files() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 4);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let path = write(
            &dir.0,
            "cancel.trm",
            &build_avi(&AviSpec::closed(2), &round_packets(&stream, &lengths, 2, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        let cancelled = || true;
        let error = deinterleave_trm_with_cancel(&path, &dir.0, Some(&info), Some(&cancelled)).unwrap_err();
        assert_eq!(error, TRM_CANCELLED_MESSAGE);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    /// The frames of `channels`, marked and scrambled as in a recording, one
    /// frame per audio chunk; `edit` may drop or repeat frames first.
    fn single_frame_packets(channels: &[Vec<Vec<u8>>], edit: impl FnOnce(&mut Vec<Vec<u8>>)) -> Vec<Vec<u8>> {
        let (stream, lengths) = ftr_frame_stream(channels);
        let mut frames = Vec::new();
        let mut pos = 0;
        for length in lengths {
            frames.push(stream[pos..pos + length].to_vec());
            pos += length;
        }
        edit(&mut frames);
        frames
    }

    fn bytes_read_by(run: impl FnOnce()) -> u64 {
        SOURCE_BYTES_READ.with(|count| count.set(0));
        run();
        SOURCE_BYTES_READ.with(std::cell::Cell::get)
    }

    #[test]
    fn small_chunks_are_read_about_once_not_once_per_chunk() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(4, 3000);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 4, 1);
        for index in [Index::Absolute, Index::None] {
            let mut spec = AviSpec::closed(4);
            spec.index = index;
            let path = write(&dir.0, "small.trm", &build_avi(&spec, &packets));
            let size = fs::metadata(&path).unwrap().len();
            let mut info = None;
            let inspected = bytes_read_by(|| info = Some(inspect_trm(&path).unwrap()));
            let info = info.unwrap();
            assert_eq!(info.frames_per_channel, 3000);
            let mut files = Vec::new();
            let split = bytes_read_by(|| {
                files = deinterleave_trm_with_cancel(&path, &dir.0, Some(&info), None)
                    .unwrap()
                    .1;
            });
            files.iter().for_each(|file| fs::remove_file(file).unwrap());
            // Each pass reads the file (and the index) about once: a seek per
            // chunk must not refill the read buffer every time.
            assert!(
                inspected < size * 3,
                "{index:?}: inspection read {inspected} bytes of a {size}-byte file"
            );
            assert!(
                split < size * 3,
                "{index:?}: the split read {split} bytes of a {size}-byte file"
            );
        }
    }

    #[test]
    fn a_lost_frame_mid_file_restarts_at_channel_0_instead_of_rotating_channels() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(3, 8);
        // Round 3's channel-1 frame is missing.
        let packets = single_frame_packets(&channels, |frames| {
            frames.remove(3 * 3 + 1);
        });
        let path = write(&dir.0, "lost.trm", &build_avi(&AviSpec::closed(3), &packets));
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.channel_resyncs, 1);
        assert_eq!(info.frames_per_channel, 7);
        assert_eq!(info.dropped_frames, 2);
        assert_eq!(info.resync_skipped_bytes, 0);
        assert!(info.native_blockers().contains(&"channel-order-resynced"));
        // The dropped round is not filled with silence: later audio moves
        // one 64 ms round earlier, and the export record says so.
        assert_eq!(info.dropped_rounds, 1);
        assert_eq!(info.timeline_shift_secs, Some(0.064));
        let note = crate::manifest::trm_timeline_note(&info).unwrap();
        assert!(note.contains("dropped 1 incomplete round(s)"), "{note}");

        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate() {
            let mut expected = expected_channel(&channels, channel, 8);
            expected.remove(3);
            assert_eq!(split_adts(&fs::read(file).unwrap()), expected, "channel {channel}");
        }
    }

    /// Insert `bytes` at `at` in a built AVI (or delete `delete` bytes there)
    /// and fix the RIFF and `movi` list sizes.
    fn splice_movi(avi: &mut Vec<u8>, at: usize, delete: usize, bytes: &[u8]) {
        avi.splice(at..at + delete, bytes.iter().copied());
        let movi = avi.windows(4).position(|window| window == b"movi").unwrap() - 8;
        let delta = bytes.len() as i64 - delete as i64;
        for size_at in [4, movi + 4] {
            let size = u32_at(avi, size_at).unwrap() as i64 + delta;
            avi[size_at..size_at + 4].copy_from_slice(&(size as u32).to_le_bytes());
        }
    }

    /// Offsets of the `00wb` chunk headers in a built AVI.
    fn audio_chunk_offsets(avi: &[u8]) -> Vec<usize> {
        let movi = avi.windows(4).position(|window| window == b"movi").unwrap();
        let mut pos = movi + 4;
        let mut offsets = Vec::new();
        while avi.get(pos..pos + 4) == Some(b"00wb") {
            offsets.push(pos);
            let size = u32_at(avi, pos + 4).unwrap() as usize;
            pos += 8 + size + (size & 1);
        }
        offsets
    }

    #[test]
    fn the_walk_continues_past_a_damaged_stretch_and_counts_it() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 8);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut spec = AviSpec::closed(2);
        spec.index = Index::None;
        let clean = build_avi(&spec, &packets);
        for (name, gap) in [("zeros", vec![0u8; 100]), ("garbage", vec![0xABu8; 50])] {
            let mut avi = clean.clone();
            let third = audio_chunk_offsets(&avi)[2];
            splice_movi(&mut avi, third, 0, &gap);
            let path = write(&dir.0, &format!("{name}.trm"), &avi);
            let info = inspect_trm(&path).unwrap();
            assert_eq!(info.index_kind, IndexKind::Walked, "{name}");
            assert_eq!((info.packets, info.frames_per_channel), (8, 8), "{name}");
            assert_eq!((info.walk_gaps, info.walk_gap_bytes), (1, gap.len() as u64), "{name}");
            assert_eq!(info.dropped_rounds, 0, "{name}");
            assert!(!info.truncated, "{name}");
            assert!(crate::manifest::trm_timeline_note(&info)
                .unwrap()
                .contains("unrecorded amount"));
            let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
            for (channel, file) in files.iter().enumerate() {
                assert_eq!(
                    split_adts(&fs::read(file).unwrap()),
                    expected_channel(&channels, channel, 8),
                    "{name} channel {channel}"
                );
            }
        }

        // An odd-sized chunk written without its pad byte: nothing is lost.
        let odd = packets.iter().position(|packet| packet.len() % 2 == 1).unwrap();
        assert!(odd + 1 < packets.len());
        let mut avi = clean.clone();
        let at = audio_chunk_offsets(&avi)[odd] + 8 + packets[odd].len();
        assert_eq!(avi[at], 0);
        splice_movi(&mut avi, at, 1, &[]);
        let info = inspect_trm(&write(&dir.0, "no_pad.trm", &avi)).unwrap();
        assert_eq!((info.packets, info.frames_per_channel), (8, 8));
        assert_eq!((info.walk_gaps, info.walk_gap_bytes, info.dropped_rounds), (1, 0, 0));
        assert_eq!(crate::manifest::trm_timeline_note(&info), None);

        // Zeros to the end of movi are still the end of the written data.
        let mut avi = clean.clone();
        let end = avi.windows(4).position(|window| window == b"movi").unwrap() + 4;
        let end = end + u32_at(&avi, end - 8).unwrap() as usize - 4;
        splice_movi(&mut avi, end, 0, &[0u8; 64]);
        let info = inspect_trm(&write(&dir.0, "tail.trm", &avi)).unwrap();
        assert_eq!((info.packets, info.walk_gaps, info.truncated), (8, 0, false));
    }

    #[test]
    fn an_index_that_lists_fewer_chunks_than_movi_holds_is_not_trusted() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 6);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let full = build_avi(&AviSpec::closed(2), &packets);
        assert!(!inspect_trm(&write(&dir.0, "full.trm", &full)).unwrap().index_mismatch);

        // idx1 cut to its first 3 entries (sizes fixed): movi holds 6.
        let idx1 = full.windows(4).rposition(|window| window == b"idx1").unwrap();
        let mut short = full[..idx1 + 8 + 3 * 16].to_vec();
        short[idx1 + 4..idx1 + 8].copy_from_slice(&48u32.to_le_bytes());
        let riff = (short.len() - 8) as u32;
        short[4..8].copy_from_slice(&riff.to_le_bytes());
        let path = write(&dir.0, "short.trm", &short);
        let info = inspect_trm(&path).unwrap();
        assert!(info.index_mismatch);
        assert_eq!(info.index_kind, IndexKind::Walked);
        assert_eq!((info.packets, info.frames_per_channel), (6, 6));
        assert!(info.native_blockers().contains(&"index-misses-audio-chunks"));
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate() {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&channels, channel, 6)
            );
        }

        // The header's chunk count (or FTRA d) disagrees with a full index:
        // the walk finds no more, so the index is kept, and native is not used.
        for (length, ftra) in [(7, None), (0, Some([1, 0, 0, 5, u32::MAX]))] {
            let mut spec = AviSpec::closed(2);
            spec.length = length;
            spec.ftra = ftra;
            let info = inspect_trm(&write(&dir.0, "count.trm", &build_avi(&spec, &packets))).unwrap();
            assert!(info.index_mismatch, "{length} {ftra:?}");
            assert_eq!(info.index_kind, IndexKind::Idx1Absolute);
            assert_eq!(info.packets, 6);
            assert!(info.native_blockers().contains(&"index-misses-audio-chunks"));
        }
    }

    #[test]
    fn a_file_that_marks_only_channel_0_frames_is_dealt_round_robin() {
        let dir = TrmTestDir::new();
        // fake_frames leaves the private bit clear on every frame.
        let channels = fake_frames(3, 4);
        let packets: Vec<Vec<u8>> = (0..4)
            .map(|round| channels.iter().flat_map(|frames| frames[round].clone()).collect())
            .collect();
        let path = write(&dir.0, "unmarked.trm", &build_avi(&AviSpec::closed(3), &packets));
        let info = inspect_trm(&path).unwrap();
        assert_eq!((info.frames_per_channel, info.channel_resyncs), (4, 0));
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate() {
            assert_eq!(split_adts(&fs::read(file).unwrap()), channels[channel]);
        }
    }

    #[test]
    fn a_missing_channel_0_frame_and_an_extra_frame_are_both_caught() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(3, 8);
        // Round 2's channel-0 frame is missing: its other frames are skipped.
        let packets = single_frame_packets(&channels, |frames| {
            frames.remove(2 * 3);
        });
        let path = write(&dir.0, "nozero.trm", &build_avi(&AviSpec::closed(3), &packets));
        let info = inspect_trm(&path).unwrap();
        assert_eq!(
            (info.channel_resyncs, info.frames_per_channel, info.dropped_frames),
            (1, 7, 2)
        );
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate() {
            let mut expected = expected_channel(&channels, channel, 8);
            expected.remove(2);
            assert_eq!(split_adts(&fs::read(file).unwrap()), expected, "channel {channel}");
        }
        files.iter().for_each(|file| fs::remove_file(file).unwrap());

        // Round 3's channel-1 frame appears twice. Only the private bit marks
        // a channel, so that round's last slot gets the repeat; the frame it
        // displaced is dropped and dealing restarts at round 4.
        let packets = single_frame_packets(&channels, |frames| {
            let repeat = frames[3 * 3 + 1].clone();
            frames.insert(3 * 3 + 2, repeat);
        });
        let path = write(&dir.0, "extra.trm", &build_avi(&AviSpec::closed(3), &packets));
        let info = inspect_trm(&path).unwrap();
        assert_eq!(
            (info.channel_resyncs, info.frames_per_channel, info.dropped_frames),
            (1, 8, 1)
        );
        assert!(info.native_blockers().contains(&"channel-order-resynced"));
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate().take(2) {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&channels, channel, 8)
            );
        }
        let last = split_adts(&fs::read(&files[2]).unwrap());
        let expected = expected_channel(&channels, 2, 8);
        assert_eq!(last.len(), 8);
        assert_eq!(last[..3], expected[..3]);
        assert_eq!(last[4..], expected[4..]);
    }

    #[test]
    fn padding_and_trailing_bytes_are_not_called_truncation_or_a_late_start() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(3, 5);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let aligned = round_packets(&stream, &lengths, 3, 1);

        let mut padded = aligned.clone();
        padded.last_mut().unwrap().extend([0u8; 10]);
        let info = inspect_trm(&write(&dir.0, "padded.trm", &build_avi(&AviSpec::closed(3), &padded))).unwrap();
        assert_eq!(info.frames_per_channel, 5);
        assert_eq!(info.padding_bytes, 10);
        assert!(!info.truncated);
        // FFmpeg's native decoder rejects a chunk with bytes after its frames
        // (FFmpeg 6.1.1), so padding still takes the split route.
        assert_eq!(info.native_blockers(), vec!["zero-padding"]);

        let mut tail = aligned.clone();
        tail.last_mut().unwrap().extend([0x11u8; 10]);
        let info = inspect_trm(&write(&dir.0, "tail.trm", &build_avi(&AviSpec::closed(3), &tail))).unwrap();
        assert_eq!(info.frames_per_channel, 5);
        assert_eq!(info.trailing_skipped_bytes, 10);
        assert_eq!((info.resync_skipped_bytes, info.mid_stream_skipped_bytes), (0, 0));
        assert!(!info.truncated);
        assert!(info.native_blockers().contains(&"unreadable-data"));
        assert!(!info.native_blockers().contains(&"data-before-channel-0"));

        // A last frame the file cuts off is truncation.
        let mut cut = aligned.clone();
        cut.push(channels[0][0][..15].to_vec());
        let info = inspect_trm(&write(&dir.0, "cut.trm", &build_avi(&AviSpec::closed(3), &cut))).unwrap();
        assert!(info.truncated);
        assert_eq!(info.frames_per_channel, 5);

        // Garbage between rounds is counted as mid-stream, not as leading.
        let mut middle = aligned;
        middle[2].splice(0..0, [0x11u8; 9]);
        let info = inspect_trm(&write(&dir.0, "middle.trm", &build_avi(&AviSpec::closed(3), &middle))).unwrap();
        assert_eq!(info.frames_per_channel, 5);
        assert_eq!((info.resync_skipped_bytes, info.mid_stream_skipped_bytes), (0, 9));
    }

    #[test]
    fn frame_length_bits_ftr_ignores_are_cleared_in_the_channel_files() {
        let dir = TrmTestDir::new();
        let mut channels = fake_frames(2, 4);
        channels[1][2][3] |= 0x01;
        let (stream, lengths) = ftr_frame_stream(&channels);
        let path = write(
            &dir.0,
            "bits.trm",
            &build_avi(&AviSpec::closed(2), &round_packets(&stream, &lengths, 2, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.length_high_bit_frames, 1);
        assert_eq!(info.native_blockers(), vec!["frame-length-high-bits"]);
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        let mut expected = expected_channel(&channels, 1, 4);
        expected[2][3] &= !0x03;
        assert_eq!(split_adts(&fs::read(&files[1]).unwrap()), expected);
    }

    #[test]
    fn a_relative_index_that_opens_with_a_large_video_chunk_is_still_relative() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 4);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let mut spec = AviSpec::closed(2);
        spec.index = Index::Relative;
        spec.lead_video = 20_000;
        spec.video_streams = 1;
        let path = write(
            &dir.0,
            "video.trm",
            &build_avi(&spec, &round_packets(&stream, &lengths, 2, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.index_kind, IndexKind::Idx1Relative);
        assert!(info.native_blockers().is_empty(), "{:?}", info.native_blockers());
        assert_eq!(info.frames_per_channel, 4);
    }

    #[test]
    fn a_recording_that_grows_after_inspection_is_split_as_inspected() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 8);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut spec = AviSpec::closed(2);
        spec.index = Index::None;
        spec.length = 0x7FFF_FFFF;
        spec.open_movi = true;
        let early = build_avi(&spec, &packets[..4]);
        // The recorder appends four more chunks; the header stays as it was.
        let mut grown = early.clone();
        grown.extend_from_slice(&build_avi(&spec, &packets)[early.len()..]);
        let path = write(&dir.0, "live.trm", &early);
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.frames_per_channel, 4);
        fs::write(&path, &grown).unwrap();
        assert_eq!(inspect_trm(&path).unwrap().frames_per_channel, 8);
        let (written, files) = deinterleave_trm_with_cancel(&path, &dir.0, Some(&info), None).unwrap();
        assert_eq!(written.frames_per_channel, 4);
        assert!(written.grew_while_read);
        for (channel, file) in files.iter().enumerate() {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&channels, channel, 4)
            );
        }
    }

    #[test]
    fn a_channel_count_that_changed_since_inspection_is_an_error_not_a_panic() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(4, 3);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let path = write(
            &dir.0,
            "four.trm",
            &build_avi(&AviSpec::closed(4), &round_packets(&stream, &lengths, 4, 1)),
        );
        let mut info = inspect_trm(&path).unwrap();
        info.channels = 2;
        let error = deinterleave_trm_with_cancel(&path, &dir.0, Some(&info), None).unwrap_err();
        assert_eq!(error, TRM_CHANGED_MESSAGE);
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            1,
            "no channel file is left behind"
        );
    }

    #[test]
    fn opendml_continuation_segments_are_reported_and_not_split() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 3);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let mut bytes = build_avi(&AviSpec::closed(2), &round_packets(&stream, &lengths, 2, 1));
        let mut avix = list(b"movi", &chunk(b"00wb", &stream));
        avix.splice(0..0, b"AVIX".iter().copied());
        bytes.extend(chunk(b"RIFF", &avix));
        let path = write(&dir.0, "avix.trm", &bytes);
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.continuation_segments, 1);
        assert_eq!(info.duration_secs, None);
        assert_eq!(info.frames_per_channel, 3);
        assert!(!info.truncated);
        assert!(deinterleave_trm(&path, &dir.0).unwrap_err().contains("AVIX"));
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_is_checked_through_long_runs_of_other_index_entries() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(1, 1);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let mut bytes = build_avi(&AviSpec::closed(1), &round_packets(&stream, &lengths, 1, 1));
        // 20,000 entries after the one audio entry that list no audio.
        let idx = bytes.windows(4).rposition(|w| w == b"idx1").unwrap();
        let mut extra = Vec::new();
        for _ in 0..20_000 {
            extra.extend(b"ix00");
            extra.extend(le(&[0, 4, 0]));
        }
        bytes.extend(&extra);
        let size = u32_at(&bytes, idx + 4).unwrap() + extra.len() as u32;
        bytes[idx + 4..idx + 8].copy_from_slice(&size.to_le_bytes());
        let riff = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        let path = write(&dir.0, "entries.trm", &bytes);
        assert_eq!(inspect_trm(&path).unwrap().frames_per_channel, 1);

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let cancelled = || calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 3;
        assert_eq!(
            inspect_trm_with_cancel(&path, Some(&cancelled)).unwrap_err(),
            TRM_CANCELLED_MESSAGE
        );
    }

    // ── FFmpeg-backed checks ─────────────────────────────────────────────────

    pub(crate) fn native_ftr_decoder_on_path() -> bool {
        std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-decoders"])
            .output()
            .is_ok_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.split_whitespace().nth(1) == Some("ftr"))
            })
    }

    /// Interleaved f32 samples FFmpeg decodes with `args` (after `-i`s).
    pub(crate) fn decode_f32(args: &[String]) -> Vec<f32> {
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args(args)
            .args(["-f", "f32le", "-"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output
            .stdout
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    /// Power of `frequency` in one channel of interleaved samples (Goertzel).
    pub(crate) fn tone_power(samples: &[f32], channels: usize, channel: usize, frequency: u32, rate: u32) -> f64 {
        let omega = 2.0 * std::f64::consts::PI * f64::from(frequency) / f64::from(rate);
        let coefficient = 2.0 * omega.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for frame in samples.chunks_exact(channels) {
            let s0 = f64::from(frame[channel]) + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coefficient * s1 * s2
    }

    /// Which of [`TONE_FREQUENCIES`] dominates each channel.
    pub(crate) fn dominant_tones(samples: &[f32], channels: usize, rate: u32) -> Vec<u32> {
        (0..channels)
            .map(|channel| {
                *TONE_FREQUENCIES
                    .iter()
                    .max_by(|a, b| {
                        tone_power(samples, channels, channel, **a, rate)
                            .total_cmp(&tone_power(samples, channels, channel, **b, rate))
                    })
                    .unwrap()
            })
            .collect()
    }

    pub(crate) fn tone_channels(dir: &Path, seconds: f64) -> Vec<Vec<Vec<u8>>> {
        TONE_FREQUENCIES
            .iter()
            .map(|frequency| tone_frames(dir, *frequency, seconds))
            .collect()
    }

    pub(crate) fn merged_decode_args(files: &[PathBuf]) -> Vec<String> {
        let mut args = Vec::new();
        let mut labels = String::new();
        for (index, file) in files.iter().enumerate() {
            args.extend(["-i".to_string(), file.to_string_lossy().to_string()]);
            labels.push_str(&format!("[{index}:a:0]"));
        }
        args.extend([
            "-filter_complex".into(),
            format!("{labels}amerge=inputs={}[out]", files.len()),
            "-map".into(),
            "[out]".into(),
        ]);
        args
    }

    #[test]
    fn native_ftr_and_the_deinterleaved_route_decode_the_same_samples() {
        if !aac_tools_available() {
            return;
        }
        if !native_ftr_decoder_on_path() {
            eprintln!("skipping: this FFmpeg has no native ftr decoder to cross-check against");
            return;
        }
        let dir = TrmTestDir::new();
        let channels = tone_channels(&dir.0, 2.0);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 4, 1);
        let path = write(&dir.0, "aligned.trm", &build_avi(&AviSpec::closed(4), &packets));
        let info = inspect_trm(&path).unwrap();
        assert!(info.native_blockers().is_empty());
        assert!(info.scrambled_frames > 0);

        let native = decode_f32(&[
            "-c:a".into(),
            "ftr".into(),
            "-i".into(),
            path.to_string_lossy().to_string(),
            "-map".into(),
            "0:a:0".into(),
        ]);
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        let merged = decode_f32(&merged_decode_args(&files));
        assert_eq!(
            native.len(),
            merged.len(),
            "both routes decode the same number of samples"
        );
        assert_eq!(
            native.len() as u64,
            info.frames_per_channel * 1024 * 4,
            "every frame decodes to 1024 samples per channel"
        );
        let differing = native
            .iter()
            .zip(&merged)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(
            differing, 0,
            "native ftr and the deinterleaved route differ in {differing} samples"
        );
        assert_eq!(dominant_tones(&merged, 4, TONE_RATE), TONE_FREQUENCIES.to_vec());
    }

    #[test]
    fn channel_files_decode_in_full_when_frames_carry_the_length_bits_ftr_ignores() {
        if !aac_tools_available() {
            return;
        }
        let dir = TrmTestDir::new();
        let mut channels = tone_channels(&dir.0, 2.0);
        for frame in channels.iter_mut().flatten() {
            frame[3] |= 0x01;
        }
        let (stream, lengths) = ftr_frame_stream(&channels);
        let path = write(
            &dir.0,
            "bits.trm",
            &build_avi(&AviSpec::closed(4), &round_packets(&stream, &lengths, 4, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.length_high_bit_frames, info.frames);
        assert_eq!(info.native_blockers(), vec!["frame-length-high-bits"]);
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        let merged = decode_f32(&merged_decode_args(&files));
        assert_eq!(merged.len() as u64, info.frames_per_channel * 1024 * 4);
        assert_eq!(dominant_tones(&merged, 4, TONE_RATE), TONE_FREQUENCIES.to_vec());
    }

    #[test]
    fn straddling_chunks_deinterleave_to_the_right_tones() {
        if !aac_tools_available() {
            return;
        }
        let dir = TrmTestDir::new();
        let channels = tone_channels(&dir.0, 2.0);
        let (stream, _) = ftr_frame_stream(&channels);
        let mut lead = channels[3][0].clone();
        lead[2] |= 0x02;
        lead.extend(&stream);
        let path = write(
            &dir.0,
            "straddle.trm",
            &build_avi(&AviSpec::closed(4), &sized_packets(&lead, 1000)),
        );
        let info = inspect_trm(&path).unwrap();
        assert!(info.straddling_frames > 0 && info.resync_skipped_bytes > 0);
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        let merged = decode_f32(&merged_decode_args(&files));
        assert_eq!(merged.len() as u64, info.frames_per_channel * 1024 * 4);
        assert_eq!(dominant_tones(&merged, 4, TONE_RATE), TONE_FREQUENCIES.to_vec());
    }

    // ── Layout details from the owner's latest notes ─────────────────────────

    fn utf16_chunk(id: &[u8; 4], text: &str, size: usize) -> Vec<u8> {
        let mut data: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        data.resize(size, 0);
        chunk(id, &data)
    }

    fn filetime_chunk(id: &[u8; 4], filetime: u64) -> Vec<u8> {
        chunk(id, &filetime.to_le_bytes())
    }

    /// 2026-01-01 09:00:00 as a FILETIME, as in the owner's sample.
    const SAMPLE_START: u64 = 0x01DC_7AFD_0268_A800;

    #[test]
    fn header_chunks_give_times_name_and_presence_only_for_private_text() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 3);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut spec = AviSpec::closed(2);
        spec.index = Index::Absolute;
        // Local time one hour ahead of UTC.
        let mut extra = filetime_chunk(b"TUTC", SAMPLE_START);
        extra.extend(filetime_chunk(b"TLOC", SAMPLE_START + 36_000_000_000 + 1_234_567));
        extra.extend(utf16_chunk(b"USER", "Private Person", 0x200));
        extra.extend(utf16_chunk(b"FILE", "Room 4_20260101-0900_01dc7afd0268a800.trm", 0x200));
        extra.extend(utf16_chunk(b"UDAT", "private note text", 0x400));
        extra.extend(chunk(b"FTRA", &le(&[1, 0, 0, 3, u32::MAX])));
        spec.header_extra = extra;
        // The level meter is counted in dwStreams but has no strl.
        spec.declared_streams = Some(2);
        spec.level_meter = true;
        let mut trailer = chunk(b"peak", &[1, 2, 3, 4]);
        trailer.extend(chunk(b"SIGN", &[9; 33]));
        trailer.extend(chunk(b"CERT", &[7; 64]));
        // A chunk after idx1 that claims more than the file holds is not
        // truncation of the audio.
        trailer.extend(b"zzzz\xFF\xFF\x00\x00");
        spec.trailer = trailer;
        let bytes = build_avi(&spec, &packets);
        let path = write(&dir.0, "header.trm", &bytes);

        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.start_utc.as_deref(), Some("2026-01-01T09:00:00Z"));
        assert_eq!(info.start_local.as_deref(), Some("2026-01-01T10:00:00.1234567"));
        assert_eq!(info.start_basis, Some(StartBasis::Local));
        assert_eq!(
            info.recorded_file_name.as_deref(),
            Some("Room 4_20260101-0900_01dc7afd0268a800.trm")
        );
        assert!(info.user_name_present && info.user_data_present);
        assert_eq!(info.signature, Some(SIGNED_NOT_VERIFIED));
        assert_eq!((info.declared_streams, info.stream_lists), (Some(2), 1));
        assert_eq!(info.ftra.map(|ftra| (ftra.a, ftra.d, ftra.e)), Some((1, 3, u32::MAX)));
        assert_eq!((info.header_scale, info.header_rate), (Some(1024), Some(TONE_RATE)));
        // Level-meter chunks are ignored; the audio is read as without them.
        assert_eq!((info.packets, info.frames_per_channel), (3, 3));
        assert!(!info.truncated);
        assert!(info.native_blockers().is_empty(), "{:?}", info.native_blockers());

        // The start feeds the BWF start: 10:00:00.1234567 local, at 16 kHz.
        let timing = info.start_timing().unwrap();
        assert_eq!(timing.source, "ftr-tloc");
        assert_eq!(timing.sample_rate, TONE_RATE);
        assert_eq!(timing.time_reference_samples, 36_000 * 16_000 + 1975);

        // The private text never reaches the record (or its debug form).
        let route = TrmRoute::new(info.clone(), DecodePath::NativeFtr, Vec::new());
        let json = serde_json::to_string(&route).unwrap();
        let debug = format!("{route:?}");
        for private in ["Private Person", "private note"] {
            assert!(!json.contains(private) && !debug.contains(private), "{private}");
        }
        assert!(json.contains("\"userNamePresent\":true"));

        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        for (channel, file) in files.iter().enumerate() {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&channels, channel, 3)
            );
        }
    }

    #[test]
    fn without_tloc_the_start_is_utc_then_the_file_name_then_unknown() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(1, 2);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 1, 1);
        let name = utf16_chunk(b"FILE", "CR4_20260101-0900_01dc7afd0268a800.tll", 0x200);
        let empty_private = [utf16_chunk(b"USER", "", 0x200), utf16_chunk(b"UDAT", "", 0x400)].concat();
        for (extra, basis, source) in [
            (
                [filetime_chunk(b"TUTC", SAMPLE_START), name.clone()].concat(),
                Some(StartBasis::Utc),
                Some("ftr-tutc"),
            ),
            (
                [name.clone(), empty_private.clone()].concat(),
                Some(StartBasis::FileName),
                Some("ftr-file-name"),
            ),
            (empty_private.clone(), None, None),
        ] {
            let mut spec = AviSpec::closed(1);
            spec.index = Index::Relative;
            spec.header_extra = extra;
            let info = inspect_trm(&write(&dir.0, "times.trm", &build_avi(&spec, &packets))).unwrap();
            assert_eq!(info.start_basis, basis);
            assert_eq!(info.start_timing().map(|timing| timing.source), source);
            if source.is_some() {
                // 09:00:00 at 16 kHz.
                assert_eq!(info.start_timing().unwrap().time_reference_samples, 9 * 3600 * 16_000);
            }
            assert!(info.start_local.is_none());
            assert!(!info.user_name_present && !info.user_data_present);
            assert_eq!(info.signature, None);
        }
        assert_eq!(
            recorder_file_name("x_20260101-0900_01dc7afd0268a800.trm"),
            Some(("x_20260101-0900_01dc7afd0268a800.trm".to_string(), SAMPLE_START))
        );
        assert_eq!(recorder_file_name("x_20260101-0900_01dc7afd0268a8.trm"), None);
        assert_eq!(recorder_file_name("plain name.trm"), None);
    }

    #[test]
    fn file_chunk_text_is_kept_only_in_the_recorders_name_format() {
        // The owner's sample name, and names the recorder's rules allow.
        for name in [
            "Synthetic sample (not FTR)_20260101-0900_01dc7afd0268a800.trm",
            "_20260101-0900_01dc7afd0268a800.trm",
            "Courtroom 4 [A]!$`+,-.=^'{}_20260101-0900_01dc7afd0268a800.tll",
        ] {
            assert_eq!(
                recorder_file_name(name).map(|(_, time)| time),
                Some(SAMPLE_START),
                "{name}"
            );
        }
        for text in [
            // Control characters, a line break, an underscore in the name
            // part, or a name part longer than 28 characters.
            "SECRETNOTE line\nbreak",
            "Room\t4_20260101-0900_01dc7afd0268a800.trm",
            "Room_4_20260101-0900_01dc7afd0268a800.trm",
            "Smith versus Jones, deposition of_20260101-0900_01dc7afd0268a800.trm",
            " Room 4_20260101-0900_01dc7afd0268a800.trm",
            // The date and time must be the FILETIME's, uppercase hex is
            // not the recorder's, and there must be an extension.
            "Room 4_20260102-0900_01dc7afd0268a800.trm",
            "Room 4_20260101-0900_01DC7AFD0268A800.trm",
            "Room 4_20260101-0900_01dc7afd0268a800",
            "Room 4_20260101-0900_01dc7afd0268a800.t r m",
        ] {
            assert_eq!(recorder_file_name(text), None, "{text:?}");
        }

        let dir = TrmTestDir::new();
        let channels = fake_frames(1, 2);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 1, 1);
        let mut spec = AviSpec::closed(1);
        spec.header_extra = utf16_chunk(b"FILE", "SECRETNOTE line\nbreak", 0x200);
        let info = inspect_trm(&write(&dir.0, "file.trm", &build_avi(&spec, &packets))).unwrap();
        assert_eq!(info.recorded_file_name, None);
        assert!(info.recorded_file_name_present);
        // No start from a name that is not the recorder's.
        assert_eq!(info.start_basis, None);
        let route = TrmRoute::new(info, DecodePath::Deinterleaved, Vec::new());
        let json = serde_json::to_string(&route).unwrap();
        assert!(!json.contains("SECRETNOTE") && !format!("{route:?}").contains("SECRETNOTE"));
        assert!(json.contains("\"recordedFileNamePresent\":true"));
    }

    #[test]
    fn private_text_is_seen_in_any_duplicate_chunk_and_anywhere_in_it() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(1, 2);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 1, 1);
        // Text in a first USER chunk, then an empty one; a UDAT chunk whose
        // text starts past 0x400 bytes; a FILE chunk whose text starts past
        // the 0x200 bytes the name is read from.
        let mut late = vec![0u8; 0x900];
        late[0x880..0x884].copy_from_slice(&[b'n', 0, b'o', 0]);
        let mut extra = utf16_chunk(b"USER", "Private Person", 0x200);
        extra.extend(utf16_chunk(b"USER", "", 0x200));
        extra.extend(chunk(b"UDAT", &late));
        extra.extend(chunk(b"FILE", &late[..0x300]));
        let mut spec = AviSpec::closed(1);
        spec.header_extra = extra;
        let info = inspect_trm(&write(&dir.0, "private.trm", &build_avi(&spec, &packets))).unwrap();
        assert!(info.user_name_present);
        assert!(info.user_data_present);
        let mut late_file = vec![0u8; 0x300];
        late_file[0x280] = b'x';
        let mut spec = AviSpec::closed(1);
        spec.header_extra = chunk(b"FILE", &late_file);
        let info = inspect_trm(&write(&dir.0, "late_file.trm", &build_avi(&spec, &packets))).unwrap();
        assert!(info.recorded_file_name_present && info.recorded_file_name.is_none());
        // Empty chunks are not text.
        let mut spec = AviSpec::closed(1);
        spec.header_extra = [utf16_chunk(b"USER", "", 0x200), utf16_chunk(b"UDAT", "", 0x400)].concat();
        let info = inspect_trm(&write(&dir.0, "empty.trm", &build_avi(&spec, &packets))).unwrap();
        assert!(!info.user_name_present && !info.user_data_present && !info.recorded_file_name_present);
    }

    /// Stereo ADTS frames: `left` Hz on channel 0, `right` Hz on channel 1.
    pub(crate) fn stereo_tone_frames(dir: &Path, left: u32, right: u32, seconds: f64) -> Vec<Vec<u8>> {
        let out = dir.join(format!("tone_{left}_{right}.aac"));
        let status = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={left}:duration={seconds}:sample_rate={TONE_RATE}"
            ))
            .args(["-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={right}:duration={seconds}:sample_rate={TONE_RATE}"
            ))
            .args(["-filter_complex", "[0:a][1:a]amerge=inputs=2"])
            .args(["-c:a", "aac", "-b:a", "64k", "-f", "adts", "-y"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&out).unwrap();
        fs::remove_file(&out).unwrap();
        let frames = split_adts_13(&bytes);
        assert!(frames.iter().all(|frame| adts_channel_config(frame) == 2));
        frames
    }

    /// ADTS frames by the standard 13-bit length.
    fn split_adts_13(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let mut pos = 0;
        while pos + 7 <= bytes.len() {
            let h = &bytes[pos..];
            let len = ((usize::from(h[3]) & 3) << 11) | (usize::from(h[4]) << 3) | (usize::from(h[5]) >> 5);
            frames.push(bytes[pos..pos + len].to_vec());
            pos += len;
        }
        frames
    }

    #[test]
    fn stereo_streams_are_split_into_stereo_files_in_channel_order() {
        let dir = TrmTestDir::new();
        // Layout only: two stereo streams of fake frames for 4 microphones.
        let mut streams = fake_frames(2, 5);
        for frame in streams.iter_mut().flatten() {
            frame[3] = 0x80; // channel configuration 2
        }
        let (stream, lengths) = ftr_frame_stream(&streams);
        let mut spec = AviSpec::closed(4);
        spec.tag = 0x8180;
        let path = write(
            &dir.0,
            "stereo.trm",
            &build_avi(&spec, &round_packets(&stream, &lengths, 2, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        assert_eq!(
            (
                info.stream_layout,
                info.channels,
                info.streams,
                info.channels_per_stream
            ),
            (Some(StreamLayout::Stereo), 4, 2, 2)
        );
        assert_eq!((info.frames, info.frames_per_channel), (10, 5));
        assert!(info.native_blockers().is_empty(), "{:?}", info.native_blockers());
        let route = TrmRoute::new(info.clone(), DecodePath::Deinterleaved, Vec::new());
        assert_eq!(route.input_count, 2);
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        assert_eq!(files.len(), 2);
        for (index, file) in files.iter().enumerate() {
            assert_eq!(
                split_adts(&fs::read(file).unwrap()),
                expected_channel(&streams, index, 5)
            );
        }
        files.iter().for_each(|file| fs::remove_file(file).unwrap());

        // Mono frames in a file whose tag says stereo streams: reported, not
        // sent to the native decoder, and not split.
        let mono = fake_frames(2, 5);
        let (stream, lengths) = ftr_frame_stream(&mono);
        let path = write(
            &dir.0,
            "mislabelled.trm",
            &build_avi(&spec, &round_packets(&stream, &lengths, 2, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.frame_channel_mismatches, 10);
        assert_eq!(info.native_blockers(), vec!["frame-channels-differ-from-header"]);
        assert_eq!(deinterleave_trm(&path, &dir.0).unwrap_err(), TRM_FRAME_CHANNELS_MESSAGE);
    }

    #[test]
    fn stereo_streams_decode_the_same_by_native_ftr_and_the_split_route() {
        if !aac_tools_available() {
            return;
        }
        if !native_ftr_decoder_on_path() {
            eprintln!("skipping: this FFmpeg has no native ftr decoder to cross-check against");
            return;
        }
        let dir = TrmTestDir::new();
        let [a, b, c, d] = TONE_FREQUENCIES;
        let streams = vec![
            stereo_tone_frames(&dir.0, a, b, 2.0),
            stereo_tone_frames(&dir.0, c, d, 2.0),
        ];
        let (stream, lengths) = ftr_frame_stream(&streams);
        let mut spec = AviSpec::closed(4);
        spec.tag = 0x8180;
        let path = write(
            &dir.0,
            "stereo.trm",
            &build_avi(&spec, &round_packets(&stream, &lengths, 2, 1)),
        );
        let info = inspect_trm(&path).unwrap();
        assert!(info.native_blockers().is_empty(), "{:?}", info.native_blockers());
        let native = decode_f32(&[
            "-c:a".into(),
            "ftr".into(),
            "-i".into(),
            path.to_string_lossy().to_string(),
            "-map".into(),
            "0:a:0".into(),
        ]);
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        let merged = decode_f32(&merged_decode_args(&files));
        assert_eq!(native.len() as u64, info.frames_per_channel * 1024 * 4);
        assert_eq!(native.len(), merged.len());
        let differing = native
            .iter()
            .zip(&merged)
            .filter(|(x, y)| x.to_bits() != y.to_bits())
            .count();
        assert_eq!(differing, 0, "the routes differ in {differing} samples");
        // Stream k holds channels 2k and 2k + 1, in order.
        assert_eq!(dominant_tones(&merged, 4, TONE_RATE), TONE_FREQUENCIES.to_vec());
        assert_eq!(dominant_tones(&native, 4, TONE_RATE), TONE_FREQUENCIES.to_vec());
    }

    pub(crate) fn vendor_notes_sample() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fixtures/trm/vendor-notes-samples/sample_hex_report.trm")
    }

    #[test]
    fn vendor_notes_sample_is_inspected_as_its_hex_report_says() {
        let path = vendor_notes_sample();
        let before = fs::read(&path).unwrap();
        assert_eq!(before.len(), 106_120);
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.tag, 0x4180);
        assert_eq!(info.audio_codec, AudioCodec::FtrAac);
        assert_eq!(
            (
                info.stream_layout,
                info.channels,
                info.streams,
                info.channels_per_stream
            ),
            (Some(StreamLayout::Mono), 4, 4, 1)
        );
        assert_eq!((info.sample_rate, info.format_sample_rate), (Some(16_000), 16_000));
        assert_eq!(
            (info.header_scale, info.header_rate, info.header_chunk_count),
            (Some(1024), Some(16_000), Some(240))
        );
        assert_eq!((info.declared_streams, info.stream_lists), (Some(1), 1));
        assert!(info.start_boundary && info.end_boundary && !info.open_ended);
        assert_eq!(info.index_kind, IndexKind::Idx1Absolute);
        assert_eq!((info.packets, info.frames_per_channel, info.frames), (240, 240, 960));
        assert_eq!(info.scrambled_frames, 960);
        assert_eq!(info.duration_secs, Some(15.36));
        assert_eq!(
            info.ftra,
            Some(FtraInfo {
                a: 1,
                b: 0,
                c: 0,
                d: 240,
                e: u32::MAX
            })
        );
        assert_eq!(info.start_utc.as_deref(), Some("2026-01-01T09:00:00Z"));
        assert_eq!(info.start_local.as_deref(), Some("2026-01-01T09:00:00"));
        assert_eq!(
            info.recorded_file_name.as_deref(),
            Some("Synthetic sample (not FTR)_20260101-0900_01dc7afd0268a800.trm")
        );
        assert!(!info.user_name_present && !info.user_data_present);
        assert_eq!(info.signature, None);
        assert!(!info.truncated && info.video_streams == 0);
        assert_eq!(fs::read(&path).unwrap(), before, "the original is never modified");
    }

    #[test]
    fn vendor_notes_sample_goes_to_the_native_decoder_and_starts_at_nine_utc() {
        let info = inspect_trm(&vendor_notes_sample()).unwrap();
        let native = crate::helpers::FeedOptions {
            native_ftr: true,
            native_failed: false,
        };
        assert_eq!(choose_route(&info, native), (DecodePath::NativeFtr, Vec::new()));
        let timing = info.start_timing().unwrap();
        assert_eq!(timing.source, "ftr-tloc");
        assert_eq!(
            (timing.time_reference_samples, timing.sample_rate),
            (9 * 3600 * 16_000, 16_000)
        );
        let record = crate::manifest::start_time_record(Some(timing), Some(48_000), false, "wav");
        assert!(record.known && record.written_to_output);
        assert_eq!(record.seconds_since_midnight, Some(32_400.0));
        assert_eq!(record.output_time_reference_samples, Some(9 * 3600 * 48_000));
        assert!(record.note.contains("TLOC"), "{}", record.note);
    }

    #[test]
    fn vendor_notes_sample_decodes_the_same_natively_and_split() {
        if !aac_tools_available() {
            return;
        }
        if !native_ftr_decoder_on_path() {
            eprintln!("skipping: this FFmpeg has no native ftr decoder to cross-check against");
            return;
        }
        let dir = TrmTestDir::new();
        let path = vendor_notes_sample();
        let native = decode_f32(&[
            "-c:a".into(),
            "ftr".into(),
            "-i".into(),
            path.to_string_lossy().to_string(),
            "-map".into(),
            "0:a:0".into(),
        ]);
        assert_eq!(native.len(), 245_760 * 4, "245 760 samples per channel");
        assert!(native.iter().all(|sample| *sample == 0.0), "the sample is silent");
        let (_, files) = deinterleave_trm(&path, &dir.0).unwrap();
        assert_eq!(files.len(), 4);
        let merged = decode_f32(&merged_decode_args(&files));
        assert_eq!(merged.len(), native.len());
        assert!(native.iter().zip(&merged).all(|(x, y)| x.to_bits() == y.to_bits()));
    }
}
