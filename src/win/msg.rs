//! A message as a window, dialog or subclass procedure receives it, and the
//! structures its `lParam` points at.

use core::cell::Cell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, LVN_BEGINDRAG, LVN_BEGINLABELEDITW, LVN_BEGINRDRAG, LVN_COLUMNCLICK,
    LVN_DELETEALLITEMS, LVN_DELETEITEM, LVN_ENDLABELEDITW, LVN_GETDISPINFOW, LVN_HOTTRACK,
    LVN_INSERTITEM, LVN_ITEMACTIVATE, LVN_ITEMCHANGED, LVN_ITEMCHANGING, LVN_KEYDOWN,
    LVN_MARQUEEBEGIN, LVN_ODCACHEHINT, LVN_ODSTATECHANGED, LVN_SETDISPINFOW, MEASUREITEMSTRUCT,
    NM_CLICK, NM_CUSTOMDRAW, NM_DBLCLK, NM_RCLICK, NM_RDBLCLK, NMCUSTOMDRAW, NMHDR, NMITEMACTIVATE,
    NMLISTVIEW, NMLVCACHEHINT, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVKEYDOWN, NMLVODSTATECHANGE,
    NMTREEVIEWW, NMTTDISPINFOW, NMTVCUSTOMDRAW, NMTVGETINFOTIPW, NMTVKEYDOWN, TTN_GETDISPINFOW,
    TVN_BEGINDRAGW, TVN_BEGINRDRAGW, TVN_DELETEITEMW, TVN_GETINFOTIPW, TVN_ITEMEXPANDEDW,
    TVN_ITEMEXPANDINGW, TVN_KEYDOWN, TVN_SELCHANGEDW, TVN_SELCHANGINGW, TVN_SINGLEEXPAND,
};
use windows_sys::Win32::UI::Shell::DefSubclassProc;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, MINMAXINFO, WINDOWPOS, WM_COPYDATA, WM_DPICHANGED, WM_DRAWITEM, WM_DROPFILES,
    WM_GETMINMAXINFO, WM_MEASUREITEM, WM_NOTIFY, WM_PAINT, WM_SETTINGCHANGE, WM_WINDOWPOSCHANGED,
    WM_WINDOWPOSCHANGING,
};

use super::class_is;

/// The end of the range of goz-gui's private messages: the registered
/// messages (`RegisterWindowMessageW`) start here.
pub(crate) const APP_END: u32 = 0xc000;

/// Who a [`Msg`] came from, and so what its default processing is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Via {
    /// Made by goz-gui's code ([`Msg::new`]): no pointer in it is trusted
    /// and it has no default processing.
    Made,
    /// Delivered to a window procedure: the default is `DefWindowProcW`.
    Window,
    /// Delivered to a dialog procedure: the dialog manager's own handling
    /// follows an answer of 0, so there is no default to call.
    Dialog,
    /// Delivered to a subclass procedure: the default is the next one,
    /// through `DefSubclassProc`.
    Subclass,
}

/// A window message. The ones the system delivers come from the trampolines
/// in [`super::class`], [`super::dlg`] and [`super::subclass`]; only those
/// hand out what their `lParam` points at.
pub struct Msg {
    /// The window it is for.
    pub hwnd: HWND,
    /// Which message it is (WM_*).
    pub id: u32,
    /// Its wParam.
    pub w: WPARAM,
    /// Its lParam (read as a pointer only through the accessors).
    pub l: LPARAM,
    via: Via,
    /// Whether a [`View`] of the `lParam` is out: one at a time, so a view
    /// never aliases another, and none while the message is passed on.
    lent: Cell<bool>,
}

impl Msg {
    /// A message made by goz-gui itself, such as a command handled as if it
    /// had come from the menu. Its `lParam` is never read as a pointer, and
    /// [`Msg::def`] answers 0 for it.
    pub fn new(hwnd: HWND, id: u32, w: WPARAM, l: LPARAM) -> Msg {
        Msg {
            hwnd,
            id,
            w,
            l,
            via: Via::Made,
            lent: Cell::new(false),
        }
    }

    /// A message the system delivered to a procedure, as `via` says.
    ///
    /// # Safety
    /// The parameters are the ones the system delivered with `id` to the
    /// procedure now running for `hwnd`: what they point at is valid for the
    /// message's accessors, and the `Msg` does not outlive the call.
    pub(crate) unsafe fn delivered(hwnd: HWND, id: u32, w: WPARAM, l: LPARAM, via: Via) -> Msg {
        Msg {
            hwnd,
            id,
            w,
            l,
            via,
            lent: Cell::new(false),
        }
    }

    /// The default processing of the message: the window class's for a
    /// window procedure, the next procedure's for a subclass; 0 for a dialog
    /// procedure (whose answer of 0 lets the dialog manager handle it) and
    /// for a message goz-gui made.
    ///
    /// # Panics
    /// If a [`View`] of the message's `lParam` is still out: the default
    /// processing may write to what it points at.
    pub fn def(&self) -> LRESULT {
        assert!(!self.lent.get(), "a view of the message is still out");
        match self.via {
            // SAFETY: the parameters as the system delivered them, to the
            // window they were delivered to.
            Via::Window => unsafe { DefWindowProcW(self.hwnd, self.id, self.w, self.l) },
            // SAFETY: as above, from within the subclass procedure the
            // system called (the trampoline's call is still running: a
            // `Msg` does not outlive it).
            Via::Subclass => unsafe { DefSubclassProc(self.hwnd, self.id, self.w, self.l) },
            Via::Dialog | Via::Made => 0,
        }
    }

    /// From a subclass's handling of a WM_PAINT: the next procedure in the
    /// window's chain (in the end the window's own) paints into `hdc`
    /// instead of the window, as the common controls do with a WM_PAINT
    /// that carries a DC. 0 for any other message, and for one not
    /// delivered to a subclass.
    pub fn paint_next_into(&self, hdc: windows_sys::Win32::Graphics::Gdi::HDC) -> LRESULT {
        if self.id != WM_PAINT || self.via != Via::Subclass {
            return 0;
        }
        // SAFETY: WM_PAINT with a DC handle, which GDI checks, and no
        // lParam: no pointers. From within the subclass procedure the
        // system called, for the window it was called for.
        unsafe { DefSubclassProc(self.hwnd, WM_PAINT, hdc as usize, 0) }
    }

    /// The point a mouse message carries (signed, in client coordinates or
    /// the screen's, as the message defines).
    pub fn point(&self) -> POINT {
        POINT {
            x: (self.l & 0xffff) as i16 as i32,
            y: ((self.l >> 16) & 0xffff) as i16 as i32,
        }
    }

    /// A view of what this delivered message's `lParam` points at; `None`
    /// for a message goz-gui made and for a null `lParam`.
    ///
    /// # Safety
    /// For a delivered message, `lParam` points at a `T` that is valid for
    /// reads and writes while the procedure runs.
    ///
    /// # Panics
    /// If another view of the message is still out.
    pub(crate) unsafe fn view_lparam<T>(&self) -> Option<View<'_, T>> {
        if self.via == Via::Made {
            return None;
        }
        let ptr = NonNull::new(self.l as *mut T)?;
        Some(View::lend(ptr, &self.lent))
    }

    /// `WM_NOTIFY`'s notification.
    pub fn notify(&self) -> Option<Notify<'_>> {
        if self.id != WM_NOTIFY {
            return None;
        }
        // SAFETY: a delivered WM_NOTIFY's lParam is an NMHDR (or a structure
        // that starts with one); it is copied out, not kept.
        let hdr = unsafe { self.view_lparam::<NMHDR>() }.map(|v| *v)?;
        Some(Notify { msg: self, hdr })
    }

    /// `WM_DRAWITEM`'s DRAWITEMSTRUCT.
    pub fn draw_item(&self) -> Option<View<'_, DRAWITEMSTRUCT>> {
        if self.id != WM_DRAWITEM {
            return None;
        }
        // SAFETY: WM_DRAWITEM's lParam is a DRAWITEMSTRUCT.
        unsafe { self.view_lparam() }
    }

    /// `WM_MEASUREITEM`'s MEASUREITEMSTRUCT, for the item's size.
    pub fn measure_item(&self) -> Option<View<'_, MEASUREITEMSTRUCT>> {
        if self.id != WM_MEASUREITEM {
            return None;
        }
        // SAFETY: WM_MEASUREITEM's lParam is a MEASUREITEMSTRUCT.
        unsafe { self.view_lparam() }
    }

    /// `WM_GETMINMAXINFO`'s MINMAXINFO.
    pub fn min_max_info(&self) -> Option<View<'_, MINMAXINFO>> {
        if self.id != WM_GETMINMAXINFO {
            return None;
        }
        // SAFETY: WM_GETMINMAXINFO's lParam is a MINMAXINFO.
        unsafe { self.view_lparam() }
    }

    /// `WM_WINDOWPOSCHANGING`'s or `WM_WINDOWPOSCHANGED`'s WINDOWPOS.
    pub fn window_pos(&self) -> Option<View<'_, WINDOWPOS>> {
        if self.id != WM_WINDOWPOSCHANGING && self.id != WM_WINDOWPOSCHANGED {
            return None;
        }
        // SAFETY: both messages' lParam is a WINDOWPOS.
        unsafe { self.view_lparam() }
    }

    /// `WM_DPICHANGED`'s suggested window rectangle.
    pub fn dpi_rect(&self) -> Option<RECT> {
        if self.id != WM_DPICHANGED {
            return None;
        }
        // SAFETY: WM_DPICHANGED's lParam is a RECT; it is copied out.
        unsafe { self.view_lparam::<RECT>() }.map(|v| *v)
    }

    /// `WM_SETTINGCHANGE`'s setting name, when it has one (`"ImmersiveColorSet"`,
    /// a policy area, an environment change).
    pub fn setting(&self) -> Option<String> {
        if self.id != WM_SETTINGCHANGE || self.via == Via::Made || self.l == 0 {
            return None;
        }
        let p = self.l as *const u16;
        let mut units = Vec::new();
        // At most 32,767 units: no setting's name is near that long.
        for i in 0..32_767 {
            // SAFETY: WM_SETTINGCHANGE's lParam, when not null, is a
            // NUL-terminated string; this stops at its NUL.
            let unit = unsafe { *p.add(i) };
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        Some(String::from_utf16_lossy(&units))
    }

    /// Whether this is `WM_SETTINGCHANGE` for the setting `name`.
    pub fn setting_is(&self, name: &str) -> bool {
        self.setting().is_some_and(|s| s == name)
    }

    /// The paths of the files a `WM_DROPFILES` brings, while it is being
    /// delivered (goz-gui frees the drop once its procedures are done with
    /// it, see [`super::dropfiles`]).
    pub fn dropped_files(&self) -> Option<Vec<Vec<u16>>> {
        if self.id != WM_DROPFILES || self.via == Via::Made {
            return None;
        }
        super::dropfiles::paths(self.w)
    }

    /// `WM_COPYDATA`'s value and a copy of its data.
    pub fn copy_data(&self) -> Option<(usize, Vec<u8>)> {
        if self.id != WM_COPYDATA {
            return None;
        }
        // SAFETY: WM_COPYDATA's lParam is a COPYDATASTRUCT, which the system
        // has copied into this process for the length of the call; it is
        // copied out.
        let cds = unsafe { self.view_lparam::<COPYDATASTRUCT>() }.map(|v| *v)?;
        let data = if cds.lpData.is_null() || cds.cbData == 0 {
            Vec::new()
        } else {
            // SAFETY: `lpData` holds `cbData` bytes, as above.
            unsafe { core::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }
                .to_vec()
        };
        Some((cds.dwData, data))
    }
}

/// What a message's `lParam` points at, lent for as long as the procedure
/// runs: one view of a message at a time (see [`Msg::def`]).
pub struct View<'a, T> {
    ptr: NonNull<T>,
    lent: &'a Cell<bool>,
    _t: PhantomData<&'a mut T>,
}

impl<'a, T> View<'a, T> {
    /// # Panics
    /// If another view is out on `lent`.
    fn lend(ptr: NonNull<T>, lent: &'a Cell<bool>) -> View<'a, T> {
        assert!(!lent.replace(true), "a view of the message is already out");
        View {
            ptr,
            lent,
            _t: PhantomData,
        }
    }

    /// The raw pointer, for the functions here that pass the structure on.
    pub(crate) fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }
}

impl<T> Deref for View<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the pointer is what the message's lParam points at (see
        // `Msg::view_lparam`), and this is the only view of it.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for View<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`; `&mut self` makes this the only borrow.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for View<'_, T> {
    fn drop(&mut self) {
        self.lent.set(false);
    }
}

/// A `WM_NOTIFY` notification: its header (copied), and the structure it
/// came in by [`Notify::get`].
pub struct Notify<'a> {
    msg: &'a Msg,
    /// The notification's header, copied.
    pub hdr: NMHDR,
}

impl<'a> Notify<'a> {
    /// The notification's code (LVN_*, NM_*…).
    pub fn code(&self) -> u32 {
        self.hdr.code
    }

    /// The control that sent it.
    pub fn from(&self) -> HWND {
        self.hdr.hwndFrom
    }

    /// The sending control's id.
    pub fn id(&self) -> usize {
        self.hdr.idFrom
    }

    /// Whether the sending control's class is `class`.
    pub fn sender_is(&self, class: &str) -> bool {
        class_is(self.hdr.hwndFrom, class)
    }

    /// The notification's structure as a `T`, if this notification is one
    /// that comes with a `T`.
    pub fn get<T: NotifyStruct>(&self) -> Option<View<'a, T>> {
        if !T::comes_with(self) {
            return None;
        }
        // SAFETY: `comes_with` vouches that this notification's lParam is a
        // `T`.
        unsafe { self.msg.view_lparam() }
    }
}

/// A structure a `WM_NOTIFY` notification comes with.
///
/// # Safety
/// `comes_with` is true only for notifications whose lParam points at a
/// `Self`.
pub unsafe trait NotifyStruct {
    fn comes_with(n: &Notify) -> bool;
}

const LIST_VIEW: &str = "SysListView32";
const TREE_VIEW: &str = "SysTreeView32";
const TOOLTIP: &str = "tooltips_class32";

macro_rules! notify_struct {
    ($t:ty, any, [$($code:expr),+ $(,)?]) => {
        // SAFETY: the notifications listed come with this structure, from
        // any control.
        unsafe impl NotifyStruct for $t {
            fn comes_with(n: &Notify) -> bool {
                [$($code),+].contains(&n.code())
            }
        }
    };
    ($t:ty, $class:expr, [$($code:expr),+ $(,)?]) => {
        // SAFETY: the notifications listed come with this structure from a
        // control of this class (which the check of the class makes sure
        // of: the same code from another class may come with another).
        unsafe impl NotifyStruct for $t {
            fn comes_with(n: &Notify) -> bool {
                [$($code),+].contains(&n.code()) && n.sender_is($class)
            }
        }
    };
}

// Every custom draw structure starts with an NMCUSTOMDRAW.
notify_struct!(NMCUSTOMDRAW, any, [NM_CUSTOMDRAW]);
notify_struct!(NMLVCUSTOMDRAW, LIST_VIEW, [NM_CUSTOMDRAW]);
notify_struct!(NMTVCUSTOMDRAW, TREE_VIEW, [NM_CUSTOMDRAW]);
notify_struct!(
    NMLVDISPINFOW,
    LIST_VIEW,
    [
        LVN_GETDISPINFOW,
        LVN_SETDISPINFOW,
        LVN_BEGINLABELEDITW,
        LVN_ENDLABELEDITW
    ]
);
// NMITEMACTIVATE starts with the same fields as NMLISTVIEW, so the
// notifications that come with one may be read as the other.
notify_struct!(
    NMLISTVIEW,
    LIST_VIEW,
    [
        LVN_ITEMCHANGING,
        LVN_ITEMCHANGED,
        LVN_INSERTITEM,
        LVN_DELETEITEM,
        LVN_DELETEALLITEMS,
        LVN_COLUMNCLICK,
        LVN_BEGINDRAG,
        LVN_BEGINRDRAG,
        LVN_HOTTRACK,
        LVN_MARQUEEBEGIN,
        LVN_ITEMACTIVATE,
        NM_CLICK,
        NM_DBLCLK,
        NM_RCLICK,
        NM_RDBLCLK,
    ]
);
notify_struct!(
    NMITEMACTIVATE,
    LIST_VIEW,
    [LVN_ITEMACTIVATE, NM_CLICK, NM_DBLCLK, NM_RCLICK, NM_RDBLCLK]
);
notify_struct!(NMLVKEYDOWN, LIST_VIEW, [LVN_KEYDOWN]);
notify_struct!(NMLVODSTATECHANGE, LIST_VIEW, [LVN_ODSTATECHANGED]);
notify_struct!(NMLVCACHEHINT, LIST_VIEW, [LVN_ODCACHEHINT]);
notify_struct!(
    NMTREEVIEWW,
    TREE_VIEW,
    [
        TVN_SELCHANGINGW,
        TVN_SELCHANGEDW,
        TVN_ITEMEXPANDINGW,
        TVN_ITEMEXPANDEDW,
        TVN_BEGINDRAGW,
        TVN_BEGINRDRAGW,
        TVN_DELETEITEMW,
        TVN_SINGLEEXPAND,
    ]
);
notify_struct!(NMTVKEYDOWN, TREE_VIEW, [TVN_KEYDOWN]);
notify_struct!(NMTVGETINFOTIPW, TREE_VIEW, [TVN_GETINFOTIPW]);
notify_struct!(NMTTDISPINFOW, TOOLTIP, [TTN_GETDISPINFOW]);

/// Copies `text` into a notification's text buffer of `cap` units (a
/// display-info or info-tip structure's), cut to fit, NUL-terminated.
///
/// # Safety
/// `buf` is null or valid for writes of `cap` units.
pub(crate) unsafe fn write_text(buf: *mut u16, cap: i32, text: &[u16]) {
    let Ok(cap) = usize::try_from(cap) else {
        return;
    };
    if buf.is_null() || cap == 0 {
        return;
    }
    let n = text.len().min(cap - 1);
    // SAFETY: `n + 1 <= cap` units, as the caller promises are writable;
    // `text` is a separate Rust slice.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
}

impl View<'_, NMLVDISPINFOW> {
    /// Puts `text` in the list's buffer for the item's text (LVIF_TEXT),
    /// cut to the buffer's length.
    pub fn set_text(&mut self, text: &[u16]) {
        let item = self.item;
        // SAFETY: LVN_GETDISPINFOW's pszText is the list's buffer of
        // cchTextMax units.
        unsafe { write_text(item.pszText, item.cchTextMax, text) };
    }

    /// The text of LVN_ENDLABELEDITW (`None` when editing was cancelled).
    pub fn text(&self) -> Option<Vec<u16>> {
        // SAFETY: a list's pszText, when not null, is NUL-terminated.
        unsafe { read_nul(self.item.pszText) }
    }
}

impl View<'_, NMTVGETINFOTIPW> {
    /// Puts `text` in the tree's buffer for the item's tip.
    pub fn set_text(&mut self, text: &[u16]) {
        let (buf, cap) = (self.pszText, self.cchTextMax);
        // SAFETY: TVN_GETINFOTIPW's pszText is the tree's buffer of
        // cchTextMax units.
        unsafe { write_text(buf, cap, text) };
    }
}

/// The NUL-terminated string at `p`; `None` for null.
///
/// # Safety
/// `p` is null or points at a NUL-terminated UTF-16 string.
pub(crate) unsafe fn read_nul(p: *const u16) -> Option<Vec<u16>> {
    if p.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        // SAFETY: as the caller promises; this stops at the NUL.
        let unit = unsafe { *p.add(i) };
        if unit == 0 {
            return Some(out);
        }
        out.push(unit);
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_message_hands_out_no_pointers() {
        let rect = RECT::default();
        let m = Msg::new(
            core::ptr::null_mut(),
            WM_DPICHANGED,
            0,
            (&raw const rect) as LPARAM,
        );
        assert!(m.dpi_rect().is_none());
        assert!(m.notify().is_none());
        assert_eq!(m.def(), 0);
    }

    #[test]
    fn one_view_at_a_time() {
        let mut item = MEASUREITEMSTRUCT::default();
        // SAFETY: a MEASUREITEMSTRUCT, as WM_MEASUREITEM has, alive for the
        // test.
        let m = unsafe {
            Msg::delivered(
                core::ptr::null_mut(),
                WM_MEASUREITEM,
                0,
                (&raw mut item) as LPARAM,
                Via::Dialog,
            )
        };
        let mut v = m.measure_item().unwrap();
        v.itemHeight = 5;
        let again =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.measure_item().is_some()));
        assert!(again.is_err());
        drop(v);
        assert_eq!(m.measure_item().unwrap().itemHeight, 5);
    }

    #[test]
    fn text_is_cut_to_the_buffer() {
        let mut buf = [7u16; 4];
        // SAFETY: the buffer and its length.
        unsafe { write_text(buf.as_mut_ptr(), 4, &[1, 2, 3, 4, 5]) };
        assert_eq!(buf, [1, 2, 3, 0]);
    }

    #[test]
    fn a_mouse_point_is_signed() {
        let m = Msg::new(core::ptr::null_mut(), 0, 0, make(0xfffe, 3));
        assert_eq!((m.point().x, m.point().y), (-2, 3));
    }

    fn make(lo: u16, hi: u16) -> LPARAM {
        super::super::make_lparam(lo, hi)
    }
}
