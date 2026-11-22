//! Liberty / BIS Digital `.dcr` court recordings (experimental).
//!
//! Layout source: owner-supplied interoperability notes; not yet confirmed
//! against a recorded file. Every test here builds its own synthetic file;
//! no real recording has been read. In our own words:
//!
//! - A 16-byte header: the ASCII signature `HGCRLCRS`, then a little-endian
//!   u32 major and u32 minor version (version = major × 100 + minor).
//! - Framed chunks: `<tag>#bgn`, a u32 id, the payload, then `<tag>#end`.
//! - A chunk directory at the end of the file: 24-byte entries (tag, id,
//!   offset and total size of the framed chunk), closed by `head` plus the
//!   directory's own offset and size. Version 21.00 and later files end with
//!   a 12-byte checksum tail after it; DepoAudio does not know how those
//!   checksums are weighted, so it never verifies them (`checksum: "not
//!   checked"`).
//! - The audio format: version 21.00+ keeps an XML document in the `meta`
//!   chunk (channel mask, a hex WAVEFORMATEX for ONE channel, channel names,
//!   and a protection flag); older files keep a raw WAVEFORMATEX in `wfmt`
//!   and the channel mask at the start of `chnl`.
//! - The `data` chunk: one block per active channel at each step ("atom"),
//!   in ascending channel-bit order, with marker records between atoms
//!   (plain/encrypted section starts, video frames, attachments, and remote
//!   participants joining or leaving).
//!
//! Privacy: the XML also holds case and docket fields and password data.
//! This module reads only the audio element (mask, format, channel names)
//! and the protection flag. Nothing else from the XML is kept, returned,
//! logged or written to an export record, and no error message quotes it.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use crate::safety::TempFile;

pub(crate) const DCR_MAGIC: &[u8; 8] = b"HGCRLCRS";
const HEADER_BYTES: u64 = 16;
const MAX_MAJOR: u32 = 27;
const META_VERSION: u32 = 2100;
const CHECKSUM_TAIL_BYTES: u64 = 12;
const DIRECTORY_POINTER_BYTES: u64 = 16;
const DIRECTORY_ENTRY_BYTES: u64 = 24;
const DIRECTORY_TERMINATOR_BYTES: u64 = 20;
const CHUNK_FRAMING_BYTES: u64 = 20;
const MAX_DIRECTORY_ENTRIES: u64 = 65_536;
const MAX_META_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FORMAT_BYTES: u64 = 4096;
/// Channel bits DepoAudio follows (bit 0 to 31).
const MAX_CHANNEL_BITS: u32 = 32;
/// Most channels one conversion handles (the conversion limit).
pub(crate) const MAX_DCR_CHANNELS: u32 = 16;
/// Conference changes kept in the inspect result and the export record.
const MAX_RECORDED_CHANGES: usize = 1000;
/// Bytes of a `ptch` record's text searched for its `ac` attribute.
const MAX_PTCH_TEXT: usize = 4096;
const MAX_CHANNEL_NAME_CHARS: usize = 64;
const SCAN_BUFFER_BYTES: usize = 1024 * 1024;
const WRITE_BUFFER_BYTES: usize = 256 * 1024;
/// How many atoms pass between cancellation checks.
const CANCEL_CHECK_ATOMS: u64 = 1 << 16;

pub(crate) const PROTECTED_MESSAGE: &str = "This recording is password protected; DepoAudio can't open protected recordings. Export it to WAV in the Liberty software.";
pub(crate) const SPEEX_MESSAGE: &str =
    "Speex recordings aren't supported yet. Export this recording to WAV in the Liberty software, then add the WAV.";
pub(crate) const LAYOUT_SOURCE: &str =
    "owner-supplied interoperability notes; not yet confirmed against a recorded file";
pub(crate) const CHECKSUM_NOT_CHECKED: &str = "not checked";

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_ALAW: u16 = 0x0006;
const WAVE_FORMAT_MULAW: u16 = 0x0007;
const WAVE_FORMAT_IMA_ADPCM: u16 = 0x0011;
const WAVE_FORMAT_GSM610: u16 = 0x0031;
const WAVE_FORMAT_MPEGLAYER3: u16 = 0x0055;
const WAVE_FORMAT_SPEEX: u16 = 0xA109;

fn damaged(reason: &str) -> String {
    format!("This .dcr recording is damaged or incomplete: {reason}. Export it to WAV in the Liberty software.")
}

fn read_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        damaged("it ends early")
    } else {
        format!("Could not read the .dcr recording: {error}")
    }
}

fn le_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

// ── Audio format ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Pcm,
    ALaw,
    MuLaw,
    ImaAdpcm,
    Gsm610,
    Mp3,
    Speex,
    Other,
}

impl Codec {
    fn from_tag(tag: u16) -> Self {
        match tag {
            WAVE_FORMAT_PCM => Self::Pcm,
            WAVE_FORMAT_ALAW => Self::ALaw,
            WAVE_FORMAT_MULAW => Self::MuLaw,
            WAVE_FORMAT_IMA_ADPCM => Self::ImaAdpcm,
            WAVE_FORMAT_GSM610 => Self::Gsm610,
            WAVE_FORMAT_MPEGLAYER3 => Self::Mp3,
            WAVE_FORMAT_SPEEX => Self::Speex,
            _ => Self::Other,
        }
    }

    /// Codecs whose channel blocks interleave into one multichannel WAV
    /// byte for byte (one block is one sample of one channel).
    fn interleaves(self) -> bool {
        matches!(self, Self::Pcm | Self::ALaw | Self::MuLaw)
    }
}

/// A WAVEFORMATEX that describes one channel of the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WaveFormat {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    /// The extra bytes after cbSize (empty when there are none).
    extra: Vec<u8>,
    /// The structure exactly as stored.
    raw: Vec<u8>,
}

impl WaveFormat {
    fn parse(raw: &[u8]) -> Result<Self, String> {
        if raw.len() < 16 {
            return Err(damaged("its audio format description is too short"));
        }
        let extra = if raw.len() >= 18 {
            let size = usize::from(le_u16(raw, 16));
            if 18 + size > raw.len() {
                return Err(damaged("its audio format description is cut short"));
            }
            raw[18..18 + size].to_vec()
        } else {
            Vec::new()
        };
        Ok(Self {
            tag: le_u16(raw, 0),
            channels: le_u16(raw, 2),
            sample_rate: le_u32(raw, 4),
            avg_bytes_per_sec: le_u32(raw, 8),
            block_align: le_u16(raw, 12),
            bits_per_sample: le_u16(raw, 14),
            extra,
            raw: raw.to_vec(),
        })
    }

    fn codec(&self) -> Codec {
        Codec::from_tag(self.tag)
    }

    fn codec_name(&self) -> String {
        match self.codec() {
            Codec::Pcm => format!("PCM {}-bit", self.bits_per_sample),
            Codec::ALaw => "A-law".into(),
            Codec::MuLaw => "mu-law".into(),
            Codec::ImaAdpcm => "IMA ADPCM".into(),
            Codec::Gsm610 => "GSM 6.10".into(),
            Codec::Mp3 => "MP3".into(),
            Codec::Speex => "Speex".into(),
            Codec::Other => format!("unknown (tag 0x{:04X})", self.tag),
        }
    }

    /// Samples in one block, for a `fact` chunk; None when not computable.
    fn samples_per_block(&self) -> Option<u64> {
        let declared = (self.extra.len() >= 2).then(|| u64::from(le_u16(&self.extra, 0)));
        match self.codec() {
            Codec::ALaw | Codec::MuLaw | Codec::Pcm => Some(1),
            Codec::ImaAdpcm => declared
                .filter(|count| *count > 0)
                .or_else(|| Some((u64::from(self.block_align) - 4) * 2 + 1)),
            Codec::Gsm610 => declared.filter(|count| *count > 0).or(Some(320)),
            _ => None,
        }
    }

    /// Why DepoAudio cannot decode this one-channel format, if it cannot.
    fn problem(&self) -> Option<String> {
        let codec = self.codec();
        match codec {
            Codec::Speex => return Some(SPEEX_MESSAGE.into()),
            Codec::Other => {
                return Some(format!(
                    "This recording uses audio codec tag 0x{:04X}, which DepoAudio can't decode yet. Export it to WAV in the Liberty software, then add the WAV.",
                    self.tag
                ))
            }
            _ => {}
        }
        let unsupported = |what: &str| {
            Some(format!(
                "This recording's {} audio uses {what}, which DepoAudio doesn't support. Export it to WAV in the Liberty software, then add the WAV.",
                self.codec_name()
            ))
        };
        if self.channels != 1 {
            return unsupported(&format!("{} channels per block (one was expected)", self.channels));
        }
        if self.sample_rate == 0 || self.sample_rate > 384_000 {
            return unsupported("a sample rate outside 1 Hz to 384 kHz");
        }
        if self.avg_bytes_per_sec == 0 || self.block_align == 0 {
            return unsupported("a zero data rate or block size");
        }
        let align = u32::from(self.block_align);
        let consistent_rate = self.avg_bytes_per_sec == self.sample_rate.saturating_mul(align);
        match codec {
            Codec::Pcm => {
                if !matches!(self.bits_per_sample, 8 | 16 | 24 | 32) {
                    return unsupported(&format!("{}-bit samples", self.bits_per_sample));
                }
                if align != u32::from(self.bits_per_sample / 8) || !consistent_rate {
                    return unsupported("a block size that does not match its sample size");
                }
            }
            Codec::ALaw | Codec::MuLaw => {
                if self.bits_per_sample != 8 || align != 1 || !consistent_rate {
                    return unsupported("a block size other than one byte per sample");
                }
            }
            Codec::ImaAdpcm => {
                if self.bits_per_sample != 4 || align <= 4 {
                    return unsupported("a block layout other than 4-bit blocks with a header");
                }
            }
            Codec::Gsm610 if align != 65 => {
                return unsupported("a block size other than 65 bytes");
            }
            _ => {}
        }
        None
    }

    /// The byte value of one silent block: used where a remote participant's
    /// channel has no audio yet (or any more).
    fn silent_block(&self) -> Vec<u8> {
        let fill = match self.codec() {
            Codec::Pcm if self.bits_per_sample == 8 => 0x80,
            Codec::ALaw => 0xD5,
            Codec::MuLaw => 0xFF,
            // IMA ADPCM: predictor 0, step index 0 and all-zero codes decode
            // to digital silence (a zero code adds step >> 3 = 0).
            _ => 0x00,
        };
        vec![fill; usize::from(self.block_align)]
    }
}

// ── Inspect result ──────────────────────────────────────────────────────────

/// A remote participant's channel starting or stopping (`ptch` record).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConferenceChange {
    /// Bytes of one channel's audio before the change.
    pub at_per_channel_byte: u64,
    pub at_seconds: f64,
    /// "joined" or "left".
    pub kind: &'static str,
    pub channels_after: u32,
    /// Always true: which channel bit is added or removed is not known, so
    /// DepoAudio guesses (lowest unused bit on join, highest used on leave).
    pub bit_guessed: bool,
}

/// What `inspect` read from a recording. Format facts and channel names only.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DcrInfo {
    pub version: u32,
    pub codec_tag: u16,
    pub codec: String,
    pub sample_rate: u32,
    /// Channels in the converted audio: every channel bit that is active at
    /// any point, in ascending bit order.
    pub channels: u32,
    /// One name per converted channel, in the same order ("Ch N" when the
    /// recording names none).
    pub channel_names: Vec<String>,
    pub atoms: u64,
    pub duration_secs: f64,
    pub video_frames: u64,
    pub attachments: u64,
    pub attachments_deleted: u64,
    /// The first `MAX_RECORDED_CHANGES` changes.
    pub conference_changes: Vec<ConferenceChange>,
    pub conference_change_count: u64,
    pub encrypted_sections: u64,
    pub checksum: &'static str,
    #[serde(skip)]
    layout: Layout,
}

#[derive(Debug, Clone, PartialEq)]
struct Layout {
    format: WaveFormat,
    data_offset: u64,
    data_len: u64,
    initial_mask: u64,
    /// Converted channel bits, ascending.
    output_bits: Vec<u32>,
    protected_flag: bool,
}

impl DcrInfo {
    /// Why this recording cannot be converted, if it cannot. Checked before
    /// any file is written.
    pub(crate) fn refusal(&self) -> Option<String> {
        if self.layout.protected_flag || self.encrypted_sections > 0 {
            return Some(PROTECTED_MESSAGE.into());
        }
        if let Some(problem) = self.layout.format.problem() {
            return Some(problem);
        }
        if self.channels > MAX_DCR_CHANNELS {
            return Some(format!(
                "This recording has {} channels; DepoAudio converts at most {MAX_DCR_CHANNELS}.",
                self.channels
            ));
        }
        if self.conference_change_count > 0 && !self.silence_known() {
            return Some(format!(
                "A remote participant joins or leaves during this {} recording. DepoAudio can keep the channels aligned across that only for PCM, A-law, mu-law and IMA ADPCM recordings so far. Export it to WAV in the Liberty software, then add the WAV.",
                self.codec
            ));
        }
        if self.atoms == 0 {
            return Some("This recording contains no audio.".into());
        }
        None
    }

    /// Whether a silent block is known for the codec (see `silent_block`).
    fn silence_known(&self) -> bool {
        matches!(
            self.layout.format.codec(),
            Codec::Pcm | Codec::ALaw | Codec::MuLaw | Codec::ImaAdpcm
        )
    }

    /// The `dcr` object of an export record.
    pub(crate) fn record(&self) -> DcrSourceRecord {
        DcrSourceRecord {
            version: self.version,
            codec: self.codec.clone(),
            codec_tag: self.codec_tag,
            channels: self.channels,
            channel_names: self.channel_names.clone(),
            atoms: self.atoms,
            video_frames_skipped: self.video_frames,
            attachments_skipped: self.attachments,
            conference_changes: self.conference_changes.clone(),
            conference_change_count: self.conference_change_count,
            checksum: CHECKSUM_NOT_CHECKED,
            layout_source: LAYOUT_SOURCE,
        }
    }

    /// The Convert queue's summary of this recording.
    pub(crate) fn queue_info(&self) -> DcrQueueInfo {
        DcrQueueInfo {
            codec: self.codec.clone(),
            channel_names: self.channel_names.clone(),
            conference_changes: self.conference_changes.clone(),
            conference_change_count: self.conference_change_count,
            video_frames: self.video_frames,
            attachments: self.attachments,
            problem: self.refusal(),
        }
    }
}

/// The optional `source.dcr` object of an export record: format facts only,
/// never case, docket or password fields.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DcrSourceRecord {
    pub version: u32,
    pub codec: String,
    pub codec_tag: u16,
    pub channels: u32,
    pub channel_names: Vec<String>,
    pub atoms: u64,
    pub video_frames_skipped: u64,
    pub attachments_skipped: u64,
    pub conference_changes: Vec<ConferenceChange>,
    pub conference_change_count: u64,
    pub checksum: &'static str,
    pub layout_source: &'static str,
}

/// What the Convert queue shows for a queued `.dcr`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DcrQueueInfo {
    pub codec: String,
    pub channel_names: Vec<String>,
    pub conference_changes: Vec<ConferenceChange>,
    pub conference_change_count: u64,
    pub video_frames: u64,
    pub attachments: u64,
    /// Why it cannot be converted, when it cannot.
    pub problem: Option<String>,
}

// ── Container: header and chunk directory ──────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    tag: [u8; 4],
    id: u32,
    offset: u64,
    size: u64,
}

struct Container {
    version: u32,
    entries: Vec<Entry>,
}

fn read_at(file: &mut File, offset: u64, length: u64) -> Result<Vec<u8>, String> {
    let length = usize::try_from(length).map_err(|_| damaged("a chunk is too large"))?;
    file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
    let mut bytes = vec![0u8; length];
    file.read_exact(&mut bytes).map_err(read_error)?;
    Ok(bytes)
}

fn read_container(file: &mut File, file_len: u64) -> Result<Container, String> {
    if file_len < HEADER_BYTES {
        return Err(damaged("it is shorter than a DCR header"));
    }
    let header = read_at(file, 0, HEADER_BYTES)?;
    if &header[..8] != DCR_MAGIC {
        return Err("This file does not start like a Liberty / BIS Digital DCR recording.".into());
    }
    let major = le_u32(&header, 8);
    let minor = le_u32(&header, 12);
    if major > MAX_MAJOR {
        return Err(format!(
            "This DCR was written by recorder format version {major}, which is newer than DepoAudio knows. Export it to WAV in the Liberty software."
        ));
    }
    if minor >= 100 {
        return Err(damaged("its version number is not valid"));
    }
    let version = major * 100 + minor;
    let tail = if version >= META_VERSION {
        CHECKSUM_TAIL_BYTES
    } else {
        0
    };
    if file_len < HEADER_BYTES + tail + DIRECTORY_TERMINATOR_BYTES {
        return Err(damaged("it is too short to hold a chunk directory"));
    }
    let pointer = read_at(file, file_len - tail - DIRECTORY_POINTER_BYTES, DIRECTORY_POINTER_BYTES)?;
    let head_offset = le_u64(&pointer, 0);
    let head_size = le_u64(&pointer, 8);
    let fits = file_len
        .checked_sub(head_offset)
        .is_some_and(|after| Some(after) == head_size.checked_add(tail));
    if !fits || head_offset < HEADER_BYTES || head_size < DIRECTORY_TERMINATOR_BYTES {
        return Err(damaged("its chunk directory does not match the file size"));
    }
    let listed = head_size - DIRECTORY_TERMINATOR_BYTES;
    if !listed.is_multiple_of(DIRECTORY_ENTRY_BYTES) || listed / DIRECTORY_ENTRY_BYTES > MAX_DIRECTORY_ENTRIES {
        return Err(damaged("its chunk directory has an impossible size"));
    }
    let directory = read_at(file, head_offset, head_size)?;
    let terminator = &directory[listed as usize..];
    if &terminator[..4] != b"head" || le_u64(terminator, 4) != head_offset || le_u64(terminator, 12) != head_size {
        return Err(damaged("its chunk directory is not closed correctly"));
    }
    let mut entries: Vec<Entry> = directory[..listed as usize]
        .chunks_exact(DIRECTORY_ENTRY_BYTES as usize)
        .map(|raw| Entry {
            tag: raw[..4].try_into().expect("four bytes"),
            id: le_u32(raw, 4),
            offset: le_u64(raw, 8),
            size: le_u64(raw, 16),
        })
        .collect();
    for entry in &entries {
        let inside = entry.offset >= HEADER_BYTES
            && entry.size >= CHUNK_FRAMING_BYTES
            && entry
                .offset
                .checked_add(entry.size)
                .is_some_and(|end| end <= head_offset);
        if !inside {
            return Err(damaged("a chunk listed in its directory lies outside the file"));
        }
    }
    entries.sort_by_key(|entry| entry.offset);
    if entries
        .windows(2)
        .any(|pair| pair[0].offset + pair[0].size > pair[1].offset)
    {
        return Err(damaged("two chunks listed in its directory overlap"));
    }
    let mut keys: Vec<([u8; 4], u32)> = entries.iter().map(|entry| (entry.tag, entry.id)).collect();
    keys.sort_unstable();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(damaged("its directory lists the same chunk twice"));
    }
    Ok(Container { version, entries })
}

impl Container {
    /// The entry `(tag, id)`; for `id` 0, a tag listed once under another id
    /// also matches.
    fn find(&self, tag: &[u8; 4], id: u32) -> Option<Entry> {
        if let Some(entry) = self.entries.iter().find(|entry| &entry.tag == tag && entry.id == id) {
            return Some(*entry);
        }
        let mut same_tag = self.entries.iter().filter(|entry| &entry.tag == tag);
        match (id, same_tag.next(), same_tag.next()) {
            (0, Some(only), None) => Some(*only),
            _ => None,
        }
    }
}

/// Check a chunk's framing and return its payload range `(offset, length)`.
fn payload_range(file: &mut File, entry: Entry) -> Result<(u64, u64), String> {
    let begin = read_at(file, entry.offset, 12)?;
    let end = read_at(file, entry.offset + entry.size - 8, 8)?;
    let mut begin_marker = entry.tag.to_vec();
    begin_marker.extend_from_slice(b"#bgn");
    let mut end_marker = entry.tag.to_vec();
    end_marker.extend_from_slice(b"#end");
    if begin[..8] != begin_marker[..] || le_u32(&begin, 8) != entry.id || end[..] != end_marker[..] {
        return Err(damaged("a chunk's start or end marker is missing"));
    }
    Ok((entry.offset + 12, entry.size - CHUNK_FRAMING_BYTES))
}

fn read_payload(file: &mut File, entry: Entry, limit: u64, what: &str) -> Result<Vec<u8>, String> {
    let (offset, length) = payload_range(file, entry)?;
    if length > limit {
        return Err(damaged(&format!("its {what} is larger than expected")));
    }
    read_at(file, offset, length)
}

// ── META XML (version 21.00 and later) ─────────────────────────────────────

/// Windows-1252 bytes as text, without control characters other than tab,
/// CR and LF.
fn decode_windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{02C6}',
        '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}',
        '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}',
        '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&byte| match byte {
            0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
            _ => char::from(byte),
        })
        .filter(|character| !character.is_control() || matches!(character, '\t' | '\r' | '\n'))
        .collect()
}

/// One element found in the XML: its attribute text and its inner text.
struct Element<'a> {
    attributes: &'a str,
    inner: &'a str,
}

/// End of an opening tag that starts at `start` (the byte after `<NAME`):
/// the index of its `>`, skipping quoted attribute values.
fn open_tag_end(text: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (index, &byte) in text.as_bytes()[start..].iter().enumerate() {
        match (quote, byte) {
            (Some(open), _) if byte == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'>') => return Some(start + index),
            _ => {}
        }
    }
    None
}

/// Index of the `</NAME>` that closes an element whose content starts at
/// `from`, skipping CDATA sections.
fn close_tag_start(text: &str, from: usize, name: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        let rest = &text[index..];
        if rest.starts_with("<![CDATA[") {
            index += rest.find("]]>")? + 3;
            continue;
        }
        if rest.starts_with("</") && tag_name_matches(&rest[2..], name) {
            return Some(index);
        }
        index += rest.chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn tag_name_matches(rest: &str, name: &str) -> bool {
    rest.len() >= name.len()
        && rest.is_char_boundary(name.len())
        && rest[..name.len()].eq_ignore_ascii_case(name)
        && rest[name.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_whitespace() || next == '>' || next == '/')
}

/// Every `NAME` element in `text`, in order (not nested in each other).
fn elements<'a>(text: &'a str, name: &str) -> Vec<Element<'a>> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(offset) = text[index..].find('<') {
        let start = index + offset;
        let rest = &text[start + 1..];
        if rest.starts_with("![CDATA[") {
            match rest.find("]]>") {
                Some(end) => {
                    index = start + 1 + end + 3;
                    continue;
                }
                None => break,
            }
        }
        if !tag_name_matches(rest, name) {
            index = start + 1;
            continue;
        }
        let attributes_start = start + 1 + name.len();
        let Some(tag_end) = open_tag_end(text, attributes_start) else {
            break;
        };
        let attributes = &text[attributes_start..tag_end];
        if attributes.trim_end().ends_with('/') {
            found.push(Element {
                attributes: attributes.trim_end().trim_end_matches('/'),
                inner: "",
            });
            index = tag_end + 1;
            continue;
        }
        let Some(close) = close_tag_start(text, tag_end + 1, name) else {
            break;
        };
        found.push(Element {
            attributes,
            inner: &text[tag_end + 1..close],
        });
        index = close + 2;
    }
    found
}

fn element<'a>(text: &'a str, name: &str) -> Option<Element<'a>> {
    elements(text, name).into_iter().next()
}

fn attribute(attributes: &str, name: &str) -> Option<String> {
    let bytes = attributes.as_bytes();
    let mut index = 0;
    while index < attributes.len() {
        let rest = &attributes[index..];
        let starts_word = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
        if starts_word && tag_like_name(rest, name) {
            let after = rest[name.len()..].trim_start();
            if let Some(after_equals) = after.strip_prefix('=') {
                let value = after_equals.trim_start();
                let quote = value.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let body = &value[1..];
                    let end = body.find(quote)?;
                    return Some(decode_entities(&body[..end]));
                }
            }
        }
        index += rest.chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn tag_like_name(rest: &str, name: &str) -> bool {
    rest.len() >= name.len()
        && rest.is_char_boundary(name.len())
        && rest[..name.len()].eq_ignore_ascii_case(name)
        && !rest[name.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphanumeric() || next == '_')
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|decimal| decimal.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// An element's text: the CDATA sections joined, or the entity-decoded text.
fn text_content(inner: &str) -> String {
    if inner.contains("<![CDATA[") {
        let mut out = String::new();
        let mut rest = inner;
        while let Some(start) = rest.find("<![CDATA[") {
            let body = &rest[start + 9..];
            let end = body.find("]]>").unwrap_or(body.len());
            out.push_str(&body[..end]);
            rest = body.get(end + 3..).unwrap_or("");
        }
        out
    } else {
        decode_entities(inner)
    }
}

fn parse_mask(text: &str) -> Option<u64> {
    let text = text.trim();
    let value = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok()?,
        None => text.parse::<u64>().ok()?,
    };
    (value != 0 && value < (1u64 << MAX_CHANNEL_BITS)).then_some(value)
}

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) || digits.len() as u64 > MAX_FORMAT_BYTES * 2 {
        return None;
    }
    digits
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

fn bounded_channel_name(raw: &str) -> Option<String> {
    let name: String = raw
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_CHANNEL_NAME_CHARS)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// What the META document says about the audio. Nothing else is read.
#[derive(Debug, Clone, PartialEq)]
struct AudioDescription {
    format: Vec<u8>,
    mask: u64,
    names: Vec<Option<String>>,
    protected_flag: bool,
}

fn parse_meta(bytes: &[u8]) -> Result<AudioDescription, String> {
    let text = decode_windows_1252(bytes);
    let meta = element(&text, "META").ok_or_else(|| damaged("its META description is missing"))?;
    let audio = element(meta.inner, "AUDIO").ok_or_else(|| damaged("its audio description is missing"))?;
    let mask = attribute(audio.attributes, "USECHNL")
        .as_deref()
        .and_then(parse_mask)
        .ok_or_else(|| damaged("its channel mask is missing or not valid"))?;
    let format = element(audio.inner, "FMTDATA")
        .map(|element| text_content(element.inner))
        .as_deref()
        .and_then(parse_hex)
        .ok_or_else(|| damaged("its audio format data is missing or not valid"))?;
    let names = element(audio.inner, "LISTCHNL")
        .map(|list| {
            elements(list.inner, "ITEM")
                .iter()
                .map(|item| {
                    element(item.inner, "CHNAME").and_then(|name| bounded_channel_name(&text_content(name.inner)))
                })
                .collect()
        })
        .unwrap_or_default();
    let protected_flag = elements(meta.inner, "CRYPT").iter().any(|crypt| {
        attribute(crypt.attributes, "HAVEREC").is_some_and(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0"
        })
    });
    Ok(AudioDescription {
        format,
        mask,
        names,
        protected_flag,
    })
}

/// Version 20.xx and earlier: `wfmt`, `chnl` and an optional `crpt` chunk.
fn read_legacy_description(file: &mut File, container: &Container) -> Result<AudioDescription, String> {
    let wfmt = container
        .find(b"wfmt", 0)
        .ok_or_else(|| damaged("its audio format chunk is missing"))?;
    let format = read_payload(file, wfmt, MAX_FORMAT_BYTES, "audio format chunk")?;
    let chnl = container
        .find(b"chnl", 0)
        .ok_or_else(|| damaged("its channel chunk is missing"))?;
    let channel = read_payload(file, chnl, MAX_FORMAT_BYTES, "channel chunk")?;
    if channel.len() < 4 {
        return Err(damaged("its channel chunk is too short"));
    }
    let mask = Some(u64::from(le_u32(&channel, 0)))
        .filter(|mask| *mask != 0)
        .ok_or_else(|| damaged("its channel mask is empty"))?;
    // Password data. Any non-zero byte counts as protected: DepoAudio does
    // not open a recording that may need a password.
    let protected_flag = match container.find(b"crpt", 0) {
        Some(crpt) => read_payload(file, crpt, MAX_META_BYTES, "protection chunk")?
            .iter()
            .any(|byte| *byte != 0),
        None => false,
    };
    Ok(AudioDescription {
        format,
        mask,
        names: Vec::new(),
        protected_flag,
    })
}

// ── Data chunk scan ─────────────────────────────────────────────────────────

/// A forward-only buffered view of one byte range of the file.
struct Region<'a> {
    file: &'a mut File,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    unread: u64,
}

impl<'a> Region<'a> {
    fn new(file: &'a mut File, offset: u64, length: u64) -> Result<Self, String> {
        file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
        Ok(Self {
            file,
            buffer: vec![0u8; SCAN_BUFFER_BYTES],
            start: 0,
            end: 0,
            unread: length,
        })
    }

    fn left(&self) -> u64 {
        (self.end - self.start) as u64 + self.unread
    }

    fn bytes(&self) -> &[u8] {
        &self.buffer[self.start..self.end]
    }

    /// Buffer at least `want` bytes, or everything left if that is less.
    fn fill(&mut self, want: usize) -> Result<(), String> {
        if self.end - self.start >= want || self.unread == 0 {
            return Ok(());
        }
        self.buffer.copy_within(self.start..self.end, 0);
        self.end -= self.start;
        self.start = 0;
        if self.buffer.len() < want {
            self.buffer.resize(want, 0);
        }
        while self.end < want && self.unread > 0 {
            let room = (self.buffer.len() - self.end).min(usize::try_from(self.unread).unwrap_or(usize::MAX));
            let read = self
                .file
                .read(&mut self.buffer[self.end..self.end + room])
                .map_err(read_error)?;
            if read == 0 {
                return Err(damaged("its audio data ends early"));
            }
            self.end += read;
            self.unread -= read as u64;
        }
        Ok(())
    }

    fn consume(&mut self, count: usize) {
        self.start += count;
    }

    fn skip(&mut self, count: u64) -> Result<(), String> {
        let buffered = (self.end - self.start) as u64;
        if count <= buffered {
            self.start += count as usize;
            return Ok(());
        }
        let rest = count - buffered;
        self.start = 0;
        self.end = 0;
        let jump = i64::try_from(rest).map_err(|_| damaged("a record is too large"))?;
        self.file.seek(SeekFrom::Current(jump)).map_err(read_error)?;
        self.unread -= rest;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ScanStats {
    atoms: u64,
    video_frames: u64,
    attachments: u64,
    attachments_deleted: u64,
    encrypted_sections: u64,
    changes: Vec<ConferenceChange>,
    change_count: u64,
    union_mask: u64,
}

/// Whether a `ptch` record's text changes the channel count: `ac="0"` does
/// not; any other value, or no `ac` attribute, does.
fn ptch_changes_channels(text: &[u8]) -> bool {
    let text = String::from_utf8_lossy(text);
    attribute(&text, "ac").is_none_or(|value| value.trim() != "0")
}

/// Walk the data chunk atom by atom. `sink` receives every plain audio atom
/// with the channel mask active for it. Encrypted audio is counted, never
/// passed on.
fn scan_data(
    file: &mut File,
    layout: &Layout,
    cancelled: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(u64, &[u8]) -> Result<(), String>,
) -> Result<ScanStats, String> {
    let format = &layout.format;
    let block = usize::from(format.block_align);
    if block == 0 {
        return Err(damaged("its audio block size is zero"));
    }
    let mut region = Region::new(file, layout.data_offset, layout.data_len)?;
    let mut stats = ScanStats {
        union_mask: layout.initial_mask,
        ..ScanStats::default()
    };
    let mut active = layout.initial_mask;
    let mut encrypted = false;
    let mut steps: u64 = 0;
    let misaligned = || damaged("its audio data is out of step with the channel layout (atom misalignment)");
    while region.left() > 0 {
        steps += 1;
        if steps.is_multiple_of(CANCEL_CHECK_ATOMS) && cancelled() {
            return Err(crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
        }
        let width = active.count_ones() as usize * block;
        region.fill(width.max(24))?;
        let left = region.left();
        let bytes = region.bytes();
        if bytes.len() >= 8 {
            let marker: [u8; 8] = bytes[..8].try_into().expect("eight bytes");
            match &marker {
                b"audi#mrk" => {
                    region.consume(8);
                    encrypted = false;
                    continue;
                }
                b"audi#crp" => {
                    region.consume(8);
                    if !encrypted {
                        stats.encrypted_sections += 1;
                    }
                    encrypted = true;
                    continue;
                }
                b"cadr#bgn" => {
                    if bytes.len() < 12 {
                        return Err(damaged("a video frame record is cut short"));
                    }
                    let total = u64::from(le_u32(bytes, 8));
                    if total < 12 || total > left {
                        return Err(damaged("a video frame record runs past the audio data"));
                    }
                    stats.video_frames += 1;
                    region.skip(total)?;
                    continue;
                }
                b"attx#bgn" => {
                    if bytes.len() < 24 {
                        return Err(damaged("an attachment record is cut short"));
                    }
                    let total = le_u64(bytes, 8);
                    let flags = le_u32(bytes, 20);
                    if total < 24 || total > left {
                        return Err(damaged("an attachment record runs past the audio data"));
                    }
                    stats.attachments += 1;
                    if flags & 1 != 0 {
                        stats.attachments_deleted += 1;
                    }
                    region.skip(total)?;
                    continue;
                }
                b"ptch#str" | b"ptch#stp" => {
                    if bytes.len() < 12 {
                        return Err(damaged("a remote-participant record is cut short"));
                    }
                    let text_len = u64::from(le_u32(bytes, 8));
                    let total = 12 + text_len;
                    if total > left {
                        return Err(damaged("a remote-participant record runs past the audio data"));
                    }
                    let joined = &marker == b"ptch#str";
                    let searched = 12 + usize::try_from(text_len).unwrap_or(usize::MAX).min(MAX_PTCH_TEXT);
                    region.fill(searched)?;
                    let changes = ptch_changes_channels(&region.bytes()[12..searched]);
                    region.skip(total)?;
                    if !changes {
                        continue;
                    }
                    if joined {
                        let bit = (0..MAX_CHANNEL_BITS)
                            .find(|bit| active & (1 << bit) == 0)
                            .ok_or_else(|| damaged("more than 32 channels are active"))?;
                        active |= 1 << bit;
                    } else {
                        if active == 0 {
                            return Err(damaged("a participant leaves when no channel is active"));
                        }
                        active &= !(1u64 << (63 - active.leading_zeros()));
                    }
                    stats.union_mask |= active;
                    stats.change_count += 1;
                    if stats.changes.len() < MAX_RECORDED_CHANGES {
                        let at_per_channel_byte = stats.atoms * block as u64;
                        stats.changes.push(ConferenceChange {
                            at_per_channel_byte,
                            at_seconds: at_per_channel_byte as f64 / f64::from(format.avg_bytes_per_sec.max(1)),
                            kind: if joined { "joined" } else { "left" },
                            channels_after: active.count_ones(),
                            bit_guessed: true,
                        });
                    }
                    continue;
                }
                _ => {}
            }
        }
        if width == 0 {
            if encrypted {
                break;
            }
            return Err(misaligned());
        }
        if bytes.len() < width {
            if encrypted {
                // Encrypted bytes need not follow the plain layout; the
                // recording is refused as protected either way.
                break;
            }
            return Err(misaligned());
        }
        if !encrypted {
            sink(active, &bytes[..width])?;
        }
        region.consume(width);
        stats.atoms += 1;
    }
    Ok(stats)
}

// ── Inspect ─────────────────────────────────────────────────────────────────

/// Read a recording's layout and walk its audio once, without writing
/// anything. Errors are damaged-file errors; reasons not to convert a
/// readable file are in `DcrInfo::refusal`.
pub(crate) fn inspect(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<DcrInfo, String> {
    let mut file = File::open(path).map_err(|error| format!("Could not open the .dcr recording: {error}"))?;
    let file_len = file.metadata().map_err(read_error)?.len();
    let container = read_container(&mut file, file_len)?;
    let description = if container.version >= META_VERSION {
        let meta = container
            .find(b"meta", 0)
            .ok_or_else(|| damaged("its META description chunk is missing"))?;
        parse_meta(&read_payload(&mut file, meta, MAX_META_BYTES, "META description")?)?
    } else {
        read_legacy_description(&mut file, &container)?
    };
    if description.mask >= (1u64 << MAX_CHANNEL_BITS) {
        return Err(damaged("its channel mask uses more than 32 channels"));
    }
    let format = WaveFormat::parse(&description.format)?;
    let data = container
        .find(b"data", 0)
        .ok_or_else(|| damaged("its audio data chunk is missing"))?;
    let (data_offset, data_len) = payload_range(&mut file, data)?;
    let mut layout = Layout {
        format,
        data_offset,
        data_len,
        initial_mask: description.mask,
        output_bits: Vec::new(),
        protected_flag: description.protected_flag,
    };
    let stats = scan_data(&mut file, &layout, cancelled, &mut |_, _| Ok(()))?;
    layout.output_bits = (0..MAX_CHANNEL_BITS)
        .filter(|bit| stats.union_mask & (1 << bit) != 0)
        .collect();
    let channel_names = channel_names(&layout.output_bits, description.mask, &description.names);
    let format = &layout.format;
    Ok(DcrInfo {
        version: container.version,
        codec_tag: format.tag,
        codec: format.codec_name(),
        sample_rate: format.sample_rate,
        channels: layout.output_bits.len() as u32,
        channel_names,
        atoms: stats.atoms,
        duration_secs: (stats.atoms * u64::from(format.block_align)) as f64
            / f64::from(format.avg_bytes_per_sec.max(1)),
        video_frames: stats.video_frames,
        attachments: stats.attachments,
        attachments_deleted: stats.attachments_deleted,
        conference_changes: stats.changes,
        conference_change_count: stats.change_count,
        encrypted_sections: stats.encrypted_sections,
        checksum: CHECKSUM_NOT_CHECKED,
        layout,
    })
}

/// Names for the converted channels. When the list has an item for every
/// bit up to the highest one used (and is not simply one per masked
/// channel), item N names bit N; otherwise the items name the channels of
/// the initial mask in ascending bit order. Unnamed channels are "Ch N",
/// N counting the converted channels from 1. Which reading the Liberty
/// software uses is not confirmed.
fn channel_names(output_bits: &[u32], mask: u64, names: &[Option<String>]) -> Vec<String> {
    let highest = output_bits.last().copied().unwrap_or(0) as usize;
    let mask_bits: Vec<u32> = (0..MAX_CHANNEL_BITS).filter(|bit| mask & (1 << bit) != 0).collect();
    let by_bit = names.len() > highest && names.len() != mask_bits.len();
    output_bits
        .iter()
        .enumerate()
        .map(|(position, bit)| {
            let name = if by_bit {
                names.get(*bit as usize).cloned().flatten()
            } else {
                mask_bits
                    .iter()
                    .position(|masked| masked == bit)
                    .and_then(|index| names.get(index).cloned().flatten())
            };
            name.unwrap_or_else(|| format!("Ch {}", position + 1))
        })
        .collect()
}

/// The Convert queue's probe answer for a `.dcr`: one stream with the
/// converted channel count, rate and duration, plus the reader's summary.
/// A file that cannot be read or converted has no streams (so no size
/// estimate) and says why in `dcr.problem`.
pub(crate) fn queue_streams_info(path: &Path) -> crate::manifest::AudioStreamsInfo {
    let (streams, duration_secs, dcr) = match inspect(path, &|| false) {
        Ok(info) => {
            let queue = info.queue_info();
            let streams = if queue.problem.is_some() {
                Vec::new()
            } else {
                vec![crate::manifest::AudioStreamSummary {
                    channels: Some(info.channels),
                    sample_rate: Some(info.sample_rate),
                    language: None,
                    title: None,
                }]
            };
            let duration = queue.problem.is_none().then_some(info.duration_secs);
            (streams, duration, queue)
        }
        Err(problem) => (
            Vec::new(),
            None,
            DcrQueueInfo {
                codec: String::new(),
                channel_names: Vec::new(),
                conference_changes: Vec::new(),
                conference_change_count: 0,
                video_frames: 0,
                attachments: 0,
                problem: Some(problem),
            },
        ),
    };
    crate::manifest::AudioStreamsInfo {
        converted_index: crate::helpers::SOURCE_AUDIO_STREAM,
        streams,
        duration_secs,
        dcr: Some(dcr),
    }
}

// ── Demux into temporary WAV feeds ──────────────────────────────────────────

fn temp_wav_path() -> PathBuf {
    std::env::temp_dir().join(format!("depoaudio_{}.wav", Uuid::new_v4().simple()))
}

/// A WAV header for `data_len` bytes of `fmt` audio. RF64 (with a `ds64`
/// chunk) when the sizes do not fit 32 bits.
fn wav_header(fmt: &[u8], fact_samples: Option<u64>, data_len: u64) -> Vec<u8> {
    let mut fmt = fmt.to_vec();
    if fmt.len() % 2 == 1 {
        fmt.push(0);
    }
    let fact_len = if fact_samples.is_some() { 12 } else { 0 };
    let body = 4 + 8 + fmt.len() as u64 + fact_len + 8 + data_len + (data_len % 2);
    let large = body > u64::from(u32::MAX) - 64 || data_len > u64::from(u32::MAX) - 64;
    let mut header = Vec::with_capacity(96 + fmt.len());
    if large {
        let body = body + 36;
        header.extend_from_slice(b"RF64");
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        header.extend_from_slice(b"WAVE");
        header.extend_from_slice(b"ds64");
        header.extend_from_slice(&28u32.to_le_bytes());
        header.extend_from_slice(&body.to_le_bytes());
        header.extend_from_slice(&data_len.to_le_bytes());
        header.extend_from_slice(&fact_samples.unwrap_or(0).to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
    } else {
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(body as u32).to_le_bytes());
        header.extend_from_slice(b"WAVE");
    }
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    header.extend_from_slice(&fmt);
    if let Some(samples) = fact_samples {
        header.extend_from_slice(b"fact");
        header.extend_from_slice(&4u32.to_le_bytes());
        header.extend_from_slice(&u32::try_from(samples).unwrap_or(u32::MAX).to_le_bytes());
    }
    header.extend_from_slice(b"data");
    let data_field = if large { u32::MAX } else { data_len as u32 };
    header.extend_from_slice(&data_field.to_le_bytes());
    header
}

fn create_temp_wav() -> Result<(TempFile, BufWriter<File>), String> {
    let path = temp_wav_path();
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Could not create a temporary file for the recording: {error}"))?;
    Ok((TempFile::new(path), BufWriter::with_capacity(WRITE_BUFFER_BYTES, file)))
}

fn finish_wav(writer: BufWriter<File>, data_len: u64) -> Result<(), String> {
    let mut file = writer
        .into_inner()
        .map_err(|error| format!("Could not write the temporary audio: {}", error.error()))?;
    if data_len % 2 == 1 {
        file.write_all(&[0])
            .map_err(|error| format!("Could not write the temporary audio: {error}"))?;
    }
    file.sync_all()
        .map_err(|error| format!("Could not write the temporary audio: {error}"))
}

fn write_error(error: io::Error) -> String {
    format!("Could not write the temporary audio: {error}")
}

/// One multichannel WAV whose frames are the atoms' blocks, in output-bit
/// order, with silence for a channel that is not active at that point.
fn write_interleaved(src: &Path, info: &DcrInfo, cancelled: &dyn Fn() -> bool) -> Result<TempFile, String> {
    let layout = &info.layout;
    let format = &layout.format;
    let channels = layout.output_bits.len() as u64;
    let block = usize::from(format.block_align);
    let data_len = info.atoms * channels * block as u64;
    let mut fmt = Vec::with_capacity(18);
    fmt.extend_from_slice(&format.tag.to_le_bytes());
    fmt.extend_from_slice(&(channels as u16).to_le_bytes());
    fmt.extend_from_slice(&format.sample_rate.to_le_bytes());
    fmt.extend_from_slice(&(format.avg_bytes_per_sec * channels as u32).to_le_bytes());
    fmt.extend_from_slice(&(format.block_align * channels as u16).to_le_bytes());
    fmt.extend_from_slice(&format.bits_per_sample.to_le_bytes());
    let fact = if format.codec() == Codec::Pcm {
        None
    } else {
        fmt.extend_from_slice(&0u16.to_le_bytes());
        Some(info.atoms)
    };
    let (guard, mut writer) = create_temp_wav()?;
    writer
        .write_all(&wav_header(&fmt, fact, data_len))
        .map_err(write_error)?;
    let silence = format.silent_block();
    let bits = layout.output_bits.clone();
    let mut written = 0u64;
    let mut file = File::open(src).map_err(|error| format!("Could not open the .dcr recording: {error}"))?;
    scan_data(&mut file, layout, cancelled, &mut |active, atom| {
        let mut next = 0;
        for bit in &bits {
            if active & (1 << bit) != 0 {
                writer
                    .write_all(&atom[next * block..(next + 1) * block])
                    .map_err(write_error)?;
                next += 1;
            } else {
                writer.write_all(&silence).map_err(write_error)?;
            }
        }
        written += (bits.len() * block) as u64;
        Ok(())
    })?;
    if written != data_len {
        return Err(damaged("its audio changed while it was being read"));
    }
    finish_wav(writer, data_len)?;
    Ok(guard)
}

/// One mono WAV per output channel: `fmt ` is the recording's one-channel
/// WAVEFORMATEX as stored, `data` that channel's blocks in order.
fn write_mono_channels(src: &Path, info: &DcrInfo, cancelled: &dyn Fn() -> bool) -> Result<Vec<TempFile>, String> {
    let layout = &info.layout;
    let format = &layout.format;
    let block = usize::from(format.block_align);
    let data_len = info.atoms * block as u64;
    let fact = format
        .samples_per_block()
        .and_then(|per_block| info.atoms.checked_mul(per_block));
    let header = wav_header(&format.raw, fact, data_len);
    let mut guards = Vec::new();
    let mut writers = Vec::new();
    for _ in &layout.output_bits {
        let (guard, mut writer) = create_temp_wav()?;
        guards.push(guard);
        writer.write_all(&header).map_err(write_error)?;
        writers.push(writer);
    }
    let silence = format.silent_block();
    let bits = layout.output_bits.clone();
    let mut atoms = 0u64;
    let mut file = File::open(src).map_err(|error| format!("Could not open the .dcr recording: {error}"))?;
    scan_data(&mut file, layout, cancelled, &mut |active, atom| {
        let mut next = 0;
        for (bit, writer) in bits.iter().zip(writers.iter_mut()) {
            if active & (1 << bit) != 0 {
                writer
                    .write_all(&atom[next * block..(next + 1) * block])
                    .map_err(write_error)?;
                next += 1;
            } else {
                writer.write_all(&silence).map_err(write_error)?;
            }
        }
        atoms += 1;
        Ok(())
    })?;
    if atoms != info.atoms {
        return Err(damaged("its audio changed while it was being read"));
    }
    for writer in writers {
        finish_wav(writer, data_len)?;
    }
    Ok(guards)
}

/// FFmpeg arguments that join the mono channel WAVs, in output order, into
/// one multichannel 16-bit PCM WAV (`amerge`). Used for codecs whose FFmpeg
/// decoders take one channel per stream (IMA ADPCM, GSM 6.10, MP3).
pub(crate) fn merge_args(inputs: &[PathBuf], output: &Path, estimated_bytes: Option<u64>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    for input in inputs {
        args.extend([
            "-protocol_whitelist".into(),
            "file".into(),
            "-i".into(),
            input.to_string_lossy().to_string(),
        ]);
    }
    let labels: String = (0..inputs.len()).map(|index| format!("[{index}:a:0]")).collect();
    args.extend([
        "-filter_complex".into(),
        format!("{labels}amerge=inputs={}[dcr]", inputs.len()),
        "-map".into(),
        "[dcr]".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
    ]);
    args.extend(crate::helpers::wav_rf64_args(estimated_bytes));
    args.extend([
        "-f".into(),
        "wav".into(),
        "-y".into(),
        output.to_string_lossy().to_string(),
    ]);
    args
}

/// The FFmpeg run still needed to finish a feed: join the mono channel
/// files into `output`. The inputs are removed when this is dropped.
pub(crate) struct MergeStep {
    pub inputs: Vec<TempFile>,
    pub args: Vec<String>,
}

/// A demuxed recording ready for FFmpeg, or one FFmpeg run away from it.
pub(crate) struct DcrFeed {
    /// The feed FFmpeg reads (once `merge` has run, when there is one).
    pub path: PathBuf,
    /// Removes the feed when dropped.
    pub guard: TempFile,
    pub merge: Option<MergeStep>,
    pub info: DcrInfo,
}

/// Inspect, refuse what cannot be converted (nothing is written then), and
/// demux the audio into temporary WAVs. Blocking file I/O.
pub(crate) fn prepare_feed(src: &Path, cancelled: &dyn Fn() -> bool) -> Result<DcrFeed, String> {
    let info = inspect(src, cancelled)?;
    if let Some(refusal) = info.refusal() {
        return Err(refusal);
    }
    if cancelled() {
        return Err(crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    if info.layout.format.codec().interleaves() {
        let guard = write_interleaved(src, &info, cancelled)?;
        return Ok(DcrFeed {
            path: guard.path.clone(),
            guard,
            merge: None,
            info,
        });
    }
    let mut inputs = write_mono_channels(src, &info, cancelled)?;
    if inputs.len() == 1 {
        let guard = inputs.remove(0);
        return Ok(DcrFeed {
            path: guard.path.clone(),
            guard,
            merge: None,
            info,
        });
    }
    let output = temp_wav_path();
    let estimate =
        crate::helpers::wav_estimated_bytes(Some(info.duration_secs), Some(info.channels), Some(info.sample_rate));
    let paths: Vec<PathBuf> = inputs.iter().map(|input| input.path.clone()).collect();
    let args = merge_args(&paths, &output, estimate);
    Ok(DcrFeed {
        path: output.clone(),
        guard: TempFile::new(output.clone()),
        merge: Some(MergeStep { inputs, args }),
        info,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Every file here is SYNTHETIC: built by `DcrBuilder` from generated
    // tones, following the layout described at the top of this module. No
    // byte of a real Liberty or BIS recording is used.

    pub(crate) struct TestDir(pub PathBuf);

    impl TestDir {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("depoaudio_dcr_test_{}", Uuid::new_v4().simple()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A one-channel WAVEFORMATEX.
    pub(crate) fn waveformatex(tag: u16, rate: u32, avg: u32, align: u16, bits: u16, extra: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(&tag.to_le_bytes());
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.extend_from_slice(&rate.to_le_bytes());
        raw.extend_from_slice(&avg.to_le_bytes());
        raw.extend_from_slice(&align.to_le_bytes());
        raw.extend_from_slice(&bits.to_le_bytes());
        raw.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        raw.extend_from_slice(extra);
        raw
    }

    pub(crate) fn pcm16_format(rate: u32) -> Vec<u8> {
        waveformatex(WAVE_FORMAT_PCM, rate, rate * 2, 2, 16, &[])
    }

    /// Builds synthetic DCR files.
    pub(crate) struct DcrBuilder {
        pub major: u32,
        pub minor: u32,
        pub format: Vec<u8>,
        pub mask: u64,
        pub names: Vec<String>,
        pub have_rec: &'static str,
        pub data: Vec<u8>,
        /// Extra chunks (tag, id, payload) written before `data`.
        pub extra_chunks: Vec<([u8; 4], u32, Vec<u8>)>,
        /// Case fields that must never leave the reader.
        pub docket: &'static str,
    }

    pub(crate) const SECRET_DOCKET: &str = "SECRET-DOCKET-4471 Smith v. Jones";

    impl DcrBuilder {
        pub(crate) fn new(format: Vec<u8>, mask: u64, names: &[&str]) -> Self {
            Self {
                major: 21,
                minor: 0,
                format,
                mask,
                names: names.iter().map(|name| name.to_string()).collect(),
                have_rec: "0",
                data: Vec::new(),
                extra_chunks: Vec::new(),
                docket: SECRET_DOCKET,
            }
        }

        pub(crate) fn legacy(mut self) -> Self {
            self.major = 20;
            self.minor = 5;
            self
        }

        pub(crate) fn marker(&mut self, marker: &[u8; 8]) -> &mut Self {
            self.data.extend_from_slice(marker);
            self
        }

        pub(crate) fn atom(&mut self, blocks: &[&[u8]]) -> &mut Self {
            for block in blocks {
                self.data.extend_from_slice(block);
            }
            self
        }

        pub(crate) fn video_frame(&mut self, payload_len: usize) -> &mut Self {
            self.data.extend_from_slice(b"cadr#bgn");
            self.data.extend_from_slice(&((12 + payload_len) as u32).to_le_bytes());
            self.data.extend(std::iter::repeat_n(0x5Au8, payload_len));
            self
        }

        pub(crate) fn attachment(&mut self, payload_len: usize, flags: u32) -> &mut Self {
            self.data.extend_from_slice(b"attx#bgn");
            self.data.extend_from_slice(&((24 + payload_len) as u64).to_le_bytes());
            self.data.extend_from_slice(&0u32.to_le_bytes());
            self.data.extend_from_slice(&flags.to_le_bytes());
            self.data.extend(std::iter::repeat_n(0x33u8, payload_len));
            self
        }

        pub(crate) fn participant(&mut self, start: bool, ac: &str) -> &mut Self {
            let text = format!("<ptch id=\"7\" ac=\"{ac}\" name=\"Remote Witness\"/>");
            self.data
                .extend_from_slice(if start { b"ptch#str" } else { b"ptch#stp" });
            self.data.extend_from_slice(&(text.len() as u32).to_le_bytes());
            self.data.extend_from_slice(text.as_bytes());
            self
        }

        fn meta_xml(&self) -> Vec<u8> {
            let hex: String = self.format.iter().map(|byte| format!("{byte:02X}")).collect();
            let items: String = self
                .names
                .iter()
                .map(|name| format!("<ITEM><CHNAME><![CDATA[{name}]]></CHNAME><CHDESC/><CHROLE/></ITEM>"))
                .collect();
            let mut xml = format!(
                "<META><AUDIO USECHNL=\"{}\"><FMTNAME><![CDATA[Synthetic]]></FMTNAME><FMTDATA><![CDATA[{hex}]]></FMTDATA><LISTCHNL>{items}</LISTCHNL></AUDIO><PROP TIMEZONE=\"-300\">x</PROP><DOCKNOTA><CASE><![CDATA[{}]]></CASE></DOCKNOTA><CRYPT HAVEREC=\"{}\" HAVEOVE=\"0\"><PWD>hunter2-secret</PWD></CRYPT></META>",
                self.mask, self.docket, self.have_rec
            )
            .into_bytes();
            // A Windows-1252 byte and a stray control character.
            xml.splice(0..0, [0x01u8]);
            xml
        }

        pub(crate) fn chunks(&self) -> Vec<([u8; 4], u32, Vec<u8>)> {
            let mut chunks = Vec::new();
            if self.major * 100 + self.minor >= META_VERSION {
                chunks.push((*b"meta", 0, self.meta_xml()));
            } else {
                chunks.push((*b"wfmt", 0, self.format.clone()));
                let mut chnl = (self.mask as u32).to_le_bytes().to_vec();
                chnl.extend_from_slice(&[0; 8]);
                chunks.push((*b"chnl", 0, chnl));
            }
            chunks.push((*b"info", 0, b"synthetic info".to_vec()));
            chunks.extend(self.extra_chunks.iter().cloned());
            chunks.push((*b"data", 0, self.data.clone()));
            chunks
        }

        pub(crate) fn bytes(&self) -> Vec<u8> {
            let mut out = DCR_MAGIC.to_vec();
            out.extend_from_slice(&self.major.to_le_bytes());
            out.extend_from_slice(&self.minor.to_le_bytes());
            let mut directory = Vec::new();
            for (tag, id, payload) in self.chunks() {
                let offset = out.len() as u64;
                out.extend_from_slice(&tag);
                out.extend_from_slice(b"#bgn");
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&payload);
                out.extend_from_slice(&tag);
                out.extend_from_slice(b"#end");
                directory.push((tag, id, offset, payload.len() as u64 + 20));
            }
            let head_offset = out.len() as u64;
            let head_size = directory.len() as u64 * 24 + 20;
            for (tag, id, offset, size) in directory {
                out.extend_from_slice(&tag);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&offset.to_le_bytes());
                out.extend_from_slice(&size.to_le_bytes());
            }
            out.extend_from_slice(b"head");
            out.extend_from_slice(&head_offset.to_le_bytes());
            out.extend_from_slice(&head_size.to_le_bytes());
            if self.major * 100 + self.minor >= META_VERSION {
                out.extend_from_slice(&[0u8; 12]);
            }
            out
        }

        pub(crate) fn write(&self, dir: &Path, name: &str) -> PathBuf {
            let path = dir.join(name);
            fs::write(&path, self.bytes()).unwrap();
            path
        }
    }

    pub(crate) fn sine_i16(frequency: f64, amplitude: f64, rate: u32, index: u64) -> i16 {
        let t = index as f64 / f64::from(rate);
        (amplitude * 32767.0 * (2.0 * std::f64::consts::PI * frequency * t).sin()).round() as i16
    }

    /// PCM 16-bit tones, one per bit of `mask`, `frames` long.
    pub(crate) fn pcm_tones(mask: u64, rate: u32, frames: u64, tones: &[(f64, f64)]) -> DcrBuilder {
        let channels = mask.count_ones() as usize;
        let mut builder = DcrBuilder::new(pcm16_format(rate), mask, &[]);
        builder.marker(b"audi#mrk");
        for index in 0..frames {
            for (frequency, amplitude) in tones.iter().take(channels) {
                builder
                    .data
                    .extend_from_slice(&sine_i16(*frequency, *amplitude, rate, index).to_le_bytes());
            }
        }
        builder
    }

    fn never() -> bool {
        false
    }

    fn read_wav_data(path: &Path) -> (Vec<u8>, Vec<u8>) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        let mut at = 12;
        let mut fmt = Vec::new();
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = le_u32(&bytes, at + 4) as usize;
            if id == b"fmt " {
                fmt = bytes[at + 8..at + 8 + size].to_vec();
            }
            if id == b"data" {
                return (fmt, bytes[at + 8..at + 8 + size].to_vec());
            }
            at += 8 + size + size % 2;
        }
        panic!("no data chunk");
    }

    #[test]
    fn inspects_a_four_channel_pcm_recording_with_non_contiguous_bits() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1011, 8000, 800, &[(300.0, 0.5), (500.0, 0.25), (700.0, 0.125)]);
        builder.names = ["Judge", "Witness", "Unused", "Counsel"].map(String::from).to_vec();
        let path = builder.write(&dir.0, "hearing.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.version, 2100);
        assert_eq!(info.codec_tag, 1);
        assert_eq!(info.codec, "PCM 16-bit");
        assert_eq!(info.sample_rate, 8000);
        assert_eq!(info.channels, 3);
        // Four items for bits 0..3: item N names bit N.
        assert_eq!(info.channel_names, ["Judge", "Witness", "Counsel"]);
        assert_eq!(info.atoms, 800);
        assert!((info.duration_secs - 0.1).abs() < 1e-9);
        assert_eq!(info.checksum, "not checked");
        assert_eq!(info.refusal(), None);
    }

    #[test]
    fn names_follow_the_mask_order_when_the_list_has_one_item_per_channel() {
        assert_eq!(
            channel_names(
                &[0, 1, 3],
                0b1011,
                &[Some("A".into()), Some("B".into()), Some("C".into())]
            ),
            ["A", "B", "C"]
        );
        assert_eq!(channel_names(&[0, 2], 0b101, &[]), ["Ch 1", "Ch 2"]);
        assert_eq!(
            channel_names(&[0, 1], 0b11, &[Some("Judge".into()), None]),
            ["Judge", "Ch 2"]
        );
    }

    #[test]
    fn interleaved_feed_holds_each_channels_blocks_in_bit_order() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b101, &["Left", "Right"]);
        builder.marker(b"audi#mrk");
        for index in 0..10i16 {
            builder.atom(&[&index.to_le_bytes(), &(1000 + index).to_le_bytes()]);
            if index == 4 {
                builder
                    .marker(b"audi#mrk")
                    .video_frame(30)
                    .attachment(10, 1)
                    .attachment(5, 0);
            }
        }
        let path = builder.write(&dir.0, "two.dcr");
        let before = fs::read(&path).unwrap();
        let feed = prepare_feed(&path, &never).unwrap();
        assert!(feed.merge.is_none());
        assert_eq!(feed.info.video_frames, 1);
        assert_eq!(feed.info.attachments, 2);
        assert_eq!(feed.info.attachments_deleted, 1);
        let (fmt, data) = read_wav_data(&feed.path);
        assert_eq!(le_u16(&fmt, 0), 1);
        assert_eq!(le_u16(&fmt, 2), 2);
        assert_eq!(le_u16(&fmt, 12), 4);
        let samples: Vec<i16> = data
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let expected: Vec<i16> = (0..10).flat_map(|index| [index, 1000 + index]).collect();
        assert_eq!(samples, expected);
        assert_eq!(fs::read(&path).unwrap(), before, "the original is never modified");
        let feed_path = feed.path.clone();
        drop(feed);
        assert!(!feed_path.exists(), "the temporary feed is removed with its guard");
    }

    #[test]
    fn a_participant_joining_and_leaving_gets_silence_while_absent() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &["Judge", "Witness"]);
        builder.marker(b"audi#mrk");
        builder.atom(&[&1i16.to_le_bytes(), &2i16.to_le_bytes()]);
        builder.participant(true, "0"); // no channel change
        builder.atom(&[&3i16.to_le_bytes(), &4i16.to_le_bytes()]);
        builder.participant(true, "1");
        builder.atom(&[&5i16.to_le_bytes(), &6i16.to_le_bytes(), &7i16.to_le_bytes()]);
        builder.participant(false, "1");
        builder.atom(&[&8i16.to_le_bytes(), &9i16.to_le_bytes()]);
        let path = builder.write(&dir.0, "remote.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        let info = &feed.info;
        assert_eq!(info.channels, 3);
        assert_eq!(info.channel_names, ["Judge", "Witness", "Ch 3"]);
        assert_eq!(info.conference_change_count, 2);
        assert_eq!(
            info.conference_changes,
            [
                ConferenceChange {
                    at_per_channel_byte: 4,
                    at_seconds: 4.0 / 16000.0,
                    kind: "joined",
                    channels_after: 3,
                    bit_guessed: true
                },
                ConferenceChange {
                    at_per_channel_byte: 6,
                    at_seconds: 6.0 / 16000.0,
                    kind: "left",
                    channels_after: 2,
                    bit_guessed: true
                },
            ]
        );
        let (_, data) = read_wav_data(&feed.path);
        let samples: Vec<i16> = data
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(samples, [1, 2, 0, 3, 4, 0, 5, 6, 7, 8, 9, 0]);
    }

    #[test]
    fn a_legacy_wfmt_and_chnl_recording_is_read() {
        let dir = TestDir::new();
        let path = pcm_tones(0b11, 8000, 80, &[(300.0, 0.5), (500.0, 0.25)])
            .legacy()
            .write(&dir.0, "old.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.version, 2005);
        assert_eq!(info.channels, 2);
        assert_eq!(info.channel_names, ["Ch 1", "Ch 2"]);
        assert_eq!(info.atoms, 80);
        assert_eq!(info.refusal(), None);
    }

    #[test]
    fn legacy_password_data_is_refused() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1, 8000, 8, &[(300.0, 0.5)]).legacy();
        builder.extra_chunks.push((*b"crpt", 0, vec![0, 0, 9, 9]));
        let path = builder.write(&dir.0, "locked.dcr");
        let error = prepare_feed(&path, &never).err().unwrap();
        assert_eq!(error, PROTECTED_MESSAGE);
    }

    #[test]
    fn encrypted_sections_and_the_crypt_flag_are_refused_before_anything_is_written() {
        let dir = TestDir::new();
        let mut encrypted = pcm_tones(0b11, 8000, 16, &[(300.0, 0.5), (500.0, 0.25)]);
        encrypted.marker(b"audi#crp").atom(&[&[1, 2], &[3, 4]]);
        let path = encrypted.write(&dir.0, "encrypted.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.encrypted_sections, 1);
        assert_eq!(info.refusal().as_deref(), Some(PROTECTED_MESSAGE));

        let mut flagged = pcm_tones(0b11, 8000, 16, &[(300.0, 0.5), (500.0, 0.25)]);
        flagged.have_rec = "1";
        let path = flagged.write(&dir.0, "flagged.dcr");
        let temp_before = temp_wav_count();
        assert_eq!(prepare_feed(&path, &never).err().as_deref(), Some(PROTECTED_MESSAGE));
        assert!(temp_wav_count() <= temp_before);
    }

    fn temp_wav_count() -> usize {
        // Other tests run in parallel, so this is only an upper-bound check.
        fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        name.starts_with("depoaudio_") && name.ends_with(".wav")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn unknown_and_speex_codecs_are_refused_by_name() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(waveformatex(0x0002, 8000, 4096, 256, 4, &[0xF4, 0x01]), 0b1, &[]);
        builder.atom(&[&[0u8; 256]]);
        let path = builder.write(&dir.0, "msadpcm.dcr");
        let error = prepare_feed(&path, &never).err().unwrap();
        assert!(error.contains("codec tag 0x0002"), "{error}");

        let mut speex = DcrBuilder::new(
            waveformatex(WAVE_FORMAT_SPEEX, 8000, 1000, 20, 16, &[1, 2, 3, 4]),
            0b1,
            &[],
        );
        speex.atom(&[&[0u8; 20]]);
        let path = speex.write(&dir.0, "speex.dcr");
        assert_eq!(prepare_feed(&path, &never).err().as_deref(), Some(SPEEX_MESSAGE));
    }

    #[test]
    fn misaligned_audio_is_refused() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b11, 8000, 4, &[(300.0, 0.5), (500.0, 0.25)]);
        builder.data.extend_from_slice(&[1, 2, 3]);
        let path = builder.write(&dir.0, "misaligned.dcr");
        let error = inspect(&path, &never).err().unwrap();
        assert!(error.contains("atom misalignment"), "{error}");
    }

    #[test]
    fn truncated_garbage_and_bad_directories_are_clear_errors() {
        let dir = TestDir::new();
        let good = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]).bytes();
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("empty", Vec::new(), "shorter than a DCR header"),
            (
                "garbage",
                b"not a recording at all, just text....".to_vec(),
                "does not start like",
            ),
            ("truncated", good[..good.len() - 30].to_vec(), "chunk directory"),
            ("header only", good[..16].to_vec(), "too short"),
            (
                "newer",
                {
                    let mut bytes = good.clone();
                    bytes[8..12].copy_from_slice(&28u32.to_le_bytes());
                    bytes
                },
                "newer than DepoAudio knows",
            ),
            (
                "directory size",
                {
                    let mut bytes = good.clone();
                    let at = bytes.len() - 12 - 8;
                    let size = le_u64(&bytes, at);
                    bytes[at..at + 8].copy_from_slice(&(size + 24).to_le_bytes());
                    bytes
                },
                "does not match the file size",
            ),
            (
                "outside",
                {
                    let mut bytes = good.clone();
                    let head_offset = le_u64(&bytes, bytes.len() - 12 - 16) as usize;
                    // Last entry (data): make its size run past the directory.
                    let entry = head_offset + 24 * 2;
                    bytes[entry + 16..entry + 24].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
                    bytes
                },
                "outside the file",
            ),
            (
                "overlap",
                {
                    let mut bytes = good.clone();
                    let head_offset = le_u64(&bytes, bytes.len() - 12 - 16) as usize;
                    let first_offset = le_u64(&bytes, head_offset + 8);
                    let entry = head_offset + 24;
                    bytes[entry + 8..entry + 16].copy_from_slice(&(first_offset + 4).to_le_bytes());
                    bytes
                },
                "overlap",
            ),
            (
                "framing",
                {
                    let mut bytes = good.clone();
                    let head_offset = le_u64(&bytes, bytes.len() - 12 - 16) as usize;
                    let data_offset = le_u64(&bytes, head_offset + 48 + 8) as usize;
                    bytes[data_offset] = b'X';
                    bytes
                },
                "start or end marker",
            ),
        ];
        for (name, bytes, expected) in cases {
            let path = dir.0.join(format!("{}.dcr", name.replace(' ', "_")));
            fs::write(&path, bytes).unwrap();
            let error = inspect(&path, &never)
                .err()
                .unwrap_or_else(|| panic!("{name} was accepted"));
            assert!(error.contains(expected), "{name}: {error}");
        }
    }

    #[test]
    fn oversized_records_inside_the_data_are_errors_not_panics() {
        let dir = TestDir::new();
        for marker in [b"cadr#bgn", b"attx#bgn", b"ptch#str"] {
            let mut builder = pcm_tones(0b1, 8000, 4, &[(300.0, 0.5)]);
            builder.data.extend_from_slice(marker);
            builder.data.extend_from_slice(&[0xFF; 16]);
            let path = builder.write(&dir.0, "big.dcr");
            let error = inspect(&path, &never).err().unwrap();
            assert!(error.contains("damaged"), "{error}");
        }
    }

    #[test]
    fn errors_and_records_never_carry_case_or_password_fields() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]);
        builder.have_rec = "1";
        let path = builder.write(&dir.0, "private.dcr");
        let info = inspect(&path, &never).unwrap();
        let everything = format!(
            "{} {} {} {:?}",
            serde_json::to_string(&info).unwrap(),
            serde_json::to_string(&info.record()).unwrap(),
            serde_json::to_string(&info.queue_info()).unwrap(),
            prepare_feed(&path, &never).err()
        );
        for secret in ["SECRET-DOCKET", "Smith v. Jones", "hunter2", "TIMEZONE", "DOCKNOTA"] {
            assert!(!everything.contains(secret), "{secret} leaked: {everything}");
        }
    }

    #[test]
    fn meta_parsing_handles_entities_cdata_and_windows_1252() {
        let mut xml = b"<META><AUDIO USECHNL=\"0x3\"><FMTDATA>".to_vec();
        xml.extend(
            pcm16_format(8000)
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>()
                .bytes(),
        );
        xml.extend_from_slice(b"</FMTDATA><LISTCHNL><ITEM><CHNAME>Caf\xE9 &amp; Bar</CHNAME></ITEM><ITEM><CHNAME><![CDATA[<Witness>]]></CHNAME></ITEM></LISTCHNL></AUDIO><CRYPT HAVEREC='0'/></META>");
        let parsed = parse_meta(&xml).unwrap();
        assert_eq!(parsed.mask, 3);
        assert_eq!(parsed.names, [Some("Caf\u{e9} & Bar".into()), Some("<Witness>".into())]);
        assert!(!parsed.protected_flag);
        assert!(parse_meta(b"<META><AUDIO USECHNL=\"0\"></AUDIO></META>").is_err());
        assert!(parse_meta(b"<META><AUDIO USECHNL=\"1\"><FMTDATA>ZZ</FMTDATA></AUDIO></META>").is_err());
    }

    #[test]
    fn ptch_ac_attribute_decides_whether_channels_change() {
        assert!(!ptch_changes_channels(b"<p ac=\"0\" x=\"1\"/>"));
        assert!(ptch_changes_channels(b"<p ac=\"1\"/>"));
        assert!(ptch_changes_channels(b"<p name=\"x\"/>"));
        assert!(!ptch_changes_channels(b"<p mac=\"1\" ac='0'/>"));
    }

    #[test]
    fn mono_codecs_get_one_wav_per_channel_and_a_merge_step() {
        let dir = TestDir::new();
        // A GSM 6.10 layout (the bytes are not real GSM frames; FFmpeg is not
        // run here).
        let format = waveformatex(WAVE_FORMAT_GSM610, 8000, 1625, 65, 0, &320u16.to_le_bytes());
        let mut builder = DcrBuilder::new(format.clone(), 0b11, &["A", "B"]);
        for index in 0..4u8 {
            builder.atom(&[&[index; 65], &[100 + index; 65]]);
        }
        let path = builder.write(&dir.0, "gsm.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        let merge = feed.merge.as_ref().expect("two GSM channels need a merge");
        assert_eq!(merge.inputs.len(), 2);
        for (channel, input) in merge.inputs.iter().enumerate() {
            let (fmt, data) = read_wav_data(&input.path);
            assert_eq!(fmt, format, "fmt is the stored one-channel WAVEFORMATEX");
            let expected: Vec<u8> = (0..4u8).flat_map(|index| [index + 100 * channel as u8; 65]).collect();
            assert_eq!(data, expected);
        }
        let filter = merge.args.iter().position(|arg| arg == "-filter_complex").unwrap();
        assert_eq!(merge.args[filter + 1], "[0:a:0][1:a:0]amerge=inputs=2[dcr]");
        let inputs: Vec<PathBuf> = merge.inputs.iter().map(|input| input.path.clone()).collect();
        drop(feed);
        assert!(inputs.iter().all(|input| !input.exists()), "mono inputs are removed");

        // GSM with a participant joining has no known silent block.
        let mut joined = DcrBuilder::new(format, 0b1, &[]);
        joined
            .atom(&[&[1; 65]])
            .participant(true, "1")
            .atom(&[&[1; 65], &[2; 65]]);
        let path = joined.write(&dir.0, "gsm-remote.dcr");
        let error = prepare_feed(&path, &never).err().unwrap();
        assert!(error.contains("remote participant"), "{error}");
    }

    #[test]
    fn rf64_header_is_used_only_past_four_gibibytes() {
        let small = wav_header(&pcm16_format(8000), None, 1000);
        assert_eq!(&small[..4], b"RIFF");
        let large = wav_header(&pcm16_format(8000), None, 5 << 30);
        assert_eq!(&large[..4], b"RF64");
        assert_eq!(&large[12..16], b"ds64");
        assert_eq!(le_u64(&large, 28), 5 << 30);
    }
}
