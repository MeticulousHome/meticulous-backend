//! Setting the system clock (`TimezoneManager.set_system_datetime`, which the
//! timezone service owns; the same command is used here) and CPython 3.11's
//! `datetime.fromisoformat`, which `POST /machine/time` accepts.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use crate::host::{CommandOutput, Host};

#[derive(Debug)]
pub enum SetTimeError {
    /// `date` could not be started (Python's `OSError`, not caught by callers).
    NotStarted(std::io::Error),
    /// `date` failed (`subprocess.CalledProcessError`).
    Failed(CommandOutput),
}

impl std::fmt::Display for SetTimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetTimeError::NotStarted(error) => write!(f, "{error}"),
            SetTimeError::Failed(output) => write!(
                f,
                "Command 'date' returned non-zero exit status {}.",
                output.returncode()
            ),
        }
    }
}

/// `strftime("%Y-%m-%d %H:%M:%S")` as glibc prints it (no year padding).
pub fn format_datetime(t: &NaiveDateTime) -> String {
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// `TimezoneManager.set_system_datetime`: `date --set "<local time>"`.
pub fn set_system_datetime(host: &Host, new_date: &NaiveDateTime) -> Result<(), SetTimeError> {
    let formatted = format_datetime(new_date);
    tracing::info!("Setting system date to {formatted}");
    match host.run(&["date", "--set", &formatted]) {
        Ok(output) if output.success() => Ok(()),
        Ok(output) => Err(SetTimeError::Failed(output)),
        Err(error) => Err(SetTimeError::NotStarted(error)),
    }
}

/// A parsed `datetime`: naive, or aware with its UTC offset in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsoDateTime {
    pub naive: NaiveDateTime,
    pub offset_seconds: Option<i32>,
}

impl IsoDateTime {
    /// `dt.astimezone()` for an aware value: the same instant in `tz`.
    pub fn in_timezone<Tz: chrono::TimeZone>(&self, tz: &Tz) -> NaiveDateTime {
        match self.offset_seconds {
            None => self.naive,
            Some(offset) => {
                let utc = self.naive - chrono::Duration::seconds(i64::from(offset));
                tz.from_utc_datetime(&utc).naive_local()
            }
        }
    }
}

/// Reading past the end of the C string reads its terminating NUL.
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

fn parse_digits(s: &[u8], pos: &mut usize, count: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..count {
        let c = at(s, *pos);
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(c - b'0');
        *pos += 1;
    }
    Some(value)
}

/// `_find_isoformat_datetime_separator`.
fn find_separator(s: &[u8]) -> usize {
    let len = s.len();
    if len == 7 {
        return 7;
    }
    if at(s, 4) == b'-' {
        if at(s, 5) == b'W' {
            if len < 8 {
                return usize::MAX;
            }
            if len > 8 && at(s, 8) == b'-' {
                if len == 9 {
                    return usize::MAX;
                }
                if len > 10 && at(s, 10).is_ascii_digit() {
                    return 8;
                }
                return 10;
            }
            return 8;
        }
        return 10;
    }
    if at(s, 4) == b'W' {
        let mut idx = 7;
        while idx < len && at(s, idx).is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return idx;
        }
        return if idx % 2 == 0 { 7 } else { 8 };
    }
    8
}

/// `parse_isoformat_date`.
fn parse_date(s: &[u8], len: usize) -> Option<NaiveDate> {
    let mut p = 0;
    let year = parse_digits(s, &mut p, 4)?;
    let uses_separator = at(s, p) == b'-';
    if uses_separator {
        p += 1;
    }
    if at(s, p) == b'W' {
        p += 1;
        let week = parse_digits(s, &mut p, 2)?;
        let day = if p < len {
            if uses_separator {
                if at(s, p) != b'-' {
                    return None;
                }
                p += 1;
            }
            parse_digits(s, &mut p, 1)?
        } else {
            1
        };
        let weekday = match day {
            1 => chrono::Weekday::Mon,
            2 => chrono::Weekday::Tue,
            3 => chrono::Weekday::Wed,
            4 => chrono::Weekday::Thu,
            5 => chrono::Weekday::Fri,
            6 => chrono::Weekday::Sat,
            7 => chrono::Weekday::Sun,
            _ => return None,
        };
        return NaiveDate::from_isoywd_opt(i32::try_from(year).ok()?, week, weekday);
    }
    let month = parse_digits(s, &mut p, 2)?;
    if uses_separator {
        if at(s, p) != b'-' {
            return None;
        }
        p += 1;
    }
    let day = parse_digits(s, &mut p, 2)?;
    if year < 1 {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)
}

/// `parse_hh_mm_ss_ff` over `s[start..end]`: (fields, microseconds, whether
/// something follows).
fn parse_hh_mm_ss_ff(s: &[u8], start: usize, end: usize) -> Option<([u32; 3], u32, bool)> {
    let mut vals = [0u32; 3];
    let mut p = start;
    let mut has_separator = true;
    let mut fraction = false;
    for (i, slot) in vals.iter_mut().enumerate() {
        *slot = parse_digits(s, &mut p, 2)?;
        let c = at(s, p);
        p += 1;
        if i == 0 {
            has_separator = c == b':';
        }
        if p >= end {
            return Some((vals, 0, c != 0));
        } else if has_separator && c == b':' {
            continue;
        } else if c == b'.' || c == b',' {
            fraction = true;
            break;
        } else if !has_separator {
            p -= 1;
        } else {
            return None;
        }
    }
    let _ = fraction;
    let remains = end.saturating_sub(p);
    let to_parse = remains.min(6);
    let mut micro = parse_digits(s, &mut p, to_parse)?;
    if to_parse < 6 {
        micro *= 10u32.pow((6 - to_parse) as u32);
    }
    while at(s, p).is_ascii_digit() {
        p += 1;
    }
    Some((vals, micro, at(s, p) != 0))
}

/// `datetime.fromisoformat` (CPython 3.11, C implementation). `None` is
/// its `ValueError`.
pub fn python_fromisoformat(text: &str) -> Option<IsoDateTime> {
    if text.chars().count() < 7 {
        return None;
    }
    let s = text.as_bytes();
    let len = s.len();
    let separator = find_separator(s);
    if separator == usize::MAX {
        return None;
    }
    let date = parse_date(s, separator)?;
    let mut time = [0u32; 3];
    let mut micro = 0u32;
    let mut offset = None;
    if len > separator {
        let start = separator + 1;
        let mut tz = start;
        while tz < len && !matches!(s[tz], b'Z' | b'+' | b'-') {
            tz += 1;
        }
        let (fields, us, trailing) = parse_hh_mm_ss_ff(s, start, tz)?;
        time = fields;
        micro = us;
        if tz == len {
            if trailing {
                return None;
            }
        } else if s[tz] == b'Z' {
            if at(s, tz + 1) != 0 {
                return None;
            }
            offset = Some(0);
        } else {
            let sign = if s[tz] == b'-' { -1 } else { 1 };
            let (fields, _, trailing) = parse_hh_mm_ss_ff(s, tz + 1, len)?;
            if trailing {
                return None;
            }
            let seconds =
                i64::from(fields[0]) * 3600 + i64::from(fields[1]) * 60 + i64::from(fields[2]);
            if seconds >= 86_400 {
                return None;
            }
            offset = Some(sign * seconds as i32);
        }
    }
    let naive = date.and_hms_micro_opt(time[0], time[1], time[2], micro)?;
    Some(IsoDateTime {
        naive,
        offset_seconds: offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(text: &str) -> Option<String> {
        python_fromisoformat(text).map(|d| d.naive.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
    }

    #[test]
    fn fromisoformat_like_cpython() {
        assert_eq!(
            naive("2026-10-09T12:34:56").as_deref(),
            Some("2026-10-09 12:34:56.000000")
        );
        assert_eq!(
            naive("2026-10-09 12:34").as_deref(),
            Some("2026-10-09 12:34:00.000000")
        );
        assert_eq!(
            naive("2026-10-09").as_deref(),
            Some("2026-10-09 00:00:00.000000")
        );
        assert_eq!(
            naive("20261009T123456").as_deref(),
            Some("2026-10-09 12:34:56.000000")
        );
        assert_eq!(
            naive("2026-10-09T12:34:56.5").as_deref(),
            Some("2026-10-09 12:34:56.500000")
        );
        assert_eq!(
            naive("2026-10-09T12:34:56,1234567").as_deref(),
            Some("2026-10-09 12:34:56.123456")
        );
        assert_eq!(
            naive("2026-W41-5T01").as_deref(),
            Some("2026-10-09 01:00:00.000000")
        );
        assert_eq!(
            naive("2026-10-09x12:34:56").as_deref(),
            Some("2026-10-09 12:34:56.000000")
        );
        assert_eq!(
            python_fromisoformat("2026-10-09T12:34:56+00:00").map(|d| d.offset_seconds),
            Some(Some(0))
        );
        assert_eq!(
            python_fromisoformat("2026-10-09T12:34:56-0530").map(|d| d.offset_seconds),
            Some(Some(-(5 * 3600 + 30 * 60)))
        );
        for bad in [
            "09/10/2026 12:34",
            "2026-13-09",
            "2026-10-09T25:00",
            "2026-10-09T12:34:56+24:00",
            "2026-10-09T12:34:56Zx",
            "2026-1",
            "",
            "2026-10-09T12:34:56 extra",
        ] {
            assert_eq!(naive(bad), None, "{bad}");
        }
    }

    /// tests/test_api_machine.py::test_utc_iso_date_is_converted_to_the_machine_local_time
    /// with TZ "TST+6" (UTC-6).
    #[test]
    fn aware_dates_convert_to_local_time() {
        let parsed = python_fromisoformat("2026-10-09T12:34:56+00:00").expect("parsed");
        let tst = chrono::FixedOffset::west_opt(6 * 3600).expect("offset");
        assert_eq!(
            format_datetime(&parsed.in_timezone(&tst)),
            "2026-10-09 06:34:56"
        );
    }
}
