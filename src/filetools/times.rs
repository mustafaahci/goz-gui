//! A file's three times, what Set File Time does to each, and the ISO 8601
//! dates its Custom boxes take (Everything's: `YYYY-MM-DDThh:mm:ss`, the
//! earliest value for what is left out, in local time).

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    TzSpecificLocalTimeToSystemTime,
};

/// A file's times, as FILETIMEs (UTC).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Times {
    pub created: u64,
    pub accessed: u64,
    pub modified: u64,
}

/// What one of the times becomes: Everything's radio buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    /// No change.
    Keep,
    /// The file's own date created, modified or accessed, as it was.
    Created,
    Modified,
    Accessed,
    /// This moment (Now, or a Custom date).
    At(u64),
}

/// The times to give a file whose times are `before`, as `set` says for
/// its date created, modified and accessed (in that order, the dialog's):
/// `(created, accessed, modified)` for `SetFileTime`, `None` to leave one
/// as it is. A copy from another time takes that time as it was before.
pub fn apply(before: Times, set: &[Set; 3]) -> (Option<u64>, Option<u64>, Option<u64>) {
    let value = |s: Set| match s {
        Set::Keep => None,
        Set::Created => Some(before.created),
        Set::Modified => Some(before.modified),
        Set::Accessed => Some(before.accessed),
        Set::At(t) => Some(t),
    };
    (value(set[0]), value(set[2]), value(set[1]))
}

/// A local date and time, as typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

fn leap(y: u16) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

fn days_in(y: u16, m: u16) -> u16 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Reads an ISO 8601 date: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, then `T` (or a
/// space) and `hh`, `hh:mm`, `hh:mm:ss` or `hh:mm:ss.fff`; what is left out
/// is the earliest (January, the 1st, midnight). Years 1601 to 9999, as a
/// FILETIME holds them. `None` when it is not such a date.
pub fn parse_iso(text: &str) -> Option<Local> {
    let text = text.trim();
    let (date, time) = match text.find(['T', 't', ' ']) {
        Some(i) => (&text[..i], Some(text[i + 1..].trim())),
        None => (text, None),
    };
    let num = |s: &str, len: usize| -> Option<u16> {
        (s.len() == len && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let mut parts = date.split('-');
    let year = num(parts.next()?, 4)?;
    let month = parts.next().map_or(Some(1), |m| num(m, 2))?;
    let day = parts.next().map_or(Some(1), |d| num(d, 2))?;
    if parts.next().is_some() {
        return None;
    }
    let (mut hour, mut minute, mut second, mut millis) = (0, 0, 0, 0);
    if let Some(time) = time {
        let (hms, frac) = match time.split_once(['.', ',']) {
            Some((a, b)) => (a, Some(b)),
            None => (time, None),
        };
        let mut t = hms.split(':');
        hour = num(t.next()?, 2)?;
        minute = t.next().map_or(Some(0), |m| num(m, 2))?;
        second = t.next().map_or(Some(0), |s| num(s, 2))?;
        if t.next().is_some() {
            return None;
        }
        if let Some(frac) = frac {
            if frac.is_empty() || frac.len() > 7 || !frac.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            // Milliseconds: the first three digits, padded.
            let ms: String = frac.chars().chain("000".chars()).take(3).collect();
            millis = ms.parse().ok()?;
        }
    }
    let ok = (1601..=9999).contains(&year)
        && (1..=12).contains(&month)
        && (1..=days_in(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60;
    ok.then_some(Local {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millis,
    })
}

impl Local {
    /// The FILETIME (UTC) of this local time, by the time zone's rules for
    /// that date; `None` if Windows will not convert it.
    pub fn to_filetime(self) -> Option<u64> {
        let local = SYSTEMTIME {
            wYear: self.year,
            wMonth: self.month,
            wDayOfWeek: 0,
            wDay: self.day,
            wHour: self.hour,
            wMinute: self.minute,
            wSecond: self.second,
            wMilliseconds: self.millis,
        };
        // SAFETY: plain in- and out-structs; a null time zone is the
        // current one.
        unsafe {
            let mut utc: SYSTEMTIME = core::mem::zeroed();
            if TzSpecificLocalTimeToSystemTime(core::ptr::null(), &local, &mut utc) == 0 {
                return None;
            }
            let mut ft = FILETIME::default();
            if SystemTimeToFileTime(&utc, &mut ft) == 0 {
                return None;
            }
            Some((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime))
        }
    }

    /// The local time of a FILETIME (UTC).
    pub fn of_filetime(ft: u64) -> Option<Local> {
        let ft = FILETIME {
            dwLowDateTime: ft as u32,
            dwHighDateTime: (ft >> 32) as u32,
        };
        // SAFETY: plain in- and out-structs; a null time zone is the
        // current one.
        unsafe {
            let mut utc: SYSTEMTIME = core::mem::zeroed();
            let mut local: SYSTEMTIME = core::mem::zeroed();
            if FileTimeToSystemTime(&ft, &mut utc) == 0
                || SystemTimeToTzSpecificLocalTime(core::ptr::null(), &utc, &mut local) == 0
            {
                return None;
            }
            Some(Local {
                year: local.wYear,
                month: local.wMonth,
                day: local.wDay,
                hour: local.wHour,
                minute: local.wMinute,
                second: local.wSecond,
                millis: local.wMilliseconds,
            })
        }
    }

    /// As ISO 8601 (`2024-05-01T09:30:00`).
    pub fn iso(self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16, ms: u16) -> Local {
        Local {
            year: y,
            month: mo,
            day: d,
            hour: h,
            minute: mi,
            second: s,
            millis: ms,
        }
    }

    #[test]
    fn iso_dates_read_with_the_earliest_for_what_is_left_out() {
        assert_eq!(parse_iso("2024"), Some(at(2024, 1, 1, 0, 0, 0, 0)));
        assert_eq!(parse_iso("2024-05"), Some(at(2024, 5, 1, 0, 0, 0, 0)));
        assert_eq!(parse_iso("2024-05-17"), Some(at(2024, 5, 17, 0, 0, 0, 0)));
        assert_eq!(
            parse_iso(" 2024-05-17T09 "),
            Some(at(2024, 5, 17, 9, 0, 0, 0))
        );
        assert_eq!(
            parse_iso("2024-05-17 09:30"),
            Some(at(2024, 5, 17, 9, 30, 0, 0))
        );
        assert_eq!(
            parse_iso("2024-05-17t23:59:58.5"),
            Some(at(2024, 5, 17, 23, 59, 58, 500))
        );
        assert_eq!(
            parse_iso("2024-02-29T00:00:00.1234567"),
            Some(at(2024, 2, 29, 0, 0, 0, 123))
        );
        for bad in [
            "",
            "24-05-17",
            "2024-5-17",
            "2024-13-01",
            "2023-02-29",
            "2024-04-31",
            "2024-05-17T24:00",
            "2024-05-17T10:60",
            "2024-05-17T10:00:00:00",
            "2024-05-17-01",
            "1600-12-31",
            "2024-05-17T10:00.",
            "today",
        ] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
    }

    #[test]
    fn local_times_go_to_filetimes_and_back() {
        let t = at(2021, 7, 14, 13, 45, 30, 0);
        let ft = t.to_filetime().unwrap();
        assert_eq!(Local::of_filetime(ft), Some(t));
        assert_eq!(t.iso(), "2021-07-14T13:45:30");
    }

    #[test]
    fn times_are_set_from_each_other_as_they_were() {
        let before = Times {
            created: 1,
            accessed: 2,
            modified: 3,
        };
        // Created from modified, modified from created: each from the
        // other's old value; accessed now.
        assert_eq!(
            apply(before, &[Set::Modified, Set::Created, Set::At(9)]),
            (Some(3), Some(9), Some(1))
        );
        assert_eq!(apply(before, &[Set::Keep; 3]), (None, None, None));
        assert_eq!(
            apply(before, &[Set::Keep, Set::Accessed, Set::Keep]),
            (None, None, Some(2))
        );
    }
}
