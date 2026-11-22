//! Liberty / BIS Digital `.dcr` court recordings (experimental).
//!
//! Layout source: owner-supplied interoperability notes; not yet confirmed
//! against a recorded file. The tests build synthetic files, and two
//! synthetic tones made by the owner's rebuild tool are committed under
//! `scripts/fixtures/dcr/vendor-notes-samples/`; no real recording has been
//! read. In our own words:
//!
//! - A 16-byte header: the ASCII signature `HGCRLCRS`, then a little-endian
//!   u32 major and u32 minor version (version = major × 100 + minor).
//! - Framed chunks: `<tag>#bgn`, a u32 id, the payload, then `<tag>#end`.
//! - A chunk directory at the end of the file: 24-byte entries (tag, id,
//!   offset and total size of the framed chunk), closed by `head` plus the
//!   directory's own offset and size. Version 21.00 and later files end with
//!   three u32 checksums: a position-weighted byte sum (41 weights, wrapping
//!   32-bit arithmetic) over everything before the `data` chunk, over an
//!   empty `data` chunk (the same for every file), and over everything after
//!   the `data` chunk up to the last checksum. They do not cover the audio.
//!   DepoAudio checks them and reports the result; a mismatch does not stop
//!   the recording from being read.
//! - The audio format: version 21.00+ keeps an XML document (UTF-8 unless it
//!   declares windows-1252) in the `meta` chunk: channel mask, a hex
//!   WAVEFORMATEX for ONE channel, channel names, the time-zone offset and a
//!   password flag; older files keep a raw WAVEFORMATEX in `wfmt` and the
//!   channel mask at the start of `chnl`. 64-bit values and times are child
//!   elements with two 32-bit halves (`N64Low`/`N64High`,
//!   `FTLow`/`FTHigh`), each written as a signed integer.
//! - The `data` chunk: one block per active channel at each step ("atom"),
//!   in ascending channel-bit order, with marker records between atoms
//!   (plain and password-protected section starts, video frames,
//!   attachments, and remote participants joining or leaving: a join adds
//!   the bits of the record's `ac` mask, a leave removes them).
//! - The `mark` chunk: bookmarks as XML. The top four bits of `IDMARK` are
//!   the bookmark type; type 13 begins a redaction (its action is a tag in
//!   the public note) and type 14 ends it. `POSMARK` is in samples per
//!   channel. `rinf` holds the session start time (`STARTREC`).
//!
//! Redactions are applied by default: a segment marked to be skipped is left
//! out of the output, one marked to be blacked out (or to have its video
//! distorted) is silent, and only one marked to play is kept. Audio in a
//! password-protected section is never decoded: it is silent in the output.
//!
//! Privacy: the XML also holds case and docket fields, registration data,
//! password data and bookmark notes. This module reads only the audio
//! element (mask, format, channel names), the password flag, the time-zone
//! offset, the session start time and each bookmark's type, position and
//! redaction action. Nothing else from the XML is kept, returned, logged or
//! written to an export record, and no error message quotes it. Bookmark
//! notes, speakers and attachment names are never kept: a redaction note is
//! searched only for its action tag. The text of a remote-participant record
//! (which may name the participant) is searched only for its `ac` attribute.

use std::collections::BTreeMap;
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
/// Versions 5.00 to 20.99 must have a `crpt` chunk (the recorder refuses a
/// file of those versions without one).
const CRPT_VERSION: u32 = 500;
/// The first version with the `TIMEZONE` attribute.
const TIME_ZONE_VERSION: u32 = 2700;
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

/// Where every refusal sends the person. Liberty Court Recorder and BIS
/// Digital recordings do not open in each other's software, so the guidance
/// names both and assumes neither.
macro_rules! export_hint {
    () => {
        "Export it to WAV in the software that recorded it (for example Liberty Court Recorder or BIS Digital), then add the WAV."
    };
}

pub(crate) const SPEEX_MESSAGE: &str = concat!("Speex recordings aren't supported yet. ", export_hint!());
pub(crate) const PTCH_UNREADABLE_MESSAGE: &str = concat!(
    "A remote-participant record in this recording can't be read, so DepoAudio can't tell which channels are recorded after it. ",
    export_hint!()
);
pub(crate) const MARK_UNREADABLE_MESSAGE: &str = concat!(
    "The bookmark section of this recording can't be read, so DepoAudio can't tell whether parts of it are redacted. ",
    export_hint!()
);
pub(crate) const BOOKMARK_UNREADABLE_MESSAGE: &str = concat!(
    "A bookmark in this recording can't be read, so DepoAudio can't tell whether parts of it are redacted. ",
    export_hint!()
);
pub(crate) const REDACTION_POSITION_MESSAGE: &str = concat!(
    "A redaction in this recording has no position, so DepoAudio can't tell which audio it covers. ",
    export_hint!()
);
pub(crate) const ALL_SKIPPED_MESSAGE: &str =
    "There is no audio to convert: the whole recording is inside redacted segments marked to be left out.";
pub(crate) const SEALED_WARNING: &str =
    "This recording has password-protected sections; they are silent in the output.";
pub(crate) const PROT_WARNING: &str =
    "This recording has a protection section that DepoAudio can't read yet. Check the output against the recording in the software that recorded it.";
pub(crate) const CHECKSUM_WARNING: &str = "This recording's checksums do not match, so its description or footer may have been changed or damaged (the checksums do not cover the audio). DepoAudio still reads it.";
pub(crate) const LAYOUT_SOURCE: &str =
    "owner-supplied interoperability notes; not yet confirmed against a recorded file";
pub(crate) const CHECKSUMS_MATCH: &str = "checksums match";
pub(crate) const CHECKSUMS_DIFFER: &str = "checksums do not match";
pub(crate) const CHECKSUMS_ABSENT: &str = "no checksums (format version before 21.00)";
const START_TIME_SOURCE: &str = "the session start stored in the recording (rinf STARTREC)";
const START_TIME_BASIS: &str = "stored as written; the format notes do not settle whether the recorder writes UTC or local time, so DepoAudio does not convert it";
const TIME_ZONE_SOURCE: &str =
    "the recording's time-zone field: local time minus UTC in minutes, measured on the recording machine";
const SKIPPED_TIMING_NOTE: &str = "Redacted audio marked to be left out is not in the output, so the output is shorter than the recording and anything after a left-out segment comes earlier in the output than in the recording.";

/// Weights of the checksum (one per byte position, repeating every 41
/// bytes), as signed 32-bit values; the sum wraps like u32 arithmetic.
const CHECKSUM_WEIGHTS: [i32; 41] = [
    -849_589_533,
    -870_359_049,
    -1_042_282_269,
    -1_881_238_787,
    -1_227_444_909,
    -1_987_268_895,
    -2_146_257_977,
    -1_466_366_915,
    -1_166_814_885,
    -945_283_949,
    -1_909_533_587,
    -339_067_949,
    -309_999_945,
    -2_092_564_389,
    -762_303_809,
    -1_504_296_365,
    -2_089_329_129,
    -664_808_483,
    -801_777_683,
    -985_554_767,
    -774_231_873,
    -2_088_218_825,
    -1_413_456_999,
    -734_757_029,
    -866_462_039,
    -512_120_073,
    -1_427_409_137,
    -506_243,
    -2_008_475_853,
    -293_134_467,
    -1_332_204_875,
    -1_796_186_499,
    -1_181_524_793,
    -953_506_605,
    -1_035_741_707,
    -2_145_128_513,
    -1_270_458_747,
    -1_485_294_515,
    -9_661_937,
    -1_551_685_893,
    -810_132_219,
];
/// The 20-byte `data` chunk as first written (no audio yet).
const EMPTY_DATA_CHUNK: &[u8; 20] = b"data#bgn\0\0\0\0data#end";
/// Bookmark types (the top four bits of `IDMARK`).
const BOOKMARK_BEGIN_REDACTION: u32 = 13;
const BOOKMARK_END_REDACTION: u32 = 14;
/// Largest bookmark section read.
const MAX_MARK_BYTES: u64 = 64 * 1024 * 1024;

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_ALAW: u16 = 0x0006;
const WAVE_FORMAT_MULAW: u16 = 0x0007;
const WAVE_FORMAT_IMA_ADPCM: u16 = 0x0011;
const WAVE_FORMAT_GSM610: u16 = 0x0031;
const WAVE_FORMAT_MPEGLAYER3: u16 = 0x0055;
const WAVE_FORMAT_SPEEX: u16 = 0xA109;

fn damaged(reason: &str) -> String {
    format!(
        "This .dcr recording is damaged or incomplete: {reason}. {}",
        export_hint!()
    )
}

fn several_data_chunks(count: usize) -> String {
    format!(
        "This recording has {count} audio data sections; DepoAudio reads recordings with one so far, so converting it could leave audio out. {}",
        export_hint!()
    )
}

fn several_mark_chunks(count: usize) -> String {
    format!(
        "This recording has {count} bookmark sections; DepoAudio reads recordings with one so far, so it can't tell whether parts of it are redacted. {}",
        export_hint!()
    )
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

    /// How long one block lasts: `(numerator, denominator)` seconds. From
    /// the samples in a block where the codec has a fixed count (the stored
    /// byte rate of an IMA ADPCM format is often only an estimate), else from
    /// the stored byte rate.
    fn block_duration(&self) -> (u64, u64) {
        match self.samples_per_block() {
            Some(samples) if self.sample_rate > 0 => (samples, u64::from(self.sample_rate)),
            _ => (u64::from(self.block_align), u64::from(self.avg_bytes_per_sec.max(1))),
        }
    }

    /// Seconds of audio in `atoms` blocks of one channel.
    fn atoms_secs(&self, atoms: u64) -> f64 {
        let (numerator, denominator) = self.block_duration();
        (u128::from(atoms) * u128::from(numerator)) as f64 / denominator as f64
    }

    /// The atom that holds sample `samples` (samples per channel from the
    /// start), or with `round_up` the first atom that starts at or after it.
    fn atom_at_sample(&self, samples: u64, round_up: bool) -> u128 {
        let (numerator, denominator) = self.block_duration();
        let scaled = u128::from(samples) * u128::from(denominator);
        let per_atom = u128::from(self.sample_rate.max(1)) * u128::from(numerator.max(1));
        if round_up {
            scaled.div_ceil(per_atom)
        } else {
            scaled / per_atom
        }
    }

    /// Why DepoAudio cannot decode this one-channel format, if it cannot.
    fn problem(&self) -> Option<String> {
        let codec = self.codec();
        match codec {
            Codec::Speex => return Some(SPEEX_MESSAGE.into()),
            Codec::Other => {
                return Some(format!(
                    "This recording uses audio codec tag 0x{:04X}, which DepoAudio can't decode yet. {}",
                    self.tag,
                    export_hint!()
                ))
            }
            _ => {}
        }
        let unsupported = |what: &str| {
            Some(format!(
                "This recording's {} audio uses {what}, which DepoAudio doesn't support. {}",
                self.codec_name(),
                export_hint!()
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
                // Times come from the samples in a block, and FFmpeg decodes
                // as many as the block size holds: the two must agree.
                if self.samples_per_block() != Some((u64::from(align) - 4) * 2 + 1) {
                    return unsupported("a samples-per-block count that does not match its block size");
                }
            }
            Codec::Gsm610 if align != 65 => {
                return unsupported("a block size other than 65 bytes");
            }
            Codec::Gsm610 if self.samples_per_block() != Some(320) => {
                return unsupported("a samples-per-block count other than 320");
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

/// A remote participant's channels starting or stopping (`ptch` record).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConferenceChange {
    /// Bytes of one channel's audio before the change.
    pub at_per_channel_byte: u64,
    pub at_seconds: f64,
    /// "joined" (the record's `ac` bits are added to the active channels)
    /// or "left" (they are removed).
    pub kind: &'static str,
    /// The record's `ac` channel mask.
    pub channel_mask: u32,
    pub channels_after: u32,
}

/// What a redacted segment does to the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RedactionAction {
    /// `<play_play>`: the audio is kept.
    Play,
    /// `<play_black>` / `<play_empty>`, and `<play_dist>` (a video effect;
    /// its audio is silenced): the audio is replaced by silence.
    Black,
    /// `<play_skip>` / `<play_omit>`, or no tag: the audio is left out.
    Skip,
}

/// One redacted segment as applied (overlapping segments are joined first;
/// the strictest action wins: skip, then black, then play).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RedactionRecord {
    /// Recording time, before anything is left out.
    pub start_secs: f64,
    pub end_secs: f64,
    pub action: RedactionAction,
    /// False only when the segment lies past the end of the audio, so there
    /// was nothing to apply it to.
    pub applied: bool,
}

/// Bookmarks read from the `mark` chunk: counts by type only. Their notes,
/// speakers and attachment names are never kept.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BookmarkCounts {
    pub total: u64,
    /// Bookmarks per type number (the top four bits of `IDMARK`).
    pub by_type: BTreeMap<u32, u64>,
    /// Bookmarks without a readable `IDMARK`, which the recorder's own
    /// reader also skips.
    #[serde(skip_serializing_if = "is_zero")]
    pub unreadable: u64,
}

/// The session start stored in the recording.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartTime {
    /// `YYYY-MM-DDTHH:MM:SS.mmm`, with no zone: see `basis`.
    pub stored: String,
    pub source: &'static str,
    pub basis: &'static str,
}

/// The recording machine's time-zone offset when it recorded.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimeZone {
    pub local_minus_utc_minutes: i32,
    /// For example "GMT-5:00".
    pub display: String,
    pub source: &'static str,
}

/// How left-out redactions change the output's length.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkippedTiming {
    pub recording_secs: f64,
    pub output_secs: f64,
    pub skipped_secs: f64,
    pub note: &'static str,
}

/// What `inspect` read from a recording. Format facts, channel names,
/// counts and times only.
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
    /// Atoms in the recording, password-protected ones included.
    pub atoms: u64,
    /// The recording's length.
    pub duration_secs: f64,
    /// Atoms in the output (fewer when redactions are left out).
    pub output_atoms: u64,
    pub output_duration_secs: f64,
    pub video_frames: u64,
    pub attachments: u64,
    pub attachments_deleted: u64,
    pub attachments_encrypted: u64,
    /// The first `MAX_RECORDED_CHANGES` changes.
    pub conference_changes: Vec<ConferenceChange>,
    pub conference_change_count: u64,
    /// Password-protected sections (`audi#crp`) and the atoms in them,
    /// which are silent in the output.
    pub sealed_sections: u64,
    pub sealed_atoms: u64,
    /// The recording says a password is set (`CRYPT HAVEREC`, or password
    /// data before version 21.00).
    pub password_set: bool,
    /// A `prot` chunk is present (its layout is not known).
    pub protection_chunk: bool,
    /// Bytes at the very end of the audio that were too few for one step
    /// across all channels and are left out (see `scan_data`).
    pub trailing_bytes_dropped: u64,
    pub checksum: &'static str,
    pub bookmarks: BookmarkCounts,
    pub redactions: Vec<RedactionRecord>,
    /// Redaction-end bookmarks with no begin before them, ignored.
    pub unpaired_redaction_ends: u64,
    /// Redactions marked to distort the video, whose audio is silenced.
    pub distort_as_silence: u64,
    pub start_time: Option<StartTime>,
    pub time_zone: Option<TimeZone>,
    #[serde(skip)]
    layout: Layout,
}

/// What happens to one atom in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomFate {
    Keep,
    Silence,
    Drop,
}

/// Atom ranges `[start, end)` that redactions silence or leave out, sorted
/// and not overlapping. Play segments are not listed (their audio is kept).
#[derive(Debug, Clone, Default, PartialEq)]
struct RedactionPlan {
    ranges: Vec<(u64, u64, RedactionAction)>,
}

impl RedactionPlan {
    fn action_at(&self, atom: u64, cursor: &mut usize) -> Option<RedactionAction> {
        while *cursor < self.ranges.len() && self.ranges[*cursor].1 <= atom {
            *cursor += 1;
        }
        self.ranges
            .get(*cursor)
            .filter(|(start, _, _)| *start <= atom)
            .map(|(_, _, action)| *action)
    }

    fn atoms_with(&self, wanted: RedactionAction) -> u64 {
        self.ranges
            .iter()
            .filter(|(_, _, action)| *action == wanted)
            .map(|(start, end, _)| end - start)
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Layout {
    format: WaveFormat,
    data_offset: u64,
    data_len: u64,
    initial_mask: u64,
    /// Converted channel bits, ascending.
    output_bits: Vec<u32>,
    plan: RedactionPlan,
    /// Why the recording cannot be converted although it could be read
    /// (an unreadable remote-participant record, several data chunks, ...).
    problem: Option<String>,
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

impl DcrInfo {
    /// Why this recording cannot be converted, if it cannot. Checked before
    /// any file is written.
    pub(crate) fn refusal(&self) -> Option<String> {
        if let Some(problem) = self.layout.format.problem() {
            return Some(problem);
        }
        if let Some(problem) = &self.layout.problem {
            return Some(problem.clone());
        }
        if self.channels > MAX_DCR_CHANNELS {
            return Some(format!(
                "This recording has {} channels; DepoAudio converts at most {MAX_DCR_CHANNELS}.",
                self.channels
            ));
        }
        if !self.silence_known() {
            let needs = [
                (self.conference_change_count > 0, "a remote participant joins or leaves"),
                (self.sealed_atoms > 0, "it has password-protected sections"),
                (!self.layout.plan.ranges.is_empty(), "it has redacted segments"),
            ];
            if let Some((_, reason)) = needs.iter().find(|(applies, _)| *applies) {
                return Some(format!(
                    "This {} recording can't be converted because {reason}: DepoAudio can write silence in place of audio, or leave audio out, only for PCM, A-law, mu-law and IMA ADPCM recordings so far. {}",
                    self.codec,
                    export_hint!()
                ));
            }
        }
        if self.atoms == 0 {
            return Some("This recording contains no audio.".into());
        }
        if self.output_atoms == 0 {
            return Some(ALL_SKIPPED_MESSAGE.into());
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

    fn skipped_timing(&self) -> Option<SkippedTiming> {
        (self.output_atoms < self.atoms).then_some(SkippedTiming {
            recording_secs: self.duration_secs,
            output_secs: self.output_duration_secs,
            skipped_secs: self.duration_secs - self.output_duration_secs,
            note: SKIPPED_TIMING_NOTE,
        })
    }

    /// Plain notices for the Convert queue, shown before anything is
    /// converted.
    pub(crate) fn notices(&self) -> Vec<String> {
        let mut notices = Vec::new();
        let applied: Vec<&RedactionRecord> = self.redactions.iter().filter(|record| record.applied).collect();
        if !applied.is_empty() {
            let count = |action| applied.iter().filter(|record| record.action == action).count() as u64;
            let parts: Vec<String> = [
                (count(RedactionAction::Skip), "left out"),
                (count(RedactionAction::Black), "silenced"),
                (count(RedactionAction::Play), "kept"),
            ]
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, what)| format!("{count} {what}"))
            .collect();
            notices.push(format!(
                "{}: {}, as marked in the recording.",
                plural(applied.len() as u64, "redacted segment", "redacted segments"),
                parts.join(", ")
            ));
        }
        if self.distort_as_silence == 1 {
            notices.push("One redacted segment is marked to distort the video; its audio is silenced.".into());
        } else if self.distort_as_silence > 1 {
            notices.push(format!(
                "{} redacted segments are marked to distort the video; their audio is silenced.",
                self.distort_as_silence
            ));
        }
        if let Some(timing) = self.skipped_timing() {
            notices.push(format!(
                "The output is {:.3} s shorter than the recording: times after a left-out segment come earlier in the output.",
                timing.skipped_secs
            ));
        }
        if self.unpaired_redaction_ends > 0 {
            notices.push(format!(
                "{} with no beginning before {} ignored.",
                plural(self.unpaired_redaction_ends, "redaction end", "redaction ends"),
                if self.unpaired_redaction_ends == 1 {
                    "it is"
                } else {
                    "them are"
                }
            ));
        }
        if self.sealed_atoms > 0 {
            notices.push(SEALED_WARNING.into());
        }
        if self.attachments_encrypted > 0 || self.attachments_deleted > 0 {
            notices.push("Protected or deleted attachments in this recording are not extracted.".into());
        }
        if self.protection_chunk {
            notices.push(PROT_WARNING.into());
        }
        if self.checksum == CHECKSUMS_DIFFER {
            notices.push(CHECKSUM_WARNING.into());
        }
        notices
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
            attachments_encrypted: self.attachments_encrypted,
            attachments_deleted: self.attachments_deleted,
            conference_changes: self.conference_changes.clone(),
            conference_change_count: self.conference_change_count,
            trailing_bytes_dropped: self.trailing_bytes_dropped,
            sealed_sections: self.sealed_sections,
            sealed_secs: self.atoms_secs(self.sealed_atoms),
            password_set: self.password_set,
            protection_chunk: self.protection_chunk,
            checksum: self.checksum,
            bookmarks: self.bookmarks.clone(),
            redactions: self.redactions.clone(),
            unpaired_redaction_ends: self.unpaired_redaction_ends,
            distort_as_silence: self.distort_as_silence,
            timing: self.skipped_timing(),
            start_time: self.start_time.clone(),
            time_zone: self.time_zone.clone(),
            layout_source: LAYOUT_SOURCE,
        }
    }

    fn atoms_secs(&self, atoms: u64) -> f64 {
        self.layout.format.atoms_secs(atoms)
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
            trailing_bytes_dropped: self.trailing_bytes_dropped,
            checksum: self.checksum.into(),
            notices: self.notices(),
            problem: self.refusal(),
        }
    }
}

/// The optional `source.dcr` object of an export record: format facts,
/// counts and times only, never case, docket, password or bookmark-note
/// fields.
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
    #[serde(skip_serializing_if = "is_zero")]
    pub attachments_encrypted: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub attachments_deleted: u64,
    pub conference_changes: Vec<ConferenceChange>,
    pub conference_change_count: u64,
    /// Present only when bytes at the end of the audio were left out.
    #[serde(skip_serializing_if = "is_zero")]
    pub trailing_bytes_dropped: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub sealed_sections: u64,
    #[serde(skip_serializing_if = "is_zero_secs")]
    pub sealed_secs: f64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub password_set: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub protection_chunk: bool,
    pub checksum: &'static str,
    pub bookmarks: BookmarkCounts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<RedactionRecord>,
    #[serde(skip_serializing_if = "is_zero")]
    pub unpaired_redaction_ends: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub distort_as_silence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<SkippedTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<StartTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<TimeZone>,
    pub layout_source: &'static str,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_zero_secs(value: &f64) -> bool {
    *value == 0.0
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
    pub trailing_bytes_dropped: u64,
    pub checksum: String,
    /// Redactions, password-protected sections, checksum and other notices.
    pub notices: Vec<String>,
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
            "This DCR was written by recorder format version {major}, which is newer than DepoAudio knows. {}",
            export_hint!()
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

    /// How many chunks have this tag, whatever their ids.
    fn count(&self, tag: &[u8; 4]) -> usize {
        self.entries.iter().filter(|entry| &entry.tag == tag).count()
    }

    /// The entry `(tag, 0)`, or else the first chunk with this tag in the
    /// file. For tags that DepoAudio refuses to read more than one of, so
    /// that the refusal names them all.
    fn first(&self, tag: &[u8; 4]) -> Option<Entry> {
        self.find(tag, 0)
            .or_else(|| self.entries.iter().find(|entry| &entry.tag == tag).copied())
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

/// The recorder's XML text: UTF-8 (a byte-order mark is dropped) unless the
/// document declares windows-1252, without control characters other than
/// tab, CR and LF.
fn decode_xml(bytes: &[u8]) -> String {
    let head = &bytes[..bytes.len().min(256)];
    let declared_1252 = String::from_utf8_lossy(head)
        .to_ascii_lowercase()
        .split("encoding=")
        .nth(1)
        .is_some_and(|rest| rest.trim_start_matches(['"', '\'']).starts_with("windows-1252"));
    if declared_1252 {
        return decode_windows_1252(bytes);
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(body)
        .chars()
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
/// `from`, skipping CDATA sections and comments.
fn close_tag_start(text: &str, from: usize, name: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        let rest = &text[index..];
        if rest.starts_with("<![CDATA[") {
            index += rest.find("]]>")? + 3;
            continue;
        }
        if rest.starts_with("<!--") {
            index += rest.find("-->")? + 3;
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
/// CDATA sections and comments are skipped.
fn elements<'a>(text: &'a str, name: &str) -> Vec<Element<'a>> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(offset) = text[index..].find('<') {
        let start = index + offset;
        let rest = &text[start + 1..];
        if let Some(closing) = [("![CDATA[", "]]>"), ("!--", "-->")]
            .iter()
            .find_map(|(opening, closing)| rest.starts_with(opening).then_some(*closing))
        {
            match rest.find(closing) {
                Some(end) => {
                    index = start + 1 + end + closing.len();
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

/// The value of attribute `name`: a name that starts the text or follows
/// white space (so `data-ac` is not `ac`), outside any quoted value, followed
/// by `=` and a quoted value.
fn attribute(attributes: &str, name: &str) -> Option<String> {
    let bytes = attributes.as_bytes();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            index += 1;
            continue;
        }
        let starts_word = index == 0 || bytes[index - 1].is_ascii_whitespace();
        if starts_word && attributes.is_char_boundary(index) && tag_like_name(&attributes[index..], name) {
            let after = attributes[index + name.len()..].trim_start();
            if let Some(after_equals) = after.strip_prefix('=') {
                let value = after_equals.trim_start();
                let open = value.chars().next()?;
                if open == '"' || open == '\'' {
                    let body = &value[1..];
                    let end = body.find(open)?;
                    return Some(decode_entities(&body[..end]));
                }
            }
        }
        index += 1;
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
            .is_some_and(|next| next.is_ascii_alphanumeric() || matches!(next, '_' | '-' | ':' | '.'))
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
/// Comments outside CDATA are not text.
fn text_content(inner: &str) -> String {
    let without_comments;
    let inner = if inner.contains("<!--") {
        without_comments = strip_comments(inner);
        without_comments.as_str()
    } else {
        inner
    };
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

/// `text` without its comments (`<!-- … -->`, an unclosed one running to
/// the end); CDATA sections are kept whole.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let comment = rest.find("<!--");
        let cdata = rest.find("<![CDATA[");
        match (comment, cdata) {
            (Some(at), cdata) if cdata.is_none_or(|cdata| at < cdata) => {
                out.push_str(&rest[..at]);
                rest = rest[at..].find("-->").map_or("", |end| &rest[at + end + 3..]);
            }
            (_, Some(at)) => {
                let end = rest[at..].find("]]>").map_or(rest.len(), |end| at + end + 3);
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
            _ => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// A 32-bit value the recorder writes with `%i`: a signed decimal (read
/// back as the same 32 bits), an unsigned decimal, or `0x` hex.
fn parse_bits32(text: &str) -> Option<u32> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    let value = text.parse::<i64>().ok()?;
    if (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&value) {
        Some(value as u32)
    } else {
        None
    }
}

fn parse_mask(text: &str) -> Option<u64> {
    parse_bits32(text).filter(|mask| *mask != 0).map(u64::from)
}

/// A 64-bit value stored as a child element with two 32-bit halves, for
/// example `<POSMARK N64Low="32000" N64High="0"/>` or a FILETIME
/// `<STARTREC FTLow="…" FTHigh="…"/>`.
fn split_u64(inner: &str, name: &str, low: &str, high: &str) -> Option<u64> {
    let found = element(inner, name)?;
    let low = parse_bits32(&attribute(found.attributes, low)?)?;
    let high = parse_bits32(&attribute(found.attributes, high)?)?;
    Some((u64::from(high) << 32) | u64::from(low))
}

/// The elements directly inside `text`, with their names, in order. Element
/// names are not checked against a list: the recorder's own reader does not
/// check them either. Nested elements with the same name are matched. None
/// when any of them cannot be read to its end (an unclosed tag, element,
/// comment or CDATA section): a partial list is never returned as whole.
fn child_elements(text: &str) -> Option<Vec<(&str, Element<'_>)>> {
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(offset) = text[index..].find('<') {
        let start = index + offset;
        let rest = &text[start..];
        let skip_to = |closing: &str| rest.find(closing).map(|end| start + end + closing.len());
        let next = if rest.starts_with("<![CDATA[") {
            skip_to("]]>")
        } else if rest.starts_with("<!--") {
            skip_to("-->")
        } else if rest.starts_with("<?") {
            skip_to("?>")
        } else if rest.starts_with("</") || rest.starts_with("<!") {
            skip_to(">")
        } else {
            None
        };
        if rest.starts_with("<!") || rest.starts_with("<?") || rest.starts_with("</") {
            index = next?;
            continue;
        }
        let name_end = rest[1..]
            .find(|character: char| character.is_whitespace() || character == '/' || character == '>')
            .map_or(rest.len(), |end| end + 1);
        let name = &rest[1..name_end];
        if name.is_empty() {
            index = start + 1;
            continue;
        }
        let tag_end = open_tag_end(text, start + name_end)?;
        let attributes = &text[start + name_end..tag_end];
        if attributes.trim_end().ends_with('/') {
            found.push((
                name,
                Element {
                    attributes: attributes.trim_end().trim_end_matches('/'),
                    inner: "",
                },
            ));
            index = tag_end + 1;
            continue;
        }
        let close = matching_close(text, tag_end + 1, name)?;
        found.push((
            name,
            Element {
                attributes,
                inner: &text[tag_end + 1..close],
            },
        ));
        index = close + text[close..].find('>')? + 1;
    }
    Some(found)
}

/// The `</NAME>` closing an element whose content starts at `from`, counting
/// nested `NAME` elements and skipping CDATA sections and comments.
fn matching_close(text: &str, from: usize, name: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = from;
    while let Some(offset) = text[index..].find('<') {
        let at = index + offset;
        let rest = &text[at..];
        if rest.starts_with("<![CDATA[") {
            index = at + rest.find("]]>")? + 3;
            continue;
        }
        if rest.starts_with("<!--") {
            index = at + rest.find("-->")? + 3;
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            if tag_name_matches(after, name) {
                if depth == 0 {
                    return Some(at);
                }
                depth -= 1;
            }
            index = at + 2;
            continue;
        }
        if tag_name_matches(&rest[1..], name) {
            let end = open_tag_end(text, at + 1 + name.len())?;
            if !text[at + 1 + name.len()..end].trim_end().ends_with('/') {
                depth += 1;
            }
            index = end + 1;
            continue;
        }
        index = at + 1;
    }
    None
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

/// Invisible format characters (Unicode category Cf) that could reorder or
/// hide text in a file name: soft hyphen, Arabic letter mark, zero-width
/// characters and joiners, direction marks, embeddings, overrides and
/// isolates, word joiner and invisible operators, the byte-order mark and
/// interlinear annotation marks.
fn is_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

fn bounded_channel_name(raw: &str) -> Option<String> {
    let name: String = raw
        .chars()
        .filter(|character| !character.is_control() && !is_format_character(*character))
        .take(MAX_CHANNEL_NAME_CHARS)
        .collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// What the META document says about the audio, the password flag and the
/// time-zone offset. Nothing else is read.
#[derive(Debug, Clone, PartialEq)]
struct AudioDescription {
    format: Vec<u8>,
    mask: u64,
    names: Vec<Option<String>>,
    password_set: bool,
    time_zone_minutes: Option<i32>,
}

fn parse_meta(bytes: &[u8], version: u32) -> Result<AudioDescription, String> {
    let text = decode_xml(bytes);
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
    let password_set = elements(meta.inner, "CRYPT").iter().any(|crypt| {
        ["HAVEREC", "HAVEOVE"].iter().any(|name| {
            attribute(crypt.attributes, name).is_some_and(|value| {
                let value = value.trim();
                !value.is_empty() && value != "0"
            })
        })
    });
    // Local minus UTC in minutes; more than a day either way is not an
    // offset. Older versions have no such field, so one is not read there.
    let time_zone_minutes = element(meta.inner, "PROP")
        .filter(|_| version >= TIME_ZONE_VERSION)
        .and_then(|prop| attribute(prop.attributes, "TIMEZONE"))
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|minutes| minutes.abs() <= 24 * 60);
    Ok(AudioDescription {
        format,
        mask,
        names,
        password_set,
        time_zone_minutes,
    })
}

/// Version 20.xx and earlier: `wfmt`, `chnl` and a `crpt` chunk (required
/// from 5.00).
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
    // Password data: any non-zero byte counts as a password being set.
    let password_set = match container.find(b"crpt", 0) {
        Some(crpt) => read_payload(file, crpt, MAX_META_BYTES, "protection chunk")?
            .iter()
            .any(|byte| *byte != 0),
        None if container.version >= CRPT_VERSION => return Err(damaged("its password chunk is missing")),
        None => false,
    };
    Ok(AudioDescription {
        format,
        mask,
        names: Vec::new(),
        password_set,
        time_zone_minutes: None,
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
    attachments_encrypted: u64,
    sealed_sections: u64,
    sealed_atoms: u64,
    changes: Vec<ConferenceChange>,
    change_count: u64,
    union_mask: u64,
    trailing_bytes_dropped: u64,
    /// Why the walk stopped early on a readable file that must be refused.
    problem: Option<String>,
}

/// What a `ptch` record's `ac` attribute says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtchEffect {
    /// The channel mask the record adds (`#str`) or removes (`#stp`); 0
    /// changes nothing.
    Mask(u32),
    /// No readable `ac` attribute: DepoAudio cannot tell.
    Unknown,
}

/// A `ptch` record's text. The notes do not give its encoding: UTF-16 (by
/// byte-order mark, or zero bytes in every other position) or else
/// Windows-1252, which reads ASCII unchanged.
fn decode_ptch_text(bytes: &[u8]) -> String {
    let utf16 = |body: &[u8], little: bool| -> String {
        let units = body.chunks_exact(2).map(|pair| {
            if little {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        });
        char::decode_utf16(units)
            .map(|unit| unit.unwrap_or('\u{FFFD}'))
            .collect()
    };
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(body, true);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(body, false);
    }
    let pairs = bytes.len() / 2;
    if pairs > 0 {
        let zero_at = |parity: usize| bytes.chunks_exact(2).filter(|pair| pair[parity] == 0).count();
        let (even, odd) = (zero_at(0), zero_at(1));
        if odd * 2 >= pairs && even * 4 < pairs {
            return utf16(bytes, true);
        }
        if even * 2 >= pairs && odd * 4 < pairs {
            return utf16(bytes, false);
        }
    }
    decode_windows_1252(bytes)
}

fn ptch_effect(text: &[u8]) -> PtchEffect {
    match attribute(&decode_ptch_text(text), "ac")
        .as_deref()
        .and_then(parse_bits32)
    {
        Some(mask) => PtchEffect::Mask(mask),
        None => PtchEffect::Unknown,
    }
}

/// The 8-byte record markers that may start at an atom boundary.
const DATA_MARKERS: [&[u8; 8]; 6] = [
    b"audi#mrk",
    b"audi#crp",
    b"cadr#bgn",
    b"attx#bgn",
    b"ptch#str",
    b"ptch#stp",
];

/// Whether one of the record markers starts inside `atom` (`bytes` runs on
/// past it) at a block boundary other than the first: the sign of an atom
/// width that does not match the data, since a marker belongs between atoms.
fn marker_inside_atom(bytes: &[u8], width: usize, block: usize) -> bool {
    (block..width).step_by(block).any(|at| {
        bytes
            .get(at..at + 8)
            .is_some_and(|candidate| DATA_MARKERS.iter().any(|marker| candidate == &marker[..]))
    })
}

/// Walk the data chunk atom by atom. `sink` receives every audio atom with
/// the channel mask active for it and whether it is in a password-protected
/// section (after `audi#crp`, until the next `audi#mrk`). Protected atoms are
/// counted at the plain atom size and never decoded; their bytes go to the
/// sink only so it can count them, and the feed writers write silence.
///
/// A remote participant joining (`ptch#str`) adds the bits of the record's
/// `ac` mask to the active channels; leaving (`ptch#stp`) removes them; an
/// `ac` of 0 changes nothing. The walk stops early, with the reason in
/// `ScanStats` (the recording is then refused), at a participant record
/// whose `ac` attribute cannot be read. A last run of bytes shorter than one
/// atom is left out and counted when no participant change came before it
/// (the channel count then comes straight from the recording's
/// description); after a change it is an error, since a misread change is
/// the likelier cause. An atom with a record marker inside it at a block
/// boundary is an error too.
fn scan_data(
    file: &mut File,
    layout: &Layout,
    cancelled: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(u64, &[u8], bool) -> Result<(), String>,
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
    let mut sealed = false;
    let mut steps: u64 = 0;
    let misaligned = || damaged("its audio data is out of step with the channel layout (atom misalignment)");
    while region.left() > 0 {
        steps += 1;
        if steps.is_multiple_of(CANCEL_CHECK_ATOMS) && cancelled() {
            return Err(crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
        }
        let width = active.count_ones() as usize * block;
        region.fill((width + 8).max(24))?;
        let left = region.left();
        let bytes = region.bytes();
        if bytes.len() >= 8 {
            let marker: [u8; 8] = bytes[..8].try_into().expect("eight bytes");
            match &marker {
                b"audi#mrk" => {
                    sealed = false;
                    region.consume(8);
                    continue;
                }
                b"audi#crp" => {
                    if !sealed {
                        stats.sealed_sections += 1;
                    }
                    sealed = true;
                    region.consume(8);
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
                    // Attachments are never extracted; deleted (bit 0) and
                    // password-protected (bit 1) ones are counted.
                    stats.attachments += 1;
                    if flags & 1 != 0 {
                        stats.attachments_deleted += 1;
                    }
                    if flags & 2 != 0 {
                        stats.attachments_encrypted += 1;
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
                    let effect = ptch_effect(&region.bytes()[12..searched]);
                    region.skip(total)?;
                    let mask = match effect {
                        PtchEffect::Mask(0) => continue,
                        PtchEffect::Mask(mask) => mask,
                        PtchEffect::Unknown => {
                            stats.problem = Some(PTCH_UNREADABLE_MESSAGE.into());
                            break;
                        }
                    };
                    if joined {
                        active |= u64::from(mask);
                    } else {
                        active &= !u64::from(mask);
                    }
                    stats.union_mask |= active;
                    stats.change_count += 1;
                    if stats.changes.len() < MAX_RECORDED_CHANGES {
                        let at_per_channel_byte = stats.atoms * block as u64;
                        stats.changes.push(ConferenceChange {
                            at_per_channel_byte,
                            at_seconds: format.atoms_secs(stats.atoms),
                            kind: if joined { "joined" } else { "left" },
                            channel_mask: mask,
                            channels_after: active.count_ones(),
                        });
                    }
                    continue;
                }
                _ => {}
            }
        }
        if width == 0 || marker_inside_atom(bytes, width.min(bytes.len()), block) {
            return Err(misaligned());
        }
        if bytes.len() < width {
            // Everything left is shorter than one atom.
            if stats.change_count > 0 {
                return Err(misaligned());
            }
            stats.trailing_bytes_dropped = left;
            break;
        }
        sink(active, &bytes[..width], sealed)?;
        region.consume(width);
        stats.atoms += 1;
        if sealed {
            stats.sealed_atoms += 1;
        }
    }
    Ok(stats)
}

// ── Footer chunks: checksums, bookmarks, session start ─────────────────────

/// Add `bytes` to a running checksum; `position` is the index of
/// `bytes[0]` in the summed range.
fn checksum_update(sum: u32, position: u64, bytes: &[u8]) -> u32 {
    let mut sum = sum;
    let mut weight = (position % CHECKSUM_WEIGHTS.len() as u64) as usize;
    for &byte in bytes {
        sum = sum.wrapping_add(u32::from(byte).wrapping_mul(CHECKSUM_WEIGHTS[weight] as u32));
        weight += 1;
        if weight == CHECKSUM_WEIGHTS.len() {
            weight = 0;
        }
    }
    sum
}

fn checksum(bytes: &[u8]) -> u32 {
    checksum_update(0, 0, bytes)
}

fn checksum_file_range(file: &mut File, offset: u64, length: u64, cancelled: &dyn Fn() -> bool) -> Result<u32, String> {
    file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
    let mut buffer = vec![0u8; SCAN_BUFFER_BYTES];
    let mut sum = 0u32;
    let mut done = 0u64;
    while done < length {
        if cancelled() {
            return Err(crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
        }
        let want = usize::try_from(length - done).unwrap_or(usize::MAX).min(buffer.len());
        file.read_exact(&mut buffer[..want]).map_err(read_error)?;
        sum = checksum_update(sum, done, &buffer[..want]);
        done += want as u64;
    }
    Ok(sum)
}

/// Check the three stored checksums (version 21.00 and later): everything
/// before the `data` chunk, the empty `data` chunk as first written, and
/// everything after the `data` chunk up to the last checksum.
fn verify_checksums(
    file: &mut File,
    file_len: u64,
    version: u32,
    data: Entry,
    cancelled: &dyn Fn() -> bool,
) -> Result<&'static str, String> {
    if version < META_VERSION {
        return Ok(CHECKSUMS_ABSENT);
    }
    let stored = read_at(file, file_len - CHECKSUM_TAIL_BYTES, CHECKSUM_TAIL_BYTES)?;
    let data_end = data.offset + data.size;
    let before = checksum_file_range(file, 0, data.offset, cancelled)?;
    let after = checksum_file_range(file, data_end, file_len - 4 - data_end, cancelled)?;
    let matches =
        le_u32(&stored, 0) == before && le_u32(&stored, 4) == checksum(EMPTY_DATA_CHUNK) && le_u32(&stored, 8) == after;
    Ok(if matches { CHECKSUMS_MATCH } else { CHECKSUMS_DIFFER })
}

/// One bookmark: its type and position. Nothing else is kept.
#[derive(Debug, Clone, PartialEq)]
struct Bookmark {
    kind: u32,
    /// Samples per channel from the start; None when it has no position.
    position: Option<u64>,
    /// For a Begin Redaction bookmark: the action in its note.
    action: Option<MarkedAction>,
}

/// The redaction action written in a Begin Redaction note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkedAction {
    Play,
    Skip,
    Black,
    Distort,
}

impl MarkedAction {
    fn applied(self) -> RedactionAction {
        match self {
            Self::Play => RedactionAction::Play,
            Self::Skip => RedactionAction::Skip,
            Self::Black | Self::Distort => RedactionAction::Black,
        }
    }
}

/// The action tag in a Begin Redaction note. Several tags: the strictest.
/// No tag: skip, as the recorder does.
fn marked_action(note: &str) -> MarkedAction {
    let note = note.to_ascii_lowercase();
    let has = |tag: &str| note.contains(tag);
    if has("<play_skip>") || has("<play_omit>") {
        MarkedAction::Skip
    } else if has("<play_black>") || has("<play_empty>") {
        MarkedAction::Black
    } else if has("<play_dist") {
        MarkedAction::Distort
    } else if has("<play_play>") {
        MarkedAction::Play
    } else {
        MarkedAction::Skip
    }
}

/// Bookmarks of the `mark` chunk, and how many items could not be read
/// (no readable `IDMARK`, or content that cannot be read to its end). None
/// when it has text but no list of bookmarks can be read in full from it.
fn parse_bookmarks(bytes: &[u8]) -> Option<(Vec<Bookmark>, u64)> {
    let text = decode_xml(bytes);
    if text.trim().is_empty() {
        return Some((Vec::new(), 0));
    }
    let root = element(&text, "Root").map_or(text.as_str(), |root| root.inner);
    let lists = child_elements(root)?;
    let (_, list) = lists.first()?;
    let mut bookmarks = Vec::new();
    let mut unreadable = 0u64;
    for (_, item) in child_elements(list.inner)? {
        let id = attribute(item.attributes, "IDMARK").as_deref().and_then(parse_bits32);
        let Some(id) = id.filter(|_| child_elements(item.inner).is_some()) else {
            unreadable += 1;
            continue;
        };
        let kind = id >> 28;
        let position = split_u64(item.inner, "POSMARK", "N64Low", "N64High").filter(|value| *value != u64::MAX);
        let action = (kind == BOOKMARK_BEGIN_REDACTION).then(|| {
            marked_action(
                &element(item.inner, "PUBLNOTE")
                    .map(|note| text_content(note.inner))
                    .unwrap_or_default(),
            )
        });
        bookmarks.push(Bookmark { kind, position, action });
    }
    Some((bookmarks, unreadable))
}

fn bookmark_counts(bookmarks: &[Bookmark], unreadable: u64) -> BookmarkCounts {
    let mut by_type = BTreeMap::new();
    for bookmark in bookmarks {
        *by_type.entry(bookmark.kind).or_insert(0u64) += 1;
    }
    BookmarkCounts {
        total: bookmarks.len() as u64 + unreadable,
        by_type,
        unreadable,
    }
}

/// A redacted segment in samples per channel from the start of the
/// recording (the unit of a bookmark position), `[start, end)`; `end` None
/// runs to the end of the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    start: u64,
    end: Option<u64>,
    action: RedactionAction,
}

/// What the redaction bookmarks mark, paired and joined.
#[derive(Debug, Default, PartialEq)]
struct Redactions {
    segments: Vec<Segment>,
    unpaired_ends: u64,
    distort: u64,
    /// A Begin Redaction without a position: the recording is refused.
    unplaced: bool,
}

/// Pair each Begin Redaction with the next unused End Redaction at or after
/// it; an unpaired Begin runs to the end of the recording, an End with no
/// Begin before it is ignored (and counted). Overlapping segments are then
/// joined; where they overlap, the strictest action wins.
fn pair_redactions(bookmarks: &[Bookmark]) -> Redactions {
    let mut result = Redactions::default();
    let mut begins: Vec<(u64, MarkedAction)> = Vec::new();
    let mut ends: Vec<u64> = Vec::new();
    for bookmark in bookmarks {
        match (bookmark.kind, bookmark.position) {
            (BOOKMARK_BEGIN_REDACTION, Some(position)) => {
                begins.push((position, bookmark.action.unwrap_or(MarkedAction::Skip)));
            }
            (BOOKMARK_BEGIN_REDACTION, None) => result.unplaced = true,
            (BOOKMARK_END_REDACTION, Some(position)) => ends.push(position),
            (BOOKMARK_END_REDACTION, None) => result.unpaired_ends += 1,
            _ => {}
        }
    }
    begins.sort_by_key(|(position, _)| *position);
    ends.sort_unstable();
    // Begins in order: each takes the first end at or after it that no
    // earlier begin took. Ends passed over are before every later begin.
    let mut next_end = 0;
    let mut raw = Vec::new();
    for (start, marked) in begins {
        while next_end < ends.len() && ends[next_end] < start {
            next_end += 1;
            result.unpaired_ends += 1;
        }
        let end = ends.get(next_end).copied();
        if end.is_some() {
            next_end += 1;
        }
        if marked == MarkedAction::Distort {
            result.distort += 1;
        }
        raw.push(Segment {
            start,
            end,
            action: marked.applied(),
        });
    }
    result.unpaired_ends += (ends.len() - next_end) as u64;
    result.segments = join_segments(&raw);
    result
}

/// Sorted, non-overlapping `[start, end)` ranges covering the same ground as
/// `ranges`: each stretch gets the strictest action of the ranges that
/// cover it (skip, then black, then play), and touching stretches with the
/// same action are joined. Empty ranges are dropped.
fn strictest_cover(ranges: &[(u64, u64, RedactionAction)]) -> Vec<(u64, u64, RedactionAction)> {
    const ACTIONS: [RedactionAction; 3] = [RedactionAction::Play, RedactionAction::Black, RedactionAction::Skip];
    let slot = |action: RedactionAction| ACTIONS.iter().position(|known| *known == action).unwrap_or(0);
    // A sweep over start (+1) and end (-1) events, counting open ranges per
    // action.
    let mut events: Vec<(u64, i64, usize)> = ranges
        .iter()
        .filter(|(start, end, _)| start < end)
        .flat_map(|(start, end, action)| [(*start, 1, slot(*action)), (*end, -1, slot(*action))])
        .collect();
    events.sort_unstable();
    let mut open = [0i64; 3];
    let mut joined: Vec<(u64, u64, RedactionAction)> = Vec::new();
    let mut index = 0;
    while index < events.len() {
        let at = events[index].0;
        while index < events.len() && events[index].0 == at {
            open[events[index].2] += events[index].1;
            index += 1;
        }
        let Some(&(next, _, _)) = events.get(index) else {
            break;
        };
        let Some(strictest) = (0..3).rev().find(|slot| open[*slot] > 0) else {
            continue;
        };
        let action = ACTIONS[strictest];
        match joined.last_mut() {
            Some(last) if last.1 == at && last.2 == action => last.1 = next,
            _ => joined.push((at, next, action)),
        }
    }
    joined
}

/// Join overlapping or touching segments (see `strictest_cover`). A
/// segment with no length covers no audio but is still listed.
fn join_segments(raw: &[Segment]) -> Vec<Segment> {
    let ranges: Vec<(u64, u64, RedactionAction)> = raw
        .iter()
        .map(|segment| (segment.start, segment.end.unwrap_or(u64::MAX), segment.action))
        .collect();
    let mut joined: Vec<Segment> = strictest_cover(&ranges)
        .into_iter()
        .map(|(start, end, action)| Segment {
            start,
            end: (end != u64::MAX).then_some(end),
            action,
        })
        .collect();
    for segment in raw {
        let empty = segment.end == Some(segment.start);
        let covered = joined
            .iter()
            .any(|kept| kept.start <= segment.start && kept.end.is_none_or(|end| end > segment.start));
        if empty && !covered {
            joined.push(*segment);
        }
    }
    joined.sort_by_key(|segment| segment.start);
    joined.dedup();
    joined
}

/// The atoms each segment covers: every atom that overlaps it (atom `k`
/// holds samples `k × n` to `(k + 1) × n`, `n` samples per block), so no
/// partly covered block is kept. Positions go straight from samples to
/// atoms, with no rounding to milliseconds on the way.
fn plan_redactions(segments: &[Segment], format: &WaveFormat, atoms: u64) -> (RedactionPlan, Vec<RedactionRecord>) {
    let atom_at = |samples: u64, round_up: bool| -> u64 {
        format.atom_at_sample(samples, round_up).min(u128::from(atoms)) as u64
    };
    let rate = f64::from(format.sample_rate.max(1));
    let duration_secs = format.atoms_secs(atoms);
    let mut ranges = Vec::new();
    let mut records = Vec::new();
    for segment in segments {
        let start = atom_at(segment.start, false);
        let end = segment.end.map_or(atoms, |end| atom_at(end, true));
        let applied = start < end;
        if applied && segment.action != RedactionAction::Play {
            ranges.push((start, end, segment.action));
        }
        let start_secs = segment.start as f64 / rate;
        let end_secs = segment.end.map_or(duration_secs, |end| end as f64 / rate);
        records.push(RedactionRecord {
            start_secs,
            end_secs: end_secs.max(start_secs),
            action: segment.action,
            applied,
        });
    }
    // Neighbouring segments can share a partly covered atom: the stricter
    // action takes it.
    let plan = RedactionPlan {
        ranges: strictest_cover(&ranges),
    };
    (plan, records)
}

/// A Windows FILETIME (100 ns ticks since 1601-01-01) as
/// `YYYY-MM-DDTHH:MM:SS.mmm`, with no zone.
fn filetime_text(filetime: u64) -> Option<String> {
    const TICKS_PER_SECOND: u64 = 10_000_000;
    const UNIX_EPOCH_SECONDS: i64 = 11_644_473_600;
    if filetime == 0 {
        return None;
    }
    let seconds = i64::try_from(filetime / TICKS_PER_SECOND).ok()? - UNIX_EPOCH_SECONDS;
    let millis = (filetime % TICKS_PER_SECOND) / 10_000;
    let moment = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{millis:03}",
        moment.year(),
        u8::from(moment.month()),
        moment.day(),
        moment.hour(),
        moment.minute(),
        moment.second()
    ))
}

/// The first session start in the `rinf` chunk.
fn parse_start_time(bytes: &[u8]) -> Option<StartTime> {
    let text = decode_xml(bytes);
    let stored = filetime_text(split_u64(&text, "STARTREC", "FTLow", "FTHigh")?)?;
    Some(StartTime {
        stored,
        source: START_TIME_SOURCE,
        basis: START_TIME_BASIS,
    })
}

fn time_zone(minutes: i32) -> TimeZone {
    let sign = if minutes < 0 { '-' } else { '+' };
    TimeZone {
        local_minus_utc_minutes: minutes,
        display: format!("GMT{sign}{}:{:02}", minutes.abs() / 60, minutes.abs() % 60),
        source: TIME_ZONE_SOURCE,
    }
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
        parse_meta(
            &read_payload(&mut file, meta, MAX_META_BYTES, "META description")?,
            container.version,
        )?
    } else {
        read_legacy_description(&mut file, &container)?
    };
    if description.mask >= (1u64 << MAX_CHANNEL_BITS) {
        return Err(damaged("its channel mask uses more than 32 channels"));
    }
    let format = WaveFormat::parse(&description.format)?;
    let data = container
        .first(b"data")
        .ok_or_else(|| damaged("its audio data chunk is missing"))?;
    let (data_offset, data_len) = payload_range(&mut file, data)?;
    let checksum = verify_checksums(&mut file, file_len, container.version, data, cancelled)?;
    // Footer chunks are optional: a missing one is simply not there.
    let bookmarks = match container.first(b"mark") {
        Some(mark) => parse_bookmarks(&read_payload(&mut file, mark, MAX_MARK_BYTES, "bookmark section")?),
        None => Some((Vec::new(), 0)),
    };
    let start_time = match container.find(b"rinf", 0) {
        Some(rinf) => parse_start_time(&read_payload(&mut file, rinf, MAX_META_BYTES, "recording information")?),
        None => None,
    };
    let protection_chunk = container.entries.iter().any(|entry| &entry.tag == b"prot");
    let mut layout = Layout {
        format,
        data_offset,
        data_len,
        initial_mask: description.mask,
        output_bits: Vec::new(),
        plan: RedactionPlan::default(),
        problem: None,
    };
    let stats = scan_data(&mut file, &layout, cancelled, &mut |_, _, _| Ok(()))?;
    let mark_unreadable = bookmarks.is_none();
    let (bookmarks, unreadable) = bookmarks.unwrap_or_default();
    let redactions = pair_redactions(&bookmarks);
    let (plan, redaction_records) = plan_redactions(&redactions.segments, &layout.format, stats.atoms);
    layout.plan = plan;
    // Only the first data chunk is read. How a recording with several
    // (for example after a pause) joins them is not known, so it is refused
    // rather than converted short.
    // The id of a repeated chunk is a stream number; how the recorder joins
    // several bookmark sections is not known either, and any of them could
    // hold a redaction, so that is refused too. An unreadable bookmark could
    // be the start of a redaction.
    let data_chunks = container.count(b"data");
    let mark_chunks = container.count(b"mark");
    layout.problem = if data_chunks > 1 {
        Some(several_data_chunks(data_chunks))
    } else if stats.problem.is_some() {
        stats.problem.clone()
    } else if mark_chunks > 1 {
        Some(several_mark_chunks(mark_chunks))
    } else if mark_unreadable {
        Some(MARK_UNREADABLE_MESSAGE.into())
    } else if unreadable > 0 {
        Some(BOOKMARK_UNREADABLE_MESSAGE.into())
    } else if redactions.unplaced {
        Some(REDACTION_POSITION_MESSAGE.into())
    } else {
        None
    };
    layout.output_bits = (0..MAX_CHANNEL_BITS)
        .filter(|bit| stats.union_mask & (1 << bit) != 0)
        .collect();
    let channel_names = channel_names(&layout.output_bits, description.mask, &description.names);
    let output_atoms = stats.atoms - layout.plan.atoms_with(RedactionAction::Skip);
    let format = &layout.format;
    let seconds = |atoms: u64| format.atoms_secs(atoms);
    Ok(DcrInfo {
        version: container.version,
        codec_tag: format.tag,
        codec: format.codec_name(),
        sample_rate: format.sample_rate,
        channels: layout.output_bits.len() as u32,
        channel_names,
        atoms: stats.atoms,
        duration_secs: seconds(stats.atoms),
        output_atoms,
        output_duration_secs: seconds(output_atoms),
        video_frames: stats.video_frames,
        attachments: stats.attachments,
        attachments_deleted: stats.attachments_deleted,
        attachments_encrypted: stats.attachments_encrypted,
        conference_changes: stats.changes,
        conference_change_count: stats.change_count,
        sealed_sections: stats.sealed_sections,
        sealed_atoms: stats.sealed_atoms,
        password_set: description.password_set,
        protection_chunk,
        trailing_bytes_dropped: stats.trailing_bytes_dropped,
        checksum,
        bookmarks: bookmark_counts(&bookmarks, unreadable),
        redactions: redaction_records,
        unpaired_redaction_ends: redactions.unpaired_ends,
        distort_as_silence: redactions.distort,
        start_time,
        time_zone: description.time_zone_minutes.map(time_zone),
        layout,
    })
}

/// File identity for the inspect cache: path, size and modification time.
type InspectKey = (PathBuf, u64, Option<std::time::SystemTime>);

/// Recently inspected recordings, so queueing and probing do not each walk
/// the whole recording again. Only for what the queue shows: a feed is
/// always prepared from a fresh `inspect` (see `prepare_feed`), because a
/// file can change and keep its size and modification time.
fn inspect_cache() -> &'static std::sync::Mutex<Vec<(InspectKey, DcrInfo)>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<(InspectKey, DcrInfo)>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

const INSPECT_CACHE_ENTRIES: usize = 8;

fn inspect_key(path: &Path) -> Option<InspectKey> {
    let metadata = fs::metadata(path).ok()?;
    Some((path.to_path_buf(), metadata.len(), metadata.modified().ok()))
}

/// `inspect`, reusing the result for a file whose path, size and
/// modification time have not changed since it was last inspected.
pub(crate) fn inspect_cached(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<DcrInfo, String> {
    let key = inspect_key(path);
    if let Some(key) = &key {
        let cache = inspect_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, info)) = cache.iter().find(|(cached, _)| cached == key) {
            return Ok(info.clone());
        }
    }
    inspect_and_remember(path, key, cancelled)
}

/// `inspect`, always reading the file, and remember the result for the
/// queue when the file did not change while it was read.
fn inspect_and_remember(path: &Path, key: Option<InspectKey>, cancelled: &dyn Fn() -> bool) -> Result<DcrInfo, String> {
    let info = inspect(path, cancelled)?;
    if let Some(key) = key.filter(|key| inspect_key(path).as_ref() == Some(key)) {
        let mut cache = inspect_cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.retain(|(cached, _)| cached.0 != key.0);
        if cache.len() >= INSPECT_CACHE_ENTRIES {
            cache.remove(0);
        }
        cache.push((key, info.clone()));
    }
    Ok(info)
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
    let (streams, duration_secs, dcr) = match inspect_cached(path, &|| false) {
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
            // The output's length: left-out redactions shorten it.
            let duration = queue.problem.is_none().then_some(info.output_duration_secs);
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
                trailing_bytes_dropped: 0,
                checksum: String::new(),
                notices: Vec::new(),
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

/// Decides each atom's fate in walk order: left out (a redaction marked
/// skip), silent (a blacked-out redaction, or a password-protected section)
/// or kept.
struct FateCursor<'a> {
    plan: &'a RedactionPlan,
    next_atom: u64,
    range: usize,
}

impl<'a> FateCursor<'a> {
    fn new(plan: &'a RedactionPlan) -> Self {
        Self {
            plan,
            next_atom: 0,
            range: 0,
        }
    }

    fn next(&mut self, sealed: bool) -> AtomFate {
        let atom = self.next_atom;
        self.next_atom += 1;
        match self.plan.action_at(atom, &mut self.range) {
            Some(RedactionAction::Skip) => AtomFate::Drop,
            Some(RedactionAction::Black) => AtomFate::Silence,
            _ if sealed => AtomFate::Silence,
            _ => AtomFate::Keep,
        }
    }
}

/// One multichannel WAV whose frames are the atoms' blocks, in output-bit
/// order, with silence for a channel that is not active at that point and
/// for silenced atoms; left-out atoms are not written.
fn write_interleaved(src: &Path, info: &DcrInfo, cancelled: &dyn Fn() -> bool) -> Result<TempFile, String> {
    let layout = &info.layout;
    let format = &layout.format;
    let channels = layout.output_bits.len() as u64;
    let block = usize::from(format.block_align);
    let data_len = info.output_atoms * channels * block as u64;
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
        Some(info.output_atoms)
    };
    let (guard, mut writer) = create_temp_wav()?;
    writer
        .write_all(&wav_header(&fmt, fact, data_len))
        .map_err(write_error)?;
    let silence = format.silent_block();
    let bits = layout.output_bits.clone();
    let mut fates = FateCursor::new(&layout.plan);
    let mut written = 0u64;
    let mut file = File::open(src).map_err(|error| format!("Could not open the .dcr recording: {error}"))?;
    scan_data(&mut file, layout, cancelled, &mut |active, atom, sealed| {
        let fate = fates.next(sealed);
        if fate == AtomFate::Drop {
            return Ok(());
        }
        let mut next = 0;
        for bit in &bits {
            if active & (1 << bit) != 0 {
                if fate == AtomFate::Keep {
                    writer
                        .write_all(&atom[next * block..(next + 1) * block])
                        .map_err(write_error)?;
                } else {
                    writer.write_all(&silence).map_err(write_error)?;
                }
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
/// WAVEFORMATEX as stored, `data` that channel's blocks in order (silent
/// and left-out atoms as in `write_interleaved`).
fn write_mono_channels(src: &Path, info: &DcrInfo, cancelled: &dyn Fn() -> bool) -> Result<Vec<TempFile>, String> {
    let layout = &info.layout;
    let format = &layout.format;
    let block = usize::from(format.block_align);
    let data_len = info.output_atoms * block as u64;
    let fact = format
        .samples_per_block()
        .and_then(|per_block| info.output_atoms.checked_mul(per_block));
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
    let mut fates = FateCursor::new(&layout.plan);
    let mut atoms = 0u64;
    let mut file = File::open(src).map_err(|error| format!("Could not open the .dcr recording: {error}"))?;
    scan_data(&mut file, layout, cancelled, &mut |active, atom, sealed| {
        let fate = fates.next(sealed);
        if fate == AtomFate::Drop {
            return Ok(());
        }
        let mut next = 0;
        for (bit, writer) in bits.iter().zip(writers.iter_mut()) {
            if active & (1 << bit) != 0 {
                if fate == AtomFate::Keep {
                    writer
                        .write_all(&atom[next * block..(next + 1) * block])
                        .map_err(write_error)?;
                } else {
                    writer.write_all(&silence).map_err(write_error)?;
                }
                next += 1;
            } else {
                writer.write_all(&silence).map_err(write_error)?;
            }
        }
        atoms += 1;
        Ok(())
    })?;
    if atoms != info.output_atoms {
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

/// Clear the speaker-position mask FFmpeg's WAV muxer writes into a merged
/// feed (`amerge` guesses a surround layout such as 4.0 for N mono inputs).
/// A court recording's channels are microphones, not speakers, so the feed
/// says "no positions" (dwChannelMask 0) and conversion reads it with
/// `-guess_layout_max 0`. Only DepoAudio's own temporary feed is changed.
pub(crate) fn clear_channel_mask(path: &Path) -> Result<(), String> {
    let error = |error: io::Error| format!("Could not finish the temporary audio: {error}");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(error)?;
    let mut header = vec![0u8; 4096];
    let filled = file.read(&mut header).map_err(error)?;
    header.truncate(filled);
    if header.len() < 12 || !matches!(&header[..4], b"RIFF" | b"RF64") || &header[8..12] != b"WAVE" {
        return Ok(());
    }
    let mut at = 12;
    while at + 8 <= header.len() {
        let size = le_u32(&header, at + 4) as usize;
        if &header[at..at + 4] == b"fmt " {
            let body = at + 8;
            let extensible = size >= 40 && body + 24 <= header.len() && le_u16(&header, body) == 0xFFFE;
            if extensible && le_u32(&header, body + 20) != 0 {
                file.seek(SeekFrom::Start((body + 20) as u64)).map_err(error)?;
                file.write_all(&0u32.to_le_bytes()).map_err(error)?;
                file.sync_all().map_err(error)?;
            }
            return Ok(());
        }
        if &header[at..at + 4] == b"data" {
            return Ok(());
        }
        at += 8 + size + size % 2;
    }
    Ok(())
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
/// demux the audio into temporary WAVs. Blocking file I/O. The plan
/// (redactions, sealed audio, channels) always comes from reading the file
/// now, never from the queue's cache: a file replaced by one of the same
/// size with its modification time kept would otherwise get the old plan.
pub(crate) fn prepare_feed(src: &Path, cancelled: &dyn Fn() -> bool) -> Result<DcrFeed, String> {
    let info = inspect_and_remember(src, inspect_key(src), cancelled)?;
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
    let estimate = crate::helpers::wav_estimated_bytes(
        Some(info.output_duration_secs),
        Some(info.channels),
        Some(info.sample_rate),
    );
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
        /// Chunks written after `data` (besides `mark` and `rinf`).
        pub footer_chunks: Vec<([u8; 4], u32, Vec<u8>)>,
        /// Bookmark items for the `mark` chunk (written when not empty).
        pub bookmarks: Vec<String>,
        /// A whole `mark` payload, in place of `bookmarks`.
        pub mark_payload: Option<Vec<u8>>,
        /// `rinf` session start (FILETIME), when set.
        pub start_rec: Option<u64>,
        pub time_zone: &'static str,
        /// Case fields that must never leave the reader.
        pub docket: &'static str,
        /// Write the `info` chunk (a footer chunk DepoAudio does not need).
        pub info: bool,
        /// The ids of the `data` and `mark` chunks (0 unless a test sets them).
        pub data_id: u32,
        pub mark_id: u32,
    }

    pub(crate) const SECRET_DOCKET: &str = "SECRET-DOCKET-4471 Smith v. Jones";
    /// Registration and participant text that must never leave the reader.
    pub(crate) const SECRET_USER: &str = "SECRET-USERREG Clerk Jane Roe";
    pub(crate) const SECRET_PARTICIPANT: &str = "Remote Witness Secret-Participant";
    /// Bookmark notes that must never leave the reader.
    pub(crate) const SECRET_NOTE: &str = "SECRET-NOTE minor's home address";
    pub(crate) const SECRET_PRIVATE: &str = "SECRET-PRIVNOTE sidebar";
    pub(crate) const SECRET_SPEAKER: &str = "SECRET-SPEAKER Dr Roe";
    /// Every secret the builder writes, for leak checks.
    pub(crate) const SECRETS: [&str; 11] = [
        "SECRET-DOCKET",
        "Smith v. Jones",
        "hunter2",
        "DOCKNOTA",
        "SECRET-USERREG",
        "Clerk Jane",
        "Secret-Participant",
        "SECRET-NOTE",
        "SECRET-PRIVNOTE",
        "SECRET-SPEAKER",
        "Begin Redaction",
    ];

    /// A bookmark item's text: `IDMARK` written with `%i` (as a signed
    /// 32-bit value), an optional position, and secret notes.
    pub(crate) fn bookmark_item(kind: u32, id: u32, position: Option<u64>, note: &str) -> String {
        let idmark = ((kind << 28) | id) as i32;
        let position = position.unwrap_or(u64::MAX);
        format!(
            "<ITEM IDMARK=\"{idmark}\" COLOR=\"0\" FLAGMARK=\"0\"><POSMARK N64Low=\"{}\" N64High=\"{}\"/><PUBLNOTE><![CDATA[{note}]]></PUBLNOTE><PRIVNOTE><![CDATA[{SECRET_PRIVATE}]]></PRIVNOTE><SPEAKER><![CDATA[{SECRET_SPEAKER}]]></SPEAKER></ITEM>",
            position as u32 as i32,
            (position >> 32) as u32 as i32
        )
    }

    impl DcrBuilder {
        pub(crate) fn new(format: Vec<u8>, mask: u64, names: &[&str]) -> Self {
            Self {
                major: 27,
                minor: 0,
                format,
                mask,
                names: names.iter().map(|name| name.to_string()).collect(),
                have_rec: "0",
                data: Vec::new(),
                extra_chunks: Vec::new(),
                footer_chunks: Vec::new(),
                bookmarks: Vec::new(),
                mark_payload: None,
                start_rec: None,
                time_zone: "-300",
                docket: SECRET_DOCKET,
                info: true,
                data_id: 0,
                mark_id: 0,
            }
        }

        /// Version 20.05: `wfmt` and `chnl` in place of `meta`, and the
        /// `crpt` chunk those versions must have (no password set).
        pub(crate) fn legacy(mut self) -> Self {
            self.major = 20;
            self.minor = 5;
            self.extra_chunks.push((*b"crpt", 0, vec![0; 8]));
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

        /// A participant record whose `HD` text carries `ac` (and a name
        /// that must never leave the reader).
        pub(crate) fn participant(&mut self, start: bool, ac: &str) -> &mut Self {
            let text = format!("<HD id=\"7\" ac=\"{ac}\" fn=\"{SECRET_PARTICIPANT}\"/>");
            self.participant_text(start, text.as_bytes())
        }

        pub(crate) fn participant_text(&mut self, start: bool, text: &[u8]) -> &mut Self {
            self.data
                .extend_from_slice(if start { b"ptch#str" } else { b"ptch#stp" });
            self.data.extend_from_slice(&(text.len() as u32).to_le_bytes());
            self.data.extend_from_slice(text);
            self
        }

        /// A Begin Redaction bookmark at `position` samples per channel with
        /// `tag` (for example `<play_black>`; "" for none) after its note.
        pub(crate) fn begin_redaction(&mut self, position: Option<u64>, tag: &str) -> &mut Self {
            let id = self.bookmarks.len() as u32 + 1;
            let note = format!("Begin Redaction {SECRET_NOTE} {tag}");
            self.bookmarks
                .push(bookmark_item(BOOKMARK_BEGIN_REDACTION, id, position, &note));
            self
        }

        pub(crate) fn end_redaction(&mut self, position: Option<u64>) -> &mut Self {
            let id = self.bookmarks.len() as u32 + 1;
            self.bookmarks
                .push(bookmark_item(BOOKMARK_END_REDACTION, id, position, "End Redaction"));
            self
        }

        fn meta_xml(&self) -> Vec<u8> {
            let hex: String = self.format.iter().map(|byte| format!("{byte:02x}")).collect();
            let items: String = self
                .names
                .iter()
                .map(|name| format!("<ITEM><CHNAME><![CDATA[{name}]]></CHNAME><CHDESC/><CHROLE/></ITEM>"))
                .collect();
            let zero_blob = "00".repeat(64);
            let mut xml = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?><Root><META><AUDIO USECHNL=\"{}\"><FMTNAME><![CDATA[Synthetic]]></FMTNAME><FMTDATA><![CDATA[{hex}]]></FMTDATA><LISTCHNL>{items}</LISTCHNL></AUDIO><PROP TIMEZONE=\"{}\" VERAPP=\"0\"><USERREG><![CDATA[{SECRET_USER}]]></USERREG><MACHINE><![CDATA[Court of Synthetic Appeals]]></MACHINE></PROP><DOCKNOTA><TCASENO><![CDATA[{}]]></TCASENO></DOCKNOTA><CRYPT HAVEREC=\"{}\" HAVEOVE=\"0\"><CHECKREC><![CDATA[{zero_blob}]]></CHECKREC><PASSREC><![CDATA[hunter2-{zero_blob}]]></PASSREC></CRYPT></META></Root>",
                self.mask as u32 as i32, self.time_zone, self.docket, self.have_rec
            )
            .into_bytes();
            // A stray control character, which the reader drops.
            let root = xml.windows(6).position(|window| window == b"<Root>").unwrap();
            xml.insert(root, 0x01);
            xml
        }

        fn mark_xml(&self) -> Option<Vec<u8>> {
            if let Some(payload) = &self.mark_payload {
                return Some(payload.clone());
            }
            (!self.bookmarks.is_empty()).then(|| {
                format!(
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?><Root><BOOK EDITVER=\"0\">{}</BOOK></Root>",
                    self.bookmarks.concat()
                )
                .into_bytes()
            })
        }

        /// Chunks in file order, and the index of `data` among them.
        pub(crate) fn chunks(&self) -> (Vec<([u8; 4], u32, Vec<u8>)>, usize) {
            let mut chunks = Vec::new();
            if self.major * 100 + self.minor >= META_VERSION {
                chunks.push((*b"meta", 0, self.meta_xml()));
            } else {
                chunks.push((*b"wfmt", 0, self.format.clone()));
                let mut chnl = (self.mask as u32).to_le_bytes().to_vec();
                chnl.extend_from_slice(&[0; 8]);
                chunks.push((*b"chnl", 0, chnl));
            }
            chunks.extend(self.extra_chunks.iter().cloned());
            let data_index = chunks.len();
            chunks.push((*b"data", self.data_id, self.data.clone()));
            if self.info {
                chunks.push((*b"info", 0, b"synthetic info".to_vec()));
            }
            if let Some(mark) = self.mark_xml() {
                chunks.push((*b"mark", self.mark_id, mark));
            }
            if let Some(filetime) = self.start_rec {
                chunks.push((
                    *b"rinf",
                    0,
                    format!(
                        "<?xml version=\"1.0\" encoding=\"utf-8\"?><Root><RINF><LISTSESS><RECSESS><STARTREC FTLow=\"{}\" FTHigh=\"{}\"/><STOPREC FTLow=\"0\" FTHigh=\"0\"/></RECSESS></LISTSESS></RINF></Root>",
                        filetime as u32 as i32,
                        (filetime >> 32) as u32 as i32
                    )
                    .into_bytes(),
                ));
            }
            chunks.extend(self.footer_chunks.iter().cloned());
            (chunks, data_index)
        }

        pub(crate) fn bytes(&self) -> Vec<u8> {
            let mut out = DCR_MAGIC.to_vec();
            out.extend_from_slice(&self.major.to_le_bytes());
            out.extend_from_slice(&self.minor.to_le_bytes());
            let mut directory = Vec::new();
            let (chunks, data_index) = self.chunks();
            let mut data_range = (0, 0);
            for (index, (tag, id, payload)) in chunks.into_iter().enumerate() {
                let offset = out.len();
                out.extend_from_slice(&tag);
                out.extend_from_slice(b"#bgn");
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&payload);
                out.extend_from_slice(&tag);
                out.extend_from_slice(b"#end");
                if index == data_index {
                    data_range = (offset, out.len());
                }
                directory.push((tag, id, offset as u64, payload.len() as u64 + 20));
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
                // Correct checksums, as the recorder writes them.
                let before = checksum(&out[..data_range.0]);
                out.extend_from_slice(&before.to_le_bytes());
                out.extend_from_slice(&checksum(EMPTY_DATA_CHUNK).to_le_bytes());
                let after = checksum(&out[data_range.1..]);
                out.extend_from_slice(&after.to_le_bytes());
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

    fn i16s(data: &[u8]) -> Vec<i16> {
        data.chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect()
    }

    pub(crate) fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/fixtures/dcr/vendor-notes-samples")
            .join(name)
    }

    pub(crate) const PLAIN_SAMPLE: &str = "test_tone_2ch_8k_pcm16.dcr";
    pub(crate) const REDACTED_SAMPLE: &str = "test_tone_redacted_4-6s.dcr";

    #[test]
    fn inspects_a_four_channel_pcm_recording_with_non_contiguous_bits() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1011, 8000, 800, &[(300.0, 0.5), (500.0, 0.25), (700.0, 0.125)]);
        builder.names = ["Judge", "Witness", "Unused", "Counsel"].map(String::from).to_vec();
        let path = builder.write(&dir.0, "hearing.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.version, 2700);
        assert_eq!(info.codec_tag, 1);
        assert_eq!(info.codec, "PCM 16-bit");
        assert_eq!(info.sample_rate, 8000);
        assert_eq!(info.channels, 3);
        // Four items for bits 0..3: item N names bit N.
        assert_eq!(info.channel_names, ["Judge", "Witness", "Counsel"]);
        assert_eq!(info.atoms, 800);
        assert_eq!(info.output_atoms, 800);
        assert!((info.duration_secs - 0.1).abs() < 1e-9);
        assert_eq!(info.checksum, CHECKSUMS_MATCH);
        assert_eq!(info.time_zone.as_ref().unwrap().display, "GMT-5:00");
        assert_eq!(info.refusal(), None);
        assert!(info.notices().is_empty(), "{:?}", info.notices());
    }

    // ── The owner's two synthetic samples ───────────────────────────────────

    #[test]
    fn the_plain_sample_reads_as_twelve_seconds_of_judge_and_witness() {
        let path = fixture(PLAIN_SAMPLE);
        let before = fs::read(&path).unwrap();
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.version, 2700);
        assert_eq!(info.codec, "PCM 16-bit");
        assert_eq!(info.sample_rate, 8000);
        assert_eq!(info.channel_names, ["Judge", "Witness"]);
        assert_eq!(info.atoms, 96_000);
        assert!((info.duration_secs - 12.0).abs() < 1e-9);
        assert_eq!(info.checksum, CHECKSUMS_MATCH);
        assert_eq!(info.bookmarks.total, 0, "its mark chunk is empty");
        assert!(info.redactions.is_empty());
        assert_eq!(info.time_zone.as_ref().unwrap().display, "GMT+0:00");
        assert_eq!(info.start_time, None, "it has no rinf chunk");
        assert_eq!(info.refusal(), None);
        let feed = prepare_feed(&path, &never).unwrap();
        let (fmt, data) = read_wav_data(&feed.path);
        assert_eq!(le_u16(&fmt, 2), 2);
        assert_eq!(le_u32(&fmt, 4), 8000);
        assert_eq!(data.len(), 96_000 * 4, "12.000 s of two 16-bit channels");
        // The data chunk starts: atom 0 is (0, 0), atom 1 (0, 1998), atom 2
        // (1, 3840).
        assert_eq!(&i16s(&data[..12]), &[0, 0, 0, 1998, 1, 3840]);
        assert_eq!(fs::read(&path).unwrap(), before, "the original is never modified");
    }

    #[test]
    fn the_samples_stored_checksums_are_the_weighted_sums() {
        for (name, after) in [(PLAIN_SAMPLE, 0x9BAD_4C6A_u32), (REDACTED_SAMPLE, 0xEAE2_BE59)] {
            let bytes = fs::read(fixture(name)).unwrap();
            let tail = &bytes[bytes.len() - 12..];
            assert_eq!(le_u32(tail, 0), 0x48D1_953D, "{name}");
            assert_eq!(le_u32(tail, 4), 0x5EF5_6404, "{name}");
            assert_eq!(le_u32(tail, 8), after, "{name}");
            // The data chunk sits at 1221 and is 384028 bytes long in both.
            assert_eq!(checksum(&bytes[..1221]), 0x48D1_953D, "{name}");
            assert_eq!(checksum(&bytes[1221 + 384_028..bytes.len() - 4]), after, "{name}");
        }
    }

    #[test]
    fn the_redacted_sample_is_silent_from_four_to_six_seconds_and_otherwise_the_plain_tone() {
        let plain = prepare_feed(&fixture(PLAIN_SAMPLE), &never).unwrap();
        let redacted_path = fixture(REDACTED_SAMPLE);
        let redacted = prepare_feed(&redacted_path, &never).unwrap();
        let info = &redacted.info;
        assert_eq!(info.checksum, CHECKSUMS_MATCH);
        assert_eq!(info.bookmarks.total, 2);
        assert_eq!(info.bookmarks.by_type, BTreeMap::from([(13, 1), (14, 1)]));
        assert_eq!(
            info.redactions,
            [RedactionRecord {
                start_secs: 4.0,
                end_secs: 6.0,
                action: RedactionAction::Black,
                applied: true,
            }]
        );
        assert_eq!(info.output_atoms, 96_000, "a blacked-out segment keeps its length");
        assert_eq!(
            info.notices(),
            ["1 redacted segment: 1 silenced, as marked in the recording."]
        );
        let (_, plain) = read_wav_data(&plain.path);
        let (_, redacted) = read_wav_data(&redacted.path);
        assert_eq!(plain.len(), redacted.len());
        let (plain, redacted) = (i16s(&plain), i16s(&redacted));
        // Frames are (Judge, Witness) pairs at 8 kHz: 4 s is frame 32000.
        for (frame, (a, b)) in plain.chunks_exact(2).zip(redacted.chunks_exact(2)).enumerate() {
            if (32_000..48_000).contains(&frame) {
                assert_eq!(b, [0, 0], "frame {frame} is not silent");
            } else {
                assert_eq!(a, b, "frame {frame} differs from the plain tone");
            }
        }
        assert!(
            plain[64_000..96_000].iter().any(|sample| *sample != 0),
            "the plain tone has audio there"
        );
        let record = serde_json::to_value(info.record()).unwrap();
        assert_eq!(
            record["redactions"],
            serde_json::json!([{ "startSecs": 4.0, "endSecs": 6.0, "action": "black", "applied": true }])
        );
        assert!(record.get("timing").is_none(), "nothing was left out");
        let text = record.to_string();
        assert!(!text.contains("Redaction") && !text.contains("play_black"), "{text}");
    }

    // ── Checksums ───────────────────────────────────────────────────────────

    #[test]
    fn checksum_matches_the_worked_examples() {
        let ramp: Vec<u8> = (0..0x40).collect();
        assert_eq!(checksum(&ramp), 0x857C_7D0C);
        assert_eq!(checksum(EMPTY_DATA_CHUNK), 0x5EF5_6404);
        assert_eq!(checksum(&[]), 0);
        // Summing in pieces gives the same value as summing at once.
        let split = checksum_update(checksum_update(0, 0, &ramp[..23]), 23, &ramp[23..]);
        assert_eq!(split, 0x857C_7D0C);
    }

    #[test]
    fn a_checksum_mismatch_is_reported_but_the_recording_is_still_read() {
        let dir = TestDir::new();
        let mut bytes = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]).bytes();
        // Change one byte of the `info` payload (after the data chunk).
        let at = bytes.windows(8).position(|window| window == b"info#bgn").unwrap() + 12;
        bytes[at] ^= 0x20;
        let path = dir.0.join("changed.dcr");
        fs::write(&path, &bytes).unwrap();
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.checksum, CHECKSUMS_DIFFER);
        assert_eq!(info.refusal(), None);
        assert!(info.notices().contains(&CHECKSUM_WARNING.to_string()));
        assert_eq!(info.record().checksum, "checksums do not match");
        assert!(prepare_feed(&path, &never).is_ok());

        let legacy = pcm_tones(0b1, 8000, 4, &[(300.0, 0.5)])
            .legacy()
            .write(&dir.0, "old.dcr");
        assert_eq!(inspect(&legacy, &never).unwrap().checksum, CHECKSUMS_ABSENT);
    }

    #[test]
    fn missing_footer_chunks_are_fine() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]);
        builder.info = false;
        let path = builder.write(&dir.0, "minimal.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.atoms, 40);
        assert_eq!(info.checksum, CHECKSUMS_MATCH);
        assert_eq!(info.refusal(), None);
        assert!(info.notices().is_empty());
    }

    // ── Redactions ──────────────────────────────────────────────────────────

    fn begin(position: Option<u64>, action: MarkedAction) -> Bookmark {
        Bookmark {
            kind: BOOKMARK_BEGIN_REDACTION,
            position,
            action: Some(action),
        }
    }

    fn end(position: Option<u64>) -> Bookmark {
        Bookmark {
            kind: BOOKMARK_END_REDACTION,
            position,
            action: None,
        }
    }

    /// A segment in samples per channel.
    fn segment(start: u64, end: Option<u64>, action: RedactionAction) -> Segment {
        Segment { start, end, action }
    }

    #[test]
    fn redaction_notes_give_the_action_and_skip_is_the_default() {
        use MarkedAction::*;
        assert_eq!(marked_action("Begin Redaction <play_play>"), Play);
        assert_eq!(marked_action("Begin Redaction <play_skip>"), Skip);
        assert_eq!(marked_action("<PLAY_OMIT>"), Skip);
        assert_eq!(marked_action("Begin Redaction <play_black>"), Black);
        assert_eq!(marked_action("x <play_empty>"), Black);
        assert_eq!(marked_action("<play_dist><play_dist_prm a=1>"), Distort);
        assert_eq!(marked_action("Begin Redaction"), Skip);
        assert_eq!(marked_action(""), Skip);
        // Several tags: the strictest.
        assert_eq!(marked_action("<play_play> <play_black>"), Black);
        assert_eq!(MarkedAction::Distort.applied(), RedactionAction::Black);
    }

    #[test]
    fn begins_pair_with_the_next_end_and_overlaps_join_with_the_strictest_action() {
        use MarkedAction::*;
        use RedactionAction as A;
        // 8000 samples per second: 8000 samples = 1000 ms.
        let paired = pair_redactions(&[
            end(Some(800)),
            begin(Some(16_000), Black),
            begin(Some(8000), Play),
            end(Some(24_000)),
            end(Some(32_000)),
            begin(Some(40_000), Skip),
        ]);
        // The end at 0.1 s has no begin before it; begin 1 s (play) takes
        // the end at 3 s, begin 2 s (black) the end at 4 s, and begin 5 s
        // (skip) has no end left, so it runs to the end of the recording.
        assert_eq!(paired.unpaired_ends, 1);
        assert!(!paired.unplaced);
        assert_eq!(
            paired.segments,
            [
                segment(8000, Some(16_000), A::Play),
                segment(16_000, Some(32_000), A::Black),
                segment(40_000, None, A::Skip),
            ]
        );

        // Overlapping segments: union, skip beats black beats play.
        assert_eq!(
            join_segments(&[
                segment(0, Some(3000), A::Play),
                segment(1000, Some(2000), A::Skip),
                segment(1500, Some(4000), A::Black),
            ]),
            [
                segment(0, Some(1000), A::Play),
                segment(1000, Some(2000), A::Skip),
                segment(2000, Some(4000), A::Black),
            ]
        );
        assert_eq!(
            join_segments(&[segment(0, Some(2000), A::Black), segment(2000, Some(3000), A::Black)]),
            [segment(0, Some(3000), A::Black)]
        );

        // A begin with no position cannot be placed; an end with none is
        // ignored.
        let unplaced = pair_redactions(&[begin(None, Black), end(None)]);
        assert!(unplaced.unplaced);
        assert_eq!(unplaced.unpaired_ends, 1);
        assert_eq!(pair_redactions(&[begin(Some(0), Distort)]).distort, 1);
    }

    #[test]
    fn redaction_atoms_cover_every_partly_covered_block() {
        // IMA ADPCM: 256-byte blocks of 505 samples at 8 kHz, so one block
        // lasts 63.125 ms. Positions are in samples (8 per millisecond).
        let format = WaveFormat::parse(&waveformatex(
            WAVE_FORMAT_IMA_ADPCM,
            8000,
            4055,
            256,
            4,
            &505u16.to_le_bytes(),
        ))
        .unwrap();
        let (plan, records) = plan_redactions(&[segment(800, Some(1600), RedactionAction::Skip)], &format, 1000);
        // 100 ms is inside block 1 (63-126 ms); 200 ms inside block 3.
        assert_eq!(plan.ranges, [(1, 4, RedactionAction::Skip)]);
        assert!(records[0].applied);
        // Two segments meeting inside block 1: the stricter action takes it.
        let (plan, _) = plan_redactions(
            &[
                segment(0, Some(800), RedactionAction::Black),
                segment(800, Some(1600), RedactionAction::Skip),
            ],
            &format,
            1000,
        );
        assert_eq!(
            plan.ranges,
            [(0, 1, RedactionAction::Black), (1, 4, RedactionAction::Skip)]
        );
        assert_eq!(plan.atoms_with(RedactionAction::Skip), 3);
        // Past the end of the audio: listed, not applied.
        let (plan, records) = plan_redactions(&[segment(720_000_000_000, None, RedactionAction::Black)], &format, 1000);
        assert!(plan.ranges.is_empty());
        assert!(!records[0].applied);
        // PCM: exact sample boundaries.
        let pcm = WaveFormat::parse(&pcm16_format(8000)).unwrap();
        let (plan, _) = plan_redactions(&[segment(32_000, Some(48_000), RedactionAction::Black)], &pcm, 96_000);
        assert_eq!(plan.ranges, [(32_000, 48_000, RedactionAction::Black)]);
        // Positions that are not whole milliseconds: every redacted sample
        // is covered (no rounding to the nearest millisecond on the way).
        let (plan, records) = plan_redactions(&[segment(32_005, Some(47_995), RedactionAction::Black)], &pcm, 96_000);
        assert_eq!(plan.ranges, [(32_005, 47_995, RedactionAction::Black)]);
        assert_eq!(records[0].start_secs, 32_005.0 / 8000.0);
        let (plan, _) = plan_redactions(&[segment(32_000, Some(48_003), RedactionAction::Skip)], &pcm, 96_000);
        assert_eq!(plan.ranges, [(32_000, 48_003, RedactionAction::Skip)]);
        // IMA: a position inside a block takes the whole block.
        let (plan, _) = plan_redactions(&[segment(506, Some(1011), RedactionAction::Black)], &format, 1000);
        assert_eq!(plan.ranges, [(1, 3, RedactionAction::Black)]);
    }

    #[test]
    fn ima_adpcm_times_come_from_samples_per_block_not_the_stored_byte_rate() {
        let dir = TestDir::new();
        // As FFmpeg's IMA ADPCM WAV encoder writes it at 8 kHz mono: 1024-byte
        // blocks of 2041 samples (0.2551 s each), with a stored byte rate of
        // 16000 that would make a block 0.064 s.
        let format = waveformatex(WAVE_FORMAT_IMA_ADPCM, 8000, 16_000, 1024, 4, &2041u16.to_le_bytes());
        let mut builder = DcrBuilder::new(format, 0b1, &["Judge"]);
        builder.marker(b"audi#mrk");
        for _ in 0..47 {
            builder.atom(&[&[0x11; 1024]]);
        }
        // A black redaction from 4 s to 6 s.
        builder
            .begin_redaction(Some(32_000), "<play_black>")
            .end_redaction(Some(48_000));
        let path = builder.write(&dir.0, "ima.dcr");
        let info = inspect(&path, &never).unwrap();
        let block = 2041.0 / 8000.0;
        assert!(
            (info.duration_secs - 47.0 * block).abs() < 1e-9,
            "{}",
            info.duration_secs
        );
        assert_eq!(info.refusal(), None);
        assert!(info.redactions[0].applied, "the redaction lies inside the recording");
        assert_eq!(info.redactions[0].start_secs, 4.0);
        assert_eq!(info.redactions[0].end_secs, 6.0);
        // Sample 32000 is in block 15 (30615-32656); sample 48000 in block 23.
        assert_eq!(info.layout.plan.ranges, [(15, 24, RedactionAction::Black)]);
        assert!((queue_streams_info(&path).duration_secs.unwrap() - 47.0 * block).abs() < 1e-9);
        let feed = prepare_feed(&path, &never).unwrap();
        let (_, data) = read_wav_data(&feed.path);
        let blocks: Vec<&[u8]> = data.chunks_exact(1024).collect();
        assert_eq!(blocks.len(), 47);
        for (index, block) in blocks.iter().enumerate() {
            let silent = block.iter().all(|byte| *byte == 0);
            assert_eq!(silent, (15..24).contains(&index), "block {index}");
        }

        // A participant change is timed the same way.
        let format = waveformatex(WAVE_FORMAT_IMA_ADPCM, 8000, 16_000, 1024, 4, &2041u16.to_le_bytes());
        let mut joined = DcrBuilder::new(format, 0b1, &[]);
        joined.marker(b"audi#mrk");
        for _ in 0..4 {
            joined.atom(&[&[0x11; 1024]]);
        }
        joined.participant(true, "2");
        joined.atom(&[&[0x11; 1024], &[0x22; 1024]]);
        let path = joined.write(&dir.0, "ima-join.dcr");
        let info = inspect(&path, &never).unwrap();
        assert!((info.conference_changes[0].at_seconds - 4.0 * block).abs() < 1e-9);

        // A samples-per-block count the block size cannot hold is refused.
        for declared in [2000u16, 2042] {
            let format = waveformatex(WAVE_FORMAT_IMA_ADPCM, 8000, 16_000, 1024, 4, &declared.to_le_bytes());
            let problem = WaveFormat::parse(&format).unwrap().problem().unwrap();
            assert!(problem.contains("samples-per-block"), "{problem}");
        }
        let gsm = waveformatex(WAVE_FORMAT_GSM610, 8000, 1625, 65, 0, &160u16.to_le_bytes());
        assert!(WaveFormat::parse(&gsm).unwrap().problem().is_some());
        // Microsoft's own header (505 samples in 256 bytes) is accepted.
        let acm = waveformatex(WAVE_FORMAT_IMA_ADPCM, 8000, 4055, 256, 4, &505u16.to_le_bytes());
        assert_eq!(WaveFormat::parse(&acm).unwrap().problem(), None);
        assert!((WaveFormat::parse(&acm).unwrap().atoms_secs(8000 * 60 * 60 / 505) - 3600.0).abs() < 0.07);
    }

    #[test]
    fn redactions_are_applied_by_default_skip_leaves_audio_out_and_black_silences_it() {
        let dir = TestDir::new();
        // Positions are kept in whole milliseconds: at 8 kHz, 8 samples.
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &["Judge", "Witness"]);
        builder.marker(b"audi#mrk");
        for index in 0..40i16 {
            builder.atom(&[&(index + 1).to_le_bytes(), &(1000 + index).to_le_bytes()]);
        }
        builder
            .begin_redaction(Some(8), "") // no tag: skip, 1-2 ms
            .end_redaction(Some(16))
            .begin_redaction(Some(24), "<play_black>") // 3-4 ms
            .end_redaction(Some(32))
            .begin_redaction(Some(32), "<play_play>") // 4-5 ms: kept
            .end_redaction(Some(40));
        let path = builder.write(&dir.0, "redacted.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        let info = &feed.info;
        assert_eq!(info.atoms, 40);
        assert_eq!(info.output_atoms, 32);
        assert_eq!(
            info.redactions.iter().map(|record| record.action).collect::<Vec<_>>(),
            [RedactionAction::Skip, RedactionAction::Black, RedactionAction::Play]
        );
        let (_, data) = read_wav_data(&feed.path);
        let samples = i16s(&data);
        let mut expected = Vec::new();
        for index in 0..40i16 {
            match index {
                8..16 => {}
                24..32 => expected.extend([0, 0]),
                _ => expected.extend([index + 1, 1000 + index]),
            }
        }
        assert_eq!(samples, expected);
        assert_eq!(
            info.notices()[..2],
            [
                "3 redacted segments: 1 left out, 1 silenced, 1 kept, as marked in the recording.".to_string(),
                "The output is 0.001 s shorter than the recording: times after a left-out segment come earlier in the output.".into(),
            ]
        );
        let record = serde_json::to_value(info.record()).unwrap();
        assert_eq!(record["redactions"][0]["startSecs"], 0.001);
        assert_eq!(record["redactions"][0]["endSecs"], 0.002);
        assert_eq!(record["redactions"][0]["action"], "skip");
        assert_eq!(record["timing"]["recordingSecs"], 0.005);
        assert_eq!(record["timing"]["outputSecs"], 0.004);
        assert_eq!(queue_streams_info(&path).duration_secs, Some(0.004));
    }

    #[test]
    fn the_queue_notice_counts_left_out_and_silenced_segments() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b11, 8000, 8000, &[(300.0, 0.5), (500.0, 0.25)]);
        builder
            .begin_redaction(Some(800), "<play_omit>")
            .end_redaction(Some(1600))
            .begin_redaction(Some(4000), "<play_empty>")
            .end_redaction(Some(4800))
            .end_redaction(Some(7000));
        let path = builder.write(&dir.0, "two.dcr");
        let info = inspect(&path, &never).unwrap();
        let notices = info.notices();
        assert_eq!(
            notices[0],
            "2 redacted segments: 1 left out, 1 silenced, as marked in the recording."
        );
        assert!(notices.contains(&"1 redaction end with no beginning before it is ignored.".to_string()));
        assert_eq!(info.record().unpaired_redaction_ends, 1);
    }

    #[test]
    fn an_unpaired_begin_redacts_to_the_end_and_skipping_everything_is_refused() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1, 8000, 800, &[(300.0, 0.5)]);
        builder.begin_redaction(Some(400), "<play_black>");
        let path = builder.write(&dir.0, "open.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        assert_eq!(feed.info.redactions[0].end_secs, 0.1);
        let (_, data) = read_wav_data(&feed.path);
        let samples = i16s(&data);
        assert!(samples[400..].iter().all(|sample| *sample == 0));
        assert!(samples[..400].iter().any(|sample| *sample != 0));

        let mut everything = pcm_tones(0b1, 8000, 80, &[(300.0, 0.5)]);
        everything.begin_redaction(Some(0), "");
        let path = everything.write(&dir.0, "all.dcr");
        assert_eq!(prepare_feed(&path, &never).err().as_deref(), Some(ALL_SKIPPED_MESSAGE));
    }

    #[test]
    fn an_unplaceable_redaction_or_an_unreadable_bookmark_section_is_refused() {
        let dir = TestDir::new();
        let mut unplaced = pcm_tones(0b1, 8000, 80, &[(300.0, 0.5)]);
        unplaced.begin_redaction(None, "<play_black>").end_redaction(Some(40));
        let path = unplaced.write(&dir.0, "unplaced.dcr");
        assert_eq!(
            prepare_feed(&path, &never).err().as_deref(),
            Some(REDACTION_POSITION_MESSAGE)
        );

        let mut garbage = pcm_tones(0b1, 8000, 80, &[(300.0, 0.5)]);
        garbage.mark_payload = Some(b"\x00\x01binary bookmark index".to_vec());
        let path = garbage.write(&dir.0, "garbage.dcr");
        assert_eq!(
            prepare_feed(&path, &never).err().as_deref(),
            Some(MARK_UNREADABLE_MESSAGE)
        );

        // A bookmark without a readable IDMARK could be the start of a
        // redaction, so it is refused (as the recorder refuses it), and
        // counted.
        for idmark in ["", " IDMARK=\"-805306367.0\"", " IDMARK=\"x\""] {
            let mut skipped = pcm_tones(0b1, 8000, 80, &[(300.0, 0.5)]);
            skipped.bookmarks.push(format!(
                "<ITEM{idmark} COLOR=\"1\"><POSMARK N64Low=\"8\" N64High=\"0\"/><PUBLNOTE><![CDATA[Begin Redaction <play_black>]]></PUBLNOTE></ITEM>"
            ));
            skipped.end_redaction(Some(16));
            skipped.bookmarks.push(bookmark_item(1, 3, Some(8), "auto"));
            let path = skipped.write(&dir.0, "skipped.dcr");
            let info = inspect(&path, &never).unwrap();
            assert_eq!(info.refusal().as_deref(), Some(BOOKMARK_UNREADABLE_MESSAGE), "{idmark}");
            assert_eq!(info.bookmarks.total, 3);
            assert_eq!(info.bookmarks.unreadable, 1);
            assert_eq!(
                info.bookmarks.by_type,
                BTreeMap::from([(1, 1), (BOOKMARK_END_REDACTION, 1)])
            );
            assert_eq!(
                prepare_feed(&path, &never).err().as_deref(),
                Some(BOOKMARK_UNREADABLE_MESSAGE)
            );
        }
    }

    #[test]
    fn a_bookmark_list_that_cannot_be_read_to_its_end_is_refused() {
        let dir = TestDir::new();
        let begin = bookmark_item(BOOKMARK_BEGIN_REDACTION, 2, Some(8000), "Begin Redaction <play_black>");
        let end = bookmark_item(BOOKMARK_END_REDACTION, 3, Some(16_000), "End Redaction");
        let unclosed_item = "<ITEM IDMARK=\"268435457\"><POSMARK N64Low=\"8\" N64High=\"0\"/><PUBLNOTE>note</PUBLNOTE>";
        let payloads = [
            // The first item is missing its </ITEM>; a well-formed redaction
            // follows it.
            format!("<Root><BOOK>{unclosed_item}{begin}{end}</BOOK></Root>"),
            // The list is not closed.
            format!("<Root><BOOK>{begin}{end}"),
            // An item whose note is not closed.
            format!("<Root><BOOK><ITEM IDMARK=\"268435457\"><PUBLNOTE>note</ITEM>{begin}{end}</BOOK></Root>"),
            // An unclosed comment or CDATA section.
            format!("<Root><BOOK>{begin}<!-- {end}</BOOK></Root>"),
            format!("<Root><BOOK>{begin}<![CDATA[ {end}</BOOK></Root>"),
            // Something after the list that cannot be read.
            format!("<Root><BOOK>{begin}{end}</BOOK><EXTRA a=\"1\"></Root>"),
        ];
        for payload in payloads {
            let mut builder = pcm_tones(0b1, 8000, 24_000, &[(300.0, 0.5)]);
            builder.mark_payload = Some(payload.clone().into_bytes());
            let path = builder.write(&dir.0, "partial.dcr");
            let info = inspect(&path, &never).unwrap();
            let refusal = info.refusal();
            assert!(
                refusal.as_deref() == Some(MARK_UNREADABLE_MESSAGE)
                    || refusal.as_deref() == Some(BOOKMARK_UNREADABLE_MESSAGE),
                "{payload}: {refusal:?}"
            );
            assert!(prepare_feed(&path, &never).is_err(), "{payload}");
        }
        // The same redaction in a list that reads to its end is applied.
        let mut whole = pcm_tones(0b1, 8000, 24_000, &[(300.0, 0.5)]);
        whole.mark_payload = Some(format!("<Root><BOOK><!-- a comment -->{begin}{end}</BOOK></Root>").into_bytes());
        let path = whole.write(&dir.0, "whole.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        let (_, data) = read_wav_data(&feed.path);
        let samples = i16s(&data);
        assert_eq!(samples.len(), 24_000);
        assert!(samples[8000..16_000].iter().all(|sample| *sample == 0));
        assert!(samples[..8000].iter().any(|sample| *sample != 0));
    }

    #[test]
    fn comments_in_a_bookmark_never_move_or_change_a_redaction() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1, 8000, 24_000, &[(300.0, 0.5)]);
        let idmark = ((BOOKMARK_BEGIN_REDACTION << 28) | 1) as i32;
        builder.bookmarks.push(format!(
            "<ITEM IDMARK=\"{idmark}\"><!-- <POSMARK N64Low=\"20000\" N64High=\"0\"/> --><POSMARK N64Low=\"8000\" N64High=\"0\"/><PUBLNOTE><!-- <play_play> --><![CDATA[Begin Redaction <play_black>]]></PUBLNOTE></ITEM>"
        ));
        builder.end_redaction(Some(16_000));
        let path = builder.write(&dir.0, "commented.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        assert_eq!(feed.info.redactions.len(), 1);
        assert_eq!(feed.info.redactions[0].start_secs, 1.0);
        assert_eq!(feed.info.redactions[0].action, RedactionAction::Black);
        let (_, data) = read_wav_data(&feed.path);
        let samples = i16s(&data);
        assert!(samples[8000..16_000].iter().all(|sample| *sample == 0));
        assert!(samples[16_000..].iter().any(|sample| *sample != 0));

        // A comment outside CDATA is not note text: here the note has no
        // tag left, so the segment is skipped (the default).
        assert_eq!(
            marked_action(&text_content("<!-- <play_play> -->Begin Redaction")),
            MarkedAction::Skip
        );
        assert_eq!(
            element("<!-- <A x='1'/> --><A x='2'/>", "A").unwrap().attributes.trim(),
            "x='2'"
        );
        assert_eq!(
            split_u64("<!-- <P L=\"9\" H=\"0\"/> --><P L=\"7\" H=\"0\"/>", "P", "L", "H"),
            Some(7)
        );
    }

    #[test]
    fn redactions_in_any_bookmark_section_are_never_ignored() {
        let dir = TestDir::new();
        let section = |begin_at: u64, end_at: u64| {
            format!(
                "<Root><BOOK>{}{}</BOOK></Root>",
                bookmark_item(
                    BOOKMARK_BEGIN_REDACTION,
                    1,
                    Some(begin_at),
                    "Begin Redaction <play_black>"
                ),
                bookmark_item(BOOKMARK_END_REDACTION, 2, Some(end_at), "End Redaction")
            )
            .into_bytes()
        };
        // One bookmark section under a non-zero id is read.
        let mut only = pcm_tones(0b1, 8000, 24_000, &[(300.0, 0.5)]);
        only.mark_id = 3;
        only.mark_payload = Some(section(8000, 16_000));
        let path = only.write(&dir.0, "one-mark.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        assert_eq!(feed.info.redactions.len(), 1);
        let (_, data) = read_wav_data(&feed.path);
        assert!(i16s(&data)[8000..16_000].iter().all(|sample| *sample == 0));

        // Two sections, with or without id 0: refused, never read as none.
        for first_id in [0, 1] {
            let mut two = pcm_tones(0b1, 8000, 24_000, &[(300.0, 0.5)]);
            two.mark_id = first_id;
            two.mark_payload = Some(if first_id == 0 {
                Vec::new()
            } else {
                section(8000, 16_000)
            });
            two.footer_chunks.push((*b"mark", 2, section(8000, 16_000)));
            let path = two.write(&dir.0, "two-marks.dcr");
            let refusal = inspect(&path, &never).unwrap().refusal().unwrap();
            assert!(refusal.contains("2 bookmark sections"), "{refusal}");
            assert_eq!(prepare_feed(&path, &never).err(), Some(refusal));
        }
    }

    #[test]
    fn gsm_and_mp3_recordings_with_redactions_are_refused() {
        let dir = TestDir::new();
        let format = waveformatex(WAVE_FORMAT_GSM610, 8000, 1625, 65, 0, &320u16.to_le_bytes());
        let mut builder = DcrBuilder::new(format, 0b1, &[]);
        for _ in 0..4 {
            builder.atom(&[&[1; 65]]);
        }
        builder
            .begin_redaction(Some(320), "<play_black>")
            .end_redaction(Some(640));
        let path = builder.write(&dir.0, "gsm.dcr");
        let error = prepare_feed(&path, &never).err().unwrap();
        assert!(error.contains("it has redacted segments"), "{error}");
    }

    // ── Password-protected sections ─────────────────────────────────────────

    #[test]
    fn sealed_audio_is_silent_at_its_plain_length_and_warned_about() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &["Judge", "Witness"]);
        builder.have_rec = "1";
        builder.marker(b"audi#mrk").atom(&[&[1, 0], &[2, 0]]);
        builder.marker(b"audi#crp");
        for _ in 0..3 {
            builder.atom(&[&[0x77, 0x77], &[0x77, 0x77]]);
        }
        builder.marker(b"audi#mrk").atom(&[&[3, 0], &[4, 0]]);
        builder.marker(b"audi#crp").atom(&[&[0x77, 0x77], &[0x77, 0x77]]);
        builder.marker(b"audi#crp").atom(&[&[0x77, 0x77], &[0x77, 0x77]]);
        builder.attachment(8, 2);
        let path = builder.write(&dir.0, "sealed.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        let info = &feed.info;
        assert!(info.password_set);
        assert_eq!(info.atoms, 7);
        assert_eq!(info.sealed_sections, 2);
        assert_eq!(info.sealed_atoms, 5);
        assert_eq!(info.attachments_encrypted, 1);
        assert!(info.notices().contains(&SEALED_WARNING.to_string()));
        assert!(info
            .notices()
            .contains(&"Protected or deleted attachments in this recording are not extracted.".to_string()));
        let (_, data) = read_wav_data(&feed.path);
        assert_eq!(i16s(&data), [1, 2, 0, 0, 0, 0, 0, 0, 3, 4, 0, 0, 0, 0]);
        assert!(!data.contains(&0x77), "protected bytes reached the feed");
        let record = serde_json::to_value(info.record()).unwrap();
        assert_eq!(record["sealedSections"], 2);
        assert_eq!(record["sealedSecs"], 5.0 / 8000.0);
        assert_eq!(record["passwordSet"], true);
        assert_eq!(record["attachmentsEncrypted"], 1);
    }

    #[test]
    fn a_password_flag_alone_does_not_stop_a_recording_without_sealed_audio() {
        let dir = TestDir::new();
        let mut legacy = pcm_tones(0b1, 8000, 8, &[(300.0, 0.5)]).legacy();
        legacy.extra_chunks = vec![(*b"crpt", 0, vec![0, 0, 9, 9])];
        let path = legacy.write(&dir.0, "locked.dcr");
        let info = inspect(&path, &never).unwrap();
        assert!(info.password_set);
        assert_eq!(info.sealed_atoms, 0);
        assert_eq!(info.refusal(), None);
        assert!(!info.notices().contains(&SEALED_WARNING.to_string()));

        let mut gsm = DcrBuilder::new(waveformatex(WAVE_FORMAT_GSM610, 8000, 1625, 65, 0, &[]), 0b1, &[]);
        gsm.atom(&[&[1; 65]]).marker(b"audi#crp").atom(&[&[2; 65]]);
        let path = gsm.write(&dir.0, "gsm-sealed.dcr");
        let error = prepare_feed(&path, &never).err().unwrap();
        assert!(error.contains("password-protected sections"), "{error}");
    }

    #[test]
    fn a_protection_chunk_is_warned_about() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1, 8000, 8, &[(300.0, 0.5)]);
        builder.footer_chunks.push((*b"prot", 0, vec![1, 2, 3, 4]));
        let path = builder.write(&dir.0, "prot.dcr");
        let info = inspect(&path, &never).unwrap();
        assert!(info.protection_chunk);
        assert_eq!(info.refusal(), None);
        assert_eq!(info.notices(), [PROT_WARNING]);
    }

    // ── Start time and time zone ───────────────────────────────────────────

    #[test]
    fn the_session_start_and_time_zone_are_recorded_with_their_basis() {
        // 2026-11-22T14:30:00.250 as a FILETIME.
        let filetime = (1_795_357_800u64 + 11_644_473_600) * 10_000_000 + 2_500_000;
        assert_eq!(filetime_text(filetime).as_deref(), Some("2026-11-22T14:30:00.250"));
        assert_eq!(filetime_text(0), None);
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1, 8000, 8, &[(300.0, 0.5)]);
        builder.start_rec = Some(filetime);
        builder.time_zone = "330";
        let path = builder.write(&dir.0, "timed.dcr");
        let record = serde_json::to_value(inspect(&path, &never).unwrap().record()).unwrap();
        assert_eq!(record["startTime"]["stored"], "2026-11-22T14:30:00.250");
        assert!(record["startTime"]["basis"].as_str().unwrap().contains("UTC or local"));
        assert_eq!(record["timeZone"]["localMinusUtcMinutes"], 330);
        assert_eq!(record["timeZone"]["display"], "GMT+5:30");
        assert_eq!(time_zone(-300).display, "GMT-5:00");

        // Before 27.00 there is no time-zone field, so one is not read.
        let mut older = pcm_tones(0b1, 8000, 8, &[(300.0, 0.5)]);
        older.major = 26;
        older.minor = 99;
        let path = older.write(&dir.0, "older.dcr");
        assert_eq!(inspect(&path, &never).unwrap().time_zone, None);
    }

    // ── Remote participants ────────────────────────────────────────────────

    #[test]
    fn a_participant_join_adds_its_ac_bits_and_a_leave_removes_them() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &["Judge", "Witness"]);
        builder.marker(b"audi#mrk");
        builder.atom(&[&1i16.to_le_bytes(), &2i16.to_le_bytes()]);
        builder.participant(true, "0"); // no audio change
        builder.atom(&[&3i16.to_le_bytes(), &4i16.to_le_bytes()]);
        builder.participant(true, "4");
        builder.atom(&[&5i16.to_le_bytes(), &6i16.to_le_bytes(), &7i16.to_le_bytes()]);
        builder.participant(false, "4");
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
                    channel_mask: 4,
                    channels_after: 3,
                },
                ConferenceChange {
                    at_per_channel_byte: 6,
                    at_seconds: 6.0 / 16000.0,
                    kind: "left",
                    channel_mask: 4,
                    channels_after: 2,
                },
            ]
        );
        let (_, data) = read_wav_data(&feed.path);
        assert_eq!(i16s(&data), [1, 2, 0, 3, 4, 0, 5, 6, 7, 8, 9, 0]);
    }

    #[test]
    fn participants_fill_channel_bits_in_order_whatever_the_gaps() {
        let dir = TestDir::new();
        // Bits 0, 1, 2 and 4; a participant brings bit 3, which sits between
        // Plaintiff and Defense in every atom while it is active.
        let mut gap = DcrBuilder::new(
            pcm16_format(8000),
            0b1_0111,
            &["Judge", "Witness", "Plaintiff", "Defense"],
        );
        gap.marker(b"audi#mrk").atom(&[&[1, 0], &[2, 0], &[3, 0], &[4, 0]]);
        gap.participant(true, "8")
            .atom(&[&[11, 0], &[12, 0], &[13, 0], &[14, 0], &[15, 0]]);
        gap.participant(false, "0x8")
            .atom(&[&[21, 0], &[22, 0], &[23, 0], &[24, 0]]);
        // A leave that removes a starting channel (Witness, bit 1).
        gap.participant(false, "2").atom(&[&[31, 0], &[32, 0], &[33, 0]]);
        let path = gap.write(&dir.0, "gap.dcr");
        let feed = prepare_feed(&path, &never).unwrap();
        assert_eq!(feed.info.channels, 5);
        assert_eq!(
            feed.info.channel_names,
            ["Judge", "Witness", "Plaintiff", "Ch 4", "Defense"]
        );
        let (_, data) = read_wav_data(&feed.path);
        let samples: Vec<u8> = data.chunks_exact(2).map(|pair| pair[0]).collect();
        assert_eq!(
            samples,
            [1, 2, 3, 0, 4, 11, 12, 13, 14, 15, 21, 22, 23, 0, 24, 31, 0, 32, 0, 33]
        );
        assert!(feed
            .info
            .conference_changes
            .iter()
            .all(|change| change.channel_mask != 0));
    }

    #[test]
    fn ptch_ac_attribute_gives_the_channel_mask() {
        use PtchEffect::*;
        assert_eq!(ptch_effect(b"<HD ac=\"0\" x=\"1\"/>"), Mask(0));
        assert_eq!(ptch_effect(b"<HD ac=\"1\"/>"), Mask(1));
        assert_eq!(ptch_effect(b"<HD AC = ' 6 '/>"), Mask(6));
        assert_eq!(ptch_effect(b"<HD ac=\"-2147483648\"/>"), Mask(0x8000_0000));
        assert_eq!(ptch_effect(b"<HD mac=\"1\" ac='0'/>"), Mask(0));
        // A missing or unreadable `ac` is unknown, never a change.
        assert_eq!(ptch_effect(b"<HD name=\"x\"/>"), Unknown);
        assert_eq!(ptch_effect(b""), Unknown);
        assert_eq!(ptch_effect(b"<HD ac=1/>"), Unknown);
        assert_eq!(ptch_effect(b"<HD ac=\"two\"/>"), Unknown);
        // Only a whole attribute name counts, outside quoted values.
        assert_eq!(ptch_effect(b"<HD data-ac=\"1\" ac=\"0\"/>"), Mask(0));
        assert_eq!(ptch_effect(b"<HD data-ac=\"1\"/>"), Unknown);
        assert_eq!(ptch_effect(b"<HD x:ac=\"1\" ac.y=\"1\"/>"), Unknown);
        assert_eq!(ptch_effect(b"<HD fn=\"a ac='1'\" ac=\"0\"/>"), Mask(0));
        // UTF-16, little- or big-endian, with or without a byte-order mark.
        assert_eq!(ptch_effect(&utf16le("<HD ac=\"0\"/>")), Mask(0));
        assert_eq!(ptch_effect(&utf16le("<HD ac=\"1\"/>")), Mask(1));
        let mut marked = vec![0xFF, 0xFE];
        marked.extend(utf16le("<HD ac=\"0\"/>"));
        assert_eq!(ptch_effect(&marked), Mask(0));
        let big: Vec<u8> = "<HD ac=\"2\"/>".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(ptch_effect(&big), Mask(2));
    }

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    // ── XML ─────────────────────────────────────────────────────────────────

    #[test]
    fn meta_xml_is_utf8_unless_windows_1252_is_declared() {
        let hex: String = pcm16_format(8000).iter().map(|byte| format!("{byte:02x}")).collect();
        let body = |declaration: &str, name: &[u8]| {
            let mut xml = format!("{declaration}<Root><META><AUDIO USECHNL=\"3\"><FMTDATA><![CDATA[{hex}]]></FMTDATA><LISTCHNL><ITEM><CHNAME>").into_bytes();
            xml.extend_from_slice(name);
            xml.extend_from_slice(b" &amp; Bar</CHNAME></ITEM><ITEM><CHNAME><![CDATA[<Witness>]]></CHNAME></ITEM></LISTCHNL></AUDIO><CRYPT HAVEREC='0'/></META></Root>");
            xml
        };
        let utf8 = parse_meta(
            &body("<?xml version=\"1.0\" encoding=\"utf-8\"?>", "Caf\u{e9}".as_bytes()),
            2700,
        )
        .unwrap();
        assert_eq!(utf8.mask, 3);
        assert_eq!(utf8.names, [Some("Caf\u{e9} & Bar".into()), Some("<Witness>".into())]);
        assert!(!utf8.password_set);
        // No declaration: UTF-8 too.
        let bare = parse_meta(&body("", "Caf\u{e9}".as_bytes()), 2700).unwrap();
        assert_eq!(bare.names[0].as_deref(), Some("Caf\u{e9} & Bar"));
        let declared = parse_meta(
            &body("<?xml version=\"1.0\" encoding=\"windows-1252\"?>", b"Caf\xE9 \x80"),
            2700,
        )
        .unwrap();
        assert_eq!(declared.names[0].as_deref(), Some("Caf\u{e9} \u{20ac} & Bar"));
        // Upper-case hex is accepted as well as lower-case.
        let upper = String::from_utf8(body("", b"A"))
            .unwrap()
            .replace(&hex, &hex.to_uppercase());
        assert_eq!(parse_meta(upper.as_bytes(), 2700).unwrap().format, pcm16_format(8000));
        // USECHNL written with %i: bit 31 reads back as a negative number.
        assert_eq!(parse_mask("-2147483647"), Some(0x8000_0001));
        assert_eq!(parse_mask("0"), None);
        assert!(parse_meta(b"<Root><META><AUDIO USECHNL=\"0\"></AUDIO></META></Root>", 2700).is_err());
        assert!(parse_meta(
            b"<Root><META><AUDIO USECHNL=\"1\"><FMTDATA>ZZ</FMTDATA></AUDIO></META></Root>",
            2700
        )
        .is_err());
    }

    #[test]
    fn split_values_read_both_halves_as_32_bits() {
        assert_eq!(
            split_u64(
                "<POSMARK N64Low=\"32000\" N64High=\"0\"/>",
                "POSMARK",
                "N64Low",
                "N64High"
            ),
            Some(32_000)
        );
        assert_eq!(
            split_u64(
                "<POSMARK N64Low=\"-1\" N64High=\"-1\"/>",
                "POSMARK",
                "N64Low",
                "N64High"
            ),
            Some(u64::MAX)
        );
        assert_eq!(
            split_u64(
                "<STARTREC FTLow=\"-1294046208\" FTHigh=\"31021245\"/>",
                "STARTREC",
                "FTLow",
                "FTHigh"
            ),
            Some((31_021_245u64 << 32) | u64::from(-1_294_046_208i32 as u32))
        );
        assert_eq!(
            split_u64("<POSMARK N64Low=\"1\"/>", "POSMARK", "N64Low", "N64High"),
            None
        );
    }

    #[test]
    fn child_elements_are_found_whatever_their_names() {
        let text = "<?xml version=\"1.0\"?><!-- c --><A x=\"1\"><A/><!-- </A> --><B><![CDATA[</A>]]></B></A><C/>";
        let children = child_elements(text).unwrap();
        assert_eq!(children.iter().map(|(name, _)| *name).collect::<Vec<_>>(), ["A", "C"]);
        assert_eq!(children[0].1.attributes.trim(), "x=\"1\"");
        assert_eq!(child_elements(children[0].1.inner).unwrap().len(), 2);
        // Anything that cannot be read to its end: no partial list.
        for broken in [
            "<A><B/>",
            "<A/><B",
            "<A/><!-- x",
            "<A/><![CDATA[ x",
            "<A x=\"1>",
            "<A/></B",
        ] {
            assert!(child_elements(broken).is_none(), "{broken}");
        }
    }

    // ── Privacy ─────────────────────────────────────────────────────────────

    #[test]
    fn errors_records_and_notices_never_carry_case_password_note_or_participant_text() {
        let dir = TestDir::new();
        let mut sealed = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]);
        sealed.have_rec = "1";
        sealed.marker(b"audi#crp").atom(&[&[9, 9], &[9, 9]]);
        sealed
            .begin_redaction(Some(8), "<play_black>")
            .end_redaction(Some(16))
            .end_redaction(Some(30));
        sealed.bookmarks.push(bookmark_item(3, 9, Some(20), SECRET_NOTE));
        let sealed = sealed.write(&dir.0, "private.dcr");
        // A readable recording with a remote participant whose record names
        // them, and one refused for an unreadable participant record.
        let mut remote = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]);
        remote
            .participant(true, "4")
            .atom(&[&[0; 2], &[0; 2], &[0; 2]])
            .participant(false, "4");
        let remote = remote.write(&dir.0, "remote.dcr");
        let mut unreadable = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]);
        unreadable.participant_text(true, format!("<HD fn=\"{SECRET_PARTICIPANT}\"/>").as_bytes());
        let unreadable = unreadable.write(&dir.0, "unreadable.dcr");
        let mut unplaced = pcm_tones(0b11, 8000, 40, &[(300.0, 0.5), (500.0, 0.25)]);
        unplaced.begin_redaction(None, "");
        let unplaced = unplaced.write(&dir.0, "unplaced.dcr");
        let mut everything = String::new();
        for path in [&sealed, &remote, &unreadable, &unplaced] {
            let info = inspect(path, &never).unwrap();
            everything.push_str(&format!(
                "{} {} {} {:?} {:?}",
                serde_json::to_string(&info).unwrap(),
                serde_json::to_string(&info.record()).unwrap(),
                serde_json::to_string(&info.queue_info()).unwrap(),
                serde_json::to_string(&queue_streams_info(path)).unwrap(),
                prepare_feed(path, &never).err()
            ));
        }
        assert!(everything.contains("password-protected sections"), "{everything}");
        assert!(everything.contains("redacted segment"), "{everything}");
        assert!(everything.contains("joined"), "{everything}");
        assert!(everything.contains("remote-participant record"), "{everything}");
        assert!(everything.contains("no position"), "{everything}");
        for secret in SECRETS {
            assert!(!everything.contains(secret), "{secret} leaked: {everything}");
        }
    }

    #[test]
    fn guidance_names_both_recorders_and_assumes_neither() {
        for message in [
            SPEEX_MESSAGE.to_string(),
            PTCH_UNREADABLE_MESSAGE.into(),
            MARK_UNREADABLE_MESSAGE.into(),
            REDACTION_POSITION_MESSAGE.into(),
            damaged("it ends early"),
            several_data_chunks(2),
        ] {
            assert!(
                message.contains("software that recorded it")
                    && message.contains("Liberty Court Recorder")
                    && message.contains("BIS Digital"),
                "{message}"
            );
            assert!(!message.contains("the Liberty software"), "{message}");
        }
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
    fn a_short_last_atom_is_left_out_and_reported_but_misalignment_after_a_change_is_refused() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b11, 8000, 1000, &[(300.0, 0.5), (500.0, 0.25)]);
        builder.data.extend_from_slice(&[1, 2]);
        let path = builder.write(&dir.0, "short-end.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.atoms, 1000);
        assert_eq!(info.trailing_bytes_dropped, 2);
        assert_eq!(info.refusal(), None);
        assert_eq!(info.queue_info().trailing_bytes_dropped, 2);
        let record = serde_json::to_value(info.record()).unwrap();
        assert_eq!(record["trailingBytesDropped"], 2);
        let feed = prepare_feed(&path, &never).unwrap();
        let (_, data) = read_wav_data(&feed.path);
        assert_eq!(data.len(), 4000, "only whole atoms reach the feed");

        // A whole recording has no trailingBytesDropped field at all.
        let whole = pcm_tones(0b11, 8000, 10, &[(300.0, 0.5), (500.0, 0.25)]).write(&dir.0, "whole.dcr");
        let record = serde_json::to_value(inspect(&whole, &never).unwrap().record()).unwrap();
        assert!(record.get("trailingBytesDropped").is_none());

        let mut changed = pcm_tones(0b11, 8000, 4, &[(300.0, 0.5), (500.0, 0.25)]);
        changed
            .participant(true, "4")
            .atom(&[&[0; 6]])
            .data
            .extend_from_slice(&[1, 2]);
        let path = changed.write(&dir.0, "misaligned.dcr");
        let error = inspect(&path, &never).err().unwrap();
        assert!(error.contains("atom misalignment"), "{error}");
    }

    #[test]
    fn a_record_marker_inside_an_atom_is_misalignment_not_audio() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &[]);
        builder.marker(b"audi#mrk").atom(&[&[1, 0], &[2, 0]]);
        // Two stray bytes put the next video frame one block into an atom.
        builder.data.extend_from_slice(&[3, 0]);
        builder.video_frame(10).atom(&[&[4, 0], &[5, 0]]);
        let path = builder.write(&dir.0, "shifted.dcr");
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
                    // The last entry: make its size run past the directory.
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
                    // Directory entry 1 is `data`.
                    let data_offset = le_u64(&bytes, head_offset + 24 + 8) as usize;
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
    fn a_utf16_ptch_record_without_a_change_keeps_the_channels() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &["Judge", "Witness"]);
        builder.marker(b"audi#mrk").atom(&[&[1, 0], &[2, 0]]);
        builder.participant_text(true, &utf16le("<p ac=\"0\"/>"));
        for index in 3..6u8 {
            builder.atom(&[&[index, 0], &[index + 10, 0]]);
        }
        let path = builder.write(&dir.0, "utf16.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.channels, 2);
        assert_eq!(info.atoms, 4);
        assert_eq!(info.channel_names, ["Judge", "Witness"]);
        assert_eq!(info.conference_change_count, 0);
        assert_eq!(info.refusal(), None);
    }

    #[test]
    fn an_unreadable_ptch_record_refuses_the_recording_before_anything_is_written() {
        let dir = TestDir::new();
        let mut builder = DcrBuilder::new(pcm16_format(8000), 0b11, &["Judge", "Witness"]);
        builder.marker(b"audi#mrk").atom(&[&[1, 0], &[2, 0]]);
        builder.participant_text(true, b"<p name=\"x\"/>");
        builder.atom(&[&[1, 0], &[2, 0], &[3, 0]]);
        let path = builder.write(&dir.0, "unknown.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.refusal().as_deref(), Some(PTCH_UNREADABLE_MESSAGE));
        assert_eq!(info.channels, 2, "the walk stops at the record");
        assert_eq!(
            prepare_feed(&path, &never).err().as_deref(),
            Some(PTCH_UNREADABLE_MESSAGE)
        );
        assert_eq!(queue_streams_info(&path).streams.len(), 0);

        // An `ac` beyond the bytes searched is unknown too.
        let mut long = DcrBuilder::new(pcm16_format(8000), 0b1, &[]);
        let mut text = b"<p note=\"".to_vec();
        text.extend(std::iter::repeat_n(b'x', MAX_PTCH_TEXT));
        text.extend_from_slice(b"\" ac=\"0\"/>");
        long.atom(&[&[1, 0]]).participant_text(true, &text).atom(&[&[2, 0]]);
        let path = long.write(&dir.0, "long.dcr");
        let info = inspect(&path, &never).unwrap();
        assert_eq!(info.refusal().as_deref(), Some(PTCH_UNREADABLE_MESSAGE));
    }

    #[test]
    fn several_data_chunks_are_refused_rather_than_converted_short() {
        let dir = TestDir::new();
        let mut builder = pcm_tones(0b1, 8000, 4, &[(300.0, 0.5)]);
        builder.extra_chunks.push((*b"data", 1, vec![0; 8000]));
        let path = builder.write(&dir.0, "resumed.dcr");
        let info = inspect(&path, &never).unwrap();
        let refusal = info.refusal().unwrap();
        assert!(refusal.contains("2 audio data sections"), "{refusal}");
        assert!(prepare_feed(&path, &never).is_err());

        // Neither has id 0: the same refusal, not a damaged-file error.
        builder.data_id = 2;
        let path = builder.write(&dir.0, "streams.dcr");
        let refusal = inspect(&path, &never).unwrap().refusal().unwrap();
        assert!(refusal.contains("2 audio data sections"), "{refusal}");
        assert_eq!(prepare_feed(&path, &never).err(), Some(refusal));
    }

    #[test]
    fn channel_names_lose_invisible_format_characters() {
        assert_eq!(
            bounded_channel_name("Wit\u{202E}ness\u{200B}\u{2066}\u{FEFF}").as_deref(),
            Some("Witness")
        );
        assert_eq!(bounded_channel_name("\u{200F}\u{00AD}"), None);
        assert_eq!(
            bounded_channel_name("Caf\u{e9} \u{1F3A4}").as_deref(),
            Some("Caf\u{e9} \u{1F3A4}")
        );
    }

    #[test]
    fn the_inspect_cache_follows_file_changes() {
        let dir = TestDir::new();
        let path = pcm_tones(0b1, 8000, 4, &[(300.0, 0.5)]).write(&dir.0, "cached.dcr");
        assert_eq!(inspect_cached(&path, &never).unwrap().atoms, 4);
        assert_eq!(inspect_cached(&path, &never).unwrap().atoms, 4);
        pcm_tones(0b1, 8000, 9, &[(300.0, 0.5)]).write(&dir.0, "cached.dcr");
        assert_eq!(
            inspect_cached(&path, &never).unwrap().atoms,
            9,
            "a new size is inspected again"
        );
    }

    #[test]
    fn a_feed_is_always_planned_from_the_file_as_it_is_now() {
        let dir = TestDir::new();
        let write = |tag: &str| {
            let mut builder = pcm_tones(0b1, 8000, 24_000, &[(300.0, 0.5)]);
            builder.begin_redaction(Some(8000), tag).end_redaction(Some(16_000));
            builder.write(&dir.0, "replaced.dcr")
        };
        // Both tags are 11 bytes, so the file keeps its size.
        let path = write("<play_play>");
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(inspect_cached(&path, &never).unwrap().output_atoms, 24_000);
        let size = fs::metadata(&path).unwrap().len();
        write("<play_skip>");
        assert_eq!(fs::metadata(&path).unwrap().len(), size);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        let feed = prepare_feed(&path, &never).unwrap();
        assert_eq!(feed.info.output_atoms, 16_000);
        assert_eq!(feed.info.redactions[0].action, RedactionAction::Skip);
        let (_, data) = read_wav_data(&feed.path);
        assert_eq!(
            i16s(&data).len(),
            16_000,
            "the segment marked to be left out is left out"
        );
        // The queue now shows the plan the feed used.
        assert_eq!(inspect_cached(&path, &never).unwrap().output_atoms, 16_000);
    }

    #[test]
    fn a_merged_feed_loses_its_guessed_speaker_mask() {
        let dir = TestDir::new();
        let path = dir.0.join("merged.wav");
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&0xFFFEu16.to_le_bytes());
        fmt.extend_from_slice(&4u16.to_le_bytes());
        fmt.extend_from_slice(&8000u32.to_le_bytes());
        fmt.extend_from_slice(&64000u32.to_le_bytes());
        fmt.extend_from_slice(&8u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&0x107u32.to_le_bytes());
        fmt.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
        let mut bytes = b"RIFF\0\0\0\0WAVEJUNK\x04\0\0\0\0\0\0\0fmt ".to_vec();
        bytes.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&fmt);
        bytes.extend_from_slice(b"data\x08\0\0\0");
        bytes.extend_from_slice(&[7; 8]);
        fs::write(&path, &bytes).unwrap();
        clear_channel_mask(&path).unwrap();
        let after = fs::read(&path).unwrap();
        let mask_at = 12 + 12 + 8 + 20;
        assert_eq!(le_u32(&after, mask_at), 0);
        let mut expected = bytes.clone();
        expected[mask_at..mask_at + 4].copy_from_slice(&[0; 4]);
        assert_eq!(after, expected, "nothing else changes");
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
            .participant(true, "2")
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
