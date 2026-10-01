//! Times as people read them where nib runs, for plugins, which cannot
//! know the time zone from inside WASI (docs/design/plugins/files.md), and the CPU time
//! the plugins' calls use (docs/design/architecture.md, "時間と資源の上限").

use std::time::Duration;

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

/// The CPU time the calling thread has used; `None` where nib cannot
/// tell.
#[cfg(unix)]
pub(crate) fn thread_cpu() -> Option<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime only writes the timespec it is given.
    let read = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } == 0;
    read.then(|| Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}

#[cfg(windows)]
pub(crate) fn thread_cpu() -> Option<Duration> {
    use std::ffi::c_void;
    // FILETIMEs, in 100 ns, read as the u64 they are on little-endian
    // machines.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }
    let (mut creation, mut exit, mut kernel, mut user) = (0, 0, 0, 0);
    // SAFETY: GetThreadTimes only writes the four times it is given.
    let read = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } != 0;
    read.then(|| Duration::from_nanos((kernel + user) * 100))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn thread_cpu() -> Option<Duration> {
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
    fn the_thread_cpu_time_grows_with_work_but_not_with_sleep() {
        let Some(before) = thread_cpu() else {
            return;
        };
        std::thread::sleep(Duration::from_millis(50));
        let slept = thread_cpu().unwrap() - before;
        assert!(slept < Duration::from_millis(25), "{slept:?}");
        // Work counts, however busy the machine: it ends.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut x = 0u64;
        while thread_cpu().unwrap() - before - slept < Duration::from_millis(20) {
            assert!(std::time::Instant::now() < deadline, "no CPU time counted");
            for _ in 0..10_000 {
                x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
            }
        }
    }

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
