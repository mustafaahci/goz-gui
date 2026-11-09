//! goz-gui's settings file, an INI like Everything's: `key=value` lines in
//! UTF-8 under a `[goz-gui]` section, CRLF between lines.
//!
//! The document keeps every line it read, so a file written back changes
//! only the keys set: unknown keys, comments, other sections and the order
//! of everything stay as they were. Keys before the first section header
//! count as goz-gui's too (a hand-written file with no header). When a key
//! is there more than once the last one wins, as Windows' own INI reader
//! does; setting it leaves one line, where the first one was.

/// The section goz-gui's keys live in.
pub const SECTION: &str = "goz-gui";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    /// `[name]`, as written.
    Header { name: String, raw: String },
    /// `key=value`; `raw` is the line as read, `None` once the value is set.
    Pair {
        key: String,
        value: String,
        raw: Option<String>,
    },
    /// A comment, a blank line, or anything else, kept as it was.
    Other(String),
}

/// An INI file's lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ini {
    lines: Vec<Line>,
}

/// The file's text, or why it is not text an INI can be: bytes that are not
/// UTF-8 (or UTF-16 with its byte order mark), or NULs, which is what a file
/// cut short by a power failure holds.
pub fn decode(bytes: &[u8]) -> Result<String, String> {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if rest.len() % 2 != 0 {
            return Err("it is UTF-16 with an odd number of bytes".into());
        }
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        String::from_utf16(&units).map_err(|_| "it is not valid UTF-16".to_owned())?
    } else {
        let rest = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        String::from_utf8(rest.to_vec()).map_err(|_| "it is not valid UTF-8".to_owned())?
    };
    if text.contains('\0') {
        return Err("it holds NUL characters".into());
    }
    Ok(text)
}

fn is_ours(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(SECTION)
}

impl Ini {
    /// Reads a file's bytes; an error says why they are not an INI.
    pub fn parse(bytes: &[u8]) -> Result<Ini, String> {
        Ok(Ini::from_text(&decode(bytes)?))
    }

    /// Reads a file's text. Every line is kept; nothing is refused.
    pub fn from_text(text: &str) -> Ini {
        let mut lines = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
            let line = &rest[..end];
            rest = &rest[end..];
            if let Some(after) = rest.strip_prefix("\r\n") {
                rest = after;
            } else if !rest.is_empty() {
                rest = &rest[1..];
            }
            lines.push(Ini::line(line));
        }
        Ini { lines }
    }

    fn line(raw: &str) -> Line {
        let trimmed = raw.trim();
        if let Some(name) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            return Line::Header {
                name: name.trim().to_owned(),
                raw: raw.to_owned(),
            };
        }
        if trimmed.starts_with([';', '#']) {
            return Line::Other(raw.to_owned());
        }
        match raw.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() => Line::Pair {
                key: key.trim().to_owned(),
                value: value.to_owned(),
                raw: Some(raw.to_owned()),
            },
            _ => Line::Other(raw.to_owned()),
        }
    }

    /// Whether each line is in goz-gui's section: before any header, or
    /// under a `[goz-gui]` one.
    fn ours(&self) -> Vec<bool> {
        let mut inside = true;
        self.lines
            .iter()
            .map(|line| {
                if let Line::Header { name, .. } = line {
                    inside = is_ours(name);
                    false
                } else {
                    inside
                }
            })
            .collect()
    }

    /// The indices of goz-gui's lines holding `key` (any case), in order.
    fn find(&self, key: &str) -> Vec<usize> {
        let ours = self.ours();
        self.lines
            .iter()
            .enumerate()
            .filter(|&(i, line)| {
                ours[i] && matches!(line, Line::Pair { key: k, .. } if k.eq_ignore_ascii_case(key))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The value of `key` in goz-gui's section; the last one when it is
    /// there more than once.
    pub fn get(&self, key: &str) -> Option<&str> {
        let i = *self.find(key).last()?;
        match &self.lines[i] {
            Line::Pair { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Sets `key` in goz-gui's section: in place, where it was first, or
    /// after the section's last key. A value equal to the one there leaves
    /// the line as it was. A line break in `value` (a search of several
    /// lines) becomes a space: a value is one line, and a break would make
    /// the rest of it a line of its own (another key, another section).
    pub fn set(&mut self, key: &str, value: &str) {
        let one_line;
        let value = if value.contains(['\r', '\n']) {
            one_line = value.replace("\r\n", " ").replace(['\r', '\n'], " ");
            one_line.as_str()
        } else {
            value
        };
        let found = self.find(key);
        if let Some((&first, later)) = found.split_first() {
            for &i in later.iter().rev() {
                self.lines.remove(i);
            }
            if let Line::Pair {
                key: k,
                value: v,
                raw,
            } = &mut self.lines[first]
                && (!later.is_empty() || v != value)
            {
                *k = key.to_owned();
                *v = value.to_owned();
                *raw = None;
            }
            return;
        }
        let pair = Line::Pair {
            key: key.to_owned(),
            value: value.to_owned(),
            raw: None,
        };
        let at = self.insertion_point();
        match at {
            Some(i) => self.lines.insert(i, pair),
            None => {
                self.lines.push(Line::Header {
                    name: SECTION.to_owned(),
                    raw: format!("[{SECTION}]"),
                });
                self.lines.push(pair);
            }
        }
    }

    /// Where a new key goes: after the last key or comment of the last
    /// `[goz-gui]` section (before the blank lines that end it), or at the
    /// end of a file with no headers at all. `None`: a header is needed.
    fn insertion_point(&self) -> Option<usize> {
        let ours = self.ours();
        let headers = self.lines.iter().any(|l| matches!(l, Line::Header { .. }));
        let last_header = self
            .lines
            .iter()
            .rposition(|l| matches!(l, Line::Header { name, .. } if is_ours(name)));
        let pairs = self.lines.iter().any(|l| matches!(l, Line::Pair { .. }));
        let start = match last_header {
            Some(h) => h + 1,
            // Other sections only, or no keys at all: a header first.
            None if headers || !pairs => return None,
            None => 0,
        };
        let mut end = start;
        while end < self.lines.len() && ours[end] {
            end += 1;
        }
        // Back over the blank lines between this section and the next.
        while end > start && matches!(&self.lines[end - 1], Line::Other(s) if s.trim().is_empty()) {
            end -= 1;
        }
        Some(end)
    }

    /// Sets each key that goz-gui's section does not have yet.
    pub fn fill_missing<'a>(&mut self, pairs: impl IntoIterator<Item = (&'a str, &'a str)>) {
        for (key, value) in pairs {
            if self.get(key).is_none() {
                self.set(key, value);
            }
        }
    }

    /// The file's text: each line as read, or as set, ending in CRLF.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                Line::Header { raw, .. } | Line::Other(raw) => out.push_str(raw),
                Line::Pair { raw: Some(raw), .. } => out.push_str(raw),
                Line::Pair {
                    key,
                    value,
                    raw: None,
                } => {
                    out.push_str(key);
                    out.push('=');
                    out.push_str(value);
                }
            }
            out.push_str("\r\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keys_with_or_without_bom_and_any_line_ends() {
        for bom in [&b""[..], &[0xef, 0xbb, 0xbf][..]] {
            let mut bytes = bom.to_vec();
            bytes.extend_from_slice(b"[goz-gui]\r\ntheme=2\nontop=1\rsearch=a=b\r\n");
            let ini = Ini::parse(&bytes).expect("an INI");
            assert_eq!(ini.get("theme"), Some("2"));
            assert_eq!(ini.get("ontop"), Some("1"));
            // Only the first `=` splits; the value keeps the rest.
            assert_eq!(ini.get("search"), Some("a=b"));
            assert_eq!(ini.get("THEME"), Some("2"), "keys match in any case");
            assert_eq!(ini.get("missing"), None);
        }
    }

    #[test]
    fn utf16_with_its_mark_is_read_too() {
        let mut bytes = vec![0xff, 0xfe];
        for unit in "[goz-gui]\r\nsearch=ünï\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(
            Ini::parse(&bytes).expect("UTF-16").get("search"),
            Some("ünï")
        );
    }

    #[test]
    fn what_is_not_an_ini_is_refused() {
        assert!(Ini::parse(&[0u8; 64]).is_err(), "a file of NULs");
        assert!(Ini::parse(b"[goz-gui]\r\ntheme=\xff\xfe1\r\n").is_err());
        assert!(Ini::parse(&[0xff, 0xfe, 0x41]).is_err());
        // Empty is fine: no settings.
        assert_eq!(Ini::parse(b"").expect("empty"), Ini::default());
    }

    #[test]
    fn the_last_duplicate_wins_and_setting_leaves_one() {
        let mut ini = Ini::from_text("[goz-gui]\r\nsort=Name\r\nx=1\r\nsort=Size\r\n");
        assert_eq!(ini.get("sort"), Some("Size"));
        ini.set("sort", "Path");
        assert_eq!(ini.to_text(), "[goz-gui]\r\nsort=Path\r\nx=1\r\n");
        // Setting a duplicate to the value it reads as also leaves one.
        let mut ini = Ini::from_text("[goz-gui]\r\nsort=Name\r\nsort=Size\r\n");
        ini.set("sort", "Size");
        assert_eq!(ini.to_text(), "[goz-gui]\r\nsort=Size\r\n");
    }

    #[test]
    fn unknown_lines_and_other_sections_are_kept_in_order() {
        let text = "; a comment\r\n\
                    [goz-gui]\r\n\
                    future_key = kept as written\r\n\
                    theme=0\r\n\
                    not a pair\r\n\
                    \r\n\
                    [Other]\r\n\
                    theme=9\r\n";
        let mut ini = Ini::from_text(text);
        assert_eq!(ini.to_text(), text, "read and written back unchanged");
        assert_eq!(
            ini.get("theme"),
            Some("0"),
            "other sections' keys are not ours"
        );
        ini.set("theme", "2");
        ini.set("ontop", "1");
        assert_eq!(
            ini.to_text(),
            "; a comment\r\n\
             [goz-gui]\r\n\
             future_key = kept as written\r\n\
             theme=2\r\n\
             not a pair\r\n\
             ontop=1\r\n\
             \r\n\
             [Other]\r\n\
             theme=9\r\n"
        );
        assert_eq!(ini.get("future_key"), Some(" kept as written"));
    }

    #[test]
    fn a_new_file_gets_the_section_and_a_headerless_one_keeps_none() {
        let mut ini = Ini::default();
        ini.set("theme", "1");
        assert_eq!(ini.to_text(), "[goz-gui]\r\ntheme=1\r\n");
        // A hand-written file with no header: its keys are goz-gui's.
        let mut ini = Ini::from_text("app_data=0\r\n");
        assert_eq!(ini.get("app_data"), Some("0"));
        ini.set("theme", "2");
        assert_eq!(ini.to_text(), "app_data=0\r\ntheme=2\r\n");
        // Only other sections: goz-gui's is added after them.
        let mut ini = Ini::from_text("[Everything]\r\napp_data=0\r\n");
        assert_eq!(ini.get("app_data"), None);
        ini.set("app_data", "0");
        assert_eq!(
            ini.to_text(),
            "[Everything]\r\napp_data=0\r\n[goz-gui]\r\napp_data=0\r\n"
        );
    }

    #[test]
    fn a_value_with_line_breaks_stays_one_line() {
        let mut ini = Ini::from_text("[goz-gui]\r\ntheme=0\r\nsearch=\r\n");
        ini.set("search", "c:\\windows\\*.exe\r\ntheme=2\n[elsewhere]\rx");
        let text = ini.to_text();
        assert_eq!(
            text,
            "[goz-gui]\r\ntheme=0\r\nsearch=c:\\windows\\*.exe theme=2 [elsewhere] x\r\n"
        );
        let back = Ini::from_text(&text);
        assert_eq!(back.get("theme"), Some("0"));
        assert_eq!(
            back.get("search"),
            Some("c:\\windows\\*.exe theme=2 [elsewhere] x")
        );
    }

    #[test]
    fn missing_keys_are_filled_and_present_ones_left() {
        let mut ini = Ini::from_text("[goz-gui]\r\ntheme=2\r\n");
        ini.fill_missing([("theme", "0"), ("ontop", "0")]);
        assert_eq!(ini.to_text(), "[goz-gui]\r\ntheme=2\r\nontop=0\r\n");
    }
}
