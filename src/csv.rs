//! Comma-separated values as Everything writes its lists (`Bookmarks.csv`,
//! and later `Search History.csv`): RFC 4180 quoting, CRLF between records,
//! UTF-8. A field remembers whether it was quoted, because Everything quotes
//! text and leaves numbers and unset values bare, and a file goz-gui reads
//! and writes back should not change where nothing changed.

/// One field of a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub text: String,
    /// Written in quotes even when the text does not need them.
    pub quoted: bool,
}

impl Field {
    /// A bare field: a number, or an unset value.
    pub fn plain(text: impl Into<String>) -> Field {
        Field {
            text: text.into(),
            quoted: false,
        }
    }

    /// A field in quotes: text.
    pub fn quoted(text: impl Into<String>) -> Field {
        Field {
            text: text.into(),
            quoted: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Nothing of the field read yet.
    Start,
    Bare,
    Quoted,
    /// Past a quoted field's closing quote.
    Closed,
}

/// What [`parse_checked`] found: the records, and which of them are not
/// well-formed CSV.
#[derive(Debug, Default)]
pub struct Checked {
    pub records: Vec<Vec<Field>>,
    /// The indices of the records with text after a closing quote, or (the
    /// last one) with a quote never closed.
    pub bad: Vec<usize>,
    /// The text ends inside a quoted value: it was cut short there.
    pub unclosed: bool,
}

impl Checked {
    /// Why the text is not well-formed CSV, if it is not.
    pub fn problem(&self) -> Option<String> {
        if self.unclosed {
            Some("a quoted value in it is never closed (it may be cut short)".into())
        } else if !self.bad.is_empty() {
            Some("a value in it has text after its closing quote".into())
        } else {
            None
        }
    }
}

/// The records of `text`, each a list of fields. A byte order mark is
/// skipped; a record ends at CRLF, LF or CR outside quotes; blank lines are
/// skipped; quoted fields may hold commas, line breaks and doubled quotes.
/// Malformed input (text after a closing quote, a quote never closed) is
/// read as leniently as it can be rather than refused (an import takes what
/// it can); [`parse_checked`] says where it was.
pub fn parse(text: &str) -> Vec<Vec<Field>> {
    parse_checked(text).records
}

/// [`parse`], noting the records that are not well-formed: a file goz-gui
/// keeps that has them was damaged (cut short, or edited into something
/// else), and is not read as if it were whole.
pub fn parse_checked(text: &str) -> Checked {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Checked::default();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut bad = false;
    let mut state = State::Start;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (state, c) {
            (State::Quoted, '"') => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    state = State::Closed;
                }
            }
            (State::Quoted, c) => field.push(c),
            (State::Start, '"') => {
                quoted = true;
                state = State::Quoted;
            }
            (_, ',') => {
                record.push(Field {
                    text: core::mem::take(&mut field),
                    quoted,
                });
                quoted = false;
                state = State::Start;
            }
            (_, '\r' | '\n') => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                // A line with nothing on it is no record.
                if state != State::Start || !record.is_empty() {
                    record.push(Field {
                        text: core::mem::take(&mut field),
                        quoted,
                    });
                    if core::mem::take(&mut bad) {
                        out.bad.push(out.records.len());
                    }
                    out.records.push(core::mem::take(&mut record));
                }
                quoted = false;
                state = State::Start;
            }
            (State::Start | State::Bare, c) => {
                field.push(c);
                state = State::Bare;
            }
            // Text after a closing quote joins the field.
            (State::Closed, c) => {
                field.push(c);
                bad = true;
            }
        }
    }
    if state != State::Start || !record.is_empty() {
        out.unclosed = state == State::Quoted;
        record.push(Field {
            text: field,
            quoted,
        });
        if bad || out.unclosed {
            out.bad.push(out.records.len());
        }
        out.records.push(record);
    }
    out
}

/// Whether `text` looks cut short: it does not end with a line break (a
/// file goz-gui or Everything writes always does) and its last record has
/// fewer fields than its first, the header. A file kept beside a good
/// backup that looks so is taken as damaged
/// (`crate::store::file::load_checked`).
pub fn cut_short(text: &str) -> bool {
    if text.is_empty() || text.ends_with(['\r', '\n']) {
        return false;
    }
    let records = parse(text);
    match (records.first(), records.last()) {
        (Some(header), Some(last)) if records.len() > 1 => last.len() < header.len(),
        _ => false,
    }
}

/// For `crate::store::file::load_checked`: why a CSV file's bytes look cut
/// short, if they do.
pub fn suspect(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    cut_short(text).then(|| "its last line is cut short".to_owned())
}

/// Appends one record to `out`, ending with CRLF. A field is quoted when it
/// was, or when its text needs it.
pub fn write_record(fields: &[Field], out: &mut String) {
    write_fields(fields.iter(), out);
}

/// [`write_record`] for fields that are not in one slice.
pub fn write_fields<'a>(fields: impl Iterator<Item = &'a Field>, out: &mut String) {
    for (i, field) in fields.enumerate() {
        if i > 0 {
            out.push(',');
        }
        let needs = field.text.contains([',', '"', '\r', '\n']);
        if field.quoted || needs {
            out.push('"');
            out.push_str(&field.text.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(&field.text);
        }
    }
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(records: &[Vec<Field>]) -> Vec<Vec<&str>> {
        records
            .iter()
            .map(|r| r.iter().map(|f| f.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn round_trip_keeps_quotes_commas_and_line_breaks() {
        let rows = vec![
            vec![Field::plain("Name"), Field::plain("Search")],
            vec![
                Field::quoted("say \"hi\", then go"),
                Field::quoted("line one\r\nline two\nthree\rfour"),
            ],
            vec![Field::quoted(""), Field::plain("")],
            vec![Field::plain("0"), Field::quoted("a,b")],
            vec![Field::plain("needs, quotes"), Field::plain("ünïcödé ✓")],
        ];
        let mut text = String::new();
        for row in &rows {
            write_record(row, &mut text);
        }
        let back = parse(&text);
        assert_eq!(texts(&back), texts(&rows));
        // A quoted empty field and a bare one stay as they were.
        assert!(back[2][0].quoted && !back[2][1].quoted);
        // A bare field that needs quotes gets them, and is read back quoted.
        assert!(back[4][0].quoted);
        // Writing what was read changes nothing.
        let mut again = String::new();
        for row in &back {
            write_record(row, &mut again);
        }
        assert_eq!(again, text);
    }

    #[test]
    fn reads_what_editors_and_other_programs_write() {
        // A byte order mark, LF line ends, blank lines, no final line end.
        let text = "\u{feff}a,b\n\n1,\"x\"\"y\"\r\n\r\n,\n2,3";
        assert_eq!(
            texts(&parse(text)),
            vec![
                vec!["a", "b"],
                vec!["1", "x\"y"],
                vec!["", ""],
                vec!["2", "3"]
            ]
        );
        // Lone CR ends a record too; a trailing comma is an empty field.
        assert_eq!(texts(&parse("a,\rb")), vec![vec!["a", ""], vec!["b"]]);
    }

    #[test]
    fn malformed_records_are_noted() {
        let c = parse_checked("a,b\r\n\"ok\",1\r\n\"x\"y,2\r\n\"z\",3\r\n");
        assert_eq!(c.records.len(), 4);
        assert_eq!(c.bad, [2]);
        assert!(!c.unclosed);
        assert!(
            c.problem()
                .is_some_and(|p| p.contains("after its closing quote"))
        );
        // A quote never closed swallows the rest: the file was cut there.
        let c = parse_checked("a,b\r\n\"ok\",1\r\n\"cut sh");
        assert_eq!(c.records.len(), 3);
        assert_eq!((c.bad.as_slice(), c.unclosed), (&[2][..], true));
        assert!(c.problem().is_some_and(|p| p.contains("never closed")));
        // Well-formed, with a comma, doubled quotes and a line break quoted.
        let c = parse_checked("a,b\r\n\"x, \"\"y\"\"\r\nz\",1\r\n");
        assert!(c.bad.is_empty() && c.problem().is_none());
        assert_eq!(c.records[1][0].text, "x, \"y\"\r\nz");
    }

    #[test]
    fn a_file_cut_short_is_told_from_one_without_a_last_line_break() {
        // Cut after a comma or inside the row: the last record is short.
        assert!(cut_short("a,b,c\r\n\"x\",1,2\r\n\"y\","));
        assert!(suspect(b"a,b,c\r\n\"x\",1,2\r\n\"y\"").is_some());
        // Whole, with or without the last line break; a header alone.
        assert!(!cut_short("a,b,c\r\n\"x\",1,2\r\n"));
        assert!(!cut_short("a,b,c\r\n\"x\",1,2"));
        assert!(!cut_short("a,b,c"));
        assert!(!cut_short(""));
        assert_eq!(suspect(b"a\r\n\xff"), None);
    }

    #[test]
    fn malformed_input_is_read_leniently() {
        assert_eq!(texts(&parse("\"ab\"cd,e")), vec![vec!["abcd", "e"]]);
        assert_eq!(texts(&parse("\"open,\r\nend")), vec![vec!["open,\r\nend"]]);
        assert_eq!(texts(&parse("a\"b,c")), vec![vec!["a\"b", "c"]]);
        assert!(parse("").is_empty());
        assert!(parse("\r\n\n").is_empty());
    }
}
