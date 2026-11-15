//! The menu bar's undocumented drawing messages. The menu bar has no dark
//! theme of its own; Windows draws it through these messages, which let a
//! window paint the bar and its items. The layout of their structures has
//! held since Windows 10 1809.

use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::HMENU;

use super::Msg;

/// The menu bar is to be painted: lParam is a [`UahMenu`].
pub const WM_UAHDRAWMENU: u32 = 0x0091;
/// A menu bar item is to be painted: lParam is a [`UahDrawMenuItem`].
pub const WM_UAHDRAWMENUITEM: u32 = 0x0092;

/// WM_UAHDRAWMENU's structure (UAHMENU).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahMenu {
    /// The menu bar's menu.
    pub hmenu: HMENU,
    /// The DC to paint it on.
    pub hdc: HDC,
    /// Undocumented flags.
    pub flags: u32,
}

/// The item part of WM_UAHDRAWMENUITEM's structure (UAHMENUITEM).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahMenuItem {
    /// The item's position in the bar, from 0.
    pub position: i32,
    /// Undocumented.
    pub metrics: [u32; 8],
    /// Undocumented.
    pub popup: [u32; 5],
}

/// WM_UAHDRAWMENUITEM's structure (UAHDRAWMENUITEM).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UahDrawMenuItem {
    /// What to draw, where and in which state.
    pub dis: DRAWITEMSTRUCT,
    /// The bar the item is in.
    pub um: UahMenu,
    /// The item.
    pub umi: UahMenuItem,
}

/// A delivered WM_UAHDRAWMENU's menu bar and DC (a copy); `None` for any
/// other message, or one goz-gui made.
pub fn draw_menu(m: &Msg) -> Option<UahMenu> {
    if m.id != WM_UAHDRAWMENU {
        return None;
    }
    // SAFETY: a delivered WM_UAHDRAWMENU's lParam points at the UAHMENU
    // laid out above (a message below WM_USER, the same for every window);
    // it is copied out, not kept.
    unsafe { m.view_lparam::<UahMenu>() }.map(|v| *v)
}

/// A delivered WM_UAHDRAWMENUITEM's item to paint (a copy); `None` for any
/// other message, or one goz-gui made.
pub fn draw_menu_item(m: &Msg) -> Option<UahDrawMenuItem> {
    if m.id != WM_UAHDRAWMENUITEM {
        return None;
    }
    // SAFETY: a delivered WM_UAHDRAWMENUITEM's lParam points at the
    // UAHDRAWMENUITEM laid out above; it is copied out, not kept.
    unsafe { m.view_lparam::<UahDrawMenuItem>() }.map(|v| *v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_message_hands_out_no_menu_bar() {
        let m = Msg::new(core::ptr::null_mut(), WM_UAHDRAWMENU, 0, 0x1000);
        assert!(draw_menu(&m).is_none());
        let m = Msg::new(core::ptr::null_mut(), WM_UAHDRAWMENUITEM, 0, 0x1000);
        assert!(draw_menu_item(&m).is_none());
    }
}
