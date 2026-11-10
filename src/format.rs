//! Sizes and dates as the user's locale writes them.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::Globalization::{
    DATE_SHORTDATE, GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, LOCALE_SDECIMAL,
    LOCALE_STHOUSAND, TIME_NOSECONDS,
};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use crate::ui_options::SizeFormat;

/// One of the user's locale strings, such as a separator, or `default`.
fn locale_text(kind: u32, default: &str) -> String {
    let mut buf = [0u16; 8];
    // SAFETY: a null locale name is LOCALE_NAME_USER_DEFAULT; the buffer
    // length passed is the buffer's.
    let n = unsafe { GetLocaleInfoEx(core::ptr::null(), kind, buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 {
        String::from_utf16_lossy(&buf[..n as usize - 1])
    } else {
        default.to_owned()
    }
}

/// The user's digit-group separator, read once.
fn thousands() -> &'static str {
    static SEP: OnceLock<String> = OnceLock::new();
    SEP.get_or_init(|| locale_text(LOCALE_STHOUSAND, ","))
}

/// `n` with digit grouping: `1,245`.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let sep = thousands();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * sep.len());
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(ch);
    }
    out
}

/// A file size in whole kilobytes, rounded up: `1,245 KB`.
pub fn size_kb(bytes: u64) -> String {
    format!("{} KB", grouped(bytes.div_ceil(1024)))
}

/// Options > View's "Size format" for the Size column, set by the window
/// (`SizeFormat::number`); the list's rows, its cell tips and the copy
/// commands all read it.
static SIZE_FORMAT: AtomicU8 = AtomicU8::new(2);

pub fn set_size_format(f: SizeFormat) {
    SIZE_FORMAT.store(f.number(), Ordering::Relaxed);
}

/// A file size as the Size column shows it, in Options > View's format.
pub fn size_cell(bytes: u64) -> String {
    let f =
        SizeFormat::from_number(i64::from(SIZE_FORMAT.load(Ordering::Relaxed))).unwrap_or_default();
    size_as(bytes, f)
}

/// A file size in format `f`: `87.2 KB` (Auto), `89,293` (bytes), `88 KB`
/// (whole kilobytes, rounded up) or `1 MB` (whole megabytes, rounded up).
pub fn size_as(bytes: u64, f: SizeFormat) -> String {
    match f {
        SizeFormat::Auto => size_auto(bytes),
        SizeFormat::Bytes => grouped(bytes),
        SizeFormat::Kb => size_kb(bytes),
        SizeFormat::Mb => format!("{} MB", grouped(bytes.div_ceil(1024 * 1024))),
    }
}

/// A byte count in the largest unit that keeps it readable: `215 MB`.
pub fn size_short(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// A byte count with three significant digits in the unit that keeps it
/// under 1,000, cut off rather than rounded, as Explorer and Everything's
/// status bar write sizes: `40.0 KB`, `87.2 KB`, `1.25 MB`, `352 KB`.
/// Under 1 KB it is whole bytes.
pub fn size_auto(bytes: u64) -> String {
    size_auto_with(bytes, decimal())
}

fn size_auto_with(bytes: u64, decimal: &str) -> String {
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1024 {
        return format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" });
    }
    // Integers throughout, so the cut-off digits are exact.
    let bytes = u128::from(bytes);
    let mut unit = 0;
    let mut per = 1024u128;
    while bytes >= 1000 * per && unit + 1 < UNITS.len() {
        per *= 1024;
        unit += 1;
    }
    let hundredths = bytes * 100 / per;
    let (whole, frac) = (hundredths / 100, hundredths % 100);
    let text = if whole < 10 {
        format!("{whole}{decimal}{frac:02}")
    } else if whole < 100 {
        format!("{whole}{decimal}{}", frac / 10)
    } else {
        whole.to_string()
    };
    format!("{text} {}", UNITS[unit])
}

/// The user's decimal separator, read once.
fn decimal() -> &'static str {
    static SEP: OnceLock<String> = OnceLock::new();
    SEP.get_or_init(|| locale_text(LOCALE_SDECIMAL, "."))
}

/// A FILETIME as the local short date and time, appended to `out` as UTF-16.
pub fn date(ft: i64, out: &mut Vec<u16>) {
    let ft = FILETIME {
        dwLowDateTime: ft as u32,
        dwHighDateTime: (ft >> 32) as u32,
    };
    // SAFETY: plain out-structs; the buffers' lengths are passed with them,
    // and a null locale name is the user default.
    unsafe {
        let mut utc: SYSTEMTIME = core::mem::zeroed();
        let mut local: SYSTEMTIME = core::mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(core::ptr::null(), &utc, &mut local) == 0
        {
            return;
        }
        let mut buf = [0u16; 64];
        let n = GetDateFormatEx(
            core::ptr::null(),
            DATE_SHORTDATE,
            &local,
            core::ptr::null(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            core::ptr::null(),
        );
        if n > 1 {
            out.extend_from_slice(&buf[..n as usize - 1]);
            out.push(u16::from(b' '));
        }
        let n = GetTimeFormatEx(
            core::ptr::null(),
            TIME_NOSECONDS,
            &local,
            core::ptr::null(),
            buf.as_mut_ptr(),
            buf.len() as i32,
        );
        if n > 1 {
            out.extend_from_slice(&buf[..n as usize - 1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kilobytes_round_up() {
        assert_eq!(size_kb(0), "0 KB");
        assert_eq!(size_kb(1), "1 KB");
        assert_eq!(size_kb(1024), "1 KB");
        assert_eq!(size_kb(1025), "2 KB");
    }

    #[test]
    fn sizes_in_each_format() {
        // Grouped as the user's locale does; "," here for the test.
        let sep = thousands();
        let g = |s: &str| s.replace(',', sep);
        assert_eq!(size_as(89_293, SizeFormat::Kb), g("88 KB"));
        assert_eq!(size_as(89_293, SizeFormat::Bytes), g("89,293"));
        assert_eq!(size_as(0, SizeFormat::Bytes), "0");
        assert_eq!(size_as(1, SizeFormat::Mb), "1 MB");
        assert_eq!(size_as(0, SizeFormat::Mb), "0 MB");
        assert_eq!(size_as(5 * 1024 * 1024, SizeFormat::Mb), "5 MB");
        assert_eq!(size_as(5 * 1024 * 1024 + 1, SizeFormat::Mb), "6 MB");
        assert_eq!(
            size_as(3_000_000_000_000, SizeFormat::Mb),
            g("2,861,023 MB")
        );
        assert_eq!(size_as(89_344, SizeFormat::Auto), size_auto(89_344));
        // The column's format is KB until the window says otherwise.
        assert_eq!(SizeFormat::default(), SizeFormat::Kb);
    }

    #[test]
    fn auto_sizes_keep_three_digits() {
        let auto = |n| size_auto_with(n, ".");
        assert_eq!(auto(0), "0 bytes");
        assert_eq!(auto(1), "1 byte");
        assert_eq!(auto(1023), "1023 bytes");
        assert_eq!(auto(1024), "1.00 KB");
        assert_eq!(auto(40 * 1024), "40.0 KB");
        assert_eq!(auto(89_344), "87.2 KB", "87.25 KB, as Everything shows it");
        assert_eq!(auto(352 * 1024 + 100), "352 KB");
        assert_eq!(auto(1_310_720), "1.25 MB");
        assert_eq!(auto(568 * 1024 * 1024), "568 MB");
        assert_eq!(auto(5 * 1024 * 1024 * 1024 * 1024), "5.00 TB");
        assert_eq!(size_auto_with(89_344, ","), "87,2 KB");
    }

    #[test]
    fn auto_sizes_cut_digits_off_and_move_up_at_a_thousand() {
        let auto = |n| size_auto_with(n, ".");
        assert_eq!(auto(40_940), "39.9 KB", "39.98 KB is not rounded up");
        assert_eq!(auto(1_023_999), "999 KB");
        assert_eq!(auto(1_024_000), "0.97 MB", "1000 KB is four digits");
        assert_eq!(auto(u64::MAX), "15.9 EB");
    }

    /// Explorer's own formatter, in the same user locale.
    fn explorer(n: u64) -> String {
        let mut buf = [0u16; 64];
        // SAFETY: the buffer and its length.
        unsafe {
            windows_sys::Win32::UI::Shell::StrFormatByteSizeW(
                n as i64,
                buf.as_mut_ptr(),
                buf.len() as u32,
            )
        };
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..len])
    }

    #[test]
    fn auto_sizes_match_explorer() {
        let mut n: u64 = 2;
        while n < i64::MAX as u64 / 3 {
            for v in [n, n + 1, n / 1024 * 1000, n.saturating_sub(1)] {
                if v > 1 {
                    assert_eq!(size_auto(v), explorer(v), "{v} bytes");
                }
            }
            n = n * 3 + 7;
        }
    }

    #[test]
    fn short_sizes() {
        assert_eq!(size_short(512), "512 bytes");
        assert_eq!(size_short(1536), "1.5 KB");
        assert_eq!(size_short(215 * 1024 * 1024), "215 MB");
    }
}
