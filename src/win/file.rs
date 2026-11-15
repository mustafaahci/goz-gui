//! Files and the file system: attributes, times, names, listings and
//! drives. Paths go in as UTF-16 (a file name need not be valid Unicode),
//! with or without their NUL. A handle opened here (a file's, a listing's)
//! is owned, and closed when its owner drops.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DeleteFileW, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_MODE, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic,
    FindExSearchLimitToDirectories, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    GetDriveTypeW, GetFileAttributesExW, GetFileAttributesW, GetFileExInfoStandard, GetFileTime,
    GetLogicalDrives, GetLongPathNameW, GetVolumeInformationW, INVALID_FILE_ATTRIBUTES,
    MOVE_FILE_FLAGS, MoveFileExW, SetFileAttributesW, SetFileTime, WIN32_FILE_ATTRIBUTE_DATA,
    WIN32_FIND_DATAW,
};
use windows_sys::Win32::UI::Shell::PathCreateFromUrlW;

use super::{len_u32, nul_ended};

/// The attributes of the file or folder at `path` (GetFileAttributesW,
/// FILE_ATTRIBUTE_*); the system's error code when there is nothing there
/// or they cannot be read.
pub fn attributes(path: &[u16]) -> Result<u32, u32> {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path, read during the call.
    let a = unsafe { GetFileAttributesW(path.as_ptr()) };
    if a == INVALID_FILE_ATTRIBUTES {
        Err(super::sys::last_error())
    } else {
        Ok(a)
    }
}

/// Sets the attributes of the file at `path`; false if it could not.
pub fn set_attributes(path: &[u16], attributes: u32) -> bool {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path.
    unsafe { SetFileAttributesW(path.as_ptr(), attributes) != 0 }
}

/// The attributes, size and times of the file at `path`
/// (GetFileAttributesExW: no handle is opened); `None` when they cannot be
/// read.
pub fn attribute_data(path: &[u16]) -> Option<WIN32_FILE_ATTRIBUTE_DATA> {
    let path = nul_ended(path);
    let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: a NUL-terminated path; GetFileExInfoStandard fills a
    // WIN32_FILE_ATTRIBUTE_DATA, which `data` is.
    let ok = unsafe {
        GetFileAttributesExW(path.as_ptr(), GetFileExInfoStandard, (&raw mut data).cast())
    } != 0;
    ok.then_some(data)
}

/// Deletes the file at `path`; false if it could not (the reason is the
/// thread's last error).
pub fn delete(path: &[u16]) -> bool {
    let path = nul_ended(path);
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.as_ptr()) != 0 }
}

/// Moves (renames) `from` to `to` as MoveFileExW does with `flags`.
pub fn move_file(from: &[u16], to: &[u16], flags: MOVE_FILE_FLAGS) -> io::Result<()> {
    let (from, to) = (nul_ended(from), nul_ended(to));
    // SAFETY: two NUL-terminated paths.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// GetLongPathNameW: `path` with its 8.3 short names expanded, into `buf`;
/// the units written (without the NUL), or the units needed (with it) when
/// `buf` is too short, 0 on failure.
pub fn long_path_name(path: &[u16], buf: &mut [u16]) -> u32 {
    let path = nul_ended(path);
    let out = if buf.is_empty() {
        core::ptr::null_mut()
    } else {
        buf.as_mut_ptr()
    };
    // SAFETY: a NUL-terminated path, read; a separate buffer of the length
    // given, written (null and 0 ask for the length).
    unsafe { GetLongPathNameW(path.as_ptr(), out, len_u32(buf.len())) }
}

/// The path a `file:` URL names (`file:///C:/a%20b/x.ico` is
/// `C:\a b\x.ico`), up to 1023 units; `None` for anything else.
pub fn path_from_url(url: &[u16]) -> Option<Vec<u16>> {
    /// The longest path given, in UTF-16 units.
    const MAX: usize = 1023;
    let url = nul_ended(url);
    let mut out = vec![0u16; MAX + 1];
    let mut len = out.len() as u32;
    // SAFETY: a NUL-terminated URL; the buffer with its length in units,
    // which the call sets to the path's (without its NUL).
    let status = unsafe { PathCreateFromUrlW(url.as_ptr(), out.as_mut_ptr(), &mut len, 0) };
    if status < 0 || len == 0 || len as usize >= out.len() {
        return None;
    }
    out.truncate(len as usize);
    Some(out)
}

/// A file or folder opened with CreateFileW, closed when this drops; only
/// made here, so the handle is always one this owns.
pub struct Handle(HANDLE);

impl Handle {
    /// CreateFileW of `path` for `access` (FILE_READ_ATTRIBUTES...), shared
    /// as `share` says, opened or made as `disposition` says
    /// (OPEN_EXISTING...) with `flags` (FILE_FLAG_BACKUP_SEMANTICS for a
    /// folder...); no security attributes or template. The error is the
    /// system's.
    pub fn open(
        path: &[u16],
        access: u32,
        share: FILE_SHARE_MODE,
        disposition: FILE_CREATION_DISPOSITION,
        flags: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> io::Result<Handle> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated path; no security attributes, no
        // template; the handle is owned by what this returns.
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                share,
                core::ptr::null(),
                disposition,
                flags,
                core::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(h))
    }

    /// The file's times: created, last accessed, last written (UTC).
    pub fn times(&self) -> Option<[FILETIME; 3]> {
        let mut t = [FILETIME::default(); 3];
        let [c, a, w] = &mut t;
        // SAFETY: the handle this owns; three out-structs.
        let ok = unsafe { GetFileTime(self.0, c, a, w) } != 0;
        ok.then_some(t)
    }

    /// Sets the file's times (`None`: leave it as it is); false if it
    /// could not (the reason is the thread's last error).
    pub fn set_times(
        &self,
        created: Option<&FILETIME>,
        accessed: Option<&FILETIME>,
        written: Option<&FILETIME>,
    ) -> bool {
        let ptr = |f: Option<&FILETIME>| f.map_or(core::ptr::null(), |f| f as *const FILETIME);
        // SAFETY: the handle this owns; each time a FILETIME read during the
        // call, or null for "leave it".
        unsafe { SetFileTime(self.0, ptr(created), ptr(accessed), ptr(written)) != 0 }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle this owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// What a [`Find`] lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    /// Everything the pattern names.
    Names,
    /// Folders, asked for (a file system may give files too: their
    /// attributes say).
    Folders,
}

/// A folder's listing (FindFirstFileExW, basic information), closed when
/// this drops.
pub struct Find(HANDLE);

impl Find {
    /// Starts listing what `pattern` names (`C:\folder\*`) as `search`
    /// says, in large fetches with `large_fetch`: the listing and its
    /// first entry. `Err` with the system's error when nothing can be
    /// listed.
    pub fn first(
        pattern: &[u16],
        search: Search,
        large_fetch: bool,
    ) -> Result<(Find, WIN32_FIND_DATAW), u32> {
        let pattern = nul_ended(pattern);
        let mut data = WIN32_FIND_DATAW::default();
        let op = match search {
            Search::Names => FindExSearchNameMatch,
            Search::Folders => FindExSearchLimitToDirectories,
        };
        let flags = if large_fetch {
            FIND_FIRST_EX_LARGE_FETCH
        } else {
            0
        };
        // SAFETY: a NUL-terminated pattern; FindExInfoBasic fills a
        // WIN32_FIND_DATAW, which `data` is; neither search takes a
        // filter.
        let h = unsafe {
            FindFirstFileExW(
                pattern.as_ptr(),
                FindExInfoBasic,
                (&raw mut data).cast(),
                op,
                core::ptr::null(),
                flags,
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(super::sys::last_error());
        }
        Ok((Find(h), data))
    }

    /// The listing's next entry; `None` at its end (or on failure).
    pub fn next(&mut self) -> Option<WIN32_FIND_DATAW> {
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: the find handle this owns; an out-struct.
        (unsafe { FindNextFileW(self.0, &mut data) } != 0).then_some(data)
    }
}

impl Drop for Find {
    fn drop(&mut self) {
        // SAFETY: the find handle this owns, closed once.
        unsafe { FindClose(self.0) };
    }
}

/// The drives there are, one bit each from A (GetLogicalDrives).
pub fn logical_drives() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetLogicalDrives() }
}

/// GetDriveTypeW of the drive whose root is `root` (`C:\`).
pub fn drive_type(root: &[u16]) -> u32 {
    let root = nul_ended(root);
    // SAFETY: a NUL-terminated root path.
    unsafe { GetDriveTypeW(root.as_ptr()) }
}

/// Whether the volume at `root` answers (GetVolumeInformationW with no
/// buffers): false for a removable drive with no disk in.
pub fn volume_ready(root: &[u16]) -> bool {
    let root = nul_ended(root);
    // SAFETY: a NUL-terminated root path; no buffers asked for.
    unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
        ) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn a_listing_goes_through_a_folder() {
        let base = std::env::temp_dir().join(format!("goz-gui-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("inner")).expect("make dirs");
        std::fs::write(base.join("file.txt"), b"x").expect("make a file");
        let mut pattern: Vec<u16> = {
            use std::os::windows::ffi::OsStrExt;
            base.as_os_str().encode_wide().collect()
        };
        pattern.extend(w("\\*"));
        for (search, large) in [(Search::Folders, true), (Search::Names, false)] {
            let (mut find, mut data) = Find::first(&pattern, search, large).expect("lists");
            let mut names = Vec::new();
            loop {
                let name = super::super::until_nul(&data.cFileName).to_vec();
                names.push(String::from_utf16_lossy(&name));
                match find.next() {
                    Some(next) => data = next,
                    None => break,
                }
            }
            assert!(names.iter().any(|n| n == "inner"));
            if search == Search::Names {
                assert!(names.iter().any(|n| n == "file.txt"));
            }
        }
        let file = {
            let mut p = pattern.clone();
            p.truncate(p.len() - 1);
            p.extend(w("file.txt"));
            p
        };
        let attrs = attributes(&file).expect("there");
        assert!(set_attributes(&file, attrs | 1));
        assert!(attribute_data(&file).is_some_and(|d| d.dwFileAttributes & 1 != 0));
        assert!(set_attributes(&file, attrs));
        assert!(delete(&file));
        assert!(attributes(&file).is_err());
        let _ = std::fs::remove_dir_all(&base);
        let mut missing = pattern.clone();
        missing.truncate(missing.len() - 1);
        missing.extend(w("nothing-here\\*"));
        assert!(Find::first(&missing, Search::Names, false).is_err());
    }

    #[test]
    fn the_drives_are_there() {
        assert!(logical_drives() != 0);
        // DRIVE_NO_ROOT_DIR for a root that is none.
        assert_eq!(drive_type(&w("no such root")), 1);
    }

    #[test]
    fn a_file_url_is_a_path() {
        let path =
            path_from_url(&w("file:///C:/a%20b/x.ico")).map(|p| String::from_utf16_lossy(&p));
        assert_eq!(path.as_deref(), Some(r"C:\a b\x.ico"));
        assert!(path_from_url(&w("https://example.com/")).is_none());
    }
}
