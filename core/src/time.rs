//! Times as people read them where nib runs, for plugins, which cannot
//! know the time zone from inside WASI (docs/files.md).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalTime {
    pub year: u32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// `seconds` since 1970 in the local time zone; UTC where nib does not
/// know it.
pub(crate) fn local(seconds: u64) -> LocalTime {
    local_offset(seconds).map_or_else(
        || utc(seconds),
        |offset| utc(seconds.saturating_add_signed(offset)),
    )
}

/// How far the local time zone is ahead of UTC at `seconds`, in seconds.
#[cfg(unix)]
fn local_offset(seconds: u64) -> Option<i64> {
    let time = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: localtime_r only writes the tm it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let filled = unsafe { libc::localtime_r(&time, &mut tm) };
    (!filled.is_null()).then_some(tm.tm_gmtoff as i64)
}

#[cfg(not(unix))]
fn local_offset(_seconds: u64) -> Option<i64> {
    None
}

/// `seconds` since 1970 in UTC.
fn utc(seconds: u64) -> LocalTime {
    let days = (seconds / 86_400) as i64;
    let in_day = seconds % 86_400;
    // Howard Hinnant's days-to-civil, for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    LocalTime {
        year: year as u32,
        month: month as u8,
        day: day as u8,
        hour: (in_day / 3_600) as u8,
        minute: (in_day % 3_600 / 60) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates_read_as_calendars_do() {
        let at = |year, month, day, hour, minute| LocalTime {
            year,
            month,
            day,
            hour,
            minute,
        };
        assert_eq!(utc(0), at(1970, 1, 1, 0, 0));
        // 2000-02-29 12:34 UTC, a leap day.
        assert_eq!(utc(951_827_640), at(2000, 2, 29, 12, 34));
        // 2026-09-28 05:02 UTC.
        assert_eq!(utc(1_790_571_720), at(2026, 9, 28, 5, 2));
    }
}
