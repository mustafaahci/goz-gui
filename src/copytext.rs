//! The text of the copy commands: the values of a row exactly as the list
//! shows them, one row per line.

use crate::client::Row;
use crate::format;
use crate::window::{COL_DATE, COL_NAME, COL_PATH, COL_SIZE};

/// What to copy of each row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    /// The folder holding the row, as the Path column shows it.
    Path,
    FullPath,
    Size,
    DateModified,
    /// Name, Path, Size and Date Modified, tab-separated.
    Tsv,
}

impl Field {
    /// The field a list column shows.
    pub fn of_column(col: usize) -> Field {
        match col {
            COL_NAME => Field::Name,
            COL_PATH => Field::Path,
            COL_SIZE => Field::Size,
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
        Field::Size => {
            if let (false, Some(size)) = (row.is_dir, row.size) {
                out.extend(format::size_kb(size).encode_utf16());
            }
        }
        Field::DateModified => {
            if let Some(ft) = row.mtime {
                format::date(ft, out);
            }
        }
        Field::Tsv => {
            for (i, col) in [COL_NAME, COL_PATH, COL_SIZE, COL_DATE]
                .into_iter()
                .enumerate()
            {
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
    use std::cell::Cell;

    fn row(path: &str, size: Option<u64>, is_dir: bool) -> Row {
        let path: Box<[u16]> = path.encode_utf16().collect();
        let name_start = path.iter().rposition(|&c| c == u16::from(b'\\')).unwrap() + 1;
        Row {
            path,
            name_start: name_start as u32,
            size,
            mtime: None,
            is_dir,
            icon: Cell::new(0),
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
        // Folders show no size, as in the list.
        assert_eq!(text(&lines(&rows, Field::Size)), "352 KB\r\n");
        assert_eq!(text(&lines(&rows, Field::DateModified)), "\r\n");
    }

    #[test]
    fn tsv_has_the_four_columns() {
        let rows = [
            row(r"C:\a\b.txt", Some(1), false),
            row(r"C:\a\c", None, true),
        ];
        assert_eq!(
            text(&lines(&rows, Field::Tsv)),
            "b.txt\tC:\\a\t1 KB\t\r\nc\tC:\\a\t\t"
        );
    }

    #[test]
    fn columns_map_to_fields() {
        assert_eq!(Field::of_column(COL_NAME), Field::Name);
        assert_eq!(Field::of_column(COL_PATH), Field::Path);
        assert_eq!(Field::of_column(COL_SIZE), Field::Size);
        assert_eq!(Field::of_column(COL_DATE), Field::DateModified);
    }
}
