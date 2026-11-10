//! The preview's text view: the start of a file read as text, for files no
//! preview handler shows. A file is text when its first bytes decode as
//! UTF-8, UTF-16 (with a byte order mark, or plainly ASCII in UTF-16) or the
//! system's ANSI code page without the control characters binary files are
//! full of.

/// How much of a file the text view reads and shows.
pub const LIMIT: usize = 64 * 1024;

/// The encoding the bytes were read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// The system's ANSI code page.
    Ansi,
}

/// What a file's start says it is: its encoding, or binary (`None`). Empty
/// is not text: there is nothing to show. `truncated`: the file goes on
/// past `bytes`, so a character may be cut off at the end.
pub fn sniff(bytes: &[u8], truncated: bool) -> Option<Encoding> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Some(Encoding::Utf8);
    }
    // UTF-32 LE's mark starts with UTF-16 LE's; it is not shown.
    if bytes.starts_with(&[0xff, 0xfe, 0, 0]) {
        return None;
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        return Some(Encoding::Utf16Le);
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        return Some(Encoding::Utf16Be);
    }
    if bytes.contains(&0) {
        return utf16_without_mark(bytes);
    }
    if utf8_prefix(bytes, truncated) {
        Some(Encoding::Utf8)
    } else {
        Some(Encoding::Ansi)
    }
}

/// UTF-16 without a byte order mark: mostly ASCII text, so one byte of
/// nearly every pair is zero, always on the same side.
fn utf16_without_mark(bytes: &[u8]) -> Option<Encoding> {
    let pairs = bytes.len() / 2;
    if pairs < 2 {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for &[lo, hi] in bytes.as_chunks::<2>().0 {
        even += usize::from(lo == 0);
        odd += usize::from(hi == 0);
    }
    // Some letters past ASCII leave their high byte non-zero; none of the
    // low bytes of ASCII is zero.
    if odd * 10 >= pairs * 6 && even == 0 {
        Some(Encoding::Utf16Le)
    } else if even * 10 >= pairs * 6 && odd == 0 {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

/// Whether `bytes` are UTF-8, allowing a character cut off at the end when
/// the read stopped in it (`truncated`); in a whole file that is a byte of
/// another code page.
fn utf8_prefix(bytes: &[u8], truncated: bool) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        // `error_len` is None when the input merely ended inside a
        // character.
        Err(e) => truncated && e.error_len().is_none(),
    }
}

/// The text of `bytes` in `encoding`, as UTF-16, without a byte order
/// mark; `ansi` decodes the system's code page. A character cut off at the
/// end is left out.
pub fn decode(bytes: &[u8], encoding: Encoding, ansi: impl Fn(&[u8]) -> Vec<u16>) -> Vec<u16> {
    match encoding {
        Encoding::Utf8 => {
            let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
            let end = match std::str::from_utf8(body) {
                Ok(_) => body.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => body.len(),
            };
            String::from_utf8_lossy(&body[..end])
                .encode_utf16()
                .collect()
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let body = bytes
                .strip_prefix(&[0xff, 0xfe])
                .or_else(|| bytes.strip_prefix(&[0xfe, 0xff]))
                .unwrap_or(bytes);
            let mut units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&p| {
                    if encoding == Encoding::Utf16Le {
                        u16::from_le_bytes(p)
                    } else {
                        u16::from_be_bytes(p)
                    }
                })
                .collect();
            // Half a surrogate pair at the end: cut off by the read.
            if units.last().is_some_and(|u| (0xd800..0xdc00).contains(u)) {
                units.pop();
            }
            units
        }
        Encoding::Ansi => ansi(bytes),
    }
}

/// Whether decoded text looks like text: no NUL, and hardly any of the
/// other control characters (tab, line ends, form feed, escape and the old
/// end-of-file mark aside).
pub fn looks_like_text(units: &[u16]) -> bool {
    if units.is_empty() || units.contains(&0) {
        return false;
    }
    let odd = units
        .iter()
        .filter(|&&u| u < 0x20 && !matches!(u, 0x09 | 0x0a | 0x0b | 0x0c | 0x0d | 0x1a | 0x1b))
        .count();
    odd * 100 <= units.len()
}

/// The text a multi-line text box shows for `units`: every line end
/// (`\n`, `\r` or `\r\n`) as `\r\n`, as the box needs them. A `truncated`
/// text (the file goes on) ends at its last whole line, if one ends in its
/// last quarter, and then a line with an ellipsis.
pub fn for_text_box(units: &[u16], truncated: bool) -> Vec<u16> {
    const CR: u16 = b'\r' as u16;
    const LF: u16 = b'\n' as u16;
    let mut body = units;
    if truncated
        && let Some(last) = units.iter().rposition(|&u| u == LF || u == CR)
        && last * 4 >= units.len() * 3
    {
        body = &units[..last];
    }
    let mut out = Vec::with_capacity(body.len() + body.len() / 16 + 4);
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            CR => {
                out.extend([CR, LF]);
                if body.get(i + 1) == Some(&LF) {
                    i += 1;
                }
            }
            LF => out.extend([CR, LF]),
            u => out.push(u),
        }
        i += 1;
    }
    if truncated {
        out.extend([CR, LF, 0x2026]);
    }
    out
}

/// The text view of a file whose first bytes are `bytes` (at most
/// [`LIMIT`]; `truncated` when the file is longer), or `None` when it is
/// not text.
pub fn text_view(
    bytes: &[u8],
    truncated: bool,
    ansi: impl Fn(&[u8]) -> Vec<u16>,
) -> Option<Vec<u16>> {
    let encoding = sniff(bytes, truncated)?;
    let units = decode(bytes, encoding, ansi);
    looks_like_text(&units).then(|| for_text_box(&units, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin1(bytes: &[u8]) -> Vec<u16> {
        bytes.iter().map(|&b| u16::from(b)).collect()
    }

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn view(bytes: &[u8]) -> Option<String> {
        text_view(bytes, false, latin1).map(|u| String::from_utf16_lossy(&u))
    }

    #[test]
    fn byte_order_marks_name_the_encoding() {
        assert_eq!(sniff(b"\xef\xbb\xbfhi", false), Some(Encoding::Utf8));
        assert_eq!(sniff(b"\xff\xfeh\0i\0", false), Some(Encoding::Utf16Le));
        assert_eq!(sniff(b"\xfe\xff\0h\0i", false), Some(Encoding::Utf16Be));
        // UTF-32 is not shown.
        assert_eq!(sniff(b"\xff\xfe\0\0h\0\0\0", false), None);
        assert_eq!(view(b"\xef\xbb\xbfcaf\xc3\xa9").as_deref(), Some("café"));
        assert_eq!(view(b"\xff\xfeh\0\xe9\0").as_deref(), Some("hé"));
        assert_eq!(view(b"\xfe\xff\0h\0\xe9").as_deref(), Some("hé"));
        assert_eq!(view(b""), None);
    }

    #[test]
    fn utf16_without_a_mark_is_found() {
        let le: Vec<u8> = utf16("hello\r\nworld")
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(sniff(&le, false), Some(Encoding::Utf16Le));
        assert_eq!(view(&le).as_deref(), Some("hello\r\nworld"));
        let be: Vec<u8> = utf16("hello")
            .iter()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        assert_eq!(sniff(&be, false), Some(Encoding::Utf16Be));
        assert_eq!(view(&be).as_deref(), Some("hello"));
    }

    #[test]
    fn binary_files_are_not_text() {
        // A PNG's signature and header.
        assert_eq!(
            view(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x01\0\0\0\x01\0\x08\x06\0\0\0"),
            None
        );
        // An executable's start.
        assert_eq!(view(b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff\0\0"), None);
        // Control characters past one in a hundred.
        let mut noisy = vec![b'a'; 98];
        noisy.extend([1, 2]);
        assert_eq!(view(&noisy), None);
        let mut fine = vec![b'a'; 99];
        fine.push(0x07);
        assert!(view(&fine).is_some());
        // Tabs, form feeds, escapes and the old end-of-file mark are text.
        assert!(view(b"a\tb\x0cc\x1b[0m\x1a").is_some());
    }

    #[test]
    fn invalid_utf8_is_the_ansi_code_page() {
        assert_eq!(sniff(b"caf\xe9", false), Some(Encoding::Ansi));
        assert_eq!(view(b"caf\xe9").as_deref(), Some("café"));
        assert_eq!(sniff(b"caf\xe9 au lait", true), Some(Encoding::Ansi));
        // Valid UTF-8 stays UTF-8.
        assert_eq!(
            view("naïve ünïcödé ✓".as_bytes()).as_deref(),
            Some("naïve ünïcödé ✓")
        );
    }

    #[test]
    fn a_character_cut_off_by_the_read_is_left_out() {
        // "é" is c3 a9: the read stopped after c3.
        let cut = b"caf\xc3";
        assert_eq!(sniff(cut, true), Some(Encoding::Utf8));
        // A whole file ending so is not UTF-8.
        assert_eq!(sniff(cut, false), Some(Encoding::Ansi));
        assert_eq!(
            String::from_utf16_lossy(&decode(cut, Encoding::Utf8, latin1)),
            "caf"
        );
        // A four-byte character cut after three.
        let emoji = "a😀".as_bytes();
        let cut = &emoji[..emoji.len() - 1];
        assert_eq!(sniff(cut, true), Some(Encoding::Utf8));
        assert_eq!(decode(cut, Encoding::Utf8, latin1), utf16("a"));
        // UTF-16 cut inside a surrogate pair.
        let mut le: Vec<u8> = utf16("a😀").iter().flat_map(|u| u.to_le_bytes()).collect();
        le.truncate(4);
        assert_eq!(decode(&le, Encoding::Utf16Le, latin1), utf16("a"));
        // An odd byte at the end of UTF-16 goes too.
        assert_eq!(
            decode(b"\xff\xfea\0b", Encoding::Utf16Le, latin1),
            utf16("a")
        );
    }

    #[test]
    fn line_ends_become_crlf() {
        let t = for_text_box(&utf16("a\nb\r\nc\rd\n\ne"), false);
        assert_eq!(String::from_utf16_lossy(&t), "a\r\nb\r\nc\r\nd\r\n\r\ne");
        assert_eq!(for_text_box(&utf16("\r"), false), utf16("\r\n"));
        assert_eq!(for_text_box(&utf16(""), false), Vec::<u16>::new());
    }

    #[test]
    fn a_long_file_ends_at_a_whole_line_and_an_ellipsis() {
        let t = for_text_box(&utf16("one\ntwo\nthree\nfou"), true);
        assert_eq!(String::from_utf16_lossy(&t), "one\r\ntwo\r\nthree\r\n…");
        // No line end in the last quarter: cut where the read stopped.
        let long_line = "x".repeat(100);
        let t = for_text_box(&utf16(&format!("a\n{long_line}")), true);
        assert_eq!(
            String::from_utf16_lossy(&t),
            format!("a\r\n{long_line}\r\n…")
        );
    }
}
