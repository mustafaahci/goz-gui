//! Window classes whose procedure is the trampoline here, and windows of
//! them, each made with the closure that handles its messages.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HBRUSH;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, HCURSOR,
    HICON, HMENU, RegisterClassExW, SetWindowLongPtrW, WM_NCCREATE, WM_NCDESTROY, WNDCLASSEXW,
    WS_EX_MDICHILD,
};

use super::msg::{Msg, Via};

/// What handles a window's messages. Kept in an `Rc` so a call that is
/// running keeps it alive through the window's destruction.
type Handler = Rc<dyn Fn(&Msg) -> LRESULT>;

/// A window class to register: its procedure is the trampoline, which
/// hands each message to the handler its window was made with ([`create`]).
pub struct Class<'a> {
    /// The class's name.
    pub name: &'a str,
    /// CS_* flags (CS_DBLCLKS, CS_HREDRAW…).
    pub style: u32,
    /// The cursor over its windows (null: each sets its own).
    pub cursor: HCURSOR,
    /// Its windows' large icon.
    pub icon: HICON,
    /// Its windows' small icon.
    pub icon_small: HICON,
    /// The brush its windows' background is erased with (null: none).
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
    /// that name is there already, say), or if its name starts with `#`
    /// (which names a class by its atom). Only a class registered here can
    /// have windows made with [`create`].
    pub fn register(&self) -> bool {
        if self.name.starts_with('#') {
            return false;
        }
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
        let ok = unsafe { RegisterClassExW(&wc) != 0 };
        if ok {
            REGISTERED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashSet::new)
                .insert(folded(self.name));
        }
        ok
    }
}

/// The names of the classes [`Class::register`] registered (their
/// procedure the trampoline), as [`folded`] names them.
static REGISTERED: Mutex<Option<HashSet<Vec<u16>>>> = Mutex::new(None);

/// `name` as the classes compare it, its ASCII letters in lower case. The
/// system compares class names ignoring case (all of it, not only
/// ASCII's): two names equal here are the same class to it.
fn folded(name: &str) -> Vec<u16> {
    name.encode_utf16().map(super::fold).collect()
}

/// Whether `name` is a class [`Class::register`] registered.
fn registered_here(name: &str) -> bool {
    REGISTERED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|set| set.contains(&folded(name)))
}

/// Whether this module has registered a window class `name`
/// (GetClassInfoExW).
pub fn is_registered(name: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassInfoExW;
    let z = super::wz(name);
    let mut wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        ..Default::default()
    };
    // SAFETY: a NUL-terminated name read during the call; an out-struct
    // with its size (what it is given back is not kept).
    unsafe { GetClassInfoExW(module(), z.as_ptr(), &mut wc) != 0 }
}

/// This module's handle.
pub fn module() -> windows_sys::Win32::Foundation::HINSTANCE {
    // SAFETY: null asks for the module of the process's exe.
    unsafe { GetModuleHandleW(core::ptr::null()) }
}

/// Where a window is made, and as what.
pub struct Create<'a> {
    /// WS_EX_* styles.
    pub ex_style: u32,
    /// A class registered with [`Class::register`].
    pub class: &'a str,
    /// The window's text.
    pub title: &'a str,
    /// WS_* styles.
    pub style: u32,
    /// x, y, width, height (CW_USEDEFAULT where the system is to choose).
    pub rect: (i32, i32, i32, i32),
    /// The parent (or owner) window; HWND_MESSAGE for a message-only one.
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
/// made, and for an MDI client or child, which read their creation data
/// (none is passed) as a structure of their own, or a class named by its
/// atom (`#n`, which may be the MDI client's).
pub fn create_plain(c: &Create) -> HWND {
    // A name "#n" is the class whose atom is n, which may be any class.
    if c.class.starts_with('#')
        || super::same_class_name(c.class, "MDIClient")
        || c.ex_style & WS_EX_MDICHILD != 0
    {
        return core::ptr::null_mut();
    }
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
/// not be made, and then `handler` is dropped: for a class not registered
/// there (whose procedure would read the handler's address as its own
/// creation data), and for an MDI child (WS_EX_MDICHILD, whose creation
/// data the system wraps in an MDICREATESTRUCT).
pub fn create(c: &Create, handler: impl Fn(&Msg) -> LRESULT + 'static) -> HWND {
    if !registered_here(c.class) || c.ex_style & WS_EX_MDICHILD != 0 {
        return core::ptr::null_mut();
    }
    let class = super::wz(c.class);
    let title = super::wz(c.title);
    let pending = Pending {
        handler: Cell::new(Some(Rc::new(handler))),
    };
    let (x, y, w, h) = c.rect;
    // SAFETY: NUL-terminated names that outlive the call; `pending` outlives
    // it too, and only the trampoline reads it (at WM_NCCREATE, during the
    // call): the class is one `Class::register` registered for this module,
    // which CreateWindowExW finds first for this module's instance, so its
    // procedure is the trampoline.
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

/// Whether `hwnd`'s class's procedure is the trampoline here: a window
/// of a class [`Class::register`] registered in this process.
pub(crate) fn is_trampoline_window(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GCLP_WNDPROC, GetClassLongPtrW};
    // SAFETY: no pointers: the address of the window's class's procedure.
    let class = unsafe { GetClassLongPtrW(hwnd, GCLP_WNDPROC) };
    class != 0 && class == window_proc as *const () as usize
}

/// The trampoline every class registered here has as its procedure.
///
/// A window's GWLP_USERDATA is a `Box<Handler>` from WM_NCCREATE until
/// WM_NCDESTROY frees it: nothing else in goz-gui writes these windows'
/// GWLP_USERDATA (the safe API has no way to).
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE && l != 0 {
        // SAFETY: WM_NCCREATE's lParam is a CREATESTRUCTW. goz-gui makes a
        // window of these classes three ways, and its lpCreateParams is
        // null or a `Pending`: `create` passes its `Pending` (alive during
        // CreateWindowExW; never for an MDI child, whose lpCreateParams
        // would be an MDICREATESTRUCT); `create_plain` passes null; and a
        // dialog's control of the class gets null, as every control of a
        // `dlg::Template` has no creation data and no field of the template
        // can be misread as one (`Template::units`, `Template::with_styles`).
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
    // A drop is held until this call (and any it nests in) returns.
    let _drop = super::dropfiles::Delivery::enter(msg, w);
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
