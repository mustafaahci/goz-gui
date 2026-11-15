//! The clipboard: text, and files the way Explorer takes them.
//!
//! Files go on as `CF_HDROP` (the list of paths Explorer pastes), `Preferred
//! DropEffect` (whether the paste copies or moves them) and `CF_UNICODETEXT`
//! (the paths, for anything that pastes text), all from one
//! OpenClipboard/CloseClipboard so a paste never sees half of them. A paste
//! reads them back the same way ([`files`]), whoever put them there.

// `dropped_files` is still called by files not yet on `crate::win`; it
// goes, and this with it, once they are.
#![allow(unsafe_code)] // Not yet moved onto `crate::win`.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
use windows_sys::Win32::UI::Shell::{DROPFILES, HDROP};

use crate::win::ext::b8::{self, Clipboard};

/// What a paste of the files does with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Copy: they stay where they are.
    Copy,
    /// Cut: the paste moves them.
    Move,
}

/// The files of a copy or cut, in the forms the clipboard holds them.
pub struct Files {
    hdrop: Vec<u8>,
    text: Vec<u16>,
}

/// `DROPFILES` as `CF_HDROP` starts with it; the paths follow it.
const DROPFILES_SIZE: usize = 20;
const _: () = assert!(size_of::<DROPFILES>() == DROPFILES_SIZE);

/// The registered format that says whether a paste copies or moves
/// (CFSTR_PREFERREDDROPEFFECT).
const PREFERRED_DROP_EFFECT: &str = "Preferred DropEffect";

impl Files {
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a [u16]>) -> Files {
        // DROPFILES: where the paths start, a drop point (none), whether the
        // point is non-client (no), and wide paths (yes).
        let mut hdrop = Vec::with_capacity(DROPFILES_SIZE + 2);
        for field in [DROPFILES_SIZE as u32, 0, 0, 0, 1] {
            hdrop.extend_from_slice(&field.to_le_bytes());
        }
        let mut text = Vec::new();
        for (i, path) in paths.into_iter().enumerate() {
            for &unit in path.iter().chain(&[0]) {
                hdrop.extend_from_slice(&unit.to_le_bytes());
            }
            if i > 0 {
                text.extend_from_slice(&[13, 10]);
            }
            text.extend_from_slice(path);
        }
        // The list ends with an empty path.
        hdrop.extend_from_slice(&[0, 0]);
        text.push(0);
        Files { hdrop, text }
    }
}

/// Tries to open the clipboard this many times, this far apart: about
/// 135 ms in all, about what OLE's own clipboard calls wait.
const OPEN_TRIES: u32 = 10;
const OPEN_RETRY_MS: u32 = 15;

/// Opens the clipboard for `hwnd`, emptied. Another program may hold it for
/// a moment (clipboard managers read every change, remote desktop syncs
/// it), so it tries for a while; `None` when it never got it, and then the
/// clipboard is as it was. It closes as the guard drops.
fn open(hwnd: HWND) -> Option<Clipboard> {
    let clipboard = open_as_is(hwnd)?;
    clipboard.empty();
    Some(clipboard)
}

/// Opens the clipboard for `hwnd` as it is, trying for a while as
/// [`open`] does; it closes as the guard drops.
fn open_as_is(hwnd: HWND) -> Option<Clipboard> {
    for attempt in 0..OPEN_TRIES {
        if let Some(clipboard) = Clipboard::open(hwnd) {
            return Some(clipboard);
        }
        if attempt + 1 < OPEN_TRIES {
            b8::sleep(OPEN_RETRY_MS);
        }
    }
    None
}

/// The clipboard holds files (CF_HDROP): Paste has something to paste.
pub fn has_files() -> bool {
    // A standard format; the clipboard need not be open.
    b8::clipboard_has(u32::from(CF_HDROP))
}

/// The clipboard holds text, for the search box's Paste.
pub fn has_text() -> bool {
    b8::clipboard_has(u32::from(CF_UNICODETEXT))
}

/// Files on the clipboard, as a paste takes them.
#[derive(Debug, PartialEq, Eq)]
pub struct Clip {
    pub paths: Vec<Vec<u16>>,
    /// Cut, not copied: the paste moves them (`Preferred DropEffect` says
    /// a move and not a copy; without it, a copy).
    pub moving: bool,
    /// The clipboard's sequence number as they were read (with the
    /// clipboard still open): it changes with anything put on it since.
    pub sequence: u32,
}

/// Whether `Preferred DropEffect` `effect` asks for a move: Explorer's Cut
/// says DROPEFFECT_MOVE alone, its Copy DROPEFFECT_COPY | DROPEFFECT_LINK.
fn is_move(effect: Option<u32>) -> bool {
    effect.is_some_and(|e| e & DROPEFFECT_MOVE != 0 && e & DROPEFFECT_COPY == 0)
}

/// The files on the clipboard (CF_HDROP) and whether they were cut;
/// `None` when it holds none or cannot be opened.
pub fn files(hwnd: HWND) -> Option<Clip> {
    let preferred = b8::register_clipboard_format(PREFERRED_DROP_EFFECT);
    let clipboard = open_as_is(hwnd)?;
    // Copies of what it holds, read while it is open.
    let paths = clipboard.files();
    let effect = if preferred != 0 {
        clipboard
            .data(preferred)
            .and_then(|d| Some(u32::from_le_bytes(d.get(..4)?.try_into().ok()?)))
    } else {
        None
    };
    // While it is still open, and after the formats were rendered for the
    // reads above: nothing put on it since can be taken for these.
    let sequence = b8::clipboard_sequence();
    drop(clipboard);
    if paths.is_empty() {
        return None;
    }
    Some(Clip {
        paths,
        moving: is_move(effect),
        sequence,
    })
}

/// W7: the text on the clipboard (CF_UNICODETEXT), for a paste of file
/// names; `None` when it holds none or cannot be opened.
pub fn text(hwnd: HWND) -> Option<String> {
    if !has_text() {
        return None;
    }
    let clipboard = open_as_is(hwnd)?;
    // A copy of what it holds, read up to its NUL.
    let bytes = clipboard.data(u32::from(CF_UNICODETEXT));
    drop(clipboard);
    let units: Vec<u16> = bytes?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_ne_bytes(c))
        .collect();
    Some(String::from_utf16_lossy(crate::win::until_nul(&units)))
}

/// The paths of a CF_HDROP block (a drop's, or the clipboard's).
///
/// # Safety
/// `drop` must be a valid HDROP for as long as the call runs.
pub unsafe fn dropped_files(drop: HDROP) -> Vec<Vec<u16>> {
    // SAFETY: as the caller promises.
    unsafe { b8::hdrop_paths(drop) }
}

/// A paste moved the files read with sequence number `sequence`: the
/// clipboard is emptied, as after a cut and paste in Explorer (the paths
/// on it are gone), unless something else was put on it since.
pub fn clear_after_move(hwnd: HWND, sequence: u32) {
    if b8::clipboard_sequence() != sequence {
        return;
    }
    // Emptied as it opens, closed as it drops.
    drop(open(hwnd));
}

/// The bytes of `units`, as the clipboard holds text (native order).
fn bytes_of(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|u| u.to_ne_bytes()).collect()
}

/// What the status bar says when the clipboard could not be opened.
pub const BUSY: &str = "Could not copy: another program is using the clipboard";

/// Puts `text` on the clipboard; false if it could not.
pub fn put_text(hwnd: HWND, text: &[u16]) -> bool {
    let Some(clipboard) = open(hwnd) else {
        return false;
    };
    let mut z = Vec::with_capacity(text.len() + 1);
    z.extend_from_slice(text);
    z.push(0);
    clipboard.set(u32::from(CF_UNICODETEXT), &bytes_of(&z))
}

/// Puts `files` on the clipboard for Explorer (or any program) to paste,
/// copied or moved as `effect` says; false if it could not.
pub fn put_files(hwnd: HWND, files: &Files, effect: Effect) -> bool {
    let drop_effect = match effect {
        Effect::Copy => DROPEFFECT_COPY,
        Effect::Move => DROPEFFECT_MOVE,
    }
    .to_le_bytes();
    let preferred = b8::register_clipboard_format(PREFERRED_DROP_EFFECT);
    let Some(clipboard) = open(hwnd) else {
        return false;
    };
    clipboard.set(u32::from(CF_HDROP), &files.hdrop)
        && preferred != 0
        && clipboard.set(preferred, &drop_effect)
        && clipboard.set(u32::from(CF_UNICODETEXT), &bytes_of(&files.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    /// The paths in a `CF_HDROP` block, read as DragQueryFile does.
    fn paths_of(hdrop: &[u8]) -> Vec<String> {
        let start = u32_at(hdrop, 0) as usize;
        assert_eq!(u32_at(hdrop, 16), 1, "wide paths");
        let units: Vec<u16> = hdrop[start..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let len = units[at..].iter().position(|&u| u == 0).unwrap();
            if len == 0 {
                assert_eq!(at + 1, units.len(), "nothing after the list's end");
                return out;
            }
            out.push(String::from_utf16(&units[at..at + len]).unwrap());
            at += len + 1;
        }
    }

    #[test]
    fn hdrop_lists_every_path() {
        let a = w(r"C:\Windows\notepad.exe");
        let b = w(r"D:\Fotoğraflar\ağaç.jpg");
        let f = Files::new([&a[..], &b[..]]);
        assert_eq!(u32_at(&f.hdrop, 0), DROPFILES_SIZE as u32);
        // No drop point, client coordinates.
        assert_eq!(&f.hdrop[4..16], &[0; 12]);
        assert_eq!(
            paths_of(&f.hdrop),
            [r"C:\Windows\notepad.exe", r"D:\Fotoğraflar\ağaç.jpg"]
        );
        assert_eq!(
            String::from_utf16(&f.text).unwrap(),
            "C:\\Windows\\notepad.exe\r\nD:\\Fotoğraflar\\ağaç.jpg\0"
        );
    }

    #[test]
    fn a_cut_moves_and_a_copy_copies() {
        // Explorer's Cut, and its Copy (copy | link).
        assert!(is_move(Some(DROPEFFECT_MOVE)));
        assert!(!is_move(Some(DROPEFFECT_COPY | 4)));
        // Either: a copy, as Explorer pastes it.
        assert!(!is_move(Some(DROPEFFECT_COPY | DROPEFFECT_MOVE)));
        assert!(!is_move(None));
        assert!(!is_move(Some(0)));
    }

    #[test]
    fn one_file_has_no_line_break() {
        let a = w(r"C:\a.txt");
        let f = Files::new([&a[..]]);
        assert_eq!(paths_of(&f.hdrop), [r"C:\a.txt"]);
        assert_eq!(String::from_utf16(&f.text).unwrap(), "C:\\a.txt\0");
    }
}
