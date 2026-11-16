//! Windows of this thread in the middle of their own processing, as far as
//! goz-gui sees, and the destruction of a window that is or holds one,
//! which waits until it is out of it.
//!
//! A control that sends its parent a message (a notification, a command,
//! an item to draw or measure, a colour to use, a scroll) is in the middle
//! of an operation of its own, and the common controls do not survive
//! being destroyed then: a tree deleting or expanding items or moving its
//! selection, a list view deleting its items, go on using their memory as
//! the message returns. goz-gui's code runs inside such an operation only
//! where a trampoline hands it a message, and each holds the control busy
//! ([`hold`]) while it runs:
//!
//! - a window's, a dialog's or a subclass's procedure handed a message a
//!   control sends its parent ([`sender`]: WM_NOTIFY's `hwndFrom`,
//!   WM_COMMAND's and the WM_CTLCOLOR* messages' control, WM_DRAWITEM's,
//!   WM_MEASUREITEM's, WM_DELETEITEM's, WM_COMPAREITEM's, WM_HSCROLL's and
//!   WM_VSCROLL's, WM_CHARTOITEM's and WM_VKEYTOITEM's, the child
//!   WM_PARENTNOTIFY is about, the child a WM_CONTEXTMENU comes from);
//! - a subclass passing a message on to a control's own procedure
//!   ([`super::Msg::def`]), as the control may send itself messages the
//!   subclass sees meanwhile;
//! - a message [`super::queue::dispatch`] hands a control, with what the
//!   control's own loops dispatch meanwhile (a drag's);
//! - a hook's closure: the window a sent message is for and the control it
//!   comes from ([`super::hook`]'s WH_CALLWNDPROC and WH_CALLWNDPROCRET),
//!   the window a CBT event is about (and the parent of one being made);
//! - [`super::tv`]'s calls to a tree's class's procedure, and the messages
//!   that pass the subclass keeping track of a tree.
//!
//! [`super::destroy`] of a window that is busy, or holds a busy one (a
//! child, a window it owns, theirs: what DestroyWindow destroys with it),
//! does not destroy it then: it is destroyed once the stack has unwound,
//! from this thread's message loop, by a message-only window of the
//! layer's own ([`doom`]), if it is still there and nothing in it is busy
//! by then. A window goz-gui made answers WM_CLOSE the same way
//! ([`super::Msg::def`]).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::UI::Controls::{
    COMPAREITEMSTRUCT, DELETEITEMSTRUCT, DRAWITEMSTRUCT, MEASUREITEMSTRUCT, NMHDR, ODT_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GA_PARENT, GW_OWNER, GetAncestor, GetDlgItem, GetWindow, HWND_MESSAGE,
    PostMessageW, WM_APP, WM_CHARTOITEM, WM_COMMAND, WM_COMPAREITEM, WM_CONTEXTMENU, WM_CREATE,
    WM_CTLCOLORMSGBOX, WM_CTLCOLORSTATIC, WM_DELETEITEM, WM_DESTROY, WM_DRAWITEM, WM_HSCROLL,
    WM_MEASUREITEM, WM_NOTIFY, WM_PARENTNOTIFY, WM_VKEYTOITEM, WM_VSCROLL, WS_CHILD,
};

thread_local! {
    /// The windows held busy, once per hold (holds nest).
    static BUSY: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    /// Windows whose destruction waits ([`doom`]), first asked first.
    static DOOMED: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    /// The thread's message-only window that destroys them, once made.
    static REAPER: Cell<HWND> = const { Cell::new(core::ptr::null_mut()) };
    /// Whether a message to it is waiting in the queue.
    static POSTED: Cell<bool> = const { Cell::new(false) };
}

/// The reaper's class, registered for the process's module.
const REAPER_CLASS: &str = "goz-gui deferred destroy";

/// The message that has the reaper destroy what waits: a private message
/// of its own class's windows, which have no other.
const REAP: u32 = WM_APP;

/// A window held busy while this lives (on the thread that holds it).
pub(crate) struct Hold {
    hwnd: HWND,
}

/// Holds `hwnd` busy until the answer drops; `None` for null.
pub(crate) fn hold(hwnd: HWND) -> Option<Hold> {
    if hwnd.is_null() {
        return None;
    }
    BUSY.with_borrow_mut(|b| b.push(hwnd));
    Some(Hold { hwnd })
}

impl Drop for Hold {
    fn drop(&mut self) {
        // Thread-locals may be gone already as the thread ends: then so are
        // its windows, and nothing waits.
        let _ = BUSY.try_with(|b| {
            let mut b = b.borrow_mut();
            if let Some(at) = b.iter().rposition(|&w| w == self.hwnd) {
                b.remove(at);
            }
        });
        let waits = DOOMED
            .try_with(|d| d.borrow().iter().any(|&root| contains(root, self.hwnd)))
            .unwrap_or(false);
        // A window waiting on this one may go now.
        if waits {
            wake();
        }
    }
}

/// Whether `root` is busy, or holds a window that is (see the module's
/// documentation).
pub(crate) fn within(root: HWND) -> bool {
    if root.is_null() {
        return false;
    }
    BUSY.with_borrow(|b| b.iter().any(|&w| contains(root, w)))
}

/// Whether `w` is `root` or a window DestroyWindow of `root` destroys with
/// it: a child (or a child's), or a window it owns (or theirs).
fn contains(root: HWND, w: HWND) -> bool {
    /// How far up a chain is followed: windows nest a few levels.
    const DEPTH: usize = 256;
    let mut at = w;
    for _ in 0..DEPTH {
        if at.is_null() {
            return false;
        }
        if at == root {
            return true;
        }
        let child = super::style(at) & WS_CHILD != 0;
        // SAFETY: no pointers; handles the system checks.
        at = unsafe {
            if child {
                GetAncestor(at, GA_PARENT)
            } else {
                GetWindow(at, GW_OWNER)
            }
        };
    }
    false
}

/// Has `hwnd`, which is or holds a busy window, destroyed once nothing in
/// it is: false if it is not a window.
pub(crate) fn doom(hwnd: HWND) -> bool {
    if !super::is_window(hwnd) {
        return false;
    }
    DOOMED.with_borrow_mut(|d| {
        if !d.contains(&hwnd) {
            d.push(hwnd);
        }
    });
    wake();
    true
}

/// Posts the reaper a message (one at a time), made first if need be.
fn wake() {
    // As the thread ends, its thread-locals may be gone: so are its windows.
    if POSTED.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    let Ok(made) = REAPER.try_with(Cell::get) else {
        return;
    };
    let reaper = if super::is_window(made) {
        made
    } else {
        make_reaper()
    };
    // SAFETY: a private message of the reaper's class, which carries
    // nothing, to a window of this thread's that the reaper's handler
    // reads so.
    if !reaper.is_null() && unsafe { PostMessageW(reaper, REAP, 0, 0) } != 0 {
        POSTED.set(true);
    }
}

/// Makes this thread's reaper (null if it could not be).
fn make_reaper() -> HWND {
    use super::class::{self, Class, Create};
    // Registered once per process; again, it is there already.
    let _ = Class::new(REAPER_CLASS).register();
    let hwnd = class::create(
        &Create {
            ex_style: 0,
            class: REAPER_CLASS,
            title: "",
            style: 0,
            rect: (0, 0, 0, 0),
            parent: HWND_MESSAGE,
            menu: core::ptr::null_mut(),
        },
        |m| {
            if m.id == REAP {
                reap();
                return 0;
            }
            m.def()
        },
    );
    REAPER.set(hwnd);
    hwnd
}

/// Destroys the windows that wait, first asked first, each that is still
/// there and holds nothing busy; the others wait on.
fn reap() {
    POSTED.set(false);
    let doomed = DOOMED.take();
    let mut kept = Vec::new();
    for hwnd in doomed {
        if !super::is_window(hwnd) {
            continue;
        }
        if within(hwnd) {
            kept.push(hwnd);
            continue;
        }
        // SAFETY: no pointers; a window of this thread that nothing in it
        // is in the middle of its processing (`within`).
        unsafe { DestroyWindow(hwnd) };
    }
    // Those asked for while these went, after.
    DOOMED.with_borrow_mut(|d| {
        kept.append(d);
        *d = kept;
    });
}

/// The control a message `msg` with parameters `w` and `l`, delivered to
/// `hwnd`, comes from, if it is one a control sends its parent (or owner)
/// in the middle of an operation of its own (see the module's
/// documentation); not `hwnd` itself.
///
/// # Safety
/// The parameters are as the system delivered them with `msg` to `hwnd`
/// (a procedure of it, or a hook seeing it sent): for a message whose
/// lParam is a structure, `l` is null or points at it, valid for reads.
pub(crate) unsafe fn sender(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<HWND> {
    /// The structure `l` points at, copied out; `None` for null.
    ///
    /// # Safety
    /// `l` is null or points at a `T`, valid for reads.
    unsafe fn read<T: Copy>(l: LPARAM) -> Option<T> {
        // SAFETY: as the caller promises; copied unaligned, as little is
        // known of the sender's alignment.
        (l != 0).then(|| unsafe { (l as *const T).read_unaligned() })
    }
    let from = match msg {
        // SAFETY: WM_NOTIFY's lParam is an NMHDR (the caller's word).
        WM_NOTIFY => unsafe { read::<NMHDR>(l) }.map(|h| h.hwndFrom),
        // From a control: lParam is its handle (0 from a menu or a key).
        WM_COMMAND | WM_HSCROLL | WM_VSCROLL | WM_CHARTOITEM | WM_VKEYTOITEM => Some(l as HWND),
        WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC => Some(l as HWND),
        // SAFETY: WM_DRAWITEM's lParam is a DRAWITEMSTRUCT.
        WM_DRAWITEM => unsafe { read::<DRAWITEMSTRUCT>(l) }
            .filter(|d| d.CtlType != ODT_MENU)
            .map(|d| d.hwndItem),
        // SAFETY: WM_MEASUREITEM's lParam is a MEASUREITEMSTRUCT; it names
        // its control by id only.
        WM_MEASUREITEM => unsafe { read::<MEASUREITEMSTRUCT>(l) }
            .filter(|m| m.CtlType != ODT_MENU)
            .map(|m| dlg_item(hwnd, m.CtlID)),
        // SAFETY: WM_DELETEITEM's lParam is a DELETEITEMSTRUCT.
        WM_DELETEITEM => unsafe { read::<DELETEITEMSTRUCT>(l) }
            .filter(|d| d.CtlType != ODT_MENU)
            .map(|d| d.hwndItem),
        // SAFETY: WM_COMPAREITEM's lParam is a COMPAREITEMSTRUCT.
        WM_COMPAREITEM => unsafe { read::<COMPAREITEMSTRUCT>(l) }.map(|c| c.hwndItem),
        WM_PARENTNOTIFY => {
            let event = u32::from(super::loword(w));
            (event == WM_CREATE || event == WM_DESTROY).then_some(l as HWND)
        }
        // DefWindowProc of the child clicked passes it to its parent.
        WM_CONTEXTMENU => Some(w as HWND),
        _ => None,
    };
    from.filter(|&f| !f.is_null() && f != hwnd)
}

/// `parent`'s child whose id is `id` (null for none).
fn dlg_item(parent: HWND, id: u32) -> HWND {
    // SAFETY: no pointers.
    unsafe { GetDlgItem(parent, id as i32) }
}

/// Holds `hwnd` busy until the answer drops if it is a control: a window
/// that is not one of goz-gui's own ([`super::is_goz_window`]), whose
/// procedure is code that may not survive being destroyed in the middle of
/// its processing. goz-gui's own procedures do: a running closure is kept
/// alive through its window's end, and the handles it goes on with are
/// checked.
pub(crate) fn hold_control(hwnd: HWND) -> Option<Hold> {
    if hwnd.is_null() || super::is_goz_window(hwnd) {
        return None;
    }
    hold(hwnd)
}

/// Holds the control a delivered message comes from busy ([`sender`]),
/// for as long as the procedure handed it runs.
///
/// # Safety
/// As for [`sender`].
pub(crate) unsafe fn hold_sender(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<Hold> {
    // SAFETY: as the caller promises.
    hold_control(unsafe { sender(hwnd, msg, w, l) }?)
}
