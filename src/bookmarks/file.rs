//! `Bookmarks.csv` in the data folder: read at startup, written by the
//! store thread.
//!
//! The merge rule: the window changes its bookmarks through [`Op`]s, which
//! name rows by identity (`ops.rs`). Before each write the file is read
//! again; when another goz-gui (or Everything, or an editor) changed it
//! since this process last read or wrote it, the changes made here since
//! then are made again on the file as it is now, and that is written. So
//! two windows never write each other's bookmarks out; an item deleted by
//! another process and edited here is added again.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::Bookmarks;
use super::ops::Op;
use crate::csv;
use crate::store::file::{self, Loaded, Stamp, stamp};
use crate::store::join_notes;

/// The file's name in the data folder, as Everything's.
pub const FILE_NAME: &str = "Bookmarks.csv";
/// How long a read or write waits for another goz-gui on the same file.
const LOCK_WAIT: Duration = Duration::from_secs(2);

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What to say when the file could not be parsed and was moved aside.
fn recovered_note(moved_to: &Path, why: &str, from_backup: bool) -> String {
    let then = if from_backup {
        "its backup was used"
    } else {
        "the bookmarks start empty"
    };
    format!(
        "The bookmarks could not be read ({why}); the file was kept as {} and {then}",
        name_of(moved_to)
    )
}

/// The file as read.
struct Read {
    list: Bookmarks,
    /// Its bytes, when it was read whole: the backup the next write keeps.
    previous: Option<Vec<u8>>,
    /// What to tell the user.
    note: Option<String>,
    /// Why it could not be read at all (then it is never written over).
    unreadable: Option<String>,
}

/// Reads the file; `base` is the list when it is missing or unreadable. A
/// file that cannot be parsed, or that is empty or cut short beside a good
/// backup, is moved aside and its backup put back in its place.
fn read(path: &Path, base: &Bookmarks) -> Read {
    let none = |list| Read {
        list,
        previous: None,
        note: None,
        unreadable: None,
    };
    match file::load_checked(path, Bookmarks::parse, csv::suspect) {
        Loaded::Missing => none(base.clone()),
        Loaded::Read(list, bytes) => Read {
            previous: Some(bytes),
            ..none(list)
        },
        Loaded::Recovered {
            backup,
            moved_to,
            why,
            ..
        } => Read {
            note: Some(recovered_note(&moved_to, &why, backup.is_some())),
            ..none(backup.unwrap_or_default())
        },
        Loaded::Unreadable(why) => Read {
            note: Some(format!("The bookmarks could not be read: {why}")),
            unreadable: Some(why),
            ..none(base.clone())
        },
    }
}

#[derive(Default)]
struct State {
    /// The file as this process last read or wrote it, and its stamp then.
    base: Arc<Bookmarks>,
    stamp: Option<Stamp>,
    /// The changes made here since, in order.
    ops: Vec<Op>,
    /// `base` with `ops` made: the bookmarks the window shows. The same
    /// list as `base` (not a copy) while no change waits.
    current: Arc<Bookmarks>,
    /// Why the file could not be read: it is not written over.
    unreadable: Option<String>,
    /// Counts the changes of `current`.
    generation: u64,
}

impl State {
    fn rebuild(&mut self) {
        let current = if self.ops.is_empty() {
            Arc::clone(&self.base)
        } else {
            let mut list = (*self.base).clone();
            for op in &self.ops {
                list.apply(op);
            }
            Arc::new(list)
        };
        let changed = !Arc::ptr_eq(&current, &self.current) && *current != *self.current;
        self.current = current;
        if changed {
            self.generation += 1;
        }
    }
}

/// The bookmarks file, shared by the window (which changes the bookmarks)
/// and the store thread (which writes them).
pub struct BookmarkFile {
    /// `None` without a data folder: nothing is read or written.
    path: Option<PathBuf>,
    /// Keep the file before each write (`csv_backup`).
    backup: bool,
    state: Mutex<State>,
}

impl BookmarkFile {
    /// Reads the file (at startup, before the window shows: it is small),
    /// and what to tell the user if it could not be read.
    pub fn load(path: Option<PathBuf>, backup: bool) -> (BookmarkFile, Option<String>) {
        let mut state = State::default();
        let mut note = None;
        if let Some(path) = &path {
            // Another goz-gui writing or recovering the file goes first.
            let _lock = file::Lock::acquire(path, LOCK_WAIT);
            let r = read(path, &Bookmarks::default());
            // Taken after the read: a backup put back is the file now.
            state.stamp = stamp(path);
            state.base = Arc::new(r.list);
            state.current = Arc::clone(&state.base);
            state.unreadable = r.unreadable;
            note = r.note;
        }
        let file = BookmarkFile {
            path,
            backup,
            state: Mutex::new(state),
        };
        (file, note)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The bookmarks as the window has them (shared, not copied).
    pub fn current(&self) -> Arc<Bookmarks> {
        Arc::clone(&self.lock().current)
    }

    /// Counts the changes of the bookmarks, here or read from the file.
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Changes not written yet.
    pub fn pending(&self) -> bool {
        !self.lock().ops.is_empty()
    }

    /// Makes `op`; false (and nothing to write) when it changed nothing.
    pub fn change(&self, op: Op) -> bool {
        let mut state = self.lock();
        let mut list = (*state.current).clone();
        if !list.apply(&op) {
            return false;
        }
        state.current = Arc::new(list);
        state.ops.push(op);
        state.generation += 1;
        true
    }

    /// Reads the file again when another program changed it since, and no
    /// change made here waits to be written (the write merges those). True
    /// when the bookmarks changed; the note says the file could not be read.
    pub fn refresh(&self) -> (bool, Option<String>) {
        let Some(path) = &self.path else {
            return (false, None);
        };
        let mut state = self.lock();
        if !state.ops.is_empty() {
            return (false, None);
        }
        let now = stamp(path);
        if now == state.stamp && state.unreadable.is_none() {
            return (false, None);
        }
        // A missing file (deleted by the user) has no bookmarks.
        let r = read(path, &Bookmarks::default());
        if r.unreadable.is_some() {
            // Kept as it is; still not written over.
            let note = (state.unreadable.is_none()).then_some(r.note).flatten();
            state.unreadable = r.unreadable;
            return (false, note);
        }
        state.unreadable = None;
        state.stamp = stamp(path);
        state.base = Arc::new(r.list);
        let before = state.generation;
        state.rebuild();
        (state.generation != before, r.note)
    }

    /// Writes the changes made here, merged into the file as it is now.
    /// Run on the store thread. `Ok(None)`: nothing to write; `Ok(Some)`:
    /// written, with a note when the file could not be parsed and was moved
    /// aside first. On failure the changes wait for the next write.
    pub fn write_now(&self) -> Result<Option<Option<String>>, String> {
        let (taken, ops, mine, base) = {
            let state = self.lock();
            if state.ops.is_empty() {
                return Ok(None);
            }
            (
                state.ops.len(),
                state.ops.clone(),
                Arc::clone(&state.current),
                Arc::clone(&state.base),
            )
        };
        let Some(path) = &self.path else {
            return Err(
                "The bookmarks could not be saved: goz-gui has no data folder \
                        (Windows did not say where the user's AppData folder is)."
                    .into(),
            );
        };
        // Not written over another goz-gui's write: tried again later.
        let _lock = file::Lock::for_write(path, file::WRITE_WAIT)?;
        // What the file holds now: read every time, so the backup this
        // write keeps is the file as read and parsed whole (never an empty
        // or cut file another program left), and a damaged one is moved
        // aside, not written over.
        let theirs = read(path, &base);
        if let Some(why) = theirs.unreadable {
            return Err(format!(
                "{} could not be read, so goz-gui does not write over it: {why}",
                path.display()
            ));
        }
        let mut note = theirs.note;
        let result = if theirs.list == *base {
            mine
        } else {
            // Changed by another program since: this process's changes are
            // made again on it.
            let mut result = theirs.list.clone();
            for op in &ops {
                result.apply(op);
            }
            Arc::new(result)
        };
        // Nothing changed: not written (a file moved aside, which `read`
        // recovered, is written again all the same).
        if *result != theirs.list || !path.is_file() {
            let text = result.to_csv();
            let previous = theirs.previous.as_deref().filter(|_| self.backup);
            let kept = file::write(path, text.as_bytes(), previous).map_err(|e| {
                format!(
                    "The bookmarks could not be saved to {}: {e}",
                    path.display()
                )
            })?;
            note = join_notes(note, kept);
        }
        let mut state = self.lock();
        state.base = result;
        state.stamp = stamp(path);
        state.unreadable = None;
        state.ops.drain(..taken);
        state.rebuild();
        Ok(Some(note))
    }
}

#[cfg(test)]
mod tests {
    use super::super::ops::{Id, Target};
    use super::super::{Bookmark, Settings};
    use super::*;
    use crate::store::file::tests::scratch;
    use std::fs;

    fn b(name: &str) -> Bookmark {
        Bookmark::new(&[], name, name, &Settings::default())
    }

    fn names(list: &Bookmarks) -> Vec<String> {
        list.rows().iter().map(|r| r.name().to_owned()).collect()
    }

    fn dir(name: &str) -> PathBuf {
        scratch(name).join(FILE_NAME)
    }

    #[test]
    fn changes_are_written_and_read_back() {
        let path = dir("bm-write");
        let (f, note) = BookmarkFile::load(Some(path.clone()), true);
        assert!(note.is_none() && f.current().is_empty());
        // Nothing changed: nothing written.
        assert_eq!(f.write_now(), Ok(None));
        assert!(!path.exists());
        assert!(f.change(Op::Add(b("one"))));
        assert!(f.pending());
        assert_eq!(f.write_now(), Ok(Some(None)));
        assert!(!f.pending());
        assert_eq!(
            fs::read_to_string(&path).expect("written"),
            f.current().to_csv()
        );
        assert!(!file::tmp_path(&path).exists());
        // The next write keeps the one before as the backup.
        let first = f.current().to_csv();
        f.change(Op::Add(b("two")));
        f.write_now().expect("written");
        assert_eq!(
            fs::read_to_string(file::backup_path(&path)).expect("backup"),
            first
        );
        // A second process reads what the first wrote.
        let (g, _) = BookmarkFile::load(Some(path.clone()), true);
        assert_eq!(names(&g.current()), ["one", "two"]);
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn two_processes_keep_each_others_changes() {
        let path = dir("bm-merge");
        let (a, _) = BookmarkFile::load(Some(path.clone()), true);
        a.change(Op::Add(b("shared")));
        a.write_now().expect("a");
        let (b_file, _) = BookmarkFile::load(Some(path.clone()), true);
        // A adds and edits; B adds and deletes, and writes first.
        a.change(Op::Add(b("from a")));
        let shared = Id::of(&a.current(), 0);
        let mut edited = b("shared");
        edited.set_search(Some("edited by a"));
        a.change(Op::Edit {
            id: shared.clone(),
            row: edited,
        });
        b_file.change(Op::Add(b("from b")));
        b_file.change(Op::Delete(vec![Target::Row(shared)]));
        b_file.write_now().expect("b");
        assert_eq!(names(&b_file.current()), ["from b"]);
        // A merges: B's bookmark stays, A's is added, and the bookmark B
        // deleted and A edited comes back, at the end.
        a.write_now().expect("a again");
        let disk = Bookmarks::parse(&fs::read(&path).expect("read")).expect("parses");
        assert_eq!(names(&disk), ["from b", "from a", "shared"]);
        assert_eq!(disk.rows()[2].search(), "edited by a");
        assert_eq!(*a.current(), disk);
        // B sees it when it looks again, with nothing of its own pending.
        assert_eq!(b_file.refresh(), (true, None));
        assert_eq!(*b_file.current(), disk);
        assert_eq!(b_file.refresh(), (false, None), "unchanged since");
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_change_waits_while_another_write_merges() {
        let path = dir("bm-pending");
        let (a, _) = BookmarkFile::load(Some(path.clone()), true);
        a.change(Op::Add(b("mine")));
        // Another program writes the file meanwhile.
        fs::write(
            &path,
            Bookmarks::default().with_rows(vec![b("theirs")]).to_csv(),
        )
        .expect("theirs");
        // With a change pending, a refresh leaves the list alone.
        assert_eq!(a.refresh(), (false, None));
        assert_eq!(names(&a.current()), ["mine"]);
        a.write_now().expect("merged");
        assert_eq!(names(&a.current()), ["theirs", "mine"]);
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_its_backup_used() {
        let path = dir("bm-corrupt");
        fs::write(
            file::backup_path(&path),
            Bookmarks::default().with_rows(vec![b("saved")]).to_csv(),
        )
        .expect("backup");
        fs::write(&path, [0u8; 64]).expect("zeroed by a power cut");
        let (f, note) = BookmarkFile::load(Some(path.clone()), true);
        assert_eq!(names(&f.current()), ["saved"]);
        let note = note.expect("a note");
        assert!(note.contains("Bookmarks.corrupt-1.csv"), "{note}");
        assert!(note.contains("its backup was used"), "{note}");
        assert_eq!(
            fs::read(file::corrupt_path(&path, 1)).expect("kept"),
            [0u8; 64]
        );
        f.change(Op::Add(b("new")));
        f.write_now().expect("written");
        let disk = Bookmarks::parse(&fs::read(&path).expect("read")).expect("parses");
        assert_eq!(names(&disk), ["saved", "new"]);
        // Corrupt again before the next write: moved aside too, and the
        // backup (the file before the last write) is merged into.
        fs::write(&path, b"\0garbage").expect("bad");
        f.change(Op::Add(b("newer")));
        let note = f.write_now().expect("written").flatten().expect("a note");
        assert!(note.contains("Bookmarks.corrupt-2.csv"), "{note}");
        let disk = Bookmarks::parse(&fs::read(&path).expect("read")).expect("parses");
        assert_eq!(names(&disk), ["saved", "newer"]);
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_recovered_backup_is_put_back_for_the_next_start_and_other_windows() {
        let path = dir("bm-put-back");
        let precious = Bookmarks::default().with_rows(vec![b("Precious")]).to_csv();
        fs::write(file::backup_path(&path), &precious).expect("backup");
        fs::write(&path, [0u8; 256]).expect("zeroed");
        // Window A recovers, changes nothing and closes.
        let (a, note) = BookmarkFile::load(Some(path.clone()), true);
        assert!(note.is_some());
        assert_eq!(names(&a.current()), ["Precious"]);
        drop(a);
        assert_eq!(fs::read_to_string(&path).expect("put back"), precious);
        // The next start (or window B) reads it, with no note.
        let (b_file, note) = BookmarkFile::load(Some(path.clone()), true);
        assert_eq!(
            (names(&b_file.current()), note),
            (vec!["Precious".into()], None)
        );
        // Two adds later the backup still has it.
        for name in ["one", "two"] {
            b_file.change(Op::Add(b(name)));
            b_file.write_now().expect("written");
        }
        let backup =
            Bookmarks::parse(&fs::read(file::backup_path(&path)).expect("backup")).expect("whole");
        assert_eq!(names(&backup), ["Precious", "one"]);
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn an_empty_or_cut_file_never_replaces_the_backup() {
        let path = dir("bm-empty");
        let precious = Bookmarks::default().with_rows(vec![b("Precious")]).to_csv();
        fs::write(file::backup_path(&path), &precious).expect("backup");
        fs::write(&path, b"").expect("emptied by a sync tool");
        let (f, note) = BookmarkFile::load(Some(path.clone()), true);
        assert_eq!(names(&f.current()), ["Precious"]);
        assert!(note.is_some_and(|n| n.contains("it is empty")));
        // Cut short after a comma while this window runs: moved aside at
        // the next write, which merges into the backup.
        let whole = Bookmarks::default()
            .with_rows(vec![b("Precious"), b("main-mark-6")])
            .to_csv();
        let cut = &whole[..whole.find("\"main-mark-6\",").expect("row") + 14];
        fs::write(&path, cut).expect("cut");
        f.change(Op::Add(b("new")));
        let note = f.write_now().expect("written").flatten();
        assert!(note.is_some_and(|n| n.contains("cut short")));
        let disk = Bookmarks::parse(&fs::read(&path).expect("read")).expect("whole");
        assert_eq!(names(&disk), ["Precious", "new"]);
        assert_eq!(
            fs::read_to_string(file::backup_path(&path)).expect("backup"),
            precious
        );
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn an_unreadable_file_is_not_written_over() {
        let path = dir("bm-unreadable");
        fs::create_dir(&path).expect("a folder in its place");
        let (f, note) = BookmarkFile::load(Some(path.clone()), true);
        assert!(
            note.expect("a note")
                .starts_with("The bookmarks could not be read")
        );
        f.change(Op::Add(b("x")));
        assert!(f.write_now().is_err());
        assert!(path.is_dir());
        assert!(f.pending(), "still waiting");
        fs::remove_dir(&path).expect("gone");
        assert_eq!(f.write_now(), Ok(Some(None)));
        assert_eq!(
            names(&Bookmarks::parse(&fs::read(&path).expect("read")).expect("parses")),
            ["x"]
        );
        // No data folder at all: nothing read, and a write says why.
        let (none, note) = BookmarkFile::load(None, true);
        assert!(note.is_none());
        none.change(Op::Add(b("y")));
        assert!(none.write_now().is_err());
        assert_eq!(none.refresh(), (false, None));
        let _ = fs::remove_dir_all(path.parent().expect("dir"));
    }
}
