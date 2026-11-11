//! File > Export: every result of a search written to a file, in one of
//! Everything's formats: an Everything File List (EFU), CSV with the
//! columns shown, or full paths one per line.
//!
//! The rows come from gozd on a thread of its own (`client::stream`), page
//! by page, and each page is written out as it arrives, so an export of
//! millions of files never holds more than a page. The file is written to
//! a tmp file beside it and renamed into place once complete; a stopped or
//! failed export deletes the tmp file and leaves whatever was there.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::client::{self, Failure, Row, Search, Stop, Streamed};
use crate::columns::{COL_DATE, COL_EXT, COL_NAME, COL_PATH, COL_SIZE, COL_TYPE, TITLES};
use crate::copytext::{self, Field};
use crate::filedlg::Kind;
use crate::store::file;

/// What an export writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Efu,
    Csv,
    Txt,
}

/// The Save dialog's file types, in [`Format::ALL`]'s order, under
/// Everything's names.
pub const KINDS: &[Kind] = &[
    ("EFU Everything File List (*.efu)", "*.efu"),
    ("CSV (Comma Separated Values) (*.csv)", "*.csv"),
    ("TXT Text, full paths (*.txt)", "*.txt"),
];

impl Format {
    /// In Everything's `last_export_type` order.
    pub const ALL: [Format; 3] = [Format::Efu, Format::Csv, Format::Txt];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Efu => "efu",
            Format::Csv => "csv",
            Format::Txt => "txt",
        }
    }

    /// The format a file name's extension names, in any case.
    pub fn of_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?;
        Format::ALL
            .into_iter()
            .find(|f| f.extension().eq_ignore_ascii_case(ext))
    }

    /// The first line, CRLF included; none for paths.
    pub fn header(self, columns: &[usize]) -> String {
        match self {
            // Everything's own header; goz has no creation dates or
            // attributes, so those columns stay empty.
            Format::Efu => "Filename,Size,Date Modified,Date Created,Attributes\r\n".into(),
            Format::Csv => {
                let titles: Vec<&str> = columns.iter().map(|&c| TITLES[c]).collect();
                format!("{}\r\n", titles.join(","))
            }
            Format::Txt => String::new(),
        }
    }

    /// Appends `row` as one line, CRLF included.
    pub fn push_row(self, row: &Row, columns: &[usize], out: &mut String) {
        match self {
            Format::Efu => {
                let mut path = String::from_utf16_lossy(&row.path);
                // Everything's `efu_folder_append_path_separator`: a folder
                // reads as one by its trailing backslash.
                if row.is_dir && !path.ends_with('\\') {
                    path.push('\\');
                }
                push_quoted(&path, out);
                out.push(',');
                if let (false, Some(size)) = (row.is_dir, row.size) {
                    out.push_str(&size.to_string());
                }
                out.push(',');
                // `efu_filetime`: raw FILETIMEs.
                if let Some(ft) = row.mtime {
                    out.push_str(&ft.to_string());
                }
                out.push_str(",,\r\n");
            }
            Format::Csv => {
                let mut text = Vec::new();
                for (i, &col) in columns.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    text.clear();
                    copytext::push(row, Field::of_column(col), &mut text);
                    // As the list shows it; an empty value stays bare, as
                    // Everything writes unset ones.
                    if !text.is_empty() {
                        push_quoted(&String::from_utf16_lossy(&text), out);
                    }
                }
                out.push_str("\r\n");
            }
            Format::Txt => {
                out.push_str(&String::from_utf16_lossy(&row.path));
                out.push_str("\r\n");
            }
        }
    }
}

/// `text` in double quotes, a quote in it doubled (RFC 4180).
fn push_quoted(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
}

/// The columns a CSV export writes: those shown, in their order on screen.
pub fn csv_columns(order: &[usize]) -> Vec<usize> {
    let known = [COL_NAME, COL_PATH, COL_SIZE, COL_EXT, COL_TYPE, COL_DATE];
    order
        .iter()
        .copied()
        .filter(|c| known.contains(c))
        .collect()
}

/// WM_EXPORT's wParam: more rows were written (the counts are in
/// [`Running::progress`]).
pub const PROGRESS: usize = 0;
/// WM_EXPORT's wParam: the export ended; lParam is a `Box<Ended>`.
pub const ENDED: usize = 1;

/// How an export ended.
pub enum Ended {
    /// Written: how many rows, and where.
    Written(u64, PathBuf),
    Stopped,
    Failed(String),
}

/// An export under way.
pub struct Running {
    stop: Stop,
    thread: Option<JoinHandle<()>>,
    /// Rows written so far, and the search's total as gozd last said.
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl Running {
    /// Rows written so far, and the total to write.
    pub fn progress(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    /// Stops it; its tmp file goes, and an `Ended::Stopped` is posted.
    pub fn stop(&self) {
        self.stop.stop();
    }

    /// Stops it and waits for its thread (the window is closing).
    pub fn finish(mut self) {
        self.stop.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What to export, and where.
pub struct Job {
    pub search: Search,
    pub format: Format,
    pub columns: Vec<usize>,
    pub path: PathBuf,
    /// Everything's `utf8_bom`: the file starts with a UTF-8 byte order mark.
    pub bom: bool,
}

/// Starts an export on its own thread; it reports to `hwnd` as `msg`
/// ([`PROGRESS`], at most a few times a second, then [`ENDED`]).
pub fn start(hwnd: HWND, msg: u32, job: Job) -> Running {
    let stop = Stop::new();
    let done = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let (t_stop, t_done, t_total) = (stop.clone(), Arc::clone(&done), Arc::clone(&total));
    let hwnd = hwnd as usize;
    let thread = std::thread::Builder::new()
        .name("goz-export".into())
        .spawn(move || {
            // The Type column asks the shell for type names, which wants COM
            // on the thread.
            // SAFETY: initializes COM for this new thread only.
            unsafe {
                windows_sys::Win32::System::Com::CoInitializeEx(
                    core::ptr::null(),
                    windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32,
                )
            };
            let ended = run(&job, &t_stop, &t_done, &t_total, hwnd, msg);
            let raw = Box::into_raw(Box::new(ended));
            if !post(hwnd, msg, ENDED, raw as isize) {
                // SAFETY: not posted (the window is gone), so still ours.
                drop(unsafe { Box::from_raw(raw) });
            }
        })
        .ok();
    Running {
        stop,
        thread,
        done,
        total,
    }
}

fn post(hwnd: usize, msg: u32, w: usize, l: isize) -> bool {
    // SAFETY: posts to the window; the caller takes back a box in `l` when
    // this fails.
    unsafe { PostMessageW(hwnd as HWND, msg, w, l) != 0 }
}

fn run(
    job: &Job,
    stop: &Stop,
    done: &AtomicU64,
    total: &AtomicU64,
    hwnd: usize,
    msg: u32,
) -> Ended {
    let tmp = file::tmp_path(&job.path);
    let out = match File::create(&tmp) {
        Ok(f) => f,
        Err(e) => return Ended::Failed(format!("{} cannot be written: {e}", job.path.display())),
    };
    let mut out = BufWriter::with_capacity(1 << 20, out);
    let mut head = String::new();
    if job.bom {
        head.push('\u{feff}');
    }
    head.push_str(&job.format.header(&job.columns));
    let head_failed = out.write_all(head.as_bytes()).err();
    let mut last_post = std::time::Instant::now();
    let mut text = String::new();
    let streamed = if head_failed.is_none() {
        client::stream(stop, &job.search, |rows, all| {
            text.clear();
            for row in &rows {
                job.format.push_row(row, &job.columns, &mut text);
            }
            out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            total.store(all, Ordering::Relaxed);
            done.fetch_add(rows.len() as u64, Ordering::Relaxed);
            // The status bar follows, a few times a second.
            if last_post.elapsed().as_millis() >= 100 {
                last_post = std::time::Instant::now();
                post(hwnd, msg, PROGRESS, 0);
            }
            Ok(())
        })
    } else {
        Streamed::Stopped
    };
    let ended = match (head_failed, streamed) {
        (Some(e), _) => Ended::Failed(format!("{} could not be written: {e}", job.path.display())),
        (None, Streamed::Done(n)) => {
            // On the disk before the rename makes it the file.
            let wrote = out
                .into_inner()
                .map_err(|e| e.error().to_string())
                .and_then(|f| f.sync_all().map_err(|e| e.to_string()))
                .and_then(|()| file::replace(&tmp, &job.path).map_err(|e| e.to_string()));
            return match wrote {
                Ok(()) => Ended::Written(n, job.path.clone()),
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    Ended::Failed(format!("{} could not be written: {e}", job.path.display()))
                }
            };
        }
        (None, Streamed::Stopped) => Ended::Stopped,
        (None, Streamed::Failed(f)) => Ended::Failed(match f {
            Failure::NotRunning => "gozd is not running".into(),
            Failure::Untrusted => "gozd could not be verified".into(),
            Failure::Refused(m) | Failure::Other(m) => m,
        }),
    };
    drop(out);
    let _ = fs::remove_file(&tmp);
    ended
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn row(path: &str, size: Option<u64>, mtime: Option<i64>, is_dir: bool) -> Row {
        let path: Box<[u16]> = path.encode_utf16().collect();
        let name_start = path
            .iter()
            .rposition(|&c| c == u16::from(b'\\'))
            .map_or(0, |i| i + 1);
        Row {
            path,
            name_start: name_start as u32,
            size,
            mtime,
            ctime: None,
            is_dir,
            icon: Cell::new(0),
        }
    }

    #[test]
    fn efu_rows_are_everythings() {
        let cols = [COL_NAME];
        assert_eq!(
            Format::Efu.header(&cols),
            "Filename,Size,Date Modified,Date Created,Attributes\r\n"
        );
        let mut out = String::new();
        Format::Efu.push_row(
            &row(
                r"C:\Windows\cmd.exe",
                Some(289_792),
                Some(133_000_000_000_000_000),
                false,
            ),
            &cols,
            &mut out,
        );
        Format::Efu.push_row(&row(r"C:\Windows", Some(5), Some(1), true), &cols, &mut out);
        Format::Efu.push_row(&row(r"C:\", None, None, true), &cols, &mut out);
        Format::Efu.push_row(&row(r#"C:\a "b".txt"#, None, None, false), &cols, &mut out);
        assert_eq!(
            out,
            "\"C:\\Windows\\cmd.exe\",289792,133000000000000000,,\r\n\
             \"C:\\Windows\\\",,1,,\r\n\
             \"C:\\\",,,,\r\n\
             \"C:\\a \"\"b\"\".txt\",,,,\r\n"
        );
    }

    #[test]
    fn csv_has_the_shown_columns_as_shown() {
        assert_eq!(
            csv_columns(&[COL_TYPE, COL_NAME, COL_EXT]),
            [COL_TYPE, COL_NAME, COL_EXT]
        );
        let cols = csv_columns(&[COL_PATH, COL_NAME]);
        assert_eq!(Format::Csv.header(&cols), "Path,Name\r\n");
        let mut out = String::new();
        Format::Csv.push_row(
            &row(r#"C:\x\say "hi", ok.txt"#, Some(10), None, false),
            &cols,
            &mut out,
        );
        assert_eq!(out, "\"C:\\x\",\"say \"\"hi\"\", ok.txt\"\r\n");
        // An empty value is bare: a folder's size, a missing date.
        let cols = [COL_NAME, COL_SIZE, COL_DATE];
        let mut out = String::new();
        Format::Csv.push_row(&row(r"C:\d", None, None, true), &cols, &mut out);
        assert_eq!(out, "\"d\",,\r\n");
    }

    #[test]
    fn txt_is_full_paths() {
        assert_eq!(Format::Txt.header(&[COL_NAME]), "");
        let mut out = String::new();
        Format::Txt.push_row(&row(r"C:\a\b.txt", None, None, false), &[], &mut out);
        Format::Txt.push_row(&row(r"C:\a", None, None, true), &[], &mut out);
        assert_eq!(out, "C:\\a\\b.txt\r\nC:\\a\r\n");
    }

    #[test]
    fn formats_by_extension() {
        assert_eq!(Format::of_path(Path::new(r"C:\x.EFU")), Some(Format::Efu));
        assert_eq!(Format::of_path(Path::new("a.csv")), Some(Format::Csv));
        assert_eq!(Format::of_path(Path::new("a.txt")), Some(Format::Txt));
        assert_eq!(Format::of_path(Path::new("a.json")), None);
        assert_eq!(Format::of_path(Path::new("a")), None);
        assert_eq!(KINDS.len(), Format::ALL.len());
        for (kind, f) in KINDS.iter().zip(Format::ALL) {
            assert!(kind.1.ends_with(f.extension()));
        }
    }
}
