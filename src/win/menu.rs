//! Menus: made, filled, read, changed and shown. user32 checks the menu
//! handles it is given, so they are plain values; text goes in and comes
//! out as Rust strings, and the item structures are built here (an item
//! made here never carries a value of its owner's, `dwItemData`).

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{HBITMAP, HDC};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODA_DRAWENTIRE, ODT_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CheckMenuItem, CreateMenu, CreatePopupMenu, DeleteMenu, DestroyMenu, DrawMenuBar,
    EnableMenuItem, GET_MENU_DEFAULT_ITEM_FLAGS, GetMenuBarInfo, GetMenuDefaultItem,
    GetMenuItemCount, GetMenuItemID, GetMenuItemInfoW, GetMenuState, GetMenuStringW, GetSystemMenu,
    HBMMENU_CALLBACK, HMENU, InsertMenuItemW, MENU_ITEM_FLAGS, MENU_ITEM_STATE, MENU_ITEM_TYPE,
    MENUBARINFO, MENUITEMINFOW, MF_BYCOMMAND, MF_BYPOSITION, MF_CHECKED, MF_DISABLED, MF_ENABLED,
    MF_GRAYED, MF_UNCHECKED, MFT_OWNERDRAW, MFT_RADIOCHECK, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP,
    MIIM_DATA, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, OBJECT_IDENTIFIER,
    SetMenu, SetMenuDefaultItem, SetMenuItemInfoW, TRACK_POPUP_MENU_FLAGS, TrackPopupMenuEx,
    WM_DRAWITEM, WM_MEASUREITEM,
};

use super::{send_raw, until_nul, wz};

/// The longest text read back from an item, in UTF-16 units.
const TEXT_MAX: usize = 32_768;

/// Which item of a menu: by its place (from 0), or by the command it
/// runs (the first item that does, its submenus searched too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// The item at this place, from 0.
    Position(u32),
    /// The first item that runs this command.
    Command(u32),
}

impl By {
    /// The item and the MF_BY* flag of the functions that take one.
    fn flagged(self) -> (u32, MENU_ITEM_FLAGS) {
        match self {
            By::Position(p) => (p, MF_BYPOSITION),
            By::Command(c) => (c, MF_BYCOMMAND),
        }
    }

    /// The item and the fByPosition of the functions that take one.
    fn by_position(self) -> (u32, i32) {
        match self {
            By::Position(p) => (p, 1),
            By::Command(c) => (c, 0),
        }
    }
}

fn info(mask: u32) -> MENUITEMINFOW {
    MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: mask,
        ..Default::default()
    }
}

/// A new, empty popup menu (null if it could not be made). It is the
/// caller's to destroy, unless it goes into another menu as a submenu.
pub fn create_popup() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreatePopupMenu() }
}

/// A new, empty menu bar (null if it could not be made).
pub fn create_bar() -> HMENU {
    // SAFETY: no pointers.
    unsafe { CreateMenu() }
}

/// Destroys the menu and its submenus; false if it could not.
pub fn destroy(menu: HMENU) -> bool {
    // SAFETY: no pointers (user32 checks the handle; an item's owner
    // value is a number to it, never freed).
    unsafe { DestroyMenu(menu) != 0 }
}

/// A menu item to put in a menu ([`insert`], [`append`]).
#[derive(Clone, Copy, Debug)]
pub struct NewItem<'a> {
    /// `None`: a separator.
    text: Option<&'a str>,
    id: Option<u32>,
    submenu: HMENU,
    radio: bool,
    state: Option<MENU_ITEM_STATE>,
}

impl<'a> NewItem<'a> {
    /// An item that runs command `id`, labelled `text` (`&` before its
    /// access key, a tab before its shortcut).
    pub fn command(id: u32, text: &'a str) -> NewItem<'a> {
        NewItem {
            id: Some(id),
            ..NewItem::text(text)
        }
    }

    /// Text that runs nothing (greyed with `state(MFS_GRAYED)`, as an
    /// empty submenu shows "(Empty)").
    pub fn text(text: &'a str) -> NewItem<'a> {
        NewItem {
            text: Some(text),
            ..NewItem::separator()
        }
    }

    /// A separator.
    pub fn separator() -> NewItem<'a> {
        NewItem {
            text: None,
            id: None,
            submenu: core::ptr::null_mut(),
            radio: false,
            state: None,
        }
    }

    /// Submenu `menu` under `text`; the menu then goes with the item
    /// (destroyed with it).
    pub fn submenu(text: &'a str, menu: HMENU) -> NewItem<'a> {
        NewItem {
            submenu: menu,
            ..NewItem::text(text)
        }
    }

    /// Checked, it shows a dot rather than a check mark: one of a group
    /// of which one is chosen.
    pub fn radio(self) -> NewItem<'a> {
        NewItem {
            radio: true,
            ..self
        }
    }

    /// Its state to begin with (MFS_CHECKED, MFS_GRAYED, MFS_DEFAULT…).
    pub fn state(self, state: MENU_ITEM_STATE) -> NewItem<'a> {
        NewItem {
            state: Some(state),
            ..self
        }
    }
}

/// Puts `item` at position `pos` of `menu` (`u32::MAX`, or any place
/// past the end: last); false if it could not.
pub fn insert(menu: HMENU, pos: u32, item: &NewItem<'_>) -> bool {
    let mut text = item.text.map(wz);
    let mut mii = info(MIIM_FTYPE);
    match text.as_mut() {
        Some(t) => {
            mii.fMask |= MIIM_STRING;
            mii.fType = if item.radio {
                MFT_STRING | MFT_RADIOCHECK
            } else {
                MFT_STRING
            };
            mii.dwTypeData = t.as_mut_ptr();
        }
        None => mii.fType = MFT_SEPARATOR,
    }
    if let Some(id) = item.id {
        mii.fMask |= MIIM_ID;
        mii.wID = id;
    }
    if !item.submenu.is_null() {
        mii.fMask |= MIIM_SUBMENU;
        mii.hSubMenu = item.submenu;
    }
    if let Some(state) = item.state {
        mii.fMask |= MIIM_STATE;
        mii.fState = state;
    }
    // SAFETY: an in-struct with its size, whose text (when it has one)
    // is a NUL-terminated string alive for the call (the system copies
    // it); its other fields are numbers and handles.
    unsafe { InsertMenuItemW(menu, pos, 1, &mii) != 0 }
}

/// Puts `item` last in `menu`; false if it could not.
pub fn append(menu: HMENU, item: &NewItem<'_>) -> bool {
    insert(menu, u32::MAX, item)
}

/// The number of items in `menu`; -1 if it is not a menu.
pub fn count(menu: HMENU) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetMenuItemCount(menu) }
}

/// The command of the item at `pos`; `u32::MAX` for a submenu or no
/// item.
pub fn item_id(menu: HMENU, pos: i32) -> u32 {
    // SAFETY: no pointers.
    unsafe { GetMenuItemID(menu, pos) }
}

/// Deletes an item; its submenu is destroyed with it. False if there is
/// no such item.
pub fn delete(menu: HMENU, by: By) -> bool {
    let (item, flag) = by.flagged();
    // SAFETY: no pointers.
    unsafe { DeleteMenu(menu, item, flag) != 0 }
}

/// Checks or unchecks an item; whether it was checked, `None` if there
/// is no such item.
pub fn check(menu: HMENU, by: By, on: bool) -> Option<bool> {
    let (item, flag) = by.flagged();
    let mark = if on { MF_CHECKED } else { MF_UNCHECKED };
    // SAFETY: no pointers.
    let was = unsafe { CheckMenuItem(menu, item, flag | mark) };
    (was != u32::MAX).then_some(was & MF_CHECKED != 0)
}

/// Enables an item, or greys it; whether it was enabled, `None` if
/// there is no such item.
pub fn enable(menu: HMENU, by: By, on: bool) -> Option<bool> {
    let (item, flag) = by.flagged();
    let state = if on { MF_ENABLED } else { MF_GRAYED };
    // SAFETY: no pointers.
    let was = unsafe { EnableMenuItem(menu, item, flag | state) };
    (was != -1).then_some(was as u32 & (MF_GRAYED | MF_DISABLED) == 0)
}

/// An item's MF_* state (GetMenuState); `None` if there is no such item.
pub fn state(menu: HMENU, by: By) -> Option<u32> {
    let (item, flag) = by.flagged();
    // SAFETY: no pointers.
    let state = unsafe { GetMenuState(menu, item, flag) };
    (state != u32::MAX).then_some(state)
}

/// Makes an item the menu's default, shown in bold (`By::Command(u32::MAX)`:
/// none); false if it could not.
pub fn set_default(menu: HMENU, by: By) -> bool {
    let (item, by_pos) = by.by_position();
    // SAFETY: no pointers.
    unsafe { SetMenuDefaultItem(menu, item, by_pos as u32) != 0 }
}

/// The menu's default item, as its command (or its position with
/// `by_position`), with GMDI_* `flags`; `None` if it has none.
pub fn default_item(
    menu: HMENU,
    by_position: bool,
    flags: GET_MENU_DEFAULT_ITEM_FLAGS,
) -> Option<u32> {
    // SAFETY: no pointers.
    let item = unsafe { GetMenuDefaultItem(menu, u32::from(by_position), flags) };
    (item != u32::MAX).then_some(item)
}

/// An item's MFT_* type (MFT_SEPARATOR, MFT_RADIOCHECK…); `None` if
/// there is no such item.
pub fn kind(menu: HMENU, by: By) -> Option<MENU_ITEM_TYPE> {
    let (item, by_pos) = by.by_position();
    let mut mii = info(MIIM_FTYPE);
    // SAFETY: an out-struct with its size; MIIM_FTYPE alone reads no
    // text, so no buffer is passed.
    let ok = unsafe { GetMenuItemInfoW(menu, item, by_pos, &mut mii) } != 0;
    ok.then_some(mii.fType)
}

/// Whether an item is a separator (false if there is no such item).
pub fn is_separator(menu: HMENU, by: By) -> bool {
    kind(menu, by).is_some_and(|t| t & MFT_SEPARATOR != 0)
}

/// What a menu item is, read from its menu ([`item_info`]).
#[derive(Clone, Debug)]
pub struct ItemInfo {
    /// MFT_* flags (MFT_SEPARATOR, MFT_RADIOCHECK, MFT_OWNERDRAW…).
    pub kind: MENU_ITEM_TYPE,
    /// MFS_* flags (MFS_CHECKED, MFS_DISABLED, MFS_DEFAULT…).
    pub state: MENU_ITEM_STATE,
    /// The command it runs.
    pub id: u32,
    /// Its submenu (null for none).
    pub submenu: HMENU,
    /// Its bitmap: null, a bitmap, or one of the HBMMENU_* values.
    pub bitmap: HBITMAP,
    /// Its text (`&` before its access key, a tab before its shortcut);
    /// empty for none.
    pub text: String,
}

impl ItemInfo {
    /// Whether it is a separator.
    pub fn is_separator(&self) -> bool {
        self.kind & MFT_SEPARATOR != 0
    }
}

/// Everything about an item; `None` if there is no such item.
pub fn item_info(menu: HMENU, by: By) -> Option<ItemInfo> {
    let (item, by_pos) = by.by_position();
    let mask = MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING | MIIM_BITMAP;
    let mut mii = info(mask);
    // SAFETY: an out-struct with its size and no text buffer (null), so
    // the call only says the text's length, in `cch`.
    if unsafe { GetMenuItemInfoW(menu, item, by_pos, &mut mii) } == 0 {
        return None;
    }
    // A unit to spare past what `cch` lets the call write.
    let len = (mii.cch as usize).min(TEXT_MAX);
    let mut buf = vec![0u16; len + 2];
    let mut mii = MENUITEMINFOW {
        dwTypeData: buf.as_mut_ptr(),
        cch: (len + 1) as u32,
        ..info(mask)
    };
    // SAFETY: an out-struct with its size, and a text buffer of more
    // than `cch` units that lives across the call.
    if unsafe { GetMenuItemInfoW(menu, item, by_pos, &mut mii) } == 0 {
        return None;
    }
    Some(ItemInfo {
        kind: mii.fType,
        state: mii.fState,
        id: mii.wID,
        submenu: mii.hSubMenu,
        bitmap: mii.hbmpItem,
        text: String::from_utf16_lossy(until_nul(&buf)),
    })
}

/// An item's text as UTF-16, without a NUL (empty for none, or no such
/// item).
pub fn text_units(menu: HMENU, by: By) -> Vec<u16> {
    let (item, flag) = by.flagged();
    // SAFETY: no buffer (null, of no length): the call gives the
    // text's length.
    let len = unsafe { GetMenuStringW(menu, item, core::ptr::null_mut(), 0, flag) };
    let len = usize::try_from(len).unwrap_or(0).min(TEXT_MAX);
    if len == 0 {
        return Vec::new();
    }
    // A unit to spare past what the length passed lets the call write.
    let mut buf = vec![0u16; len + 2];
    // SAFETY: the buffer, longer than the length passed, alive for the
    // call.
    unsafe { GetMenuStringW(menu, item, buf.as_mut_ptr(), (len + 1) as i32, flag) };
    let end = until_nul(&buf).len();
    buf.truncate(end);
    buf
}

/// Changes an item's text, its MFS_* state, or both (`None` leaves
/// one as it is); false if it could not.
pub fn set_item(menu: HMENU, by: By, text: Option<&str>, state: Option<MENU_ITEM_STATE>) -> bool {
    let (item, by_pos) = by.by_position();
    let mut text = text.map(wz);
    let mut mii = info(0);
    if let Some(t) = text.as_mut() {
        mii.fMask |= MIIM_STRING;
        mii.dwTypeData = t.as_mut_ptr();
    }
    if let Some(state) = state {
        mii.fMask |= MIIM_STATE;
        mii.fState = state;
    }
    // SAFETY: an in-struct with its size, whose text (when set) is a
    // NUL-terminated string alive for the call (the system copies it).
    unsafe { SetMenuItemInfoW(menu, item, by_pos, &mii) != 0 }
}

/// Changes an item's text; false if it could not.
pub fn set_text(menu: HMENU, by: By, text: &str) -> bool {
    set_item(menu, by, Some(text), None)
}

/// Shows `menu` at `pt` (on the screen) for `owner`, which gets its
/// messages, until it closes (TrackPopupMenuEx with TPM_* `flags`).
/// With TPM_RETURNCMD, the command chosen (0 for none); otherwise
/// nonzero if it was shown.
pub fn track(menu: HMENU, flags: TRACK_POPUP_MENU_FLAGS, pt: POINT, owner: HWND) -> i32 {
    // SAFETY: no pointers (no TPMPARAMS). The menu loop sends `owner`
    // the menu's messages, as the system defines them.
    unsafe { TrackPopupMenuEx(menu, flags, pt.x, pt.y, owner, core::ptr::null()) }
}

/// The command and the owner's value of the item at `pos`, if it is one
/// the menu loop asks its owner to measure and draw: drawn by its owner
/// (MFT_OWNERDRAW), or its picture is (HBMMENU_CALLBACK).
fn owner_drawn(menu: HMENU, pos: u32) -> Option<(u32, usize)> {
    let mut mii = info(MIIM_FTYPE | MIIM_BITMAP | MIIM_ID | MIIM_DATA);
    // SAFETY: an out-struct with its size; no text is read.
    let ok = unsafe { GetMenuItemInfoW(menu, pos, 1, &mut mii) } != 0;
    let drawn = mii.fType & MFT_OWNERDRAW != 0 || core::ptr::eq(mii.hbmpItem, HBMMENU_CALLBACK);
    (ok && drawn).then_some((mii.wID, mii.dwItemData))
}

/// Whether `root` or a menu under it (`within`, when given: that one only,
/// if it is `root` or under it) has an item its owner draws whose command
/// is `id` and whose owner's value is `data`.
pub(crate) fn holds_drawn_item(root: HMENU, within: Option<HMENU>, id: u32, data: usize) -> bool {
    /// How deep submenus are looked into (menus nest a few levels).
    const DEPTH: usize = 16;
    let mut todo = vec![(root, 0)];
    let mut seen = Vec::new();
    while let Some((menu, depth)) = todo.pop() {
        if menu.is_null() || depth > DEPTH || seen.contains(&menu) {
            continue;
        }
        seen.push(menu);
        let n = u32::try_from(count(menu)).unwrap_or(0);
        for pos in 0..n {
            if within.is_none_or(|w| w == menu) && owner_drawn(menu, pos) == Some((id, data)) {
                return true;
            }
            let mut mii = info(MIIM_SUBMENU);
            // SAFETY: an out-struct with its size; no text is read.
            if unsafe { GetMenuItemInfoW(menu, pos, 1, &mut mii) } != 0 {
                todo.push((mii.hSubMenu, depth + 1));
            }
        }
    }
    false
}

/// What `owner` measures item `pos` of `menu` at, asked as the menu loop
/// asks before it shows an item its owner draws (WM_MEASUREITEM, with
/// the item's command and value as the menu holds them): its width and
/// height. `None` if there is no such item, its owner does not draw it,
/// or `owner` is not one of goz-gui's own windows (whose procedure is a
/// trampoline: another window's may read the value as an address of its
/// own, for a menu that is not its).
pub fn measure_item(owner: HWND, menu: HMENU, pos: u32) -> Option<(u32, u32)> {
    if !super::is_goz_window(owner) {
        return None;
    }
    let (id, data) = owner_drawn(menu, pos)?;
    let mut mis = MEASUREITEMSTRUCT {
        CtlType: ODT_MENU,
        CtlID: 0,
        itemID: id,
        itemWidth: 0,
        itemHeight: 0,
        itemData: data,
    };
    // SAFETY: WM_MEASUREITEM's lParam is a MEASUREITEMSTRUCT: this one,
    // alive and writable for the call, to one of goz-gui's own windows. The
    // item is one its owner draws, and its command and value are the ones
    // `menu` holds for it: what the menu loop sends with this message to
    // the window it shows the menu for (see `track`), and what a handler
    // the window passes it on to checks is its own
    // (`shell::ShellMenu::message`). Its other fields are numbers.
    unsafe { send_raw(owner, WM_MEASUREITEM, 0, (&raw mut mis) as LPARAM) };
    Some((mis.itemWidth, mis.itemHeight))
}

/// Has `owner` draw item `pos` of `menu` into `hdc`, in `rc`, as the
/// menu loop asks (WM_DRAWITEM, the whole item, in no particular state,
/// with its command and value as the menu holds them). False if there
/// is no such item, its owner does not draw it, or `owner` is not one of
/// goz-gui's own windows (as for [`measure_item`]).
pub fn draw_item(owner: HWND, menu: HMENU, pos: u32, hdc: HDC, rc: RECT) -> bool {
    if !super::is_goz_window(owner) {
        return false;
    }
    let Some((id, data)) = owner_drawn(menu, pos) else {
        return false;
    };
    let mut dis = DRAWITEMSTRUCT {
        CtlType: ODT_MENU,
        CtlID: 0,
        itemID: id,
        itemAction: ODA_DRAWENTIRE,
        itemState: 0,
        hwndItem: menu as HWND,
        hDC: hdc,
        rcItem: rc,
        itemData: data,
    };
    // SAFETY: WM_DRAWITEM's lParam is a DRAWITEMSTRUCT: this one, alive
    // (and writable, as a handler may treat it) for the call, to one of
    // goz-gui's own windows. Its command and value are the ones `menu`
    // holds for the item, as in `measure_item`; the menu and the DC are
    // handles, the rest numbers.
    unsafe { send_raw(owner, WM_DRAWITEM, 0, (&raw mut dis) as LPARAM) };
    true
}

/// Gives the window menu bar `menu` (null: none), which then goes with
/// the window; the bar it had is not destroyed. False if it could not.
pub fn set_for_window(hwnd: HWND, menu: HMENU) -> bool {
    // SAFETY: no pointers.
    unsafe { SetMenu(hwnd, menu) != 0 }
}

/// Redraws the window's menu bar after its items changed.
pub fn draw_bar(hwnd: HWND) -> bool {
    // SAFETY: no pointers.
    unsafe { DrawMenuBar(hwnd) != 0 }
}

/// The window's system menu (`revert`: put back as the system makes
/// it, and null is given).
pub fn system_menu(hwnd: HWND, revert: bool) -> HMENU {
    // SAFETY: no pointers.
    unsafe { GetSystemMenu(hwnd, revert.into()) }
}

/// Where the window's menu bar (OBJID_MENU), system menu or item `item`
/// of one (0: the whole) is, and its menu.
pub fn bar_info(hwnd: HWND, object: OBJECT_IDENTIFIER, item: i32) -> Option<MENUBARINFO> {
    let mut mbi = MENUBARINFO {
        cbSize: size_of::<MENUBARINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct with its size.
    (unsafe { GetMenuBarInfo(hwnd, object, item, &mut mbi) } != 0).then_some(mbi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MFS_CHECKED, MFS_GRAYED};

    /// An item's text, as a string.
    fn text(menu: HMENU, by: By) -> String {
        String::from_utf16_lossy(&text_units(menu, by))
    }

    #[test]
    fn items_go_in_and_come_back() {
        let menu = create_popup();
        let sub = create_popup();
        assert!(append(sub, &NewItem::text("(Empty)").state(MFS_GRAYED)));
        assert!(append(menu, &NewItem::command(7, "&Open\tEnter")));
        assert!(append(menu, &NewItem::separator()));
        assert!(append(menu, &NewItem::command(8, "&Dot").radio()));
        assert!(append(menu, &NewItem::submenu("Se&nd to", sub)));
        assert!(insert(menu, 0, &NewItem::command(9, "First")));
        assert_eq!(count(menu), 5);
        assert_eq!(text(menu, By::Position(0)), "First");
        assert_eq!(text(menu, By::Command(7)), "&Open\tEnter");
        assert!(is_separator(menu, By::Position(2)));
        assert!(!is_separator(menu, By::Position(1)));
        assert_eq!(item_id(menu, 1), 7);
        let dot = item_info(menu, By::Command(8)).unwrap();
        assert!(dot.kind & MFT_RADIOCHECK != 0 && dot.text == "&Dot");
        let send = item_info(menu, By::Position(4)).unwrap();
        assert_eq!(send.submenu, sub);
        let empty = item_info(sub, By::Position(0)).unwrap();
        assert!(empty.state & MFS_GRAYED != 0 && empty.text == "(Empty)");
        assert!(item_info(menu, By::Position(9)).is_none());
        assert_eq!(text(menu, By::Command(99)), "");
        assert!(destroy(menu));
        assert!(!destroy(sub), "destroyed with its menu");
    }

    #[test]
    fn items_change() {
        let menu = create_popup();
        append(menu, &NewItem::command(1, "One"));
        append(menu, &NewItem::command(2, "Two"));
        assert_eq!(check(menu, By::Command(1), true), Some(false));
        assert_eq!(check(menu, By::Command(1), false), Some(true));
        assert_eq!(check(menu, By::Command(5), true), None);
        assert_eq!(enable(menu, By::Command(2), false), Some(true));
        assert_eq!(enable(menu, By::Command(2), true), Some(false));
        assert!(state(menu, By::Command(2)).is_some());
        assert!(state(menu, By::Command(5)).is_none());
        assert!(set_item(
            menu,
            By::Command(2),
            Some("Deux"),
            Some(MFS_CHECKED)
        ));
        assert_eq!(text(menu, By::Position(1)), "Deux");
        assert!(item_info(menu, By::Command(2)).unwrap().state & MFS_CHECKED != 0);
        assert_eq!(default_item(menu, false, 0), None);
        assert!(set_default(menu, By::Command(2)));
        assert_eq!(default_item(menu, false, 0), Some(2));
        assert!(delete(menu, By::Command(1)));
        assert!(!delete(menu, By::Command(1)));
        assert_eq!(count(menu), 1);
        assert_eq!(count(core::ptr::null_mut()), -1);
        assert!(destroy(menu));
    }

    #[test]
    fn an_item_its_owner_draws_goes_to_the_owner_as_the_menu_holds_it() {
        use std::cell::Cell;
        use std::rc::Rc;
        use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_MESSAGE, MFT_OWNERDRAW};

        use crate::win::class::{self, Class, Create};

        let menu = create_popup();
        let mii = MENUITEMINFOW {
            fType: MFT_OWNERDRAW,
            wID: 42,
            dwItemData: 7,
            ..info(MIIM_FTYPE | MIIM_ID | MIIM_DATA)
        };
        // SAFETY: an in-struct with its size, of numbers (no text).
        assert!(unsafe { InsertMenuItemW(menu, 0, 1, &mii) } != 0);
        let seen = Rc::new(Cell::new((0u32, 0usize, core::ptr::null_mut())));
        let got = Rc::clone(&seen);
        let _ = Class::new("goz-gui menu owner").register();
        let owner = class::create(
            &Create {
                ex_style: 0,
                class: "goz-gui menu owner",
                title: "",
                style: 0,
                rect: (0, 0, 0, 0),
                parent: HWND_MESSAGE,
                menu: core::ptr::null_mut(),
            },
            move |m| {
                if let Some(mut mis) = m.measure_item() {
                    mis.itemWidth = 30;
                    mis.itemHeight = 12;
                    got.set((mis.itemID, mis.itemData, core::ptr::null_mut()));
                    return 1;
                }
                if let Some(dis) = m.draw_item() {
                    got.set((dis.itemID, dis.itemData, dis.hwndItem));
                    return 1;
                }
                m.def()
            },
        );
        assert!(!owner.is_null());
        assert_eq!(measure_item(owner, menu, 0), Some((30, 12)));
        assert_eq!(seen.get(), (42, 7, core::ptr::null_mut()));
        let canvas = crate::win::dib::Canvas::new(30, 12).unwrap();
        assert!(draw_item(owner, menu, 0, canvas.hdc(), RECT::default()));
        assert_eq!(seen.get(), (42, 7, menu));
        // Only to one of goz-gui's own windows, not to a window of the
        // system's classes.
        let other = class::create_plain(&Create {
            ex_style: 0,
            class: "Static",
            title: "",
            style: 0,
            rect: (0, 0, 0, 0),
            parent: HWND_MESSAGE,
            menu: core::ptr::null_mut(),
        });
        assert!(!other.is_null());
        assert_eq!(measure_item(other, menu, 0), None);
        assert!(!draw_item(other, menu, 0, canvas.hdc(), RECT::default()));
        // The item is found under a menu that holds the one it is in.
        let root = create_popup();
        assert!(append(root, &NewItem::submenu("Sub", menu)));
        assert!(holds_drawn_item(root, None, 42, 7));
        assert!(holds_drawn_item(root, Some(menu), 42, 7));
        assert!(
            !holds_drawn_item(root, Some(root), 42, 7),
            "not in that menu"
        );
        assert!(!holds_drawn_item(root, None, 42, 8), "another value");
        let other_menu = create_popup();
        assert!(!holds_drawn_item(other_menu, None, 42, 7));
        assert!(crate::win::destroy(owner));
        assert!(crate::win::destroy(other));
        assert!(destroy(root));
        assert!(destroy(other_menu));
    }

    #[test]
    fn only_an_item_its_owner_draws_is_measured() {
        let menu = create_popup();
        // No item, then one the menu draws itself.
        for _ in 0..2 {
            assert_eq!(measure_item(core::ptr::null_mut(), menu, 0), None);
            assert!(!draw_item(
                core::ptr::null_mut(),
                menu,
                0,
                core::ptr::null_mut(),
                RECT::default()
            ));
            append(menu, &NewItem::command(1, "Text"));
        }
        assert!(destroy(menu));
    }
}
