//! Python's naive local datetimes, as the history stores them.
//!
//! `insert_history` stores `datetime.fromtimestamp(time)`, the *local* wall
//! clock time labelled UTC, and SQLAlchemy writes it as
//! `YYYY-MM-DD HH:MM:SS.ffffff` without an offset. Reading it back,
//! `datetime.timestamp()` interprets the naive value as local time again. To
//! read and write rows the Python can read and write, Rust does the same
//! arithmetic: CPython's `datetime_from_timet_and_us` (with its microsecond
//! rounding) and `local_to_seconds` (with `fold=0`), over the local UTC offset
//! that `localtime()` reports.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike};

use crate::error::{Error, Result};

/// Where the local UTC offset comes from.
#[derive(Clone, Default)]
pub enum LocalZone {
    /// The process's local time zone (`TZ`, `/etc/localtime`), as `localtime()`.
    #[default]
    System,
    /// A fixed offset in seconds east of UTC.
    Fixed(i32),
    /// Any zone: the offset in seconds east of UTC at a UTC instant.
    Custom(Arc<dyn Fn(i64) -> i32 + Send + Sync>),
}

impl std::fmt::Debug for LocalZone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "System"),
            Self::Fixed(offset) => write!(f, "Fixed({offset})"),
            Self::Custom(_) => write!(f, "Custom"),
        }
    }
}

/// A naive datetime with microseconds, like Python's `datetime` without tzinfo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NaiveTime {
    /// Seconds since 1970-01-01 00:00:00 of the wall clock (no zone).
    pub seconds: i64,
    pub microsecond: u32,
}

impl NaiveTime {
    pub fn from_naive(datetime: NaiveDateTime) -> Self {
        Self {
            seconds: datetime.and_utc().timestamp(),
            microsecond: datetime.nanosecond() / 1000,
        }
    }

    pub fn to_naive(self) -> Option<NaiveDateTime> {
        DateTime::from_timestamp(self.seconds, self.microsecond * 1000).map(|d| d.naive_utc())
    }

    /// SQLAlchemy's SQLite `DATETIME` storage format.
    pub fn to_sqlite(self) -> Result<String> {
        let datetime = self
            .to_naive()
            .ok_or_else(|| Error::InvalidValue("year is out of range".into()))?;
        Ok(format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
            datetime.year(),
            datetime.month(),
            datetime.day(),
            datetime.hour(),
            datetime.minute(),
            datetime.second(),
            self.microsecond
        ))
    }
}

/// A value read from a `DATETIME` column: `datetime.fromisoformat(text)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredTime {
    pub naive: NaiveTime,
    /// The UTC offset in seconds when the text carried one.
    pub offset: Option<i32>,
}

/// `datetime.fromisoformat()` for the forms a `DATETIME` column holds:
/// `YYYY-MM-DD[( |T)HH:MM[:SS[.fraction]]][Z|±HH:MM[:SS]]`.
pub fn parse_stored(text: &str) -> Result<StoredTime> {
    let invalid = || Error::InvalidValue(format!("Invalid isoformat string: '{text}'"));
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let slice = text.get(range)?;
        if slice.bytes().all(|b| b.is_ascii_digit()) && !slice.is_empty() {
            slice.parse().ok()
        } else {
            None
        }
    };
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid());
    }
    let year = digits(0..4).ok_or_else(invalid)?;
    let month = digits(5..7).ok_or_else(invalid)?;
    let day = digits(8..10).ok_or_else(invalid)?;
    let date = NaiveDate::from_ymd_opt(year as i32, month, day).ok_or_else(invalid)?;
    let (mut hour, mut minute, mut second, mut micro) = (0, 0, 0, 0);
    let mut offset = None;
    if bytes.len() > 10 {
        if bytes[10] != b' ' && bytes[10] != b'T' {
            return Err(invalid());
        }
        let rest = &text[11..];
        let tz_start = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
        let (time, zone) = rest.split_at(tz_start);
        let mut parts = time.splitn(3, ':');
        let h = parts.next().ok_or_else(invalid)?;
        let m = parts.next().ok_or_else(invalid)?;
        if h.len() != 2 || m.len() != 2 {
            return Err(invalid());
        }
        hour = h.parse().map_err(|_| invalid())?;
        minute = m.parse().map_err(|_| invalid())?;
        if let Some(s) = parts.next() {
            let (whole, fraction) = match s.split_once(['.', ',']) {
                Some((whole, fraction)) => (whole, Some(fraction)),
                None => (s, None),
            };
            if whole.len() != 2 {
                return Err(invalid());
            }
            second = whole.parse().map_err(|_| invalid())?;
            if let Some(fraction) = fraction {
                if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid());
                }
                let mut padded: String = fraction.chars().take(6).collect();
                while padded.len() < 6 {
                    padded.push('0');
                }
                micro = padded.parse().map_err(|_| invalid())?;
            }
        }
        if !zone.is_empty() {
            offset = Some(parse_offset(zone).ok_or_else(invalid)?);
        }
    }
    let datetime = date
        .and_hms_micro_opt(hour, minute, second, micro)
        .ok_or_else(invalid)?;
    Ok(StoredTime {
        naive: NaiveTime::from_naive(datetime),
        offset,
    })
}

fn parse_offset(zone: &str) -> Option<i32> {
    if zone == "Z" {
        return Some(0);
    }
    let sign = match zone.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let mut parts = zone[1..].split(':');
    let hours: i32 = parts.next()?.parse().ok()?;
    let minutes: i32 = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    let seconds: i32 = parts.next().map_or(Some(0), |s| s.parse().ok())?;
    Some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// `round()` with ties to even, as `_PyTime_ROUND_HALF_EVEN`.
fn round_half_even(value: f64) -> f64 {
    value.round_ties_even()
}

impl LocalZone {
    /// The UTC offset in seconds at a UTC instant (`localtime(u) - u`).
    pub fn offset_at(&self, utc: i64) -> i64 {
        match self {
            Self::System => match Local.timestamp_opt(utc, 0) {
                chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
                    i64::from(t.offset().fix().local_minus_utc())
                }
                chrono::LocalResult::None => 0,
            },
            Self::Fixed(offset) => i64::from(*offset),
            Self::Custom(offset) => i64::from(offset(utc)),
        }
    }

    fn local(&self, utc: i64) -> i64 {
        utc + self.offset_at(utc)
    }

    /// `datetime.fromtimestamp(timestamp)` for a float timestamp: the local
    /// wall clock, microseconds rounded half to even.
    pub fn from_timestamp(&self, timestamp: f64) -> Result<NaiveTime> {
        if !timestamp.is_finite() {
            return Err(Error::InvalidValue(if timestamp.is_nan() {
                "Invalid value NaN (not a number)".into()
            } else {
                "cannot convert float infinity to integer".into()
            }));
        }
        let mut whole = timestamp.trunc();
        let mut fraction = round_half_even((timestamp - whole) * 1e6);
        if fraction >= 1e6 {
            fraction -= 1e6;
            whole += 1.0;
        } else if fraction < 0.0 {
            fraction += 1e6;
            whole -= 1.0;
        }
        if !(-1e15..=1e15).contains(&whole) {
            return Err(Error::InvalidValue(
                "timestamp out of range for platform time_t".into(),
            ));
        }
        Ok(NaiveTime {
            seconds: self.local(whole as i64),
            microsecond: fraction as u32,
        })
    }

    /// `datetime.fromtimestamp(timestamp)` for an integer timestamp.
    pub fn from_timestamp_int(&self, timestamp: i64) -> NaiveTime {
        NaiveTime {
            seconds: self.local(timestamp),
            microsecond: 0,
        }
    }

    /// CPython's `local_to_seconds(..., fold=0)`: the UTC instant of a naive
    /// local wall clock time.
    pub fn local_to_utc(&self, wall: i64) -> i64 {
        const MAX_FOLD_SECONDS: i64 = 24 * 3600;
        let t = wall;
        let a = self.local(t) - t;
        let u1 = t - a;
        let t1 = self.local(u1);
        let b = if t1 == t {
            let u2 = u1 - MAX_FOLD_SECONDS;
            let candidate = self.local(u2) - u2;
            if a == candidate {
                return u1;
            }
            candidate
        } else {
            t1 - u1
        };
        let u2 = t - b;
        let t2 = self.local(u2);
        if t2 == t {
            return u2;
        }
        if t1 == t {
            return u1;
        }
        // t is in a gap: neither offset solves it.
        u1.max(u2)
    }

    /// `naive_datetime.timestamp()`.
    pub fn timestamp(&self, time: NaiveTime) -> f64 {
        self.local_to_utc(time.seconds) as f64 + f64::from(time.microsecond) / 1e6
    }

    /// `datetime.fromisoformat(stored).timestamp()`: local time unless the
    /// text carried an offset.
    pub fn stored_timestamp(&self, stored: StoredTime) -> f64 {
        match stored.offset {
            Some(offset) => {
                let micros = (i128::from(stored.naive.seconds) - i128::from(offset)) * 1_000_000
                    + i128::from(stored.naive.microsecond);
                micros as f64 / 1e6
            }
            None => self.timestamp(stored.naive),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_format_round_trips() {
        let zone = LocalZone::Fixed(0);
        let time = zone.from_timestamp(1760000000.25).ok();
        assert_eq!(
            time.and_then(|t| t.to_sqlite().ok()).as_deref(),
            Some("2025-10-09 08:53:20.250000")
        );
        let stored = parse_stored("2025-10-09 08:53:20.250000").ok();
        assert_eq!(
            stored.map(|s| zone.stored_timestamp(s)),
            Some(1760000000.25)
        );
    }

    #[test]
    fn fromisoformat_forms() {
        let zone = LocalZone::Fixed(3600);
        let parse = |text: &str| parse_stored(text).ok().map(|s| zone.stored_timestamp(s));
        assert_eq!(parse("1970-01-01 01:00:00.000000"), Some(0.0));
        assert_eq!(parse("1970-01-01T01:00:00"), Some(0.0));
        assert_eq!(parse("1970-01-01 01:00"), Some(0.0));
        assert_eq!(parse("1970-01-01"), Some(-3600.0));
        assert_eq!(parse("1970-01-01 00:00:00+00:00"), Some(0.0));
        assert_eq!(parse("1970-01-01 00:00:00.5Z"), Some(0.5));
        assert!(parse_stored("not a date").is_err());
    }
}
