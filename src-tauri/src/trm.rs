//! FTR (For The Record) `.trm` / `.tll` recordings: inspection and
//! per-channel extraction.
//!
//! Source of the layout below: owner-supplied interoperability notes; not yet
//! confirmed against a recorded file. It is described here in our own words.
//! Everything is little-endian.
//!
//! - The file is a RIFF `AVI ` container. An optional 20-byte `FTRA` chunk
//!   (five u32 values) can sit before `LIST hdrl`; it is reported, never used.
//! - `avih` carries two session flags in `dwFlags`: `0x20000000` marks the
//!   first file of a recording session and `0x40000000` the last.
//! - The audio stream header (`strh` type `auds`) is stream 0; video streams
//!   (`vids`) follow. An audio `dwLength` of `0x7FFFFFFF` means the recorder
//!   never closed the file, so that length says nothing about the audio.
//! - The audio format (`strf`) is a WAVEFORMATEX whose tag, masked with
//!   `0x3FFF`, is `0x0180` for FTR's AAC; `nChannels` is the microphone count.
//! - Audio chunks (`00wb`) are found through `idx1` (offsets relative to the
//!   `movi` list or absolute) or, without a usable index, by walking `movi`.
//! - The chunk payloads, joined in file order, are one run of ADTS AAC-LC
//!   frames that may cross chunk boundaries. The first frame whose ADTS
//!   private bit is clear belongs to channel 0; from there frame `k` belongs
//!   to channel `k mod N`. A frame with the original/copy bit set has five
//!   payload bytes scrambled by a self-inverse swap-and-invert, undone here.
//!   Each channel is then a plain mono ADTS stream.
//!
//! Reading streams through the file in fixed-size pieces: memory stays
//! bounded by one piece, one partial frame and one round of frames, whatever
//! the file size. Nothing here writes to the original file.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// WAVEFORMATEX tag of FTR's AAC once masked with [`FORMAT_TAG_MASK`].
pub(crate) const FTR_AAC_TAG: u16 = 0x0180;
/// The masked-tag variant FFmpeg's RIFF table maps to its `ftr` decoder.
pub(crate) const FFMPEG_FTR_TAG: u16 = 0x4180;
const FORMAT_TAG_MASK: u16 = 0x3FFF;
const OPEN_ENDED_LENGTH: u32 = 0x7FFF_FFFF;
const SESSION_FIRST_FLAG: u32 = 0x2000_0000;
const SESSION_LAST_FLAG: u32 = 0x4000_0000;
/// The registry and every conversion path cap court recordings at 16 channels.
pub(crate) const MAX_TRM_CHANNELS: u16 = 16;
const MAX_HEADER_LIST_BYTES: u64 = 1024 * 1024;
const READ_PIECE_BYTES: usize = 64 * 1024;
const ADTS_HEADER_BYTES: usize = 7;
/// Scrambled bytes reach index 12, so a frame must be at least this long.
const SCRAMBLE_MIN_FRAME_BYTES: usize = 13;
const SAMPLES_PER_FRAME: u64 = 1024;
const ADTS_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350,
];

pub(crate) const TRM_CANCELLED_MESSAGE: &str = crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE;

/// The optional `FTRA` chunk, as read. FTR's player is described as using it
/// only for `.avi` audio, so DepoAudio reports it and does not act on it.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum IndexKind {
    /// `idx1` offsets are absolute file positions.
    #[serde(rename = "idx1-absolute")]
    Idx1Absolute,
    /// `idx1` offsets count from the `movi` list.
    #[serde(rename = "idx1-relative")]
    Idx1Relative,
    /// No usable `idx1`: the `movi` list was walked chunk by chunk.
    #[serde(rename = "walked")]
    Walked,
}

/// What one full read of a `.trm` found. Counts are of what was read, not of
/// what the header claims.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrmInfo {
    /// The WAVEFORMATEX tag as stored (unmasked).
    pub tag: u16,
    /// `nChannels`: the number of microphone channels.
    pub channels: u16,
    /// From the first channel-0 ADTS header; None when no frame was found.
    pub sample_rate: Option<u32>,
    pub session_first: bool,
    pub session_last: bool,
    /// The audio length field says the recorder never closed the file.
    pub open_ended: bool,
    pub index_kind: IndexKind,
    /// Audio chunks read.
    pub packets: u64,
    /// Frames delivered to the channels (whole rounds only).
    pub frames: u64,
    pub frames_per_channel: u64,
    /// Delivered frames that needed descrambling.
    pub scrambled_frames: u64,
    /// Frames that start in one audio chunk and end in a later one.
    pub straddling_frames: u64,
    /// Audio chunks that do not start and end on a channel-0 round boundary.
    pub misaligned_packets: u64,
    /// Bytes skipped to reach a channel-0 frame (at the start, or after
    /// unreadable data).
    pub resync_skipped_bytes: u64,
    /// The last round was missing channels; its frames were not delivered.
    pub incomplete_final_round: bool,
    /// Frames read but not delivered (incomplete rounds).
    pub dropped_frames: u64,
    /// The file ends inside a chunk or a frame, or its chunk list stops at
    /// unreadable data.
    pub truncated: bool,
    pub ftra: Option<FtraInfo>,
    pub video_streams: u32,
    /// Delivered frames per channel × 1024 samples ÷ sample rate.
    pub duration_secs: Option<f64>,
}

impl TrmInfo {
    /// Whether FFmpeg's native `ftr` decoder can be expected to read this
    /// file as it is: the reasons it cannot, empty when it can. The native
    /// decoder takes each audio chunk as whole rounds starting at channel 0,
    /// does not join frames across chunks or look for the channel-0 marker,
    /// and FFmpeg maps only the 0x4180 tag to it.
    pub(crate) fn native_blockers(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.tag != FFMPEG_FTR_TAG {
            reasons.push("format-tag-not-mapped-by-ffmpeg");
        }
        if self.index_kind == IndexKind::Walked {
            reasons.push("no-usable-index");
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
        if self.resync_skipped_bytes > 0 {
            reasons.push("data-before-channel-0");
        }
        if self.dropped_frames > 0 {
            reasons.push("incomplete-rounds");
        }
        reasons
    }
}

/// How FFmpeg reads a TRM recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum DecodePath {
    /// The file as it is, with FFmpeg's native `ftr` decoder.
    #[serde(rename = "native-ftr")]
    NativeFtr,
    /// One mono ADTS file per channel, written by [`deinterleave_trm`] and
    /// joined in channel order with FFmpeg's `amerge`.
    #[serde(rename = "deinterleaved")]
    Deinterleaved,
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
}

/// Native `ftr` when the layout allows it and the decoder is present and has
/// not already failed on this file; otherwise the channels are split first.
pub(crate) fn choose_route(info: &TrmInfo, options: crate::helpers::FeedOptions) -> (DecodePath, Vec<String>) {
    let mut reasons: Vec<String> = info.native_blockers().into_iter().map(String::from).collect();
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

// ── Container header ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Layout {
    tag: u16,
    channels: u16,
    session_first: bool,
    session_last: bool,
    open_ended: bool,
    ftra: Option<FtraInfo>,
    video_streams: u32,
    /// File offset of the `movi` list type, which idx1 offsets count from.
    movi_pos: u64,
    /// End of the `movi` list data (EOF when its size is unusable).
    movi_end: u64,
    /// `idx1` data start and usable byte count.
    idx1: Option<(u64, u64)>,
    truncated: bool,
}

struct StreamHeader {
    kind: [u8; 4],
    length: Option<u32>,
    format: Vec<u8>,
}

fn parse_header_list(body: &[u8], layout: &mut Layout) -> Result<(), String> {
    let mut flags = None;
    let mut streams: Vec<StreamHeader> = Vec::new();
    for (id, data) in sub_chunks(body) {
        match &id {
            b"avih" => flags = u32_at(data, 12),
            b"LIST" if data.get(..4) == Some(b"strl") => {
                let mut stream = StreamHeader {
                    kind: [0; 4],
                    length: None,
                    format: Vec::new(),
                };
                for (inner, inner_data) in sub_chunks(&data[4..]) {
                    match &inner {
                        b"strh" => {
                            stream.kind = fourcc_at(inner_data, 0).unwrap_or([0; 4]);
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
    layout.session_first = flags & SESSION_FIRST_FLAG != 0;
    layout.session_last = flags & SESSION_LAST_FLAG != 0;
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
    layout.open_ended = audio.length == Some(OPEN_ENDED_LENGTH);
    Ok(())
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
    let mut layout = Layout {
        tag: 0,
        channels: 0,
        session_first: false,
        session_last: false,
        open_ended: false,
        ftra: None,
        video_streams: 0,
        movi_pos: 0,
        movi_end: 0,
        idx1: None,
        truncated: false,
    };
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
        match &id {
            b"LIST" if available == 12 && &head[8..12] == b"hdrl" => {
                if size > MAX_HEADER_LIST_BYTES {
                    return Err(
                        "This FTR recording's header list is implausibly large; the file may be damaged.".into(),
                    );
                }
                let take = size.min(remaining);
                let mut body = vec![0u8; take as usize];
                file.seek(SeekFrom::Start(data)).map_err(io_error)?;
                file.read_exact(&mut body).map_err(io_error)?;
                parse_header_list(body.get(4..).unwrap_or_default(), &mut layout)?;
                have_header = true;
            }
            b"LIST" if available == 12 && &head[8..12] == b"movi" => {
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
                let mut values = [0u8; 20];
                file.seek(SeekFrom::Start(data)).map_err(io_error)?;
                file.read_exact(&mut values).map_err(io_error)?;
                let value = |index: usize| u32_at(&values, index * 4).unwrap_or(0);
                layout.ftra = Some(FtraInfo {
                    a: value(0),
                    b: value(1),
                    c: value(2),
                    d: value(3),
                    e: value(4),
                });
            }
            b"idx1" => {
                // A zero size means the index was never closed: it runs to EOF.
                let usable = if size == 0 { remaining } else { size.min(remaining) };
                layout.idx1 = Some((data, usable));
                if size > remaining {
                    layout.truncated = true;
                }
            }
            _ => {}
        }
        let next = data + size + (size & 1);
        if next > file_len {
            if !matches!(&id, b"idx1") {
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
    if layout.tag & FORMAT_TAG_MASK != FTR_AAC_TAG {
        return Err(format!(
            "This recording's audio is not FTR AAC (format tag 0x{:04X}), so DepoAudio cannot read it as an FTR recording.",
            layout.tag
        ));
    }
    if layout.channels == 0 || layout.channels > MAX_TRM_CHANNELS {
        return Err(format!(
            "This FTR recording reports {} channels; DepoAudio reads 1 to {MAX_TRM_CHANNELS}. The header may be damaged.",
            layout.channels
        ));
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

/// Deals frames from the joined chunk payloads to channels in rounds.
struct Dealer<'a> {
    channels: usize,
    buffer: Vec<u8>,
    synced: bool,
    round: Vec<u8>,
    round_ends: Vec<usize>,
    round_scrambled: u64,
    /// The bytes left at the last chunk boundary start a frame begun there.
    carry_pending: bool,
    packet_start_ok: bool,
    sample_rate: Option<u32>,
    stats: DealStats,
    sink: Option<&'a mut FrameSink<'a>>,
}

#[derive(Debug, Default, Clone)]
struct DealStats {
    packets: u64,
    frames: u64,
    rounds: u64,
    scrambled: u64,
    straddling: u64,
    misaligned: u64,
    resync_skipped: u64,
    dropped: u64,
    incomplete_final_round: bool,
    trailing_bytes: u64,
}

impl<'a> Dealer<'a> {
    fn new(channels: usize, sink: Option<&'a mut FrameSink<'a>>) -> Self {
        Self {
            channels,
            buffer: Vec::with_capacity(READ_PIECE_BYTES + 4096),
            synced: false,
            round: Vec::new(),
            round_ends: Vec::with_capacity(channels),
            round_scrambled: 0,
            carry_pending: false,
            packet_start_ok: true,
            sample_rate: None,
            stats: DealStats::default(),
            sink,
        }
    }

    fn begin_packet(&mut self) {
        self.packet_start_ok = self.buffer.is_empty() && self.round_ends.is_empty();
    }

    fn end_packet(&mut self) -> Result<(), String> {
        self.process()?;
        self.stats.packets += 1;
        let end_ok = self.buffer.is_empty() && self.round_ends.is_empty() && self.synced;
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

    fn drop_round(&mut self) {
        self.stats.dropped += self.round_ends.len() as u64;
        self.round.clear();
        self.round_ends.clear();
        self.round_scrambled = 0;
    }

    fn process(&mut self) -> Result<(), String> {
        let mut index = 0usize;
        while self.buffer.len() - index >= ADTS_HEADER_BYTES {
            let at_carry = index == 0 && self.carry_pending;
            let header = &self.buffer[index..index + ADTS_HEADER_BYTES];
            let length = ftr_frame_length(header);
            if !is_ftr_frame_header(header) || length < ADTS_HEADER_BYTES {
                // Unreadable data: give up the round in progress and look for
                // the next channel-0 frame, byte by byte.
                if self.synced {
                    self.drop_round();
                    self.synced = false;
                }
                self.stats.resync_skipped += 1;
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
            if !self.synced {
                if !channel_zero {
                    self.stats.resync_skipped += length as u64;
                    index += length;
                    continue;
                }
                self.synced = true;
            }
            if self.round_ends.is_empty() && self.sample_rate.is_none() {
                self.sample_rate = adts_sample_rate(&self.buffer[index..]);
            }
            if at_carry {
                self.stats.straddling += 1;
            }
            let start = self.round.len();
            self.round.extend_from_slice(&self.buffer[index..index + length]);
            if descramble_frame(&mut self.round[start..]) {
                self.round_scrambled += 1;
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
        self.stats.frames += self.round_ends.len() as u64;
        self.stats.rounds += 1;
        self.stats.scrambled += self.round_scrambled;
        self.round.clear();
        self.round_ends.clear();
        self.round_scrambled = 0;
        Ok(())
    }

    fn finish(mut self) -> (DealStats, Option<u32>) {
        if !self.round_ends.is_empty() {
            self.stats.incomplete_final_round = true;
            self.drop_round();
        }
        self.stats.trailing_bytes = self.buffer.len() as u64;
        (self.stats, self.sample_rate)
    }
}

// ── Reading the audio chunks ─────────────────────────────────────────────────

enum IndexOutcome {
    Done(IndexKind, bool),
    /// The index is missing, empty or does not match the chunks it lists.
    Unusable,
}

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
    reader: &mut BufReader<File>,
    data: u64,
    size: u64,
    piece: &mut [u8],
    dealer: &mut Dealer<'_>,
    cancelled: Cancel<'_>,
) -> Result<(), String> {
    reader.seek(SeekFrom::Start(data)).map_err(io_error)?;
    dealer.begin_packet();
    let mut left = size;
    while left > 0 {
        check_cancel(cancelled)?;
        let take = left.min(piece.len() as u64) as usize;
        reader.read_exact(&mut piece[..take]).map_err(io_error)?;
        dealer.feed(&piece[..take])?;
        left -= take as u64;
    }
    dealer.end_packet()
}

/// Read every `00wb` chunk `idx1` lists, checking each listed chunk header
/// against the file before using it. Any mismatch makes the whole index
/// unusable, checked before a single byte is delivered (`dry_run`), so the
/// caller can fall back to walking without having written anything.
fn read_by_index(
    path: &Path,
    layout: &Layout,
    file_len: u64,
    mut dealer: Option<&mut Dealer<'_>>,
    cancelled: Cancel<'_>,
) -> Result<IndexOutcome, String> {
    let Some((idx_start, idx_len)) = layout.idx1 else {
        return Ok(IndexOutcome::Unusable);
    };
    let mut index = BufReader::new(File::open(path).map_err(io_error)?);
    index.seek(SeekFrom::Start(idx_start)).map_err(io_error)?;
    let mut payload = BufReader::with_capacity(READ_PIECE_BYTES, File::open(path).map_err(io_error)?);
    let mut piece = vec![0u8; READ_PIECE_BYTES];
    let mut kind = None;
    let mut last_end = layout.movi_pos;
    let mut truncated = false;
    let mut entries = idx_len / 16;
    let mut entry = [0u8; 16];
    let mut audio_entries = 0u64;
    while entries > 0 {
        entries -= 1;
        if audio_entries.is_multiple_of(4096) {
            check_cancel(cancelled)?;
        }
        index.read_exact(&mut entry).map_err(io_error)?;
        if &entry[0..4] != b"00wb" {
            continue;
        }
        let offset = u64::from(u32_at(&entry, 8).unwrap_or(0));
        let size = u64::from(u32_at(&entry, 12).unwrap_or(0));
        // The first audio entry decides for the whole index: an offset
        // before the movi list can only be relative to it. Later relative
        // offsets can pass movi_pos in value, so they are not tested again.
        let kind = *kind.get_or_insert(if offset < layout.movi_pos {
            IndexKind::Idx1Relative
        } else {
            IndexKind::Idx1Absolute
        });
        let header = match kind {
            IndexKind::Idx1Relative => layout.movi_pos.saturating_add(offset),
            _ => offset,
        };
        if header < last_end || header + 8 > file_len {
            return Ok(IndexOutcome::Unusable);
        }
        let mut chunk = [0u8; 8];
        payload.seek(SeekFrom::Start(header)).map_err(io_error)?;
        payload.read_exact(&mut chunk).map_err(io_error)?;
        if &chunk[0..4] != b"00wb" || u64::from(u32_at(&chunk, 4).unwrap_or(0)) != size {
            return Ok(IndexOutcome::Unusable);
        }
        let data = header + 8;
        let available = size.min(file_len - data);
        if available < size {
            truncated = true;
        }
        audio_entries += 1;
        last_end = data + available;
        if let Some(dealer) = dealer.as_deref_mut() {
            feed_payload(&mut payload, data, available, &mut piece, dealer, cancelled)?;
        }
    }
    match kind {
        Some(kind) if audio_entries > 0 => Ok(IndexOutcome::Done(kind, truncated)),
        _ => Ok(IndexOutcome::Unusable),
    }
}

/// Walk `movi` from its first chunk: skip each chunk (padded to even), read
/// `00wb` payloads, step into `LIST` sub-lists, and stop at a zero chunk id
/// (the end of the written data) or at a chunk id that is not printable.
/// Returns whether the walk ended early (cut-off chunk or unreadable id).
fn read_by_walking(
    path: &Path,
    layout: &Layout,
    file_len: u64,
    dealer: &mut Dealer<'_>,
    cancelled: Cancel<'_>,
) -> Result<bool, String> {
    let mut reader = BufReader::with_capacity(READ_PIECE_BYTES, File::open(path).map_err(io_error)?);
    let mut piece = vec![0u8; READ_PIECE_BYTES];
    let end = layout.movi_end.min(file_len);
    let mut pos = layout.movi_pos + 4;
    let mut steps = 0u64;
    while pos + 8 <= end {
        steps += 1;
        if steps.is_multiple_of(4096) {
            check_cancel(cancelled)?;
        }
        let mut head = [0u8; 8];
        reader.seek(SeekFrom::Start(pos)).map_err(io_error)?;
        reader.read_exact(&mut head).map_err(io_error)?;
        let id = &head[0..4];
        if id == [0, 0, 0, 0] {
            return Ok(false);
        }
        if !id.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b' ') {
            return Ok(true);
        }
        let size = u64::from(u32_at(&head, 4).unwrap_or(0));
        if id == b"LIST" {
            pos += 12;
            continue;
        }
        let data = pos + 8;
        let available = size.min(end - data);
        if id == b"00wb" {
            feed_payload(&mut reader, data, available, &mut piece, dealer, cancelled)?;
        }
        if available < size {
            return Ok(true);
        }
        pos = data + size + (size & 1);
    }
    // Fewer than 8 bytes left: a chunk header was cut off.
    Ok(pos < end)
}

fn build_info(
    layout: &Layout,
    kind: IndexKind,
    truncated: bool,
    stats: DealStats,
    sample_rate: Option<u32>,
) -> TrmInfo {
    let channels = u64::from(layout.channels);
    let duration_secs = sample_rate
        .filter(|rate| *rate > 0)
        .map(|rate| (stats.rounds * SAMPLES_PER_FRAME) as f64 / f64::from(rate));
    TrmInfo {
        tag: layout.tag,
        channels: layout.channels,
        sample_rate,
        session_first: layout.session_first,
        session_last: layout.session_last,
        open_ended: layout.open_ended,
        index_kind: kind,
        packets: stats.packets,
        frames: stats.rounds * channels,
        frames_per_channel: stats.rounds,
        scrambled_frames: stats.scrambled,
        straddling_frames: stats.straddling,
        misaligned_packets: stats.misaligned,
        resync_skipped_bytes: stats.resync_skipped,
        incomplete_final_round: stats.incomplete_final_round,
        dropped_frames: stats.dropped,
        truncated: layout.truncated || truncated || stats.trailing_bytes > 0,
        ftra: layout.ftra,
        video_streams: layout.video_streams,
        duration_secs,
    }
}

fn scan<'s>(
    path: &Path,
    index_hint: Option<IndexKind>,
    sink: Option<&'s mut FrameSink<'s>>,
    cancelled: Cancel<'_>,
) -> Result<TrmInfo, String> {
    check_cancel(cancelled)?;
    let mut file = File::open(path).map_err(io_error)?;
    let file_len = file.metadata().map_err(io_error)?.len();
    let layout = read_layout(&mut file, file_len)?;
    drop(file);

    let try_index = index_hint.is_none_or(|kind| kind != IndexKind::Walked);
    // Check the whole index before delivering anything, so a damaged index
    // never leaves half-written channels behind.
    let index_ok = try_index
        && matches!(
            read_by_index(path, &layout, file_len, None, cancelled)?,
            IndexOutcome::Done(..)
        );
    let mut dealer = Dealer::new(usize::from(layout.channels), sink);
    let (kind, truncated) = if index_ok {
        match read_by_index(path, &layout, file_len, Some(&mut dealer), cancelled)? {
            IndexOutcome::Done(kind, truncated) => (kind, truncated),
            IndexOutcome::Unusable => return Err("The FTR recording changed while it was being read.".into()),
        }
    } else {
        let truncated = read_by_walking(path, &layout, file_len, &mut dealer, cancelled)?;
        (IndexKind::Walked, truncated)
    };
    let (stats, sample_rate) = dealer.finish();
    Ok(build_info(&layout, kind, truncated, stats, sample_rate))
}

/// Read a `.trm` through once and report its layout. Never modifies it.
pub(crate) fn inspect_trm(path: &Path) -> Result<TrmInfo, String> {
    inspect_trm_with_cancel(path, None)
}

pub(crate) fn inspect_trm_with_cancel(path: &Path, cancelled: Cancel<'_>) -> Result<TrmInfo, String> {
    scan(path, None, None, cancelled)
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

/// Write each channel of a `.trm` to its own new mono ADTS `.aac` file in
/// `out_dir` (descrambled, whole rounds only, so every channel has the same
/// frame count). Streams: no channel is held in memory. The original is only
/// read. On any error, no channel file is left behind.
#[cfg(test)]
pub(crate) fn deinterleave_trm(path: &Path, out_dir: &Path) -> Result<(TrmInfo, Vec<PathBuf>), String> {
    deinterleave_trm_with_cancel(path, out_dir, None, None)
}

/// [`deinterleave_trm`] with cancellation, reusing an earlier inspection's
/// index choice when given.
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
    if info.frames_per_channel == 0 {
        return Err(
            "No complete round of FTR audio frames was found in this recording, so there is nothing to convert.".into(),
        );
    }
    let stem = format!("depoaudio_trm_{}", uuid::Uuid::new_v4().simple());
    let mut files = ChannelFiles {
        paths: Vec::new(),
        keep: false,
    };
    let mut writers = Vec::with_capacity(usize::from(info.channels));
    for channel in 0..info.channels {
        let channel_path = out_dir.join(format!("{stem}_ch{channel}.aac"));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&channel_path)
            .map_err(|error| format!("Could not create a temporary channel file: {error}"))?;
        files.paths.push(channel_path);
        writers.push(BufWriter::new(file));
    }
    let mut sink = |channel: usize, frame: &[u8]| -> Result<(), String> {
        writers[channel]
            .write_all(frame)
            .map_err(|error| format!("Could not write a temporary channel file: {error}"))
    };
    let written = scan(path, Some(info.index_kind), Some(&mut sink), cancelled)?;
    for writer in writers {
        let file = writer
            .into_inner()
            .map_err(|error| format!("Could not write a temporary channel file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not write a temporary channel file: {error}"))?;
    }
    check_cancel(cancelled)?;
    if written.frames_per_channel != info.frames_per_channel {
        return Err("The FTR recording changed while it was being read.".into());
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

    #[derive(Clone, Copy, PartialEq)]
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
    }

    impl AviSpec {
        pub(crate) fn closed(channels: u16) -> Self {
            Self {
                tag: FFMPEG_FTR_TAG,
                channels,
                flags: 0,
                length: 0,
                index: Index::Absolute,
                ftra: None,
                video_streams: 0,
                open_movi: false,
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
        let streams = 1 + spec.video_streams;
        let mut avih = le(&[0, 0, 0, spec.flags, packets.len() as u32, 0, streams, 0, 0, 0]);
        avih.resize(56, 0);
        let mut strh = b"auds".to_vec();
        strh.extend(le(&[0, 0, 0, 0, 1024, TONE_RATE, 0, spec.length, 4096, u32::MAX, 0]));
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
        let movi_header = out.len();
        let movi_pos = movi_header + 8;
        let mut movi = Vec::new();
        let mut entries = Vec::new();
        for packet in packets {
            let header = movi_pos + 4 + movi.len();
            entries.push((header, packet.len()));
            movi.extend(chunk(b"00wb", packet));
        }
        out.extend(list(b"movi", &movi));
        if spec.open_movi {
            out[movi_header + 4..movi_header + 8].copy_from_slice(&0u32.to_le_bytes());
        }
        if spec.index != Index::None {
            let mut idx = Vec::new();
            for (header, size) in entries {
                let offset = match spec.index {
                    Index::Relative => header - movi_pos,
                    _ => header,
                };
                idx.extend(b"00wb");
                idx.extend(le(&[0x10, offset as u32, size as u32]));
            }
            let mut idx1 = chunk(b"idx1", &idx);
            if spec.index == Index::AbsoluteZeroSize {
                idx1[4..8].copy_from_slice(&0u32.to_le_bytes());
            }
            out.extend(idx1);
        }
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
        let packets = round_packets(&stream, &lengths, 3, 2);
        for (index, kind) in [
            (Index::Relative, IndexKind::Idx1Relative),
            (Index::AbsoluteZeroSize, IndexKind::Idx1Absolute),
        ] {
            let mut spec = AviSpec::closed(3);
            spec.index = index;
            let path = write(&dir.0, "indexed.trm", &build_avi(&spec, &packets));
            let info = inspect_trm(&path).unwrap();
            assert_eq!(info.index_kind, kind);
            assert_eq!(info.packets, 3);
            assert_eq!(info.frames_per_channel, 6);
            assert_eq!(info.misaligned_packets, 0);
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
    fn session_flags_ftra_and_video_streams_are_reported() {
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
                (info.session_first, info.session_last),
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
    fn other_ftr_aac_tags_are_read_but_not_sent_to_the_native_decoder() {
        let dir = TrmTestDir::new();
        let channels = fake_frames(2, 3);
        let (stream, lengths) = ftr_frame_stream(&channels);
        let packets = round_packets(&stream, &lengths, 2, 1);
        let mut spec = AviSpec::closed(2);
        spec.tag = 0x0180;
        let path = write(&dir.0, "tag.trm", &build_avi(&spec, &packets));
        let info = inspect_trm(&path).unwrap();
        assert_eq!(info.native_blockers(), vec!["format-tag-not-mapped-by-ffmpeg"]);

        spec.tag = 0x0001;
        let path = write(&dir.0, "pcm.trm", &build_avi(&spec, &packets));
        assert!(inspect_trm(&path).unwrap_err().contains("0x0001"));
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
}
