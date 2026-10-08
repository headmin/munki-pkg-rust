//! Broken-down wall-clock time, via the system's own timezone database.
//!
//! Dynamic version tokens are stamped in local time (matching munki-pkg), while
//! provenance records UTC. Both come from libc rather than a date crate: the
//! tool is macOS-only, and `localtime_r` already knows the host's rules.

/// A broken-down calendar time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Timestamp {
    /// Construct a timestamp directly, to pin the clock in tests.
    #[cfg(test)]
    pub fn new(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    /// The current time in the host's local timezone.
    pub fn now_local() -> Self {
        Self::from_epoch(now_epoch(), Zone::Local)
    }

    /// The current time in UTC.
    pub fn now_utc() -> Self {
        Self::from_epoch(now_epoch(), Zone::Utc)
    }

    fn from_epoch(epoch: libc::time_t, zone: Zone) -> Self {
        // SAFETY: `tm` is fully initialized by localtime_r/gmtime_r, both of
        // which write into the caller's buffer and are thread-safe. We pass a
        // valid pointer to a live stack value for both arguments.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            let filled = match zone {
                Zone::Local => libc::localtime_r(&epoch, &mut tm),
                Zone::Utc => libc::gmtime_r(&epoch, &mut tm),
            };
            if filled.is_null() {
                // Conversion cannot fail for a time_t the system just produced,
                // but fall back to the epoch rather than reading a null result.
                libc::tm {
                    tm_sec: 0,
                    tm_min: 0,
                    tm_hour: 0,
                    tm_mday: 1,
                    tm_mon: 0,
                    tm_year: 70,
                    tm_wday: 0,
                    tm_yday: 0,
                    tm_isdst: 0,
                    tm_gmtoff: 0,
                    tm_zone: std::ptr::null_mut(),
                }
            } else {
                tm
            }
        };

        Self {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
            second: tm.tm_sec as u32,
        }
    }

    /// `yyyy.MM.dd`
    pub fn date(&self) -> String {
        format!("{:04}.{:02}.{:02}", self.year, self.month, self.day)
    }

    /// `yyyy.MM.dd.HHmm`
    pub fn timestamp(&self) -> String {
        format!(
            "{:04}.{:02}.{:02}.{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }

    /// `yyyy.MM.dd.HHmmss`
    pub fn datetime(&self) -> String {
        format!(
            "{:04}.{:02}.{:02}.{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// RFC 3339 / ISO 8601 in UTC, e.g. `2026-09-12T14:05:30Z`.
    ///
    /// Only meaningful on a timestamp built with [`Timestamp::now_utc`].
    pub fn iso8601_utc(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

enum Zone {
    Local,
    Utc,
}

fn now_epoch() -> libc::time_t {
    // SAFETY: passing null is the documented way to ask for the return value only.
    unsafe { libc::time(std::ptr::null_mut()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> Timestamp {
        Timestamp::new(2026, 7, 18, 14, 5, 30)
    }

    #[test]
    fn test_format_widths_are_zero_padded() {
        let ts = Timestamp::new(2026, 1, 2, 3, 4, 5);
        assert_eq!(ts.date(), "2026.01.02");
        assert_eq!(ts.timestamp(), "2026.01.02.0304");
        assert_eq!(ts.datetime(), "2026.01.02.030405");
        assert_eq!(ts.iso8601_utc(), "2026-01-02T03:04:05Z");
    }

    #[test]
    fn test_formats_match_munki_pkg() {
        assert_eq!(fixed().date(), "2026.07.18");
        assert_eq!(fixed().timestamp(), "2026.07.18.1405");
        assert_eq!(fixed().datetime(), "2026.07.18.140530");
    }

    #[test]
    fn test_now_returns_a_plausible_calendar_date() {
        let now = Timestamp::now_utc();
        assert!(now.year >= 2024, "year was {}", now.year);
        assert!((1..=12).contains(&now.month));
        assert!((1..=31).contains(&now.day));
        assert!(now.hour < 24 && now.minute < 60 && now.second < 61);
    }
}
