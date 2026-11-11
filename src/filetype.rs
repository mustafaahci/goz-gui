//! The Extension and Type columns' text.
//!
//! Extension is the name after its last dot. Type is the shell's name for
//! the kind of file ("Text Document", "File folder"), which it answers from
//! the registry by extension alone without touching the file; each answer
//! is kept, so a row costs a hash lookup once its extension has been seen.
//! The window's paints never ask the shell themselves: they take the names
//! known and have the others looked up on a thread (`typeinfo`), whose
//! answers are kept here ([`learn`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW,
};

use crate::client::Row;
use crate::highlight::fold;

/// The text after the name's last dot: empty for a folder, and for a name
/// with no dot or ending in one.
pub fn extension(name: &[u16], is_dir: bool) -> &[u16] {
    if is_dir {
        return &[];
    }
    match name.iter().rposition(|&c| c == u16::from(b'.')) {
        Some(dot) => &name[dot + 1..],
        None => &[],
    }
}

/// A row's extension, as the Extension column shows it.
pub fn of_row(row: &Row) -> &[u16] {
    extension(row.name(), row.is_dir)
}

/// The key a type is kept under: its extension, case-folded, or `None`
/// for a folder (folders share one type).
pub type Key = Option<Vec<u16>>;

/// A row's type's [`Key`].
pub fn key(row: &Row) -> Key {
    (!row.is_dir).then(|| of_row(row).iter().map(|&c| fold(c)).collect())
}

/// The shell's answers by `key`.
type Types = HashMap<Key, Rc<[u16]>>;

thread_local! {
    /// The answers this thread was given (the UI thread's, in the app).
    static TYPES: RefCell<Types> = RefCell::new(HashMap::new());
}

/// The name of type `key` if this thread knows it: looked up here before,
/// or learnt from the type thread.
pub fn known(key: &Key) -> Option<Rc<[u16]>> {
    TYPES.with(|t| t.borrow().get(key).cloned())
}

/// Keeps `name` as type `key`'s, looked up elsewhere (`typeinfo`), unless
/// this thread has one already.
pub fn learn(key: Key, name: Vec<u16>) {
    TYPES.with(|t| {
        t.borrow_mut().entry(key).or_insert_with(|| name.into());
    });
}

/// The shell's type name for `row` ("Application", "File folder"), looked
/// up once per extension, on this thread if it does not know it: for text
/// that must be right at once (Copy Property, File > Export, the preview's
/// details). Empty when the shell has none.
pub fn name(row: &Row) -> Rc<[u16]> {
    let key = key(row);
    if let Some(found) = known(&key) {
        return found;
    }
    // No borrow is held across the shell's call.
    let (_, found) = look_up(key.as_deref(), false);
    let found: Rc<[u16]> = found.into();
    TYPES.with(|t| t.borrow_mut().insert(key, Rc::clone(&found)));
    found
}

/// Asks the shell about files with extension `ext`, or about folders when
/// `None`: the type's name (empty when it has none), and with `icon` its
/// icon's index in the system image lists (`None` when it gave none).
/// Asking for both costs one call, not two.
pub fn look_up(ext: Option<&[u16]>, icon: bool) -> (Option<i32>, Vec<u16>) {
    let (mut probe, attrs): (Vec<u16>, u32) = match ext {
        None => ("folder".encode_utf16().collect(), FILE_ATTRIBUTE_DIRECTORY),
        Some(ext) => {
            let mut p: Vec<u16> = "file".encode_utf16().collect();
            if !ext.is_empty() {
                p.push(u16::from(b'.'));
                p.extend_from_slice(ext);
            }
            (p, FILE_ATTRIBUTE_NORMAL)
        }
    };
    probe.push(0);
    let mut flags = SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES;
    if icon {
        flags |= SHGFI_SYSICONINDEX | SHGFI_SMALLICON;
    }
    // SAFETY: SHFILEINFOW is plain data; all-zero is a valid value.
    let mut info: SHFILEINFOW = unsafe { core::mem::zeroed() };
    // SAFETY: an attribute-only lookup by name (nothing is read from disk);
    // `probe` is NUL-terminated and `info` a plain out-struct.
    let ok = unsafe {
        SHGetFileInfoW(
            probe.as_ptr(),
            attrs,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };
    if ok == 0 {
        return (None, Vec::new());
    }
    let len = info
        .szTypeName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szTypeName.len());
    (icon.then_some(info.iIcon), info.szTypeName[..len].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(name: &str, is_dir: bool) -> String {
        let name: Vec<u16> = name.encode_utf16().collect();
        String::from_utf16_lossy(extension(&name, is_dir))
    }

    fn row(path: &str, is_dir: bool) -> Row {
        let path: Box<[u16]> = path.encode_utf16().collect();
        let name_start = path.iter().rposition(|&c| c == u16::from(b'\\')).unwrap() + 1;
        Row {
            path,
            name_start: name_start as u32,
            size: None,
            mtime: None,
            ctime: None,
            is_dir,
            icon: std::cell::Cell::new(0),
        }
    }

    #[test]
    fn the_extension_is_after_the_last_dot() {
        assert_eq!(ext("notepad.exe", false), "exe");
        assert_eq!(ext("archive.tar.GZ", false), "GZ");
        assert_eq!(ext(".gitignore", false), "gitignore");
        assert_eq!(ext("README", false), "");
        assert_eq!(ext("trailing.", false), "");
        assert_eq!(ext("", false), "");
        // Folders have none, dots or not.
        assert_eq!(ext("v1.2", true), "");
    }

    #[test]
    fn a_type_is_kept_by_its_folded_extension() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(key(&row(r"C:\a\x.TXT", false)), Some(w("txt")));
        assert_eq!(key(&row(r"C:\a\noext", false)), Some(Vec::new()));
        assert_eq!(key(&row(r"C:\a\v1.2", true)), None);
    }

    #[test]
    fn types_come_from_the_shell_and_are_kept() {
        // The shell's words depend on the machine; that there are some, and
        // that one extension in any case is asked once, does not.
        let folder = name(&row(r"C:\Windows", true));
        assert!(!folder.is_empty());
        let a = name(&row(r"C:\a\x.TXT", false));
        let b = name(&row(r"C:\b\y.txt", false));
        assert!(!a.is_empty());
        assert!(Rc::ptr_eq(&a, &b), "one lookup per extension");
        assert!(!name(&row(r"C:\a\noext", false)).is_empty());
    }

    #[test]
    fn a_name_learnt_is_kept_and_never_replaced() {
        let key = key(&row(r"C:\a\x.goz-learnt", false));
        assert_eq!(known(&key), None);
        learn(key.clone(), "First".encode_utf16().collect());
        // A second answer for the same type (looked up here meanwhile, say)
        // leaves the one kept, so text already shown does not change.
        learn(key.clone(), "Second".encode_utf16().collect());
        let kept = known(&key).expect("kept");
        assert_eq!(String::from_utf16_lossy(&kept), "First");
        assert!(Rc::ptr_eq(&kept, &name(&row(r"C:\b\y.GOZ-LEARNT", false))));
    }
}
