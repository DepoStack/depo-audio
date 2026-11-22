//! DigitalCAT / SoundSync `.dm` audio (Stenovations).
//!
//! FFmpeg cannot read a `.dm` file, so DepoAudio reads the audio out of it
//! into a temporary RIFF WAVE file, the same way an SGMCA loses its vendor
//! prefix, and FFmpeg decodes that copy. The original is only read.
//!
//! The layout below comes from owner-supplied interoperability notes and is
//! not yet confirmed against a recorded file; `docs/formats/dm.md` describes
//! it. All numbers are little-endian:
//!
//! - a 16-byte header: version, header size (8), format size, block size;
//! - `format size` bytes of a Windows WAVEFORMATEX describing the codec;
//! - blocks to the end of the file, each a 4-byte Unix time stamp (seconds,
//!   UTC), a 4-byte length and that many bytes of codec data. A stamp of 0
//!   marks a block that was recorded over; its audio is stale and skipped.
//!
//! Every count read from the file is bounds-checked against the file's size
//! and fixed caps before anything is allocated, so a random file gives an
//! error, never a panic or a huge allocation.
//!
//! The notes say SoundSync's player stops at the first block that is shorter
//! than the block size, wherever it is, and at a block the file ends inside
//! of. DepoAudio reads the same blocks: reading stops after the first short
//! block and before an incomplete one. What comes after is left out of the
//! audio and reported, never decoded: SoundSync finds blocks (including the
//! ones it marks as recorded over) by their fixed spacing, which a short
//! block breaks, so stamps after one cannot be trusted to say what was
//! recorded over. The short block's own whole codec blocks are kept, with a
//! warning, because SoundSync does not play them but has not deleted them.

use std::fs;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Serialize;

/// The error for any `.dm` whose header does not parse.
pub(crate) const DM_HEADER_UNRECOGNIZED: &str = "This .dm file's header isn't one DepoAudio recognizes";

/// Size of the fixed header before the WAVEFORMATEX.
const FIXED_HEADER: u64 = 16;
/// The only header size seen in the notes: the two u32 fields that follow it.
const EXPECTED_HEADER_SIZE: u32 = 8;
/// A WAVEFORMATEX without `cbSize` is 16 bytes; anything shorter is not one.
const MIN_FORMAT_SIZE: u32 = 16;
const MAX_FORMAT_SIZE: u32 = 1024 * 1024;
const MAX_BLOCK_SIZE: u32 = 16 * 1024 * 1024;
/// Block bookkeeping costs 4 bytes per block; this caps it at 128 MiB. At the
/// default quarter-second GSM block it is more than 90 days of recording.
const MAX_BLOCKS: usize = 32 * 1024 * 1024;
/// At most this many segments are listed in the export record; the rest are
/// counted in `segmentsOmitted`.
pub(crate) const MAX_RECORDED_SEGMENTS: usize = 1000;
/// A stamp that moves forward by more than this many seconds starts a new
/// segment (a pause). Stamps have one-second resolution and a block lasts
/// about a quarter second, so 0 or 1 is the normal step.
const SEGMENT_GAP_SECONDS: u32 = 2;

pub(crate) const WAVE_FORMAT_PCM: u16 = 0x0001;
pub(crate) const WAVE_FORMAT_ADPCM: u16 = 0x0002;
pub(crate) const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
pub(crate) const WAVE_FORMAT_ALAW: u16 = 0x0006;
pub(crate) const WAVE_FORMAT_MULAW: u16 = 0x0007;
pub(crate) const WAVE_FORMAT_IMA_ADPCM: u16 = 0x0011;
pub(crate) const WAVE_FORMAT_GSM610: u16 = 0x0031;
pub(crate) const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Samples in one 65-byte Microsoft GSM 6.10 block (two 160-sample frames).
const GSM_SAMPLES_PER_BLOCK: u32 = 320;

/// The WAVEFORMATEX fields DepoAudio checks and uses.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WaveFormat {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub cb_size: u16,
    /// `wSamplesPerBlock`, the first extra field of GSM and ADPCM formats.
    pub samples_per_block_field: Option<u16>,
}

impl WaveFormat {
    /// Samples (per channel) in one `block_align` codec block, where the
    /// format makes it knowable. These are the counts FFmpeg's decoders
    /// produce, worked out from the block alignment, channels and bits the
    /// same way they do; the header's `wSamplesPerBlock` field is not used
    /// for the count (FFmpeg ignores it too), and a disagreeing field is
    /// reported as a warning when the file is read.
    pub(crate) fn samples_per_codec_block(&self) -> Option<u32> {
        let align = u32::from(self.block_align);
        let channels = u32::from(self.channels);
        match self.format_tag {
            WAVE_FORMAT_PCM
            | WAVE_FORMAT_IEEE_FLOAT
            | WAVE_FORMAT_ALAW
            | WAVE_FORMAT_MULAW
            | WAVE_FORMAT_EXTENSIBLE => Some(1),
            // gsm_ms: always two 160-sample frames per 65-byte block.
            WAVE_FORMAT_GSM610 => Some(GSM_SAMPLES_PER_BLOCK),
            // adpcm_ms: a 7-byte header per channel holding two samples,
            // then two samples per byte.
            WAVE_FORMAT_ADPCM => align
                .checked_sub(6 * channels)
                .filter(|_| align >= 7 * channels)
                .map(|rest| rest * 2 / channels),
            // adpcm_ima_wav: a 4-byte header per channel holding one sample,
            // then groups of `chunk` bytes per channel holding `samples`.
            WAVE_FORMAT_IMA_ADPCM => {
                let (chunk, samples) = match self.bits_per_sample {
                    2 => (4, 16),
                    3 => (12, 32),
                    4 => (4, 8),
                    5 => (20, 32),
                    _ => return None,
                };
                let rest = align.checked_sub(4 * channels)?;
                Some(1 + rest / (chunk * channels) * samples)
            }
            _ => None,
        }
    }

    /// The FFmpeg decoder DepoAudio expects for this format tag, or the tag
    /// in hex when DepoAudio has no name for it.
    pub(crate) fn codec_name(&self) -> String {
        match (self.format_tag, self.bits_per_sample) {
            (WAVE_FORMAT_PCM, 8) => "pcm_u8".into(),
            (WAVE_FORMAT_PCM, 16) => "pcm_s16le".into(),
            (WAVE_FORMAT_PCM, 24) => "pcm_s24le".into(),
            (WAVE_FORMAT_PCM, 32) => "pcm_s32le".into(),
            (WAVE_FORMAT_PCM, _) => "pcm".into(),
            (WAVE_FORMAT_IEEE_FLOAT, 64) => "pcm_f64le".into(),
            (WAVE_FORMAT_IEEE_FLOAT, _) => "pcm_f32le".into(),
            (WAVE_FORMAT_ADPCM, _) => "adpcm_ms".into(),
            (WAVE_FORMAT_ALAW, _) => "pcm_alaw".into(),
            (WAVE_FORMAT_MULAW, _) => "pcm_mulaw".into(),
            (WAVE_FORMAT_IMA_ADPCM, _) => "adpcm_ima_wav".into(),
            (WAVE_FORMAT_GSM610, _) => "gsm_ms".into(),
            (WAVE_FORMAT_EXTENSIBLE, _) => "extensible".into(),
            (tag, _) => format!("0x{tag:04x}"),
        }
    }

    /// PCM-family formats need no `fact` chunk in a WAV file.
    fn is_pcm_family(&self) -> bool {
        matches!(
            self.format_tag,
            WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT | WAVE_FORMAT_EXTENSIBLE
        )
    }
}

/// The block size the notes say the recorder picks for a format: a quarter
/// second of whole codec blocks, at least one.
pub(crate) fn expected_block_size(wave: &WaveFormat) -> u64 {
    let align = u64::from(wave.block_align.max(1));
    let frames = ((u64::from(wave.avg_bytes_per_sec) / align) >> 2).max(1);
    frames * align
}

/// The fixed header and the WAVEFORMATEX.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DmHeader {
    pub version: u32,
    pub header_size: u32,
    pub format_size: u32,
    pub block_size: u32,
    pub format_bytes: Vec<u8>,
    pub wave: WaveFormat,
}

impl DmHeader {
    /// Byte offset of the first block.
    pub(crate) fn data_start(&self) -> u64 {
        8 + u64::from(self.header_size) + u64::from(self.format_size)
    }
}

/// A parsed `.dm` file: its header and one time stamp per block read, in
/// file order. Every block read is `block_size` bytes long except, possibly,
/// the last one (`final_short_len`): reading stops after a short block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DmFile {
    pub header: DmHeader,
    pub stamps: Vec<u32>,
    /// Length of the last block read when it is shorter than `block_size`.
    pub final_short_len: Option<u32>,
    /// Bytes of the file after that short block, when it is not at the end
    /// of the file. They are not read as blocks and are left out of the audio.
    pub bytes_after_short_block: u64,
    /// Bytes after the last complete block: the start of a block the file
    /// ends inside of. They are left out of the audio.
    pub truncated_tail_bytes: u64,
    /// Facts that do not stop the conversion but are worth knowing.
    pub warnings: Vec<String>,
}

impl DmFile {
    /// Length in bytes of block `index`'s audio.
    pub(crate) fn block_len(&self, index: usize) -> u32 {
        match self.final_short_len {
            Some(length) if index + 1 == self.stamps.len() => length,
            _ => self.header.block_size,
        }
    }

    /// Length of the last block read when it is shorter than `block_size`.
    pub(crate) fn final_block_len(&self) -> Option<u32> {
        self.final_short_len
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

fn unrecognized(detail: &str) -> String {
    format!("{DM_HEADER_UNRECOGNIZED} ({detail}).")
}

/// Check the fixed header's three size fields. `prefix` is at least the
/// first 16 bytes of the file.
fn check_fixed_header(prefix: &[u8]) -> Result<(u32, u32, u32, u32), String> {
    if prefix.len() < FIXED_HEADER as usize {
        return Err(unrecognized("the file is shorter than the header"));
    }
    let version = u32_at(prefix, 0);
    let header_size = u32_at(prefix, 4);
    let format_size = u32_at(prefix, 8);
    let block_size = u32_at(prefix, 12);
    if header_size != EXPECTED_HEADER_SIZE {
        return Err(unrecognized("unexpected header size"));
    }
    if !(MIN_FORMAT_SIZE..=MAX_FORMAT_SIZE).contains(&format_size) {
        return Err(unrecognized("unexpected audio format size"));
    }
    if block_size == 0 || block_size > MAX_BLOCK_SIZE {
        return Err(unrecognized("unexpected block size"));
    }
    Ok((version, header_size, format_size, block_size))
}

/// Read and sanity-check a WAVEFORMATEX. `bytes` is the whole structure as
/// stored (`format_size` bytes), or, for detection, at least its first 16.
fn parse_wave_format(bytes: &[u8], format_size: u32) -> Result<WaveFormat, String> {
    if bytes.len() < MIN_FORMAT_SIZE as usize {
        return Err(unrecognized("the audio format is cut short"));
    }
    let format_tag = u16_at(bytes, 0);
    let channels = u16_at(bytes, 2);
    let samples_per_sec = u32_at(bytes, 4);
    let avg_bytes_per_sec = u32_at(bytes, 8);
    let block_align = u16_at(bytes, 12);
    let bits_per_sample = u16_at(bytes, 14);
    let has_cb_size = format_size >= 18 && bytes.len() >= 18;
    let cb_size = if has_cb_size { u16_at(bytes, 16) } else { 0 };
    if has_cb_size && 18 + u32::from(cb_size) > format_size {
        return Err(unrecognized("the audio format's extra bytes do not fit"));
    }
    // The notes: the format size is 18 for PCM and 18 + cbSize otherwise. A
    // 16-byte PCM format (no cbSize) is the older Windows layout and is
    // taken too. Any other size would leave bytes the format does not
    // describe between it and the first block.
    if has_cb_size && 18 + u32::from(cb_size) != format_size {
        return Err(unrecognized("the audio format's size does not match its extra bytes"));
    }
    if format_size < 18 && format_tag != WAVE_FORMAT_PCM {
        return Err(unrecognized("the audio format is cut short"));
    }
    if format_size == 17 {
        return Err(unrecognized("the audio format is cut short"));
    }
    if format_tag == 0 {
        return Err(unrecognized("no audio format tag"));
    }
    if !(1..=16).contains(&channels) {
        return Err(unrecognized("unsupported channel count"));
    }
    if !(1000..=384_000).contains(&samples_per_sec) {
        return Err(unrecognized("unsupported sample rate"));
    }
    if avg_bytes_per_sec == 0 || block_align == 0 {
        return Err(unrecognized("no byte rate or block alignment"));
    }
    let channels_wide = u32::from(channels);
    let align = u32::from(block_align);
    let consistent = match format_tag {
        WAVE_FORMAT_PCM => {
            matches!(bits_per_sample, 8 | 16 | 24 | 32) && align == channels_wide * u32::from(bits_per_sample) / 8
        }
        WAVE_FORMAT_IEEE_FLOAT => {
            matches!(bits_per_sample, 32 | 64) && align == channels_wide * u32::from(bits_per_sample) / 8
        }
        // One byte per sample.
        WAVE_FORMAT_ALAW | WAVE_FORMAT_MULAW => align == channels_wide,
        // FFmpeg's gsm_ms decodes mono 65-byte blocks only.
        WAVE_FORMAT_GSM610 => channels == 1 && block_align == 65,
        // adpcm_ms needs 4-bit samples and room for each channel's header.
        WAVE_FORMAT_ADPCM => bits_per_sample == 4 && align >= 7 * channels_wide,
        // adpcm_ima_wav takes 2 to 5 bits and needs each channel's header.
        WAVE_FORMAT_IMA_ADPCM => (2..=5).contains(&bits_per_sample) && align > 4 * channels_wide,
        // WAVEFORMATEXTENSIBLE carries 22 extra bytes.
        WAVE_FORMAT_EXTENSIBLE => !has_cb_size || cb_size >= 22,
        _ => true,
    };
    if !consistent {
        return Err(unrecognized("the audio format's fields do not fit together"));
    }
    let samples_per_block_field = (cb_size >= 2 && bytes.len() >= 20).then(|| u16_at(bytes, 18));
    Ok(WaveFormat {
        format_tag,
        channels,
        samples_per_sec,
        avg_bytes_per_sec,
        block_align,
        bits_per_sample,
        cb_size,
        samples_per_block_field,
    })
}

/// True when `prefix` (the first bytes of a file, at least 32 for a useful
/// answer) starts like a `.dm` audio file: header size 8, sizes in range and
/// a plausible WAVEFORMATEX. Detection reads only this prefix.
pub(crate) fn looks_like_dm(prefix: &[u8]) -> bool {
    let Ok((_, _, format_size, block_size)) = check_fixed_header(prefix) else {
        return false;
    };
    let format = &prefix[FIXED_HEADER as usize..];
    match parse_wave_format(format, format_size) {
        Ok(wave) => block_size % u32::from(wave.block_align) == 0,
        Err(_) => false,
    }
}

fn read_exact_or<R: Read>(reader: &mut R, buffer: &mut [u8], message: impl FnOnce() -> String) -> Result<(), String> {
    reader.read_exact(buffer).map_err(|error| match error.kind() {
        std::io::ErrorKind::UnexpectedEof => message(),
        _ => format!("Could not read the .dm file: {error}"),
    })
}

/// Parse a whole `.dm` from `reader`, whose total length is `len`.
pub(crate) fn parse_dm_reader<R: Read + Seek>(
    reader: R,
    len: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<DmFile, String> {
    let mut reader = BufReader::with_capacity(4096, reader);
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("Could not read the .dm file: {error}"))?;
    let mut fixed = [0u8; FIXED_HEADER as usize];
    if len < FIXED_HEADER {
        return Err(unrecognized("the file is shorter than the header"));
    }
    read_exact_or(&mut reader, &mut fixed, || {
        unrecognized("the file is shorter than the header")
    })?;
    let (version, header_size, format_size, block_size) = check_fixed_header(&fixed)?;
    if FIXED_HEADER + u64::from(format_size) > len {
        return Err(unrecognized("the audio format is cut short"));
    }
    // format_size is at most 1 MiB, checked above, and fits in the file.
    let mut format_bytes = vec![0u8; format_size as usize];
    read_exact_or(&mut reader, &mut format_bytes, || {
        unrecognized("the audio format is cut short")
    })?;
    let wave = parse_wave_format(&format_bytes, format_size)?;
    if block_size % u32::from(wave.block_align) != 0 {
        return Err(unrecognized("the block size is not whole codec blocks"));
    }
    let header = DmHeader {
        version,
        header_size,
        format_size,
        block_size,
        format_bytes,
        wave,
    };

    let mut warnings = Vec::new();
    let expected = expected_block_size(&header.wave);
    if u64::from(block_size) != expected {
        warnings.push(format!(
            "The header's block size is {block_size} bytes; this audio format usually uses {expected}."
        ));
    }
    let counted_from_alignment = matches!(
        header.wave.format_tag,
        WAVE_FORMAT_GSM610 | WAVE_FORMAT_ADPCM | WAVE_FORMAT_IMA_ADPCM
    );
    if let (true, Some(field), Some(decoded)) = (
        counted_from_alignment,
        header.wave.samples_per_block_field,
        header.wave.samples_per_codec_block(),
    ) {
        if u32::from(field) != decoded {
            warnings.push(format!(
                "The header says {field} samples per codec block; its block alignment gives {decoded}, which is \
                 what the decoder produces, so DepoAudio counts {decoded}."
            ));
        }
    }

    let data_start = header.data_start();
    let stride = 8 + u64::from(block_size);
    let available = len - data_start;
    // Upper bound on the block count, before anything is allocated for it.
    let most_blocks = available.div_ceil(stride);
    if most_blocks > MAX_BLOCKS as u64 {
        return Err(format!(
            "This .dm file has more blocks than DepoAudio reads ({MAX_BLOCKS}); it was not converted."
        ));
    }
    let mut stamps = Vec::with_capacity(most_blocks as usize);
    let mut final_short_len: Option<u32> = None;
    let mut bytes_after_short_block = 0u64;
    let mut truncated_tail_bytes = 0u64;
    let mut position = data_start;
    while position < len {
        if stamps.len() % 4096 == 0 && cancelled() {
            return Err(crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
        }
        let index = stamps.len();
        if len - position < 8 {
            truncated_tail_bytes = len - position;
            break;
        }
        let mut block_header = [0u8; 8];
        read_exact_or(&mut reader, &mut block_header, || {
            format!("This .dm file ended while it was being read (block {}).", index + 1)
        })?;
        let time = u32_at(&block_header, 0);
        let length = u32_at(&block_header, 4);
        // A block that claims more bytes than remain is the incomplete tail
        // whatever length it claims: the notes say SoundSync loads such a
        // file and stops playing at that block. A damaged length there must
        // not cost the complete blocks before it.
        let end = position + 8 + u64::from(length);
        if end > len {
            truncated_tail_bytes = len - position;
            break;
        }
        if length > block_size {
            return Err(format!(
                "Block {} of this .dm file has an unexpected length ({length} bytes; blocks are {block_size}).",
                index + 1
            ));
        }
        stamps.push(time);
        if length < block_size {
            // SoundSync's player stops at a short block wherever it is; so
            // does this reader (see the module notes).
            final_short_len = Some(length);
            bytes_after_short_block = len - end;
            break;
        }
        reader
            .seek_relative(i64::from(length))
            .map_err(|error| format!("Could not read the .dm file: {error}"))?;
        position = end;
    }
    if stamps.is_empty() {
        if truncated_tail_bytes > 0 {
            return Err(
                "This .dm file ends partway through its first block, so it holds no complete audio \
                        block; it may be incomplete."
                    .into(),
            );
        }
        return Err("This .dm file holds no audio blocks.".into());
    }
    if truncated_tail_bytes > 0 {
        warnings.push(format!(
            "The file ends partway through block {}: its last {truncated_tail_bytes} bytes are an incomplete \
             block and were left out. The recording or copy may not have finished; the {} complete blocks \
             before it were read.",
            stamps.len() + 1,
            stamps.len()
        ));
    }
    if let Some(length) = final_short_len {
        let number = stamps.len();
        let align = u32::from(header.wave.block_align);
        let whole = length / align;
        let stray = length % align;
        let erased = stamps[number - 1] == 0;
        let kept = if erased {
            "It is marked as recorded over, so none of it is used.".to_string()
        } else if whole == 0 {
            format!("It holds less than one codec block, so none of its {length} bytes are used.")
        } else if stray == 0 {
            format!(
                "DepoAudio kept its {whole} whole codec blocks, which the format notes say SoundSync's own \
                 player does not play."
            )
        } else {
            format!(
                "DepoAudio kept its {whole} whole codec blocks, which the format notes say SoundSync's own \
                 player does not play, and left out the {stray} bytes after them."
            )
        };
        if bytes_after_short_block > 0 {
            warnings.push(format!(
                "Block {number} is shorter than the others ({length} of {block_size} bytes) and is not at the end \
                 of the file. {kept} The format notes say SoundSync's player stops at a short block, so the \
                 {bytes_after_short_block} bytes after it were not read and are not in the converted audio."
            ));
        } else if !erased {
            warnings.push(format!(
                "The last block is shorter than the others ({length} of {block_size} bytes). {kept}"
            ));
        }
    }
    Ok(DmFile {
        header,
        stamps,
        final_short_len,
        bytes_after_short_block,
        truncated_tail_bytes,
        warnings,
    })
}

/// Parse the `.dm` at `path`.
pub(crate) fn parse_dm(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<DmFile, String> {
    let file = fs::File::open(path).map_err(|error| format!("Could not open the .dm file: {error}"))?;
    let len = file
        .metadata()
        .map_err(|error| format!("Could not read the .dm file: {error}"))?
        .len();
    parse_dm_reader(file, len, cancelled)
}

/// Bytes of sound SoundSync's loader counts in the `.dm` at `path`, as the
/// notes describe it: it walks every block header to the end of the file by
/// its stated length, counting recorded-over blocks, blocks after a short
/// block, and the stated length of a block the file ends inside of.
/// SoundSync's timeline (and so the range of bookmarks it keeps) is this
/// total divided by the byte rate. Only used to count markers; None when the
/// file cannot be read or walks more blocks than `MAX_BLOCKS`.
pub(crate) fn soundsync_loader_sound_bytes(path: &Path, header: &DmHeader) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut reader = BufReader::with_capacity(4096, file);
    let mut position = header.data_start();
    reader.seek(SeekFrom::Start(position)).ok()?;
    let mut total = 0u64;
    let mut walked = 0usize;
    while position.checked_add(8)? <= len {
        walked += 1;
        if walked > MAX_BLOCKS {
            return None;
        }
        let mut block_header = [0u8; 8];
        reader.read_exact(&mut block_header).ok()?;
        let length = u64::from(u32_at(&block_header, 4));
        total += length;
        position += 8 + length;
        if position < len {
            reader.seek_relative(i64::try_from(length).ok()?).ok()?;
        }
    }
    Some(total)
}

// ── Timeline ────────────────────────────────────────────────────────────────

/// One run of blocks whose stamps move forward by at most two seconds each.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DmSegment {
    /// First sample of the segment in the converted audio (before any
    /// resampling or trim), or null when the codec's samples per block are
    /// not known.
    pub start_sample: Option<u64>,
    /// The stamp of the segment's first block, as UTC, exactly as stored.
    /// The notes say a recording stamps a block when its buffer is handed
    /// back (about the end of the block) and a WAV import stamps the start;
    /// the file does not say which made it, so nothing is adjusted.
    pub first_stamp_utc: String,
    /// Blocks of audio in the segment.
    pub blocks: u64,
}

/// What DepoAudio found in a `.dm` and kept or skipped. Written to the export
/// record as `source.dmTimeline`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DmTimeline {
    pub format_tag: u16,
    pub codec_name: String,
    pub block_size: u32,
    pub blocks_total: u64,
    pub blocks_erased: u64,
    /// The last block read is shorter than `block_size` and was not recorded
    /// over (a recorded-over short block is counted in `blocks_erased` only).
    pub short_final_block: bool,
    /// Bytes of a kept short last block left out because they are less than
    /// one whole codec block.
    pub last_block_bytes_dropped: u32,
    /// Bytes after a short block that is not at the end of the file. Reading
    /// stops at the short block, as SoundSync's player does, so they were
    /// left out.
    pub bytes_after_short_block: u64,
    /// Bytes at the end of the file that are an incomplete block, left out.
    pub truncated_tail_bytes: u64,
    pub stamps_monotonic: bool,
    pub segments: Vec<DmSegment>,
    /// Segments past `MAX_RECORDED_SEGMENTS`, counted but not listed.
    pub segments_omitted: u64,
    /// Header facts that did not stop the conversion (for example a block
    /// size other than the one the format usually gets).
    pub warnings: Vec<String>,
    /// The `.bmf` marker file beside the `.dm`, when there is one: counts
    /// only, never marker text. Filled in by `bmf::summary_beside`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker_file: Option<crate::bmf::MarkerFileSummary>,
}

/// Bytes of block `index` that go into the audio: 0 for an erased block,
/// `block_size` for a full block, and the whole codec blocks of a short
/// block.
fn kept_bytes(dm: &DmFile, index: usize) -> u32 {
    if dm.stamps[index] == 0 {
        return 0;
    }
    let length = dm.block_len(index);
    length - length % u32::from(dm.header.wave.block_align)
}

fn samples_in(dm: &DmFile, bytes: u32) -> Option<u64> {
    let per_codec_block = dm.header.wave.samples_per_codec_block()?;
    let codec_blocks = u64::from(bytes / u32::from(dm.header.wave.block_align));
    Some(codec_blocks * u64::from(per_codec_block))
}

fn utc_from_stamp(stamp: u32) -> String {
    time::OffsetDateTime::from_unix_timestamp(i64::from(stamp))
        .map(|time| crate::clock::rfc3339_seconds_z(&time))
        .unwrap_or_default()
}

/// The timeline of a parsed `.dm`: which blocks were skipped and where the
/// stamps jump. Audio stays in file order; a segment starts wherever a stamp
/// moves forward by more than two seconds or moves backwards.
pub(crate) fn build_timeline(dm: &DmFile) -> DmTimeline {
    let mut segments: Vec<DmSegment> = Vec::new();
    let mut segments_omitted = 0u64;
    let mut current: Option<DmSegment> = None;
    let mut previous: Option<u32> = None;
    let mut monotonic = true;
    let mut erased = 0u64;
    let mut sample: Option<u64> = Some(0);
    for (index, &stamp) in dm.stamps.iter().enumerate() {
        let bytes = kept_bytes(dm, index);
        if stamp == 0 {
            erased += 1;
            continue;
        }
        if bytes == 0 {
            // A short last block of less than one codec block adds no audio.
            continue;
        }
        let starts_segment = match previous {
            None => true,
            Some(before) if stamp < before => {
                monotonic = false;
                true
            }
            Some(before) => stamp - before > SEGMENT_GAP_SECONDS,
        };
        if starts_segment {
            if let Some(done) = current.take() {
                if segments.len() < MAX_RECORDED_SEGMENTS {
                    segments.push(done);
                } else {
                    segments_omitted += 1;
                }
            }
            current = Some(DmSegment {
                start_sample: sample,
                first_stamp_utc: utc_from_stamp(stamp),
                blocks: 0,
            });
        }
        if let Some(segment) = current.as_mut() {
            segment.blocks += 1;
        }
        sample = sample.zip(samples_in(dm, bytes)).map(|(at, more)| at + more);
        previous = Some(stamp);
    }
    if let Some(done) = current {
        if segments.len() < MAX_RECORDED_SEGMENTS {
            segments.push(done);
        } else {
            segments_omitted += 1;
        }
    }
    let align = u32::from(dm.header.wave.block_align);
    let last = dm.stamps.len() - 1;
    let (short_final_block, last_block_bytes_dropped) = match dm.final_block_len() {
        Some(short) if dm.stamps[last] != 0 => (true, short % align),
        _ => (false, 0),
    };
    DmTimeline {
        format_tag: dm.header.wave.format_tag,
        codec_name: dm.header.wave.codec_name(),
        block_size: dm.header.block_size,
        blocks_total: dm.stamps.len() as u64,
        blocks_erased: erased,
        short_final_block,
        last_block_bytes_dropped,
        bytes_after_short_block: dm.bytes_after_short_block,
        truncated_tail_bytes: dm.truncated_tail_bytes,
        stamps_monotonic: monotonic,
        segments,
        segments_omitted,
        warnings: dm.warnings.clone(),
        marker_file: None,
    }
}

/// What the Convert queue shows for a `.dm` before conversion.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DmQueueInfo {
    pub codec_name: String,
    pub blocks_total: u64,
    pub blocks_erased: u64,
    pub short_final_block: bool,
    pub bytes_after_short_block: u64,
    pub truncated_tail_bytes: u64,
    pub stamps_monotonic: bool,
    pub warnings: Vec<String>,
}

pub(crate) fn queue_info(dm: &DmFile) -> DmQueueInfo {
    let timeline = build_timeline(dm);
    DmQueueInfo {
        codec_name: timeline.codec_name,
        blocks_total: timeline.blocks_total,
        blocks_erased: timeline.blocks_erased,
        short_final_block: timeline.short_final_block,
        bytes_after_short_block: timeline.bytes_after_short_block,
        truncated_tail_bytes: timeline.truncated_tail_bytes,
        stamps_monotonic: timeline.stamps_monotonic,
        warnings: dm.warnings.clone(),
    }
}

/// The Convert queue's stream answer for a `.dm`: one stream with the
/// header's channels and rate, the duration of the audio kept, and the
/// recorded-over facts. None when the file does not parse (the conversion
/// then reports why).
pub(crate) fn streams_info(path: &Path) -> Option<crate::manifest::AudioStreamsInfo> {
    let dm = parse_dm(path, &|| false).ok()?;
    let wave = &dm.header.wave;
    let duration_secs = audio_totals(&dm)
        .1
        .filter(|samples| *samples > 0)
        .map(|samples| samples as f64 / f64::from(wave.samples_per_sec));
    Some(crate::manifest::AudioStreamsInfo {
        converted_index: 0,
        streams: vec![crate::manifest::AudioStreamSummary {
            channels: Some(u32::from(wave.channels)),
            sample_rate: Some(wave.samples_per_sec),
            language: None,
            title: None,
        }],
        duration_secs,
        dm: Some(queue_info(&dm)),
    })
}

/// Bytes of audio and samples (per channel) the demuxed file holds.
pub(crate) fn audio_totals(dm: &DmFile) -> (u64, Option<u64>) {
    let mut bytes = 0u64;
    let mut samples = Some(0u64);
    for index in 0..dm.stamps.len() {
        let kept = kept_bytes(dm, index);
        bytes += u64::from(kept);
        samples = samples.zip(samples_in(dm, kept)).map(|(total, more)| total + more);
    }
    (bytes, samples)
}

// ── Demux to RIFF WAVE ──────────────────────────────────────────────────────

/// Sizes of the WAVE file DepoAudio writes for a `.dm`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RiffLayout {
    pub riff_size: u32,
    pub fmt_padding: bool,
    pub fact_samples: Option<u32>,
    pub data_size: u32,
    pub data_padding: bool,
}

/// The message for audio past the plain RIFF limit.
pub(crate) const DM_TOO_LARGE: &str =
    "This .dm file holds more than 4 GiB of audio, which DepoAudio does not convert yet. Nothing was written.";

/// Plan the RIFF chunks for `format_len` bytes of WAVEFORMATEX and
/// `data_len` bytes of audio. A plain RIFF file holds at most 4 GiB; audio
/// that would not fit is refused rather than written as RF64 (a DigitalCAT
/// GSM recording is about 8 MiB an hour, so only very long PCM reaches it).
pub(crate) fn riff_layout(
    format_len: u32,
    pcm_family: bool,
    samples: Option<u64>,
    data_len: u64,
) -> Result<RiffLayout, String> {
    let fmt_padding = format_len % 2 == 1;
    let data_padding = data_len % 2 == 1;
    let fact_samples = if pcm_family {
        None
    } else {
        samples.and_then(|samples| u32::try_from(samples).ok())
    };
    let total = 4
        + 8
        + u64::from(format_len)
        + u64::from(fmt_padding)
        + if fact_samples.is_some() { 12 } else { 0 }
        + 8
        + data_len
        + u64::from(data_padding);
    let riff_size = u32::try_from(total).map_err(|_| DM_TOO_LARGE.to_string())?;
    let data_size = u32::try_from(data_len).map_err(|_| DM_TOO_LARGE.to_string())?;
    Ok(RiffLayout {
        riff_size,
        fmt_padding,
        fact_samples,
        data_size,
        data_padding,
    })
}

/// Write the audio of `source` (parsed as `dm`) to `out` as a RIFF WAVE file:
/// the `.dm`'s own WAVEFORMATEX as `fmt `, a `fact` chunk for compressed
/// formats, and the non-erased blocks in file order as `data`. An incomplete
/// block at the end of the file is not read. The source is
/// streamed block by block. Each block header is read again and must match
/// the parse, so a file that changes meanwhile fails instead of mixing two
/// versions.
pub(crate) fn write_wav<W: Write>(
    source: &Path,
    dm: &DmFile,
    out: &mut W,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let (data_len, samples) = audio_totals(dm);
    let header = &dm.header;
    let layout = riff_layout(header.format_size, header.wave.is_pcm_family(), samples, data_len)?;
    let write_error = |error: std::io::Error| format!("Could not write the temporary audio copy: {error}");

    let mut head = Vec::with_capacity(64 + header.format_bytes.len());
    head.extend_from_slice(b"RIFF");
    head.extend_from_slice(&layout.riff_size.to_le_bytes());
    head.extend_from_slice(b"WAVEfmt ");
    head.extend_from_slice(&header.format_size.to_le_bytes());
    head.extend_from_slice(&header.format_bytes);
    if layout.fmt_padding {
        head.push(0);
    }
    if let Some(fact) = layout.fact_samples {
        head.extend_from_slice(b"fact");
        head.extend_from_slice(&4u32.to_le_bytes());
        head.extend_from_slice(&fact.to_le_bytes());
    }
    head.extend_from_slice(b"data");
    head.extend_from_slice(&layout.data_size.to_le_bytes());
    out.write_all(&head).map_err(write_error)?;

    let file = fs::File::open(source).map_err(|error| format!("Could not open the .dm file: {error}"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    reader
        .seek(SeekFrom::Start(header.data_start()))
        .map_err(|error| format!("Could not read the .dm file: {error}"))?;
    let changed = || "The .dm file changed while it was being read; nothing was converted.".to_string();
    let mut block = vec![0u8; header.block_size as usize];
    let mut written = 0u64;
    for (index, &stamp) in dm.stamps.iter().enumerate() {
        if cancelled() {
            return Err(crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
        }
        let mut block_header = [0u8; 8];
        read_exact_or(&mut reader, &mut block_header, changed)?;
        let length = u32_at(&block_header, 4);
        let expected_length = dm.block_len(index);
        if u32_at(&block_header, 0) != stamp || length != expected_length {
            return Err(changed());
        }
        let payload = &mut block[..length as usize];
        read_exact_or(&mut reader, payload, changed)?;
        let kept = kept_bytes(dm, index) as usize;
        if kept > 0 {
            out.write_all(&payload[..kept]).map_err(write_error)?;
            written += kept as u64;
        }
    }
    if written != data_len {
        return Err(changed());
    }
    if layout.data_padding {
        out.write_all(&[0]).map_err(write_error)?;
    }
    out.flush().map_err(write_error)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// A WAVEFORMATEX as bytes.
    pub(crate) fn wave_format_bytes(
        tag: u16,
        channels: u16,
        rate: u32,
        avg: u32,
        align: u16,
        bits: u16,
        extra: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&avg.to_le_bytes());
        bytes.extend_from_slice(&align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        bytes.extend_from_slice(extra);
        bytes
    }

    /// The default SoundSync format: GSM 6.10, mono, 11025 Hz.
    pub(crate) fn gsm_format() -> Vec<u8> {
        wave_format_bytes(WAVE_FORMAT_GSM610, 1, 11025, 2239, 65, 0, &320u16.to_le_bytes())
    }

    /// 16-bit mono PCM at `rate`.
    pub(crate) fn pcm_format(rate: u32) -> Vec<u8> {
        wave_format_bytes(WAVE_FORMAT_PCM, 1, rate, rate * 2, 2, 16, &[])
    }

    /// A `.dm` file: header, `format` and one block per `(stamp, payload)`.
    pub(crate) fn dm_bytes(format: &[u8], block_size: u32, blocks: &[(u32, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes());
        bytes.extend_from_slice(&(format.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&block_size.to_le_bytes());
        bytes.extend_from_slice(format);
        for (stamp, payload) in blocks {
            bytes.extend_from_slice(&stamp.to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
        }
        bytes
    }

    fn parse(bytes: &[u8]) -> Result<DmFile, String> {
        parse_dm_reader(Cursor::new(bytes), bytes.len() as u64, &|| false)
    }

    const T0: u32 = 1_763_800_000; // 2025-11-22T08:26:40Z

    #[test]
    fn parses_the_default_gsm_header() {
        let block = vec![7u8; 520];
        let bytes = dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0, &block), (T0 + 1, &block)]);
        let dm = parse(&bytes).unwrap();
        assert_eq!(dm.header.version, 1);
        assert_eq!(dm.header.format_size, 20);
        assert_eq!(dm.header.data_start(), 36);
        assert_eq!(dm.header.wave.format_tag, WAVE_FORMAT_GSM610);
        assert_eq!(dm.header.wave.samples_per_sec, 11025);
        assert_eq!(dm.header.wave.samples_per_codec_block(), Some(320));
        assert_eq!(dm.header.wave.codec_name(), "gsm_ms");
        assert_eq!(expected_block_size(&dm.header.wave), 520);
        assert_eq!(dm.stamps, vec![T0, T0, T0 + 1]);
        assert_eq!(dm.final_block_len(), None);
        assert!(dm.warnings.is_empty());
        assert!(looks_like_dm(&bytes));

        let timeline = build_timeline(&dm);
        assert_eq!(timeline.blocks_total, 3);
        assert_eq!(timeline.blocks_erased, 0);
        assert!(timeline.stamps_monotonic);
        assert_eq!(timeline.segments.len(), 1);
        assert_eq!(timeline.segments[0].first_stamp_utc, "2025-11-22T08:26:40Z");
        assert_eq!(timeline.segments[0].start_sample, Some(0));
        assert_eq!(timeline.segments[0].blocks, 3);
        // 8 codec blocks of 320 samples per 520-byte block.
        assert_eq!(audio_totals(&dm), (1560, Some(3 * 8 * 320)));
    }

    #[test]
    fn parses_a_pcm_header_and_warns_on_an_unusual_block_size() {
        let format = pcm_format(11025);
        // The block rule gives (22050 / 2) >> 2 = 2756 frames = 5512 bytes.
        let block = vec![0u8; 5512];
        let dm = parse(&dm_bytes(&format, 5512, &[(T0, &block)])).unwrap();
        assert_eq!(dm.header.wave.codec_name(), "pcm_s16le");
        assert!(dm.warnings.is_empty());

        let block = vec![0u8; 4000];
        let dm = parse(&dm_bytes(&format, 4000, &[(T0, &block)])).unwrap();
        assert_eq!(dm.warnings.len(), 1, "{:?}", dm.warnings);
        assert!(dm.warnings[0].contains("5512"));
    }

    #[test]
    fn a_truncated_or_unknown_header_is_refused_with_the_plain_message() {
        let block = vec![0u8; 520];
        let good = dm_bytes(&gsm_format(), 520, &[(T0, &block)]);
        for cut in [0, 4, 15, 16, 20, 35] {
            let error = parse(&good[..cut]).unwrap_err();
            assert!(error.starts_with(DM_HEADER_UNRECOGNIZED), "cut {cut}: {error}");
            assert!(!looks_like_dm(&good[..cut]) || cut >= 32, "cut {cut}");
        }

        let mut wrong_header_size = good.clone();
        wrong_header_size[4] = 12;
        let mut tiny_format = good.clone();
        tiny_format[8..12].copy_from_slice(&15u32.to_le_bytes());
        let mut huge_format = good.clone();
        huge_format[8..12].copy_from_slice(&(MAX_FORMAT_SIZE + 1).to_le_bytes());
        let mut zero_block = good.clone();
        zero_block[12..16].copy_from_slice(&0u32.to_le_bytes());
        let mut huge_block = good.clone();
        huge_block[12..16].copy_from_slice(&(MAX_BLOCK_SIZE + 1).to_le_bytes());
        let mut no_channels = good.clone();
        no_channels[18..20].copy_from_slice(&0u16.to_le_bytes());
        let mut extra_too_long = good.clone();
        extra_too_long[32..34].copy_from_slice(&40u16.to_le_bytes());
        let mut unaligned_block = good.clone();
        unaligned_block[12..16].copy_from_slice(&521u32.to_le_bytes());
        for (name, bytes) in [
            ("header size", wrong_header_size),
            ("format size < 16", tiny_format),
            ("format size > 1 MiB", huge_format),
            ("block size 0", zero_block),
            ("block size > 16 MiB", huge_block),
            ("no channels", no_channels),
            ("cbSize past the format", extra_too_long),
            ("block size not whole codec blocks", unaligned_block),
        ] {
            let error = parse(&bytes).unwrap_err();
            assert!(error.starts_with(DM_HEADER_UNRECOGNIZED), "{name}: {error}");
            assert!(!looks_like_dm(&bytes), "{name}");
        }
    }

    #[test]
    fn a_cut_off_last_block_keeps_every_complete_block() {
        let block = vec![1u8; 520];
        let good = dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0, &block), (T0 + 1, &block)]);
        let first_two = 36 + 2 * 528;
        // Inside the third block's header, then inside its payload, then one
        // byte short of its end.
        for cut in [first_two + 3, first_two + 8, first_two + 8 + 100, good.len() - 1] {
            let dm = parse(&good[..cut]).unwrap();
            assert_eq!(dm.stamps, vec![T0, T0], "cut {cut}");
            assert_eq!(dm.truncated_tail_bytes, (cut - first_two) as u64, "cut {cut}");
            assert_eq!(dm.final_block_len(), None);
            assert_eq!(dm.warnings.len(), 1, "{:?}", dm.warnings);
            assert!(dm.warnings[0].contains("partway through block 3"), "{:?}", dm.warnings);
            assert!(dm.warnings[0].contains("2 complete blocks"), "{:?}", dm.warnings);
            assert_eq!(audio_totals(&dm), (1040, Some(2 * 8 * 320)));
            let timeline = build_timeline(&dm);
            assert_eq!(timeline.blocks_total, 2);
            assert_eq!(timeline.truncated_tail_bytes, (cut - first_two) as u64);
            assert!(!timeline.short_final_block);
            assert_eq!(queue_info(&dm).truncated_tail_bytes, (cut - first_two) as u64);
        }
        // A complete file reports no tail.
        let dm = parse(&good).unwrap();
        assert_eq!(dm.truncated_tail_bytes, 0);
        assert_eq!(build_timeline(&dm).truncated_tail_bytes, 0);
        assert!(dm.warnings.is_empty());

        // Reading stops at a short block, so a cut after it is never reached:
        // the bytes after the short block are counted as such instead.
        let short = vec![2u8; 130];
        let bytes = dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0, &short), (T0, &block)]);
        let dm = parse(&bytes[..bytes.len() - 10]).unwrap();
        assert_eq!(dm.final_block_len(), Some(130));
        assert_eq!(dm.truncated_tail_bytes, 0);
        assert_eq!(dm.bytes_after_short_block, 518);
        assert_eq!(audio_totals(&dm).0, 650);
    }

    #[test]
    fn a_damaged_length_in_the_cut_off_last_block_keeps_every_complete_block() {
        let block = vec![1u8; 520];
        let good = dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0, &block), (T0 + 1, &block)]);
        let third = 36 + 2 * 528;
        for cut in [third + 8, third + 8 + 100] {
            let mut bytes = good[..cut].to_vec();
            bytes[third + 4..third + 8].copy_from_slice(&u32::MAX.to_le_bytes());
            let dm = parse(&bytes).unwrap();
            assert_eq!(dm.stamps, vec![T0, T0], "cut {cut}");
            assert_eq!(dm.truncated_tail_bytes, (cut - third) as u64);
            assert!(dm.warnings[0].contains("partway through block 3"), "{:?}", dm.warnings);
        }
        // The same length on a block the file holds in full is refused.
        let mut bytes = good.clone();
        bytes[36 + 528 + 4..36 + 528 + 8].copy_from_slice(&521u32.to_le_bytes());
        bytes.push(0);
        let error = parse(&bytes).unwrap_err();
        assert!(
            error.contains("Block 2") && error.contains("unexpected length"),
            "{error}"
        );
    }

    #[test]
    fn a_format_size_that_does_not_match_the_format_is_refused() {
        let block = vec![1u8; 520];
        // 64 extra bytes after a GSM WAVEFORMATEX that says it has 2.
        let mut padded = gsm_format();
        padded.extend_from_slice(&[0xEE; 64]);
        // A GSM format with no cbSize field (16 bytes), and a 17-byte PCM one.
        let short_gsm = gsm_format()[..16].to_vec();
        let mut odd_pcm = pcm_format(8000)[..16].to_vec();
        odd_pcm.push(0);
        for (name, format) in [
            ("trailing bytes", padded),
            ("non-PCM without cbSize", short_gsm),
            ("17-byte format", odd_pcm),
        ] {
            let bytes = dm_bytes(&format, 520, &[(T0, &block)]);
            let error = parse(&bytes).unwrap_err();
            assert!(error.starts_with(DM_HEADER_UNRECOGNIZED), "{name}: {error}");
            assert!(!looks_like_dm(&bytes), "{name}");
        }
        // A 16-byte PCM format (the older layout) and an 18-byte one are taken.
        for format in [pcm_format(8000)[..16].to_vec(), pcm_format(8000)] {
            let dm = parse(&dm_bytes(&format, 4000, &[(T0, &[0u8; 4000])])).unwrap();
            assert_eq!(dm.header.data_start(), 16 + format.len() as u64);
        }
    }

    #[test]
    fn formats_ffmpeg_cannot_decode_are_refused_with_the_plain_message() {
        let block = vec![1u8; 2000];
        let gsm = |channels: u16, align: u16| {
            wave_format_bytes(
                WAVE_FORMAT_GSM610,
                channels,
                11025,
                2239,
                align,
                0,
                &320u16.to_le_bytes(),
            )
        };
        let cases = [
            ("GSM block alignment 1", gsm(1, 1), 520),
            ("GSM block alignment 2", gsm(1, 2), 520),
            ("GSM stereo", gsm(2, 65), 520),
            (
                "A-law alignment 2 for mono",
                wave_format_bytes(WAVE_FORMAT_ALAW, 1, 8000, 8000, 2, 8, &[]),
                2000,
            ),
            (
                "mu-law alignment 1 for stereo",
                wave_format_bytes(WAVE_FORMAT_MULAW, 2, 8000, 16000, 1, 8, &[]),
                2000,
            ),
            (
                "float 16-bit",
                wave_format_bytes(WAVE_FORMAT_IEEE_FLOAT, 1, 8000, 16000, 2, 16, &[]),
                2000,
            ),
            (
                "MS ADPCM 8-bit",
                wave_format_bytes(WAVE_FORMAT_ADPCM, 1, 8000, 4096, 256, 8, &500u16.to_le_bytes()),
                512,
            ),
            (
                "MS ADPCM block smaller than its header",
                wave_format_bytes(WAVE_FORMAT_ADPCM, 1, 8000, 4096, 4, 4, &2u16.to_le_bytes()),
                512,
            ),
            (
                "IMA ADPCM 8-bit",
                wave_format_bytes(WAVE_FORMAT_IMA_ADPCM, 1, 8000, 4096, 256, 8, &505u16.to_le_bytes()),
                512,
            ),
            (
                "IMA ADPCM block no bigger than its header",
                wave_format_bytes(WAVE_FORMAT_IMA_ADPCM, 1, 8000, 4096, 4, 4, &1u16.to_le_bytes()),
                512,
            ),
            (
                "extensible with too few extra bytes",
                wave_format_bytes(WAVE_FORMAT_EXTENSIBLE, 1, 8000, 16000, 2, 16, &[0; 4]),
                2000,
            ),
        ];
        for (name, format, block_size) in cases {
            let bytes = dm_bytes(&format, block_size, &[(T0, &block[..block_size as usize])]);
            let error = parse(&bytes).unwrap_err();
            assert!(error.starts_with(DM_HEADER_UNRECOGNIZED), "{name}: {error}");
            assert!(!looks_like_dm(&bytes), "{name}");
        }
        // A tag DepoAudio has no checks for is left to FFmpeg.
        let other = wave_format_bytes(0x00ff, 1, 8000, 8000, 1, 8, &[]);
        assert!(parse(&dm_bytes(&other, 2000, &[(T0, &[0u8; 2000])])).is_ok());
    }

    #[test]
    fn samples_per_codec_block_follow_the_alignment_not_the_header_field() {
        // GSM with a field of 160: FFmpeg's gsm_ms still gives 320 a block.
        let format = wave_format_bytes(WAVE_FORMAT_GSM610, 1, 11025, 2239, 65, 0, &160u16.to_le_bytes());
        let block = vec![1u8; 520];
        let dm = parse(&dm_bytes(&format, 520, &[(T0, &block), (T0, &block)])).unwrap();
        assert_eq!(dm.header.wave.samples_per_codec_block(), Some(320));
        assert_eq!(audio_totals(&dm), (1040, Some(2 * 8 * 320)));
        assert_eq!(dm.warnings.len(), 1, "{:?}", dm.warnings);
        assert!(
            dm.warnings[0].contains("160") && dm.warnings[0].contains("320"),
            "{:?}",
            dm.warnings
        );
        // The default header says 320 and gets no warning.
        let dm = parse(&dm_bytes(&gsm_format(), 520, &[(T0, &block)])).unwrap();
        assert!(dm.warnings.is_empty(), "{:?}", dm.warnings);

        let ms = |channels: u16, align: u16, field: u16| {
            let format = wave_format_bytes(WAVE_FORMAT_ADPCM, channels, 8000, 4096, align, 4, &field.to_le_bytes());
            parse_wave_format(&format, format.len() as u32)
                .unwrap()
                .samples_per_codec_block()
        };
        assert_eq!(ms(1, 256, 500), Some(500));
        assert_eq!(ms(1, 256, 9), Some(500));
        assert_eq!(ms(2, 512, 500), Some(500));
        assert_eq!(ms(1, 1024, 2036), Some(2036));
        let ima = |channels: u16, align: u16, bits: u16| {
            let format = wave_format_bytes(
                WAVE_FORMAT_IMA_ADPCM,
                channels,
                8000,
                4096,
                align,
                bits,
                &7u16.to_le_bytes(),
            );
            parse_wave_format(&format, format.len() as u32)
                .unwrap()
                .samples_per_codec_block()
        };
        assert_eq!(ima(1, 256, 4), Some(505));
        assert_eq!(ima(2, 512, 4), Some(505));
        assert_eq!(ima(1, 1024, 4), Some(2041));
        assert_eq!(ima(1, 256, 3), Some(1 + 21 * 32));
    }

    #[test]
    fn a_file_with_no_complete_block_is_refused() {
        let block = vec![1u8; 520];
        let good = dm_bytes(&gsm_format(), 520, &[(T0, &block)]);
        for cut in [36 + 3, 36 + 8, 36 + 8 + 519] {
            let error = parse(&good[..cut]).unwrap_err();
            assert!(error.contains("partway through its first block"), "cut {cut}: {error}");
        }
        // A file with a header and no blocks.
        assert!(parse(&good[..36]).unwrap_err().contains("no audio blocks"));
        // A complete block header with a length longer than the block size
        // does not match the layout.
        let long = vec![0u8; 585];
        assert!(parse(&dm_bytes(&gsm_format(), 520, &[(T0, &long)]))
            .unwrap_err()
            .contains("unexpected length"));
        // An empty block is a short block like any other: reading stops
        // there, and here that leaves no audio (preparation then refuses it).
        let dm = parse(&dm_bytes(&gsm_format(), 520, &[(T0, &[]), (T0, &block)])).unwrap();
        assert_eq!(dm.final_block_len(), Some(0));
        assert_eq!(dm.bytes_after_short_block, 528);
        assert_eq!(audio_totals(&dm).0, 0);
    }

    #[test]
    fn reading_stops_at_a_short_block_before_the_end() {
        let block = vec![1u8; 520];
        // 100 bytes: one 65-byte GSM block and 35 stray bytes.
        let short = vec![2u8; 100];
        let bytes = dm_bytes(
            &gsm_format(),
            520,
            &[(T0, &block), (T0, &short), (T0 + 1, &block), (T0 + 1, &block)],
        );
        let dm = parse(&bytes).unwrap();
        // SoundSync's player stops at the short block; so does DepoAudio.
        assert_eq!(dm.stamps, vec![T0, T0]);
        assert_eq!(dm.final_block_len(), Some(100));
        assert_eq!(dm.block_len(0), 520);
        assert_eq!(dm.block_len(1), 100);
        assert_eq!(dm.bytes_after_short_block, 2 * 528);
        assert_eq!(dm.truncated_tail_bytes, 0);
        assert_eq!(dm.warnings.len(), 1, "{:?}", dm.warnings);
        assert!(dm.warnings[0].starts_with("Block 2 is shorter"), "{:?}", dm.warnings);
        assert!(
            dm.warnings[0].contains("the 1056 bytes after it were not read"),
            "{:?}",
            dm.warnings
        );
        assert!(
            dm.warnings[0].contains("kept its 1 whole codec blocks"),
            "{:?}",
            dm.warnings
        );
        assert_eq!(audio_totals(&dm), (520 + 65, Some(9 * 320)));
        let timeline = build_timeline(&dm);
        assert!(timeline.short_final_block);
        assert_eq!(timeline.last_block_bytes_dropped, 35);
        assert_eq!(timeline.bytes_after_short_block, 1056);
        assert_eq!(timeline.blocks_total, 2);
        assert_eq!(timeline.segments.len(), 1);
        assert_eq!(timeline.segments[0].blocks, 2);
        assert_eq!(queue_info(&dm).bytes_after_short_block, 1056);

        // What follows a short block is not read at all, so a damaged or
        // unexpected block after it does not matter, and its stamps (which
        // SoundSync's record-over, relying on fixed block spacing, may not
        // have zeroed where they are) never reach the audio.
        let long = vec![3u8; 600];
        let dm = parse(&dm_bytes(
            &gsm_format(),
            520,
            &[(T0, &block), (T0, &short), (T0, &long)],
        ))
        .unwrap();
        assert_eq!(dm.stamps.len(), 2);
        assert_eq!(dm.bytes_after_short_block, 608);

        // A recorded-over short block adds nothing; reading still stops there.
        let dm = parse(&dm_bytes(
            &gsm_format(),
            520,
            &[(T0, &block), (0, &short), (T0, &block)],
        ))
        .unwrap();
        let timeline = build_timeline(&dm);
        assert_eq!(timeline.blocks_erased, 1);
        assert!(!timeline.short_final_block);
        assert_eq!(timeline.bytes_after_short_block, 528);
        assert_eq!(audio_totals(&dm).0, 520);
        assert!(dm.warnings[0].contains("marked as recorded over"), "{:?}", dm.warnings);
    }

    #[test]
    fn erased_blocks_are_counted_and_skipped() {
        let block = vec![1u8; 520];
        let dm = parse(&dm_bytes(
            &gsm_format(),
            520,
            &[(T0, &block), (0, &block), (0, &block), (T0 + 1, &block)],
        ))
        .unwrap();
        let timeline = build_timeline(&dm);
        assert_eq!(timeline.blocks_total, 4);
        assert_eq!(timeline.blocks_erased, 2);
        assert_eq!(timeline.segments.len(), 1);
        assert_eq!(timeline.segments[0].blocks, 2);
        assert_eq!(audio_totals(&dm), (1040, Some(2 * 8 * 320)));
        assert_eq!(queue_info(&dm).blocks_erased, 2);
    }

    #[test]
    fn a_short_final_block_keeps_whole_codec_blocks_only() {
        let block = vec![1u8; 520];
        // 130 bytes is exactly two 65-byte GSM blocks.
        let whole = vec![2u8; 130];
        let dm = parse(&dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0, &whole)])).unwrap();
        assert_eq!(dm.final_block_len(), Some(130));
        assert_eq!(dm.bytes_after_short_block, 0);
        // Kept, but not silently: SoundSync's player would not play it.
        assert_eq!(dm.warnings.len(), 1, "{:?}", dm.warnings);
        assert!(
            dm.warnings[0].starts_with("The last block is shorter"),
            "{:?}",
            dm.warnings
        );
        assert!(dm.warnings[0].contains("does not play"), "{:?}", dm.warnings);
        let timeline = build_timeline(&dm);
        assert!(timeline.short_final_block);
        assert_eq!(timeline.last_block_bytes_dropped, 0);
        assert_eq!(audio_totals(&dm), (650, Some(10 * 320)));

        // 100 bytes is one GSM block and 35 stray bytes.
        let partial = vec![2u8; 100];
        let dm = parse(&dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0, &partial)])).unwrap();
        let timeline = build_timeline(&dm);
        assert!(timeline.short_final_block);
        assert_eq!(timeline.last_block_bytes_dropped, 35);
        assert_eq!(audio_totals(&dm), (585, Some(9 * 320)));

        // A recorded-over short last block is counted as erased, not as a
        // short block whose bytes were cut.
        let dm = parse(&dm_bytes(&gsm_format(), 520, &[(T0, &block), (0, &partial)])).unwrap();
        assert_eq!(dm.final_block_len(), Some(100));
        assert!(dm.warnings.is_empty(), "{:?}", dm.warnings);
        let timeline = build_timeline(&dm);
        assert!(!timeline.short_final_block);
        assert_eq!(timeline.last_block_bytes_dropped, 0);
        assert_eq!(timeline.blocks_erased, 1);
        assert!(!queue_info(&dm).short_final_block);
        assert_eq!(audio_totals(&dm), (520, Some(8 * 320)));

        // Less than one codec block: no audio from it, and no segment.
        let crumb = vec![2u8; 10];
        let dm = parse(&dm_bytes(&gsm_format(), 520, &[(T0, &block), (T0 + 30, &crumb)])).unwrap();
        let timeline = build_timeline(&dm);
        assert_eq!(timeline.last_block_bytes_dropped, 10);
        assert_eq!(timeline.segments.len(), 1);
        assert_eq!(audio_totals(&dm).0, 520);
    }

    #[test]
    fn backwards_and_jumping_stamps_start_segments() {
        let block = vec![1u8; 520];
        let blocks = [
            (T0, &block[..]),
            (T0 + 1, &block[..]),
            (T0 + 3, &block[..]),  // +2 s: same segment
            (T0 + 60, &block[..]), // pause
            (T0 + 61, &block[..]),
            (T0 - 100, &block[..]), // a clock set back
        ];
        let dm = parse(&dm_bytes(&gsm_format(), 520, &blocks)).unwrap();
        let timeline = build_timeline(&dm);
        assert!(!timeline.stamps_monotonic);
        let per_block = 8 * 320;
        let starts: Vec<(Option<u64>, u64)> = timeline
            .segments
            .iter()
            .map(|segment| (segment.start_sample, segment.blocks))
            .collect();
        assert_eq!(
            starts,
            vec![(Some(0), 3), (Some(3 * per_block), 2), (Some(5 * per_block), 1)]
        );
        assert_eq!(timeline.segments[1].first_stamp_utc, "2025-11-22T08:27:40Z");
        assert_eq!(timeline.segments[2].first_stamp_utc, "2025-11-22T08:25:00Z");
        assert!(!queue_info(&dm).stamps_monotonic);
    }

    #[test]
    fn many_segments_are_capped_in_the_record() {
        let block = [1u8; 65];
        let format = wave_format_bytes(WAVE_FORMAT_GSM610, 1, 8000, 1625, 65, 0, &320u16.to_le_bytes());
        let blocks: Vec<(u32, &[u8])> = (0..(MAX_RECORDED_SEGMENTS as u32 + 5))
            .map(|index| (T0 + index * 10, &block[..]))
            .collect();
        let dm = parse(&dm_bytes(&format, 65, &blocks)).unwrap();
        let timeline = build_timeline(&dm);
        assert_eq!(timeline.segments.len(), MAX_RECORDED_SEGMENTS);
        assert_eq!(timeline.segments_omitted, 5);
        // 65-byte blocks are not the usual 390 for this format.
        assert_eq!(dm.warnings.len(), 1);
    }

    #[test]
    fn garbage_never_panics() {
        // Deterministic pseudo-random bytes, with and without a valid header
        // in front.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        };
        let header = dm_bytes(&gsm_format(), 520, &[]);
        for round in 0..400 {
            let len = (round * 37) % 3000;
            let noise: Vec<u8> = (0..len).map(|_| next()).collect();
            let _ = parse(&noise);
            let _ = looks_like_dm(&noise);
            let mut framed = header.clone();
            framed.extend_from_slice(&noise);
            let _ = parse(&framed);
        }
        assert!(!looks_like_dm(
            b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0"
        ));
    }

    #[test]
    fn cancellation_stops_the_parse() {
        let block = vec![1u8; 520];
        let bytes = dm_bytes(&gsm_format(), 520, &[(T0, &block)]);
        let error = parse_dm_reader(Cursor::new(&bytes), bytes.len() as u64, &|| true).unwrap_err();
        assert_eq!(error, crate::helpers::AUDIO_PREPARATION_CANCELLED_MESSAGE);
    }

    #[test]
    fn riff_layout_pads_and_refuses_past_4_gib() {
        let layout = riff_layout(20, false, Some(640), 130).unwrap();
        assert_eq!(layout.fact_samples, Some(640));
        assert!(!layout.fmt_padding);
        assert_eq!(layout.riff_size, 4 + 8 + 20 + 12 + 8 + 130);

        let layout = riff_layout(19, true, Some(65), 65).unwrap();
        assert!(layout.fmt_padding && layout.data_padding);
        assert_eq!(layout.fact_samples, None);
        assert_eq!(layout.riff_size, 4 + 8 + 20 + 8 + 66);

        assert_eq!(
            riff_layout(18, true, None, u64::from(u32::MAX)).unwrap_err(),
            DM_TOO_LARGE
        );
        // The largest even data size that fits, then the next even one.
        let limit = u64::from(u32::MAX) - (4 + 8 + 18 + 8) - 1;
        assert_eq!(riff_layout(18, true, None, limit).unwrap().riff_size, u32::MAX - 1);
        assert_eq!(riff_layout(18, true, None, limit + 2).unwrap_err(), DM_TOO_LARGE);
    }

    /// The committed `.dm` fixtures and the WAV dm.rs writes for each. The
    /// release smoke (`scripts/container-smoke-fixtures.mjs`) feeds these
    /// same WAVs to the bundled FFmpeg sidecars and checks that its own
    /// reader gives the same bytes, so a change here that alters the WAV
    /// shows up in both places. Regenerate the WAVs with
    /// `DEPOAUDIO_WRITE_DM_GOLDEN=1 cargo test golden_dm` after a deliberate
    /// change, and say why in the commit.
    pub(crate) const GOLDEN_DM: [&str; 5] = [
        "gsm-default",
        "ima-adpcm-damaged",
        "adpcm-ms",
        "mulaw-odd-format",
        "pcm-u8-odd",
    ];

    fn golden_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fixtures/dm")
    }

    #[test]
    fn golden_dm_fixtures_demux_to_the_committed_wavs() {
        let regenerate = std::env::var_os("DEPOAUDIO_WRITE_DM_GOLDEN").is_some_and(|value| value == "1");
        for name in GOLDEN_DM {
            let source = golden_dir().join(format!("{name}.dm"));
            let dm = parse_dm(&source, &|| false).unwrap_or_else(|error| panic!("{name}: {error}"));
            let mut wav = Vec::new();
            write_wav(&source, &dm, &mut wav, &|| false).unwrap();
            let golden = golden_dir().join(format!("{name}.wav"));
            if regenerate {
                fs::write(&golden, &wav).unwrap();
            }
            assert!(
                fs::read(&golden).unwrap() == wav,
                "{name}: dm.rs no longer writes the committed {name}.wav"
            );
        }
    }

    #[test]
    fn golden_dm_fixtures_have_the_expected_timelines() {
        let timeline = |name: &str| {
            let dm = parse_dm(&golden_dir().join(format!("{name}.dm")), &|| false).unwrap();
            (build_timeline(&dm), audio_totals(&dm))
        };
        // SoundSync default header, real GSM frames: an erased block, a pause,
        // stamps that go backwards and a 160-byte last block (two codec blocks
        // and 30 stray bytes).
        let (gsm, totals) = timeline("gsm-default");
        assert_eq!((gsm.blocks_total, gsm.blocks_erased), (9, 1));
        assert!(gsm.short_final_block && !gsm.stamps_monotonic);
        assert_eq!(gsm.last_block_bytes_dropped, 30);
        let starts: Vec<_> = gsm.segments.iter().map(|segment| segment.start_sample).collect();
        assert_eq!(starts, vec![Some(0), Some(4 * 2560), Some(6 * 2560)]);
        assert_eq!(totals, (7 * 520 + 130, Some(7 * 2560 + 640)));
        // The short last block is kept, with a warning that SoundSync's
        // player would not play it.
        assert_eq!(gsm.warnings.len(), 1, "{:?}", gsm.warnings);
        assert!(gsm.warnings[0].starts_with("The last block is shorter"));
        // IMA ADPCM: a short block before the end (reading stops after it, as
        // SoundSync's player does) and, after it, a cut-off last block that
        // is never reached.
        let (ima, totals) = timeline("ima-adpcm-damaged");
        assert_eq!(ima.blocks_total, 2);
        assert!(ima.short_final_block);
        assert_eq!(ima.last_block_bytes_dropped, 10);
        assert_eq!(ima.bytes_after_short_block, 2 * 2056 + 108);
        assert_eq!(ima.truncated_tail_bytes, 0);
        assert_eq!(totals, (3 * 1024, Some(3 * 2041)));
        // The unusual block size, and the short block.
        assert_eq!(ima.warnings.len(), 2, "{:?}", ima.warnings);
        // MS ADPCM with its 50-byte format: an erased block and a short last
        // block of whole codec blocks.
        let (ms, totals) = timeline("adpcm-ms");
        assert_eq!((ms.blocks_erased, ms.last_block_bytes_dropped), (1, 0));
        assert!(ms.short_final_block);
        assert_eq!(totals, (5120, Some(5 * 2036)));
        // mu-law with a 19-byte format (padded in the WAV) and a pause.
        let (mulaw, totals) = timeline("mulaw-odd-format");
        assert_eq!(mulaw.segments.len(), 2);
        assert_eq!(totals, (4777, Some(4777)));
        // 8-bit PCM with a 16-byte format and an odd amount of audio.
        let (pcm, totals) = timeline("pcm-u8-odd");
        assert_eq!(pcm.blocks_erased, 1);
        assert_eq!(totals, (5001, Some(5001)));
    }

    #[test]
    fn write_wav_keeps_file_order_and_skips_erased_blocks() {
        let dir = std::env::temp_dir().join(format!("depoaudio_dm_test_{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("session.dm");
        let format = pcm_format(8000);
        // (8000*2/2) >> 2 = 2000 frames = 4000 bytes.
        let a = vec![0x11u8; 4000];
        let b = vec![0x22u8; 4000];
        let c = vec![0x33u8; 4000];
        let d = vec![0x44u8; 6];
        fs::write(
            &path,
            dm_bytes(&format, 4000, &[(T0 + 5, &a), (0, &b), (T0, &c), (T0, &d)]),
        )
        .unwrap();
        let dm = parse_dm(&path, &|| false).unwrap();
        let mut out = Vec::new();
        write_wav(&path, &dm, &mut out, &|| false).unwrap();
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(u32_at(&out, 4) as usize, out.len() - 8);
        assert_eq!(&out[8..16], b"WAVEfmt ");
        assert_eq!(u32_at(&out, 16), 18);
        assert_eq!(&out[20..38], &format[..]);
        assert_eq!(&out[38..42], b"data");
        assert_eq!(u32_at(&out, 42), 8006);
        let data = &out[46..];
        assert_eq!(&data[..4000], &a[..]);
        assert_eq!(&data[4000..8000], &c[..]);
        assert_eq!(&data[8000..], &d[..]);
        let timeline = build_timeline(&dm);
        assert!(!timeline.stamps_monotonic);
        assert_eq!(timeline.segments.len(), 2);

        // A file that changes after the parse is refused.
        fs::write(
            &path,
            dm_bytes(&format, 4000, &[(T0 + 6, &a), (0, &b), (T0, &c), (T0, &d)]),
        )
        .unwrap();
        assert!(write_wav(&path, &dm, &mut Vec::new(), &|| false)
            .unwrap_err()
            .contains("changed"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
