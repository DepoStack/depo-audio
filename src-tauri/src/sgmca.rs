//! Stenograph SGMCA ("Direct Audio") recordings: reading, checking, slicing
//! and writing.
//!
//! The layout below comes from owner-supplied interoperability notes and has
//! not yet been confirmed against a file CATalyst wrote (see docs/SGMCA.md).
//! All integers are little-endian.
//!
//! * A 40-byte file header: its own size (40), a format version (1), a
//!   16-byte recording identifier and 16 reserved bytes.
//! * Then one or more segments, one per recording session, in the order they
//!   were recorded. Each is a 56-byte segment header (its own size 56,
//!   version 1, record kind 2 for audio, a reserved word, the local
//!   wall-clock start as 32 NUL-padded ASCII bytes `YYYY-MM-DDTHH:MM:SS±hhmm`,
//!   and the byte length of the payload as a u64), followed by the payload:
//!   a complete, independent Ogg stream (RFC 3533) carrying one Opus stream
//!   (RFC 7845).
//!
//! A live recording uses Opus channel mapping family 255 with one uncoupled
//! mono stream per court channel; an imported file is said to use family 1
//! with the same layout. Earlier DepoAudio builds wrote, and read, a bare Ogg
//! Opus stream with no headers at all; such a file is still read, and named
//! as that older layout.
//!
//! This module works from those notes and the public specifications only:
//!
//! * the Ogg page layout and its CRC-32 (polynomial 0x04C11DB7, MSB first,
//!   initial value 0, no final XOR, CRC field zeroed while it is computed);
//! * the `OpusHead` and `OpusTags` header packets;
//! * the Opus TOC byte (RFC 6716 section 3.1), which gives every packet's
//!   duration without decoding it.
//!
//! Decoding to PCM stays with FFmpeg, which handles multistream Opus. What
//! this module adds is everything FFmpeg cannot tell us cheaply or exactly:
//! where each session's stream is, whether its headers are sound, the exact
//! duration of each session (last granule position minus pre-skip), the
//! true channel layout, and a sample-exact slice that copies Opus packets
//! without re-encoding them.
//!
//! Granule positions and pre-skip are always counted at 48 kHz, whatever the
//! informational input sample rate in `OpusHead` says.

use std::fs;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::OnceLock;

use serde::Serialize;

/// Opus timestamps (granule positions, pre-skip) always run at 48 kHz.
pub(crate) const OPUS_RATE: u64 = 48_000;

/// Size of the Direct Audio file header.
pub(crate) const FILE_HEADER_LEN: u64 = 40;

/// Size of each segment header.
pub(crate) const SEGMENT_HEADER_LEN: u64 = 56;

/// The only file and segment header version the notes describe. CATalyst is
/// said to refuse a file version above 1 and to find no playback position
/// unless it is exactly 1.
const FORMAT_VERSION: u32 = 1;
const SEGMENT_VERSION: u32 = 1;

/// Segment record kind for audio (the only kind the notes describe).
const SEGMENT_KIND_AUDIO: u32 = 2;

/// Bytes of the start-time text field in a segment header.
const START_TEXT_LEN: usize = 32;

/// Decoder pre-roll kept in front of a slice's first sample (80 ms), as RFC
/// 7845 section 4.6 recommends, so the decoder has converged by the cut.
pub(crate) const PREROLL_SAMPLES: u64 = 3840;

/// Ogg page header flags.
const FLAG_CONTINUED: u8 = 0x01;
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;

/// The fixed part of an Ogg page header, before the segment table.
const PAGE_HEADER_LEN: usize = 27;

/// A granule position of all ones means "no packet finishes on this page".
const NO_GRANULE: u64 = u64::MAX;

/// Pages written by this module are flushed once their body reaches this size
/// or holds this many packets (one second of 20 ms frames), whichever comes
/// first, which keeps seeking fine-grained without wasting header bytes.
const TARGET_PAGE_BODY: usize = 8192;
const TARGET_PAGE_PACKETS: usize = 50;

pub(crate) type CancelCheck<'a> = Option<&'a (dyn Fn() -> bool + Send + Sync)>;

pub(crate) const SLICE_CANCELLED_MESSAGE: &str = "SGMCA slicing cancelled";
pub(crate) const INSPECT_CANCELLED_MESSAGE: &str = "SGMCA inspection cancelled";
pub(crate) const WRITE_CANCELLED_MESSAGE: &str = "SGMCA writing cancelled";

// ── CRC-32 ───────────────────────────────────────────────────────────────────

fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            let mut value = (index as u32) << 24;
            for _ in 0..8 {
                value = if value & 0x8000_0000 != 0 {
                    (value << 1) ^ 0x04C1_1DB7
                } else {
                    value << 1
                };
            }
            *entry = value;
        }
        table
    })
}

fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    let table = crc_table();
    for &byte in data {
        crc = (crc << 8) ^ table[((crc >> 24) as u8 ^ byte) as usize];
    }
    crc
}

/// The Ogg CRC-32 of `data` (a whole page with its CRC field zeroed).
pub(crate) fn ogg_crc(data: &[u8]) -> u32 {
    crc_update(0, data)
}

// ── Pages ────────────────────────────────────────────────────────────────────

/// One Ogg page as read from a file.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub header_type: u8,
    pub granule: u64,
    pub serial: u32,
    pub sequence: u32,
    pub lacing: Vec<u8>,
    pub body: Vec<u8>,
    /// Byte offset of the page in the file, for error messages.
    pub offset: u64,
}

impl Page {
    pub(crate) fn is_bos(&self) -> bool {
        self.header_type & FLAG_BOS != 0
    }
    pub(crate) fn is_eos(&self) -> bool {
        self.header_type & FLAG_EOS != 0
    }
    pub(crate) fn is_continued(&self) -> bool {
        self.header_type & FLAG_CONTINUED != 0
    }
    pub(crate) fn granule(&self) -> Option<u64> {
        (self.granule != NO_GRANULE).then_some(self.granule)
    }
}

/// Read exactly `buf.len()` bytes, or fewer only at end of file.
fn read_fully<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("Could not read the recording: {error}")),
        }
    }
    Ok(filled)
}

/// Reads consecutive pages strictly: every page must start where the last one
/// ended and carry a correct CRC, so damage is reported where it is.
pub(crate) struct PageReader<R: Read> {
    inner: R,
    offset: u64,
    index: u64,
    /// The input ended partway through a page.
    truncated: bool,
}

impl<R: Read> PageReader<R> {
    pub(crate) fn new(inner: R, offset: u64) -> Self {
        Self {
            inner,
            offset,
            index: 0,
            truncated: false,
        }
    }

    /// Byte offset just past the last complete page read.
    pub(crate) fn position(&self) -> u64 {
        self.offset
    }

    pub(crate) fn next_page(&mut self) -> Result<Option<Page>, String> {
        let at = self.offset;
        let page_number = self.index + 1;
        let mut header = [0u8; PAGE_HEADER_LEN];
        let read = read_fully(&mut self.inner, &mut header)?;
        if read == 0 {
            return Ok(None);
        }
        if read < PAGE_HEADER_LEN {
            self.truncated = true;
            return Err(format!(
                "The recording ends partway through Ogg page {page_number} (byte {at}); the file looks truncated."
            ));
        }
        if &header[..4] != b"OggS" {
            return Err(format!(
                "Expected Ogg page {page_number} at byte {at}, but found other data; the file is damaged."
            ));
        }
        if header[4] != 0 {
            return Err(format!(
                "Ogg page {page_number} (byte {at}) uses unknown stream version {}.",
                header[4]
            ));
        }
        let mut lacing = vec![0u8; header[26] as usize];
        if read_fully(&mut self.inner, &mut lacing)? < lacing.len() {
            self.truncated = true;
            return Err(format!(
                "The recording ends partway through Ogg page {page_number} (byte {at}); the file looks truncated."
            ));
        }
        let body_len: usize = lacing.iter().map(|&value| value as usize).sum();
        let mut body = vec![0u8; body_len];
        if read_fully(&mut self.inner, &mut body)? < body_len {
            self.truncated = true;
            return Err(format!(
                "The recording ends partway through Ogg page {page_number} (byte {at}); the file looks truncated."
            ));
        }
        let stored = u32::from_le_bytes(header[22..26].try_into().unwrap());
        let mut crc = crc_update(0, &header[..22]);
        crc = crc_update(crc, &[0; 4]);
        crc = crc_update(crc, &header[26..]);
        crc = crc_update(crc, &lacing);
        crc = crc_update(crc, &body);
        if crc != stored {
            return Err(format!(
                "Ogg page {page_number} (byte {at}) fails its checksum; the file is damaged there."
            ));
        }
        self.offset += (PAGE_HEADER_LEN + lacing.len() + body_len) as u64;
        self.index += 1;
        Ok(Some(Page {
            header_type: header[5],
            granule: u64::from_le_bytes(header[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(header[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(header[18..22].try_into().unwrap()),
            lacing,
            body,
            offset: at,
        }))
    }
}

/// The packets that finish on one page of the Opus stream, with that page's
/// granule position (the end of its last finished packet).
pub(crate) struct PagePackets {
    pub packets: Vec<Vec<u8>>,
    pub granule: Option<u64>,
    pub eos: bool,
    /// Byte offset of the page in the file, for error messages.
    pub offset: u64,
}

/// Reassembles the packets of the first logical stream from its pages,
/// following lacing values across page boundaries. Pages of any other
/// logical stream are skipped; reading stops at the stream's end-of-stream
/// page, so a chained recording yields only its first link.
pub(crate) struct PacketReader<R: Read> {
    pages: PageReader<R>,
    serial: Option<u32>,
    partial: Vec<u8>,
    partial_open: bool,
    next_sequence: Option<u32>,
    finished: bool,
    saw_eos: bool,
    /// The input ended inside a packet that never finished.
    ended_mid_packet: bool,
    pub pages_read: u64,
}

impl<R: Read> PacketReader<R> {
    pub(crate) fn new(inner: R, offset: u64) -> Self {
        Self {
            pages: PageReader::new(inner, offset),
            serial: None,
            partial: Vec::new(),
            partial_open: false,
            next_sequence: None,
            finished: false,
            saw_eos: false,
            ended_mid_packet: false,
            pages_read: 0,
        }
    }

    /// Byte offset just past the last complete page read.
    pub(crate) fn position(&self) -> u64 {
        self.pages.position()
    }

    /// The stream's end-of-stream page was read.
    pub(crate) fn saw_eos(&self) -> bool {
        self.saw_eos
    }

    /// The input ended partway through a page or a packet, as a file that is
    /// still being written (or was cut short) does.
    pub(crate) fn truncated(&self) -> bool {
        self.pages.truncated || self.ended_mid_packet
    }

    /// The next page of the stream that finishes at least one packet, or None
    /// after the end of the stream.
    pub(crate) fn next(&mut self) -> Result<Option<PagePackets>, String> {
        loop {
            if self.finished {
                return Ok(None);
            }
            let Some(page) = self.pages.next_page()? else {
                self.finished = true;
                if self.partial_open {
                    self.ended_mid_packet = true;
                    return Err(
                        "The recording ends in the middle of an audio packet; the file looks truncated.".into(),
                    );
                }
                return Ok(None);
            };
            match self.serial {
                None => {
                    if !page.is_bos() {
                        return Err(format!(
                            "The first Ogg page (byte {}) is not the start of a stream.",
                            page.offset
                        ));
                    }
                    self.serial = Some(page.serial);
                }
                Some(serial) if serial != page.serial => continue,
                Some(_) => {}
            }
            self.pages_read += 1;
            if let Some(expected) = self.next_sequence {
                if page.sequence != expected {
                    return Err(format!(
                        "Ogg page at byte {} is out of sequence (expected page {expected}, found {}); part of the recording is missing.",
                        page.offset, page.sequence
                    ));
                }
            }
            self.next_sequence = Some(page.sequence.wrapping_add(1));

            if page.is_continued() != self.partial_open {
                return Err(if self.partial_open {
                    format!(
                        "An audio packet is cut off before the Ogg page at byte {}.",
                        page.offset
                    )
                } else {
                    format!(
                        "The Ogg page at byte {} continues a packet that never started.",
                        page.offset
                    )
                });
            }

            let mut packets = Vec::new();
            let mut cursor = 0usize;
            for &lace in &page.lacing {
                let end = cursor + lace as usize;
                self.partial.extend_from_slice(&page.body[cursor..end]);
                cursor = end;
                if lace < 255 {
                    packets.push(std::mem::take(&mut self.partial));
                    self.partial_open = false;
                } else {
                    self.partial_open = true;
                }
            }
            if page.is_eos() {
                self.finished = true;
                self.saw_eos = true;
                if self.partial_open {
                    return Err("The final Ogg page ends in the middle of an audio packet.".into());
                }
            }
            if packets.is_empty() {
                if page.is_eos() {
                    return Ok(None);
                }
                continue;
            }
            return Ok(Some(PagePackets {
                packets,
                granule: page.granule(),
                eos: page.is_eos(),
                offset: page.offset,
            }));
        }
    }
}

/// Writes one logical Ogg stream: lacing, page breaks, continued-packet and
/// BOS/EOS flags, sequence numbers and CRCs.
pub(crate) struct OggWriter<W: Write> {
    out: W,
    serial: u32,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    page_granule: u64,
    packets_on_page: usize,
    continued: bool,
    wrote_first: bool,
    pub pages_written: u64,
}

impl<W: Write> OggWriter<W> {
    pub(crate) fn new(out: W, serial: u32) -> Self {
        Self {
            out,
            serial,
            sequence: 0,
            lacing: Vec::new(),
            body: Vec::new(),
            page_granule: NO_GRANULE,
            packets_on_page: 0,
            continued: false,
            wrote_first: false,
            pages_written: 0,
        }
    }

    /// Add one packet ending at `granule`. A page fills up to 255 lacing
    /// values; a longer packet continues on the next page.
    pub(crate) fn write_packet(&mut self, packet: &[u8], granule: u64) -> Result<(), String> {
        let mut rest = packet;
        let mut inside = false;
        loop {
            if self.lacing.len() == 255 {
                // A full page: if this packet already has bytes on it, its
                // remainder continues on the next page.
                self.emit(false, inside)?;
            }
            inside = true;
            let take = rest.len().min(255);
            self.lacing.push(take as u8);
            self.body.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if take < 255 {
                break;
            }
        }
        self.page_granule = granule;
        self.packets_on_page += 1;
        Ok(())
    }

    /// Flush when the current page is full enough; callers that need a page
    /// boundary (after each header) use `flush` instead.
    pub(crate) fn maybe_flush(&mut self) -> Result<(), String> {
        if self.body.len() >= TARGET_PAGE_BODY || self.packets_on_page >= TARGET_PAGE_PACKETS {
            self.flush(false)?;
        }
        Ok(())
    }

    pub(crate) fn flush(&mut self, eos: bool) -> Result<(), String> {
        if self.lacing.is_empty() && !eos {
            return Ok(());
        }
        self.emit(eos, false)
    }

    fn emit(&mut self, eos: bool, packet_continues: bool) -> Result<(), String> {
        let mut flags = 0u8;
        if self.continued {
            flags |= FLAG_CONTINUED;
        }
        if !self.wrote_first {
            flags |= FLAG_BOS;
        }
        if eos {
            flags |= FLAG_EOS;
        }
        let granule = if self.packets_on_page == 0 {
            NO_GRANULE
        } else {
            self.page_granule
        };
        let mut page = Vec::with_capacity(PAGE_HEADER_LEN + self.lacing.len() + self.body.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(self.lacing.len() as u8);
        page.extend_from_slice(&self.lacing);
        page.extend_from_slice(&self.body);
        let crc = ogg_crc(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.out
            .write_all(&page)
            .map_err(|error| format!("Could not write the recording: {error}"))?;
        self.sequence = self.sequence.wrapping_add(1);
        self.wrote_first = true;
        self.pages_written += 1;
        self.lacing.clear();
        self.body.clear();
        // A packet that was still open when this page closed carries on: its
        // remaining bytes start the next page, which is flagged continued.
        self.continued = packet_continues;
        self.page_granule = NO_GRANULE;
        self.packets_on_page = 0;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn into_inner(self) -> W {
        self.out
    }
}

// ── Opus headers ─────────────────────────────────────────────────────────────

/// The `OpusHead` identification header (RFC 7845 section 5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpusHead {
    pub version: u8,
    pub channels: u8,
    pub pre_skip: u16,
    pub input_sample_rate: u32,
    pub output_gain: i16,
    pub mapping_family: u8,
    pub stream_count: u8,
    pub coupled_count: u8,
    pub mapping: Vec<u8>,
}

impl OpusHead {
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < 19 || &packet[..8] != b"OpusHead" {
            return Err("The stream's first packet is not an Opus identification header.".into());
        }
        let version = packet[8];
        if version >> 4 != 0 {
            return Err(format!("The Opus header uses unsupported version {version}."));
        }
        let channels = packet[9];
        if channels == 0 {
            return Err("The Opus header reports zero channels.".into());
        }
        let pre_skip = u16::from_le_bytes([packet[10], packet[11]]);
        let input_sample_rate = u32::from_le_bytes(packet[12..16].try_into().unwrap());
        let output_gain = i16::from_le_bytes([packet[16], packet[17]]);
        let mapping_family = packet[18];
        let (stream_count, coupled_count, mapping) = if mapping_family == 0 {
            if channels > 2 {
                return Err(format!(
                    "The Opus header says {channels} channels with mapping family 0, which allows at most 2."
                ));
            }
            let coupled = u8::from(channels == 2);
            (1, coupled, (0..channels).collect())
        } else {
            let table = packet
                .get(21..21 + channels as usize)
                .ok_or("The Opus header's channel mapping table is cut short.")?;
            let streams = packet[19];
            let coupled = packet[20];
            if streams == 0 {
                return Err("The Opus header reports zero streams.".into());
            }
            if coupled > streams {
                return Err("The Opus header reports more coupled streams than streams.".into());
            }
            let decoded = streams as usize + coupled as usize;
            if decoded > 255 {
                return Err("The Opus header reports too many streams.".into());
            }
            if let Some(bad) = table.iter().find(|&&entry| entry != 255 && entry as usize >= decoded) {
                return Err(format!(
                    "The Opus channel mapping points at stream channel {bad}, but only {decoded} exist."
                ));
            }
            if mapping_family == 1 && channels > 8 {
                return Err("Opus mapping family 1 allows at most 8 channels.".into());
            }
            (streams, coupled, table.to_vec())
        };
        Ok(Self {
            version,
            channels,
            pre_skip,
            input_sample_rate,
            output_gain,
            mapping_family,
            stream_count,
            coupled_count,
            mapping,
        })
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(21 + self.mapping.len());
        out.extend_from_slice(b"OpusHead");
        out.push(self.version);
        out.push(self.channels);
        out.extend_from_slice(&self.pre_skip.to_le_bytes());
        out.extend_from_slice(&self.input_sample_rate.to_le_bytes());
        out.extend_from_slice(&self.output_gain.to_le_bytes());
        out.push(self.mapping_family);
        if self.mapping_family != 0 {
            out.push(self.stream_count);
            out.push(self.coupled_count);
            out.extend_from_slice(&self.mapping);
        }
        out
    }

    /// The SGMCA layout: family 255, one uncoupled mono stream per channel,
    /// identity mapping.
    pub(crate) fn is_sgmca_layout(&self) -> bool {
        self.mapping_family == 255
            && self.stream_count == self.channels
            && self.coupled_count == 0
            && self
                .mapping
                .iter()
                .enumerate()
                .all(|(index, &entry)| entry as usize == index)
    }
}

/// The `OpusTags` comment header (RFC 7845 section 5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpusTags {
    pub vendor: String,
    pub comments: Vec<String>,
}

impl OpusTags {
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        const BAD: &str = "The Opus comment header is damaged.";
        if packet.len() < 16 || &packet[..8] != b"OpusTags" {
            return Err("The stream's second packet is not an Opus comment header.".into());
        }
        let mut at = 8usize;
        let take_len = |at: &mut usize| -> Result<usize, String> {
            let bytes = packet.get(*at..*at + 4).ok_or(BAD)?;
            *at += 4;
            Ok(u32::from_le_bytes(bytes.try_into().unwrap()) as usize)
        };
        let vendor_len = take_len(&mut at)?;
        let vendor = packet.get(at..at.checked_add(vendor_len).ok_or(BAD)?).ok_or(BAD)?;
        at += vendor_len;
        let count = take_len(&mut at)?;
        // Each comment needs at least its 4-byte length.
        if count > (packet.len() - at) / 4 {
            return Err(BAD.into());
        }
        let mut comments = Vec::with_capacity(count);
        for _ in 0..count {
            let len = take_len(&mut at)?;
            let text = packet.get(at..at.checked_add(len).ok_or(BAD)?).ok_or(BAD)?;
            at += len;
            comments.push(String::from_utf8_lossy(text).into_owned());
        }
        Ok(Self {
            vendor: String::from_utf8_lossy(vendor).into_owned(),
            comments,
        })
    }
}

/// Samples (at 48 kHz) in one Ogg Opus packet, read from its TOC byte (RFC
/// 6716 section 3.1). In a multistream packet every stream covers the same
/// time and the first stream's TOC is the packet's first byte, so the first
/// stream's duration is the packet's duration.
pub(crate) fn packet_samples(packet: &[u8]) -> Result<u64, String> {
    let toc = *packet.first().ok_or("An Opus audio packet is empty.")?;
    let config = toc >> 3;
    let frame: u64 = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let frames: u64 = match toc & 0x03 {
        0 => 1,
        1 | 2 => 2,
        _ => {
            let count = *packet.get(1).ok_or("An Opus audio packet is cut short.")? & 0x3F;
            if count == 0 {
                return Err("An Opus audio packet holds zero frames.".into());
            }
            count as u64
        }
    };
    let samples = frame * frames;
    if samples > 5760 {
        return Err("An Opus audio packet is longer than the 120 ms Opus allows.".into());
    }
    Ok(samples)
}

// ── Segment start times ──────────────────────────────────────────────────────

/// A segment's start as its header stores it, and as read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SegmentStart {
    /// The header text up to its first NUL. A byte that is not printable
    /// ASCII is shown as `?`.
    pub text: String,
    /// The local wall-clock time `YYYY-MM-DDTHH:MM:SS`, when the text parses.
    pub local: Option<String>,
    /// The stored UTC offset as `+HH:MM` or `-HH:MM`, when the text parses.
    /// CATalyst is said to read it but not apply it.
    pub utc_offset: Option<String>,
}

/// Parse `YYYY-MM-DDTHH:MM:SS±hhmm` (24 characters, the strftime pattern
/// `%Y-%m-%dT%H:%M:%S%z`) into a local date-time and an offset in minutes.
/// Anything else, or an impossible date or time, is None.
pub(crate) fn parse_start_text(text: &str) -> Option<(time::PrimitiveDateTime, i32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 24 {
        return None;
    }
    let digits = |from: usize, to: usize| -> Option<u32> {
        let part = text.get(from..to)?;
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let sign = match bytes[19] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (offset_hours, offset_minutes) = (digits(20, 22)?, digits(22, 24)?);
    if offset_hours > 23 || offset_minutes > 59 {
        return None;
    }
    let month = time::Month::try_from(u8::try_from(digits(5, 7)?).ok()?).ok()?;
    let date = time::Date::from_calendar_date(
        i32::try_from(digits(0, 4)?).ok()?,
        month,
        u8::try_from(digits(8, 10)?).ok()?,
    )
    .ok()?;
    let clock = time::Time::from_hms(
        u8::try_from(digits(11, 13)?).ok()?,
        u8::try_from(digits(14, 16)?).ok()?,
        u8::try_from(digits(17, 19)?).ok()?,
    )
    .ok()?;
    let offset = sign * i32::try_from(offset_hours * 60 + offset_minutes).ok()?;
    Some((time::PrimitiveDateTime::new(date, clock), offset))
}

/// `samples` of a second at `rate` samples per second, in milliseconds to
/// the microsecond: how far a first sample falls after the whole second a
/// segment header can hold.
pub(crate) fn offset_ms(samples: u64, rate: u64) -> f64 {
    if rate == 0 {
        return 0.0;
    }
    (samples as f64 * 1_000_000.0 / rate as f64).round() / 1_000.0
}

/// The export-record sentence for a start that falls `offset_ms` after the
/// whole second written: "CATalyst stores whole seconds; this recording's
/// first sample is 0.42 s after the time written."
pub(crate) fn whole_second_note(offset_ms: f64) -> String {
    let seconds = format!("{:.6}", offset_ms / 1_000.0);
    let seconds = seconds.trim_end_matches('0').trim_end_matches('.');
    format!("CATalyst stores whole seconds; this recording's first sample is {seconds} s after the time written.")
}

/// `YYYY-MM-DDTHH:MM:SS±hhmm` for a local wall-clock time and its offset in
/// minutes. None when the year does not fit in four digits or the offset is
/// a day or more, because the text would then not be 24 characters.
pub(crate) fn format_start_text(local: time::PrimitiveDateTime, offset_minutes: i32) -> Option<String> {
    if !(0..=9999).contains(&local.year()) || offset_minutes.unsigned_abs() >= 24 * 60 {
        return None;
    }
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let minutes = offset_minutes.unsigned_abs();
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}{:02}",
        local.year(),
        u8::from(local.month()),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
        minutes / 60,
        minutes % 60
    ))
}

impl SegmentStart {
    pub(crate) fn from_text(text: String) -> Self {
        let parsed = parse_start_text(&text);
        Self {
            local: parsed.map(|(local, _)| {
                format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                    local.year(),
                    u8::from(local.month()),
                    local.day(),
                    local.hour(),
                    local.minute(),
                    local.second()
                )
            }),
            utc_offset: parsed.map(|(_, offset)| {
                let sign = if offset < 0 { '-' } else { '+' };
                let minutes = offset.unsigned_abs();
                format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
            }),
            text,
        }
    }

    /// The start text moved later by whole seconds, keeping the stored offset.
    /// None when the text does not parse.
    pub(crate) fn shifted_text(&self, seconds: u64) -> Option<String> {
        let (local, offset) = parse_start_text(&self.text)?;
        let moved = local.checked_add(time::Duration::seconds(i64::try_from(seconds).ok()?))?;
        format_start_text(moved, offset)
    }
}

fn start_text_from_field(field: &[u8]) -> String {
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(field.len());
    field[..end]
        .iter()
        .map(|&byte| {
            if (0x20..0x7f).contains(&byte) {
                byte as char
            } else {
                '?'
            }
        })
        .collect()
}

// ── File and segment headers ─────────────────────────────────────────────────

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// The 40-byte file header: size, version, identifier, 16 reserved zeros.
/// `guid` is in the Windows byte layout (first three fields little-endian),
/// which is what `uuid::Uuid::to_bytes_le` gives.
pub(crate) fn file_header(guid: [u8; 16]) -> [u8; FILE_HEADER_LEN as usize] {
    let mut header = [0u8; FILE_HEADER_LEN as usize];
    header[0..4].copy_from_slice(&(FILE_HEADER_LEN as u32).to_le_bytes());
    header[4..8].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    header[8..24].copy_from_slice(&guid);
    header
}

/// The 56-byte segment header for a payload of `payload_len` bytes that
/// starts at `start_text` (printable ASCII, at most 32 bytes).
pub(crate) fn segment_header(start_text: &str, payload_len: u64) -> Result<[u8; SEGMENT_HEADER_LEN as usize], String> {
    if start_text.len() > START_TEXT_LEN || !start_text.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
        return Err(format!(
            "The segment start time {start_text:?} cannot be stored in a Direct Audio header."
        ));
    }
    let mut header = [0u8; SEGMENT_HEADER_LEN as usize];
    header[0..4].copy_from_slice(&(SEGMENT_HEADER_LEN as u32).to_le_bytes());
    header[4..8].copy_from_slice(&SEGMENT_VERSION.to_le_bytes());
    header[8..12].copy_from_slice(&SEGMENT_KIND_AUDIO.to_le_bytes());
    header[16..16 + start_text.len()].copy_from_slice(start_text.as_bytes());
    header[48..56].copy_from_slice(&payload_len.to_le_bytes());
    Ok(header)
}

/// How the file is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Container {
    /// File header plus segments, as the notes describe.
    DirectAudio,
    /// A bare Ogg Opus stream from byte 0 (what earlier DepoAudio builds wrote).
    BareOgg,
}

/// Where one segment's Ogg stream lies, from the headers alone.
#[derive(Debug, Clone)]
struct Place {
    /// 1-based position of the segment header in the file.
    number: usize,
    header_offset: Option<u64>,
    data_offset: u64,
    declared_length: u64,
    /// Bytes of the payload present in the file (less than declared when the
    /// file was cut short).
    readable_length: u64,
    start: Option<SegmentStart>,
}

#[derive(Debug)]
struct Layout {
    container: Container,
    file_version: Option<u32>,
    guid_present: bool,
    places: Vec<Place>,
    /// Bytes after the last complete segment that belong to no segment.
    unread_tail: u64,
    /// The last segment's payload is shorter than its header says.
    cut_short: bool,
    /// The number of a last segment header that counts no bytes yet.
    empty_last_header: Option<usize>,
    notes: Vec<String>,
}

const BARE_OGG_NOTE: &str = "This file has no Direct Audio header: it is a bare Ogg Opus stream, the layout earlier DepoAudio builds wrote. It is read as one recording session with no start time. CATalyst is not expected to open it.";

/// Walk the Ogg pages that start at `from`, whatever their stream, and say
/// where a segment header follows the last of them, if one does: a header
/// size of at least 56 and either a start time in the notes' form or an Ogg
/// page right after the header. None when the pages run to the end of the
/// file or are followed by anything else.
fn later_segment_header(file: &mut fs::File, from: u64, file_len: u64) -> Result<Option<u64>, String> {
    file.seek(SeekFrom::Start(from)).map_err(|error| error.to_string())?;
    let mut pages = PageReader::new(
        BufReader::with_capacity(1 << 16, (&mut *file).take(file_len.saturating_sub(from))),
        from,
    );
    while let Ok(Some(_)) = pages.next_page() {}
    let at = pages.position();
    drop(pages);
    if at + SEGMENT_HEADER_LEN > file_len {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(at)).map_err(|error| error.to_string())?;
    let mut header = [0u8; SEGMENT_HEADER_LEN as usize];
    if read_fully(file, &mut header)? < header.len() {
        return Ok(None);
    }
    let size = u64::from(u32_at(&header, 0));
    if size < SEGMENT_HEADER_LEN {
        return Ok(None);
    }
    if parse_start_text(&start_text_from_field(&header[16..48])).is_some() {
        return Ok(Some(at));
    }
    file.seek(SeekFrom::Start(at + size))
        .map_err(|error| error.to_string())?;
    let mut capture = [0u8; 4];
    Ok((read_fully(file, &mut capture)? == 4 && &capture == b"OggS").then_some(at))
}

/// The message for a file whose segment lengths stop short of a later
/// session: reading on from the headers would leave sessions out.
fn lengths_disagree(segment: usize, uncounted_from: u64, later: u64) -> String {
    format!(
        "Segment {segment}'s length in its header does not cover all of its audio: Ogg pages from byte {uncounted_from} are counted by no header, and another recording session's header follows them at byte {later}. The recorder may have stopped between writing audio and updating the length, and then recorded again. DepoAudio cannot tell where each session's audio ends, so it does not read this file rather than leave sessions out."
    )
}

fn read_layout(file: &mut fs::File, file_len: u64) -> Result<Layout, String> {
    if file_len == 0 {
        return Err("This SGMCA file is empty.".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|error| error.to_string())?;
    let mut head = [0u8; FILE_HEADER_LEN as usize];
    let read = read_fully(file, &mut head)?;
    if read >= 4 && &head[..4] == b"OggS" {
        return Ok(Layout {
            container: Container::BareOgg,
            file_version: None,
            guid_present: false,
            places: vec![Place {
                number: 1,
                header_offset: None,
                data_offset: 0,
                declared_length: file_len,
                readable_length: file_len,
                start: None,
            }],
            unread_tail: 0,
            cut_short: false,
            empty_last_header: None,
            notes: vec![BARE_OGG_NOTE.into()],
        });
    }
    if read < head.len() {
        return Err("This SGMCA file is shorter than the 40-byte Direct Audio file header.".into());
    }
    let size = u32_at(&head, 0);
    if u64::from(size) != FILE_HEADER_LEN {
        return Err(format!(
            "This SGMCA file's header says it is {size} bytes long, but a Direct Audio file header is 40 bytes, so this is not a Direct Audio file DepoAudio can read."
        ));
    }
    let version = u32_at(&head, 4);
    if version != FORMAT_VERSION {
        return Err(format!(
            "This SGMCA file uses Direct Audio format version {version}; DepoAudio reads only version 1."
        ));
    }
    let guid_present = head[8..24].iter().any(|&byte| byte != 0);
    let mut notes = Vec::new();
    if head[24..40].iter().any(|&byte| byte != 0) {
        notes.push("The file header's 16 reserved bytes are not all zero; they were ignored.".into());
    }

    let mut places: Vec<Place> = Vec::new();
    let mut at = FILE_HEADER_LEN;
    let mut unread_tail = 0;
    let mut cut_short = false;
    let mut empty_last_header = None;
    let mut index = 0;
    while at < file_len {
        index += 1;
        file.seek(SeekFrom::Start(at)).map_err(|error| error.to_string())?;
        let mut header = [0u8; SEGMENT_HEADER_LEN as usize];
        let read = read_fully(file, &mut header)?;
        let remaining = file_len - at;
        // Ogg pages after the last segment: the recorder appends a page and
        // only then rewrites the segment's length, so a copy taken between
        // the two has pages its header does not count yet. That is only the
        // end of the file if no later session follows those pages.
        if !places.is_empty() && read >= 4 && &header[..4] == b"OggS" {
            if let Some(later) = later_segment_header(file, at, file_len)? {
                return Err(lengths_disagree(
                    places.last().map_or(index - 1, |place| place.number),
                    at,
                    later,
                ));
            }
            unread_tail = remaining;
            break;
        }
        if read < header.len() {
            if places.is_empty() {
                return Err("This SGMCA file ends inside its first segment header; it holds no audio yet.".into());
            }
            unread_tail = remaining;
            break;
        }
        // The notes say CATalyst reads a segment header's size, then the
        // rest of the header, and finds the audio right after it; the
        // version, kind and reserved fields are written but not checked.
        let header_size = u32_at(&header, 0);
        if u64::from(header_size) < SEGMENT_HEADER_LEN {
            return Err(format!(
                "Segment {index}'s header (byte {at}) says it is {header_size} bytes long, but a segment header needs 56 bytes for its start time and length; the file is damaged there."
            ));
        }
        if u64::from(header_size) != SEGMENT_HEADER_LEN {
            notes.push(format!(
                "Segment {index}'s header (byte {at}) says it is {header_size} bytes long where the notes give 56; its audio was read from the end of that header."
            ));
        }
        let segment_version = u32_at(&header, 4);
        let kind = u32_at(&header, 8);
        if segment_version != SEGMENT_VERSION || kind != SEGMENT_KIND_AUDIO {
            notes.push(format!(
                "Segment {index}'s header (byte {at}) has version {segment_version} and record kind {kind} where the notes give version 1 and kind 2 (audio). CATalyst is said not to check these, so the segment was read as audio."
            ));
        }
        let start = SegmentStart::from_text(start_text_from_field(&header[16..48]));
        let declared = u64_at(&header, 48);
        let data_offset = at + u64::from(header_size);
        // The file may have grown since its length was taken; never count
        // more than that length.
        let available = file_len.saturating_sub(data_offset);
        if declared == 0 {
            // A header whose length was not updated yet. If another
            // session's header follows straight after it, this session
            // simply holds nothing (CATalyst would find the next header
            // there too); if one follows after uncounted pages, the lengths
            // do not describe the file.
            match later_segment_header(file, data_offset, file_len)? {
                Some(later) if later == data_offset => {
                    notes.push(format!(
                        "Segment {index} (byte {at}) has a length of 0, so it holds no audio; the sessions after it were read."
                    ));
                    at = data_offset;
                    continue;
                }
                Some(later) => return Err(lengths_disagree(index, data_offset, later)),
                None => {}
            }
            unread_tail = available;
            empty_last_header = Some(index);
            break;
        }
        let readable = declared.min(available);
        places.push(Place {
            number: index,
            header_offset: Some(at),
            data_offset,
            declared_length: declared,
            readable_length: readable,
            start: Some(start),
        });
        if declared > available {
            cut_short = true;
            break;
        }
        at = data_offset + declared;
    }
    if places.is_empty() {
        return Err(
            "This SGMCA file has a Direct Audio header but no recorded audio yet; it may still be recording.".into(),
        );
    }
    Ok(Layout {
        container: Container::DirectAudio,
        file_version: Some(version),
        guid_present,
        places,
        unread_tail,
        cut_short,
        empty_last_header,
        notes,
    })
}

// ── Inspection ───────────────────────────────────────────────────────────────

/// One recording session (segment) of an SGMCA file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SegmentInfo {
    /// 1-based, in file order.
    pub number: u32,
    /// Byte offset of the segment header; None for a bare Ogg file.
    pub header_offset: Option<u64>,
    /// Byte offset of the segment's Ogg stream.
    pub data_offset: u64,
    /// Payload length from the segment header (the file length for a bare
    /// Ogg file).
    pub payload_length: u64,
    /// Bytes of the Ogg stream read: up to the end of its last complete page.
    pub bytes_read: u64,
    /// Start time from the segment header; None for a bare Ogg file.
    pub start: Option<SegmentStart>,
    pub head: OpusHead,
    pub tags: OpusTags,
    pub pages: u64,
    pub audio_packets: u64,
    pub start_granule: u64,
    pub final_granule: u64,
    /// Where this session's first sample lands when the sessions are joined
    /// in file order, in 48 kHz samples.
    pub start_sample: u64,
    /// Playable samples at 48 kHz: last granule minus start and pre-skip.
    pub samples: u64,
    pub duration_seconds: f64,
    /// The stream has its end-of-stream page.
    pub end_of_stream: bool,
    /// False when the file ends partway through this segment.
    pub complete: bool,
}

/// What an SGMCA recording holds, from its headers and a full walk of every
/// segment's pages. Every CRC is checked and every packet's duration is read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SgmcaInfo {
    pub container: Container,
    /// The file header's format version; None for a bare Ogg file.
    pub file_version: Option<u32>,
    /// The file header carries a non-zero recording identifier. The
    /// identifier itself is not reported (see docs/SGMCA.md).
    pub guid_present: bool,
    /// Byte offset of the first segment's Ogg stream.
    pub stream_offset: u64,
    /// The first segment's headers.
    pub head: OpusHead,
    pub tags: OpusTags,
    /// Every segment uses family 255 with one uncoupled mono stream per
    /// channel and an identity mapping.
    pub sgmca_layout: bool,
    pub segments: Vec<SegmentInfo>,
    /// The file looks still recording or cut short: the last segment is
    /// shorter than its header says, or bytes after the last segment belong
    /// to none.
    pub growing: bool,
    /// Bytes at the end of the file that no complete segment covers.
    pub unread_bytes: u64,
    pub pages: u64,
    pub audio_packets: u64,
    /// First segment's first audio granule and last segment's last granule.
    pub start_granule: u64,
    pub final_granule: u64,
    /// Playable samples at 48 kHz across every segment.
    pub samples: u64,
    pub duration_seconds: f64,
    /// Plain-language remarks about the file (older layout, still recording,
    /// ignored bytes).
    pub notes: Vec<String>,
}

/// Why opening a stream failed, and whether the cause is the data ending.
struct StreamError {
    message: String,
    /// The data ended partway through a page or packet.
    truncated: bool,
    /// The data ended cleanly, on a page boundary, before both header
    /// packets were read: a session whose recorder has written its first
    /// page or two and nothing more.
    ended: bool,
}

/// The two header packets and the stream's position, read once.
pub(crate) struct OpenStream {
    head: OpusHead,
    tags: OpusTags,
    tags_raw: Vec<u8>,
    packets: PacketReader<BufReader<std::io::Take<fs::File>>>,
    /// Audio packets already read with the tags page, if any (rare).
    pending: Option<PagePackets>,
}

fn open_stream(path: &Path, offset: u64, length: u64) -> Result<OpenStream, StreamError> {
    let damaged = |message: String| StreamError {
        message,
        truncated: false,
        ended: false,
    };
    let mut file = fs::File::open(path).map_err(|error| damaged(format!("Could not open the recording: {error}")))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| damaged(error.to_string()))?;
    let mut packets = PacketReader::new(BufReader::with_capacity(1 << 16, file.take(length)), offset);
    let fail = |packets: &PacketReader<_>, message: String| StreamError {
        truncated: packets.truncated(),
        ended: false,
        message,
    };
    let ended = |message: &str| StreamError {
        message: message.into(),
        truncated: false,
        ended: true,
    };
    let first = match packets.next() {
        Ok(Some(first)) => first,
        Ok(None) => return Err(ended("The Opus stream has no identification header.")),
        Err(message) => return Err(fail(&packets, message)),
    };
    if !first.packets[0].starts_with(b"OpusHead") {
        let codec = codec_of(&first.packets[0]);
        return Err(damaged(format!(
            "This stream holds {codec} audio rather than Opus, so it is not Direct Audio."
        )));
    }
    if first.packets.len() != 1 {
        return Err(damaged(
            "The Opus identification header must be alone on the first Ogg page.".into(),
        ));
    }
    let head = OpusHead::parse(&first.packets[0]).map_err(damaged)?;

    let second = match packets.next() {
        Ok(Some(second)) => second,
        Ok(None) => return Err(ended("The Opus stream has no comment header.")),
        Err(message) => return Err(fail(&packets, message)),
    };
    let mut iter = second.packets.into_iter();
    let tags_raw = iter.next().unwrap();
    let tags = OpusTags::parse(&tags_raw).map_err(damaged)?;
    let rest: Vec<Vec<u8>> = iter.collect();
    let pending = (!rest.is_empty()).then_some(PagePackets {
        packets: rest,
        granule: second.granule,
        eos: second.eos,
        offset: second.offset,
    });
    Ok(OpenStream {
        head,
        tags,
        tags_raw,
        packets,
        pending,
    })
}

/// What the stream starts with, for the message when it is not Opus.
fn codec_of(first_packet: &[u8]) -> &'static str {
    if first_packet.starts_with(b"\x01vorbis") {
        "Vorbis"
    } else if first_packet.starts_with(b"\x7fFLAC") {
        "FLAC"
    } else if first_packet.starts_with(b"Speex") {
        "Speex"
    } else {
        "an unknown codec"
    }
}

/// One audio packet with its place on the 48 kHz timeline.
struct TimedPacket {
    data: Vec<u8>,
    start: u64,
    end: u64,
}

/// Walks audio packets and gives each its start and end granule. The start
/// of the first packet comes from the first page with a granule position:
/// that granule minus the samples finishing on the page (RFC 7845 4.3).
struct Timeline {
    next_start: Option<u64>,
}

impl Timeline {
    /// Time the packets of one page (plus any held from earlier pages).
    fn place(&mut self, page: PagePackets, buffered: &mut Vec<Vec<u8>>) -> Result<Vec<TimedPacket>, String> {
        // Opus granule positions are signed 64-bit counts (RFC 7845 4); a
        // value past the signed range is not a timestamp, and bounding it
        // keeps the sums below from overflowing.
        if page.granule.is_some_and(|granule| granule > i64::MAX as u64) {
            return Err(format!(
                "The Ogg page at byte {} carries an impossible timestamp; the recording is damaged.",
                page.offset
            ));
        }
        let mut data: Vec<Vec<u8>> = std::mem::take(buffered);
        data.extend(page.packets);
        let durations: Vec<u64> = data
            .iter()
            .map(|packet| packet_samples(packet))
            .collect::<Result<_, _>>()?;
        let start = match (self.next_start, page.granule) {
            (Some(start), _) => start,
            (None, Some(granule)) => {
                let total: u64 = durations.iter().sum();
                // A first page whose granule is smaller than its packets is
                // allowed only on a page that also ends the stream (RFC 7845
                // 4.5); otherwise treat the stream as starting at zero.
                granule.saturating_sub(total)
            }
            (None, None) => {
                // No timestamp yet: hold the packets until one arrives.
                *buffered = data;
                return Ok(Vec::new());
            }
        };
        let overflow = || {
            format!(
                "The Ogg page at byte {} carries timestamps past what Ogg can count; the recording is damaged.",
                page.offset
            )
        };
        let mut timed = Vec::with_capacity(data.len());
        let mut cursor = start;
        for (packet, duration) in data.into_iter().zip(durations) {
            let end = cursor
                .checked_add(duration)
                .filter(|&end| end <= i64::MAX as u64)
                .ok_or_else(overflow)?;
            timed.push(TimedPacket {
                data: packet,
                start: cursor,
                end,
            });
            cursor = end;
        }
        // A page's granule position is where its last finished packet ends
        // (RFC 7845 4). A granule past that means audio is missing, or the
        // timestamps are damaged, and the length they give cannot be
        // trusted. (The last page may end earlier, to trim the end.)
        if let Some(granule) = page.granule.filter(|&granule| granule > cursor) {
            return Err(format!(
                "The Ogg page at byte {} says the audio reaches sample {granule}, but its packets end at sample {cursor}; part of the recording is missing or its timestamps are damaged.",
                page.offset
            ));
        }
        self.next_start = Some(cursor);
        Ok(timed)
    }
}

/// One segment's stream, walked.
struct SegmentScan {
    head: OpusHead,
    tags: OpusTags,
    pages: u64,
    audio_packets: u64,
    start_granule: u64,
    final_granule: u64,
    samples: u64,
    end_of_stream: bool,
    bytes_read: u64,
    cut: bool,
}

/// Walk one segment's Ogg stream. With `tolerant` (the last segment of a
/// file that ends early) the walk stops at the last complete page instead of
/// failing. The `last` segment of a file may be a session still being
/// recorded: when its stream ends before both Opus headers, or holds no
/// audio packet yet and no end-of-stream page, it gives None (no complete
/// audio yet) rather than an error.
fn scan_segment(
    path: &Path,
    place: &Place,
    tolerant: bool,
    last: bool,
    cancelled: CancelCheck<'_>,
) -> Result<Option<SegmentScan>, String> {
    let mut stream = match open_stream(path, place.data_offset, place.readable_length) {
        Ok(stream) => stream,
        Err(error) if (tolerant && error.truncated) || (last && error.ended) => return Ok(None),
        Err(error) => return Err(error.message),
    };
    let mut timeline = Timeline { next_start: None };
    let mut buffered = Vec::new();
    let mut audio_packets = 0u64;
    let mut start_granule: Option<u64> = None;
    let mut final_granule = 0u64;
    let mut checked = 0u64;
    let mut cut = false;
    let mut next = stream.pending.take();
    loop {
        let page = match next.take() {
            Some(page) => page,
            None => match stream.packets.next() {
                Ok(Some(page)) => page,
                Ok(None) => break,
                Err(_) if tolerant && stream.packets.truncated() => {
                    cut = true;
                    break;
                }
                Err(message) => return Err(message),
            },
        };
        checked += 1;
        if checked.is_multiple_of(64) && cancelled.is_some_and(|check| check()) {
            return Err(INSPECT_CANCELLED_MESSAGE.into());
        }
        let granule = page.granule;
        let timed = timeline.place(page, &mut buffered)?;
        if let Some(first) = timed.first() {
            start_granule.get_or_insert(first.start);
        }
        audio_packets += timed.len() as u64;
        if let Some(granule) = granule {
            if granule < final_granule {
                return Err("Ogg page timestamps run backwards; the recording is damaged.".into());
            }
            final_granule = granule;
        }
    }
    if !buffered.is_empty() && !cut {
        return Err("The recording's audio pages carry no timestamps.".into());
    }
    let end_of_stream = stream.packets.saw_eos();
    let bytes_read = stream.packets.position() - place.data_offset;
    if !cut && bytes_read < place.readable_length {
        return Err(format!(
            "The stream has {} bytes after its end-of-stream page (byte {}); the file is damaged there.",
            place.readable_length - bytes_read,
            place.data_offset + bytes_read
        ));
    }
    if audio_packets == 0 && (cut || (last && !end_of_stream)) {
        return Ok(None);
    }
    let start_granule = start_granule.unwrap_or(0);
    let samples = final_granule
        .saturating_sub(start_granule)
        .saturating_sub(stream.head.pre_skip as u64);
    Ok(Some(SegmentScan {
        head: stream.head,
        tags: stream.tags,
        pages: stream.packets.pages_read,
        audio_packets,
        start_granule,
        final_granule,
        samples,
        end_of_stream,
        bytes_read,
        cut,
    }))
}

/// The Opus layouts an SGMCA file is read with: family 255 or 1 with one
/// uncoupled stream per channel and an identity mapping (what the notes say
/// CATalyst writes), or family 0 (plain mono or stereo).
fn check_readable_layout(head: &OpusHead, number: usize) -> Result<(), String> {
    let identity = head
        .mapping
        .iter()
        .enumerate()
        .all(|(index, &entry)| entry as usize == index);
    let readable = match head.mapping_family {
        0 => true,
        1 | 255 => head.stream_count == head.channels && head.coupled_count == 0 && identity,
        _ => false,
    };
    if readable {
        Ok(())
    } else {
        Err(format!(
            "Segment {number} uses Opus channel mapping family {} with {} streams ({} coupled) for {} channels. Direct Audio carries one uncoupled mono stream per channel (family 255 or 1) or plain mono or stereo (family 0); DepoAudio does not read other layouts from an SGMCA file.",
            head.mapping_family, head.stream_count, head.coupled_count, head.channels
        ))
    }
}

/// Segments are joined into one recording, so they must decode the same way:
/// the same channel count (CATalyst itself refuses to append a different
/// one) and the same Opus stream layout. A different output gain is not
/// refused, since nothing in the notes rules it out, but it is named: the
/// sessions are decoded as one feed, and which gain applies to the later
/// session has not been checked.
fn check_joinable(first: &OpusHead, other: &OpusHead, number: usize) -> Result<Option<String>, String> {
    if first.channels != other.channels {
        return Err(format!(
            "Segment {number} has {} channels but segment 1 has {}. DepoAudio joins the recording sessions of one file into one recording, so it cannot read a file whose sessions have different channel counts.",
            other.channels, first.channels
        ));
    }
    if (first.stream_count, first.coupled_count, &first.mapping)
        != (other.stream_count, other.coupled_count, &other.mapping)
    {
        return Err(format!(
            "Segment {number} lays out its {} channels in Opus differently from segment 1 ({} streams, {} coupled, against {} and {}), so the recording sessions cannot be joined.",
            other.channels, other.stream_count, other.coupled_count, first.stream_count, first.coupled_count
        ));
    }
    if first.output_gain != other.output_gain {
        return Ok(Some(format!(
            "Segment {number} asks for an Opus output gain of {:.2} dB where segment 1 asks for {:.2} dB. DepoAudio decodes the sessions as one joined recording and has not checked which of the two gains is applied to segment {number}, so its level may differ from what CATalyst plays.",
            f64::from(other.output_gain) / 256.0,
            f64::from(first.output_gain) / 256.0
        )));
    }
    Ok(None)
}

/// Read an SGMCA file: its headers, then every segment's pages.
pub(crate) fn inspect(path: &Path, cancelled: CancelCheck<'_>) -> Result<SgmcaInfo, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("Could not open the recording: {error}"))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("Could not read the recording: {error}"))?
        .len();
    let layout = read_layout(&mut file, file_len)?;
    drop(file);

    let mut notes = layout.notes;
    let count = layout.places.len();
    let mut segments: Vec<SegmentInfo> = Vec::with_capacity(count);
    let mut start_sample = 0u64;
    let mut dropped_last = false;
    for (index, place) in layout.places.iter().enumerate() {
        let number = place.number;
        let last = index + 1 == count;
        let tolerant = last && layout.cut_short;
        let label = |message: String| match layout.container {
            Container::DirectAudio if message != INSPECT_CANCELLED_MESSAGE => format!("Segment {number}: {message}"),
            _ => message,
        };
        let Some(scan) = scan_segment(path, place, tolerant, last, cancelled).map_err(label)? else {
            dropped_last = true;
            continue;
        };
        check_readable_layout(&scan.head, number)?;
        if let Some(first) = segments.first() {
            notes.extend(check_joinable(&first.head, &scan.head, number)?);
        }
        segments.push(SegmentInfo {
            number: number as u32,
            header_offset: place.header_offset,
            data_offset: place.data_offset,
            payload_length: place.declared_length,
            bytes_read: scan.bytes_read,
            start: place.start.clone(),
            head: scan.head,
            tags: scan.tags,
            pages: scan.pages,
            audio_packets: scan.audio_packets,
            start_granule: scan.start_granule,
            final_granule: scan.final_granule,
            start_sample,
            samples: scan.samples,
            duration_seconds: scan.samples as f64 / OPUS_RATE as f64,
            end_of_stream: scan.end_of_stream,
            complete: !scan.cut,
        });
        start_sample += scan.samples;
    }
    if segments.is_empty() {
        return Err("This SGMCA file has no complete audio yet; it may still be recording.".into());
    }
    if segments.iter().all(|segment| segment.samples == 0) {
        return Err(
            "None of this SGMCA file's recording sessions holds any audio samples (only Opus headers and pre-skip), so there is nothing to play or convert."
                .into(),
        );
    }
    let growing = layout.cut_short || layout.unread_tail > 0 || layout.empty_last_header.is_some() || dropped_last;
    let last_number = layout.places.last().map_or(count, |place| place.number);
    if dropped_last {
        notes.push(format!(
            "Segment {last_number} has no complete audio yet, so it was not read. The file may still be recording, or it was cut short."
        ));
    } else if layout.cut_short {
        notes.push(format!(
            "Segment {last_number} is shorter than its header says, so only its complete Ogg pages were read. The file may still be recording, or it was cut short."
        ));
    }
    if let Some(number) = layout.empty_last_header {
        notes.push(format!(
            "Segment {number} has a header but its length is not written yet, so it was not read. The file may still be recording, or it was cut short."
        ));
    }
    if layout.unread_tail > 0 {
        notes.push(format!(
            "The last {} bytes of the file are not counted by any segment header and were not read. The file may still be recording, or it was cut short.",
            layout.unread_tail
        ));
    }
    for segment in &segments {
        if let Some(start) = &segment.start {
            if start.local.is_none() {
                let shown = if start.text.contains('?') {
                    " (bytes that are not printable ASCII are shown as ?)"
                } else {
                    ""
                };
                notes.push(format!(
                    "Segment {}'s start time {:?}{shown} is not in the YYYY-MM-DDTHH:MM:SS±hhmm form, so it could not be read.",
                    segment.number, start.text
                ));
            }
        }
        if !segment.end_of_stream && segment.complete {
            notes.push(format!(
                "Segment {}'s Ogg stream has no end-of-stream page.",
                segment.number
            ));
        }
    }
    let first = &segments[0];
    let samples: u64 = segments.iter().map(|segment| segment.samples).sum();
    Ok(SgmcaInfo {
        container: layout.container,
        file_version: layout.file_version,
        guid_present: layout.guid_present,
        stream_offset: first.data_offset,
        head: first.head.clone(),
        tags: first.tags.clone(),
        sgmca_layout: segments.iter().all(|segment| segment.head.is_sgmca_layout()),
        growing,
        unread_bytes: layout.unread_tail,
        pages: segments.iter().map(|segment| segment.pages).sum(),
        audio_packets: segments.iter().map(|segment| segment.audio_packets).sum(),
        start_granule: first.start_granule,
        final_granule: segments.last().map_or(0, |segment| segment.final_granule),
        samples,
        duration_seconds: samples as f64 / OPUS_RATE as f64,
        segments,
        notes,
    })
}

// ── FFmpeg input ─────────────────────────────────────────────────────────────

/// An FFmpeg concat-demuxer script that plays every segment's Ogg stream in
/// file order, each read in place from the original file through FFmpeg's
/// `subfile` protocol (no copy of the audio is made). Each entry carries its
/// exact duration, so ffprobe reports the joined length. Needs
/// `-f concat -safe 0 -protocol_whitelist file,subfile` before `-i`.
pub(crate) fn concat_script(path: &Path, info: &SgmcaInfo) -> Result<String, String> {
    let absolute =
        std::path::absolute(path).map_err(|error| format!("Could not resolve the recording's path: {error}"))?;
    let text = absolute
        .to_str()
        .ok_or("The recording's path is not valid Unicode, so FFmpeg cannot be pointed at its sessions.")?;
    if text.contains(['\n', '\r']) {
        return Err("The recording's path contains a line break, so FFmpeg cannot be pointed at its sessions.".into());
    }
    // ffconcat single quotes take everything literally; a quote is written
    // by closing the quote, escaping one, and reopening.
    let quoted = text.replace('\'', "'\\''");
    let mut script = String::from("ffconcat version 1.0\n");
    for segment in info.segments.iter().filter(|segment| segment.samples > 0) {
        script.push_str(&format!(
            "file 'subfile,,start,{},end,{},,:file:{quoted}'\nduration {:.6}\n",
            segment.data_offset,
            segment.data_offset + segment.bytes_read,
            segment.duration_seconds
        ));
    }
    Ok(script)
}

// ── Slicing ──────────────────────────────────────────────────────────────────

/// What a slice wrote.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SliceReport {
    pub channels: u8,
    /// The requested start, where the cut was made, and the end reached, in
    /// seconds of the joined recording.
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// The new segment's start text.
    pub start_text: String,
    /// How far the slice's first sample falls after the whole second in
    /// `start_text`, in milliseconds (the header holds whole seconds only).
    /// None when the session had no readable start, so the text is the time
    /// of slicing.
    pub start_offset_ms: Option<f64>,
    /// What the start text means for this slice.
    pub start_note: String,
    /// Playable samples at 48 kHz in the new file.
    pub samples: u64,
    pub pre_skip: u16,
    pub audio_packets: u64,
    pub pages: u64,
}

fn seconds_to_samples(seconds: f64) -> u64 {
    (seconds * OPUS_RATE as f64).round() as u64
}

/// What `copy_range` copied.
struct CopiedRange {
    samples: u64,
    pre_skip: u16,
    audio_packets: u64,
    pages: u64,
}

/// Copy the part of one Ogg Opus stream from `start` to `end` playable
/// samples (or to the end) into `writer`, without re-encoding.
///
/// Whole Opus packets are copied, starting 80 ms before the cut so the
/// decoder converges, and the new stream's pre-skip discards exactly the
/// samples before the requested start. The last page's granule position
/// trims the end to the requested sample (RFC 7845 section 4.5). Granule
/// positions are rebased to start at zero and every page is re-framed with
/// fresh sequence numbers and CRCs.
fn copy_range<W: Write>(
    mut stream: OpenStream,
    writer: &mut OggWriter<W>,
    start: u64,
    end: Option<u64>,
    cancelled: CancelCheck<'_>,
) -> Result<CopiedRange, String> {
    let pre_skip = stream.head.pre_skip as u64;
    let mut timeline = Timeline { next_start: None };
    let mut buffered = Vec::new();
    // Absolute granule targets, known once the stream's start is known.
    let mut targets: Option<(u64, Option<u64>)> = None;
    // Rebase origin (start of the first copied packet) and the new pre-skip.
    let mut origin: Option<(u64, u16)> = None;
    // The last copied packet waits so its page can carry the end trim.
    let mut held: Option<TimedPacket> = None;
    let mut copied = 0u64;
    let mut end_reached: Option<u64> = None;
    let mut checked = 0u64;
    let mut next = stream.pending.take();

    'pages: loop {
        let page = match next.take() {
            Some(page) => page,
            None => match stream.packets.next()? {
                Some(page) => page,
                None => break,
            },
        };
        checked += 1;
        if checked.is_multiple_of(64) && cancelled.is_some_and(|check| check()) {
            return Err(SLICE_CANCELLED_MESSAGE.into());
        }
        let eos_granule = if page.eos { page.granule } else { None };
        let timed = timeline.place(page, &mut buffered)?;
        for packet in timed {
            let (cut_start, cut_end) = *targets.get_or_insert_with(|| {
                let base = packet.start.saturating_add(pre_skip);
                (base.saturating_add(start), end.map(|end| base.saturating_add(end)))
            });
            // A final page may trim its last packets (RFC 7845 4.5).
            let stream_end = eos_granule.unwrap_or(u64::MAX);
            let limit = cut_end.unwrap_or(u64::MAX).min(stream_end);
            if packet.start >= limit {
                end_reached = Some(limit);
                break 'pages;
            }
            if packet.end <= cut_start.saturating_sub(PREROLL_SAMPLES) {
                continue;
            }
            if origin.is_none() {
                let lead = cut_start
                    .checked_sub(packet.start)
                    .ok_or("The slice start falls before the recording's first sample.")?;
                let new_pre_skip = u16::try_from(lead)
                    .map_err(|_| "The slice start falls in a gap the Opus pre-skip cannot cover.".to_string())?;
                let mut head = stream.head.clone();
                head.pre_skip = new_pre_skip;
                writer.write_packet(&head.to_bytes(), 0)?;
                writer.flush(false)?;
                writer.write_packet(&stream.tags_raw, 0)?;
                writer.flush(false)?;
                origin = Some((packet.start, new_pre_skip));
            }
            let (base, _) = origin.unwrap();
            if let Some(previous) = held.take() {
                writer.write_packet(&previous.data, previous.end - base)?;
                copied += 1;
                writer.maybe_flush()?;
            }
            held = Some(packet);
            if held.as_ref().is_some_and(|packet| packet.end >= limit) {
                end_reached = Some(limit);
                break 'pages;
            }
        }
    }
    if !buffered.is_empty() {
        return Err("The recording's audio pages carry no timestamps.".into());
    }
    let (base, new_pre_skip) = origin.ok_or("The slice start is past the end of the recording.")?;
    let last = held.ok_or("The slice holds no audio.")?;
    let final_granule = end_reached.unwrap_or(last.end).min(last.end);
    // The last packet gets a page of its own so the end trim (a final
    // granule below the packet's end) touches only that packet.
    writer.flush(false)?;
    writer.write_packet(&last.data, final_granule - base)?;
    copied += 1;
    writer.flush(true)?;
    Ok(CopiedRange {
        samples: (final_granule - base).saturating_sub(new_pre_skip as u64),
        pre_skip: new_pre_skip,
        audio_packets: copied,
        pages: writer.pages_written,
    })
}

/// A fresh random Ogg serial number for a new logical stream.
fn fresh_serial() -> u32 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Writes a Direct Audio file: the file header, then segments whose payload
/// length is patched once each payload is complete. `dst` must not exist.
struct DirectAudioWriter {
    out: BufWriter<fs::File>,
    open_segment: Option<(u64, String)>,
}

impl DirectAudioWriter {
    fn create(dst: &Path) -> Result<Self, String> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)
            .map_err(|error| format!("Could not create {}: {error}", dst.display()))?;
        let mut out = BufWriter::with_capacity(1 << 16, file);
        out.write_all(&file_header(uuid::Uuid::new_v4().to_bytes_le()))
            .map_err(write_error)?;
        Ok(Self {
            out,
            open_segment: None,
        })
    }

    fn begin_segment(&mut self, start_text: &str) -> Result<(), String> {
        let at = self.out.stream_position().map_err(write_error)?;
        self.out
            .write_all(&segment_header(start_text, 0)?)
            .map_err(write_error)?;
        self.open_segment = Some((at, start_text.to_string()));
        Ok(())
    }

    fn end_segment(&mut self) -> Result<u64, String> {
        let (at, start_text) = self.open_segment.take().ok_or("No segment is open.")?;
        let end = self.out.stream_position().map_err(write_error)?;
        let length = end - at - SEGMENT_HEADER_LEN;
        if length == 0 {
            return Err("A segment of the SGMCA output holds no audio.".into());
        }
        self.out.seek(SeekFrom::Start(at)).map_err(write_error)?;
        self.out
            .write_all(&segment_header(&start_text, length)?)
            .map_err(write_error)?;
        self.out.seek(SeekFrom::Start(end)).map_err(write_error)?;
        Ok(length)
    }

    fn finish(self) -> Result<(), String> {
        if self.open_segment.is_some() {
            return Err("A segment of the SGMCA output was left open.".into());
        }
        self.out
            .into_inner()
            .map_err(|error| write_error(error.into_error()))?
            .sync_all()
            .map_err(write_error)
    }
}

fn write_error(error: std::io::Error) -> String {
    format!("Could not write the recording: {error}")
}

/// Copy the part of `src` from `start_seconds` to `end_seconds` (or to the
/// end) into a new Direct Audio file at `dst`, without re-encoding (see
/// `copy_range`). The cut must stay inside one recording session: a range
/// that runs into the next session is refused, because the sessions are
/// separate streams with a gap of unknown length between them. The cut is
/// made exactly at `start_seconds`; no audio is left out to make the start
/// round. The segment header holds whole seconds only, so the new segment's
/// start is the session's start plus the whole seconds skipped (the second
/// at or before the first sample), and the report gives the fraction left
/// over (`start_offset_ms`) and says so. A session with no readable start
/// time gets the time of slicing. `dst` must not exist yet.
///
/// Not yet offered in the app: a slice is a new output, and every output
/// needs its export record and a way to choose the range on screen.
#[allow(dead_code)]
pub(crate) fn slice(
    src: &Path,
    dst: &Path,
    start_seconds: f64,
    end_seconds: Option<f64>,
    cancelled: CancelCheck<'_>,
) -> Result<SliceReport, String> {
    if !start_seconds.is_finite() || start_seconds < 0.0 {
        return Err("The slice start must be zero or more seconds.".into());
    }
    if let Some(end) = end_seconds {
        if !end.is_finite() || end <= start_seconds {
            return Err("The slice end must come after its start.".into());
        }
    }
    if dst.exists() {
        return Err(format!(
            "{} already exists; a slice never replaces a file.",
            dst.display()
        ));
    }
    let info = inspect(src, cancelled)?;
    let start = seconds_to_samples(start_seconds);
    let segment = info
        .segments
        .iter()
        .find(|segment| start < segment.start_sample + segment.samples)
        .ok_or("The slice start is past the end of the recording.")?;
    let segment_end = segment.start_sample + segment.samples;
    let is_last = segment.number == info.segments.last().map_or(0, |last| last.number);
    let end = end_seconds.map(seconds_to_samples);
    if end.is_some_and(|end| end > segment_end) && !is_last {
        return Err(format!(
            "The slice runs from recording session {} into the next one. DepoAudio cannot cut across sessions yet; slice each session on its own.",
            segment.number
        ));
    }
    let skipped = start - segment.start_sample;
    let (start_text, start_offset_ms, start_note) = match segment
        .start
        .as_ref()
        .and_then(|start| start.shifted_text(skipped / OPUS_RATE))
    {
        Some(text) => {
            let offset = offset_ms(skipped % OPUS_RATE, OPUS_RATE);
            let note = if offset == 0.0 {
                "The slice starts on a whole second of the session's start time.".to_string()
            } else {
                format!(
                    "{} The slice starts exactly where it was asked to; no audio was left out.",
                    whole_second_note(offset)
                )
            };
            (text, Some(offset), note)
        }
        None => (
            crate::clock::local_start_text_now().ok_or("The slice's start time could not be written.")?,
            None,
            "The recording session has no readable start time, so the slice's start is the time of slicing, which is not when the recording was made.".to_string(),
        ),
    };
    let stream = open_stream(src, segment.data_offset, segment.bytes_read).map_err(|error| error.message)?;
    let channels = stream.head.channels;

    let mut out = DirectAudioWriter::create(dst)?;
    let result = (|| {
        out.begin_segment(&start_text)?;
        let mut writer = OggWriter::new(&mut out.out, fresh_serial());
        let copied = copy_range(
            stream,
            &mut writer,
            skipped,
            end.map(|end| end.min(segment_end) - segment.start_sample),
            cancelled,
        )?;
        out.end_segment()?;
        Ok::<_, String>(copied)
    })();
    let result = result.and_then(|copied| out.finish().map(|()| copied));
    let copied = match result {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(dst);
            return Err(error);
        }
    };
    let start_seconds = start as f64 / OPUS_RATE as f64;
    Ok(SliceReport {
        channels,
        start_seconds,
        end_seconds: start_seconds + copied.samples as f64 / OPUS_RATE as f64,
        start_text,
        start_offset_ms,
        start_note,
        samples: copied.samples,
        pre_skip: copied.pre_skip,
        audio_packets: copied.audio_packets,
        pages: copied.pages,
    })
}

// ── Writing ──────────────────────────────────────────────────────────────────

/// One segment of an SGMCA output: its start text and where it begins on
/// the output's playable timeline, in 48 kHz samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedSegment {
    pub start_text: String,
    pub start_sample: u64,
}

/// Turn the bare Ogg Opus stream FFmpeg wrote (`ogg`) into a new Direct
/// Audio file at `dst`: the file header with a fresh random identifier, then
/// one segment per planned segment. With one segment the Ogg bytes are
/// copied unchanged. With more, each segment is that part of the stream cut
/// out without re-encoding (see `copy_range`), as its own Ogg stream with a
/// new serial number. The first segment must start at sample 0, so no
/// encoded audio is ever left out. `dst` must not exist; on any failure it
/// is removed.
pub(crate) fn write_direct_audio(
    ogg: &Path,
    dst: &Path,
    segments: &[PlannedSegment],
    cancelled: CancelCheck<'_>,
) -> Result<(), String> {
    if segments.first().is_some_and(|first| first.start_sample != 0) {
        return Err(
            "The SGMCA output's first segment does not start at the first sample, so audio would be left out.".into(),
        );
    }
    if segments.is_empty()
        || segments
            .windows(2)
            .any(|pair| pair[1].start_sample <= pair[0].start_sample)
    {
        return Err("The SGMCA output's segments are not in order.".into());
    }
    let mut out = DirectAudioWriter::create(dst)?;
    let result = fill_direct_audio(ogg, &mut out, segments, cancelled).and_then(|()| out.finish());
    if result.is_err() {
        let _ = fs::remove_file(dst);
    }
    result
}

fn fill_direct_audio(
    ogg: &Path,
    out: &mut DirectAudioWriter,
    segments: &[PlannedSegment],
    cancelled: CancelCheck<'_>,
) -> Result<(), String> {
    let length = fs::metadata(ogg)
        .map_err(|error| format!("Could not read the encoded audio: {error}"))?
        .len();
    if let [only] = segments {
        out.begin_segment(&only.start_text)?;
        let mut input = fs::File::open(ogg).map_err(|error| format!("Could not read the encoded audio: {error}"))?;
        let mut chunk = vec![0u8; 1 << 16];
        loop {
            if cancelled.is_some_and(|check| check()) {
                return Err(WRITE_CANCELLED_MESSAGE.into());
            }
            let read = input
                .read(&mut chunk)
                .map_err(|error| format!("Could not read the encoded audio: {error}"))?;
            if read == 0 {
                break;
            }
            out.out.write_all(&chunk[..read]).map_err(write_error)?;
        }
        if out.end_segment()? != length {
            return Err("The encoded audio changed while it was being copied.".into());
        }
        return Ok(());
    }
    for (index, segment) in segments.iter().enumerate() {
        let end = segments.get(index + 1).map(|next| next.start_sample);
        let stream = open_stream(ogg, 0, length).map_err(|error| error.message)?;
        out.begin_segment(&segment.start_text)?;
        let mut writer = OggWriter::new(&mut out.out, fresh_serial());
        copy_range(stream, &mut writer, segment.start_sample, end, cancelled).map_err(|error| {
            format!(
                "Recording session {} could not be cut from the encoded audio: {error}",
                index + 1
            )
        })?;
        out.end_segment()?;
    }
    Ok(())
}

// ── Writer check ─────────────────────────────────────────────────────────────

/// Confirm that a file DepoAudio wrote as SGMCA really has the Direct Audio
/// layout: the 40-byte file header (version 1, an identifier), complete
/// segments, each a sound Ogg Opus stream in mapping family 255 with one
/// uncoupled mono stream per channel and an identity mapping, and, when
/// `expected_starts` is given, exactly those segment start texts in order.
pub(crate) fn verify_written(
    path: &Path,
    expected_starts: Option<&[String]>,
    cancelled: CancelCheck<'_>,
) -> Result<SgmcaInfo, String> {
    let info = inspect(path, cancelled)?;
    if info.container != Container::DirectAudio {
        return Err("The SGMCA output has no Direct Audio file header.".into());
    }
    if !info.guid_present {
        return Err("The SGMCA output's file header has no recording identifier.".into());
    }
    if info.growing
        || info
            .segments
            .iter()
            .any(|segment| !segment.complete || !segment.end_of_stream)
    {
        return Err("The SGMCA output is incomplete.".into());
    }
    if let Some(bad) = info.segments.iter().find(|segment| !segment.head.is_sgmca_layout()) {
        return Err(format!(
            "The SGMCA output uses Opus mapping family {} with {} streams ({} coupled) for {} channels, not one mono stream per channel.",
            bad.head.mapping_family, bad.head.stream_count, bad.head.coupled_count, bad.head.channels
        ));
    }
    if info.samples == 0 || info.segments.iter().any(|segment| segment.samples == 0) {
        return Err("The SGMCA output holds no audio.".into());
    }
    if let Some(bad) = info
        .segments
        .iter()
        .find(|segment| segment.start.as_ref().is_none_or(|start| start.local.is_none()))
    {
        return Err(format!(
            "The SGMCA output's segment {} has no start time in the YYYY-MM-DDTHH:MM:SS±hhmm form, so CATalyst could not place it against a transcript.",
            bad.number
        ));
    }
    if let Some(expected) = expected_starts {
        let written: Vec<&str> = info
            .segments
            .iter()
            .map(|segment| segment.start.as_ref().map_or("", |start| start.text.as_str()))
            .collect();
        if written.len() != expected.len()
            || written
                .iter()
                .zip(expected)
                .any(|(written, expected)| written != expected)
        {
            return Err(format!(
                "The SGMCA output's segment start times ({}) are not the ones planned ({}).",
                written.join(", "),
                expected.join(", ")
            ));
        }
    }
    Ok(info)
}

/// Synthetic Ogg Opus streams and Direct Audio files for tests, built with
/// this module's writer. The audio packets are well-framed and correctly
/// timed but never decoded.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// A family-255 header for `channels` mono streams.
    pub(crate) fn sgmca_head(channels: u8, pre_skip: u16) -> OpusHead {
        OpusHead {
            version: 1,
            channels,
            pre_skip,
            input_sample_rate: 16_000,
            output_gain: 0,
            mapping_family: 255,
            stream_count: channels,
            coupled_count: 0,
            mapping: (0..channels).collect(),
        }
    }

    pub(crate) fn tags_packet(vendor: &str, comments: &[&str]) -> Vec<u8> {
        let mut out = b"OpusTags".to_vec();
        out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        out.extend_from_slice(vendor.as_bytes());
        out.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for comment in comments {
            out.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            out.extend_from_slice(comment.as_bytes());
        }
        out
    }

    /// A fake 20 ms CELT packet (config 31, code 0) of `len` bytes. The
    /// payload is never decoded here, only framed and timed.
    pub(crate) fn fake_packet(len: usize, fill: u8) -> Vec<u8> {
        let mut packet = vec![fill; len];
        packet[0] = 31 << 3;
        packet
    }

    /// A synthetic stream with `head`: headers plus `count` 20 ms packets,
    /// end-trimmed by `trim` samples, written with the module's own writer.
    pub(crate) fn stream_with(
        head: &OpusHead,
        serial: u32,
        count: usize,
        trim: u64,
        sizes: impl Fn(usize) -> usize,
    ) -> Vec<u8> {
        let mut writer = OggWriter::new(Vec::new(), serial);
        writer.write_packet(&head.to_bytes(), 0).unwrap();
        writer.flush(false).unwrap();
        writer.write_packet(&tags_packet("test", &["TITLE=x"]), 0).unwrap();
        writer.flush(false).unwrap();
        for index in 0..count {
            let mut granule = (index as u64 + 1) * 960;
            if index + 1 == count {
                granule -= trim;
            }
            writer
                .write_packet(&fake_packet(sizes(index), index as u8), granule)
                .unwrap();
            if index + 1 == count {
                writer.flush(true).unwrap();
            } else {
                writer.maybe_flush().unwrap();
            }
        }
        writer.into_inner()
    }

    /// A 4-channel family-255 stream (see `stream_with`).
    pub(crate) fn synthetic(count: usize, pre_skip: u16, trim: u64, sizes: impl Fn(usize) -> usize) -> Vec<u8> {
        stream_with(&sgmca_head(4, pre_skip), 0x5eed, count, trim, sizes)
    }

    /// The example identifier from the notes' worked example,
    /// {12345678-9ABC-DEF0-1122-334455667788}, in the Windows byte layout.
    pub(crate) const EXAMPLE_GUID: [u8; 16] = [
        0x78, 0x56, 0x34, 0x12, 0xBC, 0x9A, 0xF0, 0xDE, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
    ];

    /// A Direct Audio file: the file header, then one segment per
    /// `(start text, Ogg stream)`.
    pub(crate) fn direct_audio(segments: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = file_header(EXAMPLE_GUID).to_vec();
        for (start, stream) in segments {
            out.extend_from_slice(&segment_header(start, stream.len() as u64).unwrap());
            out.extend_from_slice(stream);
        }
        out
    }

    /// One 4-channel session wrapped as a Direct Audio file.
    pub(crate) fn synthetic_direct(count: usize, pre_skip: u16, trim: u64, sizes: impl Fn(usize) -> usize) -> Vec<u8> {
        direct_audio(&[("2026-11-22T10:15:30-0500", &synthetic(count, pre_skip, trim, sizes))])
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn write_temp(dir: &tempdir::Dir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!("depoaudio-sgmca-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn family_one(channels: u8, pre_skip: u16) -> OpusHead {
        let mut head = sgmca_head(channels, pre_skip);
        head.mapping_family = 1;
        head
    }

    const START_1: &str = "2026-11-22T10:15:30-0500";
    const START_2: &str = "2026-11-22T11:02:05-0500";
    const START_3: &str = "2026-11-22T13:40:00-0500";

    /// Three sessions: 4-channel, families 255, 1 and 255, of 100, 250 and 60
    /// packets with different pre-skips and end trims.
    fn three_sessions() -> (Vec<u8>, [Vec<u8>; 3]) {
        let streams = [
            stream_with(&sgmca_head(4, 312), 1, 100, 0, |_| 30),
            stream_with(&family_one(4, 3840), 2, 250, 480, |index| 20 + index % 9),
            stream_with(&sgmca_head(4, 312), 3, 60, 100, |_| 25),
        ];
        let file = direct_audio(&[(START_1, &streams[0]), (START_2, &streams[1]), (START_3, &streams[2])]);
        (file, streams)
    }

    #[test]
    fn crc_matches_the_ogg_reference_values() {
        // Known values for the Ogg polynomial, MSB first, init 0, no xor.
        assert_eq!(ogg_crc(b""), 0);
        assert_eq!(ogg_crc(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn worked_example_header_bytes_match_the_notes() {
        // The identifier's text form gives the same Windows-layout bytes.
        let parsed = uuid::Uuid::parse_str("12345678-9ABC-DEF0-1122-334455667788").unwrap();
        assert_eq!(parsed.to_bytes_le(), EXAMPLE_GUID);

        let mut bytes = file_header(EXAMPLE_GUID).to_vec();
        bytes.extend_from_slice(&segment_header(START_1, 123_456).unwrap());
        let expected: Vec<u8> = [
            "28 00 00 00",
            "01 00 00 00",
            "78 56 34 12 BC 9A F0 DE 11 22 33 44 55 66 77 88",
            "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
            "38 00 00 00",
            "01 00 00 00",
            "02 00 00 00",
            "00 00 00 00",
            "32 30 32 36 2D 31 31 2D 32 32 54 31 30 3A 31 35",
            "3A 33 30 2D 30 35 30 30 00 00 00 00 00 00 00 00",
            "40 E2 01 00 00 00 00 00",
        ]
        .iter()
        .flat_map(|line| line.split(' ').map(|byte| u8::from_str_radix(byte, 16).unwrap()))
        .collect();
        assert_eq!(bytes.len(), 0x60, "everything before the first OggS is 96 bytes");
        assert_eq!(bytes, expected);
        // The next segment header would sit at 0x60 + 0x1E240.
        assert_eq!(0x60 + 123_456, 0x1E2A0);
    }

    #[test]
    fn worked_example_opus_head_page_has_the_crc_in_the_notes() {
        let page: Vec<u8> = [
            "4F 67 67 53 00 02",
            "00 00 00 00 00 00 00 00",
            "78 56 34 12",
            "00 00 00 00",
            "00 00 00 00",
            "01 17",
            "4F 70 75 73 48 65 61 64 01 02 38 01 80 BB 00 00 00 00 FF 02 00 00 01",
        ]
        .iter()
        .flat_map(|line| line.split(' ').map(|byte| u8::from_str_radix(byte, 16).unwrap()))
        .collect();
        assert_eq!(page.len(), 51);
        assert_eq!(ogg_crc(&page), 0x8013_84E6);

        // This module's writer produces the same page, CRC included.
        let head = OpusHead {
            version: 1,
            channels: 2,
            pre_skip: 312,
            input_sample_rate: 48_000,
            output_gain: 0,
            mapping_family: 255,
            stream_count: 2,
            coupled_count: 0,
            mapping: vec![0, 1],
        };
        let mut writer = OggWriter::new(Vec::new(), 0x1234_5678);
        writer.write_packet(&head.to_bytes(), 0).unwrap();
        writer.flush(false).unwrap();
        let written = writer.into_inner();
        assert_eq!(&written[22..26], &[0xE6, 0x84, 0x13, 0x80]);
        let mut unsigned = written.clone();
        unsigned[22..26].copy_from_slice(&[0; 4]);
        assert_eq!(unsigned, page);
    }

    #[test]
    fn start_text_parses_formats_and_shifts() {
        let start = SegmentStart::from_text(START_1.into());
        assert_eq!(start.local.as_deref(), Some("2026-11-22T10:15:30"));
        assert_eq!(start.utc_offset.as_deref(), Some("-05:00"));
        assert_eq!(start.shifted_text(30).as_deref(), Some("2026-11-22T10:16:00-0500"));
        // Across midnight and a year end.
        let late = SegmentStart::from_text("2026-12-31T23:59:50+0530".into());
        assert_eq!(late.shifted_text(15).as_deref(), Some("2027-01-01T00:00:05+0530"));
        for bad in [
            "2026-13-22T10:15:30-0500",
            "2026-02-30T10:15:30-0500",
            "2026-11-22T24:00:00-0500",
            "2026-11-22 10:15:30-0500",
            "2026-11-22T10:15:30",
            "2026-11-22T10:15:30-05:00",
            "",
        ] {
            let parsed = SegmentStart::from_text(bad.into());
            assert_eq!(parsed.local, None, "{bad}");
            assert_eq!(parsed.shifted_text(1), None, "{bad}");
        }
        let (local, offset) = parse_start_text(START_1).unwrap();
        assert_eq!(format_start_text(local, offset).as_deref(), Some(START_1));
        assert_eq!(start_text_from_field(b"2026\x01ab\0\0zz"), "2026?ab");
    }

    #[test]
    fn writer_pages_round_trip_through_the_strict_reader() {
        let bytes = synthetic(120, 312, 100, |index| 40 + index % 7);
        let mut reader = PageReader::new(&bytes[..], 0);
        let mut pages = Vec::new();
        while let Some(page) = reader.next_page().unwrap() {
            pages.push(page);
        }
        assert!(pages[0].is_bos() && !pages[1].is_bos());
        assert!(pages.last().unwrap().is_eos());
        assert!(pages
            .iter()
            .enumerate()
            .all(|(index, page)| page.sequence == index as u32));
        assert_eq!(pages.last().unwrap().granule, 120 * 960 - 100);
    }

    #[test]
    fn packets_longer_than_a_page_continue_across_pages() {
        let bytes = synthetic(3, 312, 0, |index| if index == 1 { 140_000 } else { 50 });
        let mut reader = PageReader::new(&bytes[..], 0);
        let mut flags = Vec::new();
        while let Some(page) = reader.next_page().unwrap() {
            flags.push((page.is_continued(), page.granule()));
        }
        // 140 000 bytes need 550 lacing values, so the packet spans three
        // pages; the middle one finishes no packet and so has no granule.
        assert!(flags.iter().any(|(continued, _)| *continued));
        assert!(flags.iter().any(|(_, granule)| granule.is_none()));
        let mut packets = PacketReader::new(&bytes[..], 0);
        let mut sizes = Vec::new();
        while let Some(page) = packets.next().unwrap() {
            sizes.extend(page.packets.iter().map(Vec::len));
        }
        assert_eq!(sizes[2..], [50, 140_000, 50]);
    }

    #[test]
    fn opus_head_round_trips_and_validates() {
        let head = sgmca_head(4, 3840);
        let parsed = OpusHead::parse(&head.to_bytes()).unwrap();
        assert_eq!(parsed, head);
        assert!(parsed.is_sgmca_layout());

        let mut coupled = head.clone();
        coupled.mapping_family = 1;
        coupled.stream_count = 2;
        coupled.coupled_count = 2;
        coupled.mapping = vec![0, 1, 2, 3];
        assert!(!OpusHead::parse(&coupled.to_bytes()).unwrap().is_sgmca_layout());

        let mut bad = head.to_bytes();
        bad[21] = 9; // mapping entry past the four streams
        assert!(OpusHead::parse(&bad).unwrap_err().contains("mapping"));
        let mut zero = head.to_bytes();
        zero[9] = 0;
        assert!(OpusHead::parse(&zero).unwrap_err().contains("zero channels"));
        assert!(OpusHead::parse(&head.to_bytes()[..22])
            .unwrap_err()
            .contains("cut short"));
    }

    #[test]
    fn opus_tags_parse_and_reject_overlong_lengths() {
        let tags = OpusTags::parse(&tags_packet("libopus 1.4", &["A=1", "B=two"])).unwrap();
        assert_eq!(tags.vendor, "libopus 1.4");
        assert_eq!(tags.comments, ["A=1", "B=two"]);
        let mut bad = tags_packet("v", &[]);
        bad[8..12].copy_from_slice(&1000u32.to_le_bytes());
        assert!(OpusTags::parse(&bad).is_err());
        let mut count = tags_packet("v", &[]);
        let at = count.len() - 4;
        count[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(OpusTags::parse(&count).is_err());
    }

    #[test]
    fn toc_durations_cover_every_mode_and_frame_count() {
        assert_eq!(packet_samples(&[31 << 3]).unwrap(), 960); // CELT 20 ms
        assert_eq!(packet_samples(&[16 << 3]).unwrap(), 120); // CELT 2.5 ms
        assert_eq!(packet_samples(&[1 << 3]).unwrap(), 960); // SILK 20 ms
        assert_eq!(packet_samples(&[3 << 3]).unwrap(), 2880); // SILK 60 ms
        assert_eq!(packet_samples(&[13 << 3]).unwrap(), 960); // hybrid 20 ms
        assert_eq!(packet_samples(&[(31 << 3) | 1]).unwrap(), 1920); // two frames
        assert_eq!(packet_samples(&[(31 << 3) | 3, 6]).unwrap(), 5760); // six frames
        assert!(packet_samples(&[(31 << 3) | 3, 7]).is_err()); // 140 ms
        assert!(packet_samples(&[(31 << 3) | 3]).is_err());
        assert!(packet_samples(&[]).is_err());
    }

    #[test]
    fn inspect_reads_one_direct_audio_session_exactly() {
        let dir = tempdir::Dir::new();
        let path = write_temp(&dir, "a.sgmca", &synthetic_direct(250, 3840, 480, |_| 60));
        let info = inspect(&path, None).unwrap();
        assert_eq!(info.container, Container::DirectAudio);
        assert_eq!(info.file_version, Some(1));
        assert!(info.guid_present);
        assert_eq!(info.stream_offset, 96);
        assert!(info.sgmca_layout);
        assert!(!info.growing);
        assert_eq!(info.head.channels, 4);
        assert_eq!(info.audio_packets, 250);
        assert_eq!(info.final_granule, 250 * 960 - 480);
        assert_eq!(info.samples, 250 * 960 - 480 - 3840);
        assert!((info.duration_seconds - 4.91).abs() < 1e-9);
        assert_eq!(info.tags.comments, ["TITLE=x"]);
        assert_eq!(info.segments.len(), 1);
        let segment = &info.segments[0];
        assert_eq!(segment.header_offset, Some(40));
        assert_eq!(segment.bytes_read, segment.payload_length);
        let start = segment.start.as_ref().unwrap();
        assert_eq!(start.text, START_1);
        assert_eq!(start.local.as_deref(), Some("2026-11-22T10:15:30"));
        assert_eq!(start.utc_offset.as_deref(), Some("-05:00"));
        assert!(segment.end_of_stream && segment.complete);
        assert!(info.notes.is_empty(), "{:?}", info.notes);
    }

    #[test]
    fn inspect_walks_every_session_and_joins_them_in_order() {
        let dir = tempdir::Dir::new();
        let (file, streams) = three_sessions();
        let path = write_temp(&dir, "three.sgmca", &file);
        let info = inspect(&path, None).unwrap();
        assert_eq!(info.segments.len(), 3);
        let expected = [100 * 960 - 312, 250 * 960 - 480 - 3840, 60 * 960 - 100 - 312];
        let mut offset = 40u64;
        let mut start_sample = 0;
        for ((segment, stream), samples) in info.segments.iter().zip(&streams).zip(expected) {
            assert_eq!(segment.header_offset, Some(offset));
            assert_eq!(segment.data_offset, offset + 56);
            assert_eq!(segment.payload_length, stream.len() as u64);
            assert_eq!(segment.samples, samples);
            assert_eq!(segment.start_sample, start_sample);
            start_sample += samples;
            offset += 56 + stream.len() as u64;
        }
        assert_eq!(info.samples, expected.iter().sum::<u64>());
        assert_eq!(
            info.segments
                .iter()
                .map(|segment| segment.start.as_ref().unwrap().text.as_str())
                .collect::<Vec<_>>(),
            [START_1, START_2, START_3]
        );
        assert_eq!(info.segments[1].head.mapping_family, 1);
        // Family 1 is read, but only family 255 is the live-recording layout.
        assert!(!info.sgmca_layout);
        assert!(!info.growing);
    }

    #[test]
    fn family_zero_mono_and_stereo_are_read() {
        let dir = tempdir::Dir::new();
        let mut stereo = sgmca_head(2, 312);
        stereo.mapping_family = 0;
        stereo.stream_count = 1;
        stereo.coupled_count = 1;
        let path = write_temp(
            &dir,
            "stereo.sgmca",
            &direct_audio(&[(START_1, &stream_with(&stereo, 4, 20, 0, |_| 30))]),
        );
        let info = inspect(&path, None).unwrap();
        assert_eq!(info.head.channels, 2);
        assert_eq!(info.head.mapping_family, 0);
    }

    #[test]
    fn sessions_with_different_channel_counts_are_refused() {
        let dir = tempdir::Dir::new();
        let file = direct_audio(&[
            (START_1, &stream_with(&sgmca_head(4, 312), 1, 20, 0, |_| 30)),
            (START_2, &stream_with(&sgmca_head(2, 312), 2, 20, 0, |_| 30)),
        ]);
        let error = inspect(&write_temp(&dir, "mixed.sgmca", &file), None).unwrap_err();
        assert!(
            error.contains("Segment 2 has 2 channels but segment 1 has 4"),
            "{error}"
        );

        let mut stereo = sgmca_head(2, 312);
        stereo.mapping_family = 0;
        stereo.stream_count = 1;
        stereo.coupled_count = 1;
        let file = direct_audio(&[
            (START_1, &stream_with(&sgmca_head(2, 312), 1, 20, 0, |_| 30)),
            (START_2, &stream_with(&stereo, 2, 20, 0, |_| 30)),
        ]);
        let error = inspect(&write_temp(&dir, "layouts.sgmca", &file), None).unwrap_err();
        assert!(error.contains("cannot be joined"), "{error}");
    }

    #[test]
    fn other_opus_layouts_are_refused() {
        let dir = tempdir::Dir::new();
        let mut coupled = sgmca_head(4, 312);
        coupled.mapping_family = 1;
        coupled.stream_count = 2;
        coupled.coupled_count = 2;
        let file = direct_audio(&[(START_1, &stream_with(&coupled, 1, 20, 0, |_| 30))]);
        let error = inspect(&write_temp(&dir, "coupled.sgmca", &file), None).unwrap_err();
        assert!(error.contains("mapping family 1 with 2 streams (2 coupled)"), "{error}");
    }

    #[test]
    fn wrong_file_and_segment_headers_are_refused_with_their_reason() {
        let dir = tempdir::Dir::new();
        let good = synthetic_direct(10, 312, 0, |_| 30);
        let cases: [(&str, usize, u32, &str); 3] = [
            ("size", 0, 0x30, "says it is 48 bytes long"),
            ("version", 4, 2, "format version 2"),
            (
                "segment-size",
                40,
                0x30,
                "Segment 1's header (byte 40) says it is 48 bytes long, but a segment header needs 56",
            ),
        ];
        for (name, at, value, message) in cases {
            let mut bytes = good.clone();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            let path = write_temp(&dir, &format!("{name}.sgmca"), &bytes);
            let error = inspect(&path, None).unwrap_err();
            assert!(error.contains(message), "{name}: {error}");
        }
        let short = write_temp(&dir, "short.sgmca", &good[..30]);
        assert!(inspect(&short, None).unwrap_err().contains("shorter than the 40-byte"));
        let empty = write_temp(&dir, "empty.sgmca", &[]);
        assert!(inspect(&empty, None).unwrap_err().contains("empty"));
        let random = write_temp(&dir, "random.sgmca", &[0x5a; 100]);
        assert!(inspect(&random, None).unwrap_err().contains("not a Direct Audio file"));
        let header_only = write_temp(&dir, "header.sgmca", &good[..40]);
        assert!(inspect(&header_only, None)
            .unwrap_err()
            .contains("no recorded audio yet"));
    }

    #[test]
    fn a_bare_ogg_stream_is_read_as_the_older_layout() {
        let dir = tempdir::Dir::new();
        let path = write_temp(&dir, "bare.sgmca", &synthetic(50, 312, 0, |_| 30));
        let info = inspect(&path, None).unwrap();
        assert_eq!(info.container, Container::BareOgg);
        assert_eq!(info.file_version, None);
        assert!(!info.guid_present);
        assert_eq!(info.stream_offset, 0);
        assert_eq!(info.segments.len(), 1);
        assert_eq!(info.segments[0].start, None);
        assert_eq!(info.samples, 50 * 960 - 312);
        assert!(info.notes.iter().any(|note| note.contains("bare Ogg Opus stream")));
    }

    #[test]
    fn a_still_growing_last_session_is_read_up_to_its_last_complete_page() {
        let dir = tempdir::Dir::new();
        let (file, streams) = three_sessions();
        let full = inspect(&write_temp(&dir, "full.sgmca", &file), None).unwrap();

        // Cut 10 bytes into the last session's last page: its header still
        // counts the whole payload.
        let cut = write_temp(&dir, "cut.sgmca", &file[..file.len() - 10]);
        let info = inspect(&cut, None).unwrap();
        assert!(info.growing);
        assert_eq!(info.segments.len(), 3);
        let last = &info.segments[2];
        assert!(!last.complete && !last.end_of_stream);
        assert!(last.bytes_read < streams[2].len() as u64 - 10);
        assert!(last.samples < full.segments[2].samples && last.samples > 0);
        assert_eq!(
            info.segments[..2].iter().map(|s| s.samples).sum::<u64>(),
            full.segments[..2].iter().map(|s| s.samples).sum::<u64>()
        );
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("shorter than its header says")));

        // A page appended after the last header update: the header counts
        // fewer bytes than the file holds.
        let mut lagging = file.clone();
        lagging.extend_from_slice(&streams[2][..200]);
        let info = inspect(&write_temp(&dir, "lag.sgmca", &lagging), None).unwrap();
        assert!(info.growing);
        assert_eq!(info.unread_bytes, 200);
        assert_eq!(info.samples, full.samples);
        assert!(info.notes.iter().any(|note| note.contains("200 bytes")));

        // A new session whose header was written but not yet updated.
        let mut started = file.clone();
        started.extend_from_slice(&segment_header(START_3, 0).unwrap());
        started.extend_from_slice(&streams[0][..100]);
        let info = inspect(&write_temp(&dir, "started.sgmca", &started), None).unwrap();
        assert!(info.growing);
        assert_eq!(info.segments.len(), 3);
        assert!(info.segments.iter().all(|segment| segment.complete));
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("Segment 4 has a header but its length is not written yet")));
        assert!(!info.notes.iter().any(|note| note.contains("shorter than its header")));
        // The same with nothing after the new header.
        let header_only = [file.as_slice(), &segment_header(START_3, 0).unwrap()].concat();
        let info = inspect(&write_temp(&dir, "header-only.sgmca", &header_only), None).unwrap();
        assert!(info.growing);
        assert_eq!(info.unread_bytes, 0);

        // A last session cut inside its headers holds no audio yet: it is
        // left out and the earlier ones are kept.
        let mut early = file[..file.len() - streams[2].len() - 56].to_vec();
        early.extend_from_slice(&segment_header(START_3, streams[2].len() as u64).unwrap());
        early.extend_from_slice(&streams[2][..40]);
        let info = inspect(&write_temp(&dir, "early.sgmca", &early), None).unwrap();
        assert!(info.growing);
        assert_eq!(info.segments.len(), 2);
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("Segment 3 has no complete audio yet")));
    }

    #[test]
    fn damage_inside_a_session_is_named_with_its_segment_and_byte() {
        let dir = tempdir::Dir::new();
        let (file, streams) = three_sessions();
        let mut corrupt = file.clone();
        let second = 40 + 56 + streams[0].len() + 56;
        corrupt[second + streams[1].len() / 2] ^= 0x01;
        let error = inspect(&write_temp(&dir, "crc.sgmca", &corrupt), None).unwrap_err();
        assert!(
            error.starts_with("Segment 2:") && error.contains("fails its checksum"),
            "{error}"
        );

        // A non-last session that ends early is damage, not growth.
        let mut short = direct_audio(&[(START_1, &streams[0][..streams[0].len() - 10])]);
        short.extend_from_slice(&segment_header(START_2, streams[1].len() as u64).unwrap());
        short.extend_from_slice(&streams[1]);
        let error = inspect(&write_temp(&dir, "short.sgmca", &short), None).unwrap_err();
        assert!(
            error.starts_with("Segment 1:") && error.contains("truncated"),
            "{error}"
        );

        // Bytes after a session's end-of-stream page.
        let mut padded = streams[0].clone();
        padded.extend_from_slice(&[0u8; 16]);
        let error = inspect(
            &write_temp(&dir, "pad.sgmca", &direct_audio(&[(START_1, &padded)])),
            None,
        )
        .unwrap_err();
        assert!(error.contains("16 bytes after its end-of-stream page"), "{error}");

        // A session holding another codec.
        let mut vorbis = OggWriter::new(Vec::new(), 1);
        vorbis.write_packet(b"\x01vorbis....", 0).unwrap();
        vorbis.flush(true).unwrap();
        let vorbis = direct_audio(&[(START_1, &vorbis.into_inner())]);
        let error = inspect(&write_temp(&dir, "vorbis.sgmca", &vorbis), None).unwrap_err();
        assert!(error.contains("Vorbis"), "{error}");
    }

    #[test]
    fn an_unreadable_start_time_is_kept_and_named() {
        let dir = tempdir::Dir::new();
        let file = direct_audio(&[("yesterday", &synthetic(10, 312, 0, |_| 30))]);
        let info = inspect(&write_temp(&dir, "when.sgmca", &file), None).unwrap();
        let start = info.segments[0].start.as_ref().unwrap();
        assert_eq!(start.text, "yesterday");
        assert_eq!(start.local, None);
        assert!(info.notes.iter().any(|note| note.contains("\"yesterday\"")));
    }

    #[test]
    fn concat_script_points_ffmpeg_at_each_session_in_place() {
        let dir = tempdir::Dir::new();
        let (file, streams) = three_sessions();
        let path = write_temp(&dir, "it's three.sgmca", &file);
        let info = inspect(&path, None).unwrap();
        let script = concat_script(&path, &info).unwrap();
        let lines: Vec<&str> = script.lines().collect();
        assert_eq!(lines[0], "ffconcat version 1.0");
        assert_eq!(lines.len(), 7);
        let quoted = path.to_str().unwrap().replace('\'', "'\\''");
        let mut offset = 40 + 56;
        for (index, stream) in streams.iter().enumerate() {
            let end = offset + stream.len();
            assert_eq!(
                lines[1 + index * 2],
                format!("file 'subfile,,start,{offset},end,{end},,:file:{quoted}'")
            );
            assert_eq!(
                lines[2 + index * 2],
                format!("duration {:.6}", info.segments[index].duration_seconds)
            );
            offset = end + 56;
        }
    }

    #[test]
    fn slice_copies_packets_with_pre_roll_pre_skip_and_end_trim() {
        let dir = tempdir::Dir::new();
        let src = write_temp(
            &dir,
            "src.sgmca",
            &synthetic_direct(500, 312, 0, |index| 20 + index % 11),
        );
        let dst = dir.0.join("cut.sgmca");
        // 2.0 s to 3.5 s: 72 000 samples.
        let report = slice(&src, &dst, 2.0, Some(3.5), None).unwrap();
        assert_eq!(report.samples, 72_000);
        assert!((report.end_seconds - 3.5).abs() < 1e-9);
        assert_eq!(report.start_text, "2026-11-22T10:15:32-0500");
        assert_eq!(report.start_offset_ms, Some(0.0));
        assert!(report.start_note.contains("whole second"), "{}", report.start_note);
        assert!(report.pre_skip as u64 >= PREROLL_SAMPLES);

        let info = verify_written(&dst, Some(&["2026-11-22T10:15:32-0500".to_string()]), None).unwrap();
        assert_eq!(info.start_granule, 0);
        assert_eq!(info.head.pre_skip, report.pre_skip);
        assert_eq!(info.samples, 72_000);
        assert_eq!(info.audio_packets, report.audio_packets);

        // The copied packets are the source's own bytes, unchanged: packet
        // contents are tagged with their index in the fill byte.
        let bytes = fs::read(&dst).unwrap();
        let mut packets = PacketReader::new(&bytes[96..], 96);
        let mut fills = Vec::new();
        while let Some(page) = packets.next().unwrap() {
            fills.extend(
                page.packets
                    .iter()
                    .filter(|p| p.len() > 1 && p[0] == 31 << 3)
                    .map(|p| p[1]),
            );
        }
        let first = fills[0] as usize;
        assert!(fills
            .iter()
            .enumerate()
            .all(|(offset, &fill)| fill as usize == (first + offset) % 256));
        // Start of slice is sample 312 + 96 000 of the source timeline; the
        // first copied packet begins at least 80 ms before it.
        assert!((first as u64) * 960 <= 312 + 96_000 - PREROLL_SAMPLES);
        assert_eq!(first as u64 * 960 + report.pre_skip as u64, 312 + 96_000);
    }

    #[test]
    fn whole_second_offsets_are_given_in_milliseconds_with_a_plain_note() {
        assert_eq!(offset_ms(0, 48_000), 0.0);
        assert_eq!(offset_ms(20_160, 48_000), 420.0);
        assert_eq!(offset_ms(1, 44_100), 0.023);
        assert_eq!(offset_ms(44_099, 44_100), 999.977);
        assert_eq!(offset_ms(5, 0), 0.0);
        assert_eq!(
            whole_second_note(420.0),
            "CATalyst stores whole seconds; this recording's first sample is 0.42 s after the time written."
        );
        assert!(whole_second_note(999.977).contains(" 0.999977 s "));
    }

    #[test]
    fn slice_from_zero_keeps_the_original_pre_skip_and_runs_to_the_end() {
        let dir = tempdir::Dir::new();
        let src = write_temp(&dir, "src.sgmca", &synthetic_direct(100, 3840, 300, |_| 25));
        let dst = dir.0.join("all.sgmca");
        let report = slice(&src, &dst, 0.0, None, None).unwrap();
        let original = inspect(&src, None).unwrap();
        assert_eq!(report.pre_skip, 3840);
        assert_eq!(report.samples, original.samples);
        assert_eq!(
            verify_written(&dst, Some(&[START_1.to_string()]), None)
                .unwrap()
                .samples,
            original.samples
        );
    }

    #[test]
    fn slice_stays_inside_one_session() {
        let dir = tempdir::Dir::new();
        let (file, _) = three_sessions();
        let src = write_temp(&dir, "three.sgmca", &file);
        let info = inspect(&src, None).unwrap();
        let second = &info.segments[1];
        let second_start = second.start_sample as f64 / 48_000.0;
        let second_end = (second.start_sample + second.samples) as f64 / 48_000.0;

        // Inside the second session: its own start time, moved by the whole
        // seconds skipped (2.5 s in, so 2 s). The cut is exactly where it was
        // asked for: half a second, nothing before 2.5 s added or removed, and
        // the half second left over is reported, not cut.
        let inside = dir.0.join("inside.sgmca");
        let report = slice(&src, &inside, second_start + 2.5, Some(second_start + 3.0), None).unwrap();
        assert_eq!(report.samples, 24_000);
        assert_eq!(report.start_seconds, (second.start_sample + 120_000) as f64 / 48_000.0);
        assert_eq!(report.start_offset_ms, Some(500.0));
        assert!(
            report.start_note.contains(
                "CATalyst stores whole seconds; this recording's first sample is 0.5 s after the time written."
            ),
            "{}",
            report.start_note
        );
        assert!(report.start_note.contains("no audio was left out"));
        let written = inspect(&inside, None).unwrap();
        assert_eq!(written.samples, 24_000);
        assert_eq!(
            written.segments[0].start.as_ref().unwrap().text,
            "2026-11-22T11:02:07-0500"
        );
        // Family 1 is copied as is, so the writer check (family 255 only)
        // refuses it; the reader reads it.
        assert_eq!(written.head.mapping_family, 1);

        let across = dir.0.join("across.sgmca");
        let error = slice(&src, &across, second_start + 1.0, Some(second_end + 0.5), None).unwrap_err();
        assert!(error.contains("from recording session 2 into the next"), "{error}");
        assert!(!across.exists());
    }

    #[test]
    fn slice_rejects_bad_ranges_and_never_overwrites() {
        let dir = tempdir::Dir::new();
        let src = write_temp(&dir, "src.sgmca", &synthetic_direct(50, 312, 0, |_| 25));
        let dst = dir.0.join("out.sgmca");
        assert!(slice(&src, &dst, -1.0, None, None).is_err());
        assert!(slice(&src, &dst, 2.0, Some(1.0), None).is_err());
        assert!(slice(&src, &dst, 30.0, None, None)
            .unwrap_err()
            .contains("past the end"));
        assert!(!dst.exists(), "a failed slice leaves no file behind");
        fs::write(&dst, b"keep me").unwrap();
        assert!(slice(&src, &dst, 0.0, None, None).is_err());
        assert_eq!(fs::read(&dst).unwrap(), b"keep me");
    }

    #[test]
    fn one_segment_output_is_the_header_then_the_ogg_bytes_unchanged() {
        let dir = tempdir::Dir::new();
        let ogg_bytes = synthetic(120, 312, 0, |_| 30);
        let ogg = write_temp(&dir, "encoded.ogg", &ogg_bytes);
        let dst = dir.0.join("out.sgmca");
        let plan = [PlannedSegment {
            start_text: START_1.into(),
            start_sample: 0,
        }];
        write_direct_audio(&ogg, &dst, &plan, None).unwrap();
        let bytes = fs::read(&dst).unwrap();
        assert_eq!(bytes.len(), 96 + ogg_bytes.len());
        assert_eq!(&bytes[0..8], &[0x28, 0, 0, 0, 1, 0, 0, 0]);
        assert!(bytes[8..24].iter().any(|&byte| byte != 0), "a fresh identifier");
        assert_eq!(bytes[8 + 7] >> 4, 4, "a version 4 (random) GUID");
        assert_eq!(&bytes[24..40], &[0; 16]);
        assert_eq!(&bytes[40..56], &[0x38, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[56..80], START_1.as_bytes());
        assert_eq!(&bytes[80..88], &[0; 8]);
        assert_eq!(
            u64::from_le_bytes(bytes[88..96].try_into().unwrap()),
            ogg_bytes.len() as u64
        );
        assert_eq!(&bytes[96..], &ogg_bytes[..]);
        verify_written(&dst, Some(&[START_1.to_string()]), None).unwrap();
        // Two writes never share an identifier, and never replace a file.
        let again = dir.0.join("again.sgmca");
        write_direct_audio(&ogg, &again, &plan, None).unwrap();
        assert_ne!(fs::read(&again).unwrap()[8..24], bytes[8..24]);
        assert!(write_direct_audio(&ogg, &again, &plan, None).is_err());
    }

    #[test]
    fn multi_segment_output_cuts_each_session_at_its_planned_sample() {
        let dir = tempdir::Dir::new();
        let ogg = write_temp(&dir, "encoded.ogg", &synthetic(300, 312, 200, |index| 20 + index % 5));
        let total = 300 * 960 - 200 - 312;
        let dst = dir.0.join("out.sgmca");
        let plan = [
            PlannedSegment {
                start_text: START_1.into(),
                start_sample: 0,
            },
            PlannedSegment {
                start_text: START_2.into(),
                start_sample: 70_001,
            },
            PlannedSegment {
                start_text: START_3.into(),
                start_sample: 200_000,
            },
        ];
        write_direct_audio(&ogg, &dst, &plan, None).unwrap();
        let starts: Vec<String> = plan.iter().map(|segment| segment.start_text.clone()).collect();
        let info = verify_written(&dst, Some(&starts), None).unwrap();
        assert_eq!(
            info.segments.iter().map(|segment| segment.samples).collect::<Vec<_>>(),
            [70_001, 200_000 - 70_001, total - 200_000]
        );
        let serials: std::collections::HashSet<u32> = info
            .segments
            .iter()
            .map(|segment| {
                let bytes = fs::read(&dst).unwrap();
                let at = segment.data_offset as usize;
                u32::from_le_bytes(bytes[at + 14..at + 18].try_into().unwrap())
            })
            .collect();
        assert_eq!(serials.len(), 3, "each session is its own Ogg stream");

        // A plan past the end of the audio fails and leaves nothing behind.
        let past = dir.0.join("past.sgmca");
        let bad = [
            plan[0].clone(),
            PlannedSegment {
                start_text: START_2.into(),
                start_sample: total + 10_000,
            },
        ];
        assert!(write_direct_audio(&ogg, &past, &bad, None).is_err());
        assert!(!past.exists());
        assert!(
            write_direct_audio(&ogg, &past, &[plan[0].clone(), plan[2].clone(), plan[1].clone()], None)
                .unwrap_err()
                .contains("not in order")
        );
        assert!(write_direct_audio(&ogg, &past, &[], None)
            .unwrap_err()
            .contains("not in order"));
        assert!(!past.exists());

        // A plan whose first segment starts after sample 0 would leave the
        // audio before it out, so it is refused and nothing is written.
        let later = dir.0.join("later.sgmca");
        assert!(write_direct_audio(&ogg, &later, &[plan[1].clone()], None)
            .unwrap_err()
            .contains("audio would be left out"));
        assert!(!later.exists());
    }

    #[test]
    fn verify_written_rejects_anything_but_complete_family_255_direct_audio() {
        let dir = tempdir::Dir::new();
        let bare = write_temp(&dir, "bare.sgmca", &synthetic(20, 312, 0, |_| 30));
        assert!(verify_written(&bare, None, None)
            .unwrap_err()
            .contains("no Direct Audio file header"));

        let mut stereo = sgmca_head(2, 312);
        stereo.mapping_family = 0;
        stereo.stream_count = 1;
        stereo.coupled_count = 1;
        let coupled = write_temp(
            &dir,
            "stereo.sgmca",
            &direct_audio(&[(START_1, &stream_with(&stereo, 7, 10, 0, |_| 20))]),
        );
        assert!(verify_written(&coupled, None, None)
            .unwrap_err()
            .contains("mapping family 0"));

        let good = synthetic_direct(200, 312, 0, |_| 30);
        let path = write_temp(&dir, "good.sgmca", &good);
        assert!(verify_written(&path, Some(&[START_2.to_string()]), None)
            .unwrap_err()
            .contains("not the ones planned"));
        let cut = write_temp(&dir, "cut.sgmca", &good[..good.len() - 5]);
        let error = verify_written(&cut, None, None).unwrap_err();
        assert!(error.contains("incomplete"), "{error}");
        let mut anonymous = good.clone();
        anonymous[8..24].copy_from_slice(&[0; 16]);
        let anonymous = write_temp(&dir, "anon.sgmca", &anonymous);
        assert!(verify_written(&anonymous, None, None)
            .unwrap_err()
            .contains("identifier"));
        // A start text CATalyst could not parse fails the check, even when it
        // is exactly the text that was planned.
        let undated = write_temp(
            &dir,
            "undated.sgmca",
            &direct_audio(&[("2026-11-22 10:15:30", &synthetic(20, 312, 0, |_| 30))]),
        );
        let error = verify_written(&undated, Some(&["2026-11-22 10:15:30".to_string()]), None).unwrap_err();
        assert!(error.contains("segment 1 has no start time in the"), "{error}");
    }

    // ── Owner-supplied samples and edge cases ──────────────────────────────

    fn vendor_sample(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/fixtures/sgmca/vendor-notes-samples")
            .join(name)
    }

    /// Where a stroke at wall-clock `time` (HH:MM:SS on the session's day)
    /// lands on the joined timeline, by the rule the notes describe: sessions
    /// laid end to end, a stroke in a gap snaps to the next session's start.
    fn timeline_seconds(info: &SgmcaInfo, clock_text: &str) -> f64 {
        let stroke = |start: &SegmentStart| {
            let (local, _) = parse_start_text(&start.text).unwrap();
            let clock = time::Time::parse(
                clock_text,
                time::macros::format_description!("[hour]:[minute]:[second]"),
            )
            .unwrap();
            (time::PrimitiveDateTime::new(local.date(), clock) - local).as_seconds_f64()
        };
        for segment in &info.segments {
            let into = stroke(segment.start.as_ref().unwrap());
            let joined = segment.start_sample as f64 / 48_000.0;
            if into < 0.0 {
                return joined;
            }
            if into < segment.duration_seconds {
                return joined + into;
            }
        }
        info.duration_seconds
    }

    #[test]
    fn vendor_notes_samples_read_as_the_notes_describe() {
        // Synthetic tones wrapped by the owner's own tool, not by CATalyst
        // (see scripts/fixtures/sgmca/vendor-notes-samples/README.md).
        let one = inspect(&vendor_sample("tone_1segment.sgmca"), None).unwrap();
        let two = inspect(&vendor_sample("tone_2sessions.sgmca"), None).unwrap();
        for info in [&one, &two] {
            assert_eq!(info.container, Container::DirectAudio);
            assert_eq!(info.file_version, Some(1));
            assert!(info.guid_present);
            assert!(!info.growing);
            assert_eq!(info.unread_bytes, 0);
            assert!(info.sgmca_layout);
            assert!(info.notes.is_empty(), "{:?}", info.notes);
            for segment in &info.segments {
                assert_eq!(segment.head.channels, 2);
                assert_eq!(segment.head.mapping_family, 255);
                assert_eq!((segment.head.stream_count, segment.head.coupled_count), (2, 0));
                assert_eq!(segment.head.pre_skip, 312);
                assert_eq!(segment.head.input_sample_rate, 48_000);
                assert!(segment.complete && segment.end_of_stream);
                assert_eq!(segment.bytes_read, segment.payload_length);
            }
        }
        assert_eq!(one.segments.len(), 1);
        let only = &one.segments[0];
        assert_eq!(only.data_offset, 96);
        assert_eq!(only.payload_length, 110_401);
        assert_eq!(only.final_granule, 480_312);
        assert_eq!(only.samples, 480_000);
        assert_eq!(only.start.as_ref().unwrap().text, "2026-11-22T10:15:30-0500");
        assert_eq!(
            fs::metadata(vendor_sample("tone_1segment.sgmca")).unwrap().len(),
            96 + 110_401
        );

        assert_eq!(two.segments.len(), 2);
        let (first, second) = (&two.segments[0], &two.segments[1]);
        assert_eq!((first.payload_length, second.payload_length), (110_401, 44_952));
        assert_eq!(second.header_offset, Some(0x1AFA1));
        assert_eq!(second.data_offset, 0x1AFD9);
        assert_eq!((first.final_granule, second.final_granule), (480_312, 240_312));
        assert_eq!((first.samples, second.samples), (480_000, 240_000));
        assert_eq!((first.start_sample, second.start_sample), (0, 480_000));
        assert_eq!(two.samples, 720_000);
        assert_eq!(two.duration_seconds, 15.0);
        assert_eq!(first.start.as_ref().unwrap().text, "2026-11-22T10:15:30-0500");
        assert_eq!(second.start.as_ref().unwrap().text, "2026-11-22T10:20:00-0500");
        assert_eq!(
            second.start.as_ref().unwrap().local.as_deref(),
            Some("2026-11-22T10:20:00")
        );
        // The notes' timeline expectations for this file.
        assert_eq!(timeline_seconds(&two, "10:15:35"), 5.0);
        assert_eq!(timeline_seconds(&two, "10:20:02"), 12.0);
        assert_eq!(timeline_seconds(&two, "10:18:00"), 10.0);
        assert_eq!(
            fs::metadata(vendor_sample("tone_2sessions.sgmca")).unwrap().len(),
            40 + 56 + 110_401 + 56 + 44_952
        );

        // A slice from 2.7 s to 4.0 s is cut exactly there (1.3 s of audio);
        // its header names 10:15:32, the whole second before its first
        // sample, and the 0.7 s left over is reported.
        let dir = tempdir::Dir::new();
        let cut = dir.0.join("cut.sgmca");
        let report = slice(&vendor_sample("tone_1segment.sgmca"), &cut, 2.7, Some(4.0), None).unwrap();
        assert!((report.start_seconds - 2.7).abs() < 1e-9);
        assert_eq!(report.samples, 62_400);
        assert_eq!(report.start_offset_ms, Some(700.0));
        assert!(report
            .start_note
            .contains("first sample is 0.7 s after the time written"));
        let written = verify_written(&cut, Some(&["2026-11-22T10:15:32-0500".to_string()]), None).unwrap();
        assert_eq!(written.samples, 62_400);
    }

    #[test]
    fn a_live_session_with_only_its_first_pages_is_left_out_not_refused() {
        let dir = tempdir::Dir::new();
        let (file, streams) = three_sessions();
        let full = inspect(&write_temp(&dir, "full.sgmca", &file), None).unwrap();
        // The OpusHead page alone, with the length rewritten to cover it,
        // exactly as the notes say the recorder leaves it after one page.
        let head_page = {
            let mut pages = PageReader::new(&streams[2][..], 0);
            pages.next_page().unwrap().unwrap();
            pages.position() as usize
        };
        let two_and_head = file.len() - streams[2].len() + head_page;
        let mut live = file[..file.len() - streams[2].len() - 56].to_vec();
        live.extend_from_slice(&segment_header(START_3, head_page as u64).unwrap());
        live.extend_from_slice(&streams[2][..head_page]);
        assert_eq!(live.len(), two_and_head);
        let info = inspect(&write_temp(&dir, "live.sgmca", &live), None).unwrap();
        assert_eq!(info.segments.len(), 2);
        assert!(info.growing);
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("Segment 3 has no complete audio yet")));
        // The headers but no audio page yet, counted by the length.
        let tags_page = {
            let mut pages = PageReader::new(&streams[2][..], 0);
            pages.next_page().unwrap().unwrap();
            pages.next_page().unwrap().unwrap();
            pages.position() as usize
        };
        let mut headers = file[..file.len() - streams[2].len() - 56].to_vec();
        headers.extend_from_slice(&segment_header(START_3, tags_page as u64).unwrap());
        headers.extend_from_slice(&streams[2][..tags_page]);
        let info = inspect(&write_temp(&dir, "headers.sgmca", &headers), None).unwrap();
        assert_eq!(info.segments.len(), 2);
        assert!(info.growing);
        // A segment header with a full length and no bytes after it.
        let mut empty = file[..file.len() - streams[2].len() - 56].to_vec();
        empty.extend_from_slice(&segment_header(START_3, streams[2].len() as u64).unwrap());
        let info = inspect(&write_temp(&dir, "empty.sgmca", &empty), None).unwrap();
        assert_eq!(info.segments.len(), 2);

        // Every cut of the file, byte by byte: the sessions finished before
        // the cut are always read, and nothing else is ever an error.
        let first_end = 40 + 56 + streams[0].len();
        for length in 40..file.len() {
            let path = write_temp(&dir, "sweep.sgmca", &file[..length]);
            match inspect(&path, None) {
                Ok(info) => {
                    let finished = full
                        .segments
                        .iter()
                        .filter(|segment| (segment.data_offset + segment.payload_length) as usize <= length)
                        .count();
                    assert!(info.segments.len() >= finished, "cut at {length}");
                    for (read, whole) in info.segments.iter().zip(&full.segments).take(finished) {
                        assert_eq!(read.samples, whole.samples, "cut at {length}");
                    }
                    // A cut exactly between two sessions looks like a
                    // complete file with fewer sessions.
                    let boundary = full
                        .segments
                        .iter()
                        .any(|segment| (segment.data_offset + segment.payload_length) as usize == length);
                    assert_eq!(info.growing, !boundary, "cut at {length}");
                }
                Err(error) => {
                    assert!(length < first_end, "cut at {length}: {error}");
                    assert!(
                        error.contains("no recorded audio yet")
                            || error.contains("no complete audio yet")
                            || error.contains("ends inside its first segment header")
                            || error.contains("holds any audio samples"),
                        "cut at {length}: {error}"
                    );
                }
            }
        }
    }

    #[test]
    fn later_sessions_behind_a_short_length_are_never_dropped() {
        let dir = tempdir::Dir::new();
        let (_, streams) = three_sessions();
        // Session 2's length was never written (0), then session 3 recorded.
        let mut zero = direct_audio(&[(START_1, &streams[0])]);
        zero.extend_from_slice(&segment_header(START_2, 0).unwrap());
        zero.extend_from_slice(&streams[1]);
        zero.extend_from_slice(&segment_header(START_3, streams[2].len() as u64).unwrap());
        zero.extend_from_slice(&streams[2]);
        let error = inspect(&write_temp(&dir, "zero.sgmca", &zero), None).unwrap_err();
        assert!(
            error.contains("Segment 2's length in its header does not cover all of its audio")
                && error.contains("rather than leave sessions out"),
            "{error}"
        );

        // Session 2's length stops before its last page, then session 3.
        let last_page = {
            let mut pages = PageReader::new(&streams[1][..], 0);
            let mut starts = Vec::new();
            while let Some(page) = pages.next_page().unwrap() {
                starts.push(page.offset);
            }
            *starts.last().unwrap()
        };
        let mut lagging = direct_audio(&[(START_1, &streams[0])]);
        lagging.extend_from_slice(&segment_header(START_2, last_page).unwrap());
        lagging.extend_from_slice(&streams[1]);
        lagging.extend_from_slice(&segment_header(START_3, streams[2].len() as u64).unwrap());
        lagging.extend_from_slice(&streams[2]);
        let error = inspect(&write_temp(&dir, "lagging.sgmca", &lagging), None).unwrap_err();
        assert!(error.contains("another recording session's header follows"), "{error}");

        // A session whose length is 0 and which holds nothing: the next
        // header follows straight after it, so the walk goes on.
        let mut hollow = direct_audio(&[(START_1, &streams[0])]);
        hollow.extend_from_slice(&segment_header(START_2, 0).unwrap());
        hollow.extend_from_slice(&segment_header(START_3, streams[2].len() as u64).unwrap());
        hollow.extend_from_slice(&streams[2]);
        let info = inspect(&write_temp(&dir, "hollow.sgmca", &hollow), None).unwrap();
        assert_eq!(info.segments.len(), 2);
        assert_eq!(info.segments[1].number, 3);
        assert!(!info.growing);
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("Segment 2 (byte") && note.contains("length of 0")));
    }

    #[test]
    fn fields_catalyst_does_not_check_are_noted_not_refused() {
        let dir = tempdir::Dir::new();
        let good = synthetic_direct(10, 312, 0, |_| 30);
        let mut odd = good.clone();
        odd[44..48].copy_from_slice(&0u32.to_le_bytes());
        odd[48..52].copy_from_slice(&7u32.to_le_bytes());
        let info = inspect(&write_temp(&dir, "odd.sgmca", &odd), None).unwrap();
        assert_eq!(info.segments.len(), 1);
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("version 0 and record kind 7")));

        // A 64-byte segment header: the audio starts after all 64 bytes.
        let mut long = good[..96].to_vec();
        long[40..44].copy_from_slice(&64u32.to_le_bytes());
        long.extend_from_slice(&[0; 8]);
        long.extend_from_slice(&good[96..]);
        let info = inspect(&write_temp(&dir, "long.sgmca", &long), None).unwrap();
        assert_eq!(info.segments[0].data_offset, 104);
        assert_eq!(
            info.samples,
            inspect(&write_temp(&dir, "good.sgmca", &good), None).unwrap().samples
        );
        assert!(info.notes.iter().any(|note| note.contains("says it is 64 bytes long")));

        // A different output gain on a later session is named, not refused.
        let mut quieter = sgmca_head(4, 312);
        quieter.output_gain = -1536;
        let file = direct_audio(&[
            (START_1, &stream_with(&sgmca_head(4, 312), 1, 20, 0, |_| 30)),
            (START_2, &stream_with(&quieter, 2, 20, 0, |_| 30)),
        ]);
        let info = inspect(&write_temp(&dir, "gain.sgmca", &file), None).unwrap();
        assert_eq!(info.segments.len(), 2);
        assert!(info
            .notes
            .iter()
            .any(|note| note.contains("output gain of -6.00 dB where segment 1 asks for 0.00 dB")));
    }

    /// A 4-channel stream whose audio pages carry `granules` (one packet
    /// per page), written with the module's own writer.
    fn stream_with_granules(granules: &[u64]) -> Vec<u8> {
        let mut writer = OggWriter::new(Vec::new(), 9);
        writer.write_packet(&sgmca_head(4, 312).to_bytes(), 0).unwrap();
        writer.flush(false).unwrap();
        writer.write_packet(&tags_packet("test", &[]), 0).unwrap();
        writer.flush(false).unwrap();
        for (index, &granule) in granules.iter().enumerate() {
            writer.write_packet(&fake_packet(30, index as u8), granule).unwrap();
            writer.flush(index + 1 == granules.len()).unwrap();
        }
        writer.into_inner()
    }

    #[test]
    fn impossible_or_jumping_timestamps_are_refused_without_overflow() {
        let dir = tempdir::Dir::new();
        for (name, granules, message) in [
            ("max", vec![u64::MAX - 1, 5, 6], "impossible timestamp"),
            (
                "signed",
                vec![i64::MAX as u64 + 1, i64::MAX as u64 + 961],
                "impossible timestamp",
            ),
            (
                "edge",
                vec![i64::MAX as u64 - 10, i64::MAX as u64 - 9],
                "past what Ogg can count",
            ),
            ("jump", vec![960, 1 << 50], "its packets end at sample 1920"),
        ] {
            let file = direct_audio(&[(START_1, &stream_with_granules(&granules))]);
            let error = inspect(&write_temp(&dir, &format!("{name}.sgmca"), &file), None).unwrap_err();
            assert!(
                error.starts_with("Segment 1:") && error.contains(message),
                "{name}: {error}"
            );
        }
        // Correct timestamps that do not start at zero are fine.
        let file = direct_audio(&[(START_1, &stream_with_granules(&[48_960, 49_920, 50_880]))]);
        let info = inspect(&write_temp(&dir, "late.sgmca", &file), None).unwrap();
        assert_eq!(info.samples, 3 * 960 - 312);
    }

    #[test]
    fn a_file_whose_sessions_hold_no_samples_is_refused_by_the_reader() {
        let dir = tempdir::Dir::new();
        // One 20 ms packet each, all of it pre-skip.
        let silent = stream_with(&sgmca_head(4, 960), 1, 1, 0, |_| 30);
        let file = direct_audio(&[(START_1, &silent), (START_2, &silent)]);
        let error = inspect(&write_temp(&dir, "silent.sgmca", &file), None).unwrap_err();
        assert!(error.contains("holds any audio samples"), "{error}");
    }
}
