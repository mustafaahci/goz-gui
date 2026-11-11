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

    const _: () = {
        assert!(crate::datatools::cmd::END <= FIRST);
        assert!(STOP < END);
        assert!((END as usize) <= crate::menu::SHELL_FIRST as usize);
    };
}

/// The editor window's WM_APP messages: `WM_APP + 340..350`.
pub const MSG_FIRST: u32 = WM_APP + 340;
/// A scan or a read has rows for the list, or ended.
pub const WM_FOUND: u32 = MSG_FIRST;
/// A save ended.
pub const WM_SAVED: u32 = MSG_FIRST + 1;
const MSG_END: u32 = MSG_FIRST + 10;

// Past the main window's ranges (G2's end at WM_APP + 320); the editor's
// window has no timers.
const _: () = assert!(MSG_FIRST >= WM_APP + 320 && WM_SAVED < MSG_END);

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
    out.push(',');
    if let (false, Some(size)) = (e.is_dir, e.size) {
        out.push_str(&size.to_string());
    }
    for date in [e.modified, e.created] {
        out.push(',');
        if let Some(t) = date.filter(|&t| t > 0) {
            out.push_str(&t.to_string());
        }
    }
    out.push(',');
    let attributes = e
        .attributes
        .or_else(|| e.is_dir.then_some(DIRECTORY))
        .map(|a| if e.is_dir { a | DIRECTORY } else { a });
    if let Some(a) = attributes {
        out.push_str(&a.to_string());
    }
    out.push_str("\r\n");
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
