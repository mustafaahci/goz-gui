//! goz-gui's own matcher for an open file list: the search box's text is
//! parsed by goz-core's own parser (so it reads exactly as gozd reads it,
//! and is refused with gozd's words), then each row of the list is tested
//! the way gozd's engine tests an index entry.
//!
//! The parser fills buckets for the terms that must all match (the kind
//! `file:` / `folder:`, `size:`, `dm:`, `ext:`, name terms and wildcards,
//! path terms and wildcards: a term with `\`, or every term under Match
//! Path / `path:`), tested first and cheapest; W9: what the buckets cannot
//! hold stays an expression ANDed with them (OR, NOT, `< >` groups, a term
//! with a modifier such as `ww:`, `startwith:`, `nopunc:`, `regex:` or
//! `diacritics:`, `stem:`, `parent:`, `len:` and the other functions on the
//! row itself), evaluated here as gozd evaluates it. The functions that
//! look at other entries (`child:`, `descendant:`, `sibling:` and their
//! kin, child and descendant counts, `total-child-size:`) need gozd's tree
//! of the volume, which a list does not have: they are refused with the
//! reason. Without Match Case (`case:`) both sides are folded as goz
//! folds them; `diacritics:` keeps accents.

use goz_core::fold::{fold_exact_into, fold_into};
use goz_core::index::Column;
use goz_core::query::{Anchors, Expr, FolderPath, Kind, Leaf, LenOf, ParsedQuery, Skip, Target};
use memchr::memmem::Finder;

use super::wild;

/// A parsed search, ready to test rows with.
pub struct Matcher {
    parsed: ParsedQuery,
    names: Vec<Finder<'static>>,
    paths: Vec<Finder<'static>>,
    /// What the buckets cannot hold, its `regex:` terms compiled.
    expr: Option<Node>,
    /// Reusable buffers: a row's texts in each form, and an extension.
    texts: Texts,
    ext_buf: Vec<u8>,
}

/// One row as the matcher sees it.
pub struct Candidate<'a> {
    /// The full path, UTF-8.
    pub path: &'a [u8],
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    /// W9: the list's Date Created and Attributes, for `dc:` and `attrib:`.
    pub ctime: Option<i64>,
    pub attrib: Option<u32>,
}

/// The name in a full path: what follows its last `\`, or the whole path
/// for a drive's root (`C:\`).
pub fn name_of(path: &[u8]) -> &[u8] {
    match memchr::memrchr(b'\\', path) {
        Some(i) if i + 1 < path.len() => &path[i + 1..],
        _ => path,
    }
}

/// How a term compares text: as stored (`case:`), folded keeping accents
/// (`diacritics:`), or folded (the default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Raw,
    Accented,
    Folded,
}

impl Form {
    fn of(case: bool, diacritics: bool) -> Form {
        if case {
            Form::Raw
        } else if diacritics {
            Form::Accented
        } else {
            Form::Folded
        }
    }
}

/// A row's name and full path folded each way, made the first time a term
/// asks.
#[derive(Default)]
struct Texts {
    name: [Vec<u8>; 2],
    path: [Vec<u8>; 2],
    made: [bool; 4],
}

impl Texts {
    fn reset(&mut self) {
        self.made = [false; 4];
    }

    /// `text` (the row's name, or its path when `path`) in `form`.
    fn get<'a>(&'a mut self, text: &'a [u8], path: bool, form: Form) -> &'a [u8] {
        let k = match form {
            Form::Raw => return text,
            Form::Folded => 0,
            Form::Accented => 1,
        };
        let slot = usize::from(path) * 2 + k;
        let buf = if path {
            &mut self.path[k]
        } else {
            &mut self.name[k]
        };
        if !self.made[slot] {
            buf.clear();
            if k == 0 {
                fold_into(text, buf);
            } else {
                fold_exact_into(text, buf);
            }
            self.made[slot] = true;
        }
        buf
    }
}

/// Whether a count is `parent-count:` (`depth:`): the one a row's path
/// answers. goz-core does not export the count's type, only its `Debug`.
fn counts_parents(of: &impl std::fmt::Debug) -> bool {
    format!("{of:?}") == "Parents"
}

/// Why a search cannot run over a file list: a function that looks at other
/// entries, which only gozd's index links.
fn unsupported(e: &Expr) -> Option<&'static str> {
    let mut why = None;
    e.for_each_leaf(&mut |leaf| {
        let w = match leaf {
            Leaf::Related { .. } => Some(
                "child:, descendant:, sibling:, ancestor-name: and the other functions on \
                 related entries",
            ),
            Leaf::Count { of, .. } if !counts_parents(of) => Some(
                "child-count:, empty:, descendant-count:, sibling-count: and the other counts \
                 of related entries",
            ),
            Leaf::TotalChildSize(_) => Some("total-child-size:"),
            _ => None,
        };
        why = why.or(w);
    });
    why
}

impl Matcher {
    /// The matcher for `parsed`, or why a file list cannot answer it.
    pub fn new(parsed: ParsedQuery) -> Result<Matcher, String> {
        let expr = match &parsed.expr {
            None => None,
            Some(e) => {
                if let Some(what) = unsupported(e) {
                    return Err(format!(
                        "{what} need gozd's index: an open file list has no folder tree to \
                         answer them"
                    ));
                }
                Some(compile(e)?)
            }
        };
        let finder = |t: &Vec<u8>| Finder::new(t.as_slice()).into_owned();
        Ok(Matcher {
            names: parsed.name_terms.iter().map(finder).collect(),
            paths: parsed.path_terms.iter().map(finder).collect(),
            parsed,
            expr,
            texts: Texts::default(),
            ext_buf: Vec::new(),
        })
    }

    /// Whether `row` matches the search.
    pub fn matches(&mut self, row: &Candidate<'_>) -> bool {
        self.texts.reset();
        let f = &self.parsed.filters;
        match f.kind {
            Some(Kind::File) if row.is_dir => return false,
            Some(Kind::Folder) if !row.is_dir => return false,
            _ => {}
        }
        // An unknown size is no match for a size filter, as in gozd.
        if let Some(range) = f.size
            && !row.size.is_some_and(|s| range.contains(s))
        {
            return false;
        }
        if let Some(date) = f.date
            && !date.matches(row.mtime)
        {
            return false;
        }
        let case = self.parsed.match_case;
        let name = name_of(row.path);
        if !self.names.is_empty() || !self.parsed.wildcards.is_empty() {
            let hay = self.texts.get(name, false, Form::of(case, false));
            if !self.names.iter().all(|t| t.find(hay).is_some())
                || !self.parsed.wildcards.iter().all(|w| w.matches(hay))
            {
                return false;
            }
        }
        // The extension, folded whatever the case (es and gozd alike); a
        // folder has none.
        if let Some(exts) = &f.ext {
            if row.is_dir {
                return false;
            }
            self.ext_buf.clear();
            fold_into(name, &mut self.ext_buf);
            if !ext_matches(exts, &self.ext_buf) {
                return false;
            }
        }
        if !self.paths.is_empty() || !self.parsed.path_wildcards.is_empty() {
            let hay = self.texts.get(row.path, true, Form::of(case, false));
            if !self.paths.iter().all(|t| t.find(hay).is_some())
                || !self.parsed.path_wildcards.iter().all(|w| w.matches(hay))
            {
                return false;
            }
        }
        self.expr
            .as_ref()
            .is_none_or(|e| eval(e, row, &mut self.texts))
    }
}

/// The expression as a list's rows are tested with it: goz-core's, with
/// each `regex:` term's pattern compiled for the rows' bytes (goz-core
/// keeps its own compiled pattern to itself).
enum Node {
    True,
    False,
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Leaf(Leaf),
    Regex(Target, regex::bytes::Regex),
}

fn compile(e: &Expr) -> Result<Node, String> {
    let all = |items: &[Expr]| items.iter().map(compile).collect::<Result<Vec<_>, _>>();
    Ok(match e {
        Expr::True => Node::True,
        Expr::False => Node::False,
        Expr::And(items) => Node::And(all(items)?),
        Expr::Or(items) => Node::Or(all(items)?),
        Expr::Not(inner) => Node::Not(Box::new(compile(inner)?)),
        Expr::Leaf(Leaf::Regex { target, regex }) => {
            // As goz-core builds it: case-insensitive unless `case:`.
            let r = regex::bytes::RegexBuilder::new(regex.source())
                .case_insensitive(!regex.matches_case())
                .unicode(true)
                .size_limit(512 << 10)
                .build()
                .map_err(|err| err.to_string())?;
            Node::Regex(*target, r)
        }
        Expr::Leaf(leaf) => Node::Leaf(leaf.clone()),
    })
}

/// Whether the folded (or as-stored) name `name` has one of the extensions
/// `values`, as gozd's `ext:` reads them: a plain value of `k` dot-separated
/// parts names the name's last `k` parts after a dot (`tar.gz` matches
/// `c.tar.gz`, `a.txt` does not match `a.txt`); a value with a wildcard
/// matches the text after the last dot (nothing when there is no dot); an
/// empty value names a name with no dot.
fn ext_matches(values: &[Vec<u8>], name: &[u8]) -> bool {
    let dot = memchr::memrchr(b'.', name);
    values.iter().any(|v| {
        if v.is_empty() {
            dot.is_none()
        } else if v.iter().any(|&b| b == b'*' || b == b'?') {
            let ext = dot.map_or(&[][..], |d| &name[d + 1..]);
            wild::glob(v, ext)
        } else {
            name.len() > v.len() && name.ends_with(v) && name[name.len() - v.len() - 1] == b'.'
        }
    })
}

/// The part of the row `target` names, in `form`: `None` when it has none
/// (a folder's extension, a name without a dot's).
fn target_text<'a>(
    target: Target,
    form: Form,
    row: &'a Candidate<'_>,
    texts: &'a mut Texts,
) -> Option<&'a [u8]> {
    match target {
        Target::Name | Target::Stem | Target::Extension => {
            let name = texts.get(name_of(row.path), false, form);
            let dot = memchr::memrchr(b'.', name);
            Some(match target {
                Target::Name => name,
                // A folder's stem is its name, and it has no extension.
                Target::Stem if row.is_dir => name,
                Target::Stem => &name[..dot.unwrap_or(name.len())],
                _ if row.is_dir => return None,
                _ => &name[dot? + 1..],
            })
        }
        Target::FullPath | Target::Location | Target::ParentName => {
            let path = texts.get(row.path, true, form);
            Some(match target {
                Target::FullPath => path,
                _ => {
                    let location = memchr::memrchr(b'\\', path).unwrap_or(0);
                    if target == Target::Location {
                        &path[..location]
                    } else {
                        let start = memchr::memrchr(b'\\', &path[..location]).map_or(0, |s| s + 1);
                        &path[start..location]
                    }
                }
            })
        }
    }
}

/// Whether `e` holds for `row`.
fn eval(e: &Node, row: &Candidate<'_>, texts: &mut Texts) -> bool {
    match e {
        Node::True => true,
        Node::False => false,
        Node::And(items) => items.iter().all(|c| eval(c, row, texts)),
        Node::Or(items) => items.iter().any(|c| eval(c, row, texts)),
        Node::Not(inner) => !eval(inner, row, texts),
        // The target as stored.
        Node::Regex(target, regex) => {
            target_text(*target, Form::Raw, row, texts).is_some_and(|hay| regex.is_match(hay))
        }
        Node::Leaf(leaf) => leaf_matches(leaf, row, texts),
    }
}

fn leaf_matches(leaf: &Leaf, row: &Candidate<'_>, texts: &mut Texts) -> bool {
    match leaf {
        Leaf::Text {
            target,
            needle,
            case,
            diacritics,
            anchors,
            skip,
        } => target_text(*target, Form::of(*case, *diacritics), row, texts)
            .is_some_and(|hay| text_matches(needle, hay, *anchors, *skip)),
        Leaf::Glob {
            target,
            glob,
            case,
            diacritics,
        } => target_text(*target, Form::of(*case, *diacritics), row, texts)
            .is_some_and(|hay| glob.matches(hay)),
        Leaf::Kind(Kind::File) => !row.is_dir,
        Leaf::Kind(Kind::Folder) => row.is_dir,
        Leaf::Ext {
            values,
            case,
            diacritics,
        } => {
            !row.is_dir && {
                let name = texts.get(name_of(row.path), false, Form::of(*case, *diacritics));
                ext_matches(values, name)
            }
        }
        Leaf::Size(range) => row.size.is_some_and(|s| range.contains(s)),
        // A folder matches no `size:unknown`, as in gozd.
        Leaf::SizeUnknown => !row.is_dir && row.size.is_none(),
        Leaf::Date(date) => date.matches(row.mtime),
        Leaf::Prop { column, test } => {
            let date = match column {
                Column::DateCreated => row.ctime,
                // A list has neither.
                Column::DateAccessed | Column::RecentChange => None,
                Column::Attributes => None,
            };
            let attrib = (*column == Column::Attributes)
                .then_some(row.attrib)
                .flatten();
            test.matches(date, attrib)
        }
        Leaf::Len { of, range } => {
            let len = match of {
                LenOf::Name => target_text(Target::Name, Form::Raw, row, texts).map(utf16_len),
                // A folder has no extension, and so no length of one.
                LenOf::Extension if row.is_dir => None,
                LenOf::Extension => {
                    Some(target_text(Target::Extension, Form::Raw, row, texts).map_or(0, utf16_len))
                }
                LenOf::FullPath => Some(utf16_len(row.path)),
                LenOf::Location => {
                    target_text(Target::Location, Form::Raw, row, texts).map(utf16_len)
                }
            };
            len.is_some_and(|n| range.contains(n))
        }
        // The folders above the row, its drive's root included.
        Leaf::Count { of, range } if counts_parents(of) => {
            let path = row.path.strip_suffix(b"\\").unwrap_or(row.path);
            let above = memchr::memchr_iter(b'\\', path).count() as u64;
            above > 0 && range.contains(above)
        }
        Leaf::Parent(folders) | Leaf::Ancestor(folders) => {
            let exact = matches!(leaf, Leaf::Parent(_));
            let accented = folders
                .iter()
                .any(|f| f.accented.is_some())
                .then(|| accented_location(row, texts));
            let path = texts.get(row.path, true, Form::Folded);
            let location = &path[..memchr::memrchr(b'\\', path).unwrap_or(0)];
            folders
                .iter()
                .any(|f| names(f, location, accented.as_deref(), exact))
        }
        Leaf::Drive(letter) => {
            row.path.len() >= 2
                && row.path[1] == b':'
                && row.path[0].to_ascii_lowercase() == *letter
        }
        // Refused before the scan (`unsupported`); a regex is compiled
        // into a `Node::Regex`.
        Leaf::Count { .. } | Leaf::Related { .. } | Leaf::TotalChildSize(_) => false,
        Leaf::Regex { .. } => false,
    }
}

/// The row's location folded keeping accents, when a folder named with
/// `diacritics:` needs it.
fn accented_location(row: &Candidate<'_>, texts: &mut Texts) -> Vec<u8> {
    let path = texts.get(row.path, true, Form::Accented);
    path[..memchr::memrchr(b'\\', path).unwrap_or(0)].to_vec()
}

/// Whether `folder` is `location` (`exact`), or `location` is in it.
fn names(folder: &FolderPath, location: &[u8], accented: Option<&[u8]>, exact: bool) -> bool {
    let within = |want: &[u8], loc: &[u8]| {
        loc == want
            || (!exact
                && loc.len() > want.len()
                && loc.starts_with(want)
                && loc[want.len()] == b'\\')
    };
    within(&folder.path, location)
        && match (&folder.accented, accented) {
            (Some(want), Some(loc)) => within(want, loc),
            _ => true,
        }
}

/// The length of `text` in UTF-16 units, as Everything counts lengths.
fn utf16_len(text: &[u8]) -> u64 {
    text.iter()
        .map(|&b| u64::from(b & 0xC0 != 0x80) + u64::from(b >= 0xF0))
        .sum()
}

/// Whether code point `c` is part of a word: a letter, a digit or a
/// combining mark (gozd's words).
fn is_word(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(u32::from(c), 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Whether `hay` holds `needle` where `anchors` say, passing over in `hay`
/// what `skip` says, as gozd's text terms match (`ww:`, `prefix:`,
/// `suffix:`, `startwith:`, `endwith:`, `whole:`, `nopunc:`, `nows:`). An
/// empty needle matches any text but the empty one.
fn text_matches(needle: &[u8], hay: &[u8], anchors: Anchors, skip: Skip) -> bool {
    if needle.is_empty() {
        return !hay.is_empty();
    }
    let hay_s = String::from_utf8_lossy(hay);
    let needle_s = String::from_utf8_lossy(needle);
    let h: Vec<char> = hay_s.chars().collect();
    let n: Vec<char> = needle_s.chars().collect();
    let skippable = |c: char| {
        (skip.punct && !is_word(c) && !c.is_whitespace()) || (skip.space && c.is_whitespace())
    };
    let a = anchors;
    let last_start = if a.start {
        if skip.is_none() {
            0
        } else {
            h.iter().position(|&c| !skippable(c)).unwrap_or(h.len())
        }
    } else {
        h.len()
    };
    for s in 0..h.len().min(last_start + 1) {
        if h[s] != n[0] || (a.word_start && s > 0 && is_word(h[s - 1])) {
            continue;
        }
        let (mut k, mut p) = (0, s);
        while k < n.len() && p < h.len() {
            if h[p] == n[k] {
                k += 1;
            } else if skip.is_none() || !skippable(h[p]) {
                break;
            }
            p += 1;
        }
        if k < n.len() {
            continue;
        }
        if a.end && !h[p..].iter().all(|&c| !skip.is_none() && skippable(c)) {
            continue;
        }
        if a.word_end && p < h.len() && is_word(h[p]) {
            continue;
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::{LocalClock, parse_query_at};

    struct Clock;
    impl LocalClock for Clock {
        fn now(&self) -> i64 {
            // 2024-06-01 12:00 UTC.
            133_617_168_000_000_000
        }
        fn to_local(&self, utc: i64) -> Option<i64> {
            Some(utc)
        }
        fn to_utc(&self, local: i64) -> Option<i64> {
            Some(local)
        }
        fn week_start(&self) -> u8 {
            0
        }
        fn date_order(&self) -> goz_core::query::DateOrder {
            goz_core::query::DateOrder::MonthDayYear
        }
    }

    fn hits(
        query: &str,
        case: bool,
        rows: &[(&str, bool, Option<u64>, Option<i64>)],
    ) -> Vec<String> {
        let mut m = Matcher::new(parse_query_at(query, case, &Clock).expect("parses"))
            .unwrap_or_else(|why| panic!("{query}: {why}"));
        rows.iter()
            .filter(|(p, d, s, t)| {
                m.matches(&Candidate {
                    path: p.as_bytes(),
                    is_dir: *d,
                    size: *s,
                    mtime: *t,
                    ctime: None,
                    attrib: None,
                })
            })
            .map(|r| r.0.to_owned())
            .collect()
    }

    const ROWS: [(&str, bool, Option<u64>, Option<i64>); 6] = [
        (
            r"C:\Music\Queen - Bohemian Rhapsody.mp3",
            false,
            Some(8_000_000),
            Some(133_600_000_000_000_000),
        ),
        (
            r"C:\Music\notes.txt",
            false,
            Some(120),
            Some(133_000_000_000_000_000),
        ),
        (r"C:\Music", true, None, None),
        (
            r"D:\Projects\Report final.PDF",
            false,
            Some(2_000_000),
            None,
        ),
        (r"D:\Projects\café.txt", false, Some(5), None),
        (r"C:\", true, None, None),
    ];

    #[test]
    fn terms_wildcards_and_paths_as_gozd_reads_them() {
        assert_eq!(hits("queen rhapsody", false, &ROWS), [ROWS[0].0]);
        assert_eq!(hits("\"final.pdf\"", false, &ROWS), [ROWS[3].0]);
        assert!(hits("Final", true, &ROWS).is_empty(), "case:");
        assert_eq!(hits("*.txt", false, &ROWS), [ROWS[1].0, ROWS[4].0]);
        assert_eq!(hits("not?s.*", false, &ROWS), [ROWS[1].0]);
        assert_eq!(hits(r"music\ .txt", false, &ROWS), [ROWS[1].0]);
        assert_eq!(hits(r"D:\", false, &ROWS).len(), 2);
        assert_eq!(
            hits("CAFÉ", false, &ROWS),
            [ROWS[4].0],
            "folded as goz folds"
        );
        // W9: `C:` alone is the drive, as in Everything (the new parser).
        assert_eq!(
            hits("C:", false, &ROWS),
            [ROWS[0].0, ROWS[1].0, ROWS[2].0, ROWS[5].0],
            "the drive"
        );
    }

    #[test]
    fn functions() {
        assert_eq!(hits("ext:pdf;mp3", false, &ROWS), [ROWS[0].0, ROWS[3].0]);
        assert_eq!(hits("size:>1mb", false, &ROWS), [ROWS[0].0, ROWS[3].0]);
        assert_eq!(hits("folder:", false, &ROWS), [ROWS[2].0, ROWS[5].0]);
        assert!(hits("file: music", false, &ROWS).is_empty());
        assert_eq!(hits("folder: music", false, &ROWS), [ROWS[2].0]);
        assert_eq!(hits("dm:2024", false, &ROWS), [ROWS[0].0]);
        assert_eq!(hits("path:music notes", false, &ROWS), [ROWS[1].0]);
    }

    /// W9: what the new parser adds, which the buckets cannot hold.
    #[test]
    fn or_not_and_groups() {
        assert_eq!(hits("queen | notes", false, &ROWS), [ROWS[0].0, ROWS[1].0]);
        assert_eq!(
            hits("*.txt !café", false, &ROWS),
            [ROWS[1].0],
            "NOT, folded as goz folds"
        );
        assert_eq!(hits("<queen | report> !pdf", false, &ROWS), [ROWS[0].0]);
        assert_eq!(
            hits("!notes file:", false, &ROWS),
            [ROWS[0].0, ROWS[3].0, ROWS[4].0]
        );
        // NOT in capitals is a word, as in Everything (not: is the operator):
        // notes.txt holds both.
        assert_eq!(hits("NOT notes", false, &ROWS), [ROWS[1].0]);
        assert_eq!(
            hits("ext:pdf | ext:mp3", false, &ROWS),
            [ROWS[0].0, ROWS[3].0]
        );
        assert_eq!(
            hits("!folder: size:<1kb", false, &ROWS),
            [ROWS[1].0, ROWS[4].0]
        );
        // The same text in quotes is text.
        assert!(hits("\"queen|notes\"", false, &ROWS).is_empty());
    }

    #[test]
    fn modifiers_and_row_functions() {
        assert_eq!(hits("ww:rhap", false, &ROWS), Vec::<String>::new());
        assert_eq!(hits("ww:rhapsody", false, &ROWS), [ROWS[0].0]);
        assert_eq!(hits("startwith:not", false, &ROWS), [ROWS[1].0]);
        assert_eq!(hits("endwith:.pdf", false, &ROWS), [ROWS[3].0]);
        assert_eq!(hits("whole:music", false, &ROWS), [ROWS[2].0]);
        assert_eq!(
            hits("nopunc:queenbohemian", false, &ROWS),
            Vec::<String>::new()
        );
        assert_eq!(hits("nows:reportfinal", false, &ROWS), [ROWS[3].0]);
        assert_eq!(hits("diacritics:cafe", false, &ROWS), Vec::<String>::new());
        assert_eq!(hits("diacritics:café", false, &ROWS), [ROWS[4].0]);
        assert_eq!(hits("regex:^n.t", false, &ROWS), [ROWS[1].0]);
        assert_eq!(hits("stem:notes", false, &ROWS), [ROWS[1].0]);
        assert_eq!(
            hits(r"parent:C:\Music", false, &ROWS),
            [ROWS[0].0, ROWS[1].0]
        );
        assert_eq!(hits(r"ancestor:D:\", false, &ROWS), [ROWS[3].0, ROWS[4].0]);
        assert_eq!(hits("len:<10 file:", false, &ROWS), [ROWS[1].0, ROWS[4].0]);
        assert_eq!(hits("d: *.txt", false, &ROWS), [ROWS[4].0]);
        assert_eq!(
            hits("depth:2", false, &ROWS),
            [ROWS[0].0, ROWS[1].0, ROWS[3].0, ROWS[4].0]
        );
        // Content searches over a list ask its rows with constants.
        assert_eq!(hits("name-length:>=0 *.txt", false, &ROWS).len(), 2);
        assert!(hits("name-length:>18446744073709551615", false, &ROWS).is_empty());
    }

    #[test]
    fn dates_created_and_attributes_from_the_list() {
        let parsed = parse_query_at("attrib:r | dc:2024", false, &Clock).expect("parses");
        let mut m = Matcher::new(parsed).expect("a list answers it");
        let row = |attrib, ctime| Candidate {
            path: br"C:\a.txt",
            is_dir: false,
            size: Some(1),
            mtime: None,
            ctime,
            attrib,
        };
        assert!(m.matches(&row(Some(0x1), None)));
        assert!(m.matches(&row(None, Some(133_600_000_000_000_000))));
        assert!(!m.matches(&row(Some(0x20), None)));
    }

    #[test]
    fn functions_on_other_entries_are_refused() {
        for q in [
            "child:x",
            "empty:",
            "descendant-count:>2",
            "total-child-size:>1mb",
        ] {
            let parsed = parse_query_at(q, false, &Clock).expect("parses");
            let why = Matcher::new(parsed).err().unwrap_or_else(|| panic!("{q}"));
            assert!(why.contains("open file list"), "{q}: {why}");
        }
    }

    #[test]
    fn names_in_paths() {
        assert_eq!(name_of(br"C:\a\b.txt"), b"b.txt");
        assert_eq!(name_of(br"C:\"), br"C:\");
        assert_eq!(name_of(b"plain"), b"plain");
    }

    #[test]
    fn extensions_as_gozd_reads_them() {
        let e = |v: &[&str], name: &str| {
            let values: Vec<Vec<u8>> = v.iter().map(|s| s.as_bytes().to_vec()).collect();
            ext_matches(&values, name.as_bytes())
        };
        assert!(e(&["gz"], "c.tar.gz") && e(&["tar.gz"], "c.tar.gz"));
        assert!(!e(&["a.txt"], "a.txt"));
        assert!(e(&["mp?"], "a.mp3") && !e(&["tar.*"], "c.tar.gz"));
        assert!(e(&[""], "README") && !e(&[""], "a.txt"));
    }
}
