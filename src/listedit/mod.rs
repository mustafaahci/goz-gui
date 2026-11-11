//! Tools > File List Editor... (wave 5): Everything's File List Editor, a
//! window of its own for making and changing Everything File Lists
//! (`.efu`). File > New, Open..., Save and Save As...; Edit > Add Files...,
//! Add Folder... (the folder and what it holds), Add Folder and
//! Subfolders... (everything under it), Remove, Remove Duplicates and Select
//! All; files and folders dropped on it are added too (a folder with its
//! subfolders). The list shows each item's Name, Path, Size, Date Modified
//! and Date Created, sorted by a click on a column; W7 adds Everything's
//! Extension, Type and Attributes columns, and Edit > Paste.
//!
//! This module is the list itself and its file, with no window: the rows
//! ([`Entry`]), their sorts, and the EFU they are written as and read
//! from. `scan` walks folders (and reads a list) on a thread of its own;
//! `ui` is the window, which runs on a thread of its own too, so the main
//! window never waits for it, nor it for the main window.

pub mod scan;
pub mod ui;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

/// The commands, 4750..=4799 (wave 5's range, past the data tools'
/// 4600..=4699 and clear of the shell's).
pub mod cmd {
    pub const FIRST: u16 = 4750;
    /// The main window's Tools > File List Editor....
    pub const FILE_LIST_EDITOR: u16 = FIRST;
    // The editor's own menus.
    pub const NEW: u16 = FIRST + 1;
    pub const OPEN: u16 = FIRST + 2;
    pub const SAVE: u16 = FIRST + 3;
    pub const SAVE_AS: u16 = FIRST + 4;
    pub const CLOSE: u16 = FIRST + 5;
    pub const ADD_FILES: u16 = FIRST + 6;
    pub const ADD_FOLDER: u16 = FIRST + 7;
    pub const ADD_FOLDER_SUBFOLDERS: u16 = FIRST + 8;
    pub const REMOVE: u16 = FIRST + 9;
    pub const REMOVE_DUPLICATES: u16 = FIRST + 10;
    pub const SELECT_ALL: u16 = FIRST + 11;
    pub const STOP: u16 = FIRST + 12;
    /// One past the last.
    pub const END: u16 = FIRST + 50;

    // W6, in wave 6's range (4850..=4899, after the slots' 4850..=4859):
    // File > Import... and Export...; Edit > Update and Read Extended
    // Information.
    pub const IMPORT: u16 = 4860;
    pub const EXPORT: u16 = 4861;
    pub const UPDATE: u16 = 4862;
    pub const READ_EXTENDED: u16 = 4863;

    const _: () = {
        assert!(crate::datatools::cmd::END <= FIRST);
        assert!(STOP < END);
        assert!((END as usize) <= crate::menu::SHELL_FIRST as usize);
        assert!(crate::filelist::slots::cmd::SET_SLOT_END <= IMPORT);
        assert!(READ_EXTENDED < crate::filelist::slots::cmd::END);
    };
}

/// The editor window's WM_APP messages: `WM_APP + 340..350`.
pub const MSG_FIRST: u32 = WM_APP + 340;
/// A scan or a read has rows for the list, or ended.
pub const WM_FOUND: u32 = MSG_FIRST;
/// A save ended.
pub const WM_SAVED: u32 = MSG_FIRST + 1;
const MSG_END: u32 = MSG_FIRST + 10;

/// W6 (`WM_APP + 360..370`): the main window's, as the editor closes
/// (its place is handed back, and a goz-gui started only for the editor
/// ends with it).
pub const WM_EDITOR_CLOSED: u32 = WM_APP + 360;
/// The editor's: open the list `-edit` handed over.
pub const WM_EDITOR_OPEN: u32 = WM_APP + 361;
/// The editor's: View > Theme changed (wParam 1 dark, lParam 1 following
/// the system).
pub const WM_EDITOR_LOOK: u32 = WM_APP + 362;
/// One past wave 6's messages.
pub const W6_MSG_END: u32 = WM_APP + 370;

// Past the main window's ranges (G2's end at WM_APP + 320); the editor's
// window has no timers.
const _: () = assert!(MSG_FIRST >= WM_APP + 320 && WM_SAVED < MSG_END);
const _: () = assert!(MSG_END <= WM_EDITOR_CLOSED && WM_EDITOR_LOOK < W6_MSG_END);

const BACKSLASH: char = '\\';
/// `FILE_ATTRIBUTE_DIRECTORY`.
const DIRECTORY: u32 = 0x10;

/// One item of a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// Its full path; a folder's without a trailing backslash, but a
    /// drive's root keeps its own (`C:\`).
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Date modified and date created, as FILETIMEs (UTC).
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub attributes: Option<u32>,
}

impl Entry {
    /// Where its name starts in its path.
    fn name_start(&self) -> usize {
        match self.path.rfind(BACKSLASH) {
            // A drive's root is all name.
            Some(i) if i + 1 < self.path.len() => i + 1,
            _ => 0,
        }
    }

    /// Its name (a drive's root: the whole path).
    pub fn name(&self) -> &str {
        &self.path[self.name_start()..]
    }

    /// The folder it is in, as the Path column shows it.
    pub fn folder(&self) -> &str {
        let start = self.name_start();
        if start == 0 {
            return "";
        }
        let folder = &self.path[..start - 1];
        // A drive's root keeps its backslash: `C:\`.
        if folder.ends_with(':') {
            &self.path[..start]
        } else {
            folder
        }
    }
}

/// The list's columns, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Path,
    Size,
    /// W7: Everything's Extension, Type and Attributes.
    Extension,
    Type,
    Modified,
    Created,
    Attributes,
}

/// The columns with their titles and widths at 96 DPI: Everything's
/// (Filename, Size, Extension, Type, Date Modified, Date Created,
/// Attributes), the Filename as goz-gui's Name and Path.
pub const COLUMNS: [(Column, &str, i32); 8] = [
    (Column::Name, "Name", 220),
    (Column::Path, "Path", 300),
    (Column::Size, "Size", 90),
    (Column::Extension, "Extension", 72),
    (Column::Type, "Type", 120),
    (Column::Modified, "Date Modified", 140),
    (Column::Created, "Date Created", 140),
    (Column::Attributes, "Attributes", 76),
];

impl Entry {
    /// W7: the Extension column's text: after the name's last dot (a
    /// folder has none).
    pub fn extension(&self) -> &str {
        if self.is_dir {
            return "";
        }
        let name = self.name();
        name.rfind('.').map_or("", |dot| &name[dot + 1..])
    }

    /// Its attributes, a folder's with FILE_ATTRIBUTE_DIRECTORY (as EFU
    /// writes them); `None` when not known.
    fn attribute_bits(&self) -> Option<u32> {
        match (self.attributes, self.is_dir) {
            (Some(a), dir) => Some(if dir { a | DIRECTORY } else { a }),
            (None, true) => Some(DIRECTORY),
            (None, false) => None,
        }
    }

    /// W7: the Attributes column's text, Everything's letters.
    pub fn attributes_text(&self) -> String {
        self.attribute_bits()
            .map(attribute_letters)
            .unwrap_or_default()
    }
}

/// Everything's attribute letters, in the order of their bits: R read-only,
/// H hidden, S system, D folder, A archive, V device, N normal, T
/// temporary, P sparse, L reparse point, C compressed, O offline, I not
/// content indexed, E encrypted, X no scrub, U unpinned, M recall on data
/// access.
pub fn attribute_letters(bits: u32) -> String {
    const LETTERS: [(u32, char); 17] = [
        (0x1, 'R'),
        (0x2, 'H'),
        (0x4, 'S'),
        (0x10, 'D'),
        (0x20, 'A'),
        (0x40, 'V'),
        (0x80, 'N'),
        (0x100, 'T'),
        (0x200, 'P'),
        (0x400, 'L'),
        (0x800, 'C'),
        (0x1000, 'O'),
        (0x2000, 'I'),
        (0x4000, 'E'),
        (0x20000, 'X'),
        (0x100000, 'U'),
        (0x400000, 'M'),
    ];
    LETTERS
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|&(_, c)| c)
        .collect()
}

/// W7: the Type column's text for `e`: the shell's name for its kind of
/// file ("Text Document", "File folder"), asked once per extension on this
/// thread (the editor's) and kept (`filetype`).
pub fn type_name(e: &Entry) -> std::rc::Rc<[u16]> {
    let key: crate::filetype::Key = (!e.is_dir).then(|| {
        e.extension()
            .encode_utf16()
            .map(crate::highlight::fold)
            .collect()
    });
    if let Some(known) = crate::filetype::known(&key) {
        return known;
    }
    let (_, found) = crate::filetype::look_up(key.as_deref(), false);
    crate::filetype::learn(key.clone(), found);
    crate::filetype::known(&key).unwrap_or_else(|| std::rc::Rc::from(Vec::new()))
}

impl Column {
    pub fn at(index: usize) -> Option<Column> {
        COLUMNS.get(index).map(|c| c.0)
    }

    /// A first click's order: names, paths, extensions, types and
    /// attributes A to Z, sizes and dates the largest and newest first (as
    /// the results' Options > Results says by default).
    pub fn first_ascending(self) -> bool {
        matches!(
            self,
            Column::Name | Column::Path | Column::Extension | Column::Type | Column::Attributes
        )
    }
}

/// Text compared as Windows compares names: any case.
fn folded(text: &str) -> String {
    text.to_lowercase()
}

/// Sorts `entries` by `column`; an unknown size or date goes last either
/// way, and ties go by the path.
pub fn sort(entries: &mut [Entry], column: Column, ascending: bool) {
    let dir = |o: Ordering| if ascending { o } else { o.reverse() };
    // Known values first, in the direction asked; the unknown after them.
    let by_value = |a: Option<i128>, b: Option<i128>| match (a, b) {
        (Some(a), Some(b)) => dir(a.cmp(&b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    match column {
        Column::Name => {
            entries.sort_by_cached_key(|e| (folded(e.name()), folded(&e.path)));
            if !ascending {
                entries.reverse();
            }
        }
        Column::Path => {
            entries.sort_by_cached_key(|e| (folded(e.folder()), folded(e.name())));
            if !ascending {
                entries.reverse();
            }
        }
        // W7: then by the path, as the others.
        Column::Extension => {
            entries.sort_by_cached_key(|e| (folded(e.extension()), folded(&e.path)));
            if !ascending {
                entries.reverse();
            }
        }
        Column::Type => {
            // One shell look-up per kind, however many rows.
            let mut types: HashMap<(bool, String), String> = HashMap::new();
            entries.sort_by_cached_key(|e| {
                let kind = (e.is_dir, folded(e.extension()));
                let name = types
                    .entry(kind)
                    .or_insert_with(|| folded(&String::from_utf16_lossy(&type_name(e))))
                    .clone();
                (name, folded(&e.path))
            });
            if !ascending {
                entries.reverse();
            }
        }
        Column::Attributes => {
            entries.sort_by_cached_key(|e| (e.attributes_text(), folded(&e.path)));
            if !ascending {
                entries.reverse();
            }
        }
        Column::Size | Column::Modified | Column::Created => {
            let value = |e: &Entry| -> Option<i128> {
                match column {
                    Column::Size => e.size.filter(|_| !e.is_dir).map(i128::from),
                    Column::Modified => e.modified.map(i128::from),
                    _ => e.created.map(i128::from),
                }
            };
            entries.sort_by_cached_key(|e| folded(&e.path));
            entries.sort_by(|a, b| by_value(value(a), value(b)));
        }
    }
}

/// Takes out the rows `rows` (indices into `entries`, any order); how many
/// went.
pub fn remove_rows(entries: &mut Vec<Entry>, rows: &[usize]) -> usize {
    let mut gone = vec![false; entries.len()];
    for &r in rows {
        if let Some(g) = gone.get_mut(r) {
            *g = true;
        }
    }
    let before = entries.len();
    let mut i = 0;
    entries.retain(|_| {
        let keep = !gone[i];
        i += 1;
        keep
    });
    before - entries.len()
}

/// Takes out every row whose path an earlier row has (any case); how many
/// went.
pub fn remove_duplicates(entries: &mut Vec<Entry>) -> usize {
    let mut seen = std::collections::HashSet::new();
    let before = entries.len();
    entries.retain(|e| seen.insert(folded(&e.path)));
    before - entries.len()
}

/// The EFU header Everything writes.
pub const EFU_HEADER: &str = "Filename,Size,Date Modified,Date Created,Attributes\r\n";

/// Appends `e` as one EFU row: its path quoted (a folder's with its
/// trailing backslash), its size (none for a folder), its dates as
/// FILETIMEs, its attributes in decimal; a value not known is left empty.
pub fn push_row(e: &Entry, out: &mut String) {
    let mut path = e.path.clone();
    if e.is_dir && !path.ends_with(BACKSLASH) {
        path.push(BACKSLASH);
    }
    out.push('"');
    out.push_str(&path.replace('"', "\"\""));
    out.push('"');
    for v in values(e) {
        out.push(',');
        out.push_str(&v);
    }
    out.push_str("\r\n");
}

/// A row's values after its path, as EFU writes them: its size (none for
/// a folder), its dates as FILETIMEs, its attributes in decimal; a value
/// not known is empty.
fn values(e: &Entry) -> [String; 4] {
    let size = match (e.is_dir, e.size) {
        (false, Some(size)) => size.to_string(),
        _ => String::new(),
    };
    let date = |d: Option<i64>| {
        d.filter(|&t| t > 0)
            .map_or_else(String::new, |t| t.to_string())
    };
    let attributes = e
        .attributes
        .or_else(|| e.is_dir.then_some(DIRECTORY))
        .map(|a| if e.is_dir { a | DIRECTORY } else { a })
        .map_or_else(String::new, |a| a.to_string());
    [size, date(e.modified), date(e.created), attributes]
}

/// The whole list as an Everything File List: UTF-8, Everything's header,
/// a row an item.
pub fn to_efu(entries: &[Entry]) -> Vec<u8> {
    let mut out = String::with_capacity(EFU_HEADER.len() + entries.len() * 96);
    out.push_str(EFU_HEADER);
    for e in entries {
        push_row(e, &mut out);
    }
    out.into_bytes()
}

/// W6: File > Export...'s file types, in Everything's order (its
/// `editor_last_export_type` counts them from 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    Csv,
    Efu,
    M3u,
    M3u8,
    Tsv,
    Txt,
}

/// How many Export has.
pub const EXPORT_KINDS: usize = 6;

/// Export...'s file types as its Save dialog lists them (then All Files).
pub const EXPORT_DIALOG: &[crate::filedlg::Kind] = &[
    ("CSV Comma Separated Values (*.csv)", "*.csv"),
    ("EFU Everything File List (*.efu)", "*.efu"),
    ("M3U Winamp Playlist (*.m3u)", "*.m3u"),
    ("M3U8 Winamp Playlist (UTF-8) (*.m3u8)", "*.m3u8"),
    ("TXT Tab Separated Values (*.txt)", "*.txt"),
    ("TXT Text Files (*.txt)", "*.txt"),
    ("All Files (*.*)", "*.*"),
];

/// File > Import...'s file types, as Everything's: what goz-gui's lists
/// read (an EFU, a CSV with a `Filename` column, a text file of paths).
pub const IMPORT_DIALOG: &[crate::filedlg::Kind] = &[
    (
        "All Supported Files (*.csv;*.efu;*.txt)",
        "*.csv;*.efu;*.txt",
    ),
    ("CSV Comma Separated Values (*.csv)", "*.csv"),
    ("EFU Everything File List (*.efu)", "*.efu"),
    ("TXT Text Files (*.txt)", "*.txt"),
    ("All Files (*.*)", "*.*"),
];

impl Export {
    pub const ALL: [Export; EXPORT_KINDS] = [
        Export::Csv,
        Export::Efu,
        Export::M3u,
        Export::M3u8,
        Export::Tsv,
        Export::Txt,
    ];

    /// Its place in Export...'s list (`editor_last_export_type`); the
    /// first for one out of range.
    pub fn at(index: usize) -> Export {
        Export::ALL.get(index).copied().unwrap_or(Export::Csv)
    }

    pub fn index(self) -> usize {
        Export::ALL.iter().position(|&e| e == self).unwrap_or(0)
    }

    /// The type a file name's extension names, for a test's file
    /// (`GOZ_GUI_EDITOR_FILE`), where no dialog says it: `.tsv` is TSV, as
    /// its dialog type writes `.txt`.
    pub fn of_path(path: &std::path::Path) -> Option<Export> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "csv" => Export::Csv,
            "efu" => Export::Efu,
            "m3u" => Export::M3u,
            "m3u8" => Export::M3u8,
            "tsv" => Export::Tsv,
            "txt" => Export::Txt,
            _ => return None,
        })
    }

    /// `entries` written as this type: CSV and EFU as Save writes a list
    /// (Everything's columns, FILETIMEs), TSV the same columns between
    /// tabs, TXT the full paths, M3U and M3U8 `#EXTM3U` then the full
    /// paths (M3U in the system's ANSI code page, Winamp's; the others
    /// UTF-8).
    pub fn bytes(self, entries: &[Entry]) -> Vec<u8> {
        match self {
            Export::Csv | Export::Efu => to_efu(entries),
            Export::Tsv => {
                let mut out = String::with_capacity(64 + entries.len() * 96);
                out.push_str(&EFU_HEADER.replace(',', "\t"));
                for e in entries {
                    out.push_str(&e.path);
                    if e.is_dir && !e.path.ends_with(BACKSLASH) {
                        out.push(BACKSLASH);
                    }
                    for v in values(e) {
                        out.push('\t');
                        out.push_str(&v);
                    }
                    out.push_str("\r\n");
                }
                out.into_bytes()
            }
            Export::Txt | Export::M3u | Export::M3u8 => {
                let mut out = String::with_capacity(16 + entries.len() * 80);
                if self != Export::Txt {
                    out.push_str("#EXTM3U\r\n");
                }
                for e in entries {
                    out.push_str(&e.path);
                    out.push_str("\r\n");
                }
                if self == Export::M3u {
                    ansi(&out)
                } else {
                    out.into_bytes()
                }
            }
        }
    }
}

/// `text` in the system's ANSI code page; a character it has not got is
/// its default character (`?`).
fn ansi(text: &str) -> Vec<u8> {
    use windows_sys::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Vec::new();
    }
    // SAFETY: two calls on buffers of the lengths given: the first asks
    // how many bytes, the second writes them.
    unsafe {
        let n = WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            core::ptr::null_mut(),
        );
        let mut out = vec![0u8; n.max(0) as usize];
        let written = WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            out.as_mut_ptr(),
            n,
            core::ptr::null(),
            core::ptr::null_mut(),
        );
        out.truncate(written.max(0) as usize);
        out
    }
}

/// The folders above `path`, nearest first: `c:\a\b.txt` gives `c:\a` and
/// then `c:\` (a drive's root keeps its backslash, as [`Entry::path`]
/// does).
fn parents(path: &str) -> impl Iterator<Item = &str> {
    let mut p = path;
    std::iter::from_fn(move || {
        let i = p.rfind(BACKSLASH)?;
        let up = if p[..i].ends_with(':') {
            &p[..=i]
        } else {
            &p[..i]
        };
        if up.len() >= p.len() {
            return None;
        }
        p = up;
        Some(up)
    })
}

/// W6: Edit > Update's root folders: the folders of the list that no other
/// folder of it holds (Everything: "Rescan root folders in the file list"),
/// in the list's order, each with whether the list holds anything below
/// its own items (it was added with its subfolders, and is walked so
/// again).
///
/// Each path is folded once and each row looks up only the folders above
/// it, so the cost is the rows times their depth, however many roots the
/// list has (an Everything export of search results can have tens of
/// thousands).
pub fn roots(entries: &[Entry]) -> Vec<(String, bool)> {
    let keys: Vec<String> = entries.iter().map(|e| folded(&e.path)).collect();
    let folders: HashSet<&str> = entries
        .iter()
        .zip(&keys)
        .filter(|(e, _)| e.is_dir)
        .map(|(_, k)| k.as_str())
        .collect();
    // The roots, each once, in the list's order: (its index in `entries`,
    // whether anything lies two levels or more below it).
    let mut found: Vec<(usize, bool)> = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for (i, (e, key)) in entries.iter().zip(&keys).enumerate() {
        if e.is_dir
            && !at.contains_key(key.as_str())
            && !parents(key).any(|up| folders.contains(up))
        {
            at.insert(key, found.len());
            found.push((i, false));
        }
    }
    // No root is under another, so a row has one root above it at most:
    // the first one its walk up meets.
    for key in &keys {
        if let Some((level, root)) = parents(key)
            .enumerate()
            .find_map(|(level, up)| at.get(up).map(|&r| (level, r)))
        {
            // Level 0 is the root's own items.
            if level > 0 {
                found[root].1 = true;
            }
        }
    }
    found
        .into_iter()
        .map(|(i, deep)| (entries[i].path.clone(), deep))
        .collect()
}

/// Takes out each row that is one of `roots` or under one; how many went.
/// Each row folds its path once and looks up the folders above it.
pub fn remove_under(entries: &mut Vec<Entry>, roots: &[String]) -> usize {
    let roots: HashSet<String> = roots.iter().map(|r| folded(r)).collect();
    let before = entries.len();
    entries.retain(|e| {
        let p = folded(&e.path);
        !roots.contains(&p) && !parents(&p).any(|up| roots.contains(up))
    });
    before - entries.len()
}

/// W7: the file names in pasted text (Edit > Paste, and the Edit File List
/// Slot dialog's): a name a line, white space and enclosing quotes taken
/// off, empty lines left out.
pub fn pasted_names(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| {
            let t = l.trim();
            t.strip_prefix('"')
                .and_then(|t| t.strip_suffix('"'))
                .unwrap_or(t)
                .trim()
        })
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// How many items, as the status bar says it.
pub fn items_text(n: usize) -> String {
    if n == 1 {
        "1 item".to_owned()
    } else {
        format!("{} items", crate::format::grouped(n as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn e(path: &str, dir: bool, size: Option<u64>, modified: Option<i64>) -> Entry {
        Entry {
            path: path.into(),
            is_dir: dir,
            size,
            modified,
            created: modified.map(|t| t - 1),
            attributes: Some(if dir { 16 } else { 32 }),
        }
    }

    fn paths(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn names_and_folders() {
        let file = e(r"C:\a\b.txt", false, None, None);
        assert_eq!((file.name(), file.folder()), ("b.txt", r"C:\a"));
        let top = e(r"C:\b.txt", false, None, None);
        assert_eq!((top.name(), top.folder()), ("b.txt", r"C:\"));
        let root = e(r"C:\", true, None, None);
        assert_eq!((root.name(), root.folder()), (r"C:\", ""));
        let share = e(r"\\srv\share\x", true, None, None);
        assert_eq!((share.name(), share.folder()), ("x", r"\\srv\share"));
    }

    #[test]
    fn sorts_with_unknown_values_last() {
        let mut list = vec![
            e(r"C:\b\Zeta.txt", false, Some(5), Some(300)),
            e(r"C:\a\alpha.txt", false, None, Some(100)),
            e(r"C:\a", true, Some(99), None),
            e(r"C:\c\beta.txt", false, Some(7), Some(200)),
        ];
        sort(&mut list, Column::Name, true);
        assert_eq!(
            paths(&list),
            [
                r"C:\a",
                r"C:\a\alpha.txt",
                r"C:\c\beta.txt",
                r"C:\b\Zeta.txt"
            ]
        );
        sort(&mut list, Column::Path, true);
        assert_eq!(
            paths(&list),
            [
                r"C:\a",
                r"C:\a\alpha.txt",
                r"C:\b\Zeta.txt",
                r"C:\c\beta.txt"
            ]
        );
        // A folder's size is no file size: last, as an unknown one.
        sort(&mut list, Column::Size, false);
        assert_eq!(
            paths(&list),
            [
                r"C:\c\beta.txt",
                r"C:\b\Zeta.txt",
                r"C:\a",
                r"C:\a\alpha.txt"
            ]
        );
        sort(&mut list, Column::Size, true);
        assert_eq!(
            paths(&list),
            [
                r"C:\b\Zeta.txt",
                r"C:\c\beta.txt",
                r"C:\a",
                r"C:\a\alpha.txt"
            ]
        );
        sort(&mut list, Column::Modified, false);
        assert_eq!(
            paths(&list),
            [
                r"C:\b\Zeta.txt",
                r"C:\c\beta.txt",
                r"C:\a\alpha.txt",
                r"C:\a"
            ]
        );
        sort(&mut list, Column::Created, true);
        assert_eq!(
            paths(&list),
            [
                r"C:\a\alpha.txt",
                r"C:\c\beta.txt",
                r"C:\b\Zeta.txt",
                r"C:\a"
            ]
        );
        assert_eq!(Column::at(5), Some(Column::Modified));
        assert!(Column::Name.first_ascending() && !Column::Size.first_ascending());
    }

    /// W7: Everything's Extension, Type and Attributes columns.
    #[test]
    fn extension_type_and_attributes_columns() {
        let mut a = e(r"C:\x\b.TXT", false, Some(1), None);
        a.attributes = Some(0x21);
        let dir = e(r"C:\x\v1.2", true, None, None);
        let bare = Entry {
            path: r"C:\x\.gitignore".into(),
            ..Entry::default()
        };
        assert_eq!((a.extension(), dir.extension()), ("TXT", ""));
        assert_eq!(bare.extension(), "gitignore");
        assert_eq!(a.attributes_text(), "RA");
        assert_eq!(dir.attributes_text(), "D");
        assert_eq!(bare.attributes_text(), "", "not known");
        assert_eq!(attribute_letters(0x2 | 0x4 | 0x20 | 0x2000), "HSAI");
        assert_eq!(attribute_letters(0x10 | 0x400), "DL");
        let mut list = vec![
            a.clone(),
            e(r"C:\x\a.doc", false, Some(1), None),
            dir.clone(),
            bare.clone(),
        ];
        sort(&mut list, Column::Extension, true);
        assert_eq!(
            paths(&list),
            [
                r"C:\x\v1.2",
                r"C:\x\a.doc",
                r"C:\x\.gitignore",
                r"C:\x\b.TXT"
            ]
        );
        sort(&mut list, Column::Attributes, true);
        assert_eq!(
            paths(&list),
            [
                r"C:\x\.gitignore",
                r"C:\x\a.doc",
                r"C:\x\v1.2",
                r"C:\x\b.TXT"
            ]
        );
        // The shell's words differ by machine; the folder has one, and the
        // sort keeps each kind together.
        assert!(!type_name(&dir).is_empty());
        sort(&mut list, Column::Type, true);
        assert_eq!(list.len(), 4);
        let docs = list.iter().position(|e| e.path.ends_with("a.doc")).unwrap();
        assert!(list.iter().filter(|e| e.extension() == "doc").count() == 1 && docs < 4);
        assert!(Column::Type.first_ascending() && Column::Attributes.first_ascending());
        assert_eq!(
            pasted_names("C:\\a.txt\r\n  \"C:\\b c\\d.txt\" \r\n\r\nname\n"),
            [r"C:\a.txt", r"C:\b c\d.txt", "name"]
        );
    }

    #[test]
    fn rows_and_duplicates_are_removed() {
        let mut list = vec![
            e(r"C:\a.txt", false, None, None),
            e(r"C:\b.txt", false, None, None),
            e(r"c:\A.TXT", false, None, None),
            e(r"C:\c.txt", false, None, None),
        ];
        assert_eq!(remove_duplicates(&mut list), 1);
        assert_eq!(paths(&list), [r"C:\a.txt", r"C:\b.txt", r"C:\c.txt"]);
        assert_eq!(remove_rows(&mut list, &[2, 0, 9]), 2);
        assert_eq!(paths(&list), [r"C:\b.txt"]);
        assert_eq!(items_text(1), "1 item");
        assert_eq!(items_text(12_345), "12,345 items");
    }

    /// W6: Export's types.
    #[test]
    fn exports_in_everythings_types() {
        let list = vec![
            e(r"C:\x", true, None, Some(5)),
            e(r"C:\x\a, b.txt", false, Some(12), Some(7)),
        ];
        let text = |t: Export| String::from_utf8(t.bytes(&list)).unwrap();
        assert_eq!(text(Export::Csv), text(Export::Efu));
        assert_eq!(
            text(Export::Tsv),
            "Filename\tSize\tDate Modified\tDate Created\tAttributes\r\n\
             C:\\x\\\t\t5\t4\t16\r\n\
             C:\\x\\a, b.txt\t12\t7\t6\t32\r\n"
        );
        assert_eq!(text(Export::Txt), "C:\\x\r\nC:\\x\\a, b.txt\r\n");
        assert_eq!(
            text(Export::M3u8),
            "#EXTM3U\r\nC:\\x\r\nC:\\x\\a, b.txt\r\n"
        );
        // M3U in the ANSI code page: plain ASCII is the same bytes.
        assert_eq!(text(Export::M3u), text(Export::M3u8));
        assert_eq!(Export::at(4), Export::Tsv);
        assert_eq!(Export::at(99), Export::Csv);
        assert_eq!(Export::M3u8.index(), 3);
        assert_eq!(EXPORT_DIALOG.len(), EXPORT_KINDS + 1);
        assert_eq!(
            Export::of_path(std::path::Path::new(r"C:\a.TSV")),
            Some(Export::Tsv)
        );
        assert_eq!(Export::of_path(std::path::Path::new("a.bin")), None);
    }

    /// W6: Update's root folders, and what it takes out to walk again.
    #[test]
    fn update_finds_the_root_folders() {
        let mut list = vec![
            e(r"C:\loose.txt", false, Some(1), None),
            e(r"C:\a", true, None, None),
            e(r"C:\a\f.txt", false, Some(1), None),
            e(r"C:\a\sub", true, None, None),
            e(r"C:\a\sub\deep.txt", false, Some(1), None),
            e(r"D:\flat", true, None, None),
            e(r"D:\flat\g.txt", false, Some(1), None),
            e(r"D:\flat\inner", true, None, None),
            // Under C:\a though C:\a\x is not listed: no root of its own.
            e(r"c:\A\x\y", true, None, None),
            e(r"E:\", true, None, None),
            e(r"E:\z.txt", false, Some(1), None),
        ];
        assert_eq!(
            roots(&list),
            [
                (r"C:\a".to_owned(), true),
                (r"D:\flat".to_owned(), false),
                (r"E:\".to_owned(), false)
            ]
        );
        let gone = remove_under(&mut list, &[r"C:\a".into(), r"E:\".into()]);
        assert_eq!(gone, 7);
        assert_eq!(
            paths(&list),
            [
                r"C:\loose.txt",
                r"D:\flat",
                r"D:\flat\g.txt",
                r"D:\flat\inner"
            ]
        );
        assert!(roots(&[e(r"C:\f.txt", false, None, None)]).is_empty());
    }

    /// W6R-1: Update on a list with many unrelated root folders (an
    /// Everything export of search results: 20,000 folders whose parents
    /// are not listed, 200,000 other rows) costs the rows times their
    /// depth. Before, each root looked at every row: billions of folds, and
    /// the editor stopped responding for minutes.
    #[test]
    fn update_with_many_roots_takes_each_row_once() {
        const ROOTS: usize = 20_000;
        const PER_ROOT: usize = 10;
        let mut list = Vec::with_capacity(ROOTS * (PER_ROOT + 1));
        for r in 0..ROOTS {
            list.push(e(&format!(r"C:\Data\r{r}"), true, None, None));
            for f in 0..PER_ROOT {
                // Every other root holds a file in a subfolder (in another
                // case): it is walked with its subfolders.
                list.push(if r % 2 == 0 && f == 0 {
                    e(&format!(r"c:\DATA\R{r}\sub\f.txt"), false, Some(1), None)
                } else {
                    e(&format!(r"C:\Data\r{r}\f{f}.txt"), false, Some(1), None)
                });
            }
        }
        let started = std::time::Instant::now();
        let found = roots(&list);
        assert_eq!(found.len(), ROOTS);
        for (r, (path, deep)) in found.iter().enumerate() {
            assert_eq!(
                (path.as_str(), *deep),
                (&*format!(r"C:\Data\r{r}"), r % 2 == 0)
            );
        }
        let paths: Vec<String> = found.into_iter().map(|(p, _)| p).collect();
        assert_eq!(remove_under(&mut list, &paths), ROOTS * (PER_ROOT + 1));
        assert!(list.is_empty());
        // Milliseconds now (a debug build: well under a second); the old
        // roots x rows would take minutes.
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(10), "{took:?}");
    }

    #[test]
    fn efu_is_everythings_and_reads_back() {
        let list = vec![
            e(r"C:\x", true, None, Some(133_000_000_000_000_000)),
            e(
                r#"C:\x\a "q", b.txt"#,
                false,
                Some(12),
                Some(133_000_000_000_000_001),
            ),
            Entry {
                path: r"C:\x\bare.txt".into(),
                ..Entry::default()
            },
        ];
        let bytes = to_efu(&list);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with(EFU_HEADER));
        assert!(text.contains("\"C:\\x\\\",,133000000000000000,132999999999999999,16\r\n"));
        assert!(text.contains(
            "\"C:\\x\\a \"\"q\"\", b.txt\",12,133000000000000001,133000000000000000,32\r\n"
        ));
        assert!(text.ends_with("\"C:\\x\\bare.txt\",,,,\r\n"));
        // What it writes is what goz-gui's lists read.
        let mut back = Vec::new();
        let stop = AtomicBool::new(false);
        crate::filelist::parse::each_row(
            bytes.as_slice(),
            r"D:\lists",
            &|t| Some(t),
            &stop,
            |path, size, dates, attributes, dir| {
                back.push(Entry {
                    path: path.to_owned(),
                    is_dir: dir,
                    size,
                    modified: dates.modified,
                    created: dates.created,
                    attributes,
                })
            },
        )
        .unwrap();
        assert_eq!(back, list);
    }
}
