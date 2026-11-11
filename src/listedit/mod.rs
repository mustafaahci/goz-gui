//! Tools > File List Editor... (wave 5): Everything's File List Editor, a
//! window of its own for making and changing Everything File Lists
//! (`.efu`). File > New, Open..., Save and Save As...; Edit > Add Files...,
//! Add Folder... (the folder and what it holds), Add Folder and
//! Subfolders... (everything under it), Remove, Remove Duplicates and Select
//! All; files and folders dropped on it are added too (a folder with its
//! subfolders). The list shows each item's Name, Path, Size, Date Modified
//! and Date Created, sorted by a click on a column.
//!
//! This module is the list itself and its file, with no window: the rows
//! ([`Entry`]), their sorts, and the EFU they are written as and read
//! from. `scan` walks folders (and reads a list) on a thread of its own;
//! `ui` is the window, which runs on a thread of its own too, so the main
//! window never waits for it, nor it for the main window.

pub mod scan;
pub mod ui;

use std::cmp::Ordering;

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
    Modified,
    Created,
}

/// The columns with their titles and widths at 96 DPI.
pub const COLUMNS: [(Column, &str, i32); 5] = [
    (Column::Name, "Name", 220),
    (Column::Path, "Path", 300),
    (Column::Size, "Size", 90),
    (Column::Modified, "Date Modified", 140),
    (Column::Created, "Date Created", 140),
];

impl Column {
    pub fn at(index: usize) -> Option<Column> {
        COLUMNS.get(index).map(|c| c.0)
    }

    /// A first click's order: names and paths A to Z, sizes and dates the
    /// largest and newest first (as the results' Options > Results says
    /// by default).
    pub fn first_ascending(self) -> bool {
        matches!(self, Column::Name | Column::Path)
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

/// Whether `path` is under the folder `root` (both folded).
fn is_under(path: &str, root: &str) -> bool {
    path.len() > root.len()
        && path.starts_with(root)
        && (root.ends_with(BACKSLASH) || path[root.len()..].starts_with(BACKSLASH))
}

/// W6: Edit > Update's root folders: the folders of the list that no other
/// folder of it holds (Everything: "Rescan root folders in the file list"),
/// in the list's order, each with whether the list holds anything below
/// its own items (it was added with its subfolders, and is walked so
/// again).
pub fn roots(entries: &[Entry]) -> Vec<(String, bool)> {
    let folders: std::collections::HashSet<String> = entries
        .iter()
        .filter(|e| e.is_dir)
        .map(|e| folded(&e.path))
        .collect();
    let held = |path: &str| {
        // Each folder above it, nearest first.
        let mut p = path;
        while let Some(i) = p.rfind(BACKSLASH) {
            let up = if p[..i].ends_with(':') {
                &p[..=i]
            } else {
                &p[..i]
            };
            if up.len() >= p.len() {
                break;
            }
            if folders.contains(up) {
                return true;
            }
            p = up;
        }
        false
    };
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for e in entries.iter().filter(|e| e.is_dir) {
        let key = folded(&e.path);
        if held(&key) || !seen.insert(key.clone()) {
            continue;
        }
        let deep = entries.iter().any(|x| {
            let x = folded(&x.path);
            is_under(&x, &key)
                && x[key.len()..]
                    .trim_start_matches(BACKSLASH)
                    .contains(BACKSLASH)
        });
        out.push((e.path.clone(), deep));
    }
    out
}

/// Takes out each row that is one of `roots` or under one; how many went.
pub fn remove_under(entries: &mut Vec<Entry>, roots: &[String]) -> usize {
    let roots: Vec<String> = roots.iter().map(|r| folded(r)).collect();
    let before = entries.len();
    entries.retain(|e| {
        let p = folded(&e.path);
        !roots.iter().any(|r| p == *r || is_under(&p, r))
    });
    before - entries.len()
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
        assert_eq!(Column::at(3), Some(Column::Modified));
        assert!(Column::Name.first_ascending() && !Column::Size.first_ascending());
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
