//! `Search History.csv`: the search history in the data folder, in
//! Everything's own format, so the file can be carried between the two:
//!
//! ```text
//! Search,Search Count,Last Search Date
//! "c:\windows\system32\*.exe",3,134386456820209713
//! ```
//!
//! The search quoted, the count and the date (a FILETIME) bare, CRLF, UTF-8
//! without a byte order mark, newest first. Columns another program added
//! are kept on a rewrite.
//!
//! The file is read once at startup, before the window shows. It is written
//! by the store thread (`HistoryFile::write_now`), which reads it again
//! first and merges this process's history into it (`history::merge`), so
//! two goz-gui windows never write each other's searches out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use crate::csv::{self, Field};
use crate::history::{self, Forgotten, Keep, Searched, filetime_now, tidy};
use crate::store::file::{self, Loaded};
use crate::store::join_notes;

/// The file's name in the data folder, as Everything's.
pub const FILE_NAME: &str = "Search History.csv";
/// Everything's columns, in its order.
const COLUMNS: [&str; 3] = ["Search", "Search Count", "Last Search Date"];
/// How long a read or write waits for another goz-gui on the same file.
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// A search history file as read: its searches, and the columns goz-gui
/// does not know, kept for the rewrite.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    /// Newest first, each search once.
    pub entries: Vec<Searched>,
    /// The names of the unknown columns, after Everything's three.
    extra: Vec<String>,
    /// Each search's values in those columns.
    extras: HashMap<String, Vec<Field>>,
}

/// The text of a file's bytes, or why they are not a search history's.
fn text_of(bytes: &[u8]) -> Result<&str, String> {
    if bytes.contains(&0) {
        return Err("it holds NUL characters".into());
    }
    std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())
}

/// Reads the search history file. Its columns may come in any order and
/// any case; a missing count is 1 and a missing date 0. A file holding
/// NULs, not UTF-8, with no `Search` column (a power cut, another file
/// saved over it) or with a value that is not well-formed CSV (cut short
/// inside a quote, edited by hand into something else) is not read as a
/// search history: `Err` says why, and the file is moved aside.
pub fn parse(bytes: &[u8]) -> Result<Table, String> {
    let checked = csv::parse_checked(text_of(bytes)?);
    if let Some(problem) = checked.problem() {
        return Err(problem);
    }
    table(checked.records)
}

/// [`parse`] for a file the user imports: a value that is not well-formed
/// CSV is read as leniently as it can be (whatever can be taken is).
pub fn parse_import(bytes: &[u8]) -> Result<Table, String> {
    table(csv::parse(text_of(bytes)?))
}

/// The searches still readable in a file that was damaged (cut short, a
/// quote never closed): its well-formed rows, but not a last one cut short.
/// They are merged with the backup's, so the searches made since the
/// backup are not lost with the damaged file.
pub fn salvage(bytes: &[u8]) -> Vec<Searched> {
    let Ok(text) = text_of(bytes) else {
        return Vec::new();
    };
    let checked = csv::parse_checked(text);
    let mut records = checked.records;
    if csv::cut_short(text) {
        records.pop();
    }
    let bad = checked.bad;
    let whole = records
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !bad.contains(i))
        .map(|(_, r)| r)
        .collect();
    table(whole).map(|t| t.entries).unwrap_or_default()
}

/// A search history from its records, the header first.
fn table(records: Vec<Vec<Field>>) -> Result<Table, String> {
    let mut records = records.into_iter();
    let Some(header) = records.next() else {
        return Ok(Table::default());
    };
    // Each of the file's columns: one of Everything's (by its index in
    // COLUMNS), or an unknown one (by its index in `extra`).
    let mut extra = Vec::new();
    let mut seen = [false; COLUMNS.len()];
    let slots: Vec<Result<usize, usize>> = header
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
                    Err(extra.len() - 1)
                }
            }
        })
        .collect();
    if !seen[0] {
        return Err("it has no Search column".into());
    }
    let mut entries = Vec::new();
    let mut extras: HashMap<String, Vec<Field>> = HashMap::new();
    for record in records {
        let mut row = Searched {
            search: String::new(),
            count: 1,
            last: 0,
        };
        let mut values = vec![Field::plain(""); extra.len()];
        // Values past the header's columns have no name to keep.
        for (field, slot) in record.into_iter().zip(&slots) {
            match *slot {
                Ok(0) => row.search = field.text,
                Ok(1) => row.count = field.text.trim().parse().unwrap_or(1),
                Ok(_) => row.last = field.text.trim().parse().unwrap_or(0),
                Err(i) => values[i] = field,
            }
        }
        if row.search.is_empty() {
            continue;
        }
        if !extra.is_empty() {
            extras.entry(row.search.clone()).or_insert(values);
        }
        entries.push(row);
    }
    Ok(Table {
        entries: tidy(entries),
        extra,
        extras,
    })
}

/// The file's text for `entries`, in Everything's columns and then the
/// unknown ones `columns` was read with (each search keeps its values).
pub fn to_csv(entries: &[Searched], columns: &Table) -> String {
    let mut out = String::new();
    let header: Vec<Field> = COLUMNS
        .iter()
        .map(|&c| Field::plain(c))
        .chain(columns.extra.iter().map(|c| Field::plain(c.as_str())))
        .collect();
    csv::write_record(&header, &mut out);
    let blank = vec![Field::plain(""); columns.extra.len()];
    for e in entries {
        let mut row = vec![
            Field::quoted(e.search.as_str()),
            Field::plain(e.count.to_string()),
            Field::plain(e.last.to_string()),
        ];
        if !columns.extra.is_empty() {
            let values = columns.extras.get(&e.search).unwrap_or(&blank);
            row.extend(values.iter().cloned());
        }
        csv::write_record(&row, &mut out);
    }
    out
}

/// The file's name, for the user.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What to say when the file could not be parsed and was moved aside.
fn recovered_note(moved_to: &Path, why: &str, from_backup: bool, salvaged: bool) -> String {
    let then = match (from_backup, salvaged) {
        (true, false) => "its backup was used",
        (true, true) => "its backup was used, with the searches still readable in it",
        (false, true) => "the searches still readable in it were kept",
        (false, false) => "the search history starts empty",
    };
    format!(
        "The search history could not be read ({why}); it was kept as {} and {then}",
        name_of(moved_to)
    )
}

/// The file as read for a write: its table, and its bytes when it was read
/// whole (the backup the write keeps); or, when it was damaged, the
/// backup's table with the rows still readable in the damaged file, the
/// note saying so, and whether that is more than the file now holds.
struct OnDisk {
    table: Table,
    previous: Option<Vec<u8>>,
    note: Option<String>,
    /// The file (put back from the backup) lacks rows `table` has.
    stale: bool,
}

/// Reads the file (holding its lock), recovering a damaged one.
fn read(path: &Path) -> Result<OnDisk, String> {
    match file::load_checked(path, parse, csv::suspect) {
        Loaded::Missing => Ok(OnDisk {
            table: Table::default(),
            previous: None,
            note: None,
            stale: false,
        }),
        Loaded::Read(table, bytes) => Ok(OnDisk {
            table,
            previous: Some(bytes),
            note: None,
            stale: false,
        }),
        Loaded::Recovered {
            backup,
            moved_to,
            why,
            damaged,
        } => {
            let from_backup = backup.is_some();
            let mut table = backup.unwrap_or_default();
            let saved = salvage(&damaged);
            let mut stale = false;
            if !saved.is_empty() {
                let mut all = table.entries.clone();
                all.extend(saved);
                let all = tidy(all);
                stale = all != table.entries;
                table.entries = all;
            }
            Ok(OnDisk {
                note: Some(recovered_note(&moved_to, &why, from_backup, stale)),
                table,
                previous: None,
                stale,
            })
        }
        Loaded::Unreadable(why) => Err(why),
    }
}

/// Reads the search history at startup (before the window shows: the file
/// is small), with the searches older than `keep` days left out, and what
/// to tell the user if the file could not be read.
pub fn load(path: &Path, keep: Keep, now: u64) -> (Vec<Searched>, Option<String>) {
    // Another goz-gui writing or recovering the file goes first.
    let _lock = file::Lock::acquire(path, LOCK_WAIT);
    let (mut entries, note) = match read(path) {
        Ok(on_disk) => {
            if on_disk.stale && !on_disk.table.entries.is_empty() {
                // The searches taken from the damaged file go in at once,
                // beside the backup's (which was put back).
                let text = to_csv(&on_disk.table.entries, &on_disk.table);
                let _ = file::write(path, text.as_bytes(), None);
            }
            (on_disk.table.entries, on_disk.note)
        }
        // It is left alone; writes refuse to go over it.
        Err(why) => (
            Vec::new(),
            Some(format!("The search history could not be read: {why}")),
        ),
    };
    history::prune(&mut entries, keep, now);
    (entries, note)
}

#[derive(Default)]
struct Staged {
    /// The history as the window has it, waiting to be written.
    snapshot: Option<Vec<Searched>>,
    /// What was deleted here and is not yet out of the file.
    forgotten: Forgotten,
}

/// The search history file, shared by the window (which stages its
/// history and its deletions) and the store thread (which writes them).
pub struct HistoryFile {
    path: PathBuf,
    keep: Keep,
    backup: bool,
    staged: Mutex<Staged>,
}

/// What a write did.
#[derive(Debug)]
pub struct Written {
    /// The history as written: this process's merged with the file's.
    pub entries: Vec<Searched>,
    /// The file could not be parsed, and was moved aside first.
    pub note: Option<String>,
}

impl HistoryFile {
    /// `keep`: the days searches are kept (`None` forever); `backup`:
    /// whether the file before each write is kept (`csv_backup`).
    pub fn new(path: PathBuf, keep: Keep, backup: bool) -> HistoryFile {
        HistoryFile {
            path,
            keep,
            backup,
            staged: Mutex::new(Staged::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Staged> {
        self.staged.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The history to write next, in place of any not written yet.
    pub fn stage(&self, entries: Vec<Searched>) {
        self.lock().snapshot = Some(entries);
    }

    /// A history staged and not written yet (a write failed): the next
    /// save writes it even when nothing else changed.
    pub fn pending(&self) -> bool {
        self.lock().snapshot.is_some()
    }

    /// `search` was deleted at `now`.
    pub fn forget(&self, search: &str, now: u64) {
        self.lock().forgotten.forget(search, now);
    }

    /// Every search was deleted at `now`.
    pub fn forget_all(&self, now: u64) {
        self.lock().forgotten.forget_all(now);
    }

    /// The deletions not yet written.
    pub fn forgotten(&self) -> Forgotten {
        self.lock().forgotten.clone()
    }

    /// Writes what is staged, merged into the file as it is now. Run on the
    /// store thread. `Ok(None)`: nothing was staged. On failure the history
    /// stays staged for the next write (unless a newer one came meanwhile).
    pub fn write_now(&self) -> Result<Option<Written>, String> {
        let (mine, forgotten) = {
            let mut staged = self.lock();
            let Some(mine) = staged.snapshot.take() else {
                return Ok(None);
            };
            (mine, staged.forgotten.clone())
        };
        let result = self.merge_and_write(&mine, &forgotten);
        let mut staged = self.lock();
        match result {
            Ok(written) => {
                staged.forgotten.settle(&forgotten);
                Ok(Some(written))
            }
            Err(why) => {
                if staged.snapshot.is_none() {
                    staged.snapshot = Some(mine);
                }
                Err(why)
            }
        }
    }

    fn merge_and_write(&self, mine: &[Searched], forgotten: &Forgotten) -> Result<Written, String> {
        let path = &self.path;
        // Not written over another goz-gui's write: tried again later.
        let _lock = file::Lock::for_write(path, file::WRITE_WAIT)?;
        let theirs = read(path).map_err(|why| {
            format!(
                "{} could not be read, so goz-gui does not write over it: {why}",
                path.display()
            )
        })?;
        let mut entries = history::merge(mine, &theirs.table.entries, forgotten);
        history::prune(&mut entries, self.keep, filetime_now());
        let mut note = theirs.note;
        if entries != theirs.table.entries || theirs.stale || !path.is_file() {
            let text = to_csv(&entries, &theirs.table);
            let previous = theirs.previous.as_deref().filter(|_| self.backup);
            let kept = file::write(path, text.as_bytes(), previous).map_err(|e| {
                format!(
                    "The search history could not be saved to {}: {e}",
                    path.display()
                )
            })?;
            note = join_notes(note, kept);
        }
        Ok(Written { entries, note })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn s(search: &str, count: u32, last: u64) -> Searched {
        Searched {
            search: search.into(),
            count,
            last,
        }
    }

    /// Everything 1.5's own file, byte for byte.
    const EVERYTHING: &str = "Search,Search Count,Last Search Date\r\n\
        \"c:\\windows\\system32\\*.exe\",3,134386456820209713\r\n\
        \"*.dll\",2,134386456534072690\r\n\
        \"*.png|*.jpg\",1,134386453783329222\r\n";

    #[test]
    fn everythings_file_reads_and_writes_back_the_same() {
        let t = parse(EVERYTHING.as_bytes()).expect("parses");
        assert_eq!(
            t.entries,
            [
                s(r"c:\windows\system32\*.exe", 3, 134386456820209713),
                s("*.dll", 2, 134386456534072690),
                s("*.png|*.jpg", 1, 134386453783329222),
            ]
        );
        assert_eq!(to_csv(&t.entries, &t), EVERYTHING);
        // A file of goz-gui's own reads the same with no columns kept.
        assert_eq!(to_csv(&t.entries, &Table::default()), EVERYTHING);
    }

    #[test]
    fn quotes_commas_line_breaks_and_unicode_survive() {
        let entries = vec![
            s("say \"hi\", then go", 2, 30),
            s("line one\r\nline two", 1, 20),
            s("ünïcödé ✓ 日本", 1, 10),
        ];
        let text = to_csv(&entries, &Table::default());
        assert!(text.contains("\"say \"\"hi\"\", then go\",2,30\r\n"));
        assert_eq!(parse(text.as_bytes()).expect("parses").entries, entries);
    }

    #[test]
    fn columns_in_any_order_missing_or_unknown() {
        let text = "\u{feff}LAST SEARCH DATE,Colour,search\r\n5,red,\"b\"\r\n9,\"blue, light\",a\r\n,green,c\r\n,,\r\n";
        let t = parse(text.as_bytes()).expect("parses");
        assert_eq!(t.entries, [s("a", 1, 9), s("b", 1, 5), s("c", 1, 0)]);
        // Rewritten in Everything's order with the unknown column after,
        // each search keeping its value; a new search gets an empty one.
        let mut entries = t.entries.clone();
        entries.insert(0, s("new", 1, 10));
        assert_eq!(
            to_csv(&entries, &t),
            "Search,Search Count,Last Search Date,Colour\r\n\
             \"new\",1,10,\r\n\
             \"a\",1,9,\"blue, light\"\r\n\
             \"b\",1,5,red\r\n\
             \"c\",1,0,green\r\n"
        );
        // A count or date that is not a number reads as the default.
        let t = parse(b"Search,Search Count,Last Search Date\r\n\"x\",many,soon\r\n").expect("ok");
        assert_eq!(t.entries, [s("x", 1, 0)]);
        // The same search twice is one, with the higher count and date.
        let t = parse(b"Search,Search Count,Last Search Date\r\nx,2,5\r\nx,7,3\r\n").expect("ok");
        assert_eq!(t.entries, [s("x", 7, 5)]);
    }

    #[test]
    fn what_is_not_a_search_history() {
        assert!(parse(b"").expect("empty is fine").entries.is_empty());
        assert!(parse(b"\r\n").expect("blank is fine").entries.is_empty());
        assert!(
            parse(b"Search\r\n")
                .expect("header only")
                .entries
                .is_empty()
        );
        assert_eq!(
            parse(b"Search,Search Count\r\n\0\0").err().as_deref(),
            Some("it holds NUL characters")
        );
        assert_eq!(
            parse(b"Search\r\n\"caf\xe9\"\r\n").err().as_deref(),
            Some("it is not UTF-8 text")
        );
        assert_eq!(
            parse(b"Name,Path\r\nx,y\r\n").err().as_deref(),
            Some("it has no Search column")
        );
    }

    #[test]
    fn a_file_that_is_not_well_formed_is_refused_but_imported() {
        // A quote never closed: the next row's opening quote closes it.
        let text = "Search,Search Count,Last Search Date\r\n\"abc,1,5\r\n\"def\",2,6\r\n\
                    \"ghi\",3,7\r\n";
        assert!(
            parse(text.as_bytes())
                .err()
                .is_some_and(|why| why.contains("closing quote"))
        );
        // Cut short inside a quote.
        let cut = "Search,Search Count,Last Search Date\r\n\"abc\",1,5\r\n\"de";
        assert!(
            parse(cut.as_bytes())
                .err()
                .is_some_and(|why| why.contains("never closed"))
        );
        // An import takes what it can.
        assert_eq!(
            parse_import(text.as_bytes())
                .expect("lenient")
                .entries
                .len(),
            2
        );
        // What is still readable: the whole rows, not the merged or cut one.
        assert_eq!(salvage(text.as_bytes()), [s("ghi", 3, 7)]);
        assert_eq!(salvage(cut.as_bytes()), [s("abc", 1, 5)]);
        let cut_after_comma = "Search,Search Count,Last Search Date\r\n\"abc\",1,5\r\n\"de\",";
        assert_eq!(salvage(cut_after_comma.as_bytes()), [s("abc", 1, 5)]);
        assert!(salvage(&[0u8; 16]).is_empty());
    }

    #[test]
    fn a_damaged_file_gives_its_backup_and_its_readable_rows_and_is_put_back() {
        let path = scratch("history-cut");
        let backup = "Search,Search Count,Last Search Date\r\n\"old\",1,10\r\n\"kept\",1,5\r\n";
        fs::write(file::backup_path(&path), backup).expect("backup");
        // Cut short by a sync tool inside the third row.
        fs::write(
            &path,
            "Search,Search Count,Last Search Date\r\n\"new\",1,30\r\n\"old\",2,20\r\n\"cu",
        )
        .expect("cut");
        let (entries, note) = load(&path, None, 100);
        assert_eq!(entries, [s("new", 1, 30), s("old", 2, 20), s("kept", 1, 5)]);
        let note = note.expect("a note");
        assert!(note.contains("with the searches still readable"), "{note}");
        // In the file at once (a second window, the next start), and the
        // backup is still the last whole file.
        assert_eq!(
            parse(&fs::read(&path).expect("put back"))
                .expect("whole")
                .entries,
            entries
        );
        assert_eq!(
            fs::read_to_string(file::backup_path(&path)).expect("backup"),
            backup
        );
        // A 0-byte file beside a good backup is damaged too: the backup is
        // never replaced by an empty file.
        fs::write(&path, "").expect("emptied");
        let (entries, note) = load(&path, None, 100);
        assert_eq!(entries, [s("old", 1, 10), s("kept", 1, 5)]);
        assert!(note.is_some_and(|n| n.contains("it is empty")));
        let f = HistoryFile::new(path.clone(), None, true);
        for (i, now) in [(1, 40), (2, 50)] {
            let mut mine = entries.clone();
            mine.insert(0, s(&format!("mine {i}"), 1, now));
            f.stage(mine);
            f.write_now().expect("ok");
        }
        let backup = parse(&fs::read(file::backup_path(&path)).expect("backup")).expect("whole");
        assert!(backup.entries.iter().any(|e| e.search == "kept"));
        assert!(backup.entries.iter().any(|e| e.search == "mine 1"));
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_failed_write_stays_pending_until_written() {
        let path = scratch("history-pending");
        let f = HistoryFile::new(path.clone(), None, true);
        assert!(!f.pending());
        fs::create_dir(&path).expect("a folder in its place");
        f.stage(vec![s("x", 1, 1)]);
        assert!(f.write_now().is_err());
        assert!(f.pending(), "kept for the next save");
        fs::remove_dir(&path).expect("gone");
        assert!(f.write_now().expect("ok").is_some());
        assert!(!f.pending());
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    fn scratch(name: &str) -> PathBuf {
        file::tests::scratch(name).join(FILE_NAME)
    }

    #[test]
    fn a_write_merges_with_what_another_process_wrote() {
        let path = scratch("history-merge");
        let mine = HistoryFile::new(path.clone(), None, true);
        // Nothing staged: nothing written.
        assert!(mine.write_now().expect("no error").is_none());
        assert!(!path.exists());
        mine.stage(vec![s("a", 1, 50)]);
        let w = mine.write_now().expect("ok").expect("written");
        assert_eq!(w.entries, [s("a", 1, 50)]);
        // Another window adds "b" and counts "a" more.
        fs::write(
            &path,
            "Search,Search Count,Last Search Date\r\n\"b\",1,60\r\n\"a\",4,40\r\n",
        )
        .expect("theirs");
        mine.stage(vec![s("c", 1, 70), s("a", 1, 50)]);
        let w = mine.write_now().expect("ok").expect("written");
        assert_eq!(w.entries, [s("c", 1, 70), s("b", 1, 60), s("a", 4, 50)]);
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "Search,Search Count,Last Search Date\r\n\"c\",1,70\r\n\"b\",1,60\r\n\"a\",4,50\r\n"
        );
        // The file before the write is the backup.
        assert!(
            fs::read_to_string(file::backup_path(&path))
                .expect("backup")
                .contains("\"b\",1,60")
        );
        // Deleted here: gone from the file, though the file still had it.
        mine.forget("b", 80);
        mine.stage(vec![s("c", 1, 70), s("a", 4, 50)]);
        mine.write_now().expect("ok");
        assert!(!fs::read_to_string(&path).expect("read").contains("\"b\""));
        assert!(mine.forgotten().is_empty(), "settled once written");
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn nothing_new_writes_nothing_and_old_searches_go() {
        const DAY: u64 = 864_000_000_000;
        let now = filetime_now();
        let path = scratch("history-prune");
        let text = format!(
            "Search,Search Count,Last Search Date\r\n\"new\",1,{}\r\n\"old\",1,{}\r\n",
            now - DAY,
            now - 40 * DAY
        );
        fs::write(&path, &text).expect("file");
        // Kept forever: staging what is there already writes nothing.
        let (entries, note) = load(&path, None, now);
        assert_eq!((entries.len(), note), (2, None));
        let f = HistoryFile::new(path.clone(), None, true);
        f.stage(entries);
        f.write_now().expect("ok");
        assert!(!file::backup_path(&path).exists(), "not written");
        // Kept 30 days: left out when read, and out of the file once
        // written (merged from the file or not).
        let (entries, _) = load(&path, Some(30), now);
        assert_eq!(entries.len(), 1);
        let f = HistoryFile::new(path.clone(), Some(30), true);
        f.stage(entries);
        f.write_now().expect("ok");
        assert!(!fs::read_to_string(&path).expect("read").contains("old"));
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_its_backup_used() {
        let path = scratch("history-corrupt");
        fs::write(
            file::backup_path(&path),
            "Search,Search Count,Last Search Date\r\n\"saved\",2,10\r\n",
        )
        .expect("backup");
        fs::write(&path, [0u8; 64]).expect("zeroed by a power cut");
        let (entries, note) = load(&path, None, 100);
        assert_eq!(entries, [s("saved", 2, 10)]);
        let note = note.expect("a note");
        assert!(note.contains("Search History.corrupt-1.csv"), "{note}");
        assert!(note.contains("its backup was used"), "{note}");
        assert_eq!(
            fs::read(file::corrupt_path(&path, 1)).expect("kept"),
            [0u8; 64]
        );
        // The window's history (the backup's searches and one more) is
        // written in the moved file's place.
        let f = HistoryFile::new(path.clone(), None, true);
        let mut mine = entries;
        mine.insert(0, s("mine", 1, 20));
        f.stage(mine);
        f.write_now().expect("ok");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"mine\",1,20") && text.contains("\"saved\",2,10"));
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn an_unreadable_file_is_not_written_over() {
        let path = scratch("history-unreadable");
        fs::create_dir(&path).expect("a folder in its place");
        let (entries, note) = load(&path, None, 1);
        assert!(entries.is_empty());
        assert!(
            note.expect("a note")
                .starts_with("The search history could not be read")
        );
        let f = HistoryFile::new(path.clone(), None, true);
        f.stage(vec![s("x", 1, 1)]);
        assert!(f.write_now().is_err());
        assert!(path.is_dir());
        // Still staged: written once the file can be.
        fs::remove_dir(&path).expect("gone");
        assert!(f.write_now().expect("ok").is_some());
        assert!(fs::read_to_string(&path).expect("read").contains("\"x\""));
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }
}
