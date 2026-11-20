//! Timestamps DepoAudio writes, built on the `time` crate.
//!
//! These used to come from `chrono`. Each formatter here reproduces the exact
//! text chrono wrote, so export records, backups and libraries written by
//! older builds and by this one look the same, and nothing that reads them
//! back (the app, a court, a script) sees a change. Everything is UTC; the
//! app never needs the local offset, so `time`'s `local-offset` feature is
//! not used.

use std::time::SystemTime;
use time::OffsetDateTime;

/// The current time in UTC.
pub(crate) fn now_utc() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// A file's modification time (or any `SystemTime`) in UTC.
pub(crate) fn from_system_time(time: SystemTime) -> OffsetDateTime {
    OffsetDateTime::from(time)
}

/// `YYYY-MM-DD` in UTC, as chrono's `format("%Y-%m-%d")` wrote it.
pub(crate) fn date_ymd(time: &OffsetDateTime) -> String {
    let time = time.to_offset(time::UtcOffset::UTC);
    format!("{:04}-{:02}-{:02}", time.year(), u8::from(time.month()), time.day())
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

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;
    use time::macros::datetime;

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
        let converted = from_system_time(epoch_plus);
        assert_eq!(rfc3339_seconds_z(&converted), "2026-11-20T18:20:00Z");
        assert_eq!(date_ymd(&converted), "2026-11-20");
    }
}
