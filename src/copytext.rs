//! The text of the copy commands: the values of a row exactly as the list
//! shows them, one row per line.

use crate::client::Row;
use crate::columns::Layout;
use crate::columns::{COL_ACCESSED, COL_ATTRIB, COL_CREATED, COL_RECENT};
use crate::window::{COL_EXT, COL_NAME, COL_PATH, COL_SIZE, COL_TYPE};
use crate::{filetype, format};

/// What to copy of each row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    /// The folder holding the row, as the Path column shows it.
    Path,
    FullPath,
    /// The full path in double quotes, as Copy Full Path writes it with
    /// Options > Results' "Double quote copy as path".
    QuotedFullPath,
    Size,
    Extension,
    /// The shell's name for the kind of file.
    Type,
    DateModified,
    /// A file list's date created (wave 5); wave 9: any row's, its
    /// source's or the disk's.
    DateCreated,
    /// Wave 9: the disk's (or, stream B2, gozd's) date accessed.
    DateAccessed,
    /// Wave 9: Everything's attribute letters.
    Attributes,
    /// Wave 9: gozd's date recently changed (blank without it).
    DateRecentlyChanged,
    /// The layout's shown columns, left to right, tab-separated.
    Tsv(Layout),
}

impl Field {
    /// The field a list column shows.
    pub fn of_column(col: usize) -> Field {
        match col {
            COL_NAME => Field::Name,
            COL_PATH => Field::Path,
            COL_SIZE => Field::Size,
            COL_EXT => Field::Extension,
            COL_TYPE => Field::Type,
            COL_CREATED => Field::DateCreated,
            COL_ACCESSED => Field::DateAccessed,
            COL_ATTRIB => Field::Attributes,
            COL_RECENT => Field::DateRecentlyChanged,
            _ => Field::DateModified,
        }
    }
}

/// Appends `row`'s `field` to `out`, as the list displays it.
pub fn push(row: &Row, field: Field, out: &mut Vec<u16>) {
    match field {
        Field::Name => out.extend_from_slice(row.name()),
        Field::Path => out.extend_from_slice(row.parent()),
        Field::FullPath => out.extend_from_slice(&row.path),
        Field::QuotedFullPath => {
            out.push(u16::from(b'"'));
            out.extend_from_slice(&row.path);
            out.push(u16::from(b'"'));
        }
        Field::Size => {
            // W9: a folder's too, when it has one (folder sizes).
            if let Some(size) = row.size {
                out.extend(format::size_cell(size).encode_utf16());
            }
        }
        Field::Extension => out.extend_from_slice(filetype::of_row(row)),
        Field::Type => out.extend_from_slice(&filetype::name(row)),
        Field::DateModified => {
            if let Some(ft) = row.mtime {
                format::date(ft, out);
            }
        }
        Field::DateCreated => {
            if let Some(ft) = row.created() {
                format::date(ft, out);
            }
        }
        Field::DateAccessed => {
            if let Some(ft) = row.accessed() {
                format::date(ft, out);
            }
        }
        Field::Attributes => {
            if let Some(a) = row.attributes() {
                crate::props::attribute_letters(a, out);
            }
        }
        Field::DateRecentlyChanged => {
            if let Some(ft) = row.recently_changed() {
                format::date(ft, out);
            }
        }
        Field::Tsv(layout) => {
            for (i, &col) in layout.shown().iter().enumerate() {
                if i > 0 {
                    out.push(u16::from(b'\t'));
                }
                push(row, Field::of_column(col), out);
            }
        }
    }
}

/// `field` of each of `rows`, one per line, CRLF-separated.
pub fn lines<'a>(rows: impl IntoIterator<Item = &'a Row>, field: Field) -> Vec<u16> {
    let mut out = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(&[13, 10]);
        }
        push(row, field, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::COL_DATE;
    use std::cell::Cell;

    fn row(path: &str, size: Option<u64>, is_dir: bool) -> Row {
        let path: Box<[u16]> = path.encode_utf16().collect();
        let name_start = path.iter().rposition(|&c| c == u16::from(b'\\')).unwrap() + 1;
        Row {
            path,
            name_start: name_start as u32,
            size,
            mtime: None,
            ctime: None,
            is_dir,
            icon: Cell::new(0),
            more: Default::default(),
            asked: Default::default(),
        }
    }

    fn text(v: &[u16]) -> String {
        String::from_utf16_lossy(v)
    }

    #[test]
    fn fields_are_the_columns_text() {
        let file = row(r"C:\Windows\notepad.exe", Some(360_448), false);
        let folder = row(r"C:\Windows\System32", Some(4096), true);
        let rows = [file, folder];
        assert_eq!(text(&lines(&rows, Field::Name)), "notepad.exe\r\nSystem32");
        assert_eq!(
            text(&lines(&rows, Field::Path)),
            "C:\\Windows\r\nC:\\Windows"
        );
        assert_eq!(
            text(&lines(&rows, Field::FullPath)),
            "C:\\Windows\\notepad.exe\r\nC:\\Windows\\System32"
        );
        assert_eq!(
            text(&lines(&rows, Field::QuotedFullPath)),
            "\"C:\\Windows\\notepad.exe\"\r\n\"C:\\Windows\\System32\""
        );
        // Folders show no size, as in the list.
        // W9: a folder's size too, when it has one (folder sizes).
        assert_eq!(text(&lines(&rows, Field::Size)), "352 KB\r\n4 KB");
        // Nor an extension.
        assert_eq!(text(&lines(&rows, Field::Extension)), "exe\r\n");
        assert_eq!(text(&lines(&rows, Field::DateModified)), "\r\n");
    }

    #[test]
    fn tsv_has_the_shown_columns_in_their_order() {
        let rows = [
            row(r"C:\a\b.txt", Some(1), false),
            row(r"C:\a\c", None, true),
        ];
        assert_eq!(
            text(&lines(&rows, Field::Tsv(Layout::default()))),
            "b.txt\tC:\\a\t1 KB\t\r\nc\tC:\\a\t\t"
        );
        let mut layout = Layout::default();
        layout.hide(COL_PATH);
        layout.show(COL_EXT);
        layout.set_order(&[COL_EXT, COL_NAME, COL_SIZE, COL_DATE]);
        assert_eq!(
            text(&lines(&rows, Field::Tsv(layout))),
            "txt\tb.txt\t1 KB\t\r\n\tc\t\t"
        );
    }

    #[test]
    fn columns_map_to_fields() {
        assert_eq!(Field::of_column(COL_NAME), Field::Name);
        assert_eq!(Field::of_column(COL_PATH), Field::Path);
        assert_eq!(Field::of_column(COL_SIZE), Field::Size);
        assert_eq!(Field::of_column(COL_EXT), Field::Extension);
        assert_eq!(Field::of_column(COL_TYPE), Field::Type);
        assert_eq!(Field::of_column(COL_DATE), Field::DateModified);
        assert_eq!(Field::of_column(COL_CREATED), Field::DateCreated);
        assert_eq!(Field::of_column(COL_ACCESSED), Field::DateAccessed);
        assert_eq!(Field::of_column(COL_ATTRIB), Field::Attributes);
        assert_eq!(Field::of_column(COL_RECENT), Field::DateRecentlyChanged);
    }

    /// Wave 9: the new columns' text, from the row's source or the disk.
    #[test]
    fn the_wave_9_fields() {
        use crate::props::{Disk, More};
        let mut r = row(r"C:\a\b.txt", Some(1), false);
        let empty = |r: &Row, f| text(&lines(std::slice::from_ref(r), f));
        assert_eq!(empty(&r, Field::Attributes), "");
        assert_eq!(empty(&r, Field::DateRecentlyChanged), "");
        r.more.set(More::default().read(Some(Disk {
            attrib: 0x21,
            ctime: 132_000_000_000_000_000,
            atime: 132_000_000_000_000_000,
        })));
        assert_eq!(empty(&r, Field::Attributes), "RA");
        let mut date = Vec::new();
        format::date(132_000_000_000_000_000, &mut date);
        assert_eq!(empty(&r, Field::DateAccessed), text(&date));
        assert_eq!(empty(&r, Field::DateCreated), text(&date));
        // A file list's own date created comes first.
        r.ctime = Some(0);
        assert_ne!(empty(&r, Field::DateCreated), text(&date));
        r.more
            .set(More::given(None, None, None, Some(132_000_000_000_000_000)));
        assert_eq!(empty(&r, Field::DateRecentlyChanged), text(&date));
    }
}
