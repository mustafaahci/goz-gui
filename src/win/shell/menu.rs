//! The Explorer context menu of some items, as the shell's handlers fill,
//! draw and run it.

use windows::Win32::Foundation::{
    HWND as WHWND, LPARAM as WLPARAM, LRESULT as WLRESULT, POINT as WPOINT, WPARAM as WWPARAM,
};
use windows::Win32::UI::Shell::{
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu, IContextMenu2,
    IContextMenu3,
};
use windows::Win32::UI::WindowsAndMessaging::HMENU as WHMENU;
use windows::core::{Interface, PCSTR, PCWSTR, PSTR};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HMENU, SW_SHOWNORMAL, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};

use crate::win::len_u32;
use crate::win::msg::Msg;

/// The Explorer context menu of some items (IContextMenu, and the versions
/// that draw their own items), from [`super::Folder::context_menu`].
pub struct ShellMenu {
    menu: IContextMenu,
    menu2: Option<IContextMenu2>,
    menu3: Option<IContextMenu3>,
}

impl ShellMenu {
    pub(super) fn new(menu: IContextMenu) -> ShellMenu {
        ShellMenu {
            menu2: menu.cast().ok(),
            menu3: menu.cast().ok(),
            menu,
        }
    }

    /// Has the handlers add their items to `menu` from position `at`, with
    /// ids from `first` to `last`, as CMF_* `flags` ask (QueryContextMenu);
    /// how many ids they used, `None` if it failed.
    pub fn query(&self, menu: HMENU, at: u32, first: u32, last: u32, flags: u32) -> Option<u32> {
        // SAFETY: a menu handle (user32 checks it, as the handlers add to
        // it); no pointers.
        let hr = unsafe {
            self.menu
                .QueryContextMenu(WHMENU(menu), at, first, last, flags)
        };
        // Success carries how many ids were used.
        hr.is_ok().then_some((hr.0 & 0xffff) as u32)
    }

    /// The canonical verb (`copy`, `delete`, `properties`…) of the item
    /// `offset` ids after the first, if its handler names one (GCS_VERBW).
    pub fn verb(&self, offset: usize) -> Option<String> {
        let mut buf = [0u16; 260];
        // SAFETY: a buffer of `len` code units for a Unicode verb; the
        // handler writes at most that many, NUL included.
        let ok = unsafe {
            self.menu
                .GetCommandString(
                    offset,
                    GCS_VERBW,
                    None,
                    PSTR(buf.as_mut_ptr().cast()),
                    len_u32(buf.len()),
                )
                .is_ok()
        };
        // A verb without its NUL is not taken.
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
    }

    /// Runs the item `offset` ids after the first for `owner`, as chosen at
    /// `pt` (screen), with CMIC_MASK_* `mask` (InvokeCommand with a
    /// CMINVOKECOMMANDINFOEX). False if the handler failed, or `offset` is
    /// not one an item can have (it travels as a resource id, under
    /// 0x10000).
    pub fn invoke(&self, owner: HWND, offset: u32, pt: POINT, mask: u32) -> bool {
        // MAKEINTRESOURCE: the offset travels as the pointer's address,
        // which only below 0x10000 is read as an id, not a string.
        if offset >= 0x1_0000 {
            return false;
        }
        let verb = offset as usize;
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: mask,
            hwnd: WHWND(owner),
            lpVerb: PCSTR(core::ptr::without_provenance(verb)),
            lpVerbW: PCWSTR(core::ptr::without_provenance(verb)),
            nShow: SW_SHOWNORMAL,
            ptInvoke: WPOINT { x: pt.x, y: pt.y },
            ..Default::default()
        };
        // SAFETY: the EX structure (its cbSize says so), which outlives the
        // call: its verbs are resource ids (checked above), and its other
        // pointers (parameters, directory, title) null, which every mask
        // allows. The handler may dispatch messages meanwhile.
        unsafe {
            self.menu
                .InvokeCommand((&raw const info).cast::<CMINVOKECOMMANDINFO>())
                .is_ok()
        }
    }

    /// A message of the menu while it is up, for the handlers that fill
    /// their submenus as they open (WM_INITMENUPOPUP, WM_MENUCHAR) or draw
    /// their own items (WM_MEASUREITEM, WM_DRAWITEM: only as the system
    /// delivered them); `Some` with the answer when one took it, `None` for
    /// any other message.
    pub fn message(&self, m: &Msg) -> Option<isize> {
        // The structure a delivered WM_MEASUREITEM or WM_DRAWITEM points
        // at, held as a view (kept from any other use) across the call.
        let measure;
        let draw;
        let l = match m.id {
            WM_MEASUREITEM => {
                measure = m.measure_item()?;
                measure.as_ptr() as isize
            }
            WM_DRAWITEM => {
                draw = m.draw_item()?;
                draw.as_ptr() as isize
            }
            // Their parameters are a menu handle and integers.
            WM_INITMENUPOPUP | WM_MENUCHAR => m.l,
            _ => return None,
        };
        let (w, l) = (WWPARAM(m.w), WLPARAM(l));
        if let Some(m3) = &self.menu3 {
            let mut result = WLRESULT(0);
            // SAFETY: the message's parameters as the system delivered them
            // (WM_MEASUREITEM's and WM_DRAWITEM's structure lent by the
            // view above, which the handler may write to), passed on while
            // the menu they are about is up; the answer comes into a local.
            unsafe { m3.HandleMenuMsg2(m.id, w, l, Some(&mut result)) }.ok()?;
            return Some(result.0);
        }
        let m2 = self.menu2.as_ref()?;
        // SAFETY: as for HandleMenuMsg2 above.
        unsafe { m2.HandleMenuMsg(m.id, w, l) }.ok()?;
        Some(0)
    }
}
