//! Which `content:` terms a search has, and what gozd is asked instead.
//!
//! gozd never reads a file's content (wave 9 spec, item 5): the window asks
//! gozd for the search without its content terms, and reads the files of
//! its answer itself (`super::Run`). That takes a [`Plan`]: the searches to
//! ask gozd, the content terms, and how a file's answers combine into the
//! verdict.
//!
//! Two planners, by what gozd reads:
//!
//! - [`plan_exact`] (W9 stream B2), for a gozd that reads the new query
//!   language: goz-core's exact helper (`ContentQuery`) turns n ≤ 3
//!   content terms anywhere in the search (after NOT, in an OR, in groups,
//!   a value that is itself a group or a `regex:`) into the 2^n gozd
//!   searches, each content term replaced by always-true or always-false,
//!   and gives the exact verdict for a file from which of them found it
//!   and, only while they disagree, its content, read term by term with
//!   goz-core's own reading (Everything's encodings and filters, as the
//!   goz CLI reads). More than 3 is refused with the reason.
//! - [`plan`] (W9 stream B1), for an older gozd (v0.6.0), which reads no
//!   OR, NOT or group and so can be asked no constant: content terms ANDed
//!   at the top level of the search (`*.txt content:"chicken noodle"
//!   utf16content:abc`); the file must be found by gozd's search and hold
//!   every term, read by `super::scan`. Any other place for a content term
//!   (after `!` or NOT, in an OR, inside `< >`, `content:<a|b>`) is refused
//!   with the reason.

use std::sync::Arc;

use goz_core::query::{ContentQuery, MAX_CONTENT_TERMS, QueryError};

use super::scan::{Encoding, Term};

/// The verdict for a file: `found` has bit `i` set when gozd's search `i`
/// found it; `content(j)` says whether term `j`'s text is in it (the file
/// is read once, at the first ask, and never when the searches decide).
pub type Combine = Arc<dyn Fn(u32, &mut dyn FnMut(usize) -> bool) -> bool + Send + Sync>;

/// What a search with content terms runs as.
#[derive(Clone)]
pub struct Plan {
    /// The searches gozd is asked, in the search box's words (the window
    /// adds the filter and the folder to each, as to any search). A file is
    /// a candidate when one of them finds it.
    pub queries: Vec<String>,
    /// [`plan`]'s terms, read by `super::scan` in one pass; empty for
    /// [`plan_exact`]'s.
    pub terms: Vec<Term>,
    /// [`plan_exact`]'s: goz-core's split of the search, whose terms read a
    /// file one at a time; and the search's text and case it was made of.
    pub exact: Option<(Arc<ContentQuery>, String, bool)>,
    pub combine: Combine,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("queries", &self.queries)
            .field("terms", &self.terms)
            .field("exact", &self.exact.as_ref().map(|e| (&e.1, e.2)))
            .finish_non_exhaustive()
    }
}

/// Plans `text` with goz-core's exact helper, for a gozd that reads the new
/// query language (`client::boolean_syntax`): any boolean shape of up to
/// [`MAX_CONTENT_TERMS`] content terms. A search goz-core cannot read is
/// planned as an older gozd's ([`plan`]: goz-gui's own terms in it, such
/// as `filelist0:`, are no goz-core function), and gozd says what is wrong
/// with it.
pub fn plan_exact(text: &str, match_case: bool) -> Planned {
    let split = match ContentQuery::new(text, match_case) {
        Ok(split) => split,
        Err(QueryError::TooManyContentTerms(n)) => {
            return Planned::Refused(format!(
                "A search can have at most {MAX_CONTENT_TERMS} content terms, and this one has \
                 {n}: each doubles the searches gozd is asked"
            ));
        }
        Err(_) => return plan(text, match_case),
    };
    if split.is_plain() {
        return Planned::None;
    }
    let split = Arc::new(split);
    let verdicts = Arc::clone(&split);
    let combine: Combine = Arc::new(move |found, content| {
        // `found` has a bit for each of the 2^n (at most 8) searches.
        verdicts.verdict(found as u8, &mut *content)
    });
    Planned::Content(Plan {
        queries: split.queries().to_vec(),
        terms: Vec::new(),
        exact: Some((split, text.to_owned(), match_case)),
        combine,
    })
}

/// What the planner made of a search.
#[derive(Debug)]
pub enum Planned {
    /// No content term: gozd answers it alone.
    None,
    Content(Plan),
    /// A content term where it cannot be answered yet, and why.
    Refused(String),
}

/// The content functions, by name (lower case, `-` and `_` left out).
fn encoding_of(name: &str) -> Option<Encoding> {
    Some(match name {
        "content" => Encoding::Auto,
        "ansicontent" => Encoding::Ansi,
        "utf8content" => Encoding::Utf8,
        "utf16content" => Encoding::Utf16,
        "utf16becontent" => Encoding::Utf16Be,
        _ => return None,
    })
}

/// The modifiers a content term takes, as Everything spells them.
#[derive(Clone, Copy, Debug, Default)]
struct Mods {
    case: Option<bool>,
    whole_word: Option<bool>,
    diacritics: Option<bool>,
}

impl Mods {
    /// Applies modifier `name`; false when it is none of a content term's.
    fn apply(&mut self, name: &str) -> bool {
        match name {
            "case" => self.case = Some(true),
            "nocase" => self.case = Some(false),
            "diacritics" => self.diacritics = Some(true),
            "nodiacritics" => self.diacritics = Some(false),
            "wholeword" | "wholewords" | "ww" => self.whole_word = Some(true),
            "nowholeword" | "nowholewords" | "noww" => self.whole_word = Some(false),
            _ => return false,
        }
        true
    }
}

/// Everything's other search modifiers: before a content term, refused by
/// name rather than read as another function.
const OTHER_MODIFIERS: &[&str] = &[
    "ascii",
    "noascii",
    "utf8",
    "noutf8",
    "binary",
    "hex",
    "eachline",
    "endwith",
    "noendwith",
    "startwith",
    "nostartwith",
    "prefix",
    "noprefix",
    "suffix",
    "nosuffix",
    "exact",
    "file",
    "files",
    "folder",
    "folders",
    "ignorepunctuation",
    "nopunc",
    "ignorewhitespace",
    "nows",
    "path",
    "nopath",
    "regex",
    "noregex",
    "wfn",
    "nowfn",
    "wholefilename",
    "nowholefilename",
    "wildcards",
    "nowildcards",
    "frommdisk",
    "fromdisk",
    "notindexed",
    "indexed",
];

/// A name and its colon at the start of `s` (letters, digits, `-`, `_`):
/// the name folded as Everything reads it, and the length with the colon.
fn head(s: &str) -> Option<(String, usize)> {
    let colon = s.bytes().take(40).position(|c| c == b':')?;
    let raw = &s[..colon];
    if raw.is_empty()
        || !raw
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return None;
    }
    let name = raw
        .bytes()
        .filter(|c| !matches!(c, b'-' | b'_'))
        .map(|c| char::from(c.to_ascii_lowercase()))
        .collect();
    Some((name, colon + 1))
}

/// What a word of the search is, read from its start.
enum Word {
    /// A content term: its encoding, modifiers, and where its value starts.
    Content {
        encoding: Encoding,
        mods: Mods,
        value_at: usize,
        func: String,
    },
    /// A content term behind another modifier of Everything's.
    Unsupported {
        modifier: String,
        func: String,
    },
    Other,
}

fn read_word(s: &str) -> Word {
    let mut mods = Mods::default();
    let mut at = 0;
    let mut other: Option<String> = None;
    while let Some((name, len)) = head(&s[at..]) {
        if let Some(encoding) = encoding_of(&name) {
            let func = format!("{}:", &s[at..at + len - 1]);
            return match other {
                Some(modifier) => Word::Unsupported { modifier, func },
                None => Word::Content {
                    encoding,
                    mods,
                    value_at: at + len,
                    func,
                },
            };
        }
        if !mods.apply(&name) {
            if !OTHER_MODIFIERS.contains(&name.as_str()) {
                return Word::Other;
            }
            other.get_or_insert_with(|| format!("{}:", &s[at..at + len - 1]));
        }
        at += len;
    }
    Word::Other
}

/// A word of the search at its top level: bytes `start..end`.
struct TopWord {
    start: usize,
    end: usize,
}

/// The search's words at its top level (white space parts them, outside
/// quotes and `< >`), and whether a `|` stands at its top level.
fn top_words(text: &str) -> (Vec<TopWord>, bool) {
    let b = text.as_bytes();
    let mut words = Vec::new();
    let (mut quoted, mut depth) = (false, 0usize);
    let mut or = false;
    let mut start = None;
    for (i, &c) in b.iter().enumerate() {
        let space = !quoted && depth == 0 && c.is_ascii_whitespace();
        if space {
            if let Some(s) = start.take() {
                words.push(TopWord { start: s, end: i });
            }
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        match c {
            b'"' => quoted = !quoted,
            b'<' if !quoted => depth += 1,
            b'>' if !quoted => depth = depth.saturating_sub(1),
            b'|' if !quoted && depth == 0 => or = true,
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(TopWord {
            start: s,
            end: b.len(),
        });
    }
    (words, or)
}

/// A content function found in a search, and how it stands.
struct Found {
    func: String,
    /// Inside `< >`.
    grouped: bool,
    /// After `!`, or in a group after one.
    negated: bool,
}

/// Every content function in `text` (outside quotes, at a term's start:
/// the text's start, after white space, `!`, `<`, `|`), with how it
/// stands.
fn find_content(text: &str) -> Vec<Found> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut quoted = false;
    // Each open group: whether a `!` stands before it.
    let mut groups: Vec<bool> = Vec::new();
    let mut at_start = true;
    let mut bang = false;
    for (i, &c) in b.iter().enumerate() {
        if !quoted && at_start && text.is_char_boundary(i) {
            match read_word(&text[i..]) {
                Word::Content { func, .. } | Word::Unsupported { func, .. } => {
                    out.push(Found {
                        func,
                        grouped: !groups.is_empty(),
                        negated: bang || groups.iter().any(|&n| n),
                    });
                }
                Word::Other => {}
            }
        }
        match c {
            b'"' => quoted = !quoted,
            b'<' if !quoted => groups.push(bang),
            b'>' if !quoted => {
                groups.pop();
            }
            _ => {}
        }
        bang = !quoted && c == b'!';
        at_start = !quoted && (c.is_ascii_whitespace() || matches!(c, b'!' | b'<' | b'|'));
    }
    out
}

/// The reasons a content term cannot be answered where it stands.
fn not_yet(func: &str, place: &str) -> String {
    format!(
        "{func} {place} is not supported by this gozd: with it goz-gui reads content for \
         {func} terms joined with spaces (AND) at the search's top level only (a gozd that \
         reads OR, goz 0.7, takes any)"
    )
}

/// Plans `text` (the search box's text, before the filter and the folder):
/// its content terms out, read with `match_case` unless a term says
/// (`case:`, `nocase:`).
pub fn plan(text: &str, match_case: bool) -> Planned {
    let found = find_content(text);
    let Some(first) = found.first() else {
        return Planned::None;
    };
    if let Some(f) = found.iter().find(|f| f.negated) {
        return Planned::Refused(not_yet(&f.func, "after ! (NOT)"));
    }
    if let Some(f) = found.iter().find(|f| f.grouped) {
        return Planned::Refused(not_yet(&f.func, "inside < >"));
    }
    let func = first.func.clone();
    let (words, or) = top_words(text);
    if or {
        return Planned::Refused(not_yet(&func, "in an OR (|)"));
    }
    let mut terms = Vec::new();
    let mut rest = String::with_capacity(text.len());
    let mut at = 0;
    let mut not_before = false;
    for w in &words {
        let word = &text[w.start..w.end];
        let upper = word.to_ascii_uppercase();
        if upper == "OR" {
            return Planned::Refused(not_yet(&func, "in an OR"));
        }
        match read_word(word) {
            Word::Unsupported { modifier, func } => {
                return Planned::Refused(format!(
                    "{func} with {modifier} is not supported: a content term takes case:, \
                     diacritics: and wholeword: (ww:)"
                ));
            }
            Word::Content {
                encoding,
                mods,
                value_at,
                func,
            } => {
                if not_before {
                    return Planned::Refused(not_yet(&func, "after NOT"));
                }
                let value = &word[value_at..];
                if value.starts_with('<') {
                    return Planned::Refused(format!(
                        "{func}<...> is not supported yet: write each text as a {func} term of \
                         its own"
                    ));
                }
                terms.push(Term {
                    encoding,
                    text: value.replace('"', ""),
                    case: mods.case.unwrap_or(match_case),
                    whole_word: mods.whole_word.unwrap_or(false),
                    diacritics: mods.diacritics.unwrap_or(false),
                });
                rest.push_str(&text[at..w.start]);
                at = w.end;
            }
            Word::Other => {}
        }
        not_before = upper == "NOT";
    }
    rest.push_str(&text[at..]);
    if terms.len() > 32 {
        return Planned::Refused("A search can have at most 32 content terms".into());
    }
    let n = terms.len();
    let combine: Combine =
        Arc::new(move |found, content| found & 1 != 0 && (0..n).all(&mut *content));
    Planned::Content(Plan {
        queries: vec![rest.split_whitespace().collect::<Vec<_>>().join(" ")],
        terms,
        exact: None,
        combine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(text: &str) -> Plan {
        match plan(text, false) {
            Planned::Content(p) => p,
            other => panic!("{text}: {other:?}"),
        }
    }

    fn refused(text: &str) -> String {
        match plan(text, false) {
            Planned::Refused(why) => why,
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn searches_without_content_are_gozds() {
        for text in [
            "",
            "abc *.txt",
            "\"content:abc\"",
            "ext:content",
            "mycontent:x",
            "a|b !c <d e>",
            "case:ext:txt",
        ] {
            assert!(matches!(plan(text, false), Planned::None), "{text}");
        }
    }

    #[test]
    fn top_level_content_terms_are_taken_out() {
        let p = content("*.txt content:\"chicken noodle\" path:docs");
        assert_eq!(p.queries, ["*.txt path:docs"]);
        assert_eq!(p.terms.len(), 1);
        let t = &p.terms[0];
        assert_eq!(t.text, "chicken noodle");
        assert_eq!(t.encoding, Encoding::Auto);
        assert!(!t.case && !t.whole_word && !t.diacritics);
        let p = content("UTF8Content:abc utf-16-content:x ANSIcontent:y utf16be_content:z");
        let enc: Vec<_> = p.terms.iter().map(|t| t.encoding).collect();
        assert_eq!(
            enc,
            [
                Encoding::Utf8,
                Encoding::Utf16,
                Encoding::Ansi,
                Encoding::Utf16Be
            ]
        );
        assert_eq!(p.queries, [""]);
        // AND is AND.
        assert_eq!(content("a AND content:b").queries, ["a AND"]);
    }

    #[test]
    fn modifiers_and_match_case() {
        let p = content("case:ww:content:Foo nodiacritics:diacritics:content:é");
        assert!(p.terms[0].case && p.terms[0].whole_word && !p.terms[0].diacritics);
        assert!(!p.terms[1].case && p.terms[1].diacritics);
        let Planned::Content(p) = plan("content:x nocase:content:y", true) else {
            panic!()
        };
        assert!(p.terms[0].case, "Match Case");
        assert!(!p.terms[1].case, "nocase: over Match Case");
        let why = refused("regex:content:a.c");
        assert!(why.contains("regex:") && why.contains("content:"), "{why}");
    }

    #[test]
    fn other_shapes_are_refused_with_the_reason() {
        assert!(refused("!content:abc").contains("after ! (NOT)"));
        assert!(refused("a !<content:x>").contains("after ! (NOT)"));
        assert!(refused("NOT content:abc").contains("after NOT"));
        assert!(refused("a <b content:x>").contains("inside < >"));
        assert!(refused("content:abc | foo").contains("in an OR"));
        assert!(refused("foo|content:abc").contains("in an OR"));
        assert!(refused("content:a OR content:b").contains("in an OR"));
        assert!(refused("content:<abc 123>").contains("<...>"));
        // Every one counts, not only the first.
        assert!(refused("content:a <b content:c>").contains("inside < >"));
        assert!(refused("content:a !utf8content:c").contains("utf8content: after !"));
        // The quotes keep a `|` and a `!` text.
        let p = content("content:\"a|b !c\"");
        assert_eq!(p.terms[0].text, "a|b !c");
    }

    /// A path term before it (its `C:` is no function).
    #[test]
    fn a_path_before_a_content_term() {
        let p = content("C:\\Users\\x\\w9b1fx\\ content:chicken");
        assert_eq!(p.queries, ["C:\\Users\\x\\w9b1fx\\"]);
        assert_eq!(p.terms[0].text, "chicken");
    }

    fn exact(text: &str) -> Plan {
        match plan_exact(text, false) {
            Planned::Content(p) => p,
            other => panic!("{text}: {other:?}"),
        }
    }

    /// The verdict of `p` for a file the searches `found`, its content
    /// answers `answers`, and how many answers were asked.
    fn verdict(p: &Plan, found: u32, answers: &[bool]) -> (bool, usize) {
        let mut asked = 0;
        let v = (p.combine)(found, &mut |j| {
            asked += 1;
            answers[j]
        });
        (v, asked)
    }

    #[test]
    fn a_newer_gozd_gets_every_shape_exactly() {
        assert!(matches!(plan_exact("abc *.txt", false), Planned::None));
        // NOT: gozd is asked the search with the term false, then true.
        let p = exact("*.txt !content:soup");
        assert_eq!(p.queries.len(), 2);
        assert!(p.terms.is_empty() && p.exact.is_some());
        // Found by the "false" search alone (it is a .txt): soup decides.
        assert_eq!(verdict(&p, 0b01, &[false]), (true, 1));
        assert_eq!(verdict(&p, 0b01, &[true]), (false, 1));
        // Found by neither (not a .txt): no read.
        assert_eq!(verdict(&p, 0b00, &[true]), (false, 0));
        // OR with a name term: a file named so needs no read.
        let p = exact("report | content:chicken");
        assert_eq!(p.queries.len(), 2);
        assert_eq!(verdict(&p, 0b11, &[false]), (true, 0));
        assert_eq!(verdict(&p, 0b10, &[true]), (true, 1));
        assert_eq!(verdict(&p, 0b10, &[false]), (false, 1));
        // Three terms in groups: eight searches.
        let p = exact("<content:a | content:b> !utf8content:c");
        assert_eq!(p.queries.len(), 8);
        // A value that is a group, or a regex: one term each.
        assert_eq!(exact("content:<abc|123>").queries.len(), 2);
        assert_eq!(exact("regex:content:a.c").queries.len(), 2);
    }

    #[test]
    fn more_than_three_are_refused_and_goz_guis_own_terms_planned_as_before() {
        let Planned::Refused(why) = plan_exact("content:a content:b content:c content:d", false)
        else {
            panic!("refused");
        };
        assert!(why.contains("at most 3") && why.contains("has 4"), "{why}");
        // `filelist0:` is goz-gui's, left in every search for the window to
        // put in gozd's words, as in any search.
        let Planned::Content(p) = plan_exact("filelist0: content:abc", false) else {
            panic!("planned");
        };
        assert!(p.queries.iter().all(|q| q.starts_with("filelist0: ")));
    }

    #[test]
    fn the_combine_needs_the_search_and_every_term() {
        let p = content("x content:a content:b");
        let all = |found: u32, answers: [bool; 2]| {
            let mut asked = 0;
            let v = (p.combine)(found, &mut |j| {
                asked += 1;
                answers[j]
            });
            (v, asked)
        };
        assert_eq!(all(1, [true, true]), (true, 2));
        assert_eq!(all(1, [false, true]), (false, 1));
        assert_eq!(
            all(0, [true, true]),
            (false, 0),
            "not read when gozd did not find it"
        );
    }
}
