//! Advanced Rename's engine (Edit > Advanced > Advanced Rename...): the new
//! names of several files from an old format and a new format, as
//! Everything 1.5's renamer makes them.
//!
//! **Old format** (without regular expressions). `%1` to `%9` match any
//! text, as few characters as will do, and keep it in that variable; `*`
//! does the same into the next variable the format does not name, and `?`
//! matches one character; `%%` is a `%`. With any of them the whole name
//! must match; a name that does not is left as it is. An old format with
//! none of them is text to find: every place it occurs is replaced by the
//! new format (Everything's find and replace).
//!
//! **New format.** `%1` to `%9` are the variables' text; `$number:` is the
//! file's order number (`$number000:` pads it to three digits), `$count:`
//! the number of files; `<uppercase:x>`, `<lowercase:x>` and
//! `<titlecase:x>` (and Everything 1.4's `$uppercase(x)`, `$lowercase(x)`,
//! `$titlecase(x)`) change the case of `x`; `%%`, `$$`, `$(` and `$)` are
//! `%`, `$`, `(` and `)`.
//!
//! **Regular expressions.** The old format is a regular expression; every
//! match in the name is replaced by the new format, where `\0` is the text
//! matched and `\1` to `\9` its groups (`\\` a backslash).
//!
//! Matching ignores case unless Match case is on, and diacritics (é is e)
//! unless Match diacritics is; the text kept is the name's own. Ignore
//! extension renames a file's name without its extension (a folder's whole
//! name). Each new name is trimmed of white-space at its ends
//! (Everything's `rename_trim=1`).

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Globalization::{NormalizationD, NormalizeString};

/// The renamer's switches, Everything's `rename_match_case`,
/// `rename_match_diacritics`, `rename_regex`, `rename_ignore_extension`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub match_case: bool,
    pub match_diacritics: bool,
    pub regex: bool,
    pub ignore_extension: bool,
}

/// A file to rename: its name, and whether it is a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    pub name: String,
    pub is_dir: bool,
}

impl Subject {
    pub fn new(name: &str, is_dir: bool) -> Subject {
        Subject {
            name: name.to_owned(),
            is_dir,
        }
    }

    /// The part a format applies to, and the rest kept after it: with
    /// Ignore extension a file's name up to its last dot (unless it starts
    /// with the dot) and its extension; otherwise the whole name.
    fn split(&self, ignore_extension: bool) -> (&str, &str) {
        if !ignore_extension || self.is_dir {
            return (&self.name, "");
        }
        match self.name.rfind('.') {
            Some(i) if i > 0 => self.name.split_at(i),
            _ => (&self.name, ""),
        }
    }
}

/// What is wrong with a format, for the dialog to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    Old(String),
    New(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Old(why) => write!(f, "Old format: {why}"),
            FormatError::New(why) => write!(f, "New format: {why}"),
        }
    }
}

// --- Folding: case and diacritics ---------------------------------------

/// `c` as matching compares it: lower case unless `case`, without its
/// diacritics unless `diacritics`. Always one character for one, so places
/// in the folded text are places in the name.
fn fold_char(c: char, case: bool, diacritics: bool) -> char {
    let mut c = c;
    if !diacritics && !c.is_ascii() {
        c = base_letter(c);
    }
    if !case {
        let mut lower = c.to_lowercase();
        if let (Some(l), None) = (lower.next(), lower.next()) {
            c = l;
        }
    }
    c
}

/// `c` without its diacritics: the first character of its canonical
/// decomposition (Windows' Unicode normalization form D), é → e. Each
/// character is asked of Windows once.
fn base_letter(c: char) -> char {
    thread_local! {
        static SEEN: RefCell<HashMap<char, char>> = RefCell::new(HashMap::new());
    }
    if let Some(base) = SEEN.with_borrow(|m| m.get(&c).copied()) {
        return base;
    }
    let mut unit = [0u16; 2];
    let src = c.encode_utf16(&mut unit);
    let mut out = [0u16; 8];
    // SAFETY: the source and its length; the buffer and its length.
    let n = unsafe {
        NormalizeString(
            NormalizationD,
            src.as_ptr(),
            src.len() as i32,
            out.as_mut_ptr(),
            out.len() as i32,
        )
    };
    let base = usize::try_from(n)
        .ok()
        .filter(|&n| n > 0 && n <= out.len())
        .and_then(|n| char::decode_utf16(out[..n].iter().copied()).next())
        .and_then(Result::ok)
        .unwrap_or(c);
    SEEN.with_borrow_mut(|m| m.insert(c, base));
    base
}

fn fold(text: &str, o: Options) -> Vec<char> {
    text.chars()
        .map(|c| fold_char(c, o.match_case, o.match_diacritics))
        .collect()
}

/// `text` without its diacritics unless `o` matches them, its case as it
/// is: for regular expressions, which ignore case themselves (lowering an
/// expression would turn `\D` into `\d`).
fn fold_marks(text: &str, o: Options) -> String {
    text.chars()
        .map(|c| fold_char(c, true, o.match_diacritics))
        .collect()
}

// --- The old format -------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Old {
    Char(char),
    /// Any text, kept in variable n (1 to 9).
    Var(u8),
    /// Any one character.
    One,
}

/// The old format's parts (characters folded as `o` says), and whether it
/// has a variable or a wildcard (then the whole name must match).
fn parse_old(format: &str, o: Options) -> (Vec<Old>, bool) {
    let chars: Vec<char> = format.chars().collect();
    let named: Vec<u8> = chars
        .windows(2)
        .enumerate()
        .filter(|&(i, w)| {
            w[0] == '%' && w[1].is_ascii_digit() && w[1] != '0' && !escaped(&chars, i)
        })
        .map(|(_, w)| w[1] as u8 - b'0')
        .collect();
    let mut next_free = (1..=9u8).filter(|n| !named.contains(n));
    let mut out = Vec::new();
    let mut wild = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '%' if chars.get(i + 1) == Some(&'%') => {
                out.push(Old::Char('%'));
                i += 2;
                continue;
            }
            '%' if chars.get(i + 1).is_some_and(|d| ('1'..='9').contains(d)) => {
                out.push(Old::Var(chars[i + 1] as u8 - b'0'));
                wild = true;
                i += 2;
                continue;
            }
            '*' => {
                // A tenth `*` has no variable left: it matches, unkept.
                out.push(Old::Var(next_free.next().unwrap_or(0)));
                wild = true;
            }
            '?' => {
                out.push(Old::One);
                wild = true;
            }
            c => out.push(Old::Char(fold_char(c, o.match_case, o.match_diacritics))),
        }
        i += 1;
    }
    (out, wild)
}

/// Whether the `%` at `i` is the second of a `%%`.
fn escaped(chars: &[char], i: usize) -> bool {
    let run = chars[..=i].iter().rev().take_while(|&&c| c == '%').count();
    run % 2 == 0
}

/// Matches the whole of `name` (folded) against `pattern`, each variable
/// taking as little as it can (Everything's %1 is `(.*?)`); the variables'
/// spans in `name`'s characters. Failed states are remembered, so a format
/// of many variables costs at most its parts times the name's length
/// squared.
fn match_whole(pattern: &[Old], name: &[char]) -> Option<[Option<(usize, usize)>; 10]> {
    let mut failed = vec![false; (pattern.len() + 1) * (name.len() + 1)];
    let mut vars = [None; 10];
    fn go(
        p: &[Old],
        pi: usize,
        name: &[char],
        at: usize,
        failed: &mut [bool],
        vars: &mut [Option<(usize, usize)>; 10],
    ) -> bool {
        let key = pi * (name.len() + 1) + at;
        if failed[key] {
            return false;
        }
        let ok = match p.get(pi) {
            None => at == name.len(),
            Some(Old::Char(c)) => {
                name.get(at) == Some(c) && go(p, pi + 1, name, at + 1, failed, vars)
            }
            Some(Old::One) => at < name.len() && go(p, pi + 1, name, at + 1, failed, vars),
            Some(&Old::Var(n)) => (at..=name.len()).any(|end| {
                let ok = go(p, pi + 1, name, end, failed, vars);
                if ok {
                    vars[usize::from(n)] = Some((at, end));
                }
                ok
            }),
        };
        if !ok {
            failed[key] = true;
        }
        ok
    }
    go(pattern, 0, name, 0, &mut failed, &mut vars).then_some(vars)
}

// --- The new format -------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Upper,
    Lower,
    Title,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum New {
    Text(String),
    Var(u8),
    /// The file's order number, padded to this many digits.
    Number(usize),
    Count,
    Case(Case, Vec<New>),
}

fn case_named(name: &str) -> Option<Case> {
    match name {
        "uppercase" => Some(Case::Upper),
        "lowercase" => Some(Case::Lower),
        "titlecase" => Some(Case::Title),
        _ => None,
    }
}

struct NewParser<'a> {
    chars: &'a [char],
    at: usize,
    /// `\1` (regular expressions) rather than `%1`.
    regex: bool,
}

impl NewParser<'_> {
    /// Whether the text from the place reached starts with `text`.
    fn starts(&self, text: &str) -> bool {
        (self.at..)
            .zip(text.chars())
            .all(|(i, c)| self.chars.get(i) == Some(&c))
    }

    /// Parts up to `end` (a `)` or `>` closing a function) or the end.
    fn parts(&mut self, end: Option<char>) -> Result<Vec<New>, String> {
        let mut out: Vec<New> = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, out: &mut Vec<New>| {
            if !text.is_empty() {
                out.push(New::Text(std::mem::take(text)));
            }
        };
        while let Some(&c) = self.chars.get(self.at) {
            if Some(c) == end {
                self.at += 1;
                flush(&mut text, &mut out);
                return Ok(out);
            }
            let next = self.chars.get(self.at + 1).copied();
            match (c, next) {
                ('%', Some('%')) if !self.regex => {
                    text.push('%');
                    self.at += 2;
                }
                ('%', Some(d @ '1'..='9')) if !self.regex => {
                    flush(&mut text, &mut out);
                    out.push(New::Var(d as u8 - b'0'));
                    self.at += 2;
                }
                ('\\', Some('\\')) if self.regex => {
                    text.push('\\');
                    self.at += 2;
                }
                ('\\', Some(d @ '0'..='9')) if self.regex => {
                    flush(&mut text, &mut out);
                    out.push(New::Var(d as u8 - b'0'));
                    self.at += 2;
                }
                ('$', Some(e @ ('$' | '(' | ')'))) => {
                    text.push(e);
                    self.at += 2;
                }
                ('$', _) if self.starts("$count:") => {
                    flush(&mut text, &mut out);
                    out.push(New::Count);
                    self.at += "$count:".len();
                }
                ('$', _) if self.starts("$number") => {
                    let zeros = self.chars[self.at + 7..]
                        .iter()
                        .take_while(|&&z| z == '0')
                        .count();
                    if self.chars.get(self.at + 7 + zeros) == Some(&':') {
                        flush(&mut text, &mut out);
                        out.push(New::Number(zeros));
                        self.at += 8 + zeros;
                    } else {
                        text.push('$');
                        self.at += 1;
                    }
                }
                ('$', _) => {
                    let name: String = self.chars[self.at + 1..]
                        .iter()
                        .take_while(|c| c.is_ascii_lowercase())
                        .collect();
                    let open = self.at + 1 + name.len();
                    match case_named(&name) {
                        Some(case) if self.chars.get(open) == Some(&'(') => {
                            flush(&mut text, &mut out);
                            self.at = open + 1;
                            let inside = self.parts(Some(')'))?;
                            out.push(New::Case(case, inside));
                        }
                        _ => {
                            text.push('$');
                            self.at += 1;
                        }
                    }
                }
                ('<', _) => {
                    let name: String = self.chars[self.at + 1..]
                        .iter()
                        .take_while(|c| c.is_ascii_lowercase())
                        .collect();
                    let colon = self.at + 1 + name.len();
                    match case_named(&name) {
                        Some(case) if self.chars.get(colon) == Some(&':') => {
                            flush(&mut text, &mut out);
                            self.at = colon + 1;
                            let inside = self.parts(Some('>'))?;
                            out.push(New::Case(case, inside));
                        }
                        _ => {
                            text.push('<');
                            self.at += 1;
                        }
                    }
                }
                (c, _) => {
                    text.push(c);
                    self.at += 1;
                }
            }
        }
        match end {
            Some(e) => Err(format!("a function has no closing {e}")),
            None => {
                flush(&mut text, &mut out);
                Ok(out)
            }
        }
    }
}

fn parse_new(format: &str, regex: bool) -> Result<Vec<New>, String> {
    let chars: Vec<char> = format.chars().collect();
    NewParser {
        chars: &chars,
        at: 0,
        regex,
    }
    .parts(None)
}

/// `text` in title case: the first letter of each word upper case, the
/// others lower case. A word starts after white-space or `- _ . ( [ { , + &`.
fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if c.is_alphabetic() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
            continue;
        }
        out.push(c);
        if c.is_numeric() {
            start = false;
        } else if c.is_whitespace() || "-_.([{,+&".contains(c) {
            start = true;
        }
    }
    out
}

/// What the new format's variables and numbers stand for, for one file.
struct Values<'a> {
    vars: [Option<&'a str>; 10],
    number: usize,
    count: usize,
}

fn render(parts: &[New], v: &Values<'_>, out: &mut String) {
    for part in parts {
        match part {
            New::Text(t) => out.push_str(t),
            New::Var(n) => out.push_str(v.vars[usize::from(*n)].unwrap_or("")),
            New::Number(width) => out.push_str(&format!("{:0width$}", v.number, width = *width)),
            New::Count => out.push_str(&v.count.to_string()),
            New::Case(case, inside) => {
                let mut text = String::new();
                render(inside, v, &mut text);
                match case {
                    Case::Upper => out.push_str(&text.to_uppercase()),
                    Case::Lower => out.push_str(&text.to_lowercase()),
                    Case::Title => out.push_str(&title_case(&text)),
                }
            }
        }
    }
}

// --- Renaming -------------------------------------------------------------

/// The new names of `subjects` (`None`: does not match the old format, so
/// left as it is), or what is wrong with a format.
pub fn rename_all(
    subjects: &[Subject],
    old: &str,
    new: &str,
    o: Options,
) -> Result<Vec<Option<String>>, FormatError> {
    let parts = parse_new(new, o.regex).map_err(FormatError::New)?;
    let count = subjects.len();
    if o.regex {
        let regex = regex::RegexBuilder::new(&fold_marks(old, o))
            .case_insensitive(!o.match_case)
            .build()
            .map_err(|e| FormatError::Old(regex_error(&e)))?;
        return Ok(subjects
            .iter()
            .enumerate()
            .map(|(i, s)| by_regex(s, &regex, &parts, o, i + 1, count))
            .collect());
    }
    let (pattern, wild) = parse_old(old, o);
    Ok(subjects
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (stem, ext) = s.split(o.ignore_extension);
            let chars: Vec<char> = stem.chars().collect();
            let folded = fold(stem, o);
            let mut values = Values {
                vars: [None; 10],
                number: i + 1,
                count,
            };
            let mut out = String::new();
            if wild {
                let spans = match_whole(&pattern, &folded)?;
                let texts: Vec<Option<String>> = spans
                    .iter()
                    .map(|span| span.map(|(a, b)| chars[a..b].iter().collect()))
                    .collect();
                for (slot, text) in values.vars.iter_mut().zip(&texts) {
                    *slot = text.as_deref();
                }
                render(&parts, &values, &mut out);
            } else {
                let needle: Vec<char> = pattern
                    .iter()
                    .filter_map(|p| match p {
                        Old::Char(c) => Some(*c),
                        _ => None,
                    })
                    .collect();
                if needle.is_empty() {
                    return None;
                }
                let mut at = 0;
                let mut found = false;
                while at < folded.len() {
                    if folded[at..].starts_with(&needle) {
                        render(&parts, &values, &mut out);
                        at += needle.len();
                        found = true;
                    } else {
                        out.push(chars[at]);
                        at += 1;
                    }
                }
                if !found {
                    return None;
                }
            }
            out.push_str(ext);
            Some(out.trim().to_owned())
        })
        .collect())
}

/// The regex crate's message, on one line.
fn regex_error(e: &regex::Error) -> String {
    let text = e.to_string();
    text.lines()
        .rev()
        .find(|l| l.trim_start().starts_with("error:"))
        .map(|l| l.trim().trim_start_matches("error:").trim().to_owned())
        .unwrap_or_else(|| text.lines().next().unwrap_or("invalid").to_owned())
}

fn by_regex(
    s: &Subject,
    regex: &regex::Regex,
    parts: &[New],
    o: Options,
    number: usize,
    count: usize,
) -> Option<String> {
    let (stem, ext) = s.split(o.ignore_extension);
    let folded = fold_marks(stem, o);
    // Where each character starts in the folded text and in the name: a
    // place in one is the same character's place in the other.
    let starts = |t: &str| -> Vec<usize> {
        t.char_indices()
            .map(|(b, _)| b)
            .chain(std::iter::once(t.len()))
            .collect()
    };
    let (folded_at, name_at) = (starts(&folded), starts(stem));
    let to_name = |b: usize| {
        let i = folded_at.binary_search(&b).unwrap_or_else(|i| i);
        name_at[i.min(name_at.len() - 1)]
    };
    let mut out = String::new();
    let mut last = 0;
    let mut found = false;
    for caps in regex.captures_iter(&folded) {
        let Some(whole) = caps.get(0) else { continue };
        found = true;
        let (a, b) = (to_name(whole.start()), to_name(whole.end()));
        out.push_str(&stem[last..a]);
        let texts: Vec<Option<&str>> = (0..10)
            .map(|g| {
                caps.get(g)
                    .map(|m| &stem[to_name(m.start())..to_name(m.end())])
            })
            .collect();
        let mut values = Values {
            vars: [None; 10],
            number,
            count,
        };
        for (slot, text) in values.vars.iter_mut().zip(texts) {
            *slot = text;
        }
        render(parts, &values, &mut out);
        last = b;
    }
    if !found {
        return None;
    }
    out.push_str(&stem[last..]);
    out.push_str(ext);
    Some(out.trim().to_owned())
}

// --- The formats made for a selection -------------------------------------

/// Everything's delimiters for the variables of a made-up format.
const DELIMITERS: [char; 3] = ['.', '-', '_'];

/// `text` as an old format's literal text.
fn escape_old(text: &str) -> String {
    text.replace('%', "%%")
}

/// `text` as a new format's literal text.
fn escape_new(text: &str) -> String {
    text.replace('%', "%%").replace('$', "$$")
}

/// `name` cut into runs between delimiters and the delimiters themselves.
fn tokens(name: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in name.char_indices() {
        if DELIMITERS.contains(&c) {
            out.push(&name[start..i]);
            out.push(&name[i..i + c.len_utf8()]);
            start = i + c.len_utf8();
        }
    }
    out.push(&name[start..]);
    out
}

/// The old and new formats Advanced Rename opens with for `subjects`
/// (Everything's "generated" formats): the same, so the new names are the
/// old ones until the new format is changed. One file: its name. Several:
/// the text they share, with `%1`, `%2`... for the parts between `.`, `-`
/// and `_` that differ; else what they start and end with around `%1`. In
/// regular expression mode, the same as a regular expression and `\1`...
pub fn generate(subjects: &[Subject], o: Options) -> (String, String) {
    let stems: Vec<&str> = subjects
        .iter()
        .map(|s| s.split(o.ignore_extension).0)
        .collect();
    let pieces = made_up(&stems).filter(|p| works(subjects, p, o));
    let pieces = pieces.unwrap_or_else(|| vec![Piece::Var(1)]);
    if o.regex {
        let mut old = String::from("^");
        let mut new = String::new();
        for p in &pieces {
            match p {
                Piece::Text(t) => {
                    old.push_str(&regex::escape(t));
                    new.push_str(&t.replace('\\', "\\\\").replace('$', "$$"));
                }
                Piece::Var(n) => {
                    old.push_str("(.*?)");
                    new.push_str(&format!("\\{n}"));
                }
            }
        }
        old.push('$');
        return (old, new);
    }
    let old: String = pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t) => escape_old(t),
            Piece::Var(n) => format!("%{n}"),
        })
        .collect();
    let new: String = pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t) => escape_new(t),
            Piece::Var(n) => format!("%{n}"),
        })
        .collect();
    (old, new)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    Var(u8),
}

/// A made-up format for `stems` as pieces, before any check.
fn made_up(stems: &[&str]) -> Option<Vec<Piece>> {
    let first = *stems.first()?;
    if stems.iter().all(|s| *s == first) {
        return Some(vec![Piece::Text(first.to_owned())]);
    }
    let cut: Vec<Vec<&str>> = stems.iter().map(|s| tokens(s)).collect();
    let same_shape = cut.iter().all(|t| {
        t.len() == cut[0].len()
            && t.iter()
                .zip(&cut[0])
                .enumerate()
                .all(|(i, (a, b))| i % 2 == 0 || a == b)
    });
    let mut pieces = Vec::new();
    let push_text = |pieces: &mut Vec<Piece>, t: &str| {
        if let Some(Piece::Text(last)) = pieces.last_mut() {
            last.push_str(t);
        } else if !t.is_empty() {
            pieces.push(Piece::Text(t.to_owned()));
        }
    };
    if same_shape {
        let mut n = 0u8;
        for i in 0..cut[0].len() {
            let t = cut[0][i];
            if i % 2 == 1 || cut.iter().all(|c| c[i] == t) {
                push_text(&mut pieces, t);
            } else {
                n += 1;
                if n > 9 {
                    return None;
                }
                pieces.push(Piece::Var(n));
            }
        }
        return Some(pieces);
    }
    // What they all start and end with, around one variable.
    let chars: Vec<Vec<char>> = stems.iter().map(|s| s.chars().collect()).collect();
    let shortest = chars.iter().map(Vec::len).min().unwrap_or(0);
    let prefix = (0..shortest)
        .take_while(|&i| chars.iter().all(|c| c[i] == chars[0][i]))
        .count();
    let suffix = (0..shortest - prefix)
        .take_while(|&i| {
            chars
                .iter()
                .all(|c| c[c.len() - 1 - i] == chars[0][chars[0].len() - 1 - i])
        })
        .count();
    let head: String = chars[0][..prefix].iter().collect();
    let tail: String = chars[0][chars[0].len() - suffix..].iter().collect();
    push_text(&mut pieces, &head);
    pieces.push(Piece::Var(1));
    push_text(&mut pieces, &tail);
    Some(pieces)
}

/// Whether the format `pieces` makes, as old and new format, matches every
/// subject and gives back its own name.
fn works(subjects: &[Subject], pieces: &[Piece], o: Options) -> bool {
    let old: String = pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t) => escape_old(t),
            Piece::Var(n) => format!("%{n}"),
        })
        .collect();
    let new: String = pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t) => escape_new(t),
            Piece::Var(n) => format!("%{n}"),
        })
        .collect();
    let exact = Options {
        match_case: true,
        match_diacritics: true,
        regex: false,
        ..o
    };
    rename_all(subjects, &old, &new, exact).is_ok_and(|names| {
        names
            .iter()
            .zip(subjects)
            .all(|(n, s)| n.as_deref() == Some(s.name.trim()))
    })
}

// --- Problems -------------------------------------------------------------

/// Why a new name cannot be had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Windows refuses the name (`crate::filename`'s words).
    Invalid(&'static str),
    /// Another file of the selection would have the same name there.
    Duplicate,
    /// A file or folder of that name is there already.
    Exists,
}

impl Problem {
    pub fn text(self) -> &'static str {
        match self {
            Problem::Invalid(why) => why,
            // Everything's words.
            Problem::Duplicate => "Duplicated new filename",
            Problem::Exists => "A file or folder with this name already exists",
        }
    }
}

/// The problem of each renamed file: `folders[i]` holds file `i`,
/// `old[i]` is its name and `new[i]` its new name (`None`: not renamed);
/// `exists(folder, name)` says whether that folder holds a file of that
/// name (in lower case) now. A name differing only in case is the same file.
pub fn problems<F>(
    folders: &[&str],
    old: &[&str],
    new: &[Option<String>],
    exists: F,
) -> Vec<Option<Problem>>
where
    F: Fn(&str, &str) -> bool,
{
    let lower = |s: &str| s.to_lowercase();
    // How many files end up with each name in each folder.
    let mut finals: HashMap<(String, String), usize> = HashMap::new();
    for i in 0..old.len() {
        let name = new[i].as_deref().unwrap_or(old[i]);
        *finals.entry((lower(folders[i]), lower(name))).or_default() += 1;
    }
    (0..old.len())
        .map(|i| {
            let name = new[i].as_deref()?;
            if name == old[i] {
                return None;
            }
            let units: Vec<u16> = name.encode_utf16().collect();
            if let Some(why) = crate::filename::problem(&units) {
                return Some(Problem::Invalid(why));
            }
            let key = (lower(folders[i]), lower(name));
            if finals.get(&key).copied().unwrap_or(0) > 1 {
                return Some(Problem::Duplicate);
            }
            if key.1 != lower(old[i]) && exists(folders[i], &key.1) {
                return Some(Problem::Exists);
            }
            None
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subjects(names: &[&str]) -> Vec<Subject> {
        names.iter().map(|n| Subject::new(n, false)).collect()
    }

    fn run(names: &[&str], old: &str, new: &str, o: Options) -> Vec<Option<String>> {
        rename_all(&subjects(names), old, new, o).unwrap()
    }

    fn some(names: &[&str]) -> Vec<Option<String>> {
        names.iter().map(|n| Some((*n).to_owned())).collect()
    }

    #[test]
    fn variables_capture_and_come_back() {
        let o = Options::default();
        // Swap two parts.
        assert_eq!(
            run(
                &["Smith - John.txt", "Doe - Jane.txt"],
                "%1 - %2.txt",
                "%2 %1.txt",
                o
            ),
            some(&["John Smith.txt", "Jane Doe.txt"])
        );
        // As few characters as will do: %1 stops at the first " - ".
        assert_eq!(
            run(&["a - b - c"], "%1 - %2", "[%1][%2]", o),
            some(&["[a][b - c]"])
        );
        // A name the old format does not match is left as it is.
        assert_eq!(
            run(&["x.txt", "y.doc"], "%1.txt", "%1.md", o),
            vec![Some("x.md".into()), None]
        );
        // * keeps its text in the next free variable, ? is one character.
        assert_eq!(
            run(&["IMG_0001.JPG"], "IMG_*.jpg", "photo %1.jpg", o),
            some(&["photo 0001.jpg"])
        );
        assert_eq!(
            run(&["ab.txt", "abc.txt"], "a?.txt", "z.txt", o),
            vec![Some("z.txt".into()), None]
        );
        // `*` beside a named %2 is %1; %2 takes as little as it can.
        assert_eq!(run(&["a.b"], "%2*", "%1|%2", o), some(&["a.b|"]));
        // %% is a literal %.
        assert_eq!(
            run(&["50%.txt"], "%1%%.txt", "%1 pc.txt", o),
            some(&["50 pc.txt"])
        );
    }

    #[test]
    fn without_a_variable_the_text_is_found_and_replaced() {
        let o = Options::default();
        assert_eq!(
            run(&["abc_ABC.txt", "none.txt"], "abc", "123", o),
            vec![Some("123_123.txt".into()), None]
        );
        // Match case.
        let case = Options {
            match_case: true,
            ..o
        };
        assert_eq!(
            run(&["abc_ABC.txt"], "abc", "123", case),
            some(&["123_ABC.txt"])
        );
        // An empty old format matches nothing.
        assert_eq!(run(&["a.txt"], "", "b", o), vec![None]);
    }

    #[test]
    fn diacritics_match_unless_asked_not_to() {
        let o = Options::default();
        assert_eq!(run(&["Café.txt"], "cafe", "Bar", o), some(&["Bar.txt"]));
        let strict = Options {
            match_diacritics: true,
            ..o
        };
        assert_eq!(run(&["Café.txt"], "cafe", "Bar", strict), vec![None]);
        // The text kept is the name's own.
        assert_eq!(
            run(&["Crème brûlée.txt"], "%1 brulee.txt", "%1.txt", o),
            some(&["Crème.txt"])
        );
    }

    #[test]
    fn the_extension_can_be_left_alone() {
        let o = Options {
            ignore_extension: true,
            ..Options::default()
        };
        assert_eq!(
            run(
                &["report.final.pdf", ".gitignore"],
                "%1",
                "<uppercase:%1>",
                o
            ),
            some(&["REPORT.FINAL.pdf", ".GITIGNORE"])
        );
        // A folder's whole name is its stem.
        let dir = vec![Subject::new("photos.2024", true)];
        assert_eq!(
            rename_all(&dir, "%1", "%1-old", o).unwrap(),
            some(&["photos.2024-old"])
        );
    }

    #[test]
    fn case_numbers_and_escapes() {
        let o = Options::default();
        let names = ["one.txt", "two.txt", "three.txt"];
        assert_eq!(
            run(&names, "%1.txt", "$number:-%1.txt", o),
            some(&["1-one.txt", "2-two.txt", "3-three.txt"])
        );
        assert_eq!(
            run(&names[..1], "%1.txt", "$number000: of $count:.txt", o),
            some(&["001 of 1.txt"])
        );
        assert_eq!(
            run(&["hello world.txt"], "%1.txt", "$titlecase(%1)$(1$).TXT", o),
            some(&["Hello World(1).TXT"])
        );
        assert_eq!(
            run(&["MiXeD.txt"], "%1.%2", "<lowercase:%1>.<uppercase:%2>", o),
            some(&["mixed.TXT"])
        );
        assert_eq!(
            run(&["a.txt"], "%1.txt", "$$%%%1.txt", o),
            some(&["$%a.txt"])
        );
        // An unknown $ word is text.
        assert_eq!(
            run(&["a.txt"], "%1.txt", "$foo%1.txt", o),
            some(&["$fooa.txt"])
        );
        assert_eq!(title_case("o'neil the-end x.y"), "O'neil The-End X.Y");
        // A function left open is an error of the new format.
        assert!(matches!(
            rename_all(&subjects(&["a"]), "%1", "$uppercase(%1", o),
            Err(FormatError::New(_))
        ));
        assert!(matches!(
            rename_all(&subjects(&["a"]), "%1", "<titlecase:%1", o),
            Err(FormatError::New(_))
        ));
        // New names are trimmed at their ends.
        assert_eq!(run(&["a.txt"], "%1.txt", "  %1.txt ", o), some(&["a.txt"]));
    }

    #[test]
    fn regular_expressions_replace_every_match() {
        let o = Options {
            regex: true,
            ..Options::default()
        };
        assert_eq!(
            run(&["a1b22c333.txt"], r"\d+", "#", o),
            some(&["a#b#c#.txt"])
        );
        assert_eq!(
            run(&["Smith, John.txt"], r"^(\w+), (\w+)", r"\2 \1", o),
            some(&["John Smith.txt"])
        );
        assert_eq!(run(&["abc"], "b", r"[\0]\\", o), some(&["a[b]\\c"]));
        // Case ignored unless asked; no match leaves the name.
        assert_eq!(run(&["ABC"], "b", "x", o), some(&["AxC"]));
        let case = Options {
            match_case: true,
            ..o
        };
        assert_eq!(run(&["ABC"], "b", "x", case), vec![None]);
        // Diacritics ignored: the groups are the name's own text.
        assert_eq!(
            run(&["résumé.doc"], r"^(r)esume", r"\1-CV", o),
            some(&["r-CV.doc"])
        );
        // A bad expression is an error of the old format.
        assert!(matches!(
            rename_all(&subjects(&["a"]), "(", "x", o),
            Err(FormatError::Old(_))
        ));
    }

    #[test]
    fn formats_are_made_from_the_selection() {
        let o = Options::default();
        let g = |names: &[&str]| generate(&subjects(names), o);
        assert_eq!(g(&["only.txt"]), ("only.txt".into(), "only.txt".into()));
        assert_eq!(
            g(&["IMG_001.jpg", "IMG_002.jpg"]),
            ("IMG_%1.jpg".into(), "IMG_%1.jpg".into())
        );
        assert_eq!(g(&["a.txt", "bb.doc"]), ("%1.%2".into(), "%1.%2".into()));
        // Shapes differ: what they share around %1.
        assert_eq!(g(&["a.b.txt", "c.txt"]), ("%1.txt".into(), "%1.txt".into()));
        // % and $ in names are escaped.
        assert_eq!(
            g(&["100%_a$.txt", "100%_b$.txt"]),
            ("100%%_%1.txt".into(), "100%%_%1.txt".into())
        );
        // With Ignore extension the extension is not in the formats.
        let ie = Options {
            ignore_extension: true,
            ..o
        };
        assert_eq!(
            generate(&subjects(&["x-1.txt", "x-2.doc"]), ie),
            ("x-%1".into(), "x-%1".into())
        );
        // As a regular expression.
        let re = Options { regex: true, ..o };
        assert_eq!(
            generate(&subjects(&["IMG_001.jpg", "IMG_002.jpg"]), re),
            (r"^IMG_(.*?)\.jpg$".into(), r"IMG_\1.jpg".into())
        );
        // Whatever the names, the made-up formats give them back.
        for names in [
            &["a", "b", "c"][..],
            &["x.tar.gz", "y.zip", "z"],
            &["same", "same"],
            &["1-2-3-4-5-6-7-8-9-10-11", "a-b-c-d-e-f-g-h-i-j-k"],
            &["%%", "$$"],
        ] {
            for o in [o, ie, re] {
                let (old, new) = generate(&subjects(names), o);
                let got = rename_all(&subjects(names), &old, &new, o).unwrap();
                assert_eq!(got, some(names), "{names:?} {old} {new}");
            }
        }
    }

    #[test]
    fn many_variables_stay_quick() {
        // Every split tried once at most: this would take forever without
        // remembering the failed ones.
        let name = "x".repeat(200);
        let start = std::time::Instant::now();
        let got = run(&[&name], "%1%2%3%4%5%6%7%8%9y", "z", Options::default());
        assert_eq!(got, vec![None]);
        assert!(start.elapsed().as_secs() < 2);
    }

    #[test]
    fn problems_are_found() {
        let folders = [r"C:\a", r"C:\a", r"C:\a", r"C:\b", r"C:\a"];
        let old = ["1.txt", "2.txt", "3.txt", "4.txt", "5.txt"];
        let new = vec![
            Some("same.txt".to_owned()),
            Some("SAME.txt".to_owned()),
            Some("bad?.txt".to_owned()),
            Some("taken.txt".to_owned()),
            None,
        ];
        let exists = |folder: &str, name: &str| folder == r"C:\b" && name == "taken.txt";
        assert_eq!(
            problems(&folders, &old, &new, exists),
            vec![
                Some(Problem::Duplicate),
                Some(Problem::Duplicate),
                Some(Problem::Invalid(crate::filename::BAD_CHARS)),
                Some(Problem::Exists),
                None,
            ]
        );
        // A change of case only is the same file, and an unchanged name
        // no problem; a name another file of the selection keeps is.
        let old = ["a.txt", "b.txt", "c.txt"];
        let new = vec![
            Some("A.txt".to_owned()),
            Some("b.txt".to_owned()),
            Some("a.TXT".to_owned()),
        ];
        let on_disk = |_: &str, name: &str| ["a.txt", "b.txt", "c.txt"].contains(&name);
        assert_eq!(
            problems(&[r"C:\a"; 3], &old, &new, on_disk),
            vec![Some(Problem::Duplicate), None, Some(Problem::Duplicate)]
        );
        let new = vec![Some("A.txt".to_owned()), None, Some("b.txt".to_owned())];
        assert_eq!(
            problems(&[r"C:\a"; 3], &old, &new, on_disk),
            vec![None, None, Some(Problem::Duplicate)]
        );
    }
}
