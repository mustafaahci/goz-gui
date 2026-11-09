//! Reading and writing goz-gui's files so that nothing the user made is
//! ever lost:
//!
//! - a write goes to `name.<pid>.tmp` first (this process's own), is
//!   flushed to the disk, and then replaces the file in one rename
//!   (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING |
//!   MOVEFILE_WRITE_THROUGH`), so a crash or a power cut leaves the old file
//!   or the new one, never half of one;
//! - the file it replaces is kept as `name.backup.ext` first (Everything's
//!   `ini_backup` / `csv_backup`), written from the bytes the same
//!   read-merge-write read and parsed as whole: never a copy of whatever is
//!   on the disk (an empty or cut-short file would take the good backup's
//!   place), and never with the file's attributes (a hidden or read-only
//!   backup would refuse every later write);
//! - a file that cannot be parsed, or that is empty or cut short beside a
//!   good backup, is moved aside as `name.corrupt-<n>.ext`, and the backup
//!   is read instead and put back in its place at once (so another window,
//!   or the next start, finds it there); it is never written over;
//! - while one goz-gui reads, merges and writes a file, the others wait
//!   (a named mutex per file), so two windows never interleave; a write
//!   that cannot get the file within [`WRITE_WAIT`] is not made unlocked
//!   but tried again later.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    SetFileAttributesW,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// How long a write waits for another goz-gui writing the same file. A
/// write is milliseconds; one held longer is a goz-gui hung or suspended.
/// Shorter than the exit's wait for the store thread (2 s), with room for
/// the other files' writes, so a write behind a busy file gives up before
/// the process ends rather than be cut off half done (a hung goz-gui holds
/// one file at most).
pub const WRITE_WAIT: Duration = Duration::from_millis(1500);

/// `path` with `.<tag>` put before its extension: `goz-gui.ini` and
/// `backup` make `goz-gui.backup.ini`; `Search History.csv` and
/// `corrupt-2` make `Search History.corrupt-2.csv`.
fn tagged(path: &Path, tag: &str) -> PathBuf {
    let stem = path.file_stem().map(OsString::from).unwrap_or_default();
    let mut name = stem;
    name.push(".");
    name.push(tag);
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    path.with_file_name(name)
}

/// Where the file before the last write is kept.
pub fn backup_path(path: &Path) -> PathBuf {
    tagged(path, "backup")
}

/// Where the `n`th file found unreadable is moved.
pub fn corrupt_path(path: &Path, n: u32) -> PathBuf {
    tagged(path, &format!("corrupt-{n}"))
}

/// Where this process's write goes before it replaces the file:
/// `name.<pid>.tmp`, so two writers (two sessions of one user sharing a
/// portable folder, which the per-session lock does not cover) never write
/// or delete each other's.
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The file's name, for the user.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Renames `from` over `to` in one step, through to the disk.
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (wide(from), wide(to));
    // SAFETY: two NUL-terminated paths that live across the call.
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Writes `bytes` to this process's tmp file for `path`, flushed to the
/// disk; the tmp file is removed again on failure.
fn write_tmp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let tmp = tmp_path(path);
    let result = File::create(&tmp).and_then(|mut file| {
        file.write_all(bytes)?;
        // FlushFileBuffers: the bytes are on the disk before the rename
        // makes them the file.
        file.sync_all()
    });
    match result {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Puts `bytes` in `path` in one step (tmp, flush, rename), without a
/// backup. A hidden or read-only file there is replaced all the same when
/// `force`.
fn put(path: &Path, bytes: &[u8], force: bool) -> io::Result<()> {
    let tmp = write_tmp(path, bytes)?;
    if force {
        // A rename over a read-only file is refused; hidden is cleared too,
        // so the file never keeps an attribute the user did not give it.
        // SAFETY: a NUL-terminated path; a file not there just fails.
        unsafe { SetFileAttributesW(wide(path).as_ptr(), FILE_ATTRIBUTE_NORMAL) };
    }
    let result = replace(&tmp, path);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Whether a file's bytes hold nothing: empty, or only a byte order mark
/// and white space. No file goz-gui or Everything writes is so.
pub fn blank(bytes: &[u8]) -> bool {
    let rest = bytes
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .or_else(|| bytes.strip_prefix(&[0xff, 0xfe]))
        .unwrap_or(bytes);
    rest.iter().all(|&b| b == 0 || b.is_ascii_whitespace())
}

/// Writes `bytes` to `path` atomically, creating its folder if needed.
/// `previous`: the file there now, as this read-merge-write read it and
/// parsed it as whole; it becomes the backup (`None`: the backup stays as
/// it is, as when the file was missing, damaged, or backups are off).
///
/// A backup that cannot be updated does not stop the write: `Ok(Some)`
/// says why, for the user.
pub fn write(path: &Path, bytes: &[u8], previous: Option<&[u8]>) -> io::Result<Option<String>> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = write_tmp(path, bytes)?;
    // An empty file is never a backup worth having over another.
    let note = previous
        .filter(|b| !blank(b))
        .and_then(|b| put(&backup_path(path), b, true).err())
        .map(|e| format!("{} could not be updated: {e}", name_of(&backup_path(path))));
    let result = replace(&tmp, path);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|()| note)
}

/// A file's size and time, to notice another program changing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub modified: SystemTime,
    pub len: u64,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok()?,
        len: meta.len(),
    })
}

/// What reading a file found.
#[derive(Debug)]
pub enum Loaded<T> {
    /// No file: nothing saved yet (or the user deleted it to start over).
    Missing,
    /// The file, as read, and its bytes: the backup the next write of it
    /// keeps. (A caller that wants to notice later changes takes its
    /// [stamp] after reading.)
    Read(T, Vec<u8>),
    /// The file could not be parsed (or was empty or cut short beside a
    /// good backup). It was moved to `moved_to`, and the backup was read
    /// instead if there was a good one, and put back in the file's place.
    /// `damaged`: the bytes moved aside, for a caller that can still take
    /// rows from them.
    Recovered {
        backup: Option<T>,
        moved_to: PathBuf,
        why: String,
        damaged: Vec<u8>,
    },
    /// The file is there but could not be read (a folder in its place,
    /// access denied): it must not be written over either.
    Unreadable(String),
}

/// Reads `path` with `parse`. A file that does not parse is moved aside
/// as the first free `name.corrupt-<n>.ext` and the backup is tried.
pub fn load<T>(path: &Path, parse: impl Fn(&[u8]) -> Result<T, String>) -> Loaded<T> {
    load_checked(path, parse, |_| None)
}

/// [`load`], with `suspect` saying why a file that parses still looks
/// damaged (a CSV cut short). Such a file, or an empty one, is taken as
/// damaged only when the backup is good: goz-gui never writes one, so
/// something else cut it, and the backup is the last whole file. With no
/// good backup it is read as it is.
pub fn load_checked<T>(
    path: &Path,
    parse: impl Fn(&[u8]) -> Result<T, String>,
    suspect: impl Fn(&[u8]) -> Option<String>,
) -> Loaded<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Unreadable(e.to_string()),
    };
    let good_backup = || -> Option<(T, Vec<u8>)> {
        let bytes = fs::read(backup_path(path)).ok()?;
        if blank(&bytes) {
            return None;
        }
        let value = parse(&bytes).ok()?;
        Some((value, bytes))
    };
    match parse(&bytes) {
        Ok(value) => {
            let damage = if blank(&bytes) {
                Some("it is empty".to_owned())
            } else {
                suspect(&bytes)
            };
            let backup = damage.as_ref().and_then(|_| good_backup());
            match (damage, backup) {
                (Some(why), Some(backup)) => recover(path, why, Some(backup), bytes),
                _ => Loaded::Read(value, bytes),
            }
        }
        Err(why) => recover(path, why, good_backup(), bytes),
    }
}

/// Moves the damaged file aside and puts the good backup (if any) back in
/// its place.
fn recover<T>(
    path: &Path,
    mut why: String,
    backup: Option<(T, Vec<u8>)>,
    damaged: Vec<u8>,
) -> Loaded<T> {
    let moved_to = match move_aside(path) {
        Ok(to) => to,
        // Not moved: leave it be and do not write over it.
        Err(e) => return Loaded::Unreadable(format!("{why}, and it could not be moved: {e}")),
    };
    let backup = backup.map(|(value, bytes)| {
        // At once, so the file is never missing: another goz-gui, or the
        // next start, reads the backup's settings rather than none (and a
        // later write never takes an empty start for the whole file).
        if let Err(e) = put(path, &bytes, false) {
            why.push_str(&format!("; its backup could not be put in its place: {e}"));
        }
        value
    });
    Loaded::Recovered {
        backup,
        moved_to,
        why,
        damaged,
    }
}

/// Moves `path` to the first `name.corrupt-<n>.ext` not taken.
fn move_aside(path: &Path) -> io::Result<PathBuf> {
    for n in 1..10_000 {
        let to = corrupt_path(path, n);
        if to.exists() {
            continue;
        }
        let (from, dest) = (wide(path), wide(&to));
        // SAFETY: two NUL-terminated paths; no flags, so a name taken
        // meanwhile fails rather than being replaced.
        if unsafe { MoveFileExW(from.as_ptr(), dest.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
            return Ok(to);
        }
        let e = io::Error::last_os_error();
        if !to.exists() {
            return Err(e);
        }
    }
    Err(io::Error::other("no free name to move it to"))
}

/// One goz-gui at a time reads, merges and writes a file: a named mutex
/// per file, held by whoever is doing it. A process killed while holding
/// it releases it (the next one gets WAIT_ABANDONED, which still owns it).
pub struct Lock(HANDLE);

/// A stable 64-bit FNV-1a hash, for the mutex's name.
fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The mutex's name for `path`: the same for every spelling's case.
fn lock_name(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let key = full.to_string_lossy().to_lowercase();
    format!("Local\\goz-gui-file-{:016x}", fnv(&key))
}

/// What waiting for a file's lock came to.
enum Acquired {
    Held(Lock),
    /// Another goz-gui held it all the time.
    Busy,
    /// No mutex could be made or opened (one made by a goz-gui running as
    /// administrator, which this one may not open).
    NoMutex,
}

fn acquire(path: &Path, timeout: Duration) -> Acquired {
    let name: Vec<u16> = lock_name(path).encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated name; the handle is closed by Drop, or here.
    unsafe {
        let h = CreateMutexW(core::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            return Acquired::NoMutex;
        }
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        match WaitForSingleObject(h, ms) {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Acquired::Held(Lock(h)),
            _ => {
                CloseHandle(h);
                Acquired::Busy
            }
        }
    }
}

impl Lock {
    /// For a read: waits up to `timeout` for the file's lock. `None` when
    /// another goz-gui held it all that time (or no mutex could be made);
    /// the caller then reads without it (a write is one rename, so the file
    /// read is whole either way).
    pub fn acquire(path: &Path, timeout: Duration) -> Option<Lock> {
        match acquire(path, timeout) {
            Acquired::Held(lock) => Some(lock),
            Acquired::Busy | Acquired::NoMutex => None,
        }
    }

    /// For a read-merge-write: waits up to `timeout` for the file's lock.
    /// `Err` (for the user) when another goz-gui held it all that time: the
    /// write is not made over its, but left staged for a later try.
    /// `Ok(None)` when no mutex could be made: there is nobody to wait for
    /// that this process can see, so the write goes ahead.
    pub fn for_write(path: &Path, timeout: Duration) -> Result<Option<Lock>, String> {
        match acquire(path, timeout) {
            Acquired::Held(lock) => Ok(Some(lock)),
            Acquired::NoMutex => Ok(None),
            Acquired::Busy => Err(format!(
                "{} was not saved yet: another goz-gui has been writing it for over {} s \
                 (goz-gui tries again)",
                name_of(path),
                timeout.as_secs_f32()
            )),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: a mutex this thread owns, and its handle.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, GetFileAttributesW,
    };

    /// A fresh folder for one test.
    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("goz-gui-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    fn attributes(path: &Path) -> u32 {
        // SAFETY: a NUL-terminated path.
        unsafe { GetFileAttributesW(wide(path).as_ptr()) }
    }

    fn set_attributes(path: &Path, attributes: u32) {
        // SAFETY: a NUL-terminated path.
        assert_ne!(
            unsafe { SetFileAttributesW(wide(path).as_ptr(), attributes) },
            0
        );
    }

    #[test]
    fn names_beside_the_file() {
        let ini = Path::new(r"C:\d\goz-gui.ini");
        assert_eq!(backup_path(ini), Path::new(r"C:\d\goz-gui.backup.ini"));
        assert_eq!(
            corrupt_path(ini, 3),
            Path::new(r"C:\d\goz-gui.corrupt-3.ini")
        );
        assert_eq!(
            tmp_path(ini),
            PathBuf::from(format!(r"C:\d\goz-gui.ini.{}.tmp", std::process::id()))
        );
        let csv = Path::new(r"C:\d\Search History.csv");
        assert_eq!(
            backup_path(csv),
            Path::new(r"C:\d\Search History.backup.csv")
        );
        assert_eq!(backup_path(Path::new("noext")), Path::new("noext.backup"));
    }

    #[test]
    fn writes_replace_whole_files_and_keep_the_one_read_before() {
        let dir = scratch("write");
        let path = dir.join("sub").join("goz-gui.ini");
        assert_eq!(write(&path, b"one", None).expect("first write"), None);
        assert_eq!(fs::read(&path).expect("read"), b"one");
        // Nothing to back up the first time.
        assert!(!backup_path(&path).exists());
        write(&path, b"two", Some(b"one")).expect("second write");
        assert_eq!(fs::read(&path).expect("read"), b"two");
        assert_eq!(fs::read(backup_path(&path)).expect("backup"), b"one");
        // No backup asked for: the one there stays.
        write(&path, b"three", None).expect("no backup");
        assert_eq!(fs::read(backup_path(&path)).expect("backup"), b"one");
        // An empty "previous" never replaces a backup.
        write(&path, b"four", Some(b"")).expect("written");
        assert_eq!(fs::read(backup_path(&path)).expect("backup"), b"one");
        assert!(!tmp_path(&path).exists());
        assert!(!tmp_path(&backup_path(&path)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hidden_or_read_only_backup_never_blocks_a_write() {
        let dir = scratch("attributes");
        let path = dir.join("Search History.csv");
        fs::write(&path, b"one").expect("file");
        fs::write(backup_path(&path), b"old").expect("backup");
        set_attributes(
            &backup_path(&path),
            FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY,
        );
        // A hidden file: its backup is not hidden.
        set_attributes(&path, FILE_ATTRIBUTE_HIDDEN);
        assert_eq!(write(&path, b"two", Some(b"one")).expect("written"), None);
        assert_eq!(fs::read(&path).expect("read"), b"two");
        assert_eq!(fs::read(backup_path(&path)).expect("backup"), b"one");
        let a = attributes(&backup_path(&path));
        assert_eq!(a & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY), 0);
        write(&path, b"three", Some(b"two")).expect("again");
        assert_eq!(fs::read(backup_path(&path)).expect("backup"), b"two");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backup_that_cannot_be_updated_does_not_stop_the_write() {
        let dir = scratch("backup-fails");
        let path = dir.join("goz-gui.ini");
        fs::write(&path, b"one").expect("file");
        // A folder where the backup goes.
        fs::create_dir(backup_path(&path)).expect("folder");
        let note = write(&path, b"two", Some(b"one")).expect("written");
        assert!(
            note.as_deref()
                .is_some_and(|n| n.starts_with("goz-gui.backup.ini could not be updated")),
            "{note:?}"
        );
        assert_eq!(fs::read(&path).expect("read"), b"two");
        let _ = fs::remove_dir_all(&dir);
    }

    fn parse_digits(bytes: &[u8]) -> Result<String, String> {
        if bytes.iter().all(u8::is_ascii_digit) {
            Ok(String::from_utf8_lossy(bytes).into_owned())
        } else {
            Err("not digits".into())
        }
    }

    #[test]
    fn a_bad_file_is_moved_aside_and_the_backup_put_back() {
        let dir = scratch("corrupt");
        let path = dir.join("f.ini");
        assert!(matches!(load(&path, parse_digits), Loaded::Missing));
        fs::write(&path, b"123").expect("good");
        assert!(
            matches!(load(&path, parse_digits), Loaded::Read(ref v, ref b) if v == "123" && b == b"123")
        );
        fs::write(backup_path(&path), b"42").expect("backup");
        fs::write(&path, b"\0\0garbage").expect("bad");
        match load(&path, parse_digits) {
            Loaded::Recovered {
                backup,
                moved_to,
                why,
                damaged,
            } => {
                assert_eq!(backup.as_deref(), Some("42"));
                assert_eq!(moved_to, corrupt_path(&path, 1));
                assert_eq!(why, "not digits");
                assert_eq!(damaged, b"\0\0garbage");
            }
            other => panic!("{other:?}"),
        }
        // The backup is the file again: the next read (another window, the
        // next start) finds it, not a missing file.
        assert_eq!(fs::read(&path).expect("put back"), b"42");
        assert!(matches!(load(&path, parse_digits), Loaded::Read(ref v, _) if v == "42"));
        assert_eq!(
            fs::read(corrupt_path(&path, 1)).expect("kept"),
            b"\0\0garbage"
        );
        // The next bad one gets the next number; no backup gives none, and
        // the file stays missing.
        fs::write(&path, b"x").expect("bad again");
        fs::remove_file(backup_path(&path)).expect("no backup");
        match load(&path, parse_digits) {
            Loaded::Recovered {
                backup, moved_to, ..
            } => {
                assert_eq!(backup, None);
                assert_eq!(moved_to, corrupt_path(&path, 2));
            }
            other => panic!("{other:?}"),
        }
        assert!(!path.exists());
        // A folder in the file's place cannot be read, and is left alone.
        fs::create_dir(&path).expect("folder");
        assert!(matches!(load(&path, parse_digits), Loaded::Unreadable(_)));
        assert!(path.is_dir());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_or_cut_file_beside_a_good_backup_is_damaged() {
        let dir = scratch("empty");
        let path = dir.join("f.ini");
        let cut = |b: &[u8]| (b.len() == 1).then(|| "cut short".to_owned());
        // Empty with no backup: read as empty (nothing saved yet).
        fs::write(&path, b"").expect("empty");
        assert!(matches!(load(&path, parse_digits), Loaded::Read(ref v, _) if v.is_empty()));
        // Empty (or blank) beside a good backup: the backup, put back.
        fs::write(backup_path(&path), b"42").expect("backup");
        for empty in [&b""[..], b"\xef\xbb\xbf \r\n"] {
            fs::write(&path, empty).expect("empty");
            let parse =
                |b: &[u8]| parse_digits(b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(b).trim_ascii());
            match load(&path, parse) {
                Loaded::Recovered { backup, why, .. } => {
                    assert_eq!(backup.as_deref(), Some("42"));
                    assert_eq!(why, "it is empty");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(fs::read(&path).expect("put back"), b"42");
        }
        // Suspect: damaged beside a good backup only.
        fs::write(&path, b"7").expect("cut");
        assert!(matches!(
            load_checked(&path, parse_digits, cut),
            Loaded::Recovered { ref why, .. } if why == "cut short"
        ));
        fs::write(&path, b"7").expect("cut");
        fs::write(backup_path(&path), b"").expect("an empty backup is none");
        assert!(
            matches!(load_checked(&path, parse_digits, cut), Loaded::Read(ref v, _) if v == "7")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_is_one_holder_at_a_time() {
        let dir = scratch("lock");
        let path = dir.join("goz-gui.ini");
        let held = Lock::acquire(&path, Duration::from_millis(100)).expect("free");
        let other = path.clone();
        let waited =
            std::thread::spawn(move || Lock::acquire(&other, Duration::from_millis(50)).is_some())
                .join()
                .expect("thread");
        assert!(!waited, "taken while held");
        // A write does not go ahead without it: it says so.
        let other = path.clone();
        let busy = std::thread::spawn(move || {
            Lock::for_write(&other, Duration::from_millis(50)).map(|l| l.is_some())
        })
        .join()
        .expect("thread");
        assert!(
            busy.as_ref()
                .is_err_and(|why| why.starts_with("goz-gui.ini was not saved yet")),
            "{busy:?}"
        );
        drop(held);
        let other = path.clone();
        let got = std::thread::spawn(move || {
            Lock::for_write(&other, Duration::from_millis(500)).is_ok_and(|l| l.is_some())
        })
        .join()
        .expect("thread");
        assert!(got, "free once released");
        // Spelt in another case, it is the same file.
        assert_eq!(
            lock_name(Path::new(r"C:\A\goz-gui.INI")),
            lock_name(Path::new(r"c:\a\GOZ-GUI.ini"))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
