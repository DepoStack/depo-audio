//! Stenograph SGMCA ("Direct Audio") recordings: reading, checking, slicing
//! and writing.
//!
//! An SGMCA recording is an ordinary Ogg stream (RFC 3533) carrying one Opus
//! logical bitstream (RFC 7845) that uses channel mapping family 255: every
//! court channel is its own uncoupled mono Opus stream, so `stream_count`
//! equals the channel count, `coupled_count` is zero and the mapping table is
//! the identity. Nothing in the file is encrypted or vendor-specific beyond
//! the extension, so this module works only from those public specifications:
//!
//! * the Ogg page layout and its CRC-32 (polynomial 0x04C11DB7, MSB first,
//!   initial value 0, no final XOR, CRC field zeroed while it is computed);
//! * the `OpusHead` and `OpusTags` header packets;
//! * the Opus TOC byte (RFC 6716 section 3.1), which gives every packet's
//!   duration without decoding it.
//!
//! Decoding to PCM stays with FFmpeg, which handles multistream Opus. What
//! this module adds is everything FFmpeg cannot tell us cheaply or exactly:
//! where the stream starts, whether its headers are sound, the exact duration
//! (final granule position minus pre-skip), the true channel layout, and a
//! sample-accurate slice that copies Opus packets without re-encoding them.
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

/// How far into a file the Ogg stream may start. Some exports carry a short
/// prefix before the first page; real streams start well inside this window.
pub(crate) const HEADER_SCAN: usize = 8192;

/// Decoder pre-roll kept in front of a slice's first sample (80 ms), as RFC
/// 7845 section 4.6 recommends, so the decoder has converged by the cut.
pub(crate) const PREROLL_SAMPLES: u64 = 3840;

/// Ogg page header flags.
const FLAG_CONTINUED: u8 = 0x01;
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;

/// The fixed part of an Ogg page header, before the segment table.
const PAGE_HEADER_LEN: usize = 27;

/// The largest possible Ogg page: header, 255 lacing values, 255 * 255 bytes.
const MAX_PAGE_LEN: usize = PAGE_HEADER_LEN + 255 + 255 * 255;

/// A granule position of all ones means "no packet finishes on this page".
const NO_GRANULE: u64 = u64::MAX;

/// Pages written by this module are flushed once their body reaches this size
/// or holds this many packets (one second of 20 ms frames), whichever comes
/// first, which keeps seeking fine-grained without wasting header bytes.
const TARGET_PAGE_BODY: usize = 8192;
const TARGET_PAGE_PACKETS: usize = 50;

type CancelCheck<'a> = Option<&'a (dyn Fn() -> bool + Send + Sync)>;

pub(crate) const SLICE_CANCELLED_MESSAGE: &str = "SGMCA slicing cancelled";
pub(crate) const INSPECT_CANCELLED_MESSAGE: &str = "SGMCA inspection cancelled";

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

/// Parse one page from the start of `bytes`. Returns the page and its length,
/// or None when `bytes` does not start with a complete, CRC-correct page.
fn parse_page_at(bytes: &[u8], offset: u64) -> Option<(Page, usize)> {
    if bytes.len() < PAGE_HEADER_LEN || &bytes[..4] != b"OggS" || bytes[4] != 0 {
        return None;
    }
    let segments = bytes[26] as usize;
    let lacing_end = PAGE_HEADER_LEN + segments;
    let lacing = bytes.get(PAGE_HEADER_LEN..lacing_end)?;
    let body_len: usize = lacing.iter().map(|&value| value as usize).sum();
    let total = lacing_end + body_len;
    let page_bytes = bytes.get(..total)?;
    let stored = u32::from_le_bytes(page_bytes[22..26].try_into().ok()?);
    let mut crc = crc_update(0, &page_bytes[..22]);
    crc = crc_update(crc, &[0; 4]);
    crc = crc_update(crc, &page_bytes[26..]);
    if crc != stored {
        return None;
    }
    Some((
        Page {
            header_type: page_bytes[5],
            granule: u64::from_le_bytes(page_bytes[6..14].try_into().ok()?),
            serial: u32::from_le_bytes(page_bytes[14..18].try_into().ok()?),
            sequence: u32::from_le_bytes(page_bytes[18..22].try_into().ok()?),
            lacing: lacing.to_vec(),
            body: page_bytes[lacing_end..].to_vec(),
            offset,
        },
        total,
    ))
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
}

impl<R: Read> PageReader<R> {
    pub(crate) fn new(inner: R, offset: u64) -> Self {
        Self {
            inner,
            offset,
            index: 0,
        }
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
            return Err(format!(
                "The recording ends partway through Ogg page {page_number} (byte {at}); the file looks truncated."
            ));
        }
        let body_len: usize = lacing.iter().map(|&value| value as usize).sum();
        let mut body = vec![0u8; body_len];
        if read_fully(&mut self.inner, &mut body)? < body_len {
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
            pages_read: 0,
        }
    }

    pub(crate) fn serial(&self) -> Option<u32> {
        self.serial
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

// ── Locating the stream ──────────────────────────────────────────────────────

/// What sits at the start of a candidate Ogg stream.
fn codec_of(first_packet: &[u8]) -> &'static str {
    if first_packet.starts_with(b"OpusHead") {
        "Opus"
    } else if first_packet.starts_with(b"\x01vorbis") {
        "Vorbis"
    } else if first_packet.starts_with(b"\x7fFLAC") {
        "FLAC"
    } else if first_packet.starts_with(b"Speex") {
        "Speex"
    } else {
        "an unknown codec"
    }
}

/// Find the Opus stream in an SGMCA file: the first offset inside the header
/// window where a CRC-correct beginning-of-stream page carries `OpusHead`.
/// Stray "OggS" bytes in a prefix are not mistaken for the stream.
pub(crate) fn locate_stream<R: Read + Seek>(file: &mut R) -> Result<u64, String> {
    file.seek(SeekFrom::Start(0)).map_err(|error| error.to_string())?;
    // A beginning-of-stream page may start anywhere in the window and is
    // usually tiny, but read enough for one whole page past the window.
    let mut window = vec![0u8; HEADER_SCAN + MAX_PAGE_LEN];
    let read = read_fully(file, &mut window)?;
    window.truncate(read);
    if window.is_empty() {
        return Err("This SGMCA file is empty.".into());
    }
    let scan_end = window.len().min(HEADER_SCAN);
    let mut saw_marker = false;
    let mut other_codec: Option<&'static str> = None;
    for start in 0..scan_end {
        if window.len() - start < 4 || &window[start..start + 4] != b"OggS" {
            continue;
        }
        saw_marker = true;
        let Some((page, _)) = parse_page_at(&window[start..], start as u64) else {
            continue;
        };
        if !page.is_bos() || page.is_continued() {
            continue;
        }
        let first = page.lacing.iter().position(|&lace| lace < 255).map(|last| {
            let len: usize = page.lacing[..=last].iter().map(|&lace| lace as usize).sum();
            &page.body[..len]
        });
        let Some(first) = first else { continue };
        if first.starts_with(b"OpusHead") {
            OpusHead::parse(first)?;
            return Ok(start as u64);
        }
        other_codec.get_or_insert(codec_of(first));
    }
    Err(match (other_codec, saw_marker) {
        (Some(codec), _) => format!(
            "This SGMCA file holds {codec} audio rather than Opus, so it is not a Direct Audio recording."
        ),
        (None, true) => "This SGMCA file has an Ogg marker but no readable Opus stream header in its first 8 KiB; the file may be damaged. Export it to WAV in Case CATalyst first.".into(),
        (None, false) => "This SGMCA file does not contain an Ogg audio stream in its supported header region; export it to WAV in Case CATalyst first".into(),
    })
}

// ── Inspection ───────────────────────────────────────────────────────────────

/// What an SGMCA (or any Ogg Opus) recording holds, from a full walk of its
/// pages. Every CRC is checked and every packet's duration is read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SgmcaInfo {
    /// Bytes before the Ogg stream (0 for a plain file).
    pub stream_offset: u64,
    pub head: OpusHead,
    pub tags: OpusTags,
    /// Family 255, one uncoupled mono stream per channel, identity mapping.
    pub sgmca_layout: bool,
    pub pages: u64,
    pub audio_packets: u64,
    /// Granule position of the first audio sample (normally 0).
    pub start_granule: u64,
    /// Granule position of the last page.
    pub final_granule: u64,
    /// Playable samples at 48 kHz: final granule minus start and pre-skip.
    pub samples: u64,
    pub duration_seconds: f64,
}

/// The two header packets and the stream's position, read once.
struct OpenStream {
    offset: u64,
    head: OpusHead,
    tags: OpusTags,
    tags_raw: Vec<u8>,
    serial: u32,
    packets: PacketReader<BufReader<fs::File>>,
    /// Audio packets already read with the tags page, if any (rare).
    pending: Option<PagePackets>,
}

fn open_stream(path: &Path) -> Result<OpenStream, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("Could not open the recording: {error}"))?;
    let offset = locate_stream(&mut file)?;
    file.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
    let mut packets = PacketReader::new(BufReader::with_capacity(1 << 16, file), offset);

    let first = packets.next()?.ok_or("The Opus stream has no identification header.")?;
    if first.packets.len() != 1 {
        return Err("The Opus identification header must be alone on the first Ogg page.".into());
    }
    let head = OpusHead::parse(&first.packets[0])?;

    let second = packets.next()?.ok_or("The Opus stream has no comment header.")?;
    let mut iter = second.packets.into_iter();
    let tags_raw = iter.next().unwrap();
    let tags = OpusTags::parse(&tags_raw)?;
    let rest: Vec<Vec<u8>> = iter.collect();
    let pending = (!rest.is_empty()).then_some(PagePackets {
        packets: rest,
        granule: second.granule,
        eos: second.eos,
    });
    let serial = packets.serial().unwrap_or(0);
    Ok(OpenStream {
        offset,
        head,
        tags,
        tags_raw,
        serial,
        packets,
        pending,
    })
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
        let mut timed = Vec::with_capacity(data.len());
        let mut cursor = start;
        for (packet, duration) in data.into_iter().zip(durations) {
            timed.push(TimedPacket {
                data: packet,
                start: cursor,
                end: cursor + duration,
            });
            cursor += duration;
        }
        self.next_start = Some(cursor);
        Ok(timed)
    }
}

/// Walk the whole recording, checking every page and packet.
pub(crate) fn inspect(path: &Path, cancelled: CancelCheck<'_>) -> Result<SgmcaInfo, String> {
    let mut stream = open_stream(path)?;
    let mut timeline = Timeline { next_start: None };
    let mut buffered = Vec::new();
    let mut audio_packets = 0u64;
    let mut start_granule: Option<u64> = None;
    let mut final_granule = 0u64;
    let mut checked = 0u64;
    let mut next = stream.pending.take();
    loop {
        let page = match next.take() {
            Some(page) => page,
            None => match stream.packets.next()? {
                Some(page) => page,
                None => break,
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
    if !buffered.is_empty() {
        return Err("The recording's audio pages carry no timestamps.".into());
    }
    let start_granule = start_granule.unwrap_or(0);
    let samples = final_granule
        .saturating_sub(start_granule)
        .saturating_sub(stream.head.pre_skip as u64);
    Ok(SgmcaInfo {
        stream_offset: stream.offset,
        sgmca_layout: stream.head.is_sgmca_layout(),
        head: stream.head,
        tags: stream.tags,
        pages: stream.packets.pages_read,
        audio_packets,
        start_granule,
        final_granule,
        samples,
        duration_seconds: samples as f64 / OPUS_RATE as f64,
    })
}

// ── Slicing ──────────────────────────────────────────────────────────────────

/// What a slice wrote.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SliceReport {
    pub channels: u8,
    /// Requested start and the end actually reached, in seconds.
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Playable samples at 48 kHz in the new file.
    pub samples: u64,
    pub pre_skip: u16,
    pub audio_packets: u64,
    pub pages: u64,
}

fn seconds_to_samples(seconds: f64) -> u64 {
    (seconds * OPUS_RATE as f64).round() as u64
}

/// Copy the part of `src` from `start_seconds` to `end_seconds` (or to the
/// end) into a new Ogg Opus file at `dst`, without re-encoding.
///
/// Whole Opus packets are copied, starting 80 ms before the cut so the
/// decoder converges, and the new file's pre-skip discards exactly the
/// samples before the requested start. The last page's granule position
/// trims the end to the requested sample (RFC 7845 section 4.5). Granule
/// positions are rebased to start at zero and every page is re-framed with
/// fresh sequence numbers and CRCs. `dst` must not exist yet.
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
    let out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|error| format!("Could not create {}: {error}", dst.display()))?;
    let result = write_slice(src, out, start_seconds, end_seconds, cancelled);
    if result.is_err() {
        let _ = fs::remove_file(dst);
    }
    result
}

fn write_slice(
    src: &Path,
    out: fs::File,
    start_seconds: f64,
    end_seconds: Option<f64>,
    cancelled: CancelCheck<'_>,
) -> Result<SliceReport, String> {
    let mut stream = open_stream(src)?;
    let pre_skip = stream.head.pre_skip as u64;
    let mut writer = OggWriter::new(BufWriter::with_capacity(1 << 16, out), stream.serial);

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
                let base = packet.start + pre_skip;
                (
                    base + seconds_to_samples(start_seconds),
                    end_seconds.map(|end| base + seconds_to_samples(end)),
                )
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
            if packet.end >= limit {
                end_reached = Some(limit);
                held = Some(packet);
                break 'pages;
            }
            held = Some(packet);
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
    let pages = writer.pages_written;
    let mut out = writer.into_inner();
    out.flush()
        .map_err(|error| format!("Could not write the recording: {error}"))?;
    out.into_inner()
        .map_err(|error| format!("Could not write the recording: {error}"))?
        .sync_all()
        .map_err(|error| format!("Could not write the recording: {error}"))?;

    let samples = (final_granule - base).saturating_sub(new_pre_skip as u64);
    Ok(SliceReport {
        channels: stream.head.channels,
        start_seconds,
        end_seconds: start_seconds + samples as f64 / OPUS_RATE as f64,
        samples,
        pre_skip: new_pre_skip,
        audio_packets: copied,
        pages,
    })
}

// ── Writer check ─────────────────────────────────────────────────────────────

/// Confirm that a file DepoAudio wrote as SGMCA really has the Direct Audio
/// layout: a sound Ogg Opus stream with mapping family 255, one uncoupled
/// mono stream per channel, identity mapping, and no prefix.
pub(crate) fn verify_written(path: &Path, cancelled: CancelCheck<'_>) -> Result<SgmcaInfo, String> {
    let info = inspect(path, cancelled)?;
    if info.stream_offset != 0 {
        return Err("The SGMCA output does not start with its Ogg stream.".into());
    }
    if !info.sgmca_layout {
        return Err(format!(
            "The SGMCA output uses Opus mapping family {} with {} streams ({} coupled) for {} channels, not one mono stream per channel.",
            info.head.mapping_family, info.head.stream_count, info.head.coupled_count, info.head.channels
        ));
    }
    if info.samples == 0 {
        return Err("The SGMCA output holds no audio.".into());
    }
    Ok(info)
}

/// Synthetic Ogg Opus streams for tests, built with this module's writer.
/// The audio packets are well-framed and correctly timed but never decoded.
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

    /// A synthetic stream: headers plus `count` 20 ms packets, end-trimmed by
    /// `trim` samples, written with the module's own writer.
    pub(crate) fn synthetic(count: usize, pre_skip: u16, trim: u64, sizes: impl Fn(usize) -> usize) -> Vec<u8> {
        let mut writer = OggWriter::new(Vec::new(), 0x5eed);
        writer.write_packet(&sgmca_head(4, pre_skip).to_bytes(), 0).unwrap();
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

    #[test]
    fn crc_matches_the_ogg_reference_values() {
        // Known values for the Ogg polynomial, MSB first, init 0, no xor.
        assert_eq!(ogg_crc(b""), 0);
        assert_eq!(ogg_crc(b"123456789"), 0x89A1_897F);
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
    fn inspect_reports_exact_duration_from_final_granule_minus_pre_skip() {
        let dir = tempdir::Dir::new();
        let path = write_temp(&dir, "a.sgmca", &synthetic(250, 3840, 480, |_| 60));
        let info = inspect(&path, None).unwrap();
        assert_eq!(info.stream_offset, 0);
        assert!(info.sgmca_layout);
        assert_eq!(info.head.channels, 4);
        assert_eq!(info.audio_packets, 250);
        assert_eq!(info.final_granule, 250 * 960 - 480);
        assert_eq!(info.samples, 250 * 960 - 480 - 3840);
        assert!((info.duration_seconds - 4.91).abs() < 1e-9);
        assert_eq!(info.tags.comments, ["TITLE=x"]);
    }

    #[test]
    fn locate_skips_a_prefix_and_stray_markers() {
        let dir = tempdir::Dir::new();
        let mut bytes = b"vendor prefix OggS not a page ".to_vec();
        bytes.resize(700, 0x5a);
        let stream = synthetic(10, 312, 0, |_| 30);
        bytes.extend_from_slice(&stream);
        let path = write_temp(&dir, "p.sgmca", &bytes);
        assert_eq!(locate_stream(&mut fs::File::open(&path).unwrap()).unwrap(), 700);
        assert_eq!(inspect(&path, None).unwrap().stream_offset, 700);
    }

    #[test]
    fn locate_explains_what_is_wrong() {
        let dir = tempdir::Dir::new();
        let none = write_temp(&dir, "none.sgmca", &[0x5a; 100]);
        assert!(locate_stream(&mut fs::File::open(&none).unwrap())
            .unwrap_err()
            .contains("does not contain an Ogg audio stream"));

        let mut vorbis = OggWriter::new(Vec::new(), 1);
        vorbis.write_packet(b"\x01vorbis....", 0).unwrap();
        vorbis.flush(true).unwrap();
        let vorbis = write_temp(&dir, "vorbis.sgmca", &vorbis.into_inner());
        assert!(locate_stream(&mut fs::File::open(&vorbis).unwrap())
            .unwrap_err()
            .contains("Vorbis"));

        let mut damaged = synthetic(5, 312, 0, |_| 30);
        damaged[30] ^= 0xff; // breaks the first page's CRC
        let damaged = write_temp(&dir, "damaged.sgmca", &damaged);
        assert!(locate_stream(&mut fs::File::open(&damaged).unwrap())
            .unwrap_err()
            .contains("no readable Opus stream header"));
    }

    #[test]
    fn inspect_names_the_damaged_page() {
        let dir = tempdir::Dir::new();
        let bytes = synthetic(200, 312, 0, |_| 60);
        let mut corrupt = bytes.clone();
        let at = corrupt.len() - 20;
        corrupt[at] ^= 0x01;
        let path = write_temp(&dir, "crc.sgmca", &corrupt);
        assert!(inspect(&path, None).unwrap_err().contains("fails its checksum"));

        let truncated = write_temp(&dir, "cut.sgmca", &bytes[..bytes.len() - 10]);
        assert!(inspect(&truncated, None).unwrap_err().contains("truncated"));
    }

    #[test]
    fn slice_copies_packets_with_pre_roll_pre_skip_and_end_trim() {
        let dir = tempdir::Dir::new();
        let src = write_temp(&dir, "src.sgmca", &synthetic(500, 312, 0, |index| 20 + index % 11));
        let dst = dir.0.join("cut.sgmca");
        // 2.0 s to 3.5 s: 72 000 samples.
        let report = slice(&src, &dst, 2.0, Some(3.5), None).unwrap();
        assert_eq!(report.samples, 72_000);
        assert!((report.end_seconds - 3.5).abs() < 1e-9);
        assert!(report.pre_skip as u64 >= PREROLL_SAMPLES);

        let info = inspect(&dst, None).unwrap();
        assert!(info.sgmca_layout);
        assert_eq!(info.start_granule, 0);
        assert_eq!(info.head.pre_skip, report.pre_skip);
        assert_eq!(info.samples, 72_000);
        assert_eq!(info.audio_packets, report.audio_packets);

        // The copied packets are the source's own bytes, unchanged: packet
        // contents are tagged with their index in the fill byte.
        let mut packets = PacketReader::new(fs::File::open(&dst).unwrap(), 0);
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
    fn slice_from_zero_keeps_the_original_pre_skip_and_runs_to_the_end() {
        let dir = tempdir::Dir::new();
        let src = write_temp(&dir, "src.sgmca", &synthetic(100, 3840, 300, |_| 25));
        let dst = dir.0.join("all.sgmca");
        let report = slice(&src, &dst, 0.0, None, None).unwrap();
        let original = inspect(&src, None).unwrap();
        assert_eq!(report.pre_skip, 3840);
        assert_eq!(report.samples, original.samples);
        assert_eq!(inspect(&dst, None).unwrap().samples, original.samples);
    }

    #[test]
    fn slice_rejects_bad_ranges_and_never_overwrites() {
        let dir = tempdir::Dir::new();
        let src = write_temp(&dir, "src.sgmca", &synthetic(50, 312, 0, |_| 25));
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
    fn verify_written_rejects_a_coupled_layout() {
        let dir = tempdir::Dir::new();
        let mut writer = OggWriter::new(Vec::new(), 7);
        let mut head = sgmca_head(2, 312);
        head.mapping_family = 0;
        head.stream_count = 1;
        head.coupled_count = 1;
        writer.write_packet(&head.to_bytes(), 0).unwrap();
        writer.flush(false).unwrap();
        writer.write_packet(&tags_packet("t", &[]), 0).unwrap();
        writer.flush(false).unwrap();
        writer.write_packet(&fake_packet(20, 0), 960 * 10).unwrap();
        writer.flush(true).unwrap();
        let path = write_temp(&dir, "stereo.sgmca", &writer.into_inner());
        assert!(verify_written(&path, None).unwrap_err().contains("mapping family 0"));
    }
}
