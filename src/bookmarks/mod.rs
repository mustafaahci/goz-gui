//! Bookmarks: saved searches with their options, kept in `Bookmarks.csv`
//! in the data folder (`crate::store::location`) in Everything 1.5's own
//! format, so a user can carry the file between the two.
//!
//! Everything 1.5.0.1424b's format, as its own reader and writer handle it
//! (read from its code; its file has these 25 columns in this order):
//!
//! - `Type`: `0` a bookmark, `1` a folder, `2` a separator.
//! - `Folder`: the folder a row is in, its names from the top joined by `/`
//!   (`Music/Rock`); empty at the top. A folder row's own place is its
//!   Folder and its Name (`Music` + `Rock`). A folder holds the rows that
//!   name it, in file order; a folder rows name but no row makes is there
//!   all the same.
//! - The match options (`Case`, `Path`, …): empty is "(No change)", `0`
//!   "Disabled", anything else "Enabled".
//! - `Search`: in quotes when set (even when empty); bare and empty for
//!   "(No change)", which keeps the search in the box.
//! - `Filter` and `Sort`: empty is "(No change)". `Descending`: not `0`
//!   sorts descending.
//! - A separator has nothing but its Type and Folder (and an empty Name).
//!
//! goz-gui edits Name, Folder, Search, Case, Path, Filter, Sort and
//! Descending. It keeps whatever a row had in the other columns, and the
//! columns a file has that are not Everything's (written after them), so a
//! bookmark made in Everything loses nothing here. Rows of a Type goz-gui
//! does not know are kept in their places but not shown.

pub mod file;
pub mod ops;
pub mod tree;

use std::sync::Arc;

use goz_core::types::{SortDir, SortKey, SortSpec};

use crate::csv::{self, Field};
use crate::filter::Filter;

/// Everything 1.5's columns, in its order.
pub const COLUMNS: [&str; 25] = [
    "Name",
    "Type",
    "Folder",
    "Case",
    "Whole Word",
    "Path",
    "Diacritic",
    "Prefix",
    "Suffix",
    "Ignore Punctuation",
    "Ignore Whitespace",
    "Regex",
    "Search",
    "Filter",
    "Columns",
    "Sort",
    "Descending",
    "View",
    "Index",
    "File List",
    "Host",
    "Link Type",
    "Macro",
    "Key",
    "Icon",
];

/// What Everything writes in each column of [`COLUMNS`] for a bookmark made
/// with the search options off: the text, and whether it is quoted.
const DEFAULTS: [(&str, bool); 25] = [
    ("", true),   // Name
    ("0", false), // Type
    ("", true),   // Folder
    ("0", false), // Case
    ("0", false), // Whole Word
    ("0", false), // Path
    ("0", false), // Diacritic
    ("0", false), // Prefix
    ("0", false), // Suffix
    ("0", false), // Ignore Punctuation
    ("0", false), // Ignore Whitespace
    ("0", false), // Regex
    ("", true),   // Search
    ("", false),  // Filter
    ("", true),   // Columns
    ("", true),   // Sort
    ("0", false), // Descending
    ("", false),  // View
    ("", false),  // Index
    ("", false),  // File List
    ("", false),  // Host
    ("0", false), // Link Type
    ("", false),  // Macro
    ("", false),  // Key
    ("", true),   // Icon
];

const NAME: usize = 0;
const TYPE: usize = 1;
const FOLDER: usize = 2;
const CASE: usize = 3;
/// The match options, Case to Regex.
const MATCH: core::ops::RangeInclusive<usize> = 3..=11;
const PATH: usize = 5;
const SEARCH: usize = 12;
const FILTER: usize = 13;
const SORT: usize = 15;
const DESCENDING: usize = 16;

/// The search options a bookmark brings back with its search. `None` leaves
/// the option in force as it is (Everything's "(No change)").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub match_case: Option<bool>,
    pub match_path: Option<bool>,
    pub filter: Option<Filter>,
    pub sort: Option<SortSpec>,
}

/// What a row of the file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Bookmark,
    Folder,
    Separator,
    /// A Type goz-gui does not know: kept, not shown.
    Other,
}

impl Kind {
    fn of(text: &str) -> Kind {
        match text.trim() {
            "" | "0" => Kind::Bookmark,
            "1" => Kind::Folder,
            "2" => Kind::Separator,
            _ => Kind::Other,
        }
    }
}

/// A folder's place: its names from the top (`["Music", "Rock"]`); empty
/// for the top.
pub type FolderPath = Vec<String>;

/// The Folder column's text as a place: its names between `/`s (empty ones
/// left out, so `Music/` and `/Music` are `Music`).
pub fn split(text: &str) -> FolderPath {
    text.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A place as the Folder column writes it.
pub fn join(path: &[String]) -> String {
    path.join("/")
}

/// Whether two places are the same folder. Folder names are told apart
/// regardless of case, as Windows tells file names.
pub fn same_path(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_name(x, y))
}

/// Whether `path` is `prefix` or inside it.
pub fn within(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && same_path(&path[..prefix.len()], prefix)
}

/// Whether two folder names are the same, regardless of case.
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// Everything's reading of a number column: its leading digits (with a
/// sign), 0 when there are none, as C's `atoi`.
fn atoi(text: &str) -> i64 {
    let t = text.trim_start();
    let (sign, digits) = match t.as_bytes().first() {
        Some(b'-') => (-1, &t[1..]),
        Some(b'+') => (1, &t[1..]),
        _ => (1, t),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            n.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    sign * n
}

/// A match option's column: empty is "(No change)".
fn tri(field: &Field) -> Option<bool> {
    (!field.text.is_empty()).then(|| atoi(&field.text) != 0)
}

fn tri_field(value: Option<bool>) -> Field {
    match value {
        None => Field::plain(""),
        Some(on) => bit(on),
    }
}

fn bit(on: bool) -> Field {
    Field::plain(if on { "1" } else { "0" })
}

/// The Sort column's name for a sort key: the column's title (also what
/// the settings file's `sort` holds).
pub fn sort_name(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "Name",
        SortKey::Path => "Path",
        SortKey::Size => "Size",
        SortKey::DateModified => "Date Modified",
    }
}

/// The sort keys, in the menu's order.
pub const SORT_KEYS: [SortKey; 4] = [
    SortKey::Name,
    SortKey::Path,
    SortKey::Size,
    SortKey::DateModified,
];

/// The sort key named `name`, in any case.
pub fn sort_key(name: &str) -> Option<SortKey> {
    let name = name.trim();
    SORT_KEYS
        .into_iter()
        .find(|&key| sort_name(key).eq_ignore_ascii_case(name))
}

/// The fields the bookmark editor shows and gives back, as their columns
/// hold them, so that what goz-gui cannot show (an Everything filter it
/// lacks, a sort by Extension) goes back unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    pub name: String,
    /// `None`: "(No change)", the search in the box stays.
    pub search: Option<String>,
    pub match_case: Option<bool>,
    pub match_path: Option<bool>,
    /// The Filter column's text: empty for "(No change)".
    pub filter: String,
    /// The Sort column's text: empty for "(No change)".
    pub sort: String,
    pub descending: bool,
}

/// One row of the file: a bookmark, a folder or a separator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    /// A value for each of [`COLUMNS`].
    values: Vec<Field>,
    /// Its values in the file's columns that are not Everything's, by name.
    extra: Vec<(String, Field)>,
}

impl Bookmark {
    /// A row with Everything's defaults.
    fn blank() -> Bookmark {
        Bookmark {
            values: DEFAULTS
                .iter()
                .map(|&(text, quoted)| Field {
                    text: text.to_owned(),
                    quoted,
                })
                .collect(),
            extra: Vec::new(),
        }
    }

    /// A new bookmark in `folder`, as Everything writes one.
    pub fn new(folder: &[String], name: &str, search: &str, settings: &Settings) -> Bookmark {
        let mut row = Bookmark::blank();
        row.set_folder(folder);
        row.set_name(name);
        row.set_search(Some(search));
        row.set_settings(settings);
        row
    }

    /// A new folder named `name` in `parent`, as Everything writes one:
    /// no option, no search.
    pub fn new_folder(parent: &[String], name: &str) -> Bookmark {
        let mut row = Bookmark::blank();
        row.values[TYPE] = Field::plain("1");
        row.set_folder(parent);
        row.set_name(name);
        for i in MATCH {
            row.values[i] = Field::plain("");
        }
        row.values[SEARCH] = Field::plain("");
        row
    }

    /// A new separator in `folder`, as Everything writes one: its Type and
    /// Folder, and nothing else.
    #[cfg(test)]
    pub fn new_separator(folder: &[String]) -> Bookmark {
        let mut row = Bookmark::blank();
        for (i, v) in row.values.iter_mut().enumerate() {
            *v = match i {
                NAME => Field::quoted(""),
                TYPE => Field::plain("2"),
                _ => Field::plain(""),
            };
        }
        row.set_folder(folder);
        row
    }

    pub fn kind(&self) -> Kind {
        Kind::of(&self.values[TYPE].text)
    }

    pub fn name(&self) -> &str {
        &self.values[NAME].text
    }

    pub fn search(&self) -> &str {
        &self.values[SEARCH].text
    }

    /// The bookmark sets a search (Everything's "Custom"); a bare empty
    /// Search is "(No change)".
    pub fn search_set(&self) -> bool {
        let f = &self.values[SEARCH];
        f.quoted || !f.text.is_empty()
    }

    /// The folder the row is in.
    pub fn folder(&self) -> FolderPath {
        split(&self.values[FOLDER].text)
    }

    /// A folder row's own place: its folder and its name.
    pub fn place(&self) -> FolderPath {
        let mut path = self.folder();
        let name = self.name();
        if name.contains('/') {
            path.extend(split(name));
        } else {
            path.push(name.to_owned());
        }
        path
    }

    pub fn set_name(&mut self, name: &str) {
        self.values[NAME] = Field::quoted(name);
    }

    /// Sets the search; `None` is "(No change)".
    pub fn set_search(&mut self, search: Option<&str>) {
        self.values[SEARCH] = match search {
            Some(s) => Field::quoted(s),
            None => Field::plain(""),
        };
    }

    pub fn set_folder(&mut self, folder: &[String]) {
        self.values[FOLDER] = Field::quoted(join(folder));
    }

    /// The options the bookmark sets. A filter or sort goz-gui does not
    /// have (one of the user's own Everything filters, a sort by Extension)
    /// leaves the one in force.
    pub fn settings(&self) -> Settings {
        let sort = sort_key(&self.values[SORT].text).map(|key| SortSpec {
            key,
            dir: if self.descending() {
                SortDir::Desc
            } else {
                SortDir::Asc
            },
        });
        Settings {
            match_case: tri(&self.values[CASE]),
            match_path: tri(&self.values[PATH]),
            filter: Filter::from_title(&self.values[FILTER].text),
            sort,
        }
    }

    fn descending(&self) -> bool {
        atoi(&self.values[DESCENDING].text) != 0
    }

    /// Sets the four options as `settings` has them: `None` is written as
    /// "(No change)".
    pub fn set_settings(&mut self, settings: &Settings) {
        self.values[CASE] = tri_field(settings.match_case);
        self.values[PATH] = tri_field(settings.match_path);
        self.values[FILTER] = match settings.filter {
            Some(filter) => Field::quoted(filter.title()),
            None => Field::plain(""),
        };
        match settings.sort {
            Some(sort) => {
                self.values[SORT] = Field::quoted(sort_name(sort.key));
                self.values[DESCENDING] = bit(sort.dir == SortDir::Desc);
            }
            None => {
                self.values[SORT] = Field::quoted("");
                self.values[DESCENDING] = bit(false);
            }
        }
    }

    /// What the bookmark editor shows.
    pub fn fields(&self) -> Fields {
        Fields {
            name: self.name().to_owned(),
            search: self.search_set().then(|| self.search().to_owned()),
            match_case: tri(&self.values[CASE]),
            match_path: tri(&self.values[PATH]),
            filter: self.values[FILTER].text.clone(),
            sort: self.values[SORT].text.clone(),
            descending: self.descending(),
        }
    }

    /// Writes what the editor changed: a field it left as it was keeps its
    /// column exactly as it was.
    pub fn set_fields(&mut self, new: &Fields) {
        let old = self.fields();
        if new.name != old.name {
            self.set_name(&new.name);
        }
        if new.search != old.search {
            self.set_search(new.search.as_deref());
        }
        if new.match_case != old.match_case {
            self.values[CASE] = tri_field(new.match_case);
        }
        if new.match_path != old.match_path {
            self.values[PATH] = tri_field(new.match_path);
        }
        if new.filter != old.filter {
            self.values[FILTER] = if new.filter.is_empty() {
                Field::plain("")
            } else {
                Field::quoted(new.filter.as_str())
            };
        }
        if new.sort != old.sort {
            self.values[SORT] = Field::quoted(new.sort.as_str());
        }
        if new.descending != old.descending {
            self.values[DESCENDING] = bit(new.descending);
        }
    }

    /// The row's value in an unknown column.
    fn extra_value(&self, column: &str) -> Option<&Field> {
        self.extra.iter().find(|(c, _)| c == column).map(|(_, v)| v)
    }
}

/// What a new bookmark takes from the window (Everything's
/// `bookmark_remember_*` settings): each option, and the search, is kept
/// only where its setting says so; otherwise it is "(No change)".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remember {
    pub case: bool,
    pub path: bool,
    pub filter: bool,
    pub sort: bool,
    pub search: bool,
}

impl Default for Remember {
    /// Everything's: the match options and the search, not the filter or
    /// the sort.
    fn default() -> Remember {
        Remember {
            case: true,
            path: true,
            filter: false,
            sort: false,
            search: true,
        }
    }
}

impl Remember {
    pub fn settings(self, case: bool, path: bool, filter: Filter, sort: SortSpec) -> Settings {
        Settings {
            match_case: self.case.then_some(case),
            match_path: self.path.then_some(path),
            filter: self.filter.then_some(filter),
            sort: self.sort.then_some(sort),
        }
    }
}

/// The rows of a bookmarks file and the columns it had.
///
/// Rows are shared: a copy of the list (the window keeps the file as read
/// and its own changes on it) copies only the rows' handles, and a change
/// copies the row it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bookmarks {
    /// The file's columns that are not Everything 1.5's, in its order; they
    /// are written after Everything's.
    extra: Vec<String>,
    rows: Vec<Arc<Bookmark>>,
}

impl Bookmarks {
    /// The text of a file's bytes, or why they are not a bookmarks file's.
    fn text_of(bytes: &[u8]) -> Result<&str, String> {
        if bytes.contains(&0) {
            return Err("it holds NUL characters".into());
        }
        std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())
    }

    /// Reads the bookmarks file. A file holding NULs, not UTF-8, whose
    /// first line names no `Name` column (a power cut, another file saved
    /// over it), or with a value that is not well-formed CSV (cut short
    /// inside a quote) is not read as bookmarks: `Err` says why, and the
    /// file is moved aside. An empty file has no bookmarks.
    pub fn parse(bytes: &[u8]) -> Result<Bookmarks, String> {
        let checked = csv::parse_checked(Bookmarks::text_of(bytes)?);
        if let Some(problem) = checked.problem() {
            return Err(problem);
        }
        Bookmarks::from_records(checked.records)
    }

    /// [`parse`](Bookmarks::parse) for a file the user imports: a value
    /// that is not well-formed CSV is read as leniently as it can be.
    pub fn parse_import(bytes: &[u8]) -> Result<Bookmarks, String> {
        Bookmarks::from_records(csv::parse(Bookmarks::text_of(bytes)?))
    }

    /// Reads a file's records. Its columns may come in any order, and some
    /// may be missing (an older Everything's file): those get Everything's
    /// defaults. A column goz-gui does not know is kept. `Err` when the
    /// header names no `Name` column.
    fn from_records(records: Vec<Vec<Field>>) -> Result<Bookmarks, String> {
        let mut records = records.into_iter();
        let Some(header) = records.next() else {
            return Ok(Bookmarks::default());
        };
        if !header
            .iter()
            .any(|c| c.text.trim().eq_ignore_ascii_case("Name"))
        {
            return Err("it has no Name column".into());
        }
        let mut extra = Vec::new();
        let mut seen = [false; COLUMNS.len()];
        // Where each of the file's columns goes: one of Everything's, or an
        // unknown one by name.
        let slots: Vec<Result<usize, String>> = header
            .iter()
            .map(|col| {
                let name = col.text.trim();
                match COLUMNS.iter().position(|c| c.eq_ignore_ascii_case(name)) {
                    Some(i) if !seen[i] => {
                        seen[i] = true;
                        Ok(i)
                    }
                    _ => {
                        extra.push(col.text.clone());
                        Err(col.text.clone())
                    }
                }
            })
            .collect();
        let rows = records
            .map(|record| {
                let mut row = Bookmark::blank();
                // Values past the header's columns have no name to keep.
                for (field, slot) in record.into_iter().zip(&slots) {
                    match slot {
                        Ok(i) => row.values[*i] = field,
                        Err(name) => row.extra.push((name.clone(), field)),
                    }
                }
                Arc::new(row)
            })
            .collect();
        Ok(Bookmarks { extra, rows })
    }

    /// The file's text: Everything's header and columns, then the unknown
    /// ones.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        let header: Vec<Field> = COLUMNS
            .iter()
            .map(|&c| Field::plain(c))
            .chain(self.extra.iter().map(|c| Field::plain(c.as_str())))
            .collect();
        csv::write_record(&header, &mut out);
        let blank = Field::plain("");
        for row in &self.rows {
            let extra = self
                .extra
                .iter()
                .map(|c| row.extra_value(c).unwrap_or(&blank));
            csv::write_fields(row.values.iter().chain(extra), &mut out);
        }
        out
    }

    pub fn rows(&self) -> &[Arc<Bookmark>] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Adds `row` at the end; its unknown columns become the list's.
    pub fn push(&mut self, row: impl Into<Arc<Bookmark>>) {
        let row = row.into();
        self.adopt_columns(&row);
        self.rows.push(row);
    }

    /// Puts `row` at `index`.
    pub fn insert(&mut self, index: usize, row: impl Into<Arc<Bookmark>>) {
        let row = row.into();
        self.adopt_columns(&row);
        self.rows.insert(index.min(self.rows.len()), row);
    }

    pub fn remove(&mut self, index: usize) -> Arc<Bookmark> {
        self.rows.remove(index)
    }

    /// Replaces row `index` by `row`.
    pub fn replace(&mut self, index: usize, row: impl Into<Arc<Bookmark>>) {
        let row = row.into();
        self.adopt_columns(&row);
        self.rows[index] = row;
    }

    /// Row `index` to change (a copy, if another list shares it).
    pub fn row_mut(&mut self, index: usize) -> &mut Bookmark {
        Arc::make_mut(&mut self.rows[index])
    }

    fn adopt_columns(&mut self, row: &Bookmark) {
        for (name, _) in &row.extra {
            if !self.extra.contains(name) {
                self.extra.push(name.clone());
            }
        }
    }

    /// A list of `rows`, with the columns of this one.
    pub fn with_rows<R: Into<Arc<Bookmark>>>(
        &self,
        rows: impl IntoIterator<Item = R>,
    ) -> Bookmarks {
        Bookmarks {
            extra: self.extra.clone(),
            rows: rows.into_iter().map(Into::into).collect(),
        }
    }
}

/// `text` on one line: an Everything bookmark's search may have several,
/// which goz-gui's one-line boxes cannot show.
pub fn one_line(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything 1.5.0.1424b's own file, as it wrote it.
    pub const EVERYTHING: &str = "Name,Type,Folder,Case,Whole Word,Path,Diacritic,Prefix,Suffix,\
        Ignore Punctuation,Ignore Whitespace,Regex,Search,Filter,Columns,Sort,Descending,View,\
        Index,File List,Host,Link Type,Macro,Key,Icon\r\n\
        \"System32 executables\",0,\"\",0,0,0,0,0,0,0,0,0,\"c:\\windows\\system32\\*.exe\",,\
        \"\",\"\",0,,,,,0,,,\"\"\r\n";

    fn names(list: &Bookmarks) -> Vec<&str> {
        list.rows().iter().map(|r| r.name()).collect()
    }

    #[test]
    fn reads_everythings_file_and_writes_it_back_unchanged() {
        let list = Bookmarks::parse(EVERYTHING.as_bytes()).expect("parses");
        assert_eq!(names(&list), ["System32 executables"]);
        let b = &list.rows()[0];
        assert_eq!(b.kind(), Kind::Bookmark);
        assert_eq!(b.search(), r"c:\windows\system32\*.exe");
        assert!(b.search_set());
        assert!(b.folder().is_empty());
        // Everything's "Disabled" match options; no filter or sort set.
        assert_eq!(
            b.settings(),
            Settings {
                match_case: Some(false),
                match_path: Some(false),
                filter: None,
                sort: None,
            }
        );
        assert_eq!(list.to_csv(), EVERYTHING);
        // The same bookmark made here is the same row.
        let made = Bookmark::new(
            &[],
            "System32 executables",
            r"c:\windows\system32\*.exe",
            &Settings {
                match_case: Some(false),
                match_path: Some(false),
                ..Settings::default()
            },
        );
        assert_eq!(made, *list.rows()[0]);
    }

    #[test]
    fn a_new_bookmark_is_written_as_everything_writes_one() {
        let mut list = Bookmarks::default();
        list.push(Bookmark::new(
            &["Music".to_owned(), "Big".to_owned()],
            "Big \"audio\", sorted",
            "music\\ size:>1mb",
            &Settings {
                match_case: Some(true),
                match_path: None,
                filter: Some(Filter::Audio),
                sort: Some(SortSpec {
                    key: SortKey::Size,
                    dir: SortDir::Desc,
                }),
            },
        ));
        let text = list.to_csv();
        let row = text.lines().nth(1).expect("a row");
        assert_eq!(
            row,
            "\"Big \"\"audio\"\", sorted\",0,\"Music/Big\",1,0,,0,0,0,0,0,0,\"music\\ size:>1mb\",\
             \"Audio\",\"\",\"Size\",1,,,,,0,,,\"\""
        );
        let back = Bookmarks::parse(text.as_bytes()).expect("parses");
        assert_eq!(back, list);
        let s = back.rows()[0].settings();
        assert_eq!(s.match_case, Some(true));
        assert_eq!(s.match_path, None, "(No change)");
        assert_eq!(s.filter, Some(Filter::Audio));
        assert_eq!(
            s.sort.map(|s| (s.key, s.dir)),
            Some((SortKey::Size, SortDir::Desc))
        );
        assert_eq!(back.rows()[0].folder(), ["Music", "Big"]);
    }

    #[test]
    fn folders_and_separators_are_written_as_everything_writes_them() {
        let music = vec!["Music".to_owned()];
        let mut list = Bookmarks::default();
        list.push(Bookmark::new_folder(&[], "Music"));
        list.push(Bookmark::new_separator(&music));
        let text = list.to_csv();
        let mut lines = text.lines().skip(1);
        assert_eq!(
            lines.next(),
            Some("\"Music\",1,\"\",,,,,,,,,,,,\"\",\"\",0,,,,,0,,,\"\"")
        );
        assert_eq!(lines.next(), Some("\"\",2,\"Music\",,,,,,,,,,,,,,,,,,,,,,"));
        let back = Bookmarks::parse(text.as_bytes()).expect("parses");
        assert_eq!(back.rows()[0].kind(), Kind::Folder);
        assert_eq!(back.rows()[0].place(), ["Music"]);
        assert!(!back.rows()[0].search_set());
        assert_eq!(back.rows()[1].kind(), Kind::Separator);
        assert_eq!(back.rows()[1].folder(), ["Music"]);
        assert_eq!(back.to_csv(), text);
    }

    #[test]
    fn names_and_searches_survive_commas_quotes_and_line_breaks() {
        let mut list = Bookmarks::default();
        let odd = [
            ("a, b", "x,y"),
            ("say \"hi\"", "\"quoted phrase\" ext:txt"),
            ("two\r\nlines", "first\nsecond"),
            ("", ""),
        ];
        for (name, search) in odd {
            list.push(Bookmark::new(&[], name, search, &Settings::default()));
        }
        let back = Bookmarks::parse(list.to_csv().as_bytes()).expect("parses");
        assert_eq!(back.len(), odd.len());
        for (i, (name, search)) in odd.iter().enumerate() {
            let b = &back.rows()[i];
            assert_eq!((b.name(), b.search()), (*name, *search));
            assert!(b.search_set(), "an empty search in quotes is set");
        }
        assert_eq!(back.to_csv(), list.to_csv());
    }

    #[test]
    fn unknown_and_missing_columns() {
        // Columns out of order, one goz-gui does not know, Everything's
        // newer ones missing, and a row shorter than the header.
        let text = "Search,Name,Colour,Case,Sort\r\n\
                    \"*.mp3\",\"Music\",\"blue\",1,\"date modified\"\r\n\
                    \"*.txt\",\"Notes\"\r\n";
        let list = Bookmarks::parse(text.as_bytes()).expect("parses");
        assert_eq!(names(&list), ["Music", "Notes"]);
        let music = list.rows()[0].settings();
        assert_eq!(music.match_case, Some(true));
        // No Descending column: ascending, as Everything reads it.
        assert_eq!(
            music.sort,
            Some(SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Asc
            })
        );
        let out = list.to_csv();
        let mut lines = out.lines();
        assert_eq!(
            lines.next(),
            Some(format!("{},Colour", COLUMNS.join(",")).as_str())
        );
        // The unknown column keeps its value; missing ones get defaults.
        assert_eq!(
            lines.next(),
            Some(
                "\"Music\",0,\"\",1,0,0,0,0,0,0,0,0,\"*.mp3\",,\"\",\"date modified\",0,,,,,0,,,\
                 \"\",\"blue\""
            )
        );
        assert_eq!(
            lines.next(),
            Some("\"Notes\",0,\"\",0,0,0,0,0,0,0,0,0,\"*.txt\",,\"\",\"\",0,,,,,0,,,\"\",")
        );
        // A row of this list put into another keeps its unknown column.
        let mut other = Bookmarks::default();
        other.push(Arc::clone(&list.rows()[0]));
        other.push(Bookmark::new(&[], "New", "new", &Settings::default()));
        let text = other.to_csv();
        assert!(text.starts_with(&format!("{},Colour\r\n", COLUMNS.join(","))));
        assert!(text.contains("\"blue\"\r\n"));
        assert!(text.ends_with(",\r\n"), "the new row's Colour is empty");
        // No text at all, or a header only: no bookmarks.
        assert!(Bookmarks::parse(b"").expect("empty").is_empty());
        assert!(
            Bookmarks::parse(COLUMNS.join(",").as_bytes())
                .expect("header")
                .is_empty()
        );
    }

    #[test]
    fn what_is_not_a_bookmarks_file() {
        assert_eq!(
            Bookmarks::parse(b"Name,Type\r\n\0\0").err().as_deref(),
            Some("it holds NUL characters")
        );
        assert_eq!(
            Bookmarks::parse(b"Name\r\n\"caf\xe9\"\r\n")
                .err()
                .as_deref(),
            Some("it is not UTF-8 text")
        );
        assert_eq!(
            Bookmarks::parse(b"Search,Search Count\r\nx,1\r\n")
                .err()
                .as_deref(),
            Some("it has no Name column")
        );
        assert!(Bookmarks::parse_import(b"Search\r\nx\r\n").is_err());
        // Cut short inside a quote: not read as bookmarks (the file is
        // moved aside), but an import takes what it can.
        let cut = "Name,Search\r\n\"a\",\"x\"\r\n\"b\",\"y";
        assert!(
            Bookmarks::parse(cut.as_bytes())
                .err()
                .is_some_and(|why| why.contains("never closed"))
        );
        assert_eq!(
            Bookmarks::parse_import(cut.as_bytes())
                .expect("lenient")
                .len(),
            2
        );
    }

    #[test]
    fn options_as_everything_reads_them() {
        // Empty is "(No change)"; any number but 0 is on, as atoi reads it.
        let text = "Name,Case,Path,Filter,Sort,Descending,Search\r\n\
                    a,,,\"Pictures\",\"Extension\",1,\r\n\
                    b,2,0,\" video \",\"PATH\",0,\"\"\r\n\
                    c,x,1,,\"size\",,x\r\n";
        let list = Bookmarks::parse(text.as_bytes()).expect("parses");
        let a = &list.rows()[0];
        assert_eq!(a.settings(), Settings::default());
        assert!(!a.search_set(), "a bare empty search is (No change)");
        let b = &list.rows()[1];
        assert_eq!(
            b.settings(),
            Settings {
                match_case: Some(true),
                match_path: Some(false),
                filter: Some(Filter::Video),
                sort: Some(SortSpec {
                    key: SortKey::Path,
                    dir: SortDir::Asc
                }),
            }
        );
        assert!(b.search_set(), "a quoted empty search is set");
        let c = &list.rows()[2];
        assert_eq!(c.settings().match_case, Some(false));
        assert_eq!(c.settings().sort.map(|s| s.dir), Some(SortDir::Asc));
        assert!(c.search_set());
        assert_eq!(atoi(" 12ab"), 12);
        assert_eq!(atoi("-3"), -3);
        assert_eq!(atoi("x"), 0);
    }

    #[test]
    fn the_editor_changes_only_what_it_changed() {
        let text = format!(
            "{}\r\n\"Pics\",0,\"\",,1,1,0,0,0,0,0,0,\"*.jpg\",\"Pictures\",\"\",\"Extension\",1,\
             3,,,,0,,,\"pic.ico\"\r\n",
            COLUMNS.join(",")
        );
        let list = Bookmarks::parse(text.as_bytes()).expect("parses");
        let row = &list.rows()[0];
        let f = row.fields();
        assert_eq!(
            f,
            Fields {
                name: "Pics".into(),
                search: Some("*.jpg".into()),
                match_case: None,
                match_path: Some(true),
                filter: "Pictures".into(),
                sort: "Extension".into(),
                descending: true,
            }
        );
        // Nothing changed: the row is byte for byte the same.
        let mut same = (**row).clone();
        same.set_fields(&f);
        assert_eq!(&same, &**row);
        // The name and Match Case changed, the search set to (No change):
        // the unknown filter, the sort, View and Icon stay.
        let mut changed = (**row).clone();
        changed.set_fields(&Fields {
            name: "Photos".into(),
            search: None,
            match_case: Some(false),
            ..f.clone()
        });
        let out = list.with_rows(vec![changed]).to_csv();
        assert_eq!(
            out.lines().nth(1),
            Some(
                "\"Photos\",0,\"\",0,1,1,0,0,0,0,0,0,,\"Pictures\",\"\",\"Extension\",1,3,,,,0,,,\
                 \"pic.ico\""
            )
        );
        // A goz-gui filter and sort.
        let mut known = (**row).clone();
        known.set_fields(&Fields {
            filter: "Audio".into(),
            sort: "Size".into(),
            descending: false,
            ..f
        });
        let s = known.settings();
        assert_eq!(s.filter, Some(Filter::Audio));
        assert_eq!(
            s.sort,
            Some(SortSpec {
                key: SortKey::Size,
                dir: SortDir::Asc
            })
        );
    }

    #[test]
    fn places_and_names() {
        assert_eq!(split("Music/Rock"), ["Music", "Rock"]);
        assert_eq!(split("/Music//"), ["Music"]);
        assert!(split("").is_empty());
        assert_eq!(join(&split("a/b")), "a/b");
        let p = split("Music/Rock");
        assert!(same_path(&p, &split("music/ROCK")));
        assert!(within(&p, &split("MUSIC")));
        assert!(within(&p, &[]));
        assert!(!within(&split("Music"), &p));
        assert!(!within(&split("Musical"), &split("Music")));
        let mut f = Bookmark::new_folder(&split("Music"), "Rock");
        assert_eq!(f.place(), ["Music", "Rock"]);
        f.set_name("a/b");
        assert_eq!(f.place(), ["Music", "a", "b"]);
    }

    #[test]
    fn remembered_settings() {
        let sort = SortSpec::default_for(SortKey::Size);
        assert_eq!(
            Remember::default().settings(true, false, Filter::Audio, sort),
            Settings {
                match_case: Some(true),
                match_path: Some(false),
                filter: None,
                sort: None,
            }
        );
        let all = Remember {
            case: false,
            path: false,
            filter: true,
            sort: true,
            search: false,
        };
        assert_eq!(
            all.settings(true, true, Filter::Audio, sort),
            Settings {
                match_case: None,
                match_path: None,
                filter: Some(Filter::Audio),
                sort: Some(sort),
            }
        );
    }

    #[test]
    fn lines() {
        assert_eq!(one_line("a\r\nb\nc\rd"), "a b c d");
    }
}
