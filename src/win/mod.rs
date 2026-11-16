//! The Win32 boundary: every `unsafe` in goz-gui is in this module.
//!
//! The rest of the crate is `#![deny(unsafe_code)]` (see `main.rs`) and
//! reaches Windows through the safe functions here. Each one is sound for
//! any arguments safe code can give it, given the trust model below; where
//! that would not hold (a message whose parameters are pointers, a raw
//! `lParam`), the API is typed instead.
//!
//! # Trust model
//!
//! - **The system behaves as documented.** user32, gdi32, comctl32, shell32
//!   and ole32 check the handles they are given: a stale or wrong `HWND`,
//!   `HDC`, `HMENU`, `HICON`, `HACCEL` or GDI object is reported as a
//!   failure, never followed as a pointer. Those handles are therefore
//!   plain values here, `Copy`, and any value is a valid argument. Two
//!   kinds of "handle" are not checked, and are not plain values here:
//!   - An image list (`HIMAGELIST`) is an object's address (an
//!     `IImageList*`, as SHGetImageList documents), followed by every call.
//!     A list is an [`imagelist::ImageList`] (owned, destroyed with its last
//!     clone) or an [`imagelist::SystemList`] (the shell's, kept for the
//!     process); no safe function takes a raw one or destroys one, and a
//!     control given one keeps it alive as long as it shows it. Which list
//!     a control shows, takes or gives back is asked of its class's own
//!     procedure (see below), after each giving: one may be nested in
//!     another, from a notification the control sends as it takes a list.
//!     A list a control gave back or no longer shows is let go only once
//!     the control is done with the giving that ended its use.
//!   - A tree view's item (`HTREEITEM`) is the address of the item's memory
//!     in its tree, which comctl32 checks only by a signature there, never
//!     against the tree it is sent to. Items are [`tv::Item`]s, made only
//!     from the tree's own answers and notifications and naming their tree,
//!     and taken only while the tree has them, for a tree kept track of
//!     from its birth (made by [`class::create_plain`] or from a
//!     [`dlg::Template`]). [`tv`]'s messages that name an item go to the
//!     tree's class's own procedure, so no subclass runs between the check
//!     that the item is in the tree and the tree's reading it; an item is
//!     noted as [`tv::insert`] puts it in, by the tree's own answer; each
//!     deletion (TVM_DELETEITEM of an item, of an item above it or of every
//!     item, a collapse that resets an item above it, the tree's end) takes
//!     its items out of the record before the tree sees it, [`tv`]'s own
//!     and, by a subclass put in as the tree is made, another's code's
//!     through the window; so an item is refused from the moment its
//!     deletion starts, and while one is under way the tree takes no
//!     insert, deletion or reset of goz-gui's. Another's code's message
//!     that names an item comes through the window's subclasses, any of
//!     which may delete the item before passing it on: each pass of one by
//!     a subclass of goz-gui's ([`Msg::def`]) checks every item it names
//!     (in its parameters, or copied from the structure its `lParam` points
//!     at) against the record first, and stops it, answering 0, if one is
//!     not there; so the last check before the tree is made whatever the
//!     order of the subclasses. A stale item names, at worst, a new item
//!     the same tree put at its address: wrong, not unsound. What is
//!     trusted to comctl32 is the tree's own handling of an item deleted
//!     from inside one of its notifications about that item (TVN_SELCHANGED,
//!     TVN_ITEMEXPANDED…, sent while the tree is already using it), as any
//!     program with a tree view trusts it.
//! - **A message's parameters mean what its documentation says.** A message
//!   below `WM_USER` is the system's: every window class of the system and
//!   of the common controls means by it what the system documents, so a
//!   wrapper for one may be sent to any window. Rich Edit is the documented
//!   exception: it reads some edit messages otherwise (EM_CHARFROMPOS takes
//!   a POINTL's address there, EM_POSFROMCHAR one to write to), and those
//!   are sent only to an Edit ([`edit`] lists the others it checked). A
//!   message from `WM_USER` up is defined by its control class (`LVM_FIRST`
//!   and `DTM_FIRST` are both `0x1000`), so the wrappers for those check the
//!   window's class first and do nothing for a window of another class.
//! - **A reply the layer's soundness depends on is asked of the control
//!   class's own procedure, never through the subclass chain.** Any safe
//!   [`subclass::set`] closure can answer any message sent to a window, so
//!   such a reply is had by calling the class's procedure directly
//!   ([`system_class_proc`]: for a window of this thread whose class's
//!   procedure is the system's, refused otherwise), with no subclass in
//!   between: the length that sizes LB_GETTEXT's buffer ([`listbox`]), the
//!   image list a list view shows or gives back (LVM_GETIMAGELIST,
//!   LVM_SETIMAGELIST) and the one a tree takes (TVM_SETIMAGELIST), a
//!   tree's new items and every walk of its items (TVM_INSERTITEMW,
//!   TVM_GETNEXTITEM). So does a message whose item a subclass could
//!   delete before passing it on (every [`tv`] message naming an item);
//!   another's code's come through the chain, which passes one on only
//!   while the tree has its items (above). Any other answer that comes
//!   back through the chain is taken as a value that may be wrong, never
//!   followed.
//! - **What the system delivers is what it documents.** A procedure's
//!   `lParam` for `WM_NOTIFY`, `WM_DRAWITEM` and the like points at the
//!   structure the message defines. Only a [`Msg`] the system delivered
//!   hands those out, as a [`View`]: read-only, but for the structures none
//!   of whose fields the system reads back as an address, a length or an
//!   index ([`msg::Writable`]: WINDOWPOS, MINMAXINFO, MEASUREITEMSTRUCT).
//!   What a notification asks its procedure to write goes in through a
//!   setter that checks the notification and writes through the pointers
//!   the system gave; a custom draw structure's colours and state through
//!   setters of their own, as its control reads its other fields back (a
//!   list view its `dwItemSpec`, as the row it goes on to draw). Which
//!   structure a `WM_NOTIFY` comes with is told by its header as
//!   delivered, copied once per message before any view is lent (none
//!   lends it writable). A message made by goz-gui's own code hands out
//!   nothing.
//! - **Another process of the user is as trusted as goz-gui.** A process of
//!   the same user and integrity can send or post goz-gui's windows any
//!   message with any parameters, and the system marshals few of them:
//!   `WM_NOTIFY`, `HDM_LAYOUT`, the menu bar's `0x91` and `0x92` and
//!   `WM_DROPFILES` from such a process carry whatever addresses it chose.
//!   The layer, like any Win32 program, trusts what its message queue
//!   delivers: that process can already do anything goz-gui can (write its
//!   memory, run code in it). What other programs leave on the clipboard is
//!   read from a copy, no further than its size.
//! - **goz-gui's private messages (`WM_APP` up to `0xC000`) carry no
//!   pointers,** and go only to goz-gui's own windows (of this process,
//!   handled by a trampoline) and message loops, which read them so; to
//!   another window the same numbers may be its own messages. A value that
//!   has to travel with one goes as a [`parcel`].
//! - **Some undocumented interfaces are as described.** The menu bar's
//!   drawing messages and their structures ([`menubar`], laid out so since
//!   Windows 10 1809), and uxtheme's dark-mode exports at ordinals 133, 135
//!   and 136 with the signatures [`uxtheme`] gives them, looked up only on
//!   build 17763 (1809) or later. A shell folder given an item ID list it
//!   did not make (another folder's, a shortcut's) checks it rather than
//!   reading past it ([`shell`]'s ID lists are the shell's own, so each is
//!   well-formed).
//! - **In-process COM servers keep their interfaces' contracts.** The calls
//!   here load third-party code into the process: shell extensions
//!   (context menu, icon and thumbnail handlers), WIC's codecs, namespace
//!   extensions. Like the system, they write no further than the buffers
//!   they are given. Code in the process could break any promise of Rust's
//!   anyway; this is the narrow part of that trust the layer relies on,
//!   stated because it is about code that is not the system's. COM is never
//!   uninitialized on a thread ([`com::Apartment`]), so no object outlives
//!   the libraries it runs from. One call more relies on servers' word:
//!   [`com::Apartment::end_unloading_unused`] (CoFreeUnusedLibrariesEx with
//!   no delay), which unloads the DLLs COM loaded for the calling thread's
//!   apartments whose DllCanUnloadNow says they can go. goz-gui makes it as
//!   the preview's thread ends, its objects released, and it does nothing
//!   on a thread whose apartment was kept (the UI thread's) or that made an
//!   Explorer context menu, whose handlers are known to answer wrongly
//!   (Work Folders' says yes while a thread of its own lives: see
//!   `crate::shell::ContextMenu`). The assumption is so: the COM servers a
//!   thread that neither kept its apartment nor made a context menu loads
//!   answer DllCanUnloadNow truthfully, saying they can go only when
//!   nothing of theirs is in use or running.
//!
//! The procedures goz-gui gives Windows (window, dialog and subclass
//! procedures) are the trampolines here; they hand each message to a safe
//! Rust closure as a [`Msg`]. The closure is kept alive for as long as a
//! call to it runs, so a window that is destroyed by its own handler does
//! not free what that handler is using; a COM object of goz-gui's holds a
//! reference to itself while a method of it runs a closure.

#![allow(unsafe_code)]
#![warn(clippy::undocumented_unsafe_blocks)]

pub mod a11y;
pub mod button;
pub mod class;
pub mod clipboard;
pub mod com;
pub mod combo;
pub mod datetime;
pub mod dib;
pub mod dlg;
pub mod dropfiles;
pub mod edit;
pub mod file;
pub mod gdi;
pub mod header;
pub mod hook;
pub mod hotkey;
pub mod icon;
pub mod imagelist;
pub mod listbox;
pub mod locale;
pub mod lv;
pub mod menu;
pub mod menubar;
pub mod msg;
pub mod parcel;
pub mod preview;
pub mod process;
pub mod queue;
pub mod reg;
pub mod resource;
pub mod scroll;
pub mod shell;
pub mod subclass;
pub mod sync;
pub mod sys;
pub mod tab;
pub mod time;
pub mod tip;
pub mod trackbar;
pub mod tv;
pub mod uxtheme;
pub mod wic;
pub mod wnd;

#[cfg(test)]
mod trampolines_tests;

pub use msg::{Msg, Notify, View};
pub use wnd::*;

use std::borrow::Cow;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, SendMessageW};

/// Sends any message. Only the wrappers in this module call it, each for a
/// message whose parameters it has typed.
///
/// # Safety
/// `w` and `l` are what message `msg` defines for `hwnd`'s class: any
/// pointer among them is valid for the access the message makes, for the
/// length of the call.
pub(crate) unsafe fn send_raw(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: as the caller promises.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

/// Whether `hwnd`'s window class is `name`, which window classes compare
/// as: ignoring ASCII case. False for a window that is not there.
pub fn class_is(hwnd: HWND, name: &str) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer and its length in code units.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    let got = &buf[..n.min(buf.len())];
    let mut want = name.encode_utf16();
    for &unit in got {
        match want.next() {
            Some(w) if fold(w) == fold(unit) => {}
            _ => return false,
        }
    }
    want.next().is_none()
}

/// Whether `hwnd` is a window of the calling thread (false for one that is
/// not there).
pub(crate) fn is_this_thread(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    // SAFETY: no pointers (no process id asked for).
    let thread = unsafe { GetWindowThreadProcessId(hwnd, core::ptr::null_mut()) };
    thread != 0 && thread == sys::thread_id()
}

/// A window procedure, as a class has one.
pub(crate) type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// The procedure of the system's own window class `name` (GetClassInfoExW
/// with no instance: the class of the system or of the common controls in
/// this activation context), if `hwnd` is a window of this thread whose
/// class's procedure is that one (GetClassLongPtrW). Called directly
/// (CallWindowProcW), it is the class's own code for the window, with no
/// subclass of the window in front of it.
pub(crate) fn system_class_proc(hwnd: HWND, name: &str) -> Option<WndProc> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GCLP_WNDPROC, GetClassInfoExW, GetClassLongPtrW, WNDCLASSEXW,
    };
    if !is_this_thread(hwnd) {
        return None;
    }
    let z = wz(name);
    let mut wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        ..Default::default()
    };
    // SAFETY: a NUL-terminated name read during the call; an out-struct
    // with its size (its pointers are not followed here).
    if unsafe { GetClassInfoExW(core::ptr::null_mut(), z.as_ptr(), &mut wc) } == 0 {
        return None;
    }
    let system = wc.lpfnWndProc?;
    // SAFETY: no pointers: the address of the window's class's procedure.
    let class = unsafe { GetClassLongPtrW(hwnd, GCLP_WNDPROC) };
    (class != 0 && class == system as usize).then_some(system)
}

/// A pointer to the units of `s` for a call that takes a pointer and a
/// length: a real address even when `s` is empty. An empty slice's own
/// pointer is dangling (fine for Rust, as nothing is read), but parts of
/// Windows read the first unit whatever the length (DrawTextW does).
pub(crate) fn units_ptr(s: &[u16]) -> *const u16 {
    static EMPTY: [u16; 1] = [0];
    if s.is_empty() {
        EMPTY.as_ptr()
    } else {
        s.as_ptr()
    }
}

/// [`units_ptr`] for bytes.
pub(crate) fn bytes_ptr(s: &[u8]) -> *const u8 {
    static EMPTY: [u8; 1] = [0];
    if s.is_empty() {
        EMPTY.as_ptr()
    } else {
        s.as_ptr()
    }
}

/// `hwnd`'s window class's name (empty for a window that is not there).
pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer and its length in code units.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// Whether `a` and `b` name the same window class: equal ignoring case
/// as the system's atoms compare names (its own upper-case table,
/// CompareStringOrdinal's), not only ASCII's.
pub(crate) fn same_class_name(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    locale::compare_ordinal(&a, &b, true) == windows_sys::Win32::Globalization::CSTR_EQUAL
}

fn fold(unit: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

/// `hwnd` if its class is `name`, else null: a message sent to null does
/// nothing and answers 0, as a message to a window that has gone does.
pub(crate) fn of_class(hwnd: HWND, name: &str) -> HWND {
    if class_is(hwnd, name) {
        hwnd
    } else {
        debug_assert!(
            hwnd.is_null() || !wnd::is_window(hwnd),
            "a {name} message for a window of another class"
        );
        core::ptr::null_mut()
    }
}

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// `s` with a NUL appended, for a call that wants a C string.
pub fn wz_units(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s);
    v.push(0);
    v
}

/// `units` NUL-terminated, for a call that wants a C string: as they are
/// when they end with a NUL, else a copy with one.
pub(crate) fn nul_ended(units: &[u16]) -> Cow<'_, [u16]> {
    if units.last() == Some(&0) {
        Cow::Borrowed(units)
    } else {
        Cow::Owned(wz_units(units))
    }
}

/// A slice's length as the `i32` count a call takes, cut to fit (the call
/// then reads or writes less of the slice, never more).
pub(crate) fn len_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// A slice's length as the `u32` count a call takes, cut to fit.
pub(crate) fn len_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// `units` as the bytes they are in memory (native order), for a byte
/// search over UTF-16 text.
pub fn units_as_bytes(units: &[u16]) -> &[u8] {
    // SAFETY: u16 has no padding and u8's alignment is 1; the bytes are
    // the slice's own, borrowed for as long as it is.
    unsafe { core::slice::from_raw_parts(units.as_ptr().cast::<u8>(), size_of_val(units)) }
}

/// The UTF-16 units before the first NUL of `buf` (all of it without one).
pub fn until_nul(buf: &[u16]) -> &[u16] {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    &buf[..end]
}

/// The low word of a message parameter.
pub fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

/// The high word of a message parameter.
pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

/// `lo` and `hi` as one `LPARAM`, as MAKELPARAM makes it.
pub fn make_lparam(lo: u16, hi: u16) -> LPARAM {
    (u32::from(lo) | (u32::from(hi) << 16)) as i32 as LPARAM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_go_in_and_come_out() {
        assert_eq!(loword(make_lparam(3, 7) as usize), 3);
        assert_eq!(hiword(make_lparam(3, 7) as usize), 7);
        assert_eq!(make_lparam(0xffff, 0xffff), -1);
    }

    #[test]
    fn units_are_their_bytes() {
        assert_eq!(units_as_bytes(&[0x0102, 0x0304]), [2, 1, 4, 3]);
        assert!(units_as_bytes(&[]).is_empty());
        assert_eq!(&*nul_ended(&[104, 0]), &[104, 0]);
        assert_eq!(&*nul_ended(&[104]), &[104, 0]);
    }

    #[test]
    fn a_string_ends_at_its_nul() {
        assert_eq!(until_nul(&[104, 105, 0, 33]), &[104, 105]);
        assert_eq!(until_nul(&[104, 105]), &[104, 105]);
        assert_eq!(wz("hi"), vec![104, 105, 0]);
    }
}
