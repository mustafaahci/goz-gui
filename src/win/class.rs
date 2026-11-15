//! Window classes whose procedure is the trampoline here, and windows of
//! them, each made with the closure that handles its messages.

use std::cell::Cell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HBRUSH;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, HCURSOR,
    HICON, HMENU, RegisterClassExW, SetWindowLongPtrW, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW,
};

use super::msg::{Msg, Via};

/// What handles a window's messages. Kept in an `Rc` so a call that is
/// running keeps it alive through the window's destruction.
type Handler = Rc<dyn Fn(&Msg) -> LRESULT>;

/// A window class to register: its procedure is the trampoline, which
/// hands each message to the handler its window was made with ([`create`]).
pub struct Class<'a> {
    pub name: &'a str,
    /// CS_* flags (CS_DBLCLKS, CS_HREDRAW…).
    pub style: u32,
    pub cursor: HCURSOR,
    pub icon: HICON,
    pub icon_small: HICON,
    pub background: HBRUSH,
}

impl<'a> Class<'a> {
    /// A class of no style, cursor, icons or background.
    pub fn new(name: &'a str) -> Class<'a> {
        Class {
            name,
            style: 0,
            cursor: core::ptr::null_mut(),
            icon: core::ptr::null_mut(),
            icon_small: core::ptr::null_mut(),
            background: core::ptr::null_mut(),
        }
    }

    /// Registers the class for this module; false if it could not (one of
    /// that name is there already, say).
    pub fn register(&self) -> bool {
        let name = super::wz(self.name);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: self.style,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: module(),
            hIcon: self.icon,
            hCursor: self.cursor,
            hbrBackground: self.background,
            lpszMenuName: core::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: self.icon_small,
        };
        // SAFETY: a complete class whose name outlives the call (the system
        // copies it); its procedure is the trampoline below.
        unsafe { RegisterClassExW(&wc) != 0 }
    }
}

/// This module's handle.
pub fn module() -> windows_sys::Win32::Foundation::HINSTANCE {
    // SAFETY: null asks for the module of the process's exe.
    unsafe { GetModuleHandleW(core::ptr::null()) }
}

/// Where a window is made, and as what.
pub struct Create<'a> {
    pub ex_style: u32,
    /// A class registered with [`Class::register`].
    pub class: &'a str,
    pub title: &'a str,
    pub style: u32,
    /// x, y, width, height (CW_USEDEFAULT where the system is to choose).
    pub rect: (i32, i32, i32, i32),
    pub parent: HWND,
    /// The menu of a top-level window, or a child's control id
    /// ([`id_menu`]).
    pub menu: HMENU,
}

/// A child window's control id where CreateWindowExW takes a menu.
pub fn id_menu(id: usize) -> HMENU {
    core::ptr::without_provenance_mut(id)
}

/// Makes a window of any class (a control, say) with no handler of
/// goz-gui's: its own class's procedure handles it. Null if it could not be
/// made.
pub fn create_plain(c: &Create) -> HWND {
    let class = super::wz(c.class);
    let title = super::wz(c.title);
    let (x, y, w, h) = c.rect;
    // SAFETY: NUL-terminated names that outlive the call; no creation
    // parameter (a window of a class registered here then has no handler,
    // see `window_proc`).
    unsafe {
        CreateWindowExW(
            c.ex_style,
            class.as_ptr(),
            title.as_ptr(),
            c.style,
            x,
            y,
            w,
            h,
            c.parent,
            c.menu,
            module(),
            core::ptr::null(),
        )
    }
}

/// Waits at WM_NCCREATE for the trampoline to take the handler.
struct Pending {
    handler: Cell<Option<Handler>>,
}

/// Makes a window of a class registered with [`Class::register`], whose
/// messages go to `handler` from WM_NCCREATE through WM_NCDESTROY (the
/// default, [`Msg::def`], for the few that come before). Null if it could
/// not be made, and then `handler` is dropped.
pub fn create(c: &Create, handler: impl Fn(&Msg) -> LRESULT + 'static) -> HWND {
    let class = super::wz(c.class);
    let title = super::wz(c.title);
    let pending = Pending {
        handler: Cell::new(Some(Rc::new(handler))),
    };
    let (x, y, w, h) = c.rect;
    // SAFETY: NUL-terminated names that outlive the call; `pending` outlives
    // it too, and only the trampoline reads it (at WM_NCCREATE, during the
    // call).
    unsafe {
        CreateWindowExW(
            c.ex_style,
            class.as_ptr(),
            title.as_ptr(),
            c.style,
            x,
            y,
            w,
            h,
            c.parent,
            c.menu,
            module(),
            (&raw const pending).cast(),
        )
    }
    // A handler the window did not take (it was not made) drops with
    // `pending`.
}

/// The trampoline every class registered here has as its procedure.
///
/// A window's GWLP_USERDATA is a `Box<Handler>` from WM_NCCREATE until
/// WM_NCDESTROY frees it: nothing else in goz-gui writes these windows'
/// GWLP_USERDATA (the safe API has no way to).
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE && l != 0 {
        // SAFETY: WM_NCCREATE's lParam is a CREATESTRUCTW; a window of these
        // classes is made only by `create`, whose lpCreateParams is its
        // `Pending` (alive during CreateWindowExW), or by a dialog template,
        // which passes null for a control without creation data, which
        // goz-gui's templates never have.
        unsafe {
            let cs = &*(l as *const CREATESTRUCTW);
            let pending = cs.lpCreateParams as *const Pending;
            if let Some(handler) = pending.as_ref().and_then(|p| p.handler.take()) {
                let slot = Box::into_raw(Box::new(handler));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, slot as isize);
            }
        }
    }
    // SAFETY: GWLP_USERDATA is null or this window's `Box<Handler>` (see
    // above).
    let slot = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Handler;
    // SAFETY: as above; the handler is cloned, so it lives through the call
    // even if the call destroys the window and so frees the box.
    let Some(handler) = (unsafe { slot.as_ref() }).map(Rc::clone) else {
        // SAFETY: the parameters as delivered.
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    };
    // SAFETY: the parameters the system delivered, for this call.
    let m = unsafe { Msg::delivered(hwnd, msg, w, l, Via::Window) };
    let answer = handler(&m);
    if msg == WM_NCDESTROY {
        // SAFETY: the box set at WM_NCCREATE, taken off the window first so
        // nothing reaches it after it is freed.
        unsafe {
            let slot = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut Handler;
            if !slot.is_null() {
                drop(Box::from_raw(slot));
            }
        }
        super::parcel::discard_for(hwnd);
    }
    answer
}
