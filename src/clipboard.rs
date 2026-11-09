//! The clipboard: text, and files the way Explorer takes them.
//!
//! Files go on as `CF_HDROP` (the list of paths Explorer pastes), `Preferred
//! DropEffect` (whether the paste copies or moves them) and `CF_UNICODETEXT`
//! (the paths, for anything that pastes text), all from one
//! OpenClipboard/CloseClipboard so a paste never sees half of them. A paste
//! reads them back the same way ([`files`]), whoever put them there.

use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
use windows_sys::Win32::System::Threading::Sleep;
use windows_sys::Win32::UI::Shell::{CFSTR_PREFERREDDROPEFFECT, DROPFILES, DragQueryFileW, HDROP};

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
/// it), so it tries for a while; false when it never got it, and then the
/// clipboard is as it was.
fn open(hwnd: HWND) -> bool {
    open_as_is(hwnd) && {
        // SAFETY: the clipboard is open; the caller closes it.
        unsafe { EmptyClipboard() };
        true
    }
}

/// Opens the clipboard for `hwnd` as it is, trying for a while as
/// [`open`] does; the caller closes it.
fn open_as_is(hwnd: HWND) -> bool {
    for attempt in 0..OPEN_TRIES {
        // SAFETY: opens the clipboard; the caller closes it.
        unsafe {
            if OpenClipboard(hwnd) != 0 {
                return true;
            }
            if attempt + 1 < OPEN_TRIES {
                Sleep(OPEN_RETRY_MS);
            }
        }
    }
    false
}

/// The clipboard holds files (CF_HDROP): Paste has something to paste.
pub fn has_files() -> bool {
    // SAFETY: asks about a standard format; the clipboard need not be open.
    unsafe { IsClipboardFormatAvailable(u32::from(CF_HDROP)) != 0 }
}

/// The clipboard holds text, for the search box's Paste.
pub fn has_text() -> bool {
    // SAFETY: as in `has_files`.
    unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT)) != 0 }
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
    // SAFETY: a registered name, NUL-terminated by windows-sys.
    let preferred = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
    if !open_as_is(hwnd) {
        return None;
    }
    // SAFETY: the clipboard is open; its handles are read while locked and
    // stay the clipboard's; it is closed below.
    let (paths, effect, sequence) = unsafe {
        let drop = GetClipboardData(u32::from(CF_HDROP)) as HDROP;
        let paths = if drop.is_null() {
            Vec::new()
        } else {
            dropped_files(drop)
        };
        let mut effect = None;
        let mem = if preferred != 0 {
            GetClipboardData(preferred)
        } else {
            core::ptr::null_mut()
        };
        if !mem.is_null() {
            let p = GlobalLock(mem) as *const u32;
            if !p.is_null() {
                effect = Some(p.read_unaligned());
                GlobalUnlock(mem);
            }
        }
        // While it is still open, and after the formats were rendered for
        // the reads above: nothing put on it since can be taken for these.
        let sequence = GetClipboardSequenceNumber();
        CloseClipboard();
        (paths, effect, sequence)
    };
    if paths.is_empty() {
        return None;
    }
    Some(Clip {
        paths,
        moving: is_move(effect),
        sequence,
    })
}

/// The paths of a CF_HDROP block (a drop's, or the clipboard's).
///
/// # Safety
/// `drop` must be a valid HDROP for as long as the call runs.
pub unsafe fn dropped_files(drop: HDROP) -> Vec<Vec<u16>> {
    // SAFETY: as the caller promises; each buffer is sized by the length
    // DragQueryFileW reports, its NUL included.
    unsafe {
        let n = DragQueryFileW(drop, u32::MAX, core::ptr::null_mut(), 0);
        (0..n)
            .filter_map(|i| {
                let len = DragQueryFileW(drop, i, core::ptr::null_mut(), 0) as usize;
                if len == 0 {
                    return None;
                }
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                buf.truncate(got);
                (got > 0).then_some(buf)
            })
            .collect()
    }
}

/// A paste moved the files read with sequence number `sequence`: the
/// clipboard is emptied, as after a cut and paste in Explorer (the paths
/// on it are gone), unless something else was put on it since.
pub fn clear_after_move(hwnd: HWND, sequence: u32) {
    // SAFETY: reads a counter.
    if unsafe { GetClipboardSequenceNumber() } != sequence {
        return;
    }
    if open(hwnd) {
        // SAFETY: closes the clipboard `open` emptied.
        unsafe { CloseClipboard() };
    }
}

/// Puts `len` bytes from `data` on the open clipboard as `format`.
///
/// # Safety
/// The clipboard must be open, and `data` readable for `len` bytes.
unsafe fn set(format: u32, data: *const u8, len: usize) -> bool {
    // SAFETY: the block is sized for `len` bytes and filled while locked; the
    // clipboard owns it once SetClipboardData succeeds, and it is freed here
    // otherwise.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, len.max(1));
        if mem.is_null() {
            return false;
        }
        let dst = GlobalLock(mem) as *mut u8;
        let mut ok = false;
        if !dst.is_null() {
            core::ptr::copy_nonoverlapping(data, dst, len);
            GlobalUnlock(mem);
            ok = !SetClipboardData(format, mem).is_null();
        }
        if !ok {
            GlobalFree(mem);
        }
        ok
    }
}

/// What the status bar says when the clipboard could not be opened.
pub const BUSY: &str = "Could not copy: another program is using the clipboard";

/// Puts `text` on the clipboard; false if it could not.
pub fn put_text(hwnd: HWND, text: &[u16]) -> bool {
    if !open(hwnd) {
        return false;
    }
    let mut z = Vec::with_capacity(text.len() + 1);
    z.extend_from_slice(text);
    z.push(0);
    // SAFETY: the clipboard is open; `z` holds `z.len()` code units.
    let ok = unsafe { set(u32::from(CF_UNICODETEXT), z.as_ptr().cast(), z.len() * 2) };
    // SAFETY: closes the clipboard opened above.
    unsafe { CloseClipboard() };
    ok
}

/// Puts `files` on the clipboard for Explorer (or any program) to paste,
/// copied or moved as `effect` says; false if it could not.
pub fn put_files(hwnd: HWND, files: &Files, effect: Effect) -> bool {
    let drop_effect = match effect {
        Effect::Copy => DROPEFFECT_COPY,
        Effect::Move => DROPEFFECT_MOVE,
    }
    .to_le_bytes();
    // SAFETY: a registered name, NUL-terminated by windows-sys.
    let preferred = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
    if !open(hwnd) {
        return false;
    }
    // SAFETY: the clipboard is open; each pointer is readable for the length
    // given with it.
    let ok = unsafe {
        set(u32::from(CF_HDROP), files.hdrop.as_ptr(), files.hdrop.len())
            && preferred != 0
            && set(preferred, drop_effect.as_ptr(), drop_effect.len())
            && set(
                u32::from(CF_UNICODETEXT),
                files.text.as_ptr().cast(),
                files.text.len() * 2,
            )
    };
    // SAFETY: closes the clipboard opened above.
    unsafe { CloseClipboard() };
    ok
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
