//! The `filelist:` search function and the file-list slots (wave 6), as
//! Everything 1.5 has them.
//!
//! `filelist:<names>` matches the items named: a name without a `\`
//! matches an item of that whole name (any folder), a path with a `\`
//! that whole full path; `*` and `?` make a wildcard of either
//! (`super::wild`). Names are separated by `;`, or by `|` inside quotes
//! (`filelist:"a.txt|C:\b c\d.txt"`, Everything 1.4's form); an unquoted
//! `|` or `>` ends the value, as an operator. Case is ignored unless the
//! search matches case. A name holding a `;` cannot be written there, but
//! can be in a slot.
//!
//! `filelist0:` .. `filelist9:` match the names in that slot of the tab:
//! File | Set File List Slot 0..9 (keyboard commands, Everything's
//! `file_set_file_list_slot_N_keys`) puts the full paths of the selected
//! results in it. Each tab has its own ten, kept in `Session.json` as
//! Everything keeps them (`filelist_slot0`..`9`, arrays of names). An
//! empty slot matches nothing.
//!
//! Where it is answered:
//! - **An open file list**: by goz-gui ([`split_for_list`]): the terms
//!   are taken out of the search, the rest goes to goz's parser as before,
//!   and a row must match every term too.
//! - **gozd's index** ([`for_gozd`]): gozd has no such function (v0.6.0
//!   and the query-language branch both refuse `filelist:` as unknown).
//!   A gozd that reads OR, groups and `whole:` (the query-language branch,
//!   which goz-gui's probe finds) gets each term as the group of its names
//!   (`<whole:"a.txt"|whole:"C:\x\b.txt"|"*.log">`), which is the same
//!   search, and an empty list as `whole:""`, which matches nothing; at
//!   most 256 names, gozd's own limit on a search's terms. The deployed
//!   v0.6.0 gozd cannot say "any of these", so the search is refused with
//!   a precise reason rather than answered wrongly.

use std::borrow::Cow;
use std::collections::HashSet;

use super::matcher::name_of;
use super::wild::{folded, glob, has_wildcard};

/// The number of slots, as Everything's.
pub const COUNT: usize = 10;

/// gozd's most terms in one search (the query-language branch's
/// `MAX_QUERY_TERMS`).
pub const GOZD_MAX_TERMS: usize = 256;

/// The keyboard commands File | Set File List Slot 0..9: 4850..=4859
/// (wave 6's range, 4850..=4899).
pub mod cmd {
    pub const SET_SLOT_FIRST: u16 = 4850;
    /// One past the last slot's.
    pub const SET_SLOT_END: u16 = SET_SLOT_FIRST + super::COUNT as u16;
    /// One past wave 6's range.
    pub const END: u16 = 4900;

    const _: () = {
        assert!(crate::listedit::cmd::END <= SET_SLOT_FIRST);
        assert!(SET_SLOT_END <= END);
        assert!((END as usize) <= crate::menu::SHELL_FIRST as usize);
    };

    /// The slot a command sets, if it is one of them.
    pub fn slot_of(id: u16) -> Option<usize> {
        (SET_SLOT_FIRST..SET_SLOT_END)
            .contains(&id)
            .then(|| usize::from(id - SET_SLOT_FIRST))
    }
}

/// A tab's ten slots: the names in each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slots(pub [Vec<String>; COUNT]);

impl Slots {
    pub fn get(&self, slot: usize) -> &[String] {
        self.0.get(slot).map_or(&[], Vec::as_slice)
    }

    /// Slot `slot` holds `names` from now on.
    pub fn set(&mut self, slot: usize, names: Vec<String>) {
        if let Some(s) = self.0.get_mut(slot) {
            *s = names;
        }
    }
}

/// A `filelist:` term of a search: where it is (bytes `start..end`, from
/// its function name to the end of its value), its slot (`filelistN:`)
/// and its value, quotes taken off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub start: usize,
    pub end: usize,
    pub slot: Option<usize>,
    pub value: String,
}

/// `filelist:` or `filelistN:` (any case) at the start of `b`: the slot,
/// and how long the function's name is with its colon.
fn head(b: &[u8]) -> Option<(Option<usize>, usize)> {
    if b.len() < 9 || !b[..8].eq_ignore_ascii_case(b"filelist") {
        return None;
    }
    match b[8] {
        b':' => Some((None, 9)),
        d @ b'0'..=b'9' if b.get(9) == Some(&b':') => Some((Some(usize::from(d - b'0')), 10)),
        _ => None,
    }
}

/// The `filelist:` terms of `query`: each where a term may start (the
/// search's start, after white space, or after `!`, `<` or `|`), not in
/// quotes.
pub fn terms(query: &str) -> Vec<Term> {
    let b = query.as_bytes();
    let mut out = Vec::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !quoted
            && at_start
            && let Some((slot, len)) = head(&b[i..])
        {
            let from = i + len;
            let mut j = from;
            let mut q = false;
            while j < b.len() {
                match b[j] {
                    b'"' => q = !q,
                    d if !q && (d.is_ascii_whitespace() || d == b'|' || d == b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            out.push(Term {
                start: i,
                end: j,
                slot,
                value: query[from..j].replace('"', ""),
            });
            i = j;
            at_start = false;
            continue;
        }
        if c == b'"' {
            quoted = !quoted;
        }
        at_start = !quoted && (c.is_ascii_whitespace() || matches!(c, b'!' | b'<' | b'|'));
        i += 1;
    }
    out
}

/// A name as a list holds it: trimmed, `/` made `\`, a folder's trailing
/// `\` taken off (a drive's root keeps its own: `C:\`).
fn normalize(name: &str) -> Option<String> {
    let t = name.trim();
    if t.is_empty() {
        return None;
    }
    let mut s = t.replace('/', "\\");
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    Some(s)
}

/// The names `term` stands for: its own, or its slot's. An error for a
/// slot's term with a value of its own.
pub fn names(term: &Term, slots: &Slots) -> Result<Vec<String>, String> {
    match term.slot {
        Some(n) if !term.value.trim().is_empty() => Err(format!(
            "filelist{n}: takes no value: it matches the names in file list slot {n} \
             (File | Set File List Slot {n} puts the selected results there)"
        )),
        Some(n) => Ok(slots.get(n).iter().filter_map(|s| normalize(s)).collect()),
        None => Ok(term.value.split([';', '|']).filter_map(normalize).collect()),
    }
}

/// What a row must match for one `filelist:` term of a search over a file
/// list.
pub struct ListMatcher {
    case: bool,
    /// Whole names and whole full paths, folded unless the case counts.
    names: HashSet<Vec<u8>>,
    paths: HashSet<Vec<u8>>,
    /// Wildcards, and whether each is on the full path.
    globs: Vec<(Vec<u8>, bool)>,
}

impl ListMatcher {
    pub fn new(names: &[String], case: bool) -> ListMatcher {
        let mut m = ListMatcher {
            case,
            names: HashSet::new(),
            paths: HashSet::new(),
            globs: Vec::new(),
        };
        for n in names {
            let key = if case {
                n.as_bytes().to_vec()
            } else {
                folded(n.as_bytes())
            };
            let path = n.contains('\\');
            if has_wildcard(n) {
                m.globs.push((key, path));
            } else if path {
                m.paths.insert(key);
            } else {
                m.names.insert(key);
            }
        }
        m
    }

    /// Whether the row at `path` (UTF-8, a folder's without its trailing
    /// `\`) is one of the names.
    pub fn matches(&self, path: &[u8]) -> bool {
        let name = name_of(path);
        let (mut n, mut p) = (None, None);
        if !self.names.is_empty()
            && self
                .names
                .contains(n.get_or_insert_with(|| key(self.case, name)).as_ref())
        {
            return true;
        }
        if !self.paths.is_empty()
            && self
                .paths
                .contains(p.get_or_insert_with(|| key(self.case, path)).as_ref())
        {
            return true;
        }
        self.globs.iter().any(|(g, on_path)| {
            if *on_path {
                glob(g, p.get_or_insert_with(|| key(self.case, path)))
            } else {
                glob(g, n.get_or_insert_with(|| key(self.case, name)))
            }
        })
    }
}

/// `text` as the sets hold it: as it is when the case counts, else folded.
fn key(case: bool, text: &[u8]) -> Cow<'_, [u8]> {
    if case {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(folded(text))
    }
}

/// A search over an open file list: the search without its `filelist:`
/// terms (each made a space), for goz's parser, and each term's names.
pub fn split_for_list(query: &str, slots: &Slots) -> Result<(String, Vec<Vec<String>>), String> {
    let found = terms(query);
    if found.is_empty() {
        return Ok((query.to_owned(), Vec::new()));
    }
    let mut rest = String::with_capacity(query.len());
    let mut lists = Vec::with_capacity(found.len());
    let mut at = 0;
    for t in &found {
        rest.push_str(&query[at..t.start]);
        rest.push(' ');
        lists.push(names(t, slots)?);
        at = t.end;
    }
    rest.push_str(&query[at..]);
    Ok((rest, lists))
}

/// Why gozd cannot be asked a `filelist:` search when it reads no OR.
pub const NEEDS_OR: &str = "filelist: needs a gozd that reads OR (|), and the gozd running \
     (goz v0.6.0) does not, so it is not asked; filelist: works in an open file list \
     (File > Open File List...)";

/// The search as gozd is asked it: as it is without a `filelist:` term;
/// with each one as the group of its names when gozd reads OR, groups and
/// `whole:` (`boolean`); else an error saying why it cannot be asked.
pub fn for_gozd<'a>(query: &'a str, slots: &Slots, boolean: bool) -> Result<Cow<'a, str>, String> {
    // Most searches: no function of that name at all.
    if !query
        .as_bytes()
        .windows(8)
        .any(|w| w.eq_ignore_ascii_case(b"filelist"))
    {
        return Ok(Cow::Borrowed(query));
    }
    let found = terms(query);
    if found.is_empty() {
        return Ok(Cow::Borrowed(query));
    }
    if !boolean {
        return Err(NEEDS_OR.into());
    }
    let mut out = String::with_capacity(query.len() + 64);
    let mut at = 0;
    for t in &found {
        out.push_str(&query[at..t.start]);
        let names = names(t, slots)?;
        if names.len() > GOZD_MAX_TERMS {
            let f = t
                .slot
                .map_or_else(|| "filelist:".to_owned(), |n| format!("filelist{n}:"));
            return Err(format!(
                "{f} holds {} names, and gozd searches at most {GOZD_MAX_TERMS} terms at once; \
                 open them as a file list instead (Tools > File List Editor...)",
                crate::format::grouped(names.len() as u64)
            ));
        }
        out.push_str(&gozd_group(&names));
        at = t.end;
    }
    out.push_str(&query[at..]);
    Ok(Cow::Owned(out))
}

/// The names as one group gozd reads: `whole:` for a whole name or path,
/// a quoted wildcard as it is (a wildcard is matched whole already).
fn gozd_group(names: &[String]) -> String {
    if names.is_empty() {
        // No name is empty: nothing matches.
        return "whole:\"\"".into();
    }
    let items: Vec<String> = names
        .iter()
        .map(|n| {
            if has_wildcard(n) {
                format!("\"{n}\"")
            } else {
                format!("whole:\"{n}\"")
            }
        })
        .collect();
    format!("<{}>", items.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots() -> Slots {
        let mut s = Slots::default();
        s.set(1, vec![r"C:\x\a.txt".into(), r"C:\x\sub\".into()]);
        s.set(9, vec!["*.log".into()]);
        s
    }

    #[test]
    fn terms_are_found_where_a_term_may_start() {
        let t = terms(r#"abc filelist:"a b.txt;c.txt" !FileList3: <filelist:x|y> "filelist:no""#);
        assert_eq!(t.len(), 3, "{t:?}");
        assert_eq!((t[0].slot, t[0].value.as_str()), (None, "a b.txt;c.txt"));
        assert_eq!(
            &r#"abc filelist:"a b.txt;c.txt""#[t[0].start..t[0].end],
            r#"filelist:"a b.txt;c.txt""#
        );
        assert_eq!((t[1].slot, t[1].value.as_str()), (Some(3), ""));
        // An unquoted `|` or `>` ends the value.
        assert_eq!(t[2].value, "x");
        assert!(terms("myfilelist:x filelistx: filelist12:").is_empty());
        assert!(terms(r#""a filelist:x""#).is_empty(), "inside quotes");
    }

    #[test]
    fn names_of_a_term_or_its_slot() {
        let s = slots();
        let t = |q: &str| terms(q).remove(0);
        assert_eq!(
            names(&t(r#"filelist:" a.txt ;C:/x/y/;|""#), &s).unwrap(),
            ["a.txt", r"C:\x\y"]
        );
        assert_eq!(names(&t(r#"filelist:"a|b""#), &s).unwrap(), ["a", "b"]);
        assert_eq!(names(&t("filelist:D:\\"), &s).unwrap(), [r"D:\"]);
        assert_eq!(
            names(&t("filelist1:"), &s).unwrap(),
            [r"C:\x\a.txt", r"C:\x\sub"]
        );
        assert!(names(&t("filelist2:"), &s).unwrap().is_empty());
        assert!(names(&t("filelist1:x"), &s).unwrap_err().contains("slot 1"));
    }

    #[test]
    fn rows_match_whole_names_paths_and_wildcards() {
        let m = ListMatcher::new(
            &[
                "Report.PDF".into(),
                r"C:\x\a.txt".into(),
                "*.log".into(),
                r"D:\data\*.csv".into(),
                r"E:\".into(),
            ],
            false,
        );
        assert!(m.matches(br"Q:\any\where\report.pdf"));
        assert!(!m.matches(br"Q:\any\report.pdf.bak"));
        assert!(m.matches(br"c:\X\A.TXT"));
        assert!(!m.matches(br"C:\y\a.txt"), "a path names one item");
        assert!(m.matches(br"C:\logs\today.LOG"));
        assert!(m.matches(br"D:\data\sub\x.csv") && !m.matches(br"D:\other\x.csv"));
        assert!(m.matches(br"E:\"), "a drive's root");
        let case = ListMatcher::new(&["Report.PDF".into()], true);
        assert!(case.matches(br"C:\Report.PDF") && !case.matches(br"C:\report.pdf"));
        assert!(!ListMatcher::new(&[], false).matches(br"C:\a"));
    }

    #[test]
    fn a_list_search_loses_its_terms() {
        let s = slots();
        let (rest, lists) = split_for_list("ext:txt filelist1: size:<1kb", &s).unwrap();
        assert_eq!(rest, "ext:txt   size:<1kb");
        assert_eq!(
            lists,
            [vec![r"C:\x\a.txt".to_owned(), r"C:\x\sub".to_owned()]]
        );
        let (rest, lists) = split_for_list("plain text", &s).unwrap();
        assert_eq!((rest.as_str(), lists.len()), ("plain text", 0));
        assert!(split_for_list("filelist4:x", &s).is_err());
    }

    #[test]
    fn gozd_gets_a_group_or_a_reason() {
        let s = slots();
        assert!(matches!(
            for_gozd("a b", &s, false),
            Ok(Cow::Borrowed("a b"))
        ));
        assert!(matches!(
            for_gozd(r#""filelist:x""#, &s, false),
            Ok(Cow::Borrowed(_))
        ));
        assert_eq!(for_gozd("x filelist:a", &s, false).unwrap_err(), NEEDS_OR);
        assert_eq!(
            for_gozd(r#"!filelist:"a.txt;C:\b c\*.log" ext:txt"#, &s, true).unwrap(),
            r#"!<whole:"a.txt"|"C:\b c\*.log"> ext:txt"#
        );
        assert_eq!(
            for_gozd("filelist1: | filelist9: filelist2:", &s, true).unwrap(),
            r#"<whole:"C:\x\a.txt"|whole:"C:\x\sub"> | <"*.log"> whole:"""#
        );
        let mut big = Slots::default();
        big.set(
            0,
            (0..=GOZD_MAX_TERMS).map(|i| format!("{i}.txt")).collect(),
        );
        let why = for_gozd("filelist0:", &big, true).unwrap_err();
        assert!(
            why.contains("257 names") && why.contains("filelist0:"),
            "{why}"
        );
        assert_eq!(cmd::slot_of(4853), Some(3));
        assert_eq!(cmd::slot_of(4860), None);
    }
}
