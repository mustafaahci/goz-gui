//! goz-gui's files: where they live (`location`), how they are read and
//! written without ever losing one (`file`), the INI they share their
//! format with Everything's in (`ini`), and the thread that writes them
//! (`writer`). `IniWrites` merges this process's settings into the file on
//! that thread.

pub mod file;
pub mod ini;
pub mod location;
pub mod writer;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use file::Loaded;
use ini::Ini;

/// `a` and `b` as one note for the user.
pub fn join_notes(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

#[derive(Default)]
struct Staged {
    /// Keys this process changed since they were last written, with their
    /// values now.
    changes: BTreeMap<String, String>,
    /// Every key goz-gui knows with its value here, for a file that lacks
    /// some (a new one, or one an older version wrote).
    all: Vec<(String, String)>,
    backup: bool,
}

/// Writes to the settings file, shared by the window (which stages them)
/// and the store thread (which writes them).
pub struct IniWrites {
    path: PathBuf,
    /// The file as read at startup (or its backup, when it could not be
    /// parsed): what a write starts from when the file is not there, so
    /// keys goz-gui does not know survive a recovery.
    base: Ini,
    staged: Mutex<Staged>,
}

impl IniWrites {
    pub fn new(path: PathBuf, base: Ini) -> IniWrites {
        IniWrites {
            path,
            base,
            staged: Mutex::new(Staged::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Staged> {
        self.staged.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds `changes` to what the next write sets, with `all` (every key
    /// and its value) for the keys the file lacks.
    pub fn stage(&self, changes: Vec<(String, String)>, all: Vec<(String, String)>, backup: bool) {
        let mut staged = self.lock();
        staged.changes.extend(changes);
        staged.all = all;
        staged.backup = backup;
    }

    /// Changes staged and not written yet (a write failed): the next save
    /// writes them even when nothing else changed.
    pub fn pending(&self) -> bool {
        !self.lock().changes.is_empty()
    }

    /// Writes what is staged: the file is read again (another goz-gui may
    /// have changed it), the keys this process changed are set over it, the
    /// keys it lacks are added, and it is written back. Run on the store
    /// thread. On failure the changes stay staged for the next write.
    /// `Ok(Some)`: written, with something to tell the user (the file was
    /// recovered from its backup, or the backup could not be updated).
    pub fn write_now(&self) -> Result<Option<String>, String> {
        let (changes, all, backup) = {
            let mut staged = self.lock();
            (
                core::mem::take(&mut staged.changes),
                staged.all.clone(),
                staged.backup,
            )
        };
        let result = merge_and_write(&self.path, &self.base, &changes, &all, backup);
        if result.is_err() {
            // Changed again meanwhile: the newer value stays.
            let mut staged = self.lock();
            for (key, value) in changes {
                staged.changes.entry(key).or_insert(value);
            }
        }
        result
    }
}

fn merge_and_write(
    path: &Path,
    base: &Ini,
    changes: &BTreeMap<String, String>,
    all: &[(String, String)],
    backup: bool,
) -> Result<Option<String>, String> {
    let _lock = file::Lock::for_write(path, file::WRITE_WAIT)?;
    let mut note = None;
    // `previous`: the file as read, whole: the backup this write keeps.
    let (mut doc, previous) = match file::load(path, Ini::parse) {
        Loaded::Missing => (base.clone(), None),
        Loaded::Read(doc, bytes) => (doc, Some(bytes)),
        Loaded::Recovered {
            backup,
            moved_to,
            why,
            ..
        } => {
            note = Some(format!(
                "The settings file could not be read ({why}); it was kept as {} and {}",
                moved_to
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                if backup.is_some() {
                    "its backup was used"
                } else {
                    "the settings read at startup were used"
                }
            ));
            (backup.unwrap_or_else(|| base.clone()), None)
        }
        Loaded::Unreadable(why) => {
            return Err(format!(
                "{} could not be read, so goz-gui does not write over it: {why}",
                path.display()
            ));
        }
    };
    let before = doc.to_text();
    // The keys the file lacks first, in goz-gui's order (a new file reads
    // as Everything's does), then this process's changes over them.
    doc.fill_missing(all.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    for (key, value) in changes {
        doc.set(key, value);
    }
    let text = doc.to_text();
    if text == before && path.is_file() {
        return Ok(note);
    }
    let kept = file::write(
        path,
        text.as_bytes(),
        previous.as_deref().filter(|_| backup),
    )
    .map_err(|e| format!("The settings could not be saved to {}: {e}", path.display()))?;
    Ok(join_notes(note, kept))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|&(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    #[test]
    fn the_keys_changed_here_win_and_the_others_come_from_the_file() {
        let dir = file::tests::scratch("ini-merge");
        let path = dir.join("goz-gui.ini");
        let all = pairs(&[("theme", "0"), ("ontop", "0"), ("match_case", "0")]);
        // This process: theme changed to 2.
        let mine = IniWrites::new(path.clone(), Ini::default());
        // Another process wrote ontop=1 and an unknown key meanwhile.
        fs::write(&path, "[goz-gui]\r\nontop=1\r\nfuture=x\r\ntheme=1\r\n").expect("theirs");
        mine.stage(pairs(&[("theme", "2")]), all.clone(), true);
        mine.write_now().expect("written");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[goz-gui]\r\nontop=1\r\nfuture=x\r\ntheme=2\r\nmatch_case=0\r\n"
        );
        assert_eq!(
            fs::read_to_string(file::backup_path(&path)).expect("backup"),
            "[goz-gui]\r\nontop=1\r\nfuture=x\r\ntheme=1\r\n"
        );
        // Written keys are not written again: the other process's later
        // theme stays.
        fs::write(&path, "[goz-gui]\r\nontop=1\r\ntheme=1\r\nmatch_case=0\r\n").expect("theirs");
        mine.stage(pairs(&[("match_case", "1")]), all, true);
        mine.write_now().expect("written");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[goz-gui]\r\nontop=1\r\ntheme=1\r\nmatch_case=1\r\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_the_backup_merged_into() {
        let dir = file::tests::scratch("ini-corrupt");
        let path = dir.join("goz-gui.ini");
        fs::write(file::backup_path(&path), "[goz-gui]\r\nontop=2\r\n").expect("backup");
        fs::write(&path, [0u8; 32]).expect("zeroed by a power cut");
        let w = IniWrites::new(path.clone(), Ini::default());
        w.stage(pairs(&[("theme", "1")]), Vec::new(), true);
        w.write_now().expect("written");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[goz-gui]\r\nontop=2\r\ntheme=1\r\n"
        );
        assert_eq!(
            fs::read(file::corrupt_path(&path, 1)).expect("kept"),
            [0u8; 32]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_gone_since_startup_is_written_from_what_was_read() {
        let dir = file::tests::scratch("ini-base");
        let path = dir.join("goz-gui.ini");
        // Read at startup (here: from the backup of a corrupt file, which
        // was moved aside), with a key goz-gui does not know.
        let base = Ini::from_text("[goz-gui]\r\nfuture=kept\r\ntheme=1\r\n");
        let w = IniWrites::new(path.clone(), base);
        w.stage(
            pairs(&[("ontop", "2")]),
            pairs(&[("theme", "0"), ("ontop", "0")]),
            true,
        );
        w.write_now().expect("written");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[goz-gui]\r\nfuture=kept\r\ntheme=1\r\nontop=2\r\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_file_is_not_written_over_and_the_changes_wait() {
        let dir = file::tests::scratch("ini-unreadable");
        let path = dir.join("goz-gui.ini");
        fs::create_dir(&path).expect("a folder in its place");
        let w = IniWrites::new(path.clone(), Ini::default());
        assert!(!w.pending());
        w.stage(pairs(&[("theme", "1")]), Vec::new(), true);
        assert!(w.pending());
        assert!(w.write_now().is_err());
        assert!(path.is_dir());
        // Still staged: the window's next save (a retry, or exit) writes it
        // though nothing else changed.
        assert!(w.pending());
        fs::remove_dir(&path).expect("gone");
        w.write_now().expect("written");
        assert!(!w.pending());
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[goz-gui]\r\ntheme=1\r\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_held_by_another_goz_gui_is_not_written_unlocked() {
        let dir = file::tests::scratch("ini-busy");
        let path = dir.join("goz-gui.ini");
        let w = IniWrites::new(path.clone(), Ini::default());
        w.stage(pairs(&[("theme", "2")]), Vec::new(), true);
        let held = file::Lock::acquire(&path, std::time::Duration::from_secs(1)).expect("lock");
        let job = std::thread::spawn(move || {
            let result = w.write_now();
            (result, w)
        });
        let (result, w) = job.join().expect("thread");
        assert!(result.is_err_and(|why| why.contains("was not saved yet")));
        assert!(!path.exists() && w.pending());
        drop(held);
        w.write_now().expect("written");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[goz-gui]\r\ntheme=2\r\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_file_beside_a_good_backup_never_replaces_it() {
        let dir = file::tests::scratch("ini-empty");
        let path = dir.join("goz-gui.ini");
        let good = "[goz-gui]\r\ntheme=2\r\nmy_own_key=keep me\r\n";
        fs::write(file::backup_path(&path), good).expect("backup");
        fs::write(&path, "").expect("cut to nothing by a sync tool");
        // Read at startup: the backup, put back.
        let doc = match file::load(&path, Ini::parse) {
            Loaded::Recovered {
                backup: Some(doc),
                why,
                ..
            } => {
                assert_eq!(why, "it is empty");
                doc
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(doc.get("theme"), Some("2"));
        assert_eq!(fs::read_to_string(&path).expect("put back"), good);
        // Two writes later the backup still holds the unknown key, and the
        // file has every key.
        let w = IniWrites::new(path.clone(), doc);
        for theme in ["1", "0"] {
            w.stage(pairs(&[("theme", theme)]), Vec::new(), true);
            w.write_now().expect("written");
        }
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("theme=0") && text.contains("my_own_key=keep me"));
        let backup = fs::read_to_string(file::backup_path(&path)).expect("backup");
        assert!(backup.contains("theme=1") && backup.contains("my_own_key=keep me"));
        let _ = fs::remove_dir_all(&dir);
    }
}
