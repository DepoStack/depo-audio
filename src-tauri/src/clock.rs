//! Timestamps DepoAudio writes, built on the `time` crate.
//!
//! These used to come from `chrono`. Each formatter here reproduces the exact
//! text chrono wrote, so export records, backups and libraries written by
//! older builds and by this one look the same, and nothing that reads them
//! back (the app, a court, a script) sees a change. Everything is UTC,
//! except the start time written into an SGMCA segment header, which the
//! Direct Audio layout stores as local wall-clock time. That one local
//! offset comes from the operating system (`local_offset_at`), not from
//! `time`'s `local-offset` feature, which refuses to answer on Linux and
//! macOS once the app has more than one thread.

use std::time::SystemTime;
use time::OffsetDateTime;

/// The current time in UTC.
pub(crate) fn now_utc() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// A file's modification time (or any `SystemTime`) in UTC, or `None` when
/// it falls outside the years -9999 to 9999 that `time` can hold.
///
/// File metadata is not trusted: a corrupt or far-future mtime (from a
/// removable drive, a network share, or `touch -d '+10000 years'`) must not
/// panic, because `OffsetDateTime::from(SystemTime)` does exactly that out of
/// range and one such file would fail a whole folder scan.
pub(crate) fn from_system_time(time: SystemTime) -> Option<OffsetDateTime> {
    let nanos = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).ok()?,
        Err(before) => i128::try_from(before.duration().as_nanos()).ok()?.checked_neg()?,
    };
    OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()
}

/// The year as chrono's `%Y` wrote it: four zero-padded digits for 0000 to
/// 9999, otherwise an explicit sign and at least four digits (`-0003`).
fn year(year: i32) -> String {
    if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        let sign = if year < 0 { '-' } else { '+' };
        format!("{sign}{:04}", year.unsigned_abs())
    }
}

/// `YYYY-MM-DD` in UTC, as chrono's `format("%Y-%m-%d")` wrote it.
pub(crate) fn date_ymd(time: &OffsetDateTime) -> String {
    let time = time.to_offset(time::UtcOffset::UTC);
    format!("{}-{:02}-{:02}", year(time.year()), u8::from(time.month()), time.day())
}

/// `YYYY-MM-DDTHH:MM:SSZ`: whole seconds, `Z` for UTC. Any fraction of a
/// second is dropped, not rounded. This is what chrono's
/// `to_rfc3339_opts(SecondsFormat::Secs, true)` wrote for the export
/// record's `createdAt` and the backup's `exportedAt`.
pub(crate) fn rfc3339_seconds_z(time: &OffsetDateTime) -> String {
    let time = time.to_offset(time::UtcOffset::UTC);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date_ymd(&time),
        time.hour(),
        time.minute(),
        time.second()
    )
}

/// RFC 3339 in UTC with a `+00:00` offset and a fraction of 0, 3, 6 or 9
/// digits (the fewest that show the value exactly). This is what chrono's
/// `to_rfc3339()` wrote for a library case's `createdAt`, for example
/// `2026-11-20T18:00:00.123+00:00`.
pub(crate) fn rfc3339_library(time: &OffsetDateTime) -> String {
    let time = time.to_offset(time::UtcOffset::UTC);
    let nanos = time.nanosecond();
    let fraction = if nanos == 0 {
        String::new()
    } else if nanos.is_multiple_of(1_000_000) {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos.is_multiple_of(1_000) {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    };
    format!(
        "{}T{:02}:{:02}:{:02}{fraction}+00:00",
        date_ymd(&time),
        time.hour(),
        time.minute(),
        time.second()
    )
}

/// This computer's UTC offset at the instant `utc`, from the operating
/// system's time-zone rules, so the daylight-saving rule for that date is
/// applied. None when the system cannot say.
pub(crate) fn local_offset_at(utc: OffsetDateTime) -> Option<time::UtcOffset> {
    let seconds = sys::offset_seconds(utc.unix_timestamp())?;
    time::UtcOffset::from_whole_seconds(seconds).ok()
}

/// The instant `utc` as an SGMCA segment start: local wall-clock time and
/// this computer's offset, `YYYY-MM-DDTHH:MM:SS±hhmm`. Seconds only. When the
/// offset cannot be read the text is in UTC with `+0000`, which is still the
/// same instant.
pub(crate) fn local_start_text(utc: OffsetDateTime) -> Option<String> {
    let offset = local_offset_at(utc).unwrap_or(time::UtcOffset::UTC);
    let local = utc.to_offset(offset);
    let minutes = offset.whole_minutes() as i32;
    crate::sgmca::format_start_text(time::PrimitiveDateTime::new(local.date(), local.time()), minutes)
}

/// `local_start_text` for the current time.
pub(crate) fn local_start_text_now() -> Option<String> {
    local_start_text(now_utc())
}

#[cfg(unix)]
mod sys {
    pub(super) fn offset_seconds(unix: i64) -> Option<i32> {
        // time_t is 64-bit here but 32-bit on some targets.
        #[allow(clippy::useless_conversion)]
        let time: libc::time_t = unix.try_into().ok()?;
        // SAFETY: `tm` is plain data (its one pointer may be null), and
        // localtime_r writes only into it. localtime_r reads the TZ
        // environment variable, which is unsound only if another thread
        // changes the environment at the same time; DepoAudio never does.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::localtime_r(&time, &mut tm) };
        if result.is_null() {
            return None;
        }
        i32::try_from(tm.tm_gmtoff).ok()
    }
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    /// Seconds between 1601-01-01 (the FILETIME epoch) and 1970-01-01.
    const FILETIME_UNIX_OFFSET: i64 = 11_644_473_600;

    pub(super) fn offset_seconds(unix: i64) -> Option<i32> {
        let ticks = u64::try_from(unix.checked_add(FILETIME_UNIX_OFFSET)?.checked_mul(10_000_000)?).ok()?;
        let utc_file = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let mut local_file = FILETIME::default();
        // SAFETY: every pointer is to a local value of the type the function
        // expects; a null time-zone pointer means the current time zone.
        let converted = unsafe {
            FileTimeToSystemTime(&utc_file, &mut utc) != 0
                && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
                && SystemTimeToFileTime(&local, &mut local_file) != 0
        };
        if !converted {
            return None;
        }
        let local_ticks = (u64::from(local_file.dwHighDateTime) << 32) | u64::from(local_file.dwLowDateTime);
        let difference = (i128::from(local_ticks) - i128::from(ticks)) / 10_000_000;
        i32::try_from(difference).ok()
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub(super) fn offset_seconds(_unix: i64) -> Option<i32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;
    use time::macros::datetime;

    #[test]
    fn local_start_text_is_the_same_instant_in_local_time() {
        let utc = datetime!(2026-11-22 15:15:30 UTC);
        let text = local_start_text(utc).unwrap();
        let (local, offset_minutes) = crate::sgmca::parse_start_text(&text).unwrap();
        let offset = time::UtcOffset::from_whole_seconds(offset_minutes * 60).unwrap();
        assert_eq!(local.assume_offset(offset), utc);
        assert_eq!(Some(offset), local_offset_at(utc).or(Some(time::UtcOffset::UTC)));
    }

    #[test]
    fn date_is_utc_year_month_day() {
        assert_eq!(date_ymd(&datetime!(2026-11-20 18:00:00 UTC)), "2026-11-20");
        assert_eq!(date_ymd(&datetime!(2026-01-02 03:04:05 UTC)), "2026-01-02");
        // A non-UTC value is converted before the date is taken.
        assert_eq!(date_ymd(&datetime!(2026-11-20 23:30:00 -05:00)), "2026-11-21");
    }

    #[test]
    fn seconds_z_matches_chrono_secs_format_and_drops_the_fraction() {
        assert_eq!(
            rfc3339_seconds_z(&datetime!(2026-11-20 18:00:00 UTC)),
            "2026-11-20T18:00:00Z"
        );
        assert_eq!(
            rfc3339_seconds_z(&datetime!(2026-11-20 18:00:00.999_999_999 UTC)),
            "2026-11-20T18:00:00Z"
        );
        assert_eq!(
            rfc3339_seconds_z(&datetime!(2026-01-02 03:04:05.5 UTC)),
            "2026-01-02T03:04:05Z"
        );
        assert_eq!(
            rfc3339_seconds_z(&datetime!(2026-11-20 13:00:00 -05:00)),
            "2026-11-20T18:00:00Z"
        );
    }

    #[test]
    fn library_format_matches_chrono_to_rfc3339() {
        // Expected strings are what chrono 0.4 `Utc.to_rfc3339()` printed.
        assert_eq!(
            rfc3339_library(&datetime!(2026-11-20 18:00:00 UTC)),
            "2026-11-20T18:00:00+00:00"
        );
        assert_eq!(
            rfc3339_library(&datetime!(2026-11-20 18:00:00.5 UTC)),
            "2026-11-20T18:00:00.500+00:00"
        );
        assert_eq!(
            rfc3339_library(&datetime!(2026-11-20 18:00:00.123_456 UTC)),
            "2026-11-20T18:00:00.123456+00:00"
        );
        assert_eq!(
            rfc3339_library(&datetime!(2026-11-20 18:00:00.123_456_789 UTC)),
            "2026-11-20T18:00:00.123456789+00:00"
        );
        assert_eq!(
            rfc3339_library(&datetime!(2026-11-20 18:00:00.000_000_001 UTC)),
            "2026-11-20T18:00:00.000000001+00:00"
        );
    }

    #[test]
    fn written_strings_parse_back_to_the_same_instant() {
        let precise = datetime!(2026-11-20 18:00:00.123_456_789 UTC);
        let library = OffsetDateTime::parse(&rfc3339_library(&precise), &Rfc3339).unwrap();
        assert_eq!(library, precise);
        let seconds = OffsetDateTime::parse(&rfc3339_seconds_z(&precise), &Rfc3339).unwrap();
        assert_eq!(seconds, precise.replace_nanosecond(0).unwrap());
    }

    #[test]
    fn strings_written_by_chrono_builds_still_parse_and_reformat_identically() {
        for written in ["2026-11-13T10:20:30Z", "2025-01-01T00:00:00Z", "2026-11-13T09:00:00Z"] {
            let parsed = OffsetDateTime::parse(written, &Rfc3339).unwrap();
            assert_eq!(rfc3339_seconds_z(&parsed), written);
        }
        for written in [
            "2026-03-04T05:06:07.891+00:00",
            "2026-03-04T05:06:07.891234+00:00",
            "2026-03-04T05:06:07.891234567+00:00",
            "2026-03-04T05:06:07+00:00",
        ] {
            let parsed = OffsetDateTime::parse(written, &Rfc3339).unwrap();
            assert_eq!(rfc3339_library(&parsed), written);
        }
    }

    #[test]
    fn system_time_converts_to_utc() {
        let epoch_plus = SystemTime::UNIX_EPOCH + std::time::Duration::new(1_795_198_800, 250_000_000);
        let converted = from_system_time(epoch_plus).unwrap();
        assert_eq!(rfc3339_seconds_z(&converted), "2026-11-20T18:20:00Z");
        assert_eq!(date_ymd(&converted), "2026-11-20");
        let before_epoch = SystemTime::UNIX_EPOCH - std::time::Duration::new(1, 250_000_000);
        assert_eq!(
            rfc3339_library(&from_system_time(before_epoch).unwrap()),
            "1969-12-31T23:59:58.750+00:00"
        );
    }

    #[test]
    fn out_of_range_system_times_are_none_not_a_panic() {
        // 253402300800 s is 10000-01-01T00:00:00Z, one second past what
        // `time` holds without `large-dates`; chrono printed "+10000-01-01".
        let year_10000 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(253_402_300_800);
        assert_eq!(from_system_time(year_10000), None);
        let last = year_10000 - std::time::Duration::from_nanos(1);
        assert_eq!(
            rfc3339_library(&from_system_time(last).unwrap()),
            "9999-12-31T23:59:59.999999999+00:00"
        );
        let year_30000 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(884_524_406_400);
        assert_eq!(from_system_time(year_30000), None);
        if let Some(year_minus_10023) =
            SystemTime::UNIX_EPOCH.checked_sub(std::time::Duration::from_secs(378_489_369_600))
        {
            assert_eq!(from_system_time(year_minus_10023), None);
        }
    }

    #[test]
    fn negative_years_are_signed_and_padded_like_chrono() {
        assert_eq!(date_ymd(&datetime!(-0003-11-27 00:00:00 UTC)), "-0003-11-27");
        assert_eq!(
            rfc3339_seconds_z(&datetime!(-0003-11-27 01:02:03 UTC)),
            "-0003-11-27T01:02:03Z"
        );
        assert_eq!(date_ymd(&datetime!(0000-01-01 00:00:00 UTC)), "0000-01-01");
        assert_eq!(date_ymd(&datetime!(0500-06-07 00:00:00 UTC)), "0500-06-07");
        assert_eq!(year(10_000), "+10000");
        assert_eq!(year(-12_345), "-12345");
    }
}
