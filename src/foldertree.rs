//! The Folders pane's file system work, done on threads of its own so the
//! window never waits on a disk (a sleeping USB drive, a folder of tens of
//! thousands of folders, a mount that does not answer):
//!
//! - the **roots**: the drives goz can search (fixed, removable with a
//!   disk in, RAM disks; not network or optical drives, which gozd does not
//!   index), each named as Explorer names it, with its icon;
//! - a folder's **subfolders**: `FindFirstFileExW` asking for directories
//!   only, in large fetches, hidden and system folders shown as Explorer's
//!   own setting says, sorted as Explorer sorts (`StrCmpLogicalW`); each
//!   one probed, within a time budget, for subfolders of its own, so a
//!   folder without any shows no expand button;
//! - the **icons** of the folders that have their own (a drive, a folder
//!   customized by its `desktop.ini`, which marks it read-only or system);
//!   the rest share the plain folder's.
//!
//! Each job runs on a thread of its own (a job stuck on a dead mount holds
//! up only itself) and posts its answer to the pane's window as `msg`, a
//! `Box<Reply>` in lParam, unless the pane has closed meanwhile ([`Alive`]);
//! a reply posted just before it closed is freed as it closes ([`close`]).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_SYSTEM, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic,
    FindExSearchLimitToDirectories, FindFirstFileExW, FindNextFileW, GetDriveTypeW,
    GetLogicalDrives, GetVolumeInformationW, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetThreadErrorMode};
use windows_sys::Win32::UI::Shell::{
    SHELLSTATEA, SHFILEINFOW, SHGFI_DISPLAYNAME, SHGFI_SMALLICON, SHGFI_SYSICONINDEX,
    SHGetFileInfoW, SHGetSetSettings, SSF_SHOWALLOBJECTS, SSF_SHOWSUPERHIDDEN, StrCmpLogicalW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW, PostMessageW};

use crate::wide::nul;

const BACKSLASH: u16 = b'\\' as u16;

/// `GetDriveTypeW`'s answers (winbase.h) that matter here.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_RAMDISK: u32 = 6;

/// How long a listing probes its folders for subfolders of their own: past
/// it, the rest show an expand button, which goes once one opens empty.
const PROBE_BUDGET: Duration = Duration::from_millis(250);

/// Which folders Explorer shows, as its own setting says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// "Show hidden files, folders and drives".
    pub hidden: bool,
    /// The opposite of "Hide protected operating system files".
    pub protected: bool,
}

impl Shown {
    /// Explorer's setting now.
    pub fn now() -> Shown {
        let mut state = SHELLSTATEA::default();
        // SAFETY: fills the two flags asked for in a plain struct.
        unsafe {
            SHGetSetSettings(&mut state, SSF_SHOWALLOBJECTS | SSF_SHOWSUPERHIDDEN, 0);
        }
        // `fShowAllObjects` is the bit field's first bit, `fShowSuperHidden`
        // its sixteenth (shlobj_core.h's SHELLSTATE).
        Shown {
            hidden: state._bitfield1 & 1 != 0,
            protected: state._bitfield1 & (1 << 15) != 0,
        }
    }

    /// Whether a folder of `attributes` shows: a hidden one only when hidden
    /// items show, and a hidden system one (Explorer's protected operating
    /// system file) only when those show too.
    pub fn shows(self, attributes: u32) -> bool {
        let hidden = attributes & FILE_ATTRIBUTE_HIDDEN != 0;
        let system = attributes & FILE_ATTRIBUTE_SYSTEM != 0;
        match (hidden, system) {
            (false, _) => true,
            (true, true) => self.hidden && self.protected,
            (true, false) => self.hidden,
        }
    }
}

/// A subfolder found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub name: Vec<u16>,
    /// Whether it has subfolders of its own; `None` when the probe's budget
    /// ran out before it.
    pub has_folders: Option<bool>,
    /// Its own icon in the system image list, for a folder that has one.
    pub icon: Option<i32>,
}

/// A drive to show at the top of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// `C:\`.
    pub path: Vec<u16>,
    /// What the tree shows: Explorer's name (`Windows (C:)`), or `C:`.
    pub label: Vec<u16>,
    pub icon: Option<i32>,
    pub has_folders: Option<bool>,
}

/// What a job found.
#[derive(Debug)]
pub enum Reply {
    Roots(Vec<Root>),
    /// The subfolders of the folder of node `node`, asked as request
    /// `request` (an older request's answer is not wanted), or the error
    /// listing it gave.
    Folders {
        node: u32,
        request: u64,
        found: Result<Vec<Folder>, u32>,
    },
}

/// Whether `name` is `.` or `..`.
fn is_dots(name: &[u16]) -> bool {
    matches!(name, [0x2e] | [0x2e, 0x2e])
}

/// The name in a find buffer, without its NUL.
fn name_of(data: &WIN32_FIND_DATAW) -> &[u16] {
    let n = data
        .cFileName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(data.cFileName.len());
    &data.cFileName[..n]
}

/// `folder` joined with `name` (`C:\` and `x` give `C:\x`).
pub fn join(folder: &[u16], name: &[u16]) -> Vec<u16> {
    let mut path = folder.to_vec();
    if path.last() != Some(&BACKSLASH) {
        path.push(BACKSLASH);
    }
    path.extend_from_slice(name);
    path
}

/// Goes through what is directly in `folder`, handing `each` every folder
/// that shows (its name and attributes) until it returns false or `limit`
/// entries (files too) have been read: `Err` with the system's error when
/// it cannot be listed (gone, access denied), else whether it read them all.
fn each_folder(
    folder: &[u16],
    shown: Shown,
    limit: usize,
    mut each: impl FnMut(&[u16], u32) -> bool,
) -> Result<bool, u32> {
    let pattern = nul(&join(folder, &[b'*' as u16]));
    // A long listing reads in large fetches; a probe wants its first few.
    let flags = if limit == usize::MAX {
        FIND_FIRST_EX_LARGE_FETCH
    } else {
        0
    };
    // SAFETY: a NUL-terminated pattern and a plain out-struct; the find
    // handle is closed on every path out.
    unsafe {
        let mut data: WIN32_FIND_DATAW = core::mem::zeroed();
        let find = FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            (&raw mut data).cast(),
            FindExSearchLimitToDirectories,
            core::ptr::null(),
            flags,
        );
        if find == INVALID_HANDLE_VALUE {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }
        let mut read = 0usize;
        let complete = loop {
            let attributes = data.dwFileAttributes;
            let name = name_of(&data);
            // The search's "directories only" is a hint a file system may
            // ignore (NTFS does): the attribute decides.
            if attributes & FILE_ATTRIBUTE_DIRECTORY != 0
                && !is_dots(name)
                && shown.shows(attributes)
                && !each(name, attributes)
            {
                break false;
            }
            read += 1;
            if read >= limit {
                break false;
            }
            if FindNextFileW(find, &mut data) == 0 {
                break true;
            }
        };
        FindClose(find);
        Ok(complete)
    }
}

/// The folders directly in `folder` that show, each with its attributes,
/// unsorted.
fn folders_in(folder: &[u16], shown: Shown) -> Result<Vec<(Vec<u16>, u32)>, u32> {
    let mut out = Vec::new();
    each_folder(folder, shown, usize::MAX, |name, attributes| {
        out.push((name.to_vec(), attributes));
        true
    })?;
    Ok(out)
}

/// How many entries a probe reads at most: a folder of many files and no
/// folders is given up on (it shows an expand button, which goes once it
/// opens empty) rather than read through.
const PROBE_ENTRIES: usize = 2048;

/// Whether `folder` has a folder in it that shows: `None` when the probe
/// gave up before knowing; no for one that cannot be listed.
fn has_folders(folder: &[u16], shown: Shown) -> Option<bool> {
    let mut found = false;
    match each_folder(folder, shown, PROBE_ENTRIES, |_, _| {
        found = true;
        false
    }) {
        Err(_) => Some(false),
        Ok(_) if found => Some(true),
        Ok(true) => Some(false),
        Ok(false) => None,
    }
}

/// Sorts names as Explorer does: `2` before `10`, case aside.
pub fn sort_names<T>(items: &mut [T], name: impl Fn(&T) -> &[u16]) {
    items.sort_by(|a, b| {
        let (a, b) = (nul(name(a)), nul(name(b)));
        // SAFETY: two NUL-terminated strings.
        let c = unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) };
        c.cmp(&0)
    });
}

/// The icon of the item at `path` in the system's small image list.
fn icon_of(path: &[u16], display_name: bool) -> (Option<i32>, Option<Vec<u16>>) {
    let p = nul(path);
    let mut flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON;
    if display_name {
        flags |= SHGFI_DISPLAYNAME;
    }
    // SAFETY: a NUL-terminated path and a plain out-struct.
    unsafe {
        let mut info: SHFILEINFOW = core::mem::zeroed();
        let list = SHGetFileInfoW(
            p.as_ptr(),
            0,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags,
        );
        if list == 0 {
            return (None, None);
        }
        let name = display_name.then(|| {
            let n = info
                .szDisplayName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDisplayName.len());
            info.szDisplayName[..n].to_vec()
        });
        (Some(info.iIcon), name.filter(|n| !n.is_empty()))
    }
}

/// The subfolders of `folder`, sorted, probed for subfolders of their own
/// (while the budget lasts) and with the icons of those customized.
pub fn list(folder: &[u16], shown: Shown) -> Result<Vec<Folder>, u32> {
    let mut found = folders_in(folder, shown)?;
    sort_names(&mut found, |(n, _)| n);
    let start = Instant::now();
    Ok(found
        .into_iter()
        .map(|(name, attributes)| {
            let path = join(folder, &name);
            let has_folders = if start.elapsed() < PROBE_BUDGET {
                has_folders(&path, shown)
            } else {
                None
            };
            // A folder Explorer draws with an icon of its own is marked
            // read-only or system (PathIsSystemFolder): only those are
            // asked for theirs, which means reading their desktop.ini.
            let icon = if attributes & (FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_SYSTEM) != 0 {
                icon_of(&path, false).0
            } else {
                None
            };
            Folder {
                name,
                has_folders,
                icon,
            }
        })
        .collect())
}

/// Whether a drive of this type is one goz may have indexed.
fn searchable_drive(kind: u32) -> bool {
    matches!(kind, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK)
}

/// The drives at the top of the tree, `labels` naming them as Explorer
/// does. A removable drive with no disk in is left out.
pub fn roots(labels: bool, shown: Shown) -> Vec<Root> {
    // SAFETY: takes no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u16 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let path: Vec<u16> = vec![b'A' as u16 + i, b':' as u16, BACKSLASH];
        let p = nul(&path);
        // SAFETY: a NUL-terminated root path.
        let kind = unsafe { GetDriveTypeW(p.as_ptr()) };
        if !searchable_drive(kind) {
            continue;
        }
        if kind == DRIVE_REMOVABLE {
            // SAFETY: asks about the volume with no buffers; fails when no
            // disk is in (without a dialog: the thread's error mode).
            let ready = unsafe {
                GetVolumeInformationW(
                    p.as_ptr(),
                    core::ptr::null_mut(),
                    0,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    0,
                )
            } != 0;
            if !ready {
                continue;
            }
        }
        let (icon, name) = icon_of(&path, labels);
        let plain = path[..2].to_vec();
        let label = if labels { name.unwrap_or(plain) } else { plain };
        let has_folders = has_folders(&path, shown);
        out.push(Root {
            path,
            label,
            icon,
            has_folders,
        });
    }
    out
}

/// `name` as folder names are told apart (NTFS's way: case aside, letter by
/// letter): each letter upper-cased where that gives one letter.
pub fn fold(name: &[u16]) -> String {
    // Most names are ASCII: the same letters, without asking each one's
    // upper case of the Unicode tables.
    if name.iter().all(|&c| c < 0x80) {
        return name
            .iter()
            .map(|&c| char::from((c as u8).to_ascii_uppercase()))
            .collect();
    }
    char::decode_utf16(name.iter().copied())
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .map(|c| {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect()
}

/// Whether two folder names name the same folder.
pub fn same_name(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && fold(a) == fold(b)
}

/// The most subfolders of one folder the Folders pane puts in its tree;
/// an item after them counts the others. A tree view takes its items out
/// again (closing, refreshing) in time that grows faster than their
/// number, and a folder of more than this is not browsed as a tree.
pub const CAP: usize = 1000;

/// Which of `found` (a folder's subfolders, in order) go in the tree: the
/// first `cap`, and past them each one of the names `wanted` (the folders
/// the tree shows under it already, the one it is to show), in their
/// order; and how many are left out.
pub fn capped(found: Vec<Folder>, cap: usize, wanted: &[&[u16]]) -> (Vec<Folder>, usize) {
    if found.len() <= cap {
        return (found, 0);
    }
    let first: std::collections::HashSet<String> =
        found[..cap].iter().map(|f| fold(&f.name)).collect();
    // The wanted names not among the first: few (usually none), each
    // looked for in the rest by its length first.
    let missing: Vec<(usize, String)> = wanted
        .iter()
        .map(|w| (w.len(), fold(w)))
        .filter(|(_, w)| !first.contains(w))
        .collect();
    let total = found.len();
    let mut out = Vec::with_capacity(cap + missing.len());
    for (i, f) in found.into_iter().enumerate() {
        let keep = i < cap
            || (missing.iter().any(|(len, _)| *len == f.name.len()) && {
                let name = fold(&f.name);
                missing.iter().any(|(_, m)| *m == name)
            });
        if keep {
            out.push(f);
        }
    }
    let left_out = total - out.len();
    (out, left_out)
}

/// The name under folder `path` on the way down to the folder `target`;
/// `None` unless `target` is inside `path`.
pub fn next_name<'a>(target: &'a [u16], path: &[u16]) -> Option<&'a [u16]> {
    let k = path.len();
    if k == 0 || target.len() <= k || !same_name(&target[..k], path) {
        return None;
    }
    // A drive's root ends with its backslash already.
    let rest = if path[k - 1] == BACKSLASH {
        &target[k..]
    } else if target[k] == BACKSLASH {
        &target[k + 1..]
    } else {
        return None;
    };
    rest.split(|&c| c == BACKSLASH)
        .next()
        .filter(|name| !name.is_empty())
}

/// One step of turning a folder's listed subfolders into a new listing:
/// keep the old one of this index where it is, or put in the new one of
/// this index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Keep(usize),
    Insert(usize),
}

/// How the subfolders `old` (as listed before, in order) become `new` (as
/// listed now, in order): a step for each of `new`, in its order, and the
/// old ones that are gone. A folder is the same folder whatever the case
/// of its name, so one renamed only in case keeps its place, and what is
/// open under it.
pub fn merge(old: &[&[u16]], new: &[&[u16]]) -> (Vec<Step>, Vec<usize>) {
    let mut by_name: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, name) in old.iter().enumerate().rev() {
        by_name.entry(fold(name)).or_default().push(i);
    }
    let mut kept = vec![false; old.len()];
    let steps = new
        .iter()
        .enumerate()
        .map(
            |(j, name)| match by_name.get_mut(&fold(name)).and_then(Vec::pop) {
                Some(i) => {
                    kept[i] = true;
                    Step::Keep(i)
                }
                None => Step::Insert(j),
            },
        )
        .collect();
    let gone = (0..old.len()).filter(|&i| !kept[i]).collect();
    (steps, gone)
}

/// A job for a thread of its own.
pub enum Job {
    Roots {
        labels: bool,
    },
    Folders {
        node: u32,
        request: u64,
        path: Vec<u16>,
    },
}

/// Whether a pane still wants its threads' replies. A thread holds it while
/// it posts one, so the pane's [`close`] comes either before the post (no
/// reply is posted) or after it (the reply is in the queue, and freed).
pub type Alive = Arc<Mutex<bool>>;

/// A pane's `Alive`, as it opens.
pub fn alive() -> Alive {
    Arc::new(Mutex::new(true))
}

/// Runs `job` on a thread of its own and posts its reply to `hwnd` as `msg`,
/// unless the pane closed meanwhile (`alive` false). The reply is freed
/// again if the post fails.
pub fn spawn(job: Job, hwnd: HWND, msg: u32, alive: Alive) {
    let hwnd = hwnd as usize;
    let spawned = std::thread::Builder::new()
        .name("goz-folders".into())
        .spawn(move || {
            // SAFETY: no dialog for a drive without a disk on this thread;
            // COM for the shell's icons and names, released below.
            let com = unsafe {
                SetThreadErrorMode(SEM_FAILCRITICALERRORS, core::ptr::null_mut());
                CoInitializeEx(
                    core::ptr::null(),
                    (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                )
            };
            let shown = Shown::now();
            let reply = match job {
                Job::Roots { labels } => Reply::Roots(roots(labels, shown)),
                Job::Folders {
                    node,
                    request,
                    path,
                } => Reply::Folders {
                    node,
                    request,
                    found: list(&path, shown),
                },
            };
            if com >= 0 {
                // SAFETY: pairs the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            let open = alive.lock().unwrap_or_else(|e| e.into_inner());
            if !*open {
                return;
            }
            let raw = Box::into_raw(Box::new(reply));
            // SAFETY: posts to the pane's window; if that fails the box is
            // taken back.
            unsafe {
                if PostMessageW(hwnd as HWND, msg, 0, raw as isize) == 0 {
                    drop(Box::from_raw(raw));
                }
            }
            drop(open);
        });
    // Without a thread the folder stays unlisted, as a failed listing
    // would leave it.
    drop(spawned);
}

/// The pane at `hwnd` is closing: its threads post nothing more, and the
/// replies they posted as `msg` are freed (its window will never read
/// them).
pub fn close(alive: &Alive, hwnd: HWND, msg: u32) {
    *alive.lock().unwrap_or_else(|e| e.into_inner()) = false;
    drain(hwnd, msg);
}

/// Frees the replies still queued for `hwnd` as `msg`.
fn drain(hwnd: HWND, msg: u32) {
    // SAFETY: takes this thread's own posted messages off its queue; each
    // lParam is a `Box<Reply>` from `spawn`.
    unsafe {
        let mut m: MSG = core::mem::zeroed();
        while PeekMessageW(&mut m, hwnd, msg, msg, PM_REMOVE) != 0 {
            if m.lParam != 0 {
                drop(Box::from_raw(m.lParam as *mut Reply));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn hidden_and_protected_folders_show_as_explorer_says() {
        let plain = 0x10;
        let hidden = 0x10 | FILE_ATTRIBUTE_HIDDEN;
        let system = 0x10 | FILE_ATTRIBUTE_SYSTEM;
        let protected = hidden | FILE_ATTRIBUTE_SYSTEM;
        let default = Shown {
            hidden: false,
            protected: false,
        };
        assert!(default.shows(plain) && default.shows(system));
        assert!(!default.shows(hidden) && !default.shows(protected));
        let hidden_on = Shown {
            hidden: true,
            protected: false,
        };
        assert!(hidden_on.shows(hidden) && !hidden_on.shows(protected));
        let all = Shown {
            hidden: true,
            protected: true,
        };
        assert!(all.shows(protected));
        // Protected files showing without hidden ones: Explorer still
        // hides them (they are hidden too).
        let odd = Shown {
            hidden: false,
            protected: true,
        };
        assert!(!odd.shows(protected));
    }

    #[test]
    fn folders_past_the_cap_are_counted_unless_wanted() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let found: Vec<Folder> = (0..10)
            .map(|i| Folder {
                name: w(&format!("f{i}")),
                has_folders: None,
                icon: None,
            })
            .collect();
        let names = |v: &[Folder]| {
            v.iter()
                .map(|f| String::from_utf16_lossy(&f.name))
                .collect::<Vec<_>>()
        };
        // Under the cap: all of them.
        let (all, left) = capped(found.clone(), 10, &[]);
        assert_eq!((all.len(), left), (10, 0));
        // Over it: the first, and a wanted one past them (any case), in
        // their order; one wanted among the first changes nothing.
        let (some, left) = capped(found.clone(), 3, &[&w("F7"), &w("f1"), &w("gone")]);
        assert_eq!(names(&some), ["f0", "f1", "f2", "f7"]);
        assert_eq!(left, 6);
        let (some, left) = capped(found, 3, &[]);
        assert_eq!((some.len(), left), (3, 7));
    }

    #[test]
    fn the_name_on_the_way_down() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let t = w(r"C:\Windows\WinSxS\amd64_x");
        assert_eq!(next_name(&t, &w(r"C:\")), Some(&w("Windows")[..]));
        assert_eq!(next_name(&t, &w(r"c:\windows")), Some(&w("WinSxS")[..]));
        assert_eq!(
            next_name(&t, &w(r"C:\Windows\WinSxS")),
            Some(&w("amd64_x")[..])
        );
        assert_eq!(next_name(&t, &t), None);
        assert_eq!(next_name(&t, &w(r"C:\Win")), None);
        assert_eq!(next_name(&t, &w(r"D:\")), None);
        // Folded names match whatever the case; non-ASCII too.
        assert!(same_name(&w("WinSxS"), &w("winsxs")));
        assert!(same_name(&w("Ärger"), &w("äRGER")));
        assert_eq!(fold(&w("abc-Z")), "ABC-Z");
    }

    #[test]
    fn names_sort_as_explorer_sorts_them() {
        let mut names = vec![w("b10"), w("B2"), w("a"), w("b1"), w("Ä")];
        sort_names(&mut names, |n| n);
        let text: Vec<String> = names.iter().map(|n| String::from_utf16_lossy(n)).collect();
        assert_eq!(text, ["a", "Ä", "b1", "B2", "b10"]);
    }

    #[test]
    fn paths_join_with_one_backslash() {
        assert_eq!(join(&w(r"C:\"), &w("x")), w(r"C:\x"));
        assert_eq!(join(&w(r"C:\a"), &w("b")), w(r"C:\a\b"));
        assert!(is_dots(&w(".")) && is_dots(&w("..")) && !is_dots(&w("...")));
    }

    #[test]
    fn a_folder_lists_its_subfolders_only() {
        let base = crate::store::file::tests::fresh_dir("goz-gui-foldertree", "");
        let _ = std::fs::remove_dir_all(&base);
        for dir in ["b10", "B2", "a\\inner", "empty"] {
            std::fs::create_dir_all(base.join(dir)).expect("make dirs");
        }
        std::fs::write(base.join("file.txt"), b"x").expect("write file");
        std::fs::write(base.join("empty").join("f.txt"), b"x").expect("write file");
        let path: Vec<u16> = base.as_os_str().encode_wide().collect();
        let shown = Shown {
            hidden: false,
            protected: false,
        };
        let found = list(&path, shown).expect("lists");
        let names: Vec<String> = found
            .iter()
            .map(|f| String::from_utf16_lossy(&f.name))
            .collect();
        assert_eq!(names, ["a", "B2", "b10", "empty"]);
        let has: Vec<Option<bool>> = found.iter().map(|f| f.has_folders).collect();
        assert_eq!(has, [Some(true), Some(false), Some(false), Some(false)]);
        // A folder that is not there.
        assert!(list(&join(&path, &w("missing")), shown).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_probe_reads_only_so_far() {
        let base = crate::store::file::tests::fresh_dir("goz-gui-probe", "");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("files")).expect("make dir");
        std::fs::create_dir_all(base.join("deep").join("inner")).expect("make dirs");
        std::fs::create_dir_all(base.join("empty")).expect("make dir");
        for i in 0..12 {
            std::fs::write(base.join("files").join(format!("f{i}.txt")), b"x").expect("write");
        }
        let wide = |p: std::path::PathBuf| -> Vec<u16> { p.as_os_str().encode_wide().collect() };
        let shown = Shown {
            hidden: false,
            protected: false,
        };
        assert_eq!(has_folders(&wide(base.join("deep")), shown), Some(true));
        assert_eq!(has_folders(&wide(base.join("empty")), shown), Some(false));
        assert_eq!(has_folders(&wide(base.join("files")), shown), Some(false));
        assert_eq!(has_folders(&wide(base.join("missing")), shown), Some(false));
        // Files and no folder past the limit: not known.
        let read_all = each_folder(&wide(base.join("files")), shown, 5, |_, _| true);
        assert_eq!(read_all, Ok(false));
        let read_all = each_folder(&wide(base.join("files")), shown, usize::MAX, |_, _| true);
        assert_eq!(read_all, Ok(true));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn names_are_the_same_whatever_their_case() {
        assert!(same_name(&w("Program Files"), &w("PROGRAM FILES")));
        assert!(same_name(&w("Ärger"), &w("äRGER")));
        assert!(!same_name(&w("a"), &w("b")));
        assert!(!same_name(&w("a"), &w("aa")));
        // A letter whose upper case is two letters stays itself.
        assert_eq!(fold(&w("straße")), "STRAßE");
    }

    #[test]
    fn a_new_listing_keeps_what_is_still_there() {
        let names = |v: &[&str]| v.iter().map(|s| w(s)).collect::<Vec<_>>();
        let old = names(&["a", "c", "d"]);
        let new = names(&["A", "b", "d", "e"]);
        let o: Vec<&[u16]> = old.iter().map(Vec::as_slice).collect();
        let n: Vec<&[u16]> = new.iter().map(Vec::as_slice).collect();
        let (steps, gone) = merge(&o, &n);
        assert_eq!(
            steps,
            [
                Step::Keep(0),
                Step::Insert(1),
                Step::Keep(2),
                Step::Insert(3)
            ]
        );
        assert_eq!(gone, [1]);
        // Everything gone, or everything new.
        let (steps, gone) = merge(&o, &[]);
        assert!(steps.is_empty());
        assert_eq!(gone, [0, 1, 2]);
        let (steps, gone) = merge(&[], &n);
        assert_eq!(steps.len(), 4);
        assert!(gone.is_empty());
        // Two old names alike are matched one each, in order.
        let old = names(&["x", "X"]);
        let o: Vec<&[u16]> = old.iter().map(Vec::as_slice).collect();
        let new = names(&["x"]);
        let n: Vec<&[u16]> = new.iter().map(Vec::as_slice).collect();
        assert_eq!(merge(&o, &n), (vec![Step::Keep(0)], vec![1]));
    }

    #[test]
    fn only_drives_goz_can_index_are_roots() {
        // Unknown, no root, network and optical drives.
        let (unknown, no_root, remote, cdrom) = (0, 1, 4, 5);
        assert!(searchable_drive(DRIVE_FIXED));
        assert!(searchable_drive(DRIVE_REMOVABLE));
        assert!(searchable_drive(DRIVE_RAMDISK));
        for k in [unknown, no_root, remote, cdrom] {
            assert!(!searchable_drive(k));
        }
    }
}
