//! SoundSync bookmark files (`.bmf`) beside a `.dm`.
//!
//! From the owner-supplied notes (`docs/formats/dm.md`): a `.bmf` is a
//! Windows compound file (structured storage) holding one stream named
//! `FileInfo`: a little-endian i32 entry count, then for each entry an i32
//! position in milliseconds on SoundSync's timeline, a u32 text length and
//! that many bytes of text with no terminator. SoundSync names it after the
//! `.dm` with the extension swapped (`Smith.dm` gives `smith.bmf`, lower
//! case, on Windows where case does not matter).
//!
//! DepoAudio reads the entries only to count them for the export record.
//! Marker text can hold notes about the case, so it is stepped over and never
//! kept, logged or written anywhere; no option to record it exists yet. The
//! `.bmf` is only read, and a missing or unreadable one never stops a
//! conversion: SoundSync itself treats a missing one as "no bookmarks".

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// A marker file bigger than this is not read (the notes' own example is
/// 20 KiB; the compound-file container is mostly padding).
const MAX_BMF_BYTES: u64 = 16 * 1024 * 1024;
/// The `FileInfo` stream is at most this long.
const MAX_STREAM_BYTES: u64 = 8 * 1024 * 1024;

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const FREE_SECTOR: u32 = 0xFFFF_FFFF;
const STREAM_NAME: &str = "FileInfo";

/// What the export record says about a `.bmf` beside a `.dm`. Counts only.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarkerFileSummary {
    /// `read`, `unreadable`, or `ambiguous` (more than one file in the folder
    /// has the `.bmf` name when case is ignored, and none is the all
    /// lower-case name SoundSync writes, so none was read).
    pub status: String,
    /// The marker file's name, without its folder. Null when ambiguous.
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    /// Entries in the `FileInfo` stream.
    pub entries: Option<u64>,
    /// Entries with any text (their text is not recorded).
    pub entries_with_text: Option<u64>,
    /// Entries SoundSync would keep when it opens the file: positions from 0
    /// to its timeline length in milliseconds (the sound bytes its loader
    /// counts, divided by the byte rate, in whole seconds). It drops the rest.
    pub entries_in_soundsync_range: Option<u64>,
    /// Kept entries whose clock time (the first block's stamp plus the
    /// position in whole seconds, which is how SoundSync turns a bookmark into
    /// a seek) is the stamp of a block DepoAudio read. Others point into a
    /// pause or at sound that is not in the file.
    pub entries_matching_a_block: Option<u64>,
    /// Why the file could not be read, in DepoAudio's words; never any of the
    /// file's content.
    pub problem: Option<String>,
}

/// The entries of a `FileInfo` stream: each position, and whether it had text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Markers {
    pub positions_ms: Vec<i32>,
    pub with_text: u64,
}

enum Lookup {
    None,
    One(PathBuf),
    Ambiguous,
}

/// The `.bmf` SoundSync would use for `dm_path`: the same name with `.bmf`
/// for the extension, matched without regard to case.
fn find_beside(dm_path: &Path) -> Lookup {
    let (Some(stem), Some(folder)) = (dm_path.file_stem().and_then(|stem| stem.to_str()), dm_path.parent()) else {
        return Lookup::None;
    };
    let wanted = format!("{stem}.bmf");
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let Ok(entries) = fs::read_dir(folder) else {
        return Lookup::None;
    };
    let mut matches: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(&wanted))
        })
        .filter(|entry| fs::metadata(entry.path()).is_ok_and(|meta| meta.is_file()))
        .map(|entry| entry.path())
        .collect();
    match matches.len() {
        0 => Lookup::None,
        1 => Lookup::One(matches.remove(0)),
        _ => {
            let lower = wanted.to_lowercase();
            match matches
                .into_iter()
                .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(lower.as_str()))
            {
                Some(path) => Lookup::One(path),
                None => Lookup::Ambiguous,
            }
        }
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

const NOT_COMPOUND: &str = "it is not a compound file DepoAudio can read";
const NO_STREAM: &str = "it has no FileInfo stream";
const DAMAGED: &str = "its compound-file structure is damaged";

/// The bytes of the stream named `name` (case ignored) directly under the
/// root of the compound file in `bytes`. Every sector number and chain is
/// checked against the file, and chains are cut at their possible length, so
/// a damaged file gives an error, never a panic or a loop.
pub(crate) fn compound_stream(bytes: &[u8], name: &str) -> Result<Vec<u8>, &'static str> {
    if bytes.len() < 512 || bytes[..8] != CFB_MAGIC || u16_at(bytes, 0x1C) != Some(0xFFFE) {
        return Err(NOT_COMPOUND);
    }
    let sector_shift = u16_at(bytes, 0x1E).ok_or(NOT_COMPOUND)?;
    let mini_shift = u16_at(bytes, 0x20).ok_or(NOT_COMPOUND)?;
    if !matches!(sector_shift, 9 | 12) || mini_shift != 6 {
        return Err(NOT_COMPOUND);
    }
    let sector_size = 1usize << sector_shift;
    let header = |at| u32_at(bytes, at).ok_or(NOT_COMPOUND);
    let fat_sectors = header(0x2C)? as usize;
    let first_directory = header(0x30)?;
    let mini_cutoff = u64::from(header(0x38)?);
    let first_mini_fat = header(0x3C)?;
    let first_difat = header(0x44)?;
    let sector_count = (bytes.len() / sector_size).saturating_sub(1);
    let sector = |id: u32| -> Result<&[u8], &'static str> {
        let id = id as usize;
        if id >= sector_count {
            return Err(DAMAGED);
        }
        Ok(&bytes[(id + 1) * sector_size..(id + 2) * sector_size])
    };
    if fat_sectors > sector_count {
        return Err(DAMAGED);
    }

    // The FAT's own sectors: 109 in the header, then the DIFAT chain.
    let mut fat_ids: Vec<u32> = (0..109)
        .filter_map(|index| u32_at(bytes, 0x4C + index * 4))
        .take(fat_sectors)
        .collect();
    let mut difat = first_difat;
    let mut hops = 0usize;
    while fat_ids.len() < fat_sectors && difat != END_OF_CHAIN && difat != FREE_SECTOR {
        hops += 1;
        if hops > sector_count {
            return Err(DAMAGED);
        }
        let page = sector(difat)?;
        let per_page = sector_size / 4 - 1;
        fat_ids.extend((0..per_page).filter_map(|index| u32_at(page, index * 4)));
        difat = u32_at(page, per_page * 4).ok_or(DAMAGED)?;
    }
    fat_ids.truncate(fat_sectors);
    if fat_ids.len() < fat_sectors {
        return Err(DAMAGED);
    }
    let mut fat: Vec<u32> = Vec::with_capacity(fat_sectors * sector_size / 4);
    for id in fat_ids {
        let page = sector(id)?;
        fat.extend((0..sector_size / 4).filter_map(|index| u32_at(page, index * 4)));
    }

    // Follow a chain in `table`, at most `table.len()` links.
    let chain = |table: &[u32], start: u32| -> Result<Vec<u32>, &'static str> {
        let mut links = Vec::new();
        let mut at = start;
        while at != END_OF_CHAIN {
            if links.len() >= table.len() {
                return Err(DAMAGED);
            }
            links.push(at);
            at = *table.get(at as usize).ok_or(DAMAGED)?;
        }
        Ok(links)
    };
    let read_chain = |start: u32, limit: u64| -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        for id in chain(&fat, start)? {
            out.extend_from_slice(sector(id)?);
            if out.len() as u64 >= limit {
                break;
            }
        }
        Ok(out)
    };

    let directory = read_chain(first_directory, MAX_BMF_BYTES)?;
    let entries = directory.len() / 128;
    let entry = |index: u32| -> Result<&[u8], &'static str> {
        let index = index as usize;
        if index >= entries {
            return Err(DAMAGED);
        }
        Ok(&directory[index * 128..(index + 1) * 128])
    };
    let root = entry(0)?;
    if root[0x42] != 5 {
        return Err(DAMAGED);
    }
    // 512-byte sectors (version 3) keep only the low 32 bits of a size.
    let stream_size = |raw: &[u8]| -> Result<u64, &'static str> {
        let size = u64_at(raw, 0x78).ok_or(DAMAGED)?;
        Ok(if sector_shift == 9 { size & 0xFFFF_FFFF } else { size })
    };

    // Walk the root's children (a tree linked by left, right and child ids),
    // visiting each entry at most once.
    let mut found = None;
    let mut stack = vec![u32_at(root, 0x4C).ok_or(DAMAGED)?];
    let mut seen = vec![false; entries];
    while let Some(id) = stack.pop() {
        if id == FREE_SECTOR {
            continue;
        }
        let raw = entry(id)?;
        if std::mem::replace(&mut seen[id as usize], true) {
            return Err(DAMAGED);
        }
        let name_len = usize::from(u16_at(raw, 0x40).ok_or(DAMAGED)?).min(64);
        let units: Vec<u16> = (0..name_len.saturating_sub(2) / 2)
            .filter_map(|unit| u16_at(raw, unit * 2))
            .collect();
        let entry_name = String::from_utf16_lossy(&units);
        if raw[0x42] == 2 && entry_name.eq_ignore_ascii_case(name) {
            found = Some(raw);
            break;
        }
        stack.push(u32_at(raw, 0x44).ok_or(DAMAGED)?);
        stack.push(u32_at(raw, 0x48).ok_or(DAMAGED)?);
    }
    let stream = found.ok_or(NO_STREAM)?;
    let size = stream_size(stream)?;
    if size > MAX_STREAM_BYTES {
        return Err(DAMAGED);
    }
    let start = u32_at(stream, 0x74).ok_or(DAMAGED)?;
    let mut data = if size < mini_cutoff {
        // Small streams live in 64-byte pieces of the mini stream, which is
        // the root entry's own data.
        let mini_stream = read_chain(u32_at(root, 0x74).ok_or(DAMAGED)?, stream_size(root)?)?;
        let mini_fat_bytes = read_chain(first_mini_fat, MAX_BMF_BYTES)?;
        let mini_fat: Vec<u32> = (0..mini_fat_bytes.len() / 4)
            .filter_map(|index| u32_at(&mini_fat_bytes, index * 4))
            .collect();
        let mut out = Vec::new();
        for id in chain(&mini_fat, start)? {
            let at = id as usize * 64;
            out.extend_from_slice(mini_stream.get(at..at + 64).ok_or(DAMAGED)?);
            if out.len() as u64 >= size {
                break;
            }
        }
        out
    } else {
        read_chain(start, size)?
    };
    if (data.len() as u64) < size {
        return Err(DAMAGED);
    }
    data.truncate(size as usize);
    Ok(data)
}

/// Parse a `FileInfo` stream. Text is stepped over, not kept.
pub(crate) fn parse_file_info(stream: &[u8]) -> Result<Markers, &'static str> {
    const BAD: &str = "its FileInfo entries do not fit the stream";
    let count = i32::from_le_bytes(stream.get(0..4).ok_or(BAD)?.try_into().map_err(|_| BAD)?);
    let count = usize::try_from(count).map_err(|_| BAD)?;
    // Every entry takes at least 8 bytes.
    if count > (stream.len() - 4) / 8 {
        return Err(BAD);
    }
    let mut positions_ms = Vec::with_capacity(count);
    let mut with_text = 0u64;
    let mut at = 4usize;
    for _ in 0..count {
        let position = u32_at(stream, at).ok_or(BAD)? as i32;
        let text_len = u32_at(stream, at + 4).ok_or(BAD)? as usize;
        at += 8;
        if text_len > stream.len() - at {
            return Err(BAD);
        }
        at += text_len;
        positions_ms.push(position);
        with_text += u64::from(text_len > 0);
    }
    Ok(Markers {
        positions_ms,
        with_text,
    })
}

/// Index of the first block SoundSync would seek to for a bookmark at
/// `position_ms`: the first block (in file order) whose stamp is the first
/// block's stamp plus the position in whole seconds. SoundSync uses a binary
/// search, which finds the same block while stamps rise.
pub(crate) fn block_for_marker(stamps: &[u32], position_ms: i32) -> Option<usize> {
    let first = u64::from(*stamps.first()?);
    let seconds = u64::try_from(position_ms).ok()? / 1000;
    let target = u32::try_from(first + seconds).ok()?;
    if target == 0 {
        return None;
    }
    stamps.iter().position(|&stamp| stamp == target)
}

fn unreadable(path: &Path, size: Option<u64>, sha256: Option<String>, problem: &str) -> MarkerFileSummary {
    MarkerFileSummary {
        status: "unreadable".into(),
        file_name: path.file_name().map(|name| name.to_string_lossy().into_owned()),
        size_bytes: size,
        sha256,
        entries: None,
        entries_with_text: None,
        entries_in_soundsync_range: None,
        entries_matching_a_block: None,
        problem: Some(format!("The marker file could not be read: {problem}.")),
    }
}

/// The summary of the `.bmf` beside `dm_path` (parsed as `dm`), or None when
/// there is none.
pub(crate) fn summary_beside(dm_path: &Path, dm: &crate::dm::DmFile) -> Option<MarkerFileSummary> {
    let path = match find_beside(dm_path) {
        Lookup::None => return None,
        Lookup::Ambiguous => {
            return Some(MarkerFileSummary {
                status: "ambiguous".into(),
                file_name: None,
                size_bytes: None,
                sha256: None,
                entries: None,
                entries_with_text: None,
                entries_in_soundsync_range: None,
                entries_matching_a_block: None,
                problem: Some(
                    "More than one file beside the .dm has its marker-file name in different letter case, \
                     so none was read."
                        .into(),
                ),
            })
        }
        Lookup::One(path) => path,
    };
    let size = fs::metadata(&path).map(|meta| meta.len()).ok();
    if size.is_none_or(|size| size > MAX_BMF_BYTES) {
        return Some(unreadable(
            &path,
            size,
            None,
            "it is larger than 16 MiB or its size is unknown",
        ));
    }
    let mut bytes = Vec::new();
    let read = fs::File::open(&path).and_then(|file| file.take(MAX_BMF_BYTES + 1).read_to_end(&mut bytes));
    if read.is_err() {
        return Some(unreadable(&path, size, None, "the file could not be opened or read"));
    }
    let sha256: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let size = Some(bytes.len() as u64);
    let markers = match compound_stream(&bytes, STREAM_NAME).and_then(|stream| parse_file_info(&stream)) {
        Ok(markers) => markers,
        Err(problem) => return Some(unreadable(&path, size, Some(sha256), problem)),
    };
    let range_ms = crate::dm::soundsync_loader_sound_bytes(dm_path, &dm.header)
        .map(|bytes| bytes / u64::from(dm.header.wave.avg_bytes_per_sec.max(1)) * 1000);
    let kept: Option<Vec<i32>> = range_ms.map(|range| {
        markers
            .positions_ms
            .iter()
            .copied()
            .filter(|&position| u64::try_from(position).is_ok_and(|position| position <= range))
            .collect()
    });
    Some(MarkerFileSummary {
        status: "read".into(),
        file_name: path.file_name().map(|name| name.to_string_lossy().into_owned()),
        size_bytes: size,
        sha256: Some(sha256),
        entries: Some(markers.positions_ms.len() as u64),
        entries_with_text: Some(markers.with_text),
        entries_in_soundsync_range: kept.as_ref().map(|kept| kept.len() as u64),
        entries_matching_a_block: kept.as_ref().map(|kept| {
            kept.iter()
                .filter(|&&position| block_for_marker(&dm.stamps, position).is_some())
                .count() as u64
        }),
        problem: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's synthetic samples (see their README).
    fn sample(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/fixtures/dm/vendor-notes-samples")
            .join(name)
    }

    /// A minimal version-3 compound file holding one small stream.
    fn compound_file(name: &str, stream: &[u8]) -> Vec<u8> {
        assert!(stream.len() < 4096);
        let sector = 512usize;
        let mut header = vec![0u8; sector];
        header[..8].copy_from_slice(&CFB_MAGIC);
        header[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
        header[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
        header[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
        header[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
        header[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
        header[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes()); // one FAT sector
        header[0x30..0x34].copy_from_slice(&1u32.to_le_bytes()); // directory at 1
        header[0x38..0x3C].copy_from_slice(&4096u32.to_le_bytes());
        header[0x3C..0x40].copy_from_slice(&2u32.to_le_bytes()); // mini FAT at 2
        header[0x40..0x44].copy_from_slice(&1u32.to_le_bytes());
        header[0x44..0x48].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        for index in 0..109 {
            header[0x4C + index * 4..0x50 + index * 4].copy_from_slice(&FREE_SECTOR.to_le_bytes());
        }
        header[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
        let mini_sectors = stream.len().div_ceil(64).max(1);
        let mini_stream_sectors = (mini_sectors * 64).div_ceil(sector);
        // FAT: 0 = FAT, 1 = directory, 2 = mini FAT, 3.. = mini stream.
        let mut fat = vec![0xFFu8; sector];
        let mut put = |index: usize, value: u32| fat[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        put(0, 0xFFFF_FFFD);
        put(1, END_OF_CHAIN);
        put(2, END_OF_CHAIN);
        for index in 0..mini_stream_sectors {
            let next = if index + 1 == mini_stream_sectors {
                END_OF_CHAIN
            } else {
                (4 + index) as u32
            };
            put(3 + index, next);
        }
        let mut directory = vec![0u8; sector];
        let mut dir_entry = |index: usize, entry_name: &str, kind: u8, child: u32, start: u32, size: u64| {
            let raw = &mut directory[index * 128..(index + 1) * 128];
            let units: Vec<u16> = entry_name.encode_utf16().collect();
            for (unit_index, unit) in units.iter().enumerate() {
                raw[unit_index * 2..unit_index * 2 + 2].copy_from_slice(&unit.to_le_bytes());
            }
            raw[0x40..0x42].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
            raw[0x42] = kind;
            raw[0x44..0x48].copy_from_slice(&FREE_SECTOR.to_le_bytes());
            raw[0x48..0x4C].copy_from_slice(&FREE_SECTOR.to_le_bytes());
            raw[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
            raw[0x74..0x78].copy_from_slice(&start.to_le_bytes());
            raw[0x78..0x80].copy_from_slice(&size.to_le_bytes());
        };
        dir_entry(0, "Root Entry", 5, 1, 3, (mini_sectors * 64) as u64);
        dir_entry(1, name, 2, FREE_SECTOR, 0, stream.len() as u64);
        let mut mini_fat = vec![0xFFu8; sector];
        for index in 0..mini_sectors {
            let next = if index + 1 == mini_sectors {
                END_OF_CHAIN
            } else {
                (index + 1) as u32
            };
            mini_fat[index * 4..index * 4 + 4].copy_from_slice(&next.to_le_bytes());
        }
        let mut mini = stream.to_vec();
        mini.resize(mini_stream_sectors * sector, 0);
        [header, fat, directory, mini_fat, mini].concat()
    }

    fn file_info(entries: &[(i32, &[u8])]) -> Vec<u8> {
        let mut out = (entries.len() as i32).to_le_bytes().to_vec();
        for (position, text) in entries {
            out.extend_from_slice(&position.to_le_bytes());
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            out.extend_from_slice(text);
        }
        out
    }

    #[test]
    fn reads_the_owner_sample_file_info_stream() {
        let bytes = fs::read(sample("tone_gsm.bmf")).unwrap();
        let stream = compound_stream(&bytes, STREAM_NAME).unwrap();
        assert_eq!(stream.len(), 34);
        let markers = parse_file_info(&stream).unwrap();
        assert_eq!(markers.positions_ms, vec![0, 1000]);
        assert_eq!(markers.with_text, 2);
        // The notes' worked example: 0 ms is block 0, 1000 ms the first block
        // stamped one second later, block 5.
        let dm = crate::dm::parse_dm(&sample("tone_gsm.dm"), &|| false).unwrap();
        assert_eq!(block_for_marker(&dm.stamps, 0), Some(0));
        assert_eq!(block_for_marker(&dm.stamps, 1000), Some(5));
        assert_eq!(block_for_marker(&dm.stamps, 1999), Some(5));
        assert_eq!(block_for_marker(&dm.stamps, 2000), None);
        assert_eq!(block_for_marker(&dm.stamps, -1), None);
    }

    #[test]
    fn the_summary_beside_the_owner_sample_has_counts_and_no_text() {
        let dm_path = sample("tone_gsm.dm");
        let dm = crate::dm::parse_dm(&dm_path, &|| false).unwrap();
        let summary = summary_beside(&dm_path, &dm).unwrap();
        assert_eq!(summary.status, "read");
        assert_eq!(summary.file_name.as_deref(), Some("tone_gsm.bmf"));
        assert_eq!(summary.size_bytes, Some(20480));
        assert_eq!(
            summary.sha256.as_deref(),
            Some("829b55dc6234ee21d6cd4be40fa99c4b9cd540d775abdc198902203747fb6b8b")
        );
        assert_eq!(summary.entries, Some(2));
        assert_eq!(summary.entries_with_text, Some(2));
        // 9 × 520 bytes at 2239 bytes a second is 2 whole seconds: 0…2000 ms.
        assert_eq!(summary.entries_in_soundsync_range, Some(2));
        assert_eq!(summary.entries_matching_a_block, Some(2));
        assert_eq!(summary.problem, None);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("Start") && !json.contains("Objection"), "{json}");
    }

    #[test]
    fn markers_out_of_range_or_in_a_pause_are_counted_apart() {
        let dir = std::env::temp_dir().join(format!("depoaudio_bmf_test_{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&dir).unwrap();
        let t0 = 1_763_800_000u32;
        let block = vec![1u8; 520];
        // 12 blocks (6240 bytes at 2239 B/s: 2 whole seconds of timeline);
        // stamps jump by 60 s after block 4.
        let stamps: Vec<u32> = (0..12).map(|index| if index < 4 { t0 } else { t0 + 60 }).collect();
        let layout: Vec<(u32, &[u8])> = stamps.iter().map(|&stamp| (stamp, &block[..])).collect();
        let dm_path = dir.join("Session.DM");
        fs::write(
            &dm_path,
            crate::dm::tests::dm_bytes(&crate::dm::tests::gsm_format(), 520, &layout),
        )
        .unwrap();
        let stream = file_info(&[(0, b""), (1500, b"private words"), (2000, b""), (2001, b"x"), (-5, b"")]);
        fs::write(dir.join("session.bmf"), compound_file("FileInfo", &stream)).unwrap();
        let dm = crate::dm::parse_dm(&dm_path, &|| false).unwrap();
        let summary = summary_beside(&dm_path, &dm).unwrap();
        assert_eq!(summary.status, "read");
        assert_eq!(summary.file_name.as_deref(), Some("session.bmf"));
        assert_eq!(summary.entries, Some(5));
        assert_eq!(summary.entries_with_text, Some(2));
        assert_eq!(summary.entries_in_soundsync_range, Some(3));
        // 0 ms finds block 0; 1500 and 2000 ms ask for t0 + 1 and t0 + 2,
        // inside the pause.
        assert_eq!(summary.entries_matching_a_block, Some(1));
        assert!(!serde_json::to_string(&summary).unwrap().contains("private"));

        // Two names that differ only in case, neither all lower case: none is read.
        fs::remove_file(dir.join("session.bmf")).unwrap();
        fs::write(dir.join("Session.bmf"), compound_file("FileInfo", &stream)).unwrap();
        fs::write(dir.join("SESSION.BMF"), compound_file("FileInfo", &stream)).unwrap();
        let ambiguous = summary_beside(&dm_path, &dm).unwrap();
        if fs::read_dir(&dir).unwrap().count() == 3 {
            assert_eq!(ambiguous.status, "ambiguous");
            assert_eq!(ambiguous.entries, None);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_or_damaged_marker_file_never_stops_anything() {
        let dir = std::env::temp_dir().join(format!("depoaudio_bmf_test_{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&dir).unwrap();
        let block = vec![1u8; 520];
        let dm_path = dir.join("a.dm");
        fs::write(
            &dm_path,
            crate::dm::tests::dm_bytes(&crate::dm::tests::gsm_format(), 520, &[(1_763_800_000, &block)]),
        )
        .unwrap();
        let dm = crate::dm::parse_dm(&dm_path, &|| false).unwrap();
        assert_eq!(summary_beside(&dm_path, &dm), None);

        fs::write(dir.join("a.bmf"), b"not a compound file").unwrap();
        let summary = summary_beside(&dm_path, &dm).unwrap();
        assert_eq!(summary.status, "unreadable");
        assert!(summary.problem.unwrap().contains("not a compound file"));
        assert_eq!(summary.size_bytes, Some(19));

        fs::write(dir.join("a.bmf"), compound_file("Other", &file_info(&[]))).unwrap();
        let summary = summary_beside(&dm_path, &dm).unwrap();
        assert!(summary.problem.unwrap().contains("no FileInfo"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_file_info_streams_are_refused() {
        assert!(parse_file_info(&[]).is_err());
        assert!(parse_file_info(&(-1i32).to_le_bytes()).is_err());
        assert!(parse_file_info(&1i32.to_le_bytes()).is_err());
        let mut long_text = file_info(&[(0, b"abc")]);
        long_text[8] = 4;
        assert!(parse_file_info(&long_text).is_err());
        assert_eq!(
            parse_file_info(&file_info(&[])).unwrap().positions_ms,
            Vec::<i32>::new()
        );

        let good = compound_file("FileInfo", &file_info(&[(7, b"t")]));
        assert_eq!(compound_stream(&good, "fileinfo").unwrap(), file_info(&[(7, b"t")]));
        // Deterministic damage anywhere never panics.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for round in 0..2000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let mut damaged = good.clone();
            let at = (state as usize) % damaged.len();
            damaged[at] = (state >> 32) as u8;
            if round % 3 == 0 {
                damaged.truncate(((state >> 8) as usize) % good.len());
            }
            let _ = compound_stream(&damaged, STREAM_NAME).map(|stream| parse_file_info(&stream));
        }
    }
}
