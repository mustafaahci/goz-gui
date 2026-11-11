//! Reading a file list as it streams in: Everything's EFU (a UTF-8 CSV
//! whose header names its columns, `Filename,Size,Date Modified,Date
//! Created,Attributes`), any CSV with a `Filename` column, or a plain list
//! of paths, one a line (a `.txt` file list). A record is read into
//! buffers kept from one to the next, so a list of millions of rows costs
//! no allocation a row.

use std::io::{self, BufRead};

/// One CSV record at a time from `reader`, quoted fields (with `""` for a
/// quote, and line breaks inside quotes) as Everything writes them. In
/// line mode each line is one field, as it is: a plain list of paths,
/// where a comma is part of a name.
pub struct Records<R> {
    reader: R,
    line: Vec<u8>,
    fields: Vec<Vec<u8>>,
    used: usize,
    /// The record's own bytes, without its line break: what line mode
    /// takes.
    raw: Vec<u8>,
    lines_only: bool,
    first: bool,
}

impl<R: BufRead> Records<R> {
    pub fn new(reader: R) -> Records<R> {
        Records {
            reader,
            line: Vec::new(),
            fields: Vec::new(),
            used: 0,
            raw: Vec::new(),
            lines_only: false,
            first: true,
        }
    }

    /// From now on each line is one field, quotes and commas included.
    pub fn lines_only(&mut self) {
        self.lines_only = true;
    }

    /// The fields of the record read last.
    pub fn fields(&self) -> impl Iterator<Item = &[u8]> {
        self.fields[..self.used].iter().map(Vec::as_slice)
    }

    /// Field `i` of the record read last; empty when it has none.
    pub fn field(&self, i: usize) -> &[u8] {
        if i < self.used { &self.fields[i] } else { &[] }
    }

    /// The record read last as it was in the file.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    fn next_field(&mut self) {
        if self.used == self.fields.len() {
            self.fields.push(Vec::new());
        }
        self.fields[self.used].clear();
        self.used += 1;
    }

    fn push(&mut self, b: u8) {
        self.fields[self.used - 1].push(b);
    }

    /// Reads the next record; false at the end of the file.
    pub fn read(&mut self) -> io::Result<bool> {
        self.used = 0;
        self.raw.clear();
        self.next_field();
        let mut quoted = false;
        // Whether the field so far is only its opening quote's content: a
        // quote that does not start a field is text.
        let mut at_start = true;
        loop {
            self.line.clear();
            if self.reader.read_until(b'\n', &mut self.line)? == 0 {
                // The end: a record without a line break at the end of the
                // file, or none.
                return Ok(!self.raw.is_empty() || quoted);
            }
            let mut line = std::mem::take(&mut self.line);
            if self.first {
                self.first = false;
                if line.starts_with(&[0xEF, 0xBB, 0xBF]) {
                    line.drain(..3);
                }
            }
            let mut end = line.len();
            if line.last() == Some(&b'\n') {
                end -= 1;
            }
            if self.lines_only {
                if end > 0 && line[end - 1] == b'\r' {
                    end -= 1;
                }
                self.raw.extend_from_slice(&line[..end]);
                self.fields[0].extend_from_slice(&line[..end]);
                self.line = line;
                return Ok(true);
            }
            if !self.raw.is_empty() {
                // A line break inside quotes.
                self.raw.push(b'\n');
            }
            self.raw.extend_from_slice(&line[..end]);
            let mut i = 0;
            while i < end {
                let b = line[i];
                if quoted {
                    if b == b'"' {
                        if line.get(i + 1) == Some(&b'"') {
                            self.push(b'"');
                            i += 1;
                        } else {
                            quoted = false;
                        }
                    } else {
                        self.push(b);
                    }
                } else {
                    match b {
                        b',' => {
                            self.next_field();
                            at_start = true;
                            i += 1;
                            continue;
                        }
                        b'"' if at_start => quoted = true,
                        b'\r' if i + 1 == end => {}
                        _ => self.push(b),
                    }
                }
                at_start = false;
                i += 1;
            }
            if quoted {
                self.push(b'\n');
                self.line = line;
                continue;
            }
            if self.raw.last() == Some(&b'\r') {
                self.raw.pop();
            }
            self.line = line;
            return Ok(true);
        }
    }
}

/// Where each of a list's columns is, by its name in the header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Columns {
    pub filename: usize,
    pub size: Option<usize>,
    pub modified: Option<usize>,
    pub created: Option<usize>,
    pub attributes: Option<usize>,
}

impl Columns {
    /// The columns a header names; `None` when it has no `Filename`
    /// column (then it is no header: a plain list of paths).
    pub fn of_header<'a>(names: impl Iterator<Item = &'a [u8]>) -> Option<Columns> {
        let mut c = Columns::default();
        let mut filename = None;
        for (i, name) in names.enumerate() {
            let name = String::from_utf8_lossy(name);
            let name = name.trim().to_ascii_lowercase();
            match name.as_str() {
                "filename" | "file name" if filename.is_none() => filename = Some(i),
                "size" => c.size = c.size.or(Some(i)),
                "date modified" => c.modified = c.modified.or(Some(i)),
                "date created" => c.created = c.created.or(Some(i)),
                "attributes" => c.attributes = c.attributes.or(Some(i)),
                _ => {}
            }
        }
        c.filename = filename?;
        Some(c)
    }
}

/// `FILE_ATTRIBUTE_DIRECTORY`.
const DIRECTORY: u32 = 0x10;

/// A size field: a number of bytes, or nothing.
pub fn size(field: &[u8]) -> Option<u64> {
    std::str::from_utf8(field).ok()?.trim().parse().ok()
}

/// An attributes field: Everything's decimal number (or `0x` hex).
pub fn attributes(field: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(field).ok()?.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Whether attributes say a folder.
pub fn is_folder_attribute(attributes: Option<u32>) -> bool {
    attributes.is_some_and(|a| a & DIRECTORY != 0)
}

/// A date field: a FILETIME (Everything's `efu_filetime=0`), or an ISO
/// 8601 date and time (its other choices), in UTC with `Z` or an offset,
/// else local time, which `local_to_utc` turns into UTC.
pub fn date(field: &[u8], local_to_utc: &dyn Fn(i64) -> Option<i64>) -> Option<i64> {
    let text = std::str::from_utf8(field).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(ft) = text.parse::<i64>() {
        return (ft > 0).then_some(ft);
    }
    iso_8601(text, local_to_utc)
}

/// Ticks of 100 ns a second, and a day.
const SECOND: i64 = 10_000_000;
const DAY: i64 = 86_400 * SECOND;
/// Days from 1601-01-01 (FILETIME's start) to 1970-01-01.
const DAYS_1601_TO_1970: i64 = 134_774;

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2024-05-17T09:30:00`, with an optional fraction of a second, a space
/// for the `T`, and `Z` or `+hh:mm` / `-hh:mm` after; seconds may be left
/// out.
fn iso_8601(text: &str, local_to_utc: &dyn Fn(i64) -> Option<i64>) -> Option<i64> {
    let b = text.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = b.get(from..from + len)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(part).ok()?.parse().ok())
            .flatten()
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') || !(1..=12).contains(&mo) {
        return None;
    }
    if !(1..=31).contains(&d) {
        return None;
    }
    let mut ticks = (days_from_civil(y, mo, d) + DAYS_1601_TO_1970) * DAY;
    let mut i = 10;
    if matches!(b.get(i), Some(b'T' | b't' | b' ')) {
        let (h, mi) = (num(i + 1, 2)?, num(i + 4, 2)?);
        if b.get(i + 3) != Some(&b':') || h > 23 || mi > 59 {
            return None;
        }
        ticks += (h * 3600 + mi * 60) * SECOND;
        i += 6;
        if b.get(i) == Some(&b':') {
            let s = num(i + 1, 2)?;
            if s > 60 {
                return None;
            }
            ticks += s * SECOND;
            i += 3;
            if matches!(b.get(i), Some(b'.' | b',')) {
                i += 1;
                let mut scale = SECOND / 10;
                while let Some(digit) = b.get(i).filter(|c| c.is_ascii_digit()) {
                    ticks += i64::from(digit - b'0') * scale;
                    scale /= 10;
                    i += 1;
                }
            }
        }
    }
    match b.get(i) {
        None => local_to_utc(ticks),
        Some(b'Z' | b'z') if i + 1 == b.len() => Some(ticks),
        Some(&sign @ (b'+' | b'-')) => {
            let h = num(i + 1, 2)?;
            let m = match b.get(i + 3) {
                Some(b':') => num(i + 4, 2)?,
                None => 0,
                _ => num(i + 3, 2)?,
            };
            let offset = (h * 60 + m) * 60 * SECOND;
            Some(if sign == b'+' {
                ticks - offset
            } else {
                ticks + offset
            })
        }
        _ => None,
    }
}

/// A list's path as goz-gui shows it: `/` as `\`, a folder's trailing
/// separator taken off (but a drive's root keeps it), and a relative path
/// put under `base`, the list's own folder. The second value is whether
/// the path ended with a separator (Everything's mark of a folder).
pub fn path(field: &[u8], base: &str) -> (String, bool) {
    let text = String::from_utf8_lossy(field);
    let mut p: String = text.trim_matches(['\r', '\n']).replace('/', "\\");
    let marked = p.len() > 1 && p.ends_with('\\');
    while p.len() > 3 && p.ends_with('\\') && !p.ends_with(":\\") {
        p.pop();
    }
    if p.len() == 2 && p.ends_with(':') {
        p.push('\\');
    }
    let absolute = p.starts_with("\\\\")
        || (p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic());
    if !absolute && !p.is_empty() && !base.is_empty() {
        let rel = p.trim_start_matches('\\');
        let mut joined = base.trim_end_matches('\\').to_owned();
        joined.push('\\');
        joined.push_str(rel);
        p = joined;
    }
    (p, marked)
}

/// A plain list's line as a path: surrounding quotes taken off.
pub fn plain_line(raw: &[u8]) -> &[u8] {
    let t = raw.trim_ascii();
    if t.len() >= 2 && t.starts_with(b"\"") && t.ends_with(b"\"") {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(text: &str) -> Vec<Vec<String>> {
        let mut r = Records::new(text.as_bytes());
        let mut out = Vec::new();
        while r.read().unwrap() {
            out.push(
                r.fields()
                    .map(|f| String::from_utf8_lossy(f).into_owned())
                    .collect(),
            );
        }
        out
    }

    #[test]
    fn csv_records_as_everything_writes_them() {
        let text = "\u{feff}Filename,Size,Date Modified,Date Created,Attributes\r\n\
                    \"C:\\a \"\"b\"\".txt\",12,133000000000000000,,32\r\n\
                    \"C:\\x,y\\\",,,,16\r\n\
                    \"two\nlines\",1\r\n\
                    last,2";
        let r = records(text);
        assert_eq!(r.len(), 5);
        assert_eq!(
            r[0],
            [
                "Filename",
                "Size",
                "Date Modified",
                "Date Created",
                "Attributes"
            ]
        );
        assert_eq!(
            r[1],
            [r#"C:\a "b".txt"#, "12", "133000000000000000", "", "32"]
        );
        assert_eq!(r[2], [r"C:\x,y\", "", "", "", "16"]);
        assert_eq!(r[3], ["two\nlines", "1"]);
        assert_eq!(r[4], ["last", "2"]);
        let c = Columns::of_header(
            [
                "Filename",
                "Size",
                "Date Modified",
                "Date Created",
                "Attributes",
            ]
            .iter()
            .map(|s| s.as_bytes()),
        )
        .unwrap();
        assert_eq!(
            (c.filename, c.size, c.modified, c.created, c.attributes),
            (0, Some(1), Some(2), Some(3), Some(4))
        );
        assert!(Columns::of_header(["C:\\a.txt"].iter().map(|s| s.as_bytes())).is_none());
    }

    #[test]
    fn plain_lists_keep_their_commas() {
        let mut r = Records::new("C:\\a,b.txt\r\n\"C:\\q.txt\"\nC:\\z".as_bytes());
        r.lines_only();
        let mut lines = Vec::new();
        while r.read().unwrap() {
            lines.push(String::from_utf8_lossy(plain_line(r.raw())).into_owned());
        }
        assert_eq!(lines, [r"C:\a,b.txt", r"C:\q.txt", r"C:\z"]);
    }

    #[test]
    fn values() {
        let utc = |t: i64| Some(t);
        assert_eq!(size(b"289792"), Some(289_792));
        assert_eq!(size(b""), None);
        assert_eq!(attributes(b"16"), Some(16));
        assert_eq!(attributes(b"0x20"), Some(32));
        assert!(is_folder_attribute(Some(0x30)));
        assert!(!is_folder_attribute(Some(0x20)) && !is_folder_attribute(None));
        assert_eq!(
            date(b"133000000000000000", &utc),
            Some(133_000_000_000_000_000)
        );
        assert_eq!(date(b"0", &utc), None);
        // 1970-01-01 is FILETIME's Unix epoch.
        assert_eq!(
            date(b"1970-01-01T00:00:00Z", &utc),
            Some(116_444_736_000_000_000)
        );
        assert_eq!(
            date(b"2024-05-17T09:30:00.5+02:00", &utc),
            date(b"2024-05-17 07:30:00.5Z", &utc)
        );
        // Local time goes through the time zone.
        let east = |t: i64| Some(t - 3600 * SECOND);
        assert_eq!(
            date(b"2024-05-17T10:30", &east),
            date(b"2024-05-17T09:30:00Z", &utc)
        );
        assert_eq!(date(b"2024-13-01", &utc), None);
        assert_eq!(date(b"yesterday", &utc), None);
    }

    #[test]
    fn paths() {
        assert_eq!(path(br"C:\Windows\", ""), (r"C:\Windows".into(), true));
        assert_eq!(path(br"C:\", ""), (r"C:\".into(), true));
        assert_eq!(path(b"C:", ""), (r"C:\".into(), false));
        assert_eq!(path(b"C:/a/b.txt", ""), (r"C:\a\b.txt".into(), false));
        assert_eq!(
            path(br"\\server\share\x\", ""),
            (r"\\server\share\x".into(), true)
        );
        assert_eq!(
            path(br"sub\a.txt", r"D:\lists\"),
            (r"D:\lists\sub\a.txt".into(), false)
        );
    }
}
