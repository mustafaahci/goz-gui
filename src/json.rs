//! A small JSON reader and writer for `Session.json`.
//!
//! Strings are kept as UTF-16 code units, so a file name holding a lone
//! surrogate (NTFS allows it) survives a round trip as a `\uD800` escape.
//! Objects keep their keys in order; a key given twice reads as its last
//! value, as the INI's do.
//!
//! The writer lays the text out as Everything 1.5 writes its own
//! `Session.json`: tabs, CRLF, `"key":value` with no spaces, arrays of plain
//! values on one line, arrays of objects one element per line, and a final
//! CRLF. An object holding only plain values inside such an array goes on
//! one line.

use std::fmt::Write;

/// How deep arrays and objects may nest: far more than any session, and
/// little enough that a hostile file cannot exhaust the stack.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(Vec<u16>),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn str(text: &str) -> Json {
        Json::Str(text.encode_utf16().collect())
    }

    pub fn int(n: i64) -> Json {
        Json::Num(n as f64)
    }

    pub fn bit(on: bool) -> Json {
        Json::int(i64::from(on))
    }

    /// The value of `key` in an object (its last one); `None` for anything
    /// else.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().rev().find(|(k, _)| k == key).map(|m| &m.1),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// A whole number in `i64`'s range (`true` and `false` as 1 and 0, as a
    /// hand-edited file may have them).
    pub fn as_int(&self) -> Option<i64> {
        match *self {
            Json::Num(n) if n.fract() == 0.0 && n.abs() < 9.0e18 => Some(n as i64),
            Json::Bool(b) => Some(i64::from(b)),
            _ => None,
        }
    }

    /// A flag written as 0 or 1 (or `true`/`false`): any other number is on.
    pub fn as_flag(&self) -> Option<bool> {
        self.as_int().map(|n| n != 0)
    }

    pub fn as_wide(&self) -> Option<&[u16]> {
        match self {
            Json::Str(units) => Some(units),
            _ => None,
        }
    }

    /// A string as text; a lone surrogate becomes U+FFFD.
    pub fn as_text(&self) -> Option<String> {
        self.as_wide().map(String::from_utf16_lossy)
    }

    /// Plain: neither an array nor an object.
    fn is_plain(&self) -> bool {
        !matches!(self, Json::Array(_) | Json::Object(_))
    }
}

/// Reads a JSON document. UTF-8, with or without a byte order mark; one
/// value, with nothing but white space after it.
pub fn parse(bytes: &[u8]) -> Result<Json, String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())?;
    let mut p = Parser {
        s: text.as_bytes(),
        at: 0,
        text,
    };
    p.space();
    let value = p.value(0)?;
    p.space();
    if p.at != p.s.len() {
        return Err(p.error("text after the end"));
    }
    Ok(value)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
    text: &'a str,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        let line = 1 + self.s[..self.at.min(self.s.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        format!("it is not valid JSON ({what}, line {line})")
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.s[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("an unknown word"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deep"));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("an unexpected character")),
            None => Err(self.error("it ends early")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.at += 1;
        let mut members = Vec::new();
        self.space();
        if self.eat(b'}') {
            return Ok(Json::Object(members));
        }
        loop {
            self.space();
            if self.peek() != Some(b'"') {
                return Err(self.error("a key is not a string"));
            }
            let key = String::from_utf16_lossy(&self.string()?);
            self.space();
            if !self.eat(b':') {
                return Err(self.error("no ':' after a key"));
            }
            self.space();
            let value = self.value(depth + 1)?;
            members.push((key, value));
            self.space();
            if self.eat(b',') {
                continue;
            }
            if self.eat(b'}') {
                return Ok(Json::Object(members));
            }
            return Err(self.error("no ',' or '}' in an object"));
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.eat(b']') {
            return Ok(Json::Array(items));
        }
        loop {
            self.space();
            items.push(self.value(depth + 1)?);
            self.space();
            if self.eat(b',') {
                continue;
            }
            if self.eat(b']') {
                return Ok(Json::Array(items));
            }
            return Err(self.error("no ',' or ']' in an array"));
        }
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let digits = self
            .s
            .get(self.at..self.at + 4)
            .and_then(|d| std::str::from_utf8(d).ok())
            .filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| self.error("a bad \\u escape"))?;
        self.at += 4;
        u16::from_str_radix(digits, 16).map_err(|_| self.error("a bad \\u escape"))
    }

    /// A string, as UTF-16 code units; `\uXXXX` escapes are taken as code
    /// units as they are, so pairs make their character and a lone
    /// surrogate stays one.
    fn string(&mut self) -> Result<Vec<u16>, String> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let start = self.at;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            // Whole UTF-8 characters: the run stops only at ASCII bytes.
            out.extend(self.text[start..self.at].encode_utf16());
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.error("it ends early"));
                    };
                    self.at += 1;
                    let unit = match e {
                        b'"' => u16::from(b'"'),
                        b'\\' => u16::from(b'\\'),
                        b'/' => u16::from(b'/'),
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => u16::from(b'\n'),
                        b'r' => u16::from(b'\r'),
                        b't' => u16::from(b'\t'),
                        b'u' => self.hex4()?,
                        _ => return Err(self.error("a bad escape")),
                    };
                    out.push(unit);
                }
                Some(_) => return Err(self.error("a control character in a string")),
                None => return Err(self.error("a string does not end")),
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        self.eat(b'-');
        let digits = |p: &mut Self| {
            let from = p.at;
            while matches!(p.peek(), Some(b'0'..=b'9')) {
                p.at += 1;
            }
            p.at > from
        };
        if !digits(self) {
            return Err(self.error("a bad number"));
        }
        if self.eat(b'.') && !digits(self) {
            return Err(self.error("a bad number"));
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if !digits(self) {
                return Err(self.error("a bad number"));
            }
        }
        self.text[start..self.at]
            .parse()
            .map(Json::Num)
            .map_err(|_| self.error("a bad number"))
    }
}

/// `value` on one line with no spaces, as Everything writes JSON in an INI
/// value (`rename_presets`).
pub fn to_line(value: &Json) -> String {
    let mut out = String::new();
    write_inline(&mut out, value);
    out
}

/// `value` as text, laid out as Everything lays out its session.
pub fn to_text(value: &Json) -> String {
    let mut out = String::new();
    write_block(&mut out, value, 0);
    out.push_str("\r\n");
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

/// A value that starts on a line of its own at `depth` (its first line is
/// indented by the caller).
fn write_block(out: &mut String, value: &Json, depth: usize) {
    match value {
        Json::Object(members) => {
            out.push_str("{\r\n");
            for (i, (key, v)) in members.iter().enumerate() {
                indent(out, depth + 1);
                write_string(out, &key.encode_utf16().collect::<Vec<u16>>());
                out.push(':');
                if spans_lines(v) {
                    out.push_str("\r\n");
                    indent(out, depth + 1);
                    write_block(out, v, depth + 1);
                } else {
                    write_inline(out, v);
                }
                if i + 1 < members.len() {
                    out.push(',');
                }
                out.push_str("\r\n");
            }
            indent(out, depth);
            out.push('}');
        }
        Json::Array(items) if spans_lines(value) => {
            out.push_str("[\r\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, depth + 1);
                if is_flat(item) {
                    write_inline(out, item);
                } else {
                    write_block(out, item, depth + 1);
                }
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push_str("\r\n");
            }
            indent(out, depth);
            out.push(']');
        }
        other => write_inline(out, other),
    }
}

/// Whether a member's value goes on lines of its own: an object, or an
/// array holding arrays or objects.
fn spans_lines(value: &Json) -> bool {
    match value {
        Json::Object(_) => true,
        Json::Array(items) => items.iter().any(|i| !i.is_plain()),
        _ => false,
    }
}

/// An object of plain values (one line in an array of objects), or a plain
/// value.
fn is_flat(value: &Json) -> bool {
    match value {
        Json::Object(members) => members.iter().all(|(_, v)| v.is_plain()),
        Json::Array(_) => false,
        _ => true,
    }
}

/// A value on one line.
fn write_inline(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                let _ = write!(out, "{}", *n as i64);
            } else if n.is_finite() {
                let _ = write!(out, "{n}");
            } else {
                out.push('0');
            }
        }
        Json::Str(units) => write_string(out, units),
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_inline(out, item);
            }
            out.push(']');
        }
        Json::Object(members) => {
            out.push('{');
            for (i, (key, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, &key.encode_utf16().collect::<Vec<u16>>());
                out.push(':');
                write_inline(out, v);
            }
            out.push('}');
        }
    }
}

/// A string with JSON's escapes: `"` and `\`, control characters, and lone
/// surrogates as `\uXXXX`; everything else as itself.
fn write_string(out: &mut String, units: &[u16]) {
    out.push('"');
    for unit in char::decode_utf16(units.iter().copied()) {
        match unit {
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\r') => out.push_str("\\r"),
            Ok('\t') => out.push_str("\\t"),
            Ok(c) if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            Ok(c) => out.push(c),
            Err(lone) => {
                let _ = write!(out, "\\u{:04x}", lone.unpaired_surrogate());
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(members: &[(&str, Json)]) -> Json {
        Json::Object(
            members
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn reads_every_kind_of_value() {
        let v = parse(
            br#" {"a":1,"b":-2.5e1,"c":"x\"y\\z\/\n\u0041","d":[true,false,null],"e":{},"f":[]} "#,
        )
        .expect("valid");
        assert_eq!(v.get("a").and_then(Json::as_int), Some(1));
        assert_eq!(v.get("b"), Some(&Json::Num(-25.0)));
        assert_eq!(
            v.get("c").and_then(Json::as_text).as_deref(),
            Some("x\"y\\z/\nA")
        );
        assert_eq!(
            v.get("d"),
            Some(&Json::Array(vec![
                Json::Bool(true),
                Json::Bool(false),
                Json::Null
            ]))
        );
        assert_eq!(v.get("e"), Some(&Json::Object(Vec::new())));
        assert_eq!(v.get("f").and_then(Json::as_array), Some(&[][..]));
        assert_eq!(v.get("zzz"), None);
        // A key twice: the last one.
        let v = parse(br#"{"k":1,"k":2}"#).expect("valid");
        assert_eq!(v.get("k").and_then(Json::as_int), Some(2));
        // A byte order mark is skipped; true and false read as flags.
        let v = parse(b"\xef\xbb\xbf{\"on\":true}").expect("valid");
        assert_eq!(v.get("on").and_then(Json::as_flag), Some(true));
    }

    #[test]
    fn broken_text_is_refused() {
        for bad in [
            &b""[..],
            b"{",
            b"[1,]",
            b"[1 2]",
            b"{\"a\" 1}",
            b"{a:1}",
            b"\"no end",
            b"\"bad \\q escape\"",
            b"\"raw\ttab\"",
            b"01x",
            b"-",
            b"1.",
            b"tru",
            b"[] []",
            b"\0\0\0\0",
            b"\xff\xfe[",
            b"\"\\u12\"",
        ] {
            assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        // Nesting past the limit, not a stack overflow.
        let deep = "[".repeat(10_000);
        assert!(parse(deep.as_bytes()).is_err());
        let ok = format!("{}{}", "[".repeat(40), "]".repeat(40));
        assert!(parse(ok.as_bytes()).is_ok());
    }

    #[test]
    fn strings_keep_every_code_unit() {
        // A pair is its character; a lone surrogate stays itself.
        let v = parse(br#"["\ud83d\ude00","a\udc00b","\u00e9t\u00e9","\u00c7"]"#).expect("valid");
        let items = v.as_array().expect("array");
        assert_eq!(items[0].as_text().as_deref(), Some("\u{1f600}"));
        assert_eq!(items[1].as_wide(), Some(&[0x61, 0xdc00, 0x62][..]));
        assert_eq!(items[2].as_text().as_deref(), Some("été"));
        // Raw UTF-8 too.
        let v = parse("[\"Ç:\\\\été\\\\😀\"]".as_bytes()).expect("valid");
        assert_eq!(
            v.as_array().expect("array")[0].as_text().as_deref(),
            Some("Ç:\\été\\😀")
        );
        // Written back: the lone surrogate as an escape, the rest as is.
        let text = to_text(&Json::Array(vec![
            items[1].clone(),
            Json::str("q\"b\\s\r\n\t\u{1}é😀"),
        ]));
        assert_eq!(
            text,
            "[\"a\\udc00b\",\"q\\\"b\\\\s\\r\\n\\t\\u0001é😀\"]\r\n"
        );
        assert_eq!(
            parse(text.as_bytes())
                .expect("round trip")
                .as_array()
                .map(<[Json]>::len),
            Some(2)
        );
        assert_eq!(
            parse(text.as_bytes())
                .expect("round trip")
                .as_array()
                .expect("array")[0],
            items[1]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(parse(b"0").expect("n").as_int(), Some(0));
        assert_eq!(parse(b"-17").expect("n").as_int(), Some(-17));
        assert_eq!(parse(b"4294967296").expect("n").as_int(), Some(1 << 32));
        assert_eq!(parse(b"1.5").expect("n").as_int(), None);
        assert_eq!(parse(b"1e3").expect("n").as_int(), Some(1000));
        assert_eq!(parse(b"\"1\"").expect("s").as_int(), None);
        let mut out = String::new();
        write_inline(&mut out, &Json::Num(1.5));
        write_inline(&mut out, &Json::int(-3));
        write_inline(&mut out, &Json::Num(f64::NAN));
        assert_eq!(out, "1.5-30");
    }

    #[test]
    fn written_as_everything_writes_its_session() {
        let session = Json::Array(vec![obj(&[
            ("is_closed", Json::int(0)),
            ("window_x", Json::int(200)),
            ("bookmarks_sidebar_expanded", Json::Array(Vec::new())),
            (
                "tabs",
                Json::Array(vec![obj(&[
                    ("search", Json::str(r"C:\a")),
                    (
                        "listview_selection",
                        Json::Array(vec![Json::str(r"C:\a\b"), Json::str("x")]),
                    ),
                    (
                        "nav_history",
                        Json::Array(vec![
                            obj(&[("search", Json::str("")), ("sort", Json::str("Name"))]),
                            obj(&[("search", Json::str("y")), ("sort", Json::str("Size"))]),
                        ]),
                    ),
                ])]),
            ),
        ])]);
        let text = to_text(&session);
        assert_eq!(
            text,
            "[\r\n\
             \t{\r\n\
             \t\t\"is_closed\":0,\r\n\
             \t\t\"window_x\":200,\r\n\
             \t\t\"bookmarks_sidebar_expanded\":[],\r\n\
             \t\t\"tabs\":\r\n\
             \t\t[\r\n\
             \t\t\t{\r\n\
             \t\t\t\t\"search\":\"C:\\\\a\",\r\n\
             \t\t\t\t\"listview_selection\":[\"C:\\\\a\\\\b\",\"x\"],\r\n\
             \t\t\t\t\"nav_history\":\r\n\
             \t\t\t\t[\r\n\
             \t\t\t\t\t{\"search\":\"\",\"sort\":\"Name\"},\r\n\
             \t\t\t\t\t{\"search\":\"y\",\"sort\":\"Size\"}\r\n\
             \t\t\t\t]\r\n\
             \t\t\t}\r\n\
             \t\t]\r\n\
             \t}\r\n\
             ]\r\n"
        );
        assert_eq!(parse(text.as_bytes()).expect("round trip"), session);
    }
}
