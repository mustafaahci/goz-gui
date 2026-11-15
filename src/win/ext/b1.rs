//! Batch 1 of the move onto `win`: what the main window's core needs that
//! the modules above lacked. The undocumented menu-bar messages, the
//! header's HDM_LAYOUT, a tooltip's text as UTF-16, menus, hotkeys, and a
//! few user32 and GDI calls.

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    DRAW_TEXT_FORMAT, DT_MODIFYSTRING, DrawTextW, GetTextColor, GetTextExtentExPointW, HDC,
    RoundRect, SetDCPenColor,
};
use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, HDHITTESTINFO, HDLAYOUT, HDM_HITTEST, HDM_LAYOUT, TTM_UPDATETIPTEXTW,
    TTTOOLINFOW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::Shell::DragAcceptFiles;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, DrawMenuBar, GetCursorPos, GetInputState,
    GetMenuBarInfo, GetMenuItemInfoW, HICON, HMENU, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
    MENU_ITEM_FLAGS, MENUBARINFO, MENUITEMINFOW, MF_BITMAP, MF_OWNERDRAW, MF_SEPARATOR,
    MIIM_STRING, OBJID_MENU, SetMenuDefaultItem, TrackPopupMenuEx, WINDOWPOS, WM_SETCURSOR,
    WM_WINDOWPOSCHANGED,
};

use crate::win::{Msg, class, of_class, send_raw, tip, until_nul, wz, wz_units};

// The menu bar has no dark theme of its own. Windows draws it through these
// undocumented messages, which let the app paint the bar and its items; the
// layout of their structs has held since Windows 10 1809.

/// The menu bar is to be painted: lParam is a [`UahMenu`].
pub const WM_UAHDRAWMENU: u32 = 0x0091;
/// A menu bar item is to be painted: lParam is a [`UahDrawMenuItem`].
pub const WM_UAHDRAWMENUITEM: u32 = 0x0092;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahMenu {
    pub hmenu: HMENU,
    pub hdc: HDC,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahMenuItem {
    pub position: i32,
    pub metrics: [u32; 8],
    pub popup: [u32; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahDrawMenuItem {
    pub dis: DRAWITEMSTRUCT,
    pub um: UahMenu,
    pub umi: UahMenuItem,
}

/// `WM_UAHDRAWMENU`'s menu bar and DC (a copy).
pub fn uah_menu(m: &Msg) -> Option<UahMenu> {
    if m.id != WM_UAHDRAWMENU {
        return None;
    }
    // SAFETY: a delivered WM_UAHDRAWMENU's lParam points at the UAHMENU
    // laid out above (a message below WM_USER, the same for every window);
    // it is copied out, not kept.
    unsafe { m.view_lparam::<UahMenu>() }.map(|v| *v)
}

/// `WM_UAHDRAWMENUITEM`'s item to paint (a copy).
pub fn uah_draw_menu_item(m: &Msg) -> Option<UahDrawMenuItem> {
    if m.id != WM_UAHDRAWMENUITEM {
        return None;
    }
    // SAFETY: a delivered WM_UAHDRAWMENUITEM's lParam points at the
    // UAHDRAWMENUITEM laid out above; it is copied out, not kept.
    unsafe { m.view_lparam::<UahDrawMenuItem>() }.map(|v| *v)
}

/// HDM_LAYOUT as a header's procedure (or its subclass) gets it: `f`
/// changes where the header goes (the WINDOWPOS) and what of the parent's
/// rectangle is left under it (the RECT). `None` for any other message, a
/// window that is not a header, or a layout without its two structures.
pub fn header_layout<R>(m: &Msg, f: impl FnOnce(&mut WINDOWPOS, &mut RECT) -> R) -> Option<R> {
    if m.id != HDM_LAYOUT || !crate::win::class_is(m.hwnd, crate::win::header::CLASS) {
        return None;
    }
    // SAFETY: a delivered HDM_LAYOUT's lParam, for a header, is an HDLAYOUT.
    let view = unsafe { m.view_lparam::<HDLAYOUT>() }?;
    let (prc, pwpos) = (view.prc, view.pwpos);
    if prc.is_null() || pwpos.is_null() {
        return None;
    }
    // SAFETY: HDLAYOUT's two pointers are the sender's RECT and WINDOWPOS,
    // two objects apart from the HDLAYOUT and from each other, valid while
    // the header's procedure runs. They are lent to `f` only, while the view
    // is out (so the message cannot be passed on meanwhile).
    let (pos, rest) = unsafe { (&mut *pwpos, &mut *prc) };
    let r = f(pos, rest);
    drop(view);
    Some(r)
}

/// HDM_HITTEST's answer at `pt` (the header's client coordinates): the item
/// there, -1 for none. (`header::hit_test` gives the filled structure,
/// whose item a header may set even where it answers -1.)
pub fn header_item_at(h: HWND, pt: POINT) -> i32 {
    let mut ht = HDHITTESTINFO {
        pt,
        ..Default::default()
    };
    // SAFETY: HDM_HITTEST's lParam is an HDHITTESTINFO, in and out, alive
    // for the call; sent only to a header (or to null).
    unsafe {
        send_raw(
            of_class(h, crate::win::header::CLASS),
            HDM_HITTEST,
            0,
            (&raw mut ht) as LPARAM,
        ) as i32
    }
}

/// Sets the text of the tool `ti` names (its hwnd and uId) to `text`
/// (UTF-16; a NUL is added), as `tip::update_text` does for a `&str`.
pub fn tip_update_text_wide(tip: HWND, ti: &TTTOOLINFOW, text: &[u16]) {
    let z = wz_units(text);
    let mut t = *ti;
    t.cbSize = size_of::<TTTOOLINFOW>() as u32;
    t.lpReserved = core::ptr::null_mut();
    t.hinst = core::ptr::null_mut();
    t.lpszText = z.as_ptr().cast_mut();
    // SAFETY: TTM_UPDATETIPTEXTW's lParam is a TTTOOLINFOW, read (its text
    // copied) during the call, whose pointers are set above; sent only to a
    // tooltip (or to null).
    unsafe {
        send_raw(
            of_class(tip, tip::CLASS),
            TTM_UPDATETIPTEXTW,
            0,
            (&raw const t) as LPARAM,
        )
    };
}

/// Sends WM_WINDOWPOSCHANGED to `hwnd` with `wp`, as if it had moved there
/// (an owner-drawn list then measures its items again).
pub fn send_window_pos_changed(hwnd: HWND, wp: &WINDOWPOS) -> LRESULT {
    let mut pos = *wp;
    // SAFETY: WM_WINDOWPOSCHANGED (below WM_USER, the same for every
    // window) takes a WINDOWPOS of handles and integers: a copy, alive for
    // the call.
    unsafe { send_raw(hwnd, WM_WINDOWPOSCHANGED, 0, (&raw mut pos) as LPARAM) }
}

/// Sends WM_SETCURSOR to `hwnd` as a mouse message `mouse` over hit-test
/// area `hit` of window `over` would; whether the cursor was set.
pub fn send_set_cursor(hwnd: HWND, over: HWND, hit: u16, mouse: u16) -> LRESULT {
    // SAFETY: WM_SETCURSOR (below WM_USER, the same for every window) takes
    // a window handle and two words, no pointer.
    unsafe {
        send_raw(
            hwnd,
            WM_SETCURSOR,
            over as usize,
            crate::win::make_lparam(hit, mouse),
        )
    }
}

/// How the program that started this process asked for its first window
/// to be shown (STARTUPINFO's wShowWindow), if it asked.
pub fn startup_show_window() -> Option<u16> {
    let mut si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size; only its flags and show command
    // are read (its strings are the process's, not kept).
    unsafe { GetStartupInfoW(&mut si) };
    (si.dwFlags & STARTF_USESHOWWINDOW != 0).then_some(si.wShowWindow)
}

/// Icon resource `id` of the exe (one build.rs embeds) at `size` pixels;
/// null if there is none. The caller owns it (`gdi::destroy_icon`).
pub fn module_icon(id: u16, size: i32) -> HICON {
    // SAFETY: MAKEINTRESOURCE(id): the id travels as the name pointer's
    // address, below 0x10000, which LoadImageW reads as an id, never as a
    // string.
    unsafe {
        LoadImageW(
            class::module(),
            core::ptr::without_provenance(usize::from(id)),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    }
}

/// Whether the window takes files dropped on it (WM_DROPFILES).
pub fn drag_accept_files(hwnd: HWND, accept: bool) {
    // SAFETY: no pointers.
    unsafe { DragAcceptFiles(hwnd, accept.into()) };
}

/// The mouse pointer on the screen; `None` if it could not be read (a
/// secure desktop, say).
pub fn cursor_pos() -> Option<POINT> {
    let mut pt = POINT::default();
    // SAFETY: an out-struct.
    (unsafe { GetCursorPos(&mut pt) } != 0).then_some(pt)
}

/// Whether mouse or keyboard input waits in this thread's queue.
pub fn input_waiting() -> bool {
    // SAFETY: no pointers.
    unsafe { GetInputState() != 0 }
}

/// The clipboard's sequence number, which changes with its contents.
pub fn clipboard_sequence() -> u32 {
    // SAFETY: no pointers.
    unsafe { GetClipboardSequenceNumber() }
}

/// Registers hotkey `id` of `hwnd` (WM_HOTKEY): MOD_* `mods` and key `vk`;
/// false if another program has it.
pub fn register_hotkey(hwnd: HWND, id: i32, mods: u32, vk: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { RegisterHotKey(hwnd, id, mods, vk) != 0 }
}

/// Unregisters hotkey `id` of `hwnd` (one not registered is fine).
pub fn unregister_hotkey(hwnd: HWND, id: i32) {
    // SAFETY: no pointers.
    unsafe { UnregisterHotKey(hwnd, id) };
}

/// A new empty popup menu (null if it could not be made); the caller
/// destroys it ([`destroy_menu`]) unless a menu it is put in does.
pub fn create_popup_menu() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreatePopupMenu() }
}

/// Appends an item of text `text` and command `id` (with MF_POPUP, a
/// submenu's handle) to `menu`, with MF_* `flags`. Flags that would read the
/// text as something else (MF_BITMAP, MF_OWNERDRAW, MF_SEPARATOR) are
/// dropped.
pub fn append_menu_string(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) -> bool {
    let z = wz(text);
    let flags = flags & !(MF_BITMAP | MF_OWNERDRAW | MF_SEPARATOR);
    // SAFETY: a NUL-terminated string the menu copies during the call; with
    // the flags above it is read as a string.
    unsafe { AppendMenuW(menu, flags, id, z.as_ptr()) != 0 }
}

/// Appends a separator to `menu`.
pub fn append_menu_separator(menu: HMENU) -> bool {
    // SAFETY: a separator has no text.
    unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null()) != 0 }
}

/// Makes `item` (a command id, or a position with `by_position`) the menu's
/// default item, drawn bold.
pub fn set_menu_default_item(menu: HMENU, item: u32, by_position: bool) -> bool {
    // SAFETY: no pointers.
    unsafe { SetMenuDefaultItem(menu, item, u32::from(by_position)) != 0 }
}

/// Shows popup menu `menu` at `pt` (screen) for `owner` until it closes,
/// with TPM_* `flags`: the command chosen with TPM_RETURNCMD (0 for none),
/// else whether it showed.
pub fn track_popup_menu(menu: HMENU, flags: u32, pt: POINT, owner: HWND) -> i32 {
    // SAFETY: no exclusion rectangle; the rest are handles and integers.
    unsafe { TrackPopupMenuEx(menu, flags, pt.x, pt.y, owner, core::ptr::null()) }
}

/// Destroys a menu and its submenus; false if it could not.
pub fn destroy_menu(menu: HMENU) -> bool {
    // SAFETY: no pointers (user32 checks the handle).
    unsafe { DestroyMenu(menu) != 0 }
}

/// Redraws the window's menu bar.
pub fn draw_menu_bar(hwnd: HWND) {
    // SAFETY: no pointers.
    unsafe { DrawMenuBar(hwnd) };
}

/// The window's menu bar: where it is (`rcBar`, on the screen) and its
/// menu.
pub fn menu_bar_info(hwnd: HWND) -> Option<MENUBARINFO> {
    let mut mbi = MENUBARINFO {
        cbSize: size_of::<MENUBARINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size, of handles and integers.
    (unsafe { GetMenuBarInfo(hwnd, OBJID_MENU, 0, &mut mbi) } != 0).then_some(mbi)
}

/// The text of a menu's item (by position with `by_position`, else by
/// command id), up to 255 units; empty if it has none or is not there.
pub fn menu_item_text(menu: HMENU, item: u32, by_position: bool) -> Vec<u16> {
    let mut buf = [0u16; 256];
    let mut mii = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING,
        dwTypeData: buf.as_mut_ptr(),
        cch: buf.len() as u32 - 1,
        ..Default::default()
    };
    // SAFETY: an in-out struct with its size whose one pointer (MIIM_STRING
    // asks only for the text) is the buffer, of `cch` units and a NUL.
    unsafe { GetMenuItemInfoW(menu, item, by_position.into(), &mut mii) };
    until_nul(&buf).to_vec()
}

/// The DC's text colour.
pub fn text_color(hdc: HDC) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { GetTextColor(hdc) }
}

/// How many of `text`'s units fit in `max` pixels in the DC's font.
pub fn text_fit(hdc: HDC, text: &[u16], max: i32) -> i32 {
    let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
    let (mut fit, mut size) = (0, SIZE::default());
    // SAFETY: `len` units of `text`, read during the call; two out-values;
    // no array of widths.
    unsafe {
        GetTextExtentExPointW(
            hdc,
            text.as_ptr(),
            len,
            max,
            &mut fit,
            core::ptr::null_mut(),
            &mut size,
        )
    };
    fit
}

/// The colour of the DC's DC_PEN stock pen; the one before.
pub fn set_dc_pen_color(hdc: HDC, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { SetDCPenColor(hdc, c) }
}

/// A rectangle with corners rounded by an ellipse of `cx` × `cy`, outlined
/// in the DC's pen and filled with its brush.
pub fn round_rect(hdc: HDC, rc: &RECT, cx: i32, cy: i32) {
    // SAFETY: no pointers.
    unsafe { RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, cx, cy) };
}

/// DrawTextW of `text` in `rc` with DT_MODIFYSTRING added to `format`: the
/// text as drawn, with the ellipsis DT_END_ELLIPSIS or DT_PATH_ELLIPSIS
/// put in. With DT_CALCRECT nothing is drawn and `rc` comes back as the
/// text's extent.
pub fn draw_text_modified(
    hdc: HDC,
    text: &[u16],
    rc: &mut RECT,
    format: DRAW_TEXT_FORMAT,
) -> Vec<u16> {
    // DT_MODIFYSTRING may write up to four units more than the text, and a
    // NUL after them.
    let mut buf = text.to_vec();
    buf.extend_from_slice(&[0; 5]);
    let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
    // SAFETY: `len` units of the buffer, which has five more after them, as
    // DT_MODIFYSTRING needs; the rectangle is in-out.
    unsafe { DrawTextW(hdc, buf.as_mut_ptr(), len, rc, format | DT_MODIFYSTRING) };
    let end = until_nul(&buf).len();
    buf.truncate(end);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_message_hands_out_no_menu_bar() {
        let m = Msg::new(core::ptr::null_mut(), WM_UAHDRAWMENU, 0, 0x1000);
        assert!(uah_menu(&m).is_none());
        let m = Msg::new(core::ptr::null_mut(), WM_UAHDRAWMENUITEM, 0, 0x1000);
        assert!(uah_draw_menu_item(&m).is_none());
        let m = Msg::new(core::ptr::null_mut(), HDM_LAYOUT, 0, 0x1000);
        assert!(header_layout(&m, |_, _| ()).is_none());
    }

    #[test]
    fn a_long_path_is_shortened_in_its_own_buffer() {
        use windows_sys::Win32::Graphics::Gdi::{
            DT_CALCRECT, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_SINGLELINE,
        };
        let dc = crate::win::gdi::MemDc::compatible(core::ptr::null_mut());
        let path: Vec<u16> = r"C:\Users\someone\Documents\Projects\goz\src\window.rs"
            .encode_utf16()
            .collect();
        let all = text_fit(dc.hdc(), &path, i32::MAX);
        assert_eq!(all, path.len() as i32);
        let mut rc = RECT {
            right: crate::win::gdi::text_extent(dc.hdc(), &path).cx / 2,
            bottom: 1,
            ..Default::default()
        };
        let format = DT_SINGLELINE | DT_NOPREFIX | DT_PATH_ELLIPSIS | DT_CALCRECT;
        let short = draw_text_modified(dc.hdc(), &path, &mut rc, format);
        assert!(short.len() < path.len());
        let dots = [0x2e; 3];
        assert!(
            short.windows(3).any(|w| w == dots),
            "an ellipsis in the middle"
        );
        assert!(short.ends_with(&path[path.len() - 9..]), "the name kept");
    }
}
