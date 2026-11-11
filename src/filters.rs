//! The user's own filters (Search > Add to Filters..., Organize Filters...)
//! and `Filters.csv`, where they are kept in the data folder in Everything
//! 1.5's format: a header naming the columns (`Name`, `Case`, `Whole
//! Word`, `Path`, `Diacritics`, ..., `Search`, ..., `Macro`, `Key`), then a
//! row per filter, Everything's own eight first under its names
//! (`EVERYTHING`, `AUDIO`, ...). So Everything's file can be copied over
//! goz-gui's or imported, and goz-gui's read by Everything. Columns are
//! read by their names, in any order; every column goz-gui does not edit
//! (Columns, Sort, View, Macro, Key, and any it does not know) is written
//! back as it was read.
//!
//! goz-gui edits a filter's Name, Search, Match case and Match path. A
//! filter that also needs a match option gozd cannot apply yet (whole
//! words, diacritics, prefix, suffix, punctuation, white-space, regular
//! expressions) is kept and listed in Organize Filters, saying so, but not
//! offered in the menus: searching with it would find the wrong files. The
//! built-in filters are goz-gui's own (`crate::filter`), listed first and
//! not edited.
//!
//! A custom filter is a `Filter::Custom(id)` everywhere else: `id` names
//! one filter for as long as goz-gui runs, wherever the list moves it
//! (a navigation entry, the window's options), and is never given to
//! another. Its name and menu label are kept as `&'static str` (each
//! distinct name once), so `Filter` stays a plain `Copy` value and its
//! names borrow from nothing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};

use crate::csv::{self, Field};
use crate::filter::Filter;

/// The file's name in the data folder, as Everything's.
pub const FILE_NAME: &str = "Filters.csv";

/// Everything's columns, in the order goz-gui writes them.
pub const COLUMNS: [&str; 17] = [
    "Name",
    "Case",
    "Whole Word",
    "Path",
    "Diacritics",
    "Prefix",
    "Suffix",
    "Ignore Punctuation",
    "Ignore Whitespace",
    "Regex",
    "Search",
    "Columns",
    "Sort",
    "Descending",
    "View",
    "Macro",
    "Key",
];

/// The match options goz cannot apply yet, by column, with what Organize
/// Filters calls them.
const UNSUPPORTED: [(&str, &str); 7] = [
    ("Whole Word", "whole words"),
    ("Diacritics", "diacritics"),
    ("Prefix", "prefix"),
    ("Suffix", "suffix"),
    ("Ignore Punctuation", "ignore punctuation"),
    ("Ignore Whitespace", "ignore white-space"),
    ("Regex", "regular expressions"),
];

/// Everything's names for its own filters in the file (it shows them in
/// the user's language); `PICTURE` is Image's name in older files.
const BUILTIN_NAMES: [(&str, Filter); 9] = [
    ("EVERYTHING", Filter::Everything),
    ("AUDIO", Filter::Audio),
    ("COMPRESSED", Filter::Compressed),
    ("DOCUMENT", Filter::Document),
    ("EXECUTABLE", Filter::Executable),
    ("FOLDER", Filter::Folder),
    ("IMAGE", Filter::Image),
    ("PICTURE", Filter::Image),
    ("VIDEO", Filter::Video),
];

/// Everything's macros for its own filters (`audio:` in its searches).
fn builtin_macro(f: Filter) -> &'static str {
    match f {
        Filter::Audio => "audio",
        Filter::Compressed => "zip",
        Filter::Document => "doc",
        Filter::Executable => "exe",
        Filter::Folder => "dir",
        Filter::Image => "image",
        Filter::Video => "video",
        _ => "",
    }
}

/// The built-in filter a row of the file is, by its name.
pub fn builtin_of(name: &str) -> Option<Filter> {
    let name = name.trim();
    BUILTIN_NAMES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, f)| f)
        .or_else(|| {
            Filter::ALL
                .into_iter()
                .find(|f| f.title().eq_ignore_ascii_case(name))
        })
}

/// One filter: a row of the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterDef {
    pub name: String,
    pub search: String,
    pub case: bool,
    pub path: bool,
    /// The other columns as read, by name, written back as they were.
    pub other: Vec<(String, Field)>,
}

fn flag(field: Option<&Field>) -> bool {
    field.is_some_and(|f| {
        let t = f.text.trim();
        !t.is_empty() && t != "0"
    })
}

impl FilterDef {
    /// A new filter of goz-gui's: its options and search, the columns it
    /// does not edit unset, as Everything writes a new one.
    pub fn new(name: &str, search: &str, case: bool, path: bool) -> FilterDef {
        FilterDef {
            name: name.to_owned(),
            search: search.to_owned(),
            case,
            path,
            other: Vec::new(),
        }
    }

    fn other(&self, column: &str) -> Option<&Field> {
        self.other
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(column))
            .map(|(_, f)| f)
    }

    /// The match options it asks for that goz cannot apply yet.
    pub fn unsupported(&self) -> Vec<&'static str> {
        UNSUPPORTED
            .iter()
            .filter(|(column, _)| flag(self.other(column)))
            .map(|&(_, what)| what)
            .collect()
    }

    /// Whether goz can search as the filter asks.
    pub fn usable(&self) -> bool {
        self.unsupported().is_empty()
    }

    /// Its Macro column: kept and written back, not used (goz has no
    /// search macros).
    #[cfg(test)]
    pub fn macro_name(&self) -> &str {
        self.other("Macro").map_or("", |f| f.text.as_str())
    }

    /// The terms it adds in front of the user's search: its search on one
    /// line (several lines are searched as one), each text term a full
    /// path term (`path:`) with Match path, and `case:` with Match case
    /// (gozd's case is the whole search's: it then matches case
    /// throughout). `None` for an empty search, which keeps every result.
    pub fn terms(&self) -> Option<String> {
        let search = one_line(&self.search);
        let search = search.trim();
        if search.is_empty() {
            return None;
        }
        let mut out = String::new();
        if self.case {
            out.push_str("case: ");
        }
        if self.path {
            let tokens: Vec<String> = tokens(search)
                .into_iter()
                .map(|t| {
                    if is_function(&t) {
                        t
                    } else {
                        format!("path:{t}")
                    }
                })
                .collect();
            out.push_str(&tokens.join(" "));
        } else {
            out.push_str(search);
        }
        Some(out)
    }

    /// The row for the file, its columns in `columns`' order.
    fn row(&self, columns: &[String]) -> Vec<Field> {
        let bit = |b: bool| Field::plain(u8::from(b).to_string());
        columns
            .iter()
            .map(|c| match c.as_str() {
                "Name" => Field::quoted(self.name.clone()),
                "Search" => Field::quoted(self.search.clone()),
                "Case" => bit(self.case),
                "Path" => bit(self.path),
                other => self
                    .other(other)
                    .cloned()
                    .unwrap_or_else(|| default_field(other)),
            })
            .collect()
    }
}

/// What Everything writes in a column a filter leaves unset.
fn default_field(column: &str) -> Field {
    match column {
        "Columns" | "View" | "Macro" | "Sort" => Field::quoted(""),
        "Key" => Field::plain("0"),
        c if COLUMNS.contains(&c) => Field::plain("0"),
        _ => Field::plain(""),
    }
}

/// `text` on one line: line breaks become spaces.
pub fn one_line(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

/// `text` split as goz splits a search: on white-space outside quotes, the
/// quotes kept with their token.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                cur.push(ch);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether `token` is a `name:` function to gozd: letters (two or more)
/// up to a colon before any quote.
fn is_function(token: &str) -> bool {
    let Some(colon) = token.find(':') else {
        return false;
    };
    let name = &token[..colon];
    name.len() > 1 && name.bytes().all(|b| b.is_ascii_alphabetic())
}

/// The filters the file holds: Everything's built-in rows (as read, or
/// goz-gui's when the file has none), then the user's, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    /// One row per built-in filter, in `Filter::ALL`'s order.
    pub builtins: Vec<FilterDef>,
    pub custom: Vec<FilterDef>,
    /// Columns the file had that goz-gui does not know, kept in order.
    pub extra_columns: Vec<String>,
}

/// The row goz-gui writes for a built-in filter that was not read.
fn builtin_row(f: Filter) -> FilterDef {
    let name = BUILTIN_NAMES
        .iter()
        .find(|&&(_, b)| b == f)
        .map_or("", |&(n, _)| n);
    FilterDef {
        name: name.to_owned(),
        search: f.search(),
        case: false,
        path: false,
        other: vec![("Macro".to_owned(), Field::quoted(builtin_macro(f)))],
    }
}

impl Default for List {
    fn default() -> List {
        List {
            builtins: Filter::ALL.into_iter().map(builtin_row).collect(),
            custom: Vec::new(),
            extra_columns: Vec::new(),
        }
    }
}

impl List {
    /// The file's text: the header, the built-in rows, the user's.
    pub fn to_csv(&self) -> String {
        let mut columns: Vec<String> = COLUMNS.iter().map(|&c| c.to_owned()).collect();
        columns.extend(self.extra_columns.iter().cloned());
        let mut out = String::new();
        let header: Vec<Field> = columns.iter().map(|c| Field::plain(c.clone())).collect();
        csv::write_record(&header, &mut out);
        for def in self.builtins.iter().chain(&self.custom) {
            csv::write_record(&def.row(&columns), &mut out);
        }
        out
    }

    /// Reads a file's bytes (UTF-8, with or without a byte order mark).
    pub fn parse(bytes: &[u8]) -> Result<List, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())?;
        let checked = csv::parse_checked(text);
        if let Some(problem) = checked.problem() {
            return Err(problem);
        }
        let mut records = checked.records.into_iter();
        let Some(header) = records.next() else {
            // An empty file: the defaults.
            return Ok(List::default());
        };
        let names: Vec<String> = header.iter().map(|f| f.text.trim().to_owned()).collect();
        let Some(name_at) = names.iter().position(|n| n.eq_ignore_ascii_case("Name")) else {
            return Err("its first line names no Name column".into());
        };
        let known = |n: &str| COLUMNS.iter().any(|c| c.eq_ignore_ascii_case(n));
        let extra_columns: Vec<String> = names
            .iter()
            .filter(|n| !n.is_empty() && !known(n))
            .cloned()
            .collect();
        let mut builtins: Vec<Option<FilterDef>> = vec![None; Filter::ALL.len()];
        let mut custom = Vec::new();
        for record in records {
            let get = |column: &str| {
                names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(column))
                    .and_then(|i| record.get(i))
            };
            let name = record
                .get(name_at)
                .map(|f| f.text.clone())
                .unwrap_or_default();
            if name.trim().is_empty() {
                continue;
            }
            let mut def = FilterDef {
                name: name.clone(),
                search: get("Search").map(|f| f.text.clone()).unwrap_or_default(),
                case: flag(get("Case")),
                path: flag(get("Path")),
                other: Vec::new(),
            };
            for (i, column) in names.iter().enumerate() {
                let edited = ["Name", "Search", "Case", "Path"]
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(column));
                if column.is_empty() || edited {
                    continue;
                }
                if let Some(field) = record.get(i) {
                    // Under Everything's own spelling, whatever the file's.
                    let column = COLUMNS
                        .iter()
                        .find(|c| c.eq_ignore_ascii_case(column))
                        .map_or_else(|| column.clone(), |c| (*c).to_owned());
                    def.other.push((column, field.clone()));
                }
            }
            match builtin_of(&name) {
                Some(f) => {
                    let at = f.index();
                    if builtins[at].is_none() {
                        builtins[at] = Some(def);
                    }
                }
                None => custom.push(def),
            }
        }
        Ok(List {
            builtins: builtins
                .into_iter()
                .zip(Filter::ALL)
                .map(|(row, f)| row.unwrap_or_else(|| builtin_row(f)))
                .collect(),
            custom,
            extra_columns,
        })
    }

    /// Merges `other` in (Import...): a filter of the same name (any case)
    /// takes the imported one's place, the others are added after. The
    /// built-in rows stay as they are. How many filters came in.
    pub fn import(&mut self, other: List) -> usize {
        let mut n = 0;
        for def in other.custom {
            match self
                .custom
                .iter_mut()
                .find(|d| d.name.eq_ignore_ascii_case(&def.name))
            {
                Some(slot) => *slot = def,
                None => self.custom.push(def),
            }
            n += 1;
        }
        for column in other.extra_columns {
            if !self
                .extra_columns
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&column))
            {
                self.extra_columns.push(column);
            }
        }
        n
    }
}

/// Why `name` cannot be a new filter's (or `editing`'s new) name, if it
/// cannot: empty, a built-in's, or another filter's.
pub fn name_problem(list: &[FilterDef], name: &str, editing: Option<usize>) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Some("Type a name for the filter.".into());
    }
    let taken = builtin_of(trimmed).is_some()
        || list
            .iter()
            .enumerate()
            .any(|(i, d)| Some(i) != editing && d.name.trim().eq_ignore_ascii_case(trimmed));
    taken.then(|| format!("There is already a filter named {trimmed}."))
}

/// `text` kept for the rest of the process: the names `Filter::title`
/// hands out live as long as any `Filter` might.
fn intern(text: &str) -> &'static str {
    static NAMES: LazyLock<Mutex<HashSet<&'static str>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&kept) = names.get(text) {
        return kept;
    }
    let kept: &'static str = Box::leak(text.to_owned().into_boxed_str());
    names.insert(kept);
    kept
}

/// A custom filter as the rest of goz-gui sees it.
#[derive(Clone, Debug)]
struct Slot {
    name: &'static str,
    /// Its menu label (`&` doubled).
    label: &'static str,
    terms: Option<String>,
    /// In the list now (not deleted).
    live: bool,
    usable: bool,
}

/// The custom filters by id, and their order.
#[derive(Debug, Default)]
pub struct Registry {
    slots: Vec<Slot>,
    /// The ids of the list's custom filters, in its order.
    order: Vec<u16>,
}

impl Registry {
    /// Takes `defs` as the custom filters, in their order. A filter keeps
    /// the id it had under the same name (the n-th of that name keeps the
    /// n-th's); one gone keeps its id, no longer listed; a new one gets the
    /// next (past 65,535 names in one run, the rest are left out).
    pub fn set(&mut self, defs: &[FilterDef]) {
        for slot in &mut self.slots {
            slot.live = false;
        }
        self.order.clear();
        for def in defs {
            let name = def.name.trim();
            // The first slot of this name not taken by an earlier one (a
            // slot taken is live again).
            let found = self
                .slots
                .iter()
                .position(|s| !s.live && s.name.eq_ignore_ascii_case(name));
            let slot = Slot {
                name: intern(name),
                label: intern(&name.replace('&', "&&")),
                terms: def.terms(),
                live: true,
                usable: def.usable(),
            };
            let id = match found {
                Some(i) => {
                    self.slots[i] = slot;
                    i
                }
                None => {
                    if self.slots.len() >= usize::from(u16::MAX) {
                        continue;
                    }
                    self.slots.push(slot);
                    self.slots.len() - 1
                }
            };
            self.order.push(id as u16);
        }
    }

    /// The custom filters the menus offer, in order: listed and usable.
    pub fn listed(&self) -> Vec<u16> {
        self.order
            .iter()
            .copied()
            .filter(|&id| self.slots[usize::from(id)].usable)
            .collect()
    }

    pub fn name(&self, id: u16) -> &'static str {
        self.slots.get(usize::from(id)).map_or("", |s| s.name)
    }

    pub fn label(&self, id: u16) -> &'static str {
        self.slots.get(usize::from(id)).map_or("", |s| s.label)
    }

    pub fn terms(&self, id: u16) -> Option<String> {
        self.slots.get(usize::from(id))?.terms.clone()
    }

    /// The listed, usable filter named `name` (any case).
    pub fn find(&self, name: &str) -> Option<u16> {
        let name = name.trim();
        self.listed()
            .into_iter()
            .find(|&id| self.slots[usize::from(id)].name.eq_ignore_ascii_case(name))
    }

    /// Whether `id` is listed and usable.
    pub fn is_listed(&self, id: u16) -> bool {
        self.slots
            .get(usize::from(id))
            .is_some_and(|s| s.live && s.usable)
    }
}

/// The file as last read or written.
#[derive(Debug, Default)]
pub struct Store {
    pub path: Option<PathBuf>,
    pub list: List,
    pub stamp: Option<crate::store::file::Stamp>,
    /// Its bytes when it was read whole: the backup the next write keeps.
    pub previous: Option<Vec<u8>>,
    /// Why it could not be read: it is not written over.
    pub unreadable: Option<String>,
    /// What to tell the user about reading it, once.
    pub note: Option<String>,
    /// A change not written yet (a write that failed is tried again).
    pub pending: bool,
}

struct Shared {
    registry: Registry,
    store: Store,
    loaded: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| {
    Mutex::new(Shared {
        registry: Registry::default(),
        store: Store::default(),
        loaded: false,
    })
});

/// The shared state, the file read first: at the first use, which is the
/// settings being read as goz-gui starts (they may name a custom filter).
/// Tests never read the user's file.
fn shared() -> MutexGuard<'static, Shared> {
    let mut s = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    if !s.loaded {
        s.loaded = true;
        #[cfg(not(test))]
        {
            let store = read(crate::store::location::data_file(FILE_NAME));
            s.registry.set(&store.list.custom);
            s.store = store;
        }
    }
    s
}

/// Reads the file at `path` (`None`: no data folder, nothing kept).
pub fn read(path: Option<PathBuf>) -> Store {
    use crate::store::file::{self, Loaded};
    let Some(path) = path else {
        return Store::default();
    };
    // Another goz-gui writing the file goes first.
    let _lock = file::Lock::acquire(&path, std::time::Duration::from_secs(2));
    let loaded = file::load_checked(&path, List::parse, csv::suspect);
    let stamp = file::stamp(&path);
    let mut store = Store {
        path: Some(path),
        stamp,
        ..Store::default()
    };
    match loaded {
        Loaded::Missing => {}
        Loaded::Read(list, bytes) => {
            store.list = list;
            store.previous = Some(bytes);
        }
        Loaded::Recovered {
            backup,
            moved_to,
            why,
            ..
        } => {
            let name = moved_to
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let then = if backup.is_some() {
                "its backup was used"
            } else {
                "the filters start from the defaults"
            };
            store.note = Some(format!(
                "The filters could not be read ({why}); the file was kept as {name} and {then}"
            ));
            store.list = backup.unwrap_or_default();
        }
        Loaded::Unreadable(why) => {
            store.note = Some(format!("The filters could not be read: {why}"));
            store.unreadable = Some(why);
        }
    }
    store
}

/// The custom filters the menus offer, in order, by id.
pub fn listed() -> Vec<u16> {
    shared().registry.listed()
}

/// A custom filter's name (`""` for none such).
pub fn name(id: u16) -> &'static str {
    shared().registry.name(id)
}

/// A custom filter's menu label.
pub fn label(id: u16) -> &'static str {
    shared().registry.label(id)
}

/// The terms a custom filter adds to a search.
pub fn terms(id: u16) -> Option<String> {
    shared().registry.terms(id)
}

/// The listed custom filter named `name`.
pub fn find(name: &str) -> Option<u16> {
    shared().registry.find(name)
}

/// Whether the custom filter `id` is listed (not deleted, usable).
pub fn is_listed(id: u16) -> bool {
    shared().registry.is_listed(id)
}

/// The filters as the file has them now, for Organize Filters.
pub fn list() -> List {
    shared().store.list.clone()
}

/// What to tell the user about reading the file, once.
pub fn take_note() -> Option<String> {
    shared().store.note.take()
}

/// Makes `list` the filters (Organize Filters' OK, Add to Filters): the
/// menus and the window take them now; the file is written by the caller
/// (`write_job`).
pub fn set(list: List) {
    let mut s = shared();
    s.registry.set(&list.custom);
    s.store.list = list;
    s.store.pending = true;
}

/// Reads the file again when another program (another goz-gui, Everything,
/// an editor) changed it since it was last read or written. True when the
/// filters changed.
pub fn refresh() -> bool {
    let path = {
        let s = shared();
        match &s.store.path {
            Some(p) if crate::store::file::stamp(p) != s.store.stamp => p.clone(),
            _ => return false,
        }
    };
    let store = read(Some(path));
    let mut s = shared();
    if s.store.pending {
        // A change of this window's not written yet is written over
        // theirs, as Everything's last write wins.
        return false;
    }
    if store.unreadable.is_some() {
        // Kept as it is; still not written over.
        s.store.unreadable = store.unreadable;
        s.store.note = store.note;
        return false;
    }
    let changed = store.list != s.store.list;
    s.registry.set(&store.list.custom);
    s.store = store;
    changed
}

/// What the store thread writes: the file's path and text, and the bytes
/// kept as its backup (with `backup` on). `Ok(None)` when no change waits;
/// `Err` when it is not to be written (no data folder, or a file that could
/// not be read).
#[allow(clippy::type_complexity)]
pub fn write_job(backup: bool) -> Result<Option<(PathBuf, String, Option<Vec<u8>>)>, String> {
    let s = shared();
    if !s.store.pending {
        return Ok(None);
    }
    if let Some(why) = &s.store.unreadable {
        return Err(format!(
            "{FILE_NAME} could not be read, so goz-gui does not write over it: {why}"
        ));
    }
    let Some(path) = s.store.path.clone() else {
        return Err("The filters could not be saved: goz-gui has no data folder.".into());
    };
    let previous = if backup {
        s.store.previous.clone()
    } else {
        None
    };
    Ok(Some((path, s.store.list.to_csv(), previous)))
}

/// The store thread wrote `text` to the file: it is the backup next, its
/// new stamp is not another program's change, and nothing waits unless
/// the filters changed again meanwhile.
pub fn written(text: String) {
    let mut s = shared();
    if let Some(path) = &s.store.path {
        s.store.stamp = crate::store::file::stamp(path);
    }
    if s.store.list.to_csv() == text {
        s.store.pending = false;
    }
    s.store.previous = Some(text.into_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::{LocalClock, parse_query_at};

    struct Clock;
    impl LocalClock for Clock {
        fn now(&self) -> i64 {
            133_801_632_000_000_000
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

    fn parses(q: &str) -> goz_core::query::ParsedQuery {
        parse_query_at(q, false, &Clock).unwrap_or_else(|e| panic!("{q}: {e}"))
    }

    #[test]
    fn everythings_file_is_read_by_its_column_names() {
        let text = "Name,Case,Whole Word,Path,Diacritics,Regex,Search,Macro,Key,Extra\r\n\
                    \"EVERYTHING\",0,0,0,0,0,\"\",\"\",0,x\r\n\
                    \"AUDIO\",0,0,0,0,0,\"ext:mp3\",\"audio\",0,y\r\n\
                    \"Big PDFs\",1,0,1,0,0,\"*.pdf size:>1mb\",\"bigpdf\",0,z\r\n\
                    \"Regex one\",0,0,0,0,1,\"^a.*\",\"\",0,\r\n";
        let list = List::parse(text.as_bytes()).expect("parses");
        assert_eq!(list.builtins.len(), Filter::ALL.len());
        // The audio row as read; a built-in the file lacks, goz-gui's.
        assert_eq!(list.builtins[1].search, "ext:mp3");
        assert_eq!(list.builtins[2].name, "COMPRESSED");
        assert_eq!(list.custom.len(), 2);
        let big = &list.custom[0];
        assert_eq!(
            (big.name.as_str(), big.case, big.path),
            ("Big PDFs", true, true)
        );
        assert_eq!(big.macro_name(), "bigpdf");
        assert!(big.usable());
        assert_eq!(list.custom[1].unsupported(), ["regular expressions"]);
        assert!(!list.custom[1].usable());
        assert_eq!(list.extra_columns, ["Extra"]);
        // Written back: Everything's columns, then the unknown one, each
        // row's other columns as they were.
        let out = list.to_csv();
        let back = List::parse(out.as_bytes()).expect("parses again");
        // Read back, it is the same list, and written again the same text.
        assert_eq!(back.custom.len(), 2);
        assert_eq!(
            (back.custom[0].name.as_str(), back.custom[0].search.as_str()),
            ("Big PDFs", "*.pdf size:>1mb")
        );
        assert_eq!(back.to_csv(), out);
        assert!(out.starts_with(
            "Name,Case,Whole Word,Path,Diacritics,Prefix,Suffix,Ignore Punctuation,\
             Ignore Whitespace,Regex,Search,Columns,Sort,Descending,View,Macro,Key,Extra\r\n"
        ));
        assert!(out.contains(
            "\"Big PDFs\",1,0,1,0,0,0,0,0,0,\"*.pdf size:>1mb\",\"\",\"\",0,\"\",\"bigpdf\",0,z\r\n"
        ));
    }

    #[test]
    fn the_defaults_are_everythings_rows() {
        let list = List::default();
        let text = list.to_csv();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1 + Filter::ALL.len());
        assert!(lines[1].starts_with("\"EVERYTHING\",0,"));
        assert!(lines[2].starts_with("\"AUDIO\",0,0,0,0,0,0,0,0,0,\"ext:aac;ac3;"));
        assert!(lines[2].ends_with(",\"audio\",0"));
        assert!(lines[6].contains("\"folder:\"") && lines[6].contains("\"dir\""));
        assert_eq!(List::parse(text.as_bytes()).expect("parses").to_csv(), text);
        // An empty file is the defaults; a file without a Name column is
        // not a filter list.
        assert_eq!(List::parse(b"").expect("empty"), List::default());
        assert!(List::parse(b"Search,Case\r\nx,0\r\n").is_err());
        assert!(List::parse(b"Name\r\n\"cut sh").is_err());
    }

    #[test]
    fn a_filters_terms_parse_as_goz_does() {
        let plain = FilterDef::new("A", "*.pdf  report", false, false);
        assert_eq!(plain.terms().as_deref(), Some("*.pdf  report"));
        let path = FilterDef::new("B", "docs \"my files\" ext:pdf", false, true);
        let terms = path.terms().expect("terms");
        assert_eq!(terms, "path:docs path:\"my files\" ext:pdf");
        let q = parses(&terms);
        assert_eq!(q.path_terms.len(), 2);
        assert!(q.name_terms.is_empty());
        assert!(q.filters.ext.is_some());
        let case = FilterDef::new("C", "Report\r\nSummary", true, false);
        let terms = case.terms().expect("terms");
        assert_eq!(terms, "case: Report Summary");
        assert!(parses(&terms).match_case);
        assert_eq!(FilterDef::new("D", "  \r\n ", true, true).terms(), None);
    }

    #[test]
    fn names_must_be_new() {
        let list = vec![FilterDef::new("Mine", "x", false, false)];
        assert!(name_problem(&list, "", None).is_some());
        assert!(name_problem(&list, "audio", None).is_some());
        assert!(name_problem(&list, "EVERYTHING", None).is_some());
        assert!(name_problem(&list, " mine ", None).is_some());
        assert_eq!(name_problem(&list, "Mine", Some(0)), None);
        assert_eq!(name_problem(&list, "Other", None), None);
    }

    #[test]
    fn ids_stay_with_their_filters() {
        let mut r = Registry::default();
        let a = FilterDef::new("A & B", "a", false, false);
        let b = FilterDef::new("B", "b", false, false);
        let mut bad = FilterDef::new("Bad", "c", false, false);
        bad.other.push(("Regex".into(), Field::plain("1")));
        r.set(&[a.clone(), b.clone(), bad.clone()]);
        assert_eq!(r.listed(), [0, 1]);
        assert_eq!((r.name(0), r.label(0)), ("A & B", "A && B"));
        assert_eq!(r.find("b"), Some(1));
        assert_eq!(r.find("Bad"), None, "not offered");
        // Moved, one deleted, one added: the ids follow the names.
        let c = FilterDef::new("C", "c", false, false);
        r.set(&[b.clone(), c.clone()]);
        assert_eq!(r.listed(), [1, 3]);
        assert!(!r.is_listed(0) && r.is_listed(1));
        // The deleted one keeps its name for whoever still holds it.
        assert_eq!(r.name(0), "A & B");
        // Back again: its old id.
        r.set(&[a, b, c]);
        assert_eq!(r.listed(), [0, 1, 3]);
        assert_eq!(r.terms(3).as_deref(), Some("c"));
    }

    #[test]
    fn import_merges_by_name() {
        let mut list = List::default();
        list.custom
            .push(FilterDef::new("Mine", "old", false, false));
        let mut other = List::default();
        other
            .custom
            .push(FilterDef::new("MINE", "new", false, false));
        other
            .custom
            .push(FilterDef::new("Theirs", "t", true, false));
        other.extra_columns.push("Icon".into());
        assert_eq!(list.import(other), 2);
        assert_eq!(list.custom.len(), 2);
        assert_eq!(list.custom[0].search, "new");
        assert_eq!(list.custom[1].name, "Theirs");
        assert_eq!(list.extra_columns, ["Icon"]);
    }
}
