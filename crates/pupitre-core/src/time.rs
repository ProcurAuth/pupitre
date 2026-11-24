// SPDX-License-Identifier: MIT OR Apache-2.0
//! Times in UTC to the millisecond, with the one text form `2026-11-24T10:00:00.000Z` (5.3).

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A time in milliseconds since the Unix epoch, in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcMillis(pub u64);

/// The largest time that the text form can hold: 9999-12-31T23:59:59.999Z.
const MAX_MS: u64 = 253_402_300_799_999;

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl UtcMillis {
    /// The time of the system clock, cut to the millisecond. A clock before 1970 gives 0.
    pub fn now() -> UtcMillis {
        UtcMillis::from_system(SystemTime::now())
    }

    /// Convert a system time, cut to the millisecond. A time before 1970 gives 0.
    pub fn from_system(time: SystemTime) -> UtcMillis {
        let ms = time.duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        UtcMillis(u64::try_from(ms).unwrap_or(u64::MAX).min(MAX_MS))
    }

    /// Read the text form `YYYY-MM-DDTHH:MM:SS.mmmZ`. Any other form is an error.
    pub fn parse(text: &str) -> Option<UtcMillis> {
        let b = text.as_bytes();
        if b.len() != 24
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
            || b[19] != b'.'
            || b[23] != b'Z'
        {
            return None;
        }
        let num = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = &text[range];
            if part.bytes().all(|c| c.is_ascii_digit()) { part.parse().ok() } else { None }
        };
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, s, ms) = (num(11..13)?, num(14..16)?, num(17..19)?, num(20..23)?);
        if !(1..=12).contains(&mo) || d == 0 || h > 23 || mi > 59 || s > 59 || y < 1970 {
            return None;
        }
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days_in_month = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        if d > days_in_month[(mo - 1) as usize] {
            return None;
        }
        let days = days_from_civil(i64::from(y), mo, d);
        let secs = days * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s);
        let total = u64::try_from(secs).ok()?.checked_mul(1000)?.checked_add(u64::from(ms))?;
        Some(UtcMillis(total))
    }

    /// The milliseconds since `earlier`, or 0 when `earlier` is later.
    pub fn since(&self, earlier: UtcMillis) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl fmt::Display for UtcMillis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.0.min(MAX_MS);
        let secs = (ms / 1000) as i64;
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let rem = secs.rem_euclid(86_400);
        write!(f, "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, (rem % 3600) / 60, rem % 60, ms % 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for text in ["1970-01-01T00:00:00.000Z", "2026-11-24T10:00:00.000Z", "2028-02-29T23:59:59.999Z"] {
            let t = UtcMillis::parse(text).unwrap();
            assert_eq!(t.to_string(), text);
        }
        assert_eq!(UtcMillis::parse("2026-11-24T10:00:00.000Z").unwrap().0, 1_795_514_400_000);
    }

    #[test]
    fn refused_forms() {
        for bad in [
            "2026-11-24T10:00:00Z",
            "2026-11-24T10:00:00.000+00:00",
            "2026-02-30T10:00:00.000Z",
            "2027-02-29T10:00:00.000Z",
            "2026-11-24 10:00:00.000Z",
            "2026-13-01T00:00:00.000Z",
            "1969-12-31T23:59:59.999Z",
        ] {
            assert!(UtcMillis::parse(bad).is_none(), "{bad}");
        }
    }
}
