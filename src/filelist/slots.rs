//! The `filelist:` search function and the file-list slots (wave 6), with
//! Everything 1.5's other slot functions (wave 7), as Everything has them.
//!
//! `filelist:<names>` matches the items named: a name without a `\`
//! matches an item of that whole name (any folder), a path with a `\`
//! that whole full path; `*` and `?` make a wildcard of either
//! (`super::wild`). Names are separated by `;`; a `;` in quotes is part of
//! a name, as Everything documents (`filelist:a.txt;"b;c.txt"`: two
//! names). A `|`, which no name holds, separates them too (it can only be
//! in quotes: `filelist:"a.txt|C:\b c\d.txt"`, Everything 1.4's form); an
//! unquoted `|` or `>` ends the value, as an operator. Case is ignored
//! unless the search matches case (the Match Case toggle, or a `case:` in
//! the search); Match Path (`matchpath:`, `path:`) does not change what a
//! name matches, as in Everything.
//!
//! `filelist0:` .. `filelist9:` match the names in that slot of the tab:
//! File | Set File List Slot 0..9 (keyboard commands, Everything's
//! `file_set_file_list_slot_N_keys`) puts the full paths of the selected
//! results in it. Each tab has its own ten, kept in `Session.json` as
//! Everything keeps them (`filelist_slot0`..`9`, arrays of names). An
//! empty slot matches nothing.
//!
//! W7: the rest of Everything's slot functions ([`Func`]), each spelt as
//! Everything spells it, with or without its hyphens (`parent-filelist1:`,
//! `parentfilelist1:`, `parent-file-list-1:`). With the items "found" being
//! the items of the index (or of the open file list) that `filelistN:`
//! matches:
//! - `exists-in-filelistN:<name>`: the name, with `$name:`, `$stem:`,
//!   `$extension:`, `$path:`, `$full-path:` or `$parent-name:` put in from
//!   each item, is in the slot (`exists-in-filelist1:$stem:.srt`); a name
//!   with a `\` is a full path, else a whole name;
//! - `parent-filelistN:` / `ancestor-filelistN:`: the item's folder / one
//!   of the folders above it is in the slot (by full path or name);
//! - `child-filelistN:` (and `child-filelist:<names>`): folders holding a
//!   found item; `descendant-filelistN:`: folders above a found item;
//!   `sibling-filelistN:`: items in a found item's folder, but that item;
//! - `size-filelistN:`: files of a found file's size;
//! - `name-filelistN:` / `stem-filelistN:`: items whose name / stem (the
//!   name to its last dot; a folder's is its name) is a slot item's;
//! - `save-filelistN:`: no test: the search's results go in slot N
//!   (Everything's "Save the current results to the specified file list
//!   slot"); the window does that ([`saves`]).
//!
//! Where it is answered:
//! - **An open file list**: by goz-gui ([`split_for_list`], [`RowTests`]):
//!   the terms are taken out of the search, the rest goes to goz's parser
//!   as before, and a row must match every term too. The functions that
//!   need the found items look at every row once first.
//! - **gozd's index** ([`for_gozd`]): gozd has no such functions (v0.6.0
//!   and the query-language branch both refuse them as unknown). A gozd
//!   that reads OR, groups and `whole:` (the query-language branch, which
//!   goz-gui's probe finds) gets each term as the same search in its own
//!   words: `filelist:` as the group of its names
//!   (`<whole:"a.txt"|whole:"C:\x\b.txt"|"*.log">`), `parent-` as
//!   `parent:` / `parent-name:`, `ancestor-` as `ancestor:` / a path
//!   wildcard, `name-` / `stem-` as whole names / `stem:`; the found items'
//!   functions (`child-`, `descendant-`, `sibling-`, `size-`) look the
//!   slot's items up in the index first ([`Looked`], one search) and send
//!   their folders, sizes and paths. An empty list is `whole:""`, which
//!   matches nothing; at most 256 terms, gozd's own limit on a search. What
//!   gozd cannot be asked exactly (each result tested against the slot:
//!   `exists-in-`; a wildcard path under `parent-`/`ancestor-`; more found
//!   items than one look-up takes) is refused with a precise reason rather
//!   than answered wrongly, and so is all of it with the deployed v0.6.0
//!   gozd, which cannot say "any of these". gozd never lists a drive's root
//!   itself (`C:\`), so `child-`/`descendant-` leave roots out.
//!
//! W8: the last of Everything's file-list functions, as voidtools documents
//! them (the "Search functions" topic, t=10176, and "Combining multiple
//! searches", t=17882):
//! - `new-search:` (also `newsearch:`) starts a new search: the search is
//!   run in parts, left to right ([`parts`], [`resolve`]); a part's results
//!   are thrown away when the next one starts, so only what its
//!   `save-filelistN:` terms save to a slot lives on, and the last part's
//!   results are the answer. Everything's example `*.mp3 save-filelist1:
//!   new-search: childfilelist1:` is one search: the folders holding an mp3.
//!   The window runs each part with its filter, Match Path and folder, as a
//!   search of its own would be run;
//! - `include-file-list:<file>`: the items named in a list on the disk (an
//!   EFU or CSV's `Filename` column, or a name a line), matched as
//!   `filelist:` matches its names ([`include_names`]); the file's full path
//!   is needed (a relative one is never looked for in goz-gui's current
//!   folder);
//! - `open-filelist:<file>`: the search runs over that file list instead of
//!   the index or the tab's list, for this search only ([`open_list`]; the
//!   window opens it);
//! - `filelist-filename:<names>`: the items that belong to a file list of
//!   that file name (matched as `filelist:` matches a name or a full path; an
//!   empty value: any file list). Every row of an open file list belongs to
//!   it; no item of gozd's index belongs to any.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::matcher::name_of;
use super::wild::{folded, glob, has_wildcard};

/// The number of slots, as Everything's.
pub const COUNT: usize = 10;

/// W7: the most results `save-filelistN:` puts in a slot (each is kept in
/// `Session.json` too).
pub const SAVE_MAX: usize = 100_000;

/// gozd's most terms in one search (the query-language branch's
/// `MAX_QUERY_TERMS`).
pub const GOZD_MAX_TERMS: usize = 256;

/// The most items one look-up in gozd's index takes for the found items'
/// functions.
pub const LOOKUP_MAX: usize = 4096;

/// W8R-4: how much of an `include-file-list:` file a search reads: at
/// most `cap` names, and only while `stale` says the search is still
/// wanted (asked every 4,096 rows).
#[derive(Clone, Copy)]
pub struct Reading<'a> {
    pub cap: usize,
    pub stale: &'a dyn Fn() -> bool,
}

impl Reading<'_> {
    /// All of the file, however long it takes.
    pub const WHOLE: Reading<'static> = Reading {
        cap: usize::MAX,
        stale: &never,
    };
}

/// A search that is always wanted.
fn never() -> bool {
    false
}

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

/// Everything's file-list functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    /// `filelist:` / `filelistN:`: the items named.
    Items,
    /// `exists-in-filelistN:<name>`.
    ExistsIn,
    /// `parent-filelistN:`.
    Parent,
    /// `child-filelist:` / `child-filelistN:`.
    Child,
    /// `sibling-filelistN:`.
    Sibling,
    /// `ancestor-filelistN:`.
    Ancestor,
    /// `descendant-filelistN:`.
    Descendant,
    /// `size-filelistN:`.
    Size,
    /// `name-filelistN:`.
    Name,
    /// `stem-filelistN:`.
    Stem,
    /// `save-filelistN:`.
    Save,
    /// W8: `include-file-list:<file>`: the items a list on the disk names.
    Include,
    /// W8: `open-filelist:<file>`: the search runs over that file list.
    Open,
    /// W8: `filelist-filename:<names>`: items of a file list so named.
    ListName,
}

/// Whether a function takes a slot (`filelist1:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotRule {
    /// Only with a slot (`parent-filelist1:`).
    Needed,
    /// With a slot or a value of its own (`filelist:`, `filelist1:`).
    Either,
    /// Never: its value is its own (`include-file-list:x.efu`).
    Never,
}

impl Func {
    /// The function a name stands for (any case, hyphens and underscores
    /// left out), and whether it takes a slot.
    fn named(base: &str) -> Option<(Func, SlotRule)> {
        Some(match base {
            "filelist" => (Func::Items, SlotRule::Either),
            "existsinfilelist" => (Func::ExistsIn, SlotRule::Needed),
            "parentfilelist" => (Func::Parent, SlotRule::Needed),
            "childfilelist" => (Func::Child, SlotRule::Either),
            "siblingfilelist" => (Func::Sibling, SlotRule::Needed),
            "ancestorfilelist" => (Func::Ancestor, SlotRule::Needed),
            "descendantfilelist" => (Func::Descendant, SlotRule::Needed),
            "sizefilelist" => (Func::Size, SlotRule::Needed),
            "namefilelist" => (Func::Name, SlotRule::Needed),
            "stemfilelist" => (Func::Stem, SlotRule::Needed),
            "savefilelist" => (Func::Save, SlotRule::Needed),
            // W8: Everything.exe's own spellings are `include-filelist`,
            // `includefilelist:`, `open-filelist` and `filelist-filename`;
            // its documentation's `include-file-list:` is the same name.
            "includefilelist" => (Func::Include, SlotRule::Never),
            "openfilelist" => (Func::Open, SlotRule::Never),
            "filelistfilename" => (Func::ListName, SlotRule::Never),
            _ => return None,
        })
    }

    /// W8: whether Ctrl+click and F2 on such a term open something: a
    /// slot's or a list's names (Edit File List Slot), or a file to pick
    /// (`include-file-list:`, `open-filelist:`, Everything's "Ctrl + click
    /// the include-file-list: text in the search box to select a file").
    pub fn editable(self) -> bool {
        self != Func::ListName
    }

    /// W8: whether its value names a file on the disk to pick.
    pub fn picks_file(self) -> bool {
        matches!(self, Func::Include | Func::Open)
    }
}

/// A file-list term of a search: where it is (bytes `start..end`, from its
/// function name to the end of its value), its function as typed (with its
/// colon), its slot (`filelistN:`) and its value, quotes taken off (`raw`:
/// as typed, quotes and all, which say where its names part).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub start: usize,
    pub end: usize,
    pub func: Func,
    pub name: String,
    pub slot: Option<usize>,
    pub value: String,
    pub raw: String,
}

/// A file-list function at the start of `b`: the function, its slot, and
/// how long its name is with its colon.
fn head(b: &[u8]) -> Option<(Func, Option<usize>, usize)> {
    // The longest name, hyphens and all, is well under 40 bytes.
    let colon = b.iter().take(40).position(|&c| c == b':')?;
    let raw = &b[..colon];
    if raw.is_empty()
        || !raw
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return None;
    }
    let mut base: String = raw
        .iter()
        .filter(|c| !matches!(c, b'-' | b'_'))
        .map(|&c| char::from(c.to_ascii_lowercase()))
        .collect();
    let slot = match base.as_bytes().last() {
        Some(d @ b'0'..=b'9') => {
            let n = usize::from(d - b'0');
            base.pop();
            Some(n)
        }
        _ => None,
    };
    let (func, rule) = Func::named(&base)?;
    match (rule, slot) {
        (SlotRule::Needed, None) | (SlotRule::Never, Some(_)) => None,
        _ => Some((func, slot, colon + 1)),
    }
}

/// The file-list terms of `query`: each where a term may start (the
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
            && let Some((func, slot, len)) = head(&b[i..])
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
                func,
                name: query[i..from].to_owned(),
                slot,
                value: query[from..j].replace('"', ""),
                raw: query[from..j].to_owned(),
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

/// The term of `query` at UTF-16 position `unit` (the search box's caret,
/// or the character under the mouse), from its function's first character
/// to its value's last.
pub fn term_at(query: &str, unit: usize) -> Option<Term> {
    let mut units = 0usize;
    let mut byte = query.len();
    for (i, ch) in query.char_indices() {
        if units >= unit {
            byte = i;
            break;
        }
        units += ch.len_utf16();
    }
    terms(query)
        .into_iter()
        .find(|t| t.start <= byte && byte <= t.end)
}

/// The search with `term`'s value made `names` (an inline list's: the
/// File List editor's OK), each name quoted when it needs it (white space,
/// a `;`, `<` or `>`); `None` when a name cannot be written in a search (it
/// holds a `"` or a `|`, which no file's name holds).
pub fn with_names(query: &str, term: &Term, names: &[String]) -> Option<String> {
    if names.iter().any(|n| n.contains(['"', '|'])) {
        return None;
    }
    let mut out = String::with_capacity(query.len() + names.len() * 16);
    out.push_str(&query[..term.start]);
    out.push_str(&term.name);
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            out.push(';');
        }
        if n.chars()
            .any(|c| c.is_whitespace() || matches!(c, ';' | '<' | '>'))
        {
            out.push('"');
            out.push_str(n);
            out.push('"');
        } else {
            out.push_str(n);
        }
    }
    out.push_str(&query[term.end..]);
    Some(out)
}

/// An inline list's names as typed (`raw`): parted at each `;` out of
/// quotes and at each `|` (no name holds one, and an unquoted one ended the
/// value already), each one's quotes taken off.
fn split_names(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut name = String::new();
    let mut quoted = false;
    for c in raw.chars() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => out.push(std::mem::take(&mut name)),
            '|' => out.push(std::mem::take(&mut name)),
            _ => name.push(c),
        }
    }
    out.push(name);
    out
}

/// Whether a search matches case: its last `case:` directive (`case:` or
/// `case:1` on, `case:0` off, as goz's parsers read it: a word of its own,
/// its value quoted or not), else the Match Case toggle `match_case`.
pub fn search_case(query: &str, match_case: bool) -> bool {
    let b = query.as_bytes();
    let mut case = match_case;
    let mut quoted = false;
    let mut at_start = true;
    let mut i = 0;
    while i < b.len() {
        if !quoted && at_start && b.len() - i >= 5 && b[i..i + 5].eq_ignore_ascii_case(b"case:") {
            let mut j = i + 5;
            let mut q = false;
            let mut value = Vec::new();
            while j < b.len() {
                match b[j] {
                    b'"' => q = !q,
                    d if !q && (d.is_ascii_whitespace() || d == b'|' || d == b'>') => break,
                    d => value.push(d),
                }
                j += 1;
            }
            match value.as_slice() {
                b"" | b"1" => case = true,
                b"0" => case = false,
                // `case:Foo`: one term's modifier, not the search's.
                _ => {}
            }
            i = j;
            at_start = false;
            continue;
        }
        let c = b[i];
        if c == b'"' {
            quoted = !quoted;
        }
        at_start = !quoted && (c.is_ascii_whitespace() || matches!(c, b'!' | b'<' | b'|'));
        i += 1;
    }
    case
}

/// A name as a list holds it: trimmed, `/` made `\`, a folder's trailing
/// `\` taken off (a drive's root keeps its own: `C:\`).
pub fn normalize(name: &str) -> Option<String> {
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
/// slot's term with a value of its own (but `exists-in-filelistN:`, whose
/// value is the name it looks for). W8R-4: an `include-file-list:` file is
/// read as `reading` says ([`NotAsked::Stale`] when the search went stale).
pub fn names<E>(term: &Term, slots: &Slots, reading: Reading) -> Result<Vec<String>, NotAsked<E>> {
    // W8: the names in a list on the disk.
    if term.func == Func::Include {
        return include_names(term, reading);
    }
    match term.slot {
        Some(n) if term.func != Func::ExistsIn && !term.value.trim().is_empty() => Err(format!(
            "{} takes no value: it works with the names in file list slot {n} \
                 (File | Set File List Slot {n} puts the selected results there)",
            term.name
        )
        .into()),
        Some(n) => Ok(slots.get(n).iter().filter_map(|s| normalize(s)).collect()),
        None => Ok(split_names(&term.raw)
            .iter()
            .filter_map(|n| normalize(n))
            .collect()),
    }
}

/// W8: the search with `term`'s value made the file `path` (in quotes),
/// as Ctrl+click on `include-file-list:` or `open-filelist:` picks it.
pub fn with_file(query: &str, term: &Term, path: &str) -> String {
    format!(
        "{}{}\"{path}\"{}",
        &query[..term.start],
        term.name,
        &query[term.end..]
    )
}

/// W8: whether `path` is a full path (`C:\x`, `\\server\share\x`).
fn is_full_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with("\\\\")
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
}

/// W8: a function's file (`include-file-list:`, `open-filelist:`): its
/// value, `/` made `\`; an error unless it is a full path.
fn file_of(term: &Term) -> Result<String, String> {
    let path = term.value.trim().replace('/', "\\");
    if path.is_empty() {
        return Err(format!(
            "{} needs a file list's full path (Ctrl+click it to pick one)",
            term.name
        ));
    }
    if !is_full_path(&path) {
        return Err(format!(
            "{} needs the file list's full path, as C:\\Lists\\{path}: a relative name is \
             not looked for in goz-gui's current folder",
            term.name
        ));
    }
    Ok(path)
}

/// W8: the names `include-file-list:<file>` stands for: each row's
/// `Filename` of an EFU or CSV (Everything's "CSV and EFU files must be an
/// Everything File List with the Filename column"), or each line of any
/// other list ("TXT files must specify a filename on each line. Use a path
/// separator to specify the full path"): there a name with a `\` is a full
/// path, any other a whole name, as `filelist:` reads them. W8R-3: an EFU's
/// or CSV's relative path is put under the list's folder, as Open File
/// List, `open-filelist:` and the File List Editor read the same file
/// (Everything writes such lists with `file_list_relative_paths`).
///
/// W8R-4: at most `reading.cap` names are read; a search gone stale stops
/// the read ([`NotAsked::Stale`]).
fn include_names<E>(term: &Term, reading: Reading) -> Result<Vec<String>, NotAsked<E>> {
    let path = file_of(term)?;
    let cannot = |e: std::io::Error| format!("{} cannot read {path}: {e}", term.name);
    if std::fs::metadata(&path).map_err(cannot)?.is_dir() {
        return Err(format!("{} names the folder {path}, not a file list", term.name).into());
    }
    let file = std::fs::File::open(&path).map_err(cannot)?;
    let reader = super::utf8_reader(file).map_err(cannot)?;
    let folder = std::path::absolute(&path)
        .ok()
        .and_then(|p| p.parent().map(|f| f.display().to_string()))
        .unwrap_or_default();
    let mut out = Vec::new();
    let read = std::cell::Cell::new(0usize);
    let stale = std::cell::Cell::new(false);
    let stop = || {
        read.get() >= reading.cap || {
            stale.set((reading.stale)());
            stale.get()
        }
    };
    super::parse::each_row_in(reader, &folder, "", &|_| None, &stop, |p, _, _, _, _| {
        if read.get() < reading.cap
            && let Some(n) = normalize(p)
        {
            out.push(n);
            read.set(out.len());
        }
    })
    .map_err(cannot)?;
    if stale.get() {
        return Err(NotAsked::Stale);
    }
    Ok(out)
}

/// W8: the files the search's `include-file-list:` terms read, as their
/// stamps (length and time last written; `None` when it cannot be read): an
/// answer kept for the search is good while they are the same.
pub fn include_stamps(query: &str) -> Vec<Option<(u64, std::time::SystemTime)>> {
    if !may_hold(query) {
        return Vec::new();
    }
    terms(query)
        .iter()
        .filter(|t| t.func == Func::Include)
        .map(|t| {
            let m = std::fs::metadata(file_of(t).ok()?).ok()?;
            Some((m.len(), m.modified().ok()?))
        })
        .collect()
}

/// W8: the file list an `open-filelist:` term of `query` opens for the
/// search (its full path); `None` without one; an error for a relative or
/// empty path, or two terms naming two lists.
pub fn open_list(query: &str) -> Option<Result<String, String>> {
    if !may_hold(query) {
        return None;
    }
    let mut found: Option<String> = None;
    for t in terms(query).iter().filter(|t| t.func == Func::Open) {
        let path = match file_of(t) {
            Ok(p) => p,
            Err(why) => return Some(Err(why)),
        };
        match &found {
            Some(p) if !p.eq_ignore_ascii_case(&path) => {
                return Some(Err(format!(
                    "{} names a second file list ({path} after {p}); a search runs over one",
                    t.name
                )));
            }
            Some(_) => {}
            None => found = Some(path),
        }
    }
    found.map(Ok)
}

/// W8: whether the search reads a file of the disk (`include-file-list:`)
/// or runs in parts (`new-search:`): what gozd is asked for it is worked
/// out once for all its blocks.
pub fn is_costly(query: &str) -> bool {
    new_searches(query).is_ok_and(|cuts| !cuts.is_empty())
        || (may_hold(query) && terms(query).iter().any(|t| t.func == Func::Include))
}

/// W8: a `new-search:` function at the start of `b` (any case, hyphens
/// and underscores left out): how long its name is with its colon.
fn new_search_head(b: &[u8]) -> Option<usize> {
    let colon = b.iter().take(16).position(|&c| c == b':')?;
    let base: Vec<u8> = b[..colon]
        .iter()
        .filter(|c| !matches!(c, b'-' | b'_'))
        .map(u8::to_ascii_lowercase)
        .collect();
    (base == b"newsearch").then_some(colon + 1)
}

/// W8: whether `query` may hold a `new-search:` term at all (every
/// spelling ends `search:`): most searches do not, and cost no more.
fn has_new_search(query: &str) -> bool {
    query
        .as_bytes()
        .windows(7)
        .any(|w| w.eq_ignore_ascii_case(b"search:"))
}

/// W8: where the search's `new-search:` terms are (bytes `start..end`):
/// each where a term may start, not in quotes; an error for one with a
/// value, inside `< >`, or after `!` or `|` (a new search is no term of the
/// search before it).
fn new_searches(query: &str) -> Result<Vec<(usize, usize)>, String> {
    let mut out = Vec::new();
    if !has_new_search(query) {
        return Ok(out);
    }
    let b = query.as_bytes();
    let mut quoted = false;
    let mut at_start = true;
    let mut depth = 0usize;
    // The last byte outside quotes that is not white space.
    let mut last = 0u8;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !quoted
            && at_start
            && let Some(len) = new_search_head(&b[i..])
        {
            let name = &query[i..i + len];
            let mut j = i + len;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'|' && b[j] != b'>' {
                j += 1;
            }
            if j > i + len {
                return Err(format!(
                    "{name} takes no value: it starts a new search, which the text after it is"
                ));
            }
            if depth > 0 || matches!(last, b'!' | b'|' | b'<') {
                return Err(format!(
                    "{name} starts a new search, so it cannot be inside < >, after ! or in an \
                     OR (|)"
                ));
            }
            out.push((i, j));
            i = j;
            at_start = false;
            last = b':';
            continue;
        }
        if c == b'"' {
            quoted = !quoted;
        } else if !quoted {
            // A group opens where a term may start (`size:<1mb` is no
            // group), and closes after a term.
            match c {
                b'<' if at_start => depth += 1,
                b'>' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if !quoted && !c.is_ascii_whitespace() {
            last = c;
        }
        at_start = !quoted && (c.is_ascii_whitespace() || matches!(c, b'!' | b'<' | b'|'));
        i += 1;
    }
    Ok(out)
}

/// W8: the parts of a search that `new-search:` starts anew, in order
/// (one, the search itself, without one).
pub fn parts(query: &str) -> Result<Vec<&str>, String> {
    let cuts = new_searches(query)?;
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut at = 0;
    for (start, end) in cuts {
        out.push(&query[at..start]);
        at = end;
    }
    out.push(&query[at..]);
    Ok(out)
}

/// W8: the search's last part, the one whose results show (all of it
/// without `new-search:`; all of it too when a `new-search:` is misplaced,
/// which [`resolve`] refuses).
pub fn last_part(query: &str) -> &str {
    match parts(query) {
        Ok(p) => p.last().copied().unwrap_or(query),
        Err(_) => query,
    }
}

/// W8: `text` with each of its parts made into a search by `make` (the
/// window's filter, Match Path and folder), joined again by `new-search:`;
/// as `make(text)` without one (or with a misplaced one, which is refused
/// when it is searched).
pub fn each_part(text: &str, mut make: impl FnMut(&str) -> String) -> String {
    match parts(text) {
        Ok(p) if p.len() > 1 => p
            .iter()
            .map(|part| make(part.trim()))
            .collect::<Vec<_>>()
            .join(" new-search: "),
        _ => make(text),
    }
}

/// W8: what the parts before a search's last saved: the slots after them,
/// and each slot saved with how many items it holds now, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartSaves {
    pub slots: Arc<Slots>,
    pub saved: Vec<(usize, usize)>,
}

/// W8: a search made ready to run: its last part, and the slots it reads,
/// which its earlier parts' `save-filelistN:` terms filled; what they saved
/// (`None` without an earlier part that saves).
#[derive(Debug)]
pub struct Resolved<'a> {
    pub last: &'a str,
    pub slots: Arc<Slots>,
    pub saves: Option<PartSaves>,
}

/// W8: what answers a part of a search for [`resolve`]: its results' full
/// paths, with the slots the parts before it left.
pub type RunPart<'r, E> = dyn FnMut(&str, &Arc<Slots>) -> Result<Vec<String>, NotAsked<E>> + 'r;

/// W8: runs the parts of `query` before its last, left to right, each with
/// the slots the parts before it left (`slots` first): `run` answers a part
/// with all its results' full paths (it may stop after [`SAVE_MAX`] + 1),
/// which go in the slots its `save-filelistN:` terms name. A part that
/// saves nothing is not run: its results are thrown away by the next one,
/// as Everything's are. An error when a part cannot be run, or holds more
/// results than a slot is given.
pub fn resolve<'a, E>(
    query: &'a str,
    slots: &Arc<Slots>,
    run: &mut RunPart<'_, E>,
) -> Result<Resolved<'a>, NotAsked<E>> {
    let all = parts(query)?;
    let (last, before) = all.split_last().expect("a search has one part at least");
    let mut now = Arc::clone(slots);
    let mut saved: Vec<(usize, usize)> = Vec::new();
    for part in before {
        let saves = saves(part)?;
        if saves.is_empty() {
            continue;
        }
        let paths = run(part, &now)?;
        if paths.len() > SAVE_MAX {
            return Err(NotAsked::Refused(format!(
                "{} before new-search: saves at most {} results, and its search has more, so \
                 the search after it is not run",
                saves
                    .iter()
                    .map(|n| format!("save-filelist{n}:"))
                    .collect::<Vec<_>>()
                    .join(" and "),
                crate::format::grouped(SAVE_MAX as u64)
            )));
        }
        let mut next = (*now).clone();
        for &n in &saves {
            next.set(n, paths.clone());
            saved.retain(|&(s, _)| s != n);
            saved.push((n, paths.len()));
        }
        now = Arc::new(next);
    }
    let saves = (!saved.is_empty()).then(|| PartSaves {
        slots: Arc::clone(&now),
        saved,
    });
    Ok(Resolved {
        last,
        slots: now,
        saves,
    })
}

/// The slots the search's `save-filelistN:` terms put its results in, in
/// their order, each once; an error when the search also reads a slot it
/// saves (its answer would change what it answers).
pub fn saves(query: &str) -> Result<Vec<usize>, String> {
    if !may_hold(query) {
        return Ok(Vec::new());
    }
    let found = terms(query);
    let mut out = Vec::new();
    for t in &found {
        if t.func == Func::Save
            && let Some(n) = t.slot
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    for t in &found {
        if t.func != Func::Save
            && let Some(n) = t.slot
            && out.contains(&n)
        {
            return Err(format!(
                "{} reads file list slot {n}, which this search's save-filelist{n}: \
                 fills; search for one in another search",
                t.name
            ));
        }
    }
    Ok(out)
}

/// Whether `query` may hold a file-list function at all: most searches
/// do not, and cost no more.
fn may_hold(query: &str) -> bool {
    let b = query.as_bytes();
    b.windows(4).any(|w| w.eq_ignore_ascii_case(b"list"))
}

/// The name part of an item's path (a drive's root: all of it).
fn name_part(path: &str) -> &str {
    match path.rfind('\\') {
        Some(i) if i + 1 < path.len() => &path[i + 1..],
        _ => path,
    }
}

/// A file's stem as Everything parts a name: up to its last dot (a
/// folder's is its whole name).
fn stem_of(name: &[u8], is_dir: bool) -> &[u8] {
    if is_dir {
        return name;
    }
    match memchr::memrchr(b'.', name) {
        Some(dot) => &name[..dot],
        None => name,
    }
}

/// A file's extension: after its last dot (a folder has none).
fn extension_of(name: &[u8], is_dir: bool) -> &[u8] {
    if is_dir {
        return &[];
    }
    match memchr::memrchr(b'.', name) {
        Some(dot) => &name[dot + 1..],
        None => &[],
    }
}

/// The folder holding `path` (`C:\x\a` → `C:\x`, `C:\a` → `C:\`); none for
/// a drive's root or a bare name.
pub fn parent_of(path: &[u8]) -> Option<&[u8]> {
    let i = memchr::memrchr(b'\\', path)?;
    if i + 1 >= path.len() {
        // A root's own backslash (`C:\`).
        return None;
    }
    if i > 0 && path[i - 1] == b':' {
        Some(&path[..=i])
    } else if i == 0 {
        None
    } else {
        Some(&path[..i])
    }
}

/// The folders above `path`, nearest first.
fn ancestors(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut at = Some(path);
    std::iter::from_fn(move || {
        let up = parent_of(at?)?;
        at = Some(up);
        Some(up)
    })
}

/// Whether `path` is a drive's root (`C:\`).
fn is_root(path: &[u8]) -> bool {
    path.len() == 3 && path[1] == b':' && path[2] == b'\\'
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

    /// Whether it names nothing.
    fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty() && self.globs.is_empty()
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

/// An item's property `exists-in-filelistN:` can put in its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prop {
    Name,
    Stem,
    Extension,
    /// The folder it is in.
    Path,
    FullPath,
    ParentName,
}

/// A piece of an `exists-in-filelistN:` value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    Prop(Prop),
}

/// An `exists-in-filelistN:` value read into its pieces: `$name:` and its
/// kind put in an item's property; a `$` not so is itself.
fn substitution(value: &str, func: &str) -> Result<Vec<Piece>, String> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        text.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let word_len = after
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            .count();
        if word_len == 0 || after.as_bytes().get(word_len) != Some(&b':') {
            text.push('$');
            rest = after;
            continue;
        }
        let word: String = after[..word_len]
            .chars()
            .filter(|c| !matches!(c, '-' | '_'))
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let prop = match word.as_str() {
            "name" => Prop::Name,
            "stem" => Prop::Stem,
            "ext" | "extension" => Prop::Extension,
            "path" | "location" => Prop::Path,
            "fullpath" | "pathandname" => Prop::FullPath,
            "parentname" => Prop::ParentName,
            _ => {
                return Err(format!(
                    "{func} cannot put in ${}: (goz-gui puts in $name:, $stem:, $extension:, \
                     $path:, $full-path: and $parent-name:)",
                    &after[..word_len]
                ));
            }
        };
        if !text.is_empty() {
            out.push(Piece::Text(std::mem::take(&mut text)));
        }
        out.push(Piece::Prop(prop));
        rest = &after[word_len + 1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    Ok(out)
}

/// The name `pieces` make for the item at `path`.
fn substitute(pieces: &[Piece], path: &[u8], is_dir: bool, out: &mut Vec<u8>) {
    out.clear();
    let name = name_of(path);
    for p in pieces {
        match p {
            Piece::Text(t) => out.extend_from_slice(t.as_bytes()),
            Piece::Prop(Prop::Name) => out.extend_from_slice(name),
            Piece::Prop(Prop::Stem) => out.extend_from_slice(stem_of(name, is_dir)),
            Piece::Prop(Prop::Extension) => out.extend_from_slice(extension_of(name, is_dir)),
            Piece::Prop(Prop::Path) => {
                if let Some(up) = parent_of(path) {
                    // `$path:\x` makes one backslash after a drive's root.
                    out.extend_from_slice(up.strip_suffix(b"\\").unwrap_or(up));
                }
            }
            Piece::Prop(Prop::FullPath) => out.extend_from_slice(path),
            Piece::Prop(Prop::ParentName) => {
                if let Some(up) = parent_of(path) {
                    out.extend_from_slice(name_of(up));
                }
            }
        }
    }
}

/// A file-list term of a search over an open file list, before the case
/// is known: its function, its names and its value.
#[derive(Clone, Debug)]
pub struct Planned {
    func: Func,
    name: String,
    names: Vec<String>,
    value: String,
}

/// A search over an open file list: the search without its file-list terms
/// (each made a space), for goz's parser, and each term's plan; a
/// `save-filelistN:` term is taken out and tests nothing. `None` when
/// `stale` said the search is not wanted any more while an
/// `include-file-list:` file was read (W8R-4).
#[allow(clippy::type_complexity)]
pub fn split_for_list(
    query: &str,
    slots: &Slots,
    stale: &dyn Fn() -> bool,
) -> Result<Option<(String, Vec<Planned>)>, String> {
    if !may_hold(query) {
        return Ok(Some((query.to_owned(), Vec::new())));
    }
    let found = terms(query);
    if found.is_empty() {
        return Ok(Some((query.to_owned(), Vec::new())));
    }
    let reading = Reading {
        cap: usize::MAX,
        stale,
    };
    saves(query)?;
    let mut rest = String::with_capacity(query.len());
    let mut planned = Vec::with_capacity(found.len());
    let mut at = 0;
    for t in &found {
        rest.push_str(&query[at..t.start]);
        rest.push(' ');
        at = t.end;
        // W8: `open-filelist:` chose the list being searched (the window
        // opened it), and tests nothing of a row.
        if matches!(t.func, Func::Save | Func::Open) {
            continue;
        }
        let names = match names::<()>(t, slots, reading) {
            Ok(names) => names,
            Err(NotAsked::Refused(why)) => return Err(why),
            Err(NotAsked::Stale | NotAsked::Lookup(())) => return Ok(None),
        };
        planned.push(Planned {
            func: t.func,
            name: t.name.clone(),
            names,
            value: t.value.clone(),
        });
    }
    rest.push_str(&query[at..]);
    Ok(Some((rest, planned)))
}

/// One term's test of a row.
enum Test {
    /// `filelist:`: the row is named.
    Items(ListMatcher),
    /// `name-filelist`: its name is a slot item's name.
    Name(ListMatcher),
    /// `stem-filelist`: its stem is a slot item's (whole, or wildcards).
    Stem {
        whole: HashSet<Vec<u8>>,
        globs: Vec<Vec<u8>>,
    },
    /// `parent-filelist`: its folder is named.
    Parent(ListMatcher),
    /// `ancestor-filelist`: a folder above it is named.
    Ancestor(ListMatcher),
    /// `exists-in-filelist`: the name its value makes for it is in the
    /// slot (a full path among the full paths, a name among the names).
    ExistsIn {
        pieces: Vec<Piece>,
        /// The slot's full paths, and its names.
        slot: Box<(ListMatcher, ListMatcher)>,
    },
    /// `child-filelist`: a folder holding a found item (folded paths).
    Child {
        slot: ListMatcher,
        folders: HashSet<Vec<u8>>,
    },
    /// `descendant-filelist`: a folder above a found item.
    Descendant {
        slot: ListMatcher,
        folders: HashSet<Vec<u8>>,
    },
    /// `sibling-filelist`: by folded folder, the first found item there
    /// and whether another one is.
    Sibling {
        slot: ListMatcher,
        folders: HashMap<Vec<u8>, (Vec<u8>, bool)>,
    },
    /// `size-filelist`: a found file's size.
    Size {
        slot: ListMatcher,
        sizes: HashSet<u64>,
    },
    /// W8: `filelist-filename:`: every row of the list is in it, so it is
    /// the same for each (whether the list's file is so named).
    Each(bool),
}

/// The tests a search's file-list terms make of each row of an open file
/// list. Those that need the found items see every row first
/// ([`RowTests::see`]).
pub struct RowTests {
    case: bool,
    tests: Vec<Test>,
    buf: Vec<u8>,
}

impl RowTests {
    /// The tests of `planned`, the case as the search's (goz's parser said
    /// it, `case:` and all), over the file list at `list` (its full path).
    pub fn new(planned: &[Planned], case: bool, list: &str) -> Result<RowTests, String> {
        let mut tests = Vec::with_capacity(planned.len());
        for p in planned {
            let slot = || ListMatcher::new(&p.names, case);
            let name_parts =
                || -> Vec<String> { p.names.iter().map(|n| name_part(n).to_owned()).collect() };
            tests.push(match p.func {
                // W8: a list on the disk's names, as `filelist:`'s own.
                Func::Items | Func::Include => Test::Items(slot()),
                // W8: an empty value: any file list.
                Func::ListName => Test::Each(p.names.is_empty() || slot().matches(list.as_bytes())),
                Func::Open => continue,
                Func::Name => Test::Name(ListMatcher::new(&name_parts(), case)),
                Func::Stem => {
                    let (mut whole, mut globs) = (HashSet::new(), Vec::new());
                    for n in name_parts() {
                        let stem = key(case, stem_of(n.as_bytes(), false)).into_owned();
                        if has_wildcard(&n) {
                            globs.push(stem);
                        } else {
                            whole.insert(stem);
                        }
                    }
                    Test::Stem { whole, globs }
                }
                Func::Parent => Test::Parent(slot()),
                Func::Ancestor => Test::Ancestor(slot()),
                Func::ExistsIn => Test::ExistsIn {
                    pieces: substitution(&p.value, &p.name)?,
                    slot: Box::new((slot(), ListMatcher::new(&name_parts(), case))),
                },
                Func::Child => Test::Child {
                    slot: slot(),
                    folders: HashSet::new(),
                },
                Func::Descendant => Test::Descendant {
                    slot: slot(),
                    folders: HashSet::new(),
                },
                Func::Sibling => Test::Sibling {
                    slot: slot(),
                    folders: HashMap::new(),
                },
                Func::Size => Test::Size {
                    slot: slot(),
                    sizes: HashSet::new(),
                },
                Func::Save => continue,
            });
        }
        Ok(RowTests {
            case,
            tests,
            buf: Vec::new(),
        })
    }

    /// Whether there is a test at all.
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }

    /// Whether a test needs the found items: every row is [`seen`] first.
    ///
    /// [`seen`]: RowTests::see
    pub fn needs_rows(&self) -> bool {
        self.tests.iter().any(|t| {
            matches!(
                t,
                Test::Child { .. }
                    | Test::Descendant { .. }
                    | Test::Sibling { .. }
                    | Test::Size { .. }
            )
        })
    }

    /// A row of the list, before any is tested: a found item's folder,
    /// folders above it and size are kept.
    pub fn see(&mut self, path: &[u8], is_dir: bool, size: Option<u64>) {
        for t in &mut self.tests {
            match t {
                Test::Child { slot, folders } if !slot.is_empty() && slot.matches(path) => {
                    if let Some(up) = parent_of(path) {
                        folders.insert(folded(up));
                    }
                }
                Test::Descendant { slot, folders } if !slot.is_empty() && slot.matches(path) => {
                    for up in ancestors(path) {
                        if !folders.insert(folded(up)) {
                            // Those above it are in already.
                            break;
                        }
                    }
                }
                Test::Sibling { slot, folders } if !slot.is_empty() && slot.matches(path) => {
                    if let Some(up) = parent_of(path) {
                        let me = folded(path);
                        folders
                            .entry(folded(up))
                            .and_modify(|(first, more)| *more |= *first != me)
                            .or_insert((me, false));
                    }
                }
                Test::Size { slot, sizes } if !is_dir && !slot.is_empty() && slot.matches(path) => {
                    if let Some(s) = size {
                        sizes.insert(s);
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether the row at `path` passes every test.
    pub fn matches(&mut self, path: &[u8], is_dir: bool, size: Option<u64>) -> bool {
        let case = self.case;
        let buf = &mut self.buf;
        self.tests.iter().all(|t| match t {
            Test::Items(m) => m.matches(path),
            Test::Name(m) => m.matches(name_of(path)),
            Test::Stem { whole, globs } => {
                let stem = key(case, stem_of(name_of(path), is_dir));
                whole.contains(stem.as_ref()) || globs.iter().any(|g| glob(g, &stem))
            }
            Test::Parent(m) => parent_of(path).is_some_and(|up| m.matches(up)),
            Test::Ancestor(m) => ancestors(path).any(|up| m.matches(up)),
            Test::ExistsIn { pieces, slot } => {
                substitute(pieces, path, is_dir, buf);
                if buf.is_empty() {
                    false
                } else if buf.contains(&b'\\') {
                    slot.0.matches(buf)
                } else {
                    slot.1.matches(buf)
                }
            }
            Test::Child { folders, .. } | Test::Descendant { folders, .. } => {
                !folders.is_empty() && folders.contains(&folded(path))
            }
            Test::Sibling { folders, .. } => {
                !folders.is_empty()
                    && parent_of(path).is_some_and(|up| {
                        folders
                            .get(&folded(up))
                            .is_some_and(|(first, more)| *more || *first != folded(path))
                    })
            }
            Test::Size { sizes, .. } => !is_dir && size.is_some_and(|s| sizes.contains(&s)),
            Test::Each(yes) => *yes,
        })
    }
}

/// Why gozd cannot be asked a file-list search when it reads no OR.
pub fn needs_or(func: &str) -> String {
    format!(
        "{func} needs a gozd that reads OR (|), and the gozd running (goz v0.6.0) does not, \
         so it is not asked; {func} works in an open file list (File > Open File List...)"
    )
}

/// W8: why a gozd that reads no OR cannot be asked any part of `query`
/// (all its parts, before any is run); `None` when it can.
pub fn needs_boolean(query: &str) -> Option<String> {
    if !may_hold(query) {
        return None;
    }
    let found = terms(query);
    let t = found
        .iter()
        .find(|t| !matches!(t.func, Func::Save | Func::Open))?;
    Some(if t.func == Func::ListName {
        needs_syntax(&t.name)
    } else {
        needs_or(&t.name)
    })
}

/// W8: why gozd cannot be asked `filelist-filename:` when it reads no
/// `whole:`: nothing in its index belongs to a file list, but v0.6.0 has no
/// way to say "nothing".
pub fn needs_syntax(func: &str) -> String {
    format!(
        "{func} needs a gozd that reads the newer syntax (whole:), and the gozd running \
         (goz v0.6.0) does not, so it is not asked; {func} works in an open file list \
         (File > Open File List...)"
    )
}

/// The found items of a slot as gozd's index has them: an item of the
/// index that the slot names, with whether it is a folder and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundItem {
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// What a look-up in gozd's index found: at most [`LOOKUP_MAX`] + 1 items,
/// and how many there are in all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Looked {
    pub items: Vec<FoundItem>,
    pub total: u64,
}

/// Why [`for_gozd`] gave no search: a reason to show, or the look-up's own
/// failure (the connection's, for the caller to tell apart).
#[derive(Debug, PartialEq, Eq)]
pub enum NotAsked<E> {
    Refused(String),
    Lookup(E),
    /// W8R-4: the search went stale while a list on the disk was read.
    Stale,
}

impl<E> From<String> for NotAsked<E> {
    fn from(why: String) -> NotAsked<E> {
        NotAsked::Refused(why)
    }
}

/// The search as gozd is asked it: as it is without a file-list term; with
/// each one in gozd's own words when gozd reads OR, groups and `whole:`
/// (`boolean`), the found items' functions asking `lookup` (a search of
/// gozd's index: at most [`LOOKUP_MAX`] + 1 items with their sizes; and
/// whether it matches case, as the search does: [`search_case`] of the
/// Match Case toggle `match_case`) for them first; else why it cannot be
/// asked. A `save-filelistN:` term is taken out (the window saves the
/// results), with any gozd. W8R-4: an `include-file-list:` file is read
/// only while `stale` says the search is wanted, and only as far as gozd
/// can be asked ([`GOZD_MAX_TERMS`] names, and one more).
pub fn for_gozd<'a, E>(
    query: &'a str,
    slots: &Slots,
    boolean: bool,
    match_case: bool,
    lookup: &mut dyn FnMut(&str, bool) -> Result<Looked, E>,
    stale: &dyn Fn() -> bool,
) -> Result<Cow<'a, str>, NotAsked<E>> {
    // Most searches: no function of that name at all.
    if !may_hold(query) {
        return Ok(Cow::Borrowed(query));
    }
    let found = terms(query);
    if found.is_empty() {
        return Ok(Cow::Borrowed(query));
    }
    saves(query)?;
    // W8: the window searches the list an `open-filelist:` names; one that
    // reaches gozd's index was not opened, and gozd is not asked instead.
    if let Some(t) = found.iter().find(|t| t.func == Func::Open) {
        return Err(NotAsked::Refused(format!(
            "{} names a file list goz-gui has not opened, so gozd's index is not searched \
             in its place",
            t.name
        )));
    }
    if !boolean && let Some(t) = found.iter().find(|t| t.func != Func::Save) {
        return Err(if t.func == Func::ListName {
            needs_syntax(&t.name)
        } else {
            needs_or(&t.name)
        }
        .into());
    }
    let mut out = String::with_capacity(query.len() + 64);
    let mut at = 0;
    for t in &found {
        out.push_str(&query[at..t.start]);
        at = t.end;
        if t.func == Func::Save {
            out.push(' ');
            continue;
        }
        // W8: no item of gozd's index belongs to a file list: nothing
        // (`whole:""`), the file names not even read.
        let names = if t.func == Func::ListName {
            Vec::new()
        } else {
            names(
                t,
                slots,
                Reading {
                    cap: GOZD_MAX_TERMS + 1,
                    stale,
                },
            )?
        };
        let terms = match t.func {
            Func::Include if names.len() > GOZD_MAX_TERMS => {
                return Err(NotAsked::Refused(format!(
                    "{} names more than {GOZD_MAX_TERMS} items, and gozd searches at most \
                     {GOZD_MAX_TERMS} terms at once; open the list instead (open-filelist:, or \
                     File > Open File List...)",
                    t.name
                )));
            }
            Func::Items | Func::Include => names.iter().map(|n| item_term(n)).collect(),
            Func::ListName => Vec::new(),
            Func::Open => unreachable!("refused above"),
            Func::Name => names.iter().map(|n| item_term(name_part(n))).collect(),
            Func::Stem => stem_terms(t, &names)?,
            Func::Parent => parent_terms(t, &names)?,
            Func::Ancestor => ancestor_terms(t, &names)?,
            Func::ExistsIn => {
                return Err(NotAsked::Refused(format!(
                    "{} tests each result against file list slot {}, which gozd cannot be \
                     asked; it works in an open file list (File > Open File List...)",
                    t.name,
                    t.slot.unwrap_or(0)
                )));
            }
            Func::Child | Func::Descendant | Func::Sibling | Func::Size => {
                let looked = if names.is_empty() {
                    Looked::default()
                } else {
                    if names.len() > GOZD_MAX_TERMS {
                        return Err(too_many(t, names.len(), "names").into());
                    }
                    let items: Vec<String> = names.iter().map(|n| item_term(n)).collect();
                    lookup(
                        &format!("<{}>", items.join("|")),
                        search_case(query, match_case),
                    )
                    .map_err(NotAsked::Lookup)?
                };
                if looked.total > LOOKUP_MAX as u64 || looked.items.len() > LOOKUP_MAX {
                    return Err(NotAsked::Refused(format!(
                        "{} names {} items of the index, and goz-gui looks up at most {} for \
                         it; open them as a file list instead (Tools > File List Editor...)",
                        t.name,
                        crate::format::grouped(looked.total.max(looked.items.len() as u64)),
                        crate::format::grouped(LOOKUP_MAX as u64)
                    )));
                }
                found_terms(t, &looked.items)?
            }
            Func::Save => unreachable!("taken out above"),
        };
        if terms.len() > GOZD_MAX_TERMS {
            return Err(too_many(t, terms.len(), "terms").into());
        }
        out.push_str(&group(&terms));
    }
    out.push_str(&query[at..]);
    Ok(Cow::Owned(out))
}

/// Why a term is too long for gozd.
fn too_many(t: &Term, n: usize, what: &str) -> String {
    format!(
        "{} makes {} {what}, and gozd searches at most {GOZD_MAX_TERMS} terms at once; \
         open them as a file list instead (Tools > File List Editor...)",
        t.name,
        crate::format::grouped(n as u64)
    )
}

/// Terms as one group gozd reads; none: `whole:""`, which matches nothing
/// (no name is empty).
fn group(terms: &[String]) -> String {
    if terms.is_empty() {
        return "whole:\"\"".into();
    }
    format!("<{}>", terms.join("|"))
}

/// An item of a list as gozd reads it: `whole:` for a whole name or path,
/// a quoted wildcard as it is (a wildcard is matched whole already). A name
/// (no `\`) is `nopath:` too, so that it stays on the name under Match Path
/// (`matchpath:`) or `path:`, as Everything's file-list functions do and as
/// [`ListMatcher`] matches it in an open file list.
fn item_term(n: &str) -> String {
    let on_name = if n.contains('\\') { "" } else { "nopath:" };
    if has_wildcard(n) {
        format!("{on_name}\"{n}\"")
    } else {
        format!("{on_name}whole:\"{n}\"")
    }
}

/// `stem-filelistN:`: each slot item's stem, whole (a wildcard as it is).
fn stem_terms(t: &Term, names: &[String]) -> Result<Vec<String>, String> {
    names
        .iter()
        .map(|n| {
            let name = name_part(n);
            let stem = String::from_utf8_lossy(stem_of(name.as_bytes(), false)).into_owned();
            if stem.is_empty() {
                return Err(format!(
                    "{} cannot be asked of gozd: {name} has an empty stem",
                    t.name
                ));
            }
            Ok(if has_wildcard(&stem) {
                format!("stem:\"{stem}\"")
            } else {
                format!("whole:stem:\"{stem}\"")
            })
        })
        .collect()
}

/// `parent-filelistN:`: a full path is `parent:`, a name `parent-name:`.
fn parent_terms(t: &Term, names: &[String]) -> Result<Vec<String>, String> {
    names
        .iter()
        .map(|n| match (n.contains('\\'), has_wildcard(n)) {
            (true, false) => Ok(format!("parent:\"{n}\"")),
            (false, false) => Ok(format!("whole:parent-name:\"{n}\"")),
            (false, true) => Ok(format!("parent-name:\"{n}\"")),
            (true, true) => Err(wildcard_path(t, n)),
        })
        .collect()
}

/// `ancestor-filelistN:`: a full path is `ancestor:`, a name a folder of
/// that name anywhere above (`"**\name\**"`).
fn ancestor_terms(t: &Term, names: &[String]) -> Result<Vec<String>, String> {
    names
        .iter()
        .map(|n| match (n.contains('\\'), has_wildcard(n)) {
            (true, false) => Ok(format!("ancestor:\"{n}\"")),
            (false, _) => Ok(format!("\"**\\{n}\\**\"")),
            (true, true) => Err(wildcard_path(t, n)),
        })
        .collect()
}

fn wildcard_path(t: &Term, n: &str) -> String {
    format!(
        "{} can ask gozd for full paths and names, not for the wildcard path {n}; \
         it works in an open file list (File > Open File List...)",
        t.name
    )
}

/// The found items' functions in gozd's words, from the items found.
fn found_terms(t: &Term, items: &[FoundItem]) -> Result<Vec<String>, String> {
    // Each folder once (any case), as the index spells it.
    let mut seen = HashSet::new();
    let mut once = |p: &str, out: &mut Vec<String>, term: &dyn Fn(&str) -> String| {
        if seen.insert(folded(p.as_bytes())) {
            out.push(term(p));
        }
    };
    let mut out = Vec::new();
    match t.func {
        Func::Child => {
            for f in items {
                if let Some(up) = parent_of(f.path.as_bytes()).filter(|up| !is_root(up)) {
                    let up = String::from_utf8_lossy(up);
                    once(&up, &mut out, &|p| format!("whole:\"{p}\""));
                }
            }
        }
        Func::Descendant => {
            for f in items {
                for up in ancestors(f.path.as_bytes()).filter(|up| !is_root(up)) {
                    let up = String::from_utf8_lossy(up);
                    once(&up, &mut out, &|p| format!("whole:\"{p}\""));
                }
            }
        }
        Func::Sibling => {
            // By folder: the items found there.
            let mut by_folder: Vec<(String, Vec<&str>)> = Vec::new();
            let mut at: HashMap<Vec<u8>, usize> = HashMap::new();
            for f in items {
                let Some(up) = parent_of(f.path.as_bytes()) else {
                    continue;
                };
                let i = *at.entry(folded(up)).or_insert_with(|| {
                    by_folder.push((String::from_utf8_lossy(up).into_owned(), Vec::new()));
                    by_folder.len() - 1
                });
                let mine = &mut by_folder[i].1;
                if !mine
                    .iter()
                    .any(|p| folded(p.as_bytes()) == folded(f.path.as_bytes()))
                {
                    mine.push(&f.path);
                }
            }
            for (folder, found) in by_folder {
                out.push(match found.as_slice() {
                    [one] => format!("<parent:\"{folder}\" !whole:\"{one}\">"),
                    // Each is the others' sibling: all of the folder.
                    _ => format!("parent:\"{folder}\""),
                });
            }
        }
        Func::Size => {
            let mut sizes = HashSet::new();
            for f in items.iter().filter(|f| !f.is_dir) {
                match f.size {
                    Some(s) => {
                        if sizes.insert(s) {
                            out.push(format!("size:={s}"));
                        }
                    }
                    None => {
                        return Err(format!(
                            "{} needs the size of {}, which gozd has not read yet; \
                             try again in a moment",
                            t.name, f.path
                        ));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(out)
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

    /// No look-up wanted.
    fn none(_: &str, _: bool) -> Result<Looked, ()> {
        panic!("no look-up was wanted")
    }

    fn gozd<'a>(q: &'a str, s: &Slots, boolean: bool) -> Result<Cow<'a, str>, NotAsked<()>> {
        for_gozd(q, s, boolean, false, &mut none, &never)
    }

    /// A term's names, its file read whole.
    fn names_of(t: &Term, s: &Slots) -> Result<Vec<String>, String> {
        names::<()>(t, s, Reading::WHOLE).map_err(|e| match e {
            NotAsked::Refused(why) => why,
            other => panic!("{other:?}"),
        })
    }

    /// A search over a list, its file-list terms read whole.
    fn split(query: &str, s: &Slots) -> Result<(String, Vec<Planned>), String> {
        split_for_list(query, s, &never).map(|r| r.expect("never stale"))
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
        assert_eq!(t[1].name, "FileList3:");
        // An unquoted `|` or `>` ends the value.
        assert_eq!(t[2].value, "x");
        assert!(terms("myfilelist:x filelistx: filelist12: filelist-filename1:a").is_empty());
        assert!(terms(r#""a filelist:x""#).is_empty(), "inside quotes");
    }

    /// W7: every function, in each of Everything's spellings.
    #[test]
    fn every_function_in_everythings_spellings() {
        let one = |q: &str| {
            let t = terms(q);
            assert_eq!(t.len(), 1, "{q}: {t:?}");
            (t[0].func, t[0].slot)
        };
        for (q, want) in [
            ("exists-in-filelist2:a", (Func::ExistsIn, Some(2))),
            ("existsinfilelist2:a", (Func::ExistsIn, Some(2))),
            ("parent-filelist0:", (Func::Parent, Some(0))),
            ("parentfilelist0:", (Func::Parent, Some(0))),
            ("parent-file-list-0:", (Func::Parent, Some(0))),
            ("child-filelist:a;b", (Func::Child, None)),
            ("childfilelist4:", (Func::Child, Some(4))),
            ("child-file-list-4:", (Func::Child, Some(4))),
            ("sibling-filelist1:", (Func::Sibling, Some(1))),
            ("ANCESTORFILELIST9:", (Func::Ancestor, Some(9))),
            ("descendant-file-list-1:", (Func::Descendant, Some(1))),
            ("size-file-list-1:", (Func::Size, Some(1))),
            ("sizefilelist1:", (Func::Size, Some(1))),
            ("name-filelist1:", (Func::Name, Some(1))),
            ("stem-filelist1:", (Func::Stem, Some(1))),
            ("save-filelist1:", (Func::Save, Some(1))),
            ("savefilelist1:", (Func::Save, Some(1))),
            ("file-list-1:", (Func::Items, Some(1))),
        ] {
            assert_eq!(one(q), want, "{q}");
        }
        // A slot's function needs its slot.
        for q in [
            "parentfilelist:",
            "size-filelist:",
            "savefilelist:",
            "existsinfilelist:a",
        ] {
            assert!(terms(q).is_empty(), "{q}");
        }
    }

    #[test]
    fn the_term_at_a_place_of_the_box() {
        let q = "ab parent-filelist1: é filelist:\"x y\"";
        let t = term_at(q, 3).unwrap();
        assert_eq!(t.func, Func::Parent);
        assert_eq!(term_at(q, 20).unwrap().func, Func::Parent, "its end");
        assert!(term_at(q, 1).is_none());
        // UTF-16 places: the é is one unit.
        let t = term_at(q, 25).unwrap();
        assert_eq!((t.func, t.value.as_str()), (Func::Items, "x y"));
        let two = vec!["a b.txt".to_owned(), r"C:\c".to_owned()];
        assert_eq!(
            with_names(q, &t, &two).unwrap(),
            "ab parent-filelist1: é filelist:\"a b.txt\";C:\\c"
        );
        assert_eq!(
            with_names("filelist:a x", &terms("filelist:a x")[0], &["b".into()]).unwrap(),
            "filelist:b x"
        );
        // W7R-3: a name with a `;` is written in quotes of its own, and
        // read back as one name.
        let odd = vec![
            "a.txt".to_owned(),
            "b;c.txt".to_owned(),
            r"C:\d e\f;g".to_owned(),
        ];
        let new = with_names(q, &t, &odd).unwrap();
        assert_eq!(
            new,
            "ab parent-filelist1: é filelist:a.txt;\"b;c.txt\";\"C:\\d e\\f;g\""
        );
        let back = terms(&new).remove(1);
        assert_eq!(
            names::<()>(&back, &Slots::default(), Reading::WHOLE).unwrap(),
            odd
        );
        assert!(with_names(q, &t, &["a\"b".to_owned()]).is_none());
        assert!(with_names(q, &t, &["a|b".to_owned()]).is_none());
    }

    #[test]
    fn names_of_a_term_or_its_slot() {
        let s = slots();
        let t = |q: &str| terms(q).remove(0);
        assert_eq!(
            names_of(&t(r#"filelist:" a.txt ";"C:/x/y/";"|""#), &s).unwrap(),
            ["a.txt", r"C:\x\y"]
        );
        assert_eq!(names_of(&t(r#"filelist:"a|b""#), &s).unwrap(), ["a", "b"]);
        // W7R-3: a `;` in quotes is part of a name, as Everything
        // documents (`filelist:...;"a-file-with-a-semicolon(;).txt"`).
        assert_eq!(
            names_of(&t(r#"filelist:a.txt;"b;c.txt""#), &s).unwrap(),
            ["a.txt", "b;c.txt"]
        );
        assert_eq!(
            names_of(&t(r#"child-filelist:"x;y";z;"C:\a b\c;d.txt""#), &s).unwrap(),
            ["x;y", "z", r"C:\a b\c;d.txt"]
        );
        assert_eq!(
            names_of(&t(r#"filelist:a"b;c"d;e"#), &s).unwrap(),
            ["ab;cd", "e"],
            "quotes around part of a name"
        );
        assert_eq!(
            names_of(&t(r#"filelist:"a;b|c d""#), &s).unwrap(),
            ["a;b", "c d"],
            "Everything 1.4's | in quotes still parts names"
        );
        assert_eq!(names_of(&t("filelist:D:\\"), &s).unwrap(), [r"D:\"]);
        assert_eq!(
            names_of(&t("filelist1:"), &s).unwrap(),
            [r"C:\x\a.txt", r"C:\x\sub"]
        );
        assert!(names_of(&t("filelist2:"), &s).unwrap().is_empty());
        assert!(
            names_of(&t("filelist1:x"), &s)
                .unwrap_err()
                .contains("slot 1")
        );
        assert!(
            names_of(&t("parent-filelist1:x"), &s)
                .unwrap_err()
                .starts_with("parent-filelist1: takes no value")
        );
        // exists-in's value is its own: the slot's names are the list.
        assert_eq!(
            names_of(&t("exists-in-filelist1:$name:"), &s)
                .unwrap()
                .len(),
            2
        );
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
        let (rest, planned) = split("ext:txt filelist1: size:<1kb", &s).unwrap();
        assert_eq!(rest, "ext:txt   size:<1kb");
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].names, [r"C:\x\a.txt", r"C:\x\sub"]);
        let (rest, planned) = split("plain text", &s).unwrap();
        assert_eq!((rest.as_str(), planned.len()), ("plain text", 0));
        assert!(split("filelist4:x", &s).is_err());
        // save-filelist tests nothing.
        let (rest, planned) = split("*.txt save-filelist3:", &s).unwrap();
        assert_eq!((rest.as_str(), planned.len()), ("*.txt  ", 0));
        assert!(
            split("save-filelist1: filelist1:", &s)
                .unwrap_err()
                .contains("slot 1")
        );
    }

    /// A list's rows: `(path, is_dir, size)`.
    const ROWS: &[(&str, bool, Option<u64>)] = &[
        (r"C:\", true, None),
        (r"C:\x", true, None),
        (r"C:\x\a.txt", false, Some(10)),
        (r"C:\x\b.TXT", false, Some(20)),
        (r"C:\x\a.srt", false, Some(5)),
        (r"C:\x\sub", true, None),
        (r"C:\x\sub\deep.txt", false, Some(10)),
        (r"C:\x\sub\v1.2", true, None),
        (r"C:\y", true, None),
        (r"C:\y\c.log", false, Some(20)),
        (r"C:\y\a.txt", false, Some(99)),
        (r"C:\top.txt", false, Some(1)),
    ];

    /// The rows of [`ROWS`] the search's file-list terms pass.
    fn hits(query: &str, s: &Slots) -> Vec<&'static str> {
        let (_, planned) = split(query, s).unwrap();
        let mut t = RowTests::new(&planned, false, LIST).unwrap();
        if t.needs_rows() {
            for &(p, d, size) in ROWS {
                t.see(p.as_bytes(), d, size);
            }
        }
        ROWS.iter()
            .filter(|&&(p, d, size)| t.matches(p.as_bytes(), d, size))
            .map(|r| r.0)
            .collect()
    }

    fn with(slot: &[&str]) -> Slots {
        let mut s = Slots::default();
        s.set(1, slot.iter().map(|p| (*p).to_owned()).collect());
        s
    }

    /// W7: each slot function over an open file list's rows.
    #[test]
    fn each_slot_function_over_a_lists_rows() {
        let s = with(&[r"C:\x\a.txt", r"C:\x\sub"]);
        assert_eq!(hits("filelist1:", &s), [r"C:\x\a.txt", r"C:\x\sub"]);
        assert_eq!(
            hits("parent-filelist1:", &s),
            [r"C:\x\sub\deep.txt", r"C:\x\sub\v1.2"]
        );
        // By name too: a folder named x.
        assert_eq!(
            hits("parentfilelist1:", &with(&["x"])),
            [r"C:\x\a.txt", r"C:\x\b.TXT", r"C:\x\a.srt", r"C:\x\sub"]
        );
        assert_eq!(
            hits("ancestor-filelist1:", &with(&[r"C:\x"])),
            [
                r"C:\x\a.txt",
                r"C:\x\b.TXT",
                r"C:\x\a.srt",
                r"C:\x\sub",
                r"C:\x\sub\deep.txt",
                r"C:\x\sub\v1.2"
            ]
        );
        assert_eq!(
            hits("ancestorfilelist1:", &with(&["sub"])),
            [r"C:\x\sub\deep.txt", r"C:\x\sub\v1.2"]
        );
        // The folders holding a found item: C:\x holds both.
        assert_eq!(hits("child-filelist1:", &s), [r"C:\x"]);
        assert_eq!(hits("child-filelist:top.txt;c.log", &s), [r"C:\", r"C:\y"]);
        // A name found in two folders: both.
        assert_eq!(
            hits("childfilelist1:", &with(&["a.txt"])),
            [r"C:\x", r"C:\y"]
        );
        // Not found in the list: nothing holds it.
        assert!(hits("childfilelist1:", &with(&[r"C:\gone\a.txt"])).is_empty());
        assert_eq!(
            hits("descendant-filelist1:", &with(&[r"C:\x\sub\deep.txt"])),
            [r"C:\", r"C:\x", r"C:\x\sub"]
        );
        // Siblings: not the item itself...
        assert_eq!(
            hits("sibling-filelist1:", &with(&[r"C:\y\c.log"])),
            [r"C:\y\a.txt"]
        );
        // ...but each of two found in one folder is the other's.
        assert_eq!(
            hits("sibling-filelist1:", &with(&[r"C:\y\c.log", r"C:\y\a.txt"])),
            [r"C:\y\c.log", r"C:\y\a.txt"]
        );
        // Files of a found file's size (10: a.txt and deep.txt).
        assert_eq!(
            hits("size-filelist1:", &with(&[r"C:\x\a.txt"])),
            [r"C:\x\a.txt", r"C:\x\sub\deep.txt"]
        );
        assert_eq!(
            hits("sizefilelist1:", &with(&[r"C:\y\c.log"])),
            [r"C:\x\b.TXT", r"C:\y\c.log"]
        );
        assert_eq!(
            hits("name-filelist1:", &with(&[r"D:\other\A.TXT"])),
            [r"C:\x\a.txt", r"C:\y\a.txt"]
        );
        // Stems: a folder's is its whole name.
        assert_eq!(
            hits("stem-filelist1:", &with(&[r"D:\a.doc", "v1.2.zip"])),
            [
                r"C:\x\a.txt",
                r"C:\x\a.srt",
                r"C:\x\sub\v1.2",
                r"C:\y\a.txt"
            ]
        );
        // exists-in: the name each item makes is in the slot.
        let subs = with(&[r"E:\subs\a.srt", r"E:\subs\z.srt"]);
        assert_eq!(
            hits("exists-in-filelist1:$stem:.srt", &subs)
                .into_iter()
                .filter(|p| p.ends_with(".txt"))
                .collect::<Vec<_>>(),
            [r"C:\x\a.txt", r"C:\y\a.txt"]
        );
        assert_eq!(
            hits(
                r"exists-in-filelist1:$path:\$stem:.srt",
                &with(&[r"C:\x\a.srt"])
            ),
            [r"C:\x\a.txt", r"C:\x\a.srt"]
        );
        assert_eq!(
            hits("exists-in-filelist1:$parent-name:", &with(&["sub"])),
            [r"C:\x\sub\deep.txt", r"C:\x\sub\v1.2"]
        );
        // An empty slot: nothing, whatever the function.
        let empty = Slots::default();
        for f in [
            "filelist1:",
            "parent-filelist1:",
            "child-filelist1:",
            "descendant-filelist1:",
            "sibling-filelist1:",
            "size-filelist1:",
            "name-filelist1:",
            "stem-filelist1:",
            "ancestor-filelist1:",
            "exists-in-filelist1:$name:",
        ] {
            assert!(hits(f, &empty).is_empty(), "{f}");
        }
    }

    #[test]
    fn exists_in_reads_its_substitutions() {
        assert_eq!(
            substitution("$stem:.srt", "f").unwrap(),
            [Piece::Prop(Prop::Stem), Piece::Text(".srt".into())]
        );
        assert_eq!(
            substitution("a$b$Full-Path:", "f").unwrap(),
            [Piece::Text("a$b".into()), Piece::Prop(Prop::FullPath)]
        );
        let why = substitution("$size:", "exists-in-filelist1:").unwrap_err();
        assert!(
            why.starts_with("exists-in-filelist1: cannot put in $size:"),
            "{why}"
        );
        let mut out = Vec::new();
        let p = substitution("$path:|$name:|$ext:|$parent-name:", "f").unwrap();
        substitute(&p, br"C:\a.b", false, &mut out);
        assert_eq!(out, br"C:|a.b|b|C:\");
        substitute(&p, br"C:\x\v1.2", true, &mut out);
        assert_eq!(out, br"C:\x|v1.2||x");
    }

    #[test]
    fn folders_above_and_parents() {
        assert_eq!(parent_of(br"C:\x\a"), Some(&br"C:\x"[..]));
        assert_eq!(parent_of(br"C:\x"), Some(&br"C:\"[..]));
        assert_eq!(parent_of(br"C:\"), None);
        assert_eq!(parent_of(b"name"), None);
        assert_eq!(parent_of(br"\\srv\share\x"), Some(&br"\\srv\share"[..]));
        let up: Vec<&[u8]> = ancestors(br"C:\a\b\c").collect();
        assert_eq!(up, [&br"C:\a\b"[..], br"C:\a", br"C:\"]);
    }

    #[test]
    fn gozd_gets_a_group_or_a_reason() {
        let s = slots();
        assert!(matches!(gozd("a b", &s, false), Ok(Cow::Borrowed("a b"))));
        assert!(matches!(
            gozd(r#""filelist:x""#, &s, false),
            Ok(Cow::Borrowed(_))
        ));
        assert_eq!(
            gozd("x filelist:a", &s, false).unwrap_err(),
            NotAsked::Refused(needs_or("filelist:"))
        );
        assert_eq!(
            gozd(r#"!filelist:a.txt;"C:\b c\*.log" ext:txt"#, &s, true).unwrap(),
            r#"!<nopath:whole:"a.txt"|"C:\b c\*.log"> ext:txt"#
        );
        assert_eq!(
            gozd("filelist1: | filelist9: filelist2:", &s, true).unwrap(),
            r#"<whole:"C:\x\a.txt"|whole:"C:\x\sub"> | <nopath:"*.log"> whole:"""#
        );
        // W7R-3: a quoted `;` stays in its name, in gozd's words too.
        assert_eq!(
            gozd(r#"filelist:a.txt;"b;c.txt""#, &s, true).unwrap(),
            r#"<nopath:whole:"a.txt"|nopath:whole:"b;c.txt">"#
        );
        let mut big = Slots::default();
        big.set(
            0,
            (0..=GOZD_MAX_TERMS).map(|i| format!("{i}.txt")).collect(),
        );
        let NotAsked::Refused(why) = gozd("filelist0:", &big, true).unwrap_err() else {
            panic!()
        };
        assert!(
            why.contains("257 terms") && why.contains("filelist0:"),
            "{why}"
        );
        assert_eq!(cmd::slot_of(4853), Some(3));
        assert_eq!(cmd::slot_of(4860), None);
    }

    /// W7: the functions gozd is asked in its own words, with no look-up.
    #[test]
    fn gozd_gets_parents_ancestors_names_and_stems() {
        let s = with(&[r"C:\x", "sub", "*.d", r"D:\a.b.txt"]);
        assert_eq!(
            gozd(
                "parent-filelist1: ext:txt",
                &with(&[r"C:\x", "sub", "*.d"]),
                true
            )
            .unwrap(),
            r#"<parent:"C:\x"|whole:parent-name:"sub"|parent-name:"*.d"> ext:txt"#
        );
        assert_eq!(
            gozd("ancestor-filelist1:", &with(&[r"C:\x", "sub", "*.d"]), true).unwrap(),
            r#"<ancestor:"C:\x"|"**\sub\**"|"**\*.d\**">"#
        );
        assert_eq!(
            gozd("name-filelist1:", &s, true).unwrap(),
            r#"<nopath:whole:"x"|nopath:whole:"sub"|nopath:"*.d"|nopath:whole:"a.b.txt">"#
        );
        assert_eq!(
            gozd("stem-filelist1:", &s, true).unwrap(),
            r#"<whole:stem:"x"|whole:stem:"sub"|stem:"*"|whole:stem:"a.b">"#
        );
        // A wildcard path is a file list's alone.
        let NotAsked::Refused(why) =
            gozd("parent-filelist1:", &with(&[r"C:\x\*"]), true).unwrap_err()
        else {
            panic!()
        };
        assert!(why.contains(r"wildcard path C:\x\*"), "{why}");
        // exists-in: never gozd's.
        let NotAsked::Refused(why) = gozd("exists-in-filelist1:$stem:.srt", &s, true).unwrap_err()
        else {
            panic!()
        };
        assert!(
            why.starts_with("exists-in-filelist1: tests each result"),
            "{why}"
        );
        // The old gozd: not asked, the function named.
        assert_eq!(
            gozd("size-filelist1:", &s, false).unwrap_err(),
            NotAsked::Refused(needs_or("size-filelist1:"))
        );
        // save-filelist alone needs no OR: it is taken out.
        assert_eq!(gozd("*.txt savefilelist2:", &s, false).unwrap(), "*.txt  ");
        assert_eq!(
            saves("*.txt savefilelist2: save-filelist0: SaveFileList2:").unwrap(),
            [2, 0]
        );
        assert!(saves("savefilelist1: parent-filelist1:").is_err());
        assert!(saves("savefilelist1: parent-filelist2:").is_ok());
    }

    /// W7: the found items' functions look the slot up in gozd's index
    /// first, then send folders, paths and sizes.
    #[test]
    fn gozd_gets_the_found_items_functions_after_a_look_up() {
        let s = with(&["a.txt", r"C:\y\c.log"]);
        let found = |p: &str, d: bool, size: Option<u64>| FoundItem {
            path: p.into(),
            is_dir: d,
            size,
        };
        let index = vec![
            found(r"C:\x\a.txt", false, Some(10)),
            found(r"C:\y\a.txt", false, Some(99)),
            found(r"c:\Y\c.log", false, Some(10)),
            found(r"C:\top\a.txt", false, Some(5)),
            found(r"C:\a.txt", false, Some(5)),
        ];
        let asked = std::cell::RefCell::new(Vec::new());
        let mut lookup = |q: &str, case: bool| -> Result<Looked, ()> {
            asked.borrow_mut().push((q.to_owned(), case));
            Ok(Looked {
                items: index.clone(),
                total: index.len() as u64,
            })
        };
        assert_eq!(
            for_gozd("childfilelist1:", &s, true, false, &mut lookup, &never).unwrap(),
            r#"<whole:"C:\x"|whole:"C:\y"|whole:"C:\top">"#
        );
        assert_eq!(
            *asked.borrow(),
            [(
                r#"<nopath:whole:"a.txt"|whole:"C:\y\c.log">"#.to_owned(),
                false
            )]
        );
        assert_eq!(
            for_gozd(
                "descendant-filelist1:",
                &s,
                true,
                false,
                &mut lookup,
                &never
            )
            .unwrap(),
            r#"<whole:"C:\x"|whole:"C:\y"|whole:"C:\top">"#
        );
        assert_eq!(
            for_gozd("sibling-filelist1:", &s, true, false, &mut lookup, &never).unwrap(),
            r#"<<parent:"C:\x" !whole:"C:\x\a.txt">|parent:"C:\y"|<parent:"C:\top" !whole:"C:\top\a.txt">|<parent:"C:\" !whole:"C:\a.txt">>"#
        );
        assert_eq!(
            for_gozd("size-filelist1:", &s, true, false, &mut lookup, &never).unwrap(),
            "<size:=10|size:=99|size:=5>"
        );
        // W7R-2: the look-up matches case as the search does: the Match
        // Case toggle, or the search's own `case:` (the last one wins).
        asked.borrow_mut().clear();
        for (q, toggle) in [
            ("child-filelist1:", true),
            ("case: child-filelist1:", false),
            ("CASE:1 child-filelist1:", false),
            ("case:0 child-filelist1:", true),
            ("case: child-filelist1: case:0", false),
            ("case:Foo child-filelist1:", false),
        ] {
            for_gozd(q, &s, true, toggle, &mut lookup, &never).unwrap();
        }
        let cases: Vec<bool> = asked.borrow().iter().map(|a| a.1).collect();
        assert_eq!(cases, [true, true, true, false, false, false]);
        // An empty slot: no look-up, nothing matches.
        let mut no = |_: &str, _: bool| -> Result<Looked, ()> { panic!("no look-up") };
        assert_eq!(
            for_gozd(
                "child-filelist1:",
                &Slots::default(),
                true,
                false,
                &mut no,
                &never
            )
            .unwrap(),
            r#"whole:"""#
        );
        // Too many found: refused, named.
        let mut many = |_: &str, _: bool| -> Result<Looked, ()> {
            Ok(Looked {
                items: Vec::new(),
                total: 5000,
            })
        };
        let NotAsked::Refused(why) =
            for_gozd("child-filelist1:", &s, true, false, &mut many, &never).unwrap_err()
        else {
            panic!()
        };
        assert!(why.contains("5,000 items"), "{why}");
        // A size not read yet: refused rather than wrong.
        let mut pending = |_: &str, _: bool| -> Result<Looked, ()> {
            Ok(Looked {
                items: vec![found(r"C:\x\a.txt", false, None)],
                total: 1,
            })
        };
        assert!(matches!(
            for_gozd("size-filelist1:", &s, true, false, &mut pending, &never),
            Err(NotAsked::Refused(_))
        ));
        // The look-up's own failure comes back as it is.
        let mut lost = |_: &str, _: bool| -> Result<Looked, u8> { Err(7) };
        assert_eq!(
            for_gozd("sibling-filelist1:", &s, true, false, &mut lost, &never).unwrap_err(),
            NotAsked::Lookup(7)
        );
    }

    /// W7R-1: under Match Path (the window sends `matchpath: ` before the
    /// search) a name stays on the name in gozd's words (`nopath:`), as in
    /// an open file list and in Everything; a path is a full path anyway.
    #[test]
    fn gozd_keeps_names_on_the_name_under_match_path() {
        let s = with(&["a.txt", r"C:\x\b.txt", "*.log", r"D:\data\*.csv"]);
        assert_eq!(
            gozd("matchpath: name-filelist1:", &s, true).unwrap(),
            r#"matchpath: <nopath:whole:"a.txt"|nopath:whole:"b.txt"|nopath:"*.log"|nopath:"*.csv">"#
        );
        assert_eq!(
            gozd("matchpath: filelist1:", &s, true).unwrap(),
            r#"matchpath: <nopath:whole:"a.txt"|whole:"C:\x\b.txt"|nopath:"*.log"|"D:\data\*.csv">"#
        );
        assert_eq!(
            gozd("path: filelist:a.txt;*.log", &s, true).unwrap(),
            r#"path: <nopath:whole:"a.txt"|nopath:"*.log">"#
        );
        // The look-up of the found items' functions too.
        let asked = std::cell::RefCell::new(Vec::new());
        let mut lookup = |q: &str, _: bool| -> Result<Looked, ()> {
            asked.borrow_mut().push(q.to_owned());
            Ok(Looked::default())
        };
        for_gozd(
            "matchpath: child-filelist:a.txt",
            &s,
            true,
            false,
            &mut lookup,
            &never,
        )
        .unwrap();
        assert_eq!(*asked.borrow(), [r#"<nopath:whole:"a.txt">"#]);
        // An open file list answers the same: names on the name.
        assert_eq!(
            hits("matchpath: name-filelist1:", &with(&[r"D:\other\A.TXT"])),
            [r"C:\x\a.txt", r"C:\y\a.txt"]
        );
    }

    /// W7R-2: the search's own `case:`, as goz's parsers read it.
    #[test]
    fn a_searchs_own_case_directive() {
        for (q, toggle, want) in [
            ("abc", false, false),
            ("abc", true, true),
            ("case: abc", false, true),
            ("abc Case:", false, true),
            ("case:1 abc", false, true),
            (r#"case:"1" abc"#, false, true),
            ("case:0 abc", true, false),
            ("case: x case:0", false, false),
            ("case:0 x case:", false, true),
            // One term's modifier, not the search's.
            ("case:Foo", false, false),
            ("mycase: x", false, false),
            (r#""case:" x"#, false, false),
            ("<a|case:>", false, true),
        ] {
            assert_eq!(search_case(q, toggle), want, "{q} (toggle {toggle})");
        }
    }

    /// The file list [`hits`] searches: its full path.
    const LIST: &str = r"C:\Lists\Test List.efu";

    /// W8: the new functions in each spelling Everything.exe and its
    /// documentation have; none takes a slot.
    #[test]
    fn w8_functions_in_everythings_spellings() {
        for (q, want) in [
            (r"include-file-list:C:\l.efu", Func::Include),
            (r"include-filelist:C:\l.efu", Func::Include),
            (r"includefilelist:C:\l.efu", Func::Include),
            (r#"open-filelist:"C:\a b\l.efu""#, Func::Open),
            (r"OpenFileList:C:\l.efu", Func::Open),
            (r"open-file-list:C:\l.efu", Func::Open),
            ("filelist-filename:l.efu", Func::ListName),
            ("filelistfilename:l.efu", Func::ListName),
            ("file-list-filename:", Func::ListName),
        ] {
            let t = terms(q);
            assert_eq!(t.len(), 1, "{q}: {t:?}");
            assert_eq!((t[0].func, t[0].slot), (want, None), "{q}");
        }
        for q in [
            "include-file-list1:x",
            "open-filelist2:x",
            "filelist-filename3:",
        ] {
            assert!(terms(q).is_empty(), "{q}");
        }
        assert!(Func::Include.picks_file() && Func::Open.picks_file());
        assert!(!Func::ListName.editable() && Func::Items.editable());
        let q = r"abc open-filelist:old.efu x";
        let t = terms(q).remove(0);
        assert_eq!(
            with_file(q, &t, r"C:\a b\l.efu"),
            r#"abc open-filelist:"C:\a b\l.efu" x"#
        );
    }

    /// W8: `new-search:` parts a search where a term may start, outside
    /// quotes, in each spelling; misplaced, it is refused.
    #[test]
    fn new_search_parts_a_search() {
        let q = "*.mp3 save-filelist1: new-search: childfilelist1:";
        assert_eq!(
            parts(q).unwrap(),
            ["*.mp3 save-filelist1: ", " childfilelist1:"]
        );
        assert_eq!(last_part(q), " childfilelist1:");
        assert_eq!(
            parts("a NewSearch: b new_search:").unwrap(),
            ["a ", " b ", ""]
        );
        assert_eq!(parts(r#""new-search:" a"#).unwrap(), [r#""new-search:" a"#]);
        assert_eq!(parts("my-new-search: a").unwrap(), ["my-new-search: a"]);
        // A comparison is no group.
        assert_eq!(
            parts("size:<1mb dm:>2024 new-search: <a|b>").unwrap(),
            ["size:<1mb dm:>2024 ", " <a|b>"]
        );
        assert_eq!(parts("abc").unwrap(), ["abc"]);
        for bad in [
            "a new-search:b",
            "<a new-search: b>",
            "!new-search: b",
            "a | new-search: b",
        ] {
            assert!(parts(bad).unwrap_err().contains("new-search:"), "{bad}");
            assert_eq!(last_part(bad), bad);
        }
        // Each part made a search of its own (the window's filter).
        assert_eq!(
            each_part(q, |p| format!("ext:mp3 {p}")),
            "ext:mp3 *.mp3 save-filelist1: new-search: ext:mp3 childfilelist1:"
        );
        assert_eq!(each_part("abc", |p| format!("[{p}]")), "[abc]");
        assert!(is_costly(q) && !is_costly("research: x") && !is_costly("abc"));
    }

    /// The paths a part "finds" in [`resolve`]'s tests: its first word's
    /// letters, each a file in `C:\r`, or the names of slot 1 when it reads
    /// it (`filelist1:`).
    fn run_part(part: &str, now: &Arc<Slots>) -> Result<Vec<String>, NotAsked<()>> {
        if part.contains("filelist1:") && !part.contains("save-filelist1:") {
            return Ok(now.get(1).to_vec());
        }
        let word = part.split_whitespace().next().unwrap_or("");
        Ok(word.chars().map(|c| format!(r"C:\r\{c}.txt")).collect())
    }

    /// W8: the parts before the last run left to right, each saving to the
    /// slots the next ones read; a part that saves nothing is not run.
    #[test]
    fn earlier_parts_fill_the_slots_the_later_read() {
        let start = Arc::new(slots());
        let mut asked = Vec::new();
        let mut run = |part: &str, now: &Arc<Slots>| {
            asked.push(part.trim().to_owned());
            run_part(part, now)
        };
        let r = resolve(
            "ab save-filelist1: new-search: zz new-search: filelist1: save-filelist2: \
             new-search: childfilelist2:",
            &start,
            &mut run,
        )
        .unwrap();
        assert_eq!(asked, ["ab save-filelist1:", "filelist1: save-filelist2:"]);
        assert_eq!(r.last, " childfilelist2:");
        assert_eq!(r.slots.get(1), [r"C:\r\a.txt", r"C:\r\b.txt"]);
        assert_eq!(r.slots.get(2), r.slots.get(1));
        // The slots not saved are as they were.
        assert_eq!(r.slots.get(9), start.get(9));
        let saves = r.saves.unwrap();
        assert_eq!(saves.saved, [(1, 2), (2, 2)]);
        assert!(Arc::ptr_eq(&saves.slots, &r.slots));
        // Without new-search:, nothing is run and the slots are the same.
        let r = resolve::<()>("ab save-filelist1:", &start, &mut |_, _| panic!("not run")).unwrap();
        assert!(Arc::ptr_eq(&r.slots, &start) && r.saves.is_none());
        assert_eq!(r.last, "ab save-filelist1:");
        // A part reading the slot it saves, and a misplaced new-search:.
        let refused = |q: &str| match resolve(q, &start, &mut run_part) {
            Err(NotAsked::Refused(why)) => why,
            other => panic!("{q}: {other:?}"),
        };
        assert!(
            refused("filelist1: save-filelist1: new-search: x").contains("reads file list slot 1")
        );
        assert!(refused("a new-search:x").contains("takes no value"));
        // More results than a slot is given: the later part is not run.
        let mut many = |_: &str, _: &Arc<Slots>| -> Result<Vec<String>, NotAsked<()>> {
            Ok((0..=SAVE_MAX).map(|i| format!(r"C:\m\{i}")).collect())
        };
        let Err(NotAsked::Refused(why)) = resolve(
            "x save-filelist3: new-search: filelist3:",
            &start,
            &mut many,
        ) else {
            panic!()
        };
        assert!(why.starts_with("save-filelist3: before new-search: saves at most 100,000"));
        // A stale run ends it.
        let Err(NotAsked::Lookup(())) = resolve(
            "x save-filelist3: new-search: filelist3:",
            &start,
            &mut |_, _| Err(NotAsked::Lookup(())),
        ) else {
            panic!()
        };
    }

    /// W8: Everything's example, `save-filelist1: newsearch:
    /// childfilelist1:`, over a list's rows: the folders holding a `.txt`.
    #[test]
    fn everythings_new_search_example_over_rows() {
        let none = Arc::new(Slots::default());
        let mut run = |part: &str, now: &Arc<Slots>| -> Result<Vec<String>, NotAsked<()>> {
            let (rest, planned) = split(part, now)?;
            assert_eq!(rest.trim(), "*.txt");
            let mut t = RowTests::new(&planned, false, LIST)?;
            Ok(ROWS
                .iter()
                .filter(|&&(p, d, s)| {
                    !d && p.to_ascii_lowercase().ends_with(".txt") && t.matches(p.as_bytes(), d, s)
                })
                .map(|r| r.0.to_owned())
                .collect())
        };
        let r = resolve(
            "*.txt savefilelist1: newsearch: childfilelist1:",
            &none,
            &mut run,
        )
        .unwrap();
        assert_eq!(
            hits(r.last, &r.slots),
            [r"C:\", r"C:\x", r"C:\x\sub", r"C:\y"]
        );
    }

    /// W8: `include-file-list:` reads an EFU's `Filename` column, or a
    /// name a line, and matches as `filelist:`; its file must be a full
    /// path that can be read.
    #[test]
    fn include_file_list_reads_a_list_on_the_disk() {
        let dir = crate::store::file::tests::scratch("w8-include");
        let efu = dir.join("names.efu");
        std::fs::write(
            &efu,
            "Filename,Size\r\nC:\\x\\a.txt,10\r\n\"C:\\x\\sub\\\",\r\nc.log,\r\n",
        )
        .unwrap();
        let txt = dir.join("names.txt");
        // UTF-16 with its mark, as Notepad may save one.
        let mut bytes = vec![0xFF, 0xFE];
        for u in "a.srt\r\n*.TXT\r\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&txt, bytes).unwrap();
        let none = Slots::default();
        let q = format!("include-file-list:\"{}\"", efu.display());
        // W8R-3: the EFU's relative `c.log` is the one in the list's folder.
        assert_eq!(hits(&q, &none), [r"C:\x\a.txt", r"C:\x\sub"]);
        let q = format!("includefilelist:\"{}\" x", txt.display());
        assert_eq!(
            hits(&q, &none),
            [
                r"C:\x\a.txt",
                r"C:\x\b.TXT",
                r"C:\x\a.srt",
                r"C:\x\sub\deep.txt",
                r"C:\y\a.txt",
                r"C:\top.txt"
            ]
        );
        // gozd: the same group as filelist:'s, with a gozd that reads OR.
        let q = format!("include-file-list:\"{}\"", efu.display());
        assert_eq!(
            gozd(&q, &none, true).unwrap(),
            format!(
                r#"<whole:"C:\x\a.txt"|whole:"C:\x\sub"|whole:"{}\c.log">"#,
                dir.display()
            )
        );
        assert_eq!(
            gozd(&q, &none, false).unwrap_err(),
            NotAsked::Refused(needs_or("include-file-list:"))
        );
        assert!(is_costly(&q));
        // Its stamp changes with the file.
        let before = include_stamps(&q);
        assert!(before[0].is_some());
        std::fs::write(&efu, "Filename\r\nC:\\x\\b.TXT\r\n").unwrap();
        assert_ne!(include_stamps(&q), before);
        let why = |q: &str| split(q, &none).unwrap_err();
        assert!(why("include-file-list:names.efu").contains("full path"));
        assert!(why("include-file-list:").contains("full path"));
        let missing = format!("include-file-list:\"{}\"", dir.join("gone.efu").display());
        assert!(why(&missing).starts_with("include-file-list: cannot read"));
        assert!(why(&format!("include-file-list:\"{}\"", dir.display())).contains("folder"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W8R-3: an EFU's or CSV's relative rows are under the list's folder,
    /// as `open-filelist:`, Open File List and the File List Editor read
    /// them; a TXT's lines are names or full paths, as Everything's "Use a
    /// path separator to specify the full path".
    #[test]
    fn include_file_list_puts_an_efus_relative_rows_under_its_folder() {
        let dir = crate::store::file::tests::scratch("w8r3-include");
        let d = dir.display().to_string();
        let lists = [
            (
                "x.efu",
                "Filename,Size\r\nsub\\a.txt,1\r\na.txt,2\r\nC:\\y\\c.log,3\r\n",
            ),
            (
                "x.csv",
                "Filename\r\nsub/a.txt\r\na.txt\r\nC:\\y\\c.log\r\n",
            ),
        ];
        for (name, text) in lists {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap();
            let q = format!("include-file-list:\"{}\"", path.display());
            let t = terms(&q).remove(0);
            assert_eq!(
                names::<()>(&t, &Slots::default(), Reading::WHOLE).unwrap(),
                [
                    format!(r"{d}\sub\a.txt"),
                    format!(r"{d}\a.txt"),
                    r"C:\y\c.log".into()
                ],
                "{name}"
            );
            // The same rows as the list opened as a file list.
            let list = crate::filelist::FileList::open(&path, None).unwrap();
            while !list.loaded() {
                std::thread::yield_now();
            }
            let m = list
                .answer(
                    "",
                    crate::filelist::Order::Gozd(Default::default()),
                    false,
                    &Arc::new(Slots::default()),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            let mut opened: Vec<String> = list
                .rows_of(&m, 0..10)
                .iter()
                .map(|r| String::from_utf16_lossy(&r.path))
                .collect();
            opened.sort();
            let mut included = names::<()>(&t, &Slots::default(), Reading::WHOLE).unwrap();
            included.sort();
            assert_eq!(opened, included, "{name}");
        }
        // A TXT's lines: a name alone is a whole name, a path a full path.
        let txt = dir.join("x.txt");
        std::fs::write(&txt, "sub\\a.txt\r\na.txt\r\n").unwrap();
        let q = format!("include-file-list:\"{}\"", txt.display());
        let t = terms(&q).remove(0);
        assert_eq!(
            names::<()>(&t, &Slots::default(), Reading::WHOLE).unwrap(),
            [r"sub\a.txt", "a.txt"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W8R-4: for gozd, `include-file-list:` reads no more of its file than
    /// gozd can be asked (one name past [`GOZD_MAX_TERMS`] says "too many"),
    /// and a search gone stale stops the read, for gozd and for a list.
    #[test]
    fn include_file_list_reads_only_what_the_search_can_use() {
        let dir = crate::store::file::tests::scratch("w8r4-include");
        let big = dir.join("big.efu");
        let mut text = String::from("Filename\r\n");
        for i in 0..20_000 {
            text.push_str(&format!("C:\\many\\{i}.txt\r\n"));
        }
        std::fs::write(&big, &text).unwrap();
        let q = format!("include-file-list:\"{}\" x", big.display());
        let t = terms(&q).remove(0);
        let capped = Reading {
            cap: GOZD_MAX_TERMS + 1,
            stale: &never,
        };
        let got = names::<()>(&t, &Slots::default(), capped).unwrap();
        assert_eq!(got.len(), GOZD_MAX_TERMS + 1);
        assert_eq!(got[0], r"C:\many\0.txt");
        let NotAsked::Refused(why) = gozd(&q, &Slots::default(), true).unwrap_err() else {
            panic!("refused")
        };
        assert!(
            why.starts_with("include-file-list: names more than 256 items")
                && why.contains("open-filelist:"),
            "{why}"
        );
        // Exactly 256 names are asked.
        let fits = dir.join("fits.efu");
        let mut text = String::from("Filename\r\n");
        for i in 0..GOZD_MAX_TERMS {
            text.push_str(&format!("C:\\many\\{i}.txt\r\n"));
        }
        std::fs::write(&fits, &text).unwrap();
        let q = format!("include-file-list:\"{}\"", fits.display());
        assert!(gozd(&q, &Slots::default(), true).is_ok());
        // Stale: the read stops, and nothing half read is answered.
        let q = format!("include-file-list:\"{}\" x", big.display());
        let stale = Reading {
            cap: usize::MAX,
            stale: &|| true,
        };
        assert_eq!(
            names::<()>(&t, &Slots::default(), stale),
            Err(NotAsked::Stale)
        );
        assert!(matches!(
            split_for_list(&q, &Slots::default(), &|| true),
            Ok(None)
        ));
        let gone = |stale: &dyn Fn() -> bool| {
            for_gozd(&q, &Slots::default(), true, false, &mut none, stale).unwrap_err()
        };
        let asked = std::cell::Cell::new(0);
        let later = || {
            asked.set(asked.get() + 1);
            true
        };
        // The cap is reached before the first ask (every 4,096 rows): the
        // refusal, not a stale read.
        assert!(matches!(gone(&later), NotAsked::Refused(_)));
        assert_eq!(asked.get(), 0);
        // A file whose first rows name nothing: a stale search's read
        // stops at the first ask, and gozd is asked nothing.
        let blank = dir.join("blank.efu");
        let mut text = String::from("Filename,Size\r\n");
        for _ in 0..5_000 {
            text.push_str(",1\r\n");
        }
        text.push_str("C:\\late.txt,1\r\n");
        std::fs::write(&blank, &text).unwrap();
        let q = format!("include-file-list:\"{}\"", blank.display());
        assert_eq!(
            for_gozd(&q, &Slots::default(), true, false, &mut none, &|| true).unwrap_err(),
            NotAsked::Stale
        );
        assert_eq!(
            gozd(&q, &Slots::default(), true).unwrap(),
            r#"<whole:"C:\late.txt">"#
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W8: `filelist-filename:` holds for every row of a list of that name
    /// (an empty value: any list), and for nothing of gozd's index.
    #[test]
    fn filelist_filename_is_the_lists_own_name() {
        let none = Slots::default();
        let all: Vec<&str> = ROWS.iter().map(|r| r.0).collect();
        for q in [
            "filelist-filename:\"test list.efu\"",
            r#"filelist-filename:"C:\Lists\Test List.efu""#,
            "filelist-filename:*.efu",
            "filelist-filename:a.efu;\"Test List.efu\"",
            "filelist-filename:",
        ] {
            assert_eq!(hits(q, &none), all, "{q}");
        }
        for q in [
            "filelist-filename:other.efu",
            r"filelist-filename:D:\Lists\Test List.efu",
            "!filelist-filename:",
        ] {
            assert_eq!(
                hits(q.trim_start_matches('!'), &none).is_empty(),
                !q.starts_with('!'),
                "{q}"
            );
        }
        assert_eq!(
            gozd("a filelist-filename:x.efu", &none, true).unwrap(),
            "a whole:\"\""
        );
        assert_eq!(
            gozd("!filelist-filename:", &none, true).unwrap(),
            "!whole:\"\""
        );
        assert_eq!(
            gozd("filelist-filename:x.efu", &none, false).unwrap_err(),
            NotAsked::Refused(needs_syntax("filelist-filename:"))
        );
        assert_eq!(
            needs_boolean("x save-filelist1: new-search: filelist-filename:a"),
            Some(needs_syntax("filelist-filename:"))
        );
        assert_eq!(needs_boolean("x save-filelist1: new-search: y"), None);
    }

    /// W8: `open-filelist:` names the list the search runs over; it tests
    /// nothing of a row, and gozd is never asked in its place.
    #[test]
    fn open_filelist_names_the_list_searched() {
        let none = Slots::default();
        assert_eq!(
            open_list(r#"x open-filelist:"C:\a b\l.efu" y"#),
            Some(Ok(r"C:\a b\l.efu".to_owned()))
        );
        assert_eq!(
            open_list(r"open-filelist:C:/l.efu Open-FileList:c:\L.EFU"),
            Some(Ok(r"C:\l.efu".to_owned()))
        );
        assert!(open_list("abc").is_none() && open_list("filelist:a").is_none());
        assert!(
            open_list("open-filelist:l.efu")
                .unwrap()
                .unwrap_err()
                .contains("full path")
        );
        assert!(
            open_list(r"open-filelist:C:\a.efu open-filelist:C:\b.efu")
                .unwrap()
                .unwrap_err()
                .contains("second file list")
        );
        let q = r"open-filelist:C:\l.efu *.txt";
        assert_eq!(hits(q, &none).len(), ROWS.len());
        assert_eq!(split(q, &none).unwrap().0.trim(), "*.txt");
        let NotAsked::Refused(why) = gozd(q, &none, true).unwrap_err() else {
            panic!()
        };
        assert!(why.contains("has not opened"), "{why}");
        // Not of a gozd that reads no OR either: the same reason.
        assert!(needs_boolean(q).is_none());
    }
}
