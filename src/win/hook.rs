//! Windows hooks for this thread (and the low-level keyboard hook), each a
//! closure that sees the events as Rust values: what a hook's `lParam`
//! points at is copied out before any closure runs.
//!
//! One system hook per kind and thread, put in for the first closure of its
//! kind and taken out with the last; every closure of a kind sees each
//! event, in the order they were set, until one answers it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CBT_CREATEWNDW, CBTACTIVATESTRUCT, CWPRETSTRUCT, CWPSTRUCT, CallNextHookEx, HC_ACTION,
    HCBT_ACTIVATE, HCBT_CREATEWND, HHOOK, HOOKPROC, KBDLLHOOKSTRUCT, MSG, PM_REMOVE,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_CALLWNDPROC, WH_CALLWNDPROCRET, WH_CBT,
    WH_GETMESSAGE, WH_KEYBOARD_LL, WINDOWS_HOOK_ID,
};

/// The kinds of hook goz-gui sets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// WH_CBT: windows made, activated…
    Cbt,
    /// WH_GETMESSAGE: messages taken from this thread's queue.
    GetMessage,
    /// WH_CALLWNDPROC: messages sent to this thread's windows, before.
    CallWndProc,
    /// WH_CALLWNDPROCRET: the same, after.
    CallWndProcRet,
    /// WH_KEYBOARD_LL: every key, system-wide (seen on this thread).
    KeyboardLl,
}

const KINDS: usize = 5;

impl Kind {
    fn index(self) -> usize {
        self as usize
    }

    fn id(self) -> WINDOWS_HOOK_ID {
        match self {
            Kind::Cbt => WH_CBT,
            Kind::GetMessage => WH_GETMESSAGE,
            Kind::CallWndProc => WH_CALLWNDPROC,
            Kind::CallWndProcRet => WH_CALLWNDPROCRET,
            Kind::KeyboardLl => WH_KEYBOARD_LL,
        }
    }

    fn proc(self) -> HOOKPROC {
        match self {
            Kind::Cbt => Some(cbt_proc),
            Kind::GetMessage => Some(get_message_proc),
            Kind::CallWndProc => Some(call_wnd_proc),
            Kind::CallWndProcRet => Some(call_wnd_proc_ret),
            Kind::KeyboardLl => Some(keyboard_ll_proc),
        }
    }
}

/// An event a hook sees, copied out of what the system passed.
#[derive(Clone, Copy)]
pub enum Event {
    /// HCBT_CREATEWND: window `hwnd` is being made, with these styles,
    /// under `parent`.
    Create {
        hwnd: HWND,
        style: u32,
        ex_style: u32,
        parent: HWND,
    },
    /// HCBT_ACTIVATE: window `hwnd` is about to be activated.
    Activate { hwnd: HWND, by_mouse: bool },
    /// Any other CBT event: its code and parameters (as numbers).
    Cbt { code: i32, w: WPARAM, l: LPARAM },
    /// A message taken from the queue (`removed`), or peeked at.
    Got { msg: MSG, removed: bool },
    /// A message sent to a window of this thread, before its procedure.
    Sent {
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
    },
    /// The same, after, with the procedure's answer.
    Returned {
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
        answer: LRESULT,
    },
    /// A key (WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP).
    Key {
        message: u32,
        vk: u32,
        scan: u32,
        flags: u32,
        time: u32,
    },
}

/// What handles a hook's events: `Some` answers the event (and the next
/// hooks do not see it), `None` passes it on.
type Handler = Rc<dyn Fn(&Event) -> Option<LRESULT>>;

thread_local! {
    static HANDLERS: RefCell<Vec<(Kind, u64, Handler)>> = const { RefCell::new(Vec::new()) };
    static SYSTEM: RefCell<[HHOOK; KINDS]> =
        const { RefCell::new([core::ptr::null_mut(); KINDS]) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// A hook closure, taken out when this drops (on the thread that set it:
/// not `Send`).
pub struct Hook {
    kind: Kind,
    id: u64,
    _not_send: core::marker::PhantomData<*mut ()>,
}

/// Sets `handler` as a hook of `kind` for this thread; `None` if the system
/// would not hook it.
pub fn set(kind: Kind, handler: impl Fn(&Event) -> Option<LRESULT> + 'static) -> Option<Hook> {
    let installed = SYSTEM.with_borrow(|s| !s[kind.index()].is_null());
    if !installed {
        let thread = if kind == Kind::KeyboardLl {
            0
        } else {
            super::sys::thread_id()
        };
        let module = if kind == Kind::KeyboardLl {
            super::class::module()
        } else {
            core::ptr::null_mut()
        };
        // SAFETY: the trampoline below for this kind; a thread hook for this
        // thread, or the low-level keyboard hook (which runs on this thread).
        let hook = unsafe { SetWindowsHookExW(kind.id(), kind.proc(), module, thread) };
        if hook.is_null() {
            return None;
        }
        SYSTEM.with_borrow_mut(|s| s[kind.index()] = hook);
    }
    let id = NEXT_ID.get();
    NEXT_ID.set(id + 1);
    HANDLERS.with_borrow_mut(|h| h.push((kind, id, Rc::new(handler))));
    Some(Hook {
        kind,
        id,
        _not_send: core::marker::PhantomData,
    })
}

impl Drop for Hook {
    fn drop(&mut self) {
        // A hook kept in a thread-local may drop as the thread ends, after
        // these thread-locals have gone: then the thread's hooks go with it
        // and there is nothing left to undo.
        let Ok(left) = HANDLERS.try_with(|h| {
            let mut h = h.borrow_mut();
            h.retain(|&(_, id, _)| id != self.id);
            h.iter().any(|&(k, ..)| k == self.kind)
        }) else {
            return;
        };
        if !left {
            let hook = SYSTEM
                .try_with(|s| {
                    core::mem::replace(
                        &mut s.borrow_mut()[self.kind.index()],
                        core::ptr::null_mut(),
                    )
                })
                .unwrap_or(core::ptr::null_mut());
            if !hook.is_null() {
                // SAFETY: the hook `set` put in on this thread.
                unsafe { UnhookWindowsHookEx(hook) };
            }
        }
    }
}

/// Hands `event` to the closures of `kind`; the first answer, if any.
fn dispatch(kind: Kind, event: &Event) -> Option<LRESULT> {
    // Cloned first: a closure may set or drop hooks.
    let handlers: Vec<Handler> = HANDLERS.with_borrow(|h| {
        h.iter()
            .filter(|&&(k, ..)| k == kind)
            .map(|(_, _, f)| Rc::clone(f))
            .collect()
    });
    handlers.iter().find_map(|f| f(event))
}

fn next(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: passes the event on unchanged.
    unsafe { CallNextHookEx(core::ptr::null_mut(), code, w, l) }
}

unsafe extern "system" fn cbt_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    let event = if code == HCBT_CREATEWND as i32 && l != 0 {
        // SAFETY: HCBT_CREATEWND's lParam is a CBT_CREATEWNDW whose lpcs is
        // the CREATESTRUCTW of window `w`; both copied out.
        let cs = unsafe { *(*(l as *const CBT_CREATEWNDW)).lpcs };
        Event::Create {
            hwnd: w as HWND,
            style: cs.style as u32,
            ex_style: cs.dwExStyle,
            parent: cs.hwndParent,
        }
    } else if code == HCBT_ACTIVATE as i32 && l != 0 {
        // SAFETY: HCBT_ACTIVATE's lParam is a CBTACTIVATESTRUCT; copied out.
        let a = unsafe { *(l as *const CBTACTIVATESTRUCT) };
        Event::Activate {
            hwnd: w as HWND,
            by_mouse: a.fMouse != 0,
        }
    } else {
        Event::Cbt { code, w, l }
    };
    dispatch(Kind::Cbt, &event).unwrap_or_else(|| next(code, w, l))
}

unsafe extern "system" fn get_message_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && l != 0 {
        // SAFETY: WH_GETMESSAGE's lParam is the MSG; copied out.
        let msg = unsafe { *(l as *const MSG) };
        let event = Event::Got {
            msg,
            removed: w == PM_REMOVE as usize,
        };
        if let Some(answer) = dispatch(Kind::GetMessage, &event) {
            return answer;
        }
    }
    next(code, w, l)
}

unsafe extern "system" fn call_wnd_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && l != 0 {
        // SAFETY: WH_CALLWNDPROC's lParam is a CWPSTRUCT; copied out.
        let c = unsafe { *(l as *const CWPSTRUCT) };
        let event = Event::Sent {
            hwnd: c.hwnd,
            message: c.message,
            w: c.wParam,
            l: c.lParam,
        };
        if let Some(answer) = dispatch(Kind::CallWndProc, &event) {
            return answer;
        }
    }
    next(code, w, l)
}

unsafe extern "system" fn call_wnd_proc_ret(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && l != 0 {
        // SAFETY: WH_CALLWNDPROCRET's lParam is a CWPRETSTRUCT; copied out.
        let c = unsafe { *(l as *const CWPRETSTRUCT) };
        let event = Event::Returned {
            hwnd: c.hwnd,
            message: c.message,
            w: c.wParam,
            l: c.lParam,
            answer: c.lResult,
        };
        if let Some(answer) = dispatch(Kind::CallWndProcRet, &event) {
            return answer;
        }
    }
    next(code, w, l)
}

unsafe extern "system" fn keyboard_ll_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && l != 0 {
        // SAFETY: for HC_ACTION, lParam is a KBDLLHOOKSTRUCT; copied out.
        let k = unsafe { *(l as *const KBDLLHOOKSTRUCT) };
        let event = Event::Key {
            message: w as u32,
            vk: k.vkCode,
            scan: k.scanCode,
            flags: k.flags,
            time: k.time,
        };
        if let Some(answer) = dispatch(Kind::KeyboardLl, &event) {
            return answer;
        }
    }
    next(code, w, l)
}
