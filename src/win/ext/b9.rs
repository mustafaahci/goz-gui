//! Batch 9 of the move onto `win`: menus, scroll bars, the registry, the
//! shell's COM objects (the file dialogs, shortcuts, the jump list, icon
//! handlers), icons, resources and pixels drawn with GDI.

use std::borrow::Cow;

/// `units` NUL-terminated: as they are if they end with a NUL, else a copy
/// with one.
fn z(units: &[u16]) -> Cow<'_, [u16]> {
    if units.last() == Some(&0) {
        Cow::Borrowed(units)
    } else {
        Cow::Owned(crate::win::wz_units(units))
    }
}

pub mod menu {
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
        EnableMenuItem, GET_MENU_DEFAULT_ITEM_FLAGS, GetMenu, GetMenuBarInfo, GetMenuDefaultItem,
        GetMenuItemCount, GetMenuItemID, GetMenuItemInfoW, GetMenuState, GetMenuStringW,
        GetSubMenu, GetSystemMenu, HBMMENU_CALLBACK, HMENU, InsertMenuItemW, MENU_ITEM_FLAGS,
        MENU_ITEM_STATE, MENU_ITEM_TYPE, MENUBARINFO, MENUITEMINFOW, MF_BYCOMMAND, MF_BYPOSITION,
        MF_CHECKED, MF_DISABLED, MF_ENABLED, MF_GRAYED, MF_UNCHECKED, MFT_OWNERDRAW,
        MFT_RADIOCHECK, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP, MIIM_DATA, MIIM_FTYPE, MIIM_ID,
        MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, OBJECT_IDENTIFIER, RemoveMenu, SetMenu,
        SetMenuDefaultItem, SetMenuItemInfoW, TRACK_POPUP_MENU_FLAGS, TrackPopupMenuEx,
        WM_DRAWITEM, WM_MEASUREITEM,
    };

    use crate::win::{send_raw, until_nul, wz};

    /// The longest text read back from an item, in UTF-16 units.
    const TEXT_MAX: usize = 32_768;

    /// Which item of a menu: by its place (from 0), or by the command it
    /// runs (the first item that does, its submenus searched too).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum By {
        Position(u32),
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

    /// A menu destroyed (with its submenus) when this drops.
    pub struct Owned(HMENU);

    impl Owned {
        pub fn new(menu: HMENU) -> Owned {
            Owned(menu)
        }

        pub fn get(&self) -> HMENU {
            self.0
        }

        /// Gives the menu up without destroying it.
        pub fn into_raw(self) -> HMENU {
            let menu = self.0;
            core::mem::forget(self);
            menu
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            destroy(self.0);
        }
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

    /// The submenu of the item at `pos` (null for none).
    pub fn sub_menu(menu: HMENU, pos: i32) -> HMENU {
        // SAFETY: no pointers.
        unsafe { GetSubMenu(menu, pos) }
    }

    /// Deletes an item; its submenu is destroyed with it. False if there is
    /// no such item.
    pub fn delete(menu: HMENU, by: By) -> bool {
        let (item, flag) = by.flagged();
        // SAFETY: no pointers.
        unsafe { DeleteMenu(menu, item, flag) != 0 }
    }

    /// Takes an item out; its submenu is not destroyed (it is the caller's
    /// again). False if there is no such item.
    pub fn remove(menu: HMENU, by: By) -> bool {
        let (item, flag) = by.flagged();
        // SAFETY: no pointers.
        unsafe { RemoveMenu(menu, item, flag) != 0 }
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
        /// The value its owner keeps with it (`dwItemData`), as a number.
        pub data: usize,
        /// Its bitmap: null, a bitmap, or one of the HBMMENU_* values.
        pub bitmap: HBITMAP,
        /// Its text (`&` before its access key, a tab before its shortcut);
        /// empty for none.
        pub text: String,
    }

    impl ItemInfo {
        pub fn is_separator(&self) -> bool {
            self.kind & MFT_SEPARATOR != 0
        }
    }

    /// Everything about an item; `None` if there is no such item.
    pub fn item_info(menu: HMENU, by: By) -> Option<ItemInfo> {
        let (item, by_pos) = by.by_position();
        let mask = MIIM_FTYPE
            | MIIM_STATE
            | MIIM_ID
            | MIIM_SUBMENU
            | MIIM_STRING
            | MIIM_DATA
            | MIIM_BITMAP;
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
            data: mii.dwItemData,
            bitmap: mii.hbmpItem,
            text: String::from_utf16_lossy(until_nul(&buf)),
        })
    }

    /// An item's text (empty for none, or no such item).
    pub fn text(menu: HMENU, by: By) -> String {
        let (item, flag) = by.flagged();
        // SAFETY: no buffer (null, of no length): the call gives the
        // text's length.
        let len = unsafe { GetMenuStringW(menu, item, core::ptr::null_mut(), 0, flag) };
        let len = usize::try_from(len).unwrap_or(0).min(TEXT_MAX);
        if len == 0 {
            return String::new();
        }
        // A unit to spare past what the length passed lets the call write.
        let mut buf = vec![0u16; len + 2];
        // SAFETY: the buffer, longer than the length passed, alive for the
        // call.
        unsafe { GetMenuStringW(menu, item, buf.as_mut_ptr(), (len + 1) as i32, flag) };
        String::from_utf16_lossy(until_nul(&buf))
    }

    /// Changes an item's text, its MFS_* state, or both (`None` leaves
    /// one as it is); false if it could not.
    pub fn set_item(
        menu: HMENU,
        by: By,
        text: Option<&str>,
        state: Option<MENU_ITEM_STATE>,
    ) -> bool {
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

    /// What `owner` measures item `pos` of `menu` at, asked as the menu loop
    /// asks before it shows an item its owner draws (WM_MEASUREITEM, with
    /// the item's command and value as the menu holds them): its width and
    /// height. `None` if there is no such item, or its owner does not draw
    /// it.
    pub fn measure_item(owner: HWND, menu: HMENU, pos: u32) -> Option<(u32, u32)> {
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
        // alive and writable for the call. The item is one its owner draws,
        // and its command and value are the ones `menu` holds for it: what
        // the menu loop sends with this message to the window it shows the
        // menu for (see `track`). Its other fields are numbers.
        unsafe { send_raw(owner, WM_MEASUREITEM, 0, (&raw mut mis) as LPARAM) };
        Some((mis.itemWidth, mis.itemHeight))
    }

    /// Has `owner` draw item `pos` of `menu` into `hdc`, in `rc`, as the
    /// menu loop asks (WM_DRAWITEM, the whole item, in no particular state,
    /// with its command and value as the menu holds them). False if there
    /// is no such item, or its owner does not draw it.
    pub fn draw_item(owner: HWND, menu: HMENU, pos: u32, hdc: HDC, rc: RECT) -> bool {
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
        // (and writable, as a handler may treat it) for the call. Its
        // command and value are the ones `menu` holds for the item, as in
        // `measure_item`; the menu and the DC are handles, the rest numbers.
        unsafe { send_raw(owner, WM_DRAWITEM, 0, (&raw mut dis) as LPARAM) };
        true
    }

    /// The window's menu bar (null for none).
    pub fn of_window(hwnd: HWND) -> HMENU {
        // SAFETY: no pointers.
        unsafe { GetMenu(hwnd) }
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

        #[test]
        fn items_go_in_and_come_back() {
            let menu = Owned::new(create_popup());
            let sub = create_popup();
            assert!(append(sub, &NewItem::text("(Empty)").state(MFS_GRAYED)));
            assert!(append(menu.get(), &NewItem::command(7, "&Open\tEnter")));
            assert!(append(menu.get(), &NewItem::separator()));
            assert!(append(menu.get(), &NewItem::command(8, "&Dot").radio()));
            assert!(append(menu.get(), &NewItem::submenu("Se&nd to", sub)));
            assert!(insert(menu.get(), 0, &NewItem::command(9, "First")));
            assert_eq!(count(menu.get()), 5);
            assert_eq!(text(menu.get(), By::Position(0)), "First");
            assert_eq!(text(menu.get(), By::Command(7)), "&Open\tEnter");
            assert!(is_separator(menu.get(), By::Position(2)));
            assert!(!is_separator(menu.get(), By::Position(1)));
            assert_eq!(item_id(menu.get(), 1), 7);
            let dot = item_info(menu.get(), By::Command(8)).unwrap();
            assert!(dot.kind & MFT_RADIOCHECK != 0 && dot.text == "&Dot");
            let send = item_info(menu.get(), By::Position(4)).unwrap();
            assert_eq!(send.submenu, sub);
            assert_eq!(sub_menu(menu.get(), 4), sub);
            let empty = item_info(sub, By::Position(0)).unwrap();
            assert!(empty.state & MFS_GRAYED != 0 && empty.text == "(Empty)");
            assert!(item_info(menu.get(), By::Position(9)).is_none());
            assert_eq!(text(menu.get(), By::Command(99)), "");
        }

        #[test]
        fn items_change() {
            let menu = Owned::new(create_popup());
            append(menu.get(), &NewItem::command(1, "One"));
            append(menu.get(), &NewItem::command(2, "Two"));
            assert_eq!(check(menu.get(), By::Command(1), true), Some(false));
            assert_eq!(check(menu.get(), By::Command(1), false), Some(true));
            assert_eq!(check(menu.get(), By::Command(5), true), None);
            assert_eq!(enable(menu.get(), By::Command(2), false), Some(true));
            assert_eq!(enable(menu.get(), By::Command(2), true), Some(false));
            assert!(state(menu.get(), By::Command(2)).is_some());
            assert!(state(menu.get(), By::Command(5)).is_none());
            assert!(set_item(
                menu.get(),
                By::Command(2),
                Some("Deux"),
                Some(MFS_CHECKED)
            ));
            assert_eq!(text(menu.get(), By::Position(1)), "Deux");
            assert!(item_info(menu.get(), By::Command(2)).unwrap().state & MFS_CHECKED != 0);
            assert_eq!(default_item(menu.get(), false, 0), None);
            assert!(set_default(menu.get(), By::Command(2)));
            assert_eq!(default_item(menu.get(), false, 0), Some(2));
            assert!(delete(menu.get(), By::Command(1)));
            assert!(!delete(menu.get(), By::Command(1)));
            assert_eq!(count(menu.get()), 1);
            assert_eq!(count(core::ptr::null_mut()), -1);
        }

        #[test]
        fn an_item_its_owner_draws_goes_to_the_owner_as_the_menu_holds_it() {
            use std::cell::Cell;
            use std::rc::Rc;
            use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_MESSAGE, MFT_OWNERDRAW};

            use crate::win::class::{self, Class, Create};

            let menu = Owned::new(create_popup());
            let mii = MENUITEMINFOW {
                fType: MFT_OWNERDRAW,
                wID: 42,
                dwItemData: 7,
                ..info(MIIM_FTYPE | MIIM_ID | MIIM_DATA)
            };
            // SAFETY: an in-struct with its size, of numbers (no text).
            assert!(unsafe { InsertMenuItemW(menu.get(), 0, 1, &mii) } != 0);
            let seen = Rc::new(Cell::new((0u32, 0usize, core::ptr::null_mut())));
            let got = Rc::clone(&seen);
            let _ = Class::new("goz-gui b9 menu owner").register();
            let owner = class::create(
                &Create {
                    ex_style: 0,
                    class: "goz-gui b9 menu owner",
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
            assert_eq!(measure_item(owner, menu.get(), 0), Some((30, 12)));
            assert_eq!(seen.get(), (42, 7, core::ptr::null_mut()));
            let canvas = super::super::gdi::Canvas::new(30, 12).unwrap();
            assert!(draw_item(
                owner,
                menu.get(),
                0,
                canvas.hdc(),
                RECT::default()
            ));
            assert_eq!(seen.get(), (42, 7, menu.get()));
            assert!(crate::win::destroy(owner));
        }

        #[test]
        fn only_an_item_its_owner_draws_is_measured() {
            let menu = Owned::new(create_popup());
            // No item, then one the menu draws itself.
            for _ in 0..2 {
                assert_eq!(measure_item(core::ptr::null_mut(), menu.get(), 0), None);
                assert!(!draw_item(
                    core::ptr::null_mut(),
                    menu.get(),
                    0,
                    core::ptr::null_mut(),
                    RECT::default()
                ));
                append(menu.get(), &NewItem::command(1, "Text"));
            }
        }
    }
}

pub mod scroll {
    //! Scroll bars, and how far the wheel scrolls.

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Controls::SetScrollInfo;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetScrollInfo, SCROLLBAR_CONSTANTS, SCROLLINFO, SCROLLINFO_MASK, SPI_GETWHEELSCROLLLINES,
        SystemParametersInfoW,
    };

    /// Sets the range, page or position of a window's scroll bar (`bar`:
    /// SB_VERT, SB_HORZ, or SB_CTL for a scroll bar control) as `info`
    /// says (its fMask; its size is set here), redrawing it if `redraw`;
    /// the position now.
    pub fn set_info(hwnd: HWND, bar: SCROLLBAR_CONSTANTS, info: &SCROLLINFO, redraw: bool) -> i32 {
        let si = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            ..*info
        };
        // SAFETY: an in-struct with its size.
        unsafe { SetScrollInfo(hwnd, bar, &si, redraw.into()) }
    }

    /// What `mask` (SIF_*) asks of a window's scroll bar; `None` if it has
    /// none.
    pub fn info(hwnd: HWND, bar: SCROLLBAR_CONSTANTS, mask: SCROLLINFO_MASK) -> Option<SCROLLINFO> {
        let mut si = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: mask,
            ..Default::default()
        };
        // SAFETY: an out-struct with its size.
        (unsafe { GetScrollInfo(hwnd, bar, &mut si) } != 0).then_some(si)
    }

    /// The lines a notch of the mouse wheel scrolls, as the user set it.
    pub fn wheel_lines() -> Option<u32> {
        let mut lines = 0u32;
        // SAFETY: SPI_GETWHEELSCROLLLINES writes one u32, to `lines`.
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0)
        } != 0;
        ok.then_some(lines)
    }
}

pub mod gdi {
    //! GDI beyond `win::gdi`: pixels, bitmaps of a format, and a canvas
    //! whose pixels go in and out by copy.

    use windows_sys::Win32::Foundation::COLORREF;
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDIBits, GetPixel, HBITMAP, HDC,
        HGDIOBJ, SelectObject, SetDIBits,
    };

    /// The colour of the pixel at `x, y` of the DC (CLR_INVALID outside its
    /// clip).
    pub fn get_pixel(hdc: HDC, x: i32, y: i32) -> COLORREF {
        // SAFETY: no pointers.
        unsafe { GetPixel(hdc, x, y) }
    }

    /// A new `cx` × `cy` bitmap of `planes` planes of `bits_per_pixel`, its
    /// pixels not set (null if it could not be made).
    pub fn create_bitmap(cx: i32, cy: i32, planes: u32, bits_per_pixel: u32) -> HBITMAP {
        // SAFETY: no initial pixels (null).
        unsafe { CreateBitmap(cx, cy, planes, bits_per_pixel, core::ptr::null()) }
    }

    /// A 32-bit top-down DIB section in a memory DC of its own, to draw on
    /// with GDI ([`Canvas::hdc`]) and read back. Its pixels go in and out
    /// by copy, through GDI (GetDIBits, SetDIBits), never through a pointer
    /// into the section: GDI checks the bitmap each time, whatever was done
    /// with the DC meanwhile. Pixels are BGRA, rows top-down.
    pub struct Canvas {
        dc: HDC,
        bitmap: HBITMAP,
        /// The bitmap the DC had, put back to read or write the pixels.
        old: HGDIOBJ,
        width: i32,
        height: i32,
    }

    impl Canvas {
        /// A `width` × `height` canvas, every pixel zero (clear black);
        /// `None` for a size of no pixels, or if it could not be made.
        pub fn new(width: i32, height: i32) -> Option<Canvas> {
            if width <= 0 || height <= 0 {
                return None;
            }
            let bmi = info(width, height);
            let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
            // SAFETY: a complete header; the pointer to the pixels is
            // written to a local and never used (they are reached through
            // GDI only).
            let bitmap = unsafe {
                CreateDIBSection(
                    core::ptr::null_mut(),
                    &bmi,
                    DIB_RGB_COLORS,
                    &mut bits,
                    core::ptr::null_mut(),
                    0,
                )
            };
            if bitmap.is_null() {
                return None;
            }
            // SAFETY: no pointers.
            let dc = unsafe { CreateCompatibleDC(core::ptr::null_mut()) };
            if dc.is_null() {
                // SAFETY: no pointers; the bitmap made above, in no DC.
                unsafe { DeleteObject(bitmap) };
                return None;
            }
            // SAFETY: no pointers.
            let old = unsafe { SelectObject(dc, bitmap) };
            Some(Canvas {
                dc,
                bitmap,
                old,
                width,
                height,
            })
        }

        /// The DC to draw on (with the canvas selected into it).
        pub fn hdc(&self) -> HDC {
            self.dc
        }

        pub fn size(&self) -> (i32, i32) {
            (self.width, self.height)
        }

        fn count(&self) -> usize {
            self.width as usize * self.height as usize
        }

        /// Runs `f` with the canvas out of its DC, as GetDIBits and
        /// SetDIBits want it, after what was drawn has been done (GdiFlush).
        fn out_of_dc<R>(&self, f: impl FnOnce() -> R) -> R {
            // SAFETY: no pointers.
            unsafe {
                GdiFlush();
                SelectObject(self.dc, self.old);
            }
            let r = f();
            // SAFETY: no pointers.
            unsafe { SelectObject(self.dc, self.bitmap) };
            r
        }

        /// A copy of the pixels; `None` if they could not be read.
        pub fn pixels(&self) -> Option<Vec<[u8; 4]>> {
            let mut out = vec![[0u8; 4]; self.count()];
            let mut bmi = info(self.width, self.height);
            let lines = self.out_of_dc(|| {
                // SAFETY: the header gives the size and format of the DIB
                // written (whatever the bitmap is): `height` rows of
                // `width` 32-bit pixels (rows of 32-bit pixels need no
                // padding), which the buffer holds; the header is in-out
                // (BI_RGB at 32 bits: no colour table or masks follow it).
                unsafe {
                    GetDIBits(
                        self.dc,
                        self.bitmap,
                        0,
                        self.height as u32,
                        out.as_mut_ptr().cast(),
                        &mut bmi,
                        DIB_RGB_COLORS,
                    )
                }
            });
            (lines == self.height).then_some(out)
        }

        /// Sets the pixels from `pixels` (`width` × `height` of them); false
        /// if they are not that many or could not be written.
        pub fn set_pixels(&self, pixels: &[[u8; 4]]) -> bool {
            if pixels.len() != self.count() {
                return false;
            }
            let bmi = info(self.width, self.height);
            let lines = self.out_of_dc(|| {
                // SAFETY: `height` rows of `width` 32-bit pixels, read
                // during the call, as the header says (and as many as the
                // slice holds, checked above).
                unsafe {
                    SetDIBits(
                        self.dc,
                        self.bitmap,
                        0,
                        self.height as u32,
                        pixels.as_ptr().cast(),
                        &bmi,
                        DIB_RGB_COLORS,
                    )
                }
            });
            lines == self.height
        }

        /// Sets every pixel to `pixel`.
        pub fn fill(&self, pixel: [u8; 4]) -> bool {
            self.set_pixels(&vec![pixel; self.count()])
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            // SAFETY: no pointers; the DC and bitmap this made, the bitmap
            // taken out of the DC before both are deleted.
            unsafe {
                SelectObject(self.dc, self.old);
                DeleteDC(self.dc);
                DeleteObject(self.bitmap);
            }
        }
    }

    /// The header of a 32-bit top-down DIB of `width` × `height`.
    fn info(width: i32, height: i32) -> BITMAPINFO {
        BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Top-down.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_canvas_starts_clear_and_keeps_what_it_is_given() {
            let canvas = Canvas::new(3, 2).unwrap();
            assert_eq!(canvas.pixels().unwrap(), vec![[0u8; 4]; 6]);
            let px: Vec<[u8; 4]> = (0..6u8).map(|i| [i, i + 1, i + 2, 255]).collect();
            assert!(canvas.set_pixels(&px));
            assert_eq!(canvas.pixels().unwrap(), px);
            assert!(!canvas.set_pixels(&px[..5]), "too few pixels");
            assert!(canvas.fill([255, 255, 255, 0]));
            assert_eq!(canvas.pixels().unwrap(), vec![[255, 255, 255, 0]; 6]);
            assert!(Canvas::new(0, 4).is_none());
        }
    }
}

pub mod imagelist {
    //! Image lists beyond `win::imagelist`.

    use windows_sys::Win32::UI::Controls::{IMAGELISTDRAWPARAMS, ImageList_DrawIndirect};

    /// Draws as `params` says (its size is set here); false if it did not
    /// (with ILD_ASYNC: the image is not loaded yet).
    pub fn draw_indirect(params: &IMAGELISTDRAWPARAMS) -> bool {
        let p = IMAGELISTDRAWPARAMS {
            cbSize: size_of::<IMAGELISTDRAWPARAMS>() as u32,
            ..*params
        };
        // SAFETY: an in-struct with its size, of numbers and handles.
        unsafe { ImageList_DrawIndirect(&p) != 0 }
    }
}

pub mod icon {
    //! Icons: owned ones (destroyed when they drop), the shell's icons of
    //! files and its stock icons, and the system image lists' entries.

    use windows_sys::Win32::Graphics::Gdi::DeleteObject;
    use windows_sys::Win32::UI::Controls::HIMAGELIST;
    use windows_sys::Win32::UI::Shell::{
        SHDefExtractIconW, SHFILEINFOW, SHGFI_FLAGS, SHGFI_ICON, SHGFI_PIDL, SHGFI_SYSICONINDEX,
        SHGSI_ICONLOCATION, SHGSI_SYSICONINDEX, SHGetFileInfoW, SHGetStockIconInfo, SHSTOCKICONID,
        SHSTOCKICONINFO, Shell_GetCachedImageIndexW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    use super::z;
    use crate::win::until_nul;

    /// An icon goz-gui made (extracted, loaded), destroyed when this drops.
    pub struct Icon(HICON);

    impl Icon {
        /// Takes `icon`, one made for the caller (not null), to destroy when
        /// this drops.
        pub(super) fn own(icon: HICON) -> Option<Icon> {
            (!icon.is_null()).then_some(Icon(icon))
        }

        pub fn handle(&self) -> HICON {
            self.0
        }

        /// The width of its image, in pixels.
        pub fn width(&self) -> Option<i32> {
            let mut info = ICONINFO::default();
            // SAFETY: an out-struct.
            if unsafe { GetIconInfo(self.0, &mut info) } == 0 {
                return None;
            }
            let shown = if info.hbmColor.is_null() {
                info.hbmMask
            } else {
                info.hbmColor
            };
            let bm = crate::win::gdi::bitmap_info(shown);
            for b in [info.hbmColor, info.hbmMask] {
                if !b.is_null() {
                    // SAFETY: no pointers; the bitmaps GetIconInfo made for
                    // the caller, deleted once.
                    unsafe { DeleteObject(b) };
                }
            }
            bm.filter(|bm| bm.bmWidth > 0).map(|bm| bm.bmWidth)
        }
    }

    impl Drop for Icon {
        fn drop(&mut self) {
            // SAFETY: no pointers; the icon this owns, destroyed once.
            unsafe { DestroyIcon(self.0) };
        }
    }

    /// Icon `index` of `file` (a program, library or icon file; minus a
    /// resource id), `size` pixels square, as the shell extracts it
    /// (SHDefExtractIconW); `None` if it could not.
    pub fn def_extract(file: &[u16], index: i32, size: u32) -> Option<Icon> {
        let file = z(file);
        let mut icon: HICON = core::ptr::null_mut();
        // SAFETY: a NUL-terminated path; an out-handle for the large icon,
        // none for the small one.
        unsafe {
            SHDefExtractIconW(
                file.as_ptr(),
                index,
                0,
                &mut icon,
                core::ptr::null_mut(),
                size,
            )
        };
        Icon::own(icon)
    }

    /// The system image lists' entry for icon `index` of `file`, as
    /// Explorer asks for it (Shell_GetCachedImageIndexW); -1 for none.
    pub fn cached_image_index(file: &[u16], index: i32) -> i32 {
        let file = z(file);
        // SAFETY: a NUL-terminated path.
        unsafe { Shell_GetCachedImageIndexW(file.as_ptr(), index, 0) }
    }

    /// The system image list (null if the shell could not say) and the
    /// index in it of the icon of `path` (SHGetFileInfoW with
    /// SHGFI_SYSICONINDEX and `flags`, such as SHGFI_SMALLICON, or
    /// SHGFI_USEFILEATTRIBUTES with `attributes`). SHGFI_PIDL and
    /// SHGFI_ICON are ignored: `path` is a path, and no icon is made.
    pub fn sys_icon(path: &[u16], attributes: u32, flags: SHGFI_FLAGS) -> (HIMAGELIST, i32) {
        let path = z(path);
        let flags = (flags & !(SHGFI_PIDL | SHGFI_ICON)) | SHGFI_SYSICONINDEX;
        let mut info = SHFILEINFOW::default();
        // SAFETY: a NUL-terminated path (not an item id list: SHGFI_PIDL is
        // off); an out-struct with its size, which holds no handle to free
        // (SHGFI_ICON is off).
        let list = unsafe {
            SHGetFileInfoW(
                path.as_ptr(),
                attributes,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                flags,
            )
        };
        (list as HIMAGELIST, info.iIcon)
    }

    /// Stock icon `id`'s index in the system image lists.
    pub fn stock_index(id: SHSTOCKICONID) -> Option<i32> {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: an out-struct with its size; no icon is made.
        let hr = unsafe { SHGetStockIconInfo(id, SHGSI_SYSICONINDEX, &mut info) };
        (hr >= 0).then_some(info.iSysImageIndex)
    }

    /// Where stock icon `id` is: its file and its index there.
    pub fn stock_location(id: SHSTOCKICONID) -> Option<(Vec<u16>, i32)> {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: an out-struct with its size; no icon is made.
        let hr = unsafe { SHGetStockIconInfo(id, SHGSI_ICONLOCATION, &mut info) };
        (hr == 0).then(|| (until_nul(&info.szPath).to_vec(), info.iIcon))
    }
}

pub mod path {
    //! Paths as the shell writes them: `file:` URLs and environment
    //! variables.

    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows_sys::Win32::UI::Shell::PathCreateFromUrlW;

    use super::z;

    /// The longest path [`from_url`] gives, in UTF-16 units.
    const URL_PATH_MAX: usize = 1023;

    /// The path a `file:` URL names (`file:///C:/a%20b/x.ico` is
    /// `C:\a b\x.ico`), up to 1023 units; `None` for anything else.
    pub fn from_url(url: &[u16]) -> Option<Vec<u16>> {
        let url = z(url);
        let mut out = vec![0u16; URL_PATH_MAX + 1];
        let mut len = out.len() as u32;
        // SAFETY: a NUL-terminated URL; the buffer with its length in
        // units, which the call sets to the path's (without its NUL).
        let status = unsafe { PathCreateFromUrlW(url.as_ptr(), out.as_mut_ptr(), &mut len, 0) };
        if status < 0 || len == 0 || len as usize >= out.len() {
            return None;
        }
        out.truncate(len as usize);
        Some(out)
    }

    /// `text` with its environment variables (`%SystemRoot%`) expanded;
    /// `None` if it could not be.
    pub fn expand_environment(text: &[u16]) -> Option<Vec<u16>> {
        let source = z(text);
        let mut out = vec![0u16; 1024];
        for _ in 0..2 {
            let cap = u32::try_from(out.len()).ok()?;
            // SAFETY: a NUL-terminated source; the buffer with its length.
            let n = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), out.as_mut_ptr(), cap) };
            match n as usize {
                0 => return None,
                // The count includes the NUL.
                n if n <= out.len() => {
                    out.truncate(n - 1);
                    return Some(out);
                }
                n => out = vec![0u16; n],
            }
        }
        None
    }
}

pub mod resource {
    //! Resources of programs and libraries, read without running them.

    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{
        EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE,
        LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource, LockResource, SizeofResource,
    };

    use super::z;

    /// A program or library loaded as data (nothing in it runs), to copy its
    /// resources; freed when this drops.
    pub struct DataFile(HMODULE);

    impl DataFile {
        /// Loads `path` as data; `None` if it could not.
        pub fn load(path: &[u16]) -> Option<DataFile> {
            let path = z(path);
            // SAFETY: a NUL-terminated path, loaded as a data file and image
            // resource: mapped to be read, none of its code run.
            let module = unsafe {
                LoadLibraryExW(
                    path.as_ptr(),
                    core::ptr::null_mut(),
                    LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
                )
            };
            (!module.is_null()).then_some(DataFile(module))
        }

        /// A copy of resource `id` of the numbered type `kind` (RT_*).
        pub fn by_id(&self, kind: u16, id: u16) -> Option<Vec<u8>> {
            // SAFETY: the file this holds, loaded; numbered names
            // (MAKEINTRESOURCE: below 0x10000, never read as strings).
            unsafe { copy(self.0, int_name(id), int_name(kind)) }
        }

        /// A copy of the `n`th resource (from 0) of the numbered type `kind`,
        /// in the order the file lists them.
        pub fn nth(&self, kind: u16, n: usize) -> Option<Vec<u8>> {
            struct Search {
                want: usize,
                seen: usize,
                found: Option<Vec<u8>>,
            }
            /// One resource of the type: copied when it is the one wanted.
            unsafe extern "system" fn each(
                module: HMODULE,
                kind: *const u16,
                name: *const u16,
                param: isize,
            ) -> i32 {
                // SAFETY: `param` is the `Search` below, alive across the
                // enumeration and reached only here.
                let search = unsafe { &mut *(param as *mut Search) };
                if search.seen == search.want {
                    // SAFETY: the module being enumerated, and the type and
                    // name the enumeration gives, valid during this call.
                    search.found = unsafe { copy(module, name, kind) };
                    return 0;
                }
                search.seen += 1;
                1
            }
            let mut search = Search {
                want: n,
                seen: 0,
                found: None,
            };
            // SAFETY: the file this holds; a numbered type; the callback
            // above with the `Search` it expects, alive across the call.
            unsafe {
                EnumResourceNamesW(
                    self.0,
                    int_name(kind),
                    Some(each),
                    (&raw mut search) as isize,
                )
            };
            search.found
        }
    }

    impl Drop for DataFile {
        fn drop(&mut self) {
            // SAFETY: no pointers; the module this loaded, freed once.
            unsafe { FreeLibrary(self.0) };
        }
    }

    /// A numbered resource name or type (MAKEINTRESOURCE).
    fn int_name(id: u16) -> *const u16 {
        core::ptr::without_provenance(usize::from(id))
    }

    /// A copy of resource `name` of type `kind` in `module`.
    ///
    /// # Safety
    /// `module` is loaded; `name` and `kind` are resource names (numbers
    /// below 0x10000, or NUL-terminated strings).
    unsafe fn copy(module: HMODULE, name: *const u16, kind: *const u16) -> Option<Vec<u8>> {
        // SAFETY: as the caller promises; the bytes are copied while the
        // module stays loaded, `size` of them as the resource says.
        unsafe {
            let found = FindResourceW(module, name, kind);
            if found.is_null() {
                return None;
            }
            let size = SizeofResource(module, found) as usize;
            let data = LockResource(LoadResource(module, found));
            if data.is_null() || size == 0 {
                return None;
            }
            Some(core::slice::from_raw_parts(data.cast::<u8>(), size).to_vec())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_library_s_icon_groups_are_read_as_data() {
            const RT_GROUP_ICON: u16 = 14;
            let path: Vec<u16> = r"C:\Windows\System32\shell32.dll".encode_utf16().collect();
            let dll = DataFile::load(&path).unwrap();
            // An icon directory: reserved 0, type 1.
            let first = dll.nth(RT_GROUP_ICON, 0).unwrap();
            assert_eq!(&first[..4], &[0, 0, 1, 0]);
            assert!(dll.nth(RT_GROUP_ICON, 1_000_000).is_none());
            assert!(dll.by_id(RT_GROUP_ICON, 0xfff0).is_none());
        }
    }
}

pub mod reg {
    //! The registry: string values read, written and deleted under the
    //! system's root keys. A key opened here is closed before the call
    //! returns.

    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
    };

    use crate::win::wz;

    /// A root key of the registry.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Hive {
        CurrentUser,
        LocalMachine,
        ClassesRoot,
    }

    impl Hive {
        fn key(self) -> HKEY {
            match self {
                Hive::CurrentUser => HKEY_CURRENT_USER,
                Hive::LocalMachine => HKEY_LOCAL_MACHINE,
                Hive::ClassesRoot => HKEY_CLASSES_ROOT,
            }
        }
    }

    fn ptr(s: &Option<Vec<u16>>) -> *const u16 {
        s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr())
    }

    /// The REG_SZ value `value` of `key` under `hive` (`None`: the key's
    /// default value); `None` if there is none.
    pub fn get_string(hive: Hive, key: &str, value: Option<&str>) -> Option<String> {
        let key = wz(key);
        let value = value.map(wz);
        let mut buf = vec![0u16; 2048];
        // Twice more at most, for a value that grows while it is read.
        for _ in 0..3 {
            let mut size = u32::try_from(buf.len() * 2).ok()?;
            // SAFETY: NUL-terminated names (or null for the default value)
            // and a buffer whose size in bytes is passed.
            let rc = unsafe {
                RegGetValueW(
                    hive.key(),
                    key.as_ptr(),
                    ptr(&value),
                    RRF_RT_REG_SZ,
                    core::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if rc == ERROR_MORE_DATA {
                buf = vec![0u16; (size as usize).div_ceil(2) + 1];
                continue;
            }
            if rc != ERROR_SUCCESS {
                return None;
            }
            // The size counts the NUL.
            let len = (size as usize / 2).saturating_sub(1).min(buf.len());
            buf.truncate(len);
            return Some(String::from_utf16_lossy(&buf));
        }
        None
    }

    /// Sets the REG_SZ value `value` of `key` under `hive` (`None`: the
    /// key's default) to `data`, making the key if need be; false if it
    /// could not.
    pub fn set_string(hive: Hive, key: &str, value: Option<&str>, data: &str) -> bool {
        let key = wz(key);
        let value = value.map(wz);
        let data = wz(data);
        let Ok(bytes) = u32::try_from(data.len() * 2) else {
            return false;
        };
        let mut h: HKEY = core::ptr::null_mut();
        // SAFETY: a NUL-terminated name, no class or security attributes,
        // an out-handle.
        let rc = unsafe {
            RegCreateKeyExW(
                hive.key(),
                key.as_ptr(),
                0,
                core::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                core::ptr::null(),
                &mut h,
                core::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return false;
        }
        // SAFETY: the key just opened; a NUL-terminated name (or null) and
        // the data's `bytes`, its NUL included; then the key is closed.
        unsafe {
            let rc = RegSetValueExW(h, ptr(&value), 0, REG_SZ, data.as_ptr().cast(), bytes);
            RegCloseKey(h);
            rc == ERROR_SUCCESS
        }
    }

    /// Deletes value `value` of `key` under `hive`; false if it could not
    /// (it is not there, say).
    pub fn delete_value(hive: Hive, key: &str, value: &str) -> bool {
        let key = wz(key);
        let value = wz(value);
        // SAFETY: NUL-terminated names.
        unsafe { RegDeleteKeyValueW(hive.key(), key.as_ptr(), value.as_ptr()) == ERROR_SUCCESS }
    }

    /// Deletes `key` under `hive` with its subkeys and values; false if it
    /// could not (it is not there, say). An empty `key` (the whole hive) is
    /// refused.
    pub fn delete_tree(hive: Hive, key: &str) -> bool {
        if key.is_empty() {
            return false;
        }
        let key = wz(key);
        // SAFETY: a NUL-terminated name.
        unsafe { RegDeleteTreeW(hive.key(), key.as_ptr()) == ERROR_SUCCESS }
    }

    /// Deletes `key` under `hive`, which has no subkeys left; false if it
    /// could not.
    pub fn delete_key(hive: Hive, key: &str) -> bool {
        let key = wz(key);
        // SAFETY: a NUL-terminated name.
        unsafe { RegDeleteKeyW(hive.key(), key.as_ptr()) == ERROR_SUCCESS }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_value_round_trips_under_a_throwaway_key() {
            let root = format!(r"Software\goz-gui-b9-test-{}", std::process::id());
            let key = format!(r"{root}\a");
            assert_eq!(get_string(Hive::CurrentUser, &key, None), None);
            assert!(set_string(Hive::CurrentUser, &key, None, "default"));
            let long = "x".repeat(5000);
            assert!(set_string(Hive::CurrentUser, &key, Some("long"), &long));
            assert_eq!(
                get_string(Hive::CurrentUser, &key, None).as_deref(),
                Some("default")
            );
            assert_eq!(
                get_string(Hive::CurrentUser, &key, Some("long")),
                Some(long)
            );
            assert!(delete_value(Hive::CurrentUser, &key, "long"));
            assert!(!delete_value(Hive::CurrentUser, &key, "long"));
            assert!(!delete_tree(Hive::CurrentUser, ""));
            // A key with no subkeys goes by itself.
            assert!(delete_key(Hive::CurrentUser, &key));
            assert_eq!(get_string(Hive::CurrentUser, &key, None), None);
            // A tree goes whole.
            assert!(set_string(Hive::CurrentUser, &key, None, "again"));
            assert!(delete_tree(Hive::CurrentUser, &root));
            assert!(!delete_key(Hive::CurrentUser, &root), "gone already");
            assert!(!delete_tree(Hive::CurrentUser, &root));
        }
    }
}

pub mod shell {
    //! The shell's COM objects goz-gui uses: the common item dialogs, shell
    //! items and their icon handlers, shortcuts, property stores and the
    //! taskbar's jump list; and a few shell calls that take or give memory
    //! of its own.
    //!
    //! Each object is owned by a wrapper that releases it when it drops, and
    //! reached only through the wrapper's methods, which build every pointer
    //! they pass and copy out (and free) every string the shell allocates.
    //! They are made on a thread with COM (`win::com::Apartment`; making one
    //! fails without), and stay on it: no wrapper is `Send`.

    use std::cell::RefCell;

    use windows::Win32::Foundation::{HWND as WHWND, PROPERTYKEY};
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemAlloc, CoTaskMemFree, IPersistFile,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, IObjectArray, IObjectCollection};
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyStore, SHGetPropertyStoreForWindow,
    };
    use windows::Win32::UI::Shell::{
        BHID_SFUIObject, CDCONTROLSTATEF, DestinationList, EnumerableObjectCollection,
        FILEOPENDIALOGOPTIONS, FileOpenDialog, FileSaveDialog, ICustomDestinationList,
        IExtractIconW, IFileDialog, IFileDialogCustomize, IFileOpenDialog, IFileSaveDialog,
        IShellItem, IShellLinkW, SHCreateItemFromParsingName, SIGDN, SIGDN_FILESYSPATH,
        SetCurrentProcessExplicitAppUserModelID, ShellLink as SHELL_LINK,
    };
    use windows::core::{Interface, PCWSTR, PWSTR, Result};
    use windows_sys::Win32::Foundation::HWND;

    use super::icon::Icon;
    use super::z;
    use crate::win::{until_nul, wz};

    /// A copy of a string the shell allocated, which is freed.
    fn take_string(s: PWSTR) -> Option<Vec<u16>> {
        if s.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated string from the COM allocator, read up
        // to its NUL, then freed once (nothing else holds it).
        unsafe {
            let units = s.as_wide().to_vec();
            CoTaskMemFree(Some(s.0 as *const core::ffi::c_void));
            Some(units)
        }
    }

    /// Gives the process the AppUserModelID `id` (before it shows a
    /// window), under which the taskbar groups its windows.
    pub fn set_app_user_model_id(id: &str) -> Result<()> {
        let id = wz(id);
        // SAFETY: a NUL-terminated string read during the call.
        unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr())) }
    }

    /// A known folder's path (`FOLDERID_*`).
    pub fn known_folder(id: &windows_sys::core::GUID) -> Option<Vec<u16>> {
        use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
        let mut raw: *mut u16 = core::ptr::null_mut();
        // SAFETY: the folder's id, no token; the shell allocates the
        // string, which is read up to its NUL and freed with CoTaskMemFree
        // as documented (on failure too).
        unsafe {
            let hr = SHGetKnownFolderPath(id, 0, core::ptr::null_mut(), &mut raw);
            let path = (hr >= 0 && !raw.is_null()).then(|| PWSTR(raw).as_wide().to_vec());
            windows_sys::Win32::System::Com::CoTaskMemFree(raw.cast());
            path
        }
    }

    /// Tells the shell that a file association changed (SHCNE_ASSOCCHANGED).
    pub fn notify_associations_changed() {
        use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        // SAFETY: a notification with no items (both null).
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                core::ptr::null(),
                core::ptr::null(),
            )
        };
    }

    /// A shell item: a file or folder as the shell sees it.
    pub struct ShellItem(IShellItem);

    impl ShellItem {
        /// The item at `path` (a path, or another name the shell parses).
        pub fn from_parsing_name(path: &[u16]) -> Option<ShellItem> {
            let path = z(path);
            // SAFETY: a NUL-terminated name read during the call; no bind
            // context.
            unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) }
                .ok()
                .map(ShellItem)
        }

        /// Its name of kind `kind` (SIGDN_*).
        pub fn display_name(&self, kind: SIGDN) -> Option<Vec<u16>> {
            // SAFETY: the item this holds; the name it gives is the
            // caller's, taken (copied and freed) here.
            let name = unsafe { self.0.GetDisplayName(kind) }.ok()?;
            take_string(name)
        }

        /// Its path in the file system.
        pub fn file_system_path(&self) -> Option<Vec<u16>> {
            self.display_name(SIGDN_FILESYSPATH)
        }

        /// Its icon handler (as Explorer binds it, `BHID_SFUIObject`).
        pub fn extract_icon(&self) -> Option<ExtractIcon> {
            // SAFETY: the item this holds; no bind context; the handler's
            // id, a static GUID.
            unsafe {
                self.0
                    .BindToHandler::<_, IExtractIconW>(None, &BHID_SFUIObject)
            }
            .ok()
            .map(ExtractIcon)
        }
    }

    /// A file's icon handler (IExtractIconW).
    pub struct ExtractIcon(IExtractIconW);

    impl ExtractIcon {
        /// Where the icon is, asked with GIL_* `flags`: the file (`"*"` for
        /// an entry of the system image lists), its index there, and GIL_*
        /// flags about it.
        pub fn location(&self, flags: u32) -> Option<(Vec<u16>, i32, u32)> {
            let mut location = [0u16; 1024];
            let (mut index, mut out_flags) = (0i32, 0u32);
            // SAFETY: the handler this holds; a buffer with its length (the
            // slice) and two out-values.
            unsafe {
                self.0
                    .GetIconLocation(flags, &mut location, &mut index, &mut out_flags)
            }
            .ok()?;
            Some((until_nul(&location).to_vec(), index, out_flags))
        }

        /// The icon its location named (`file` and `index`), at `size`
        /// pixels (the low word; the high word is the small icon's, not
        /// made); `None` if the handler made none.
        pub fn extract(&self, file: &[u16], index: u32, size: u32) -> Option<Icon> {
            let file = z(file);
            let mut icon = windows::Win32::UI::WindowsAndMessaging::HICON::default();
            // SAFETY: the handler this holds; a NUL-terminated file name and
            // an out-handle for the large icon, none for the small one.
            let _ = unsafe {
                self.0
                    .Extract(PCWSTR(file.as_ptr()), index, Some(&mut icon), None, size)
            };
            Icon::own(icon.0)
        }
    }

    /// The system's Open or Save dialog (the common item dialog), set up and
    /// shown modal. The strings it is given are kept for as long as it is,
    /// whether it copies them or not.
    pub struct FileDialog {
        // Released before what it was given is freed (fields drop in order).
        dialog: Dialog,
        strings: RefCell<Vec<Vec<u16>>>,
        specs: RefCell<Vec<Vec<COMDLG_FILTERSPEC>>>,
    }

    enum Dialog {
        Open(IFileOpenDialog),
        Save(IFileSaveDialog),
    }

    impl FileDialog {
        /// An Open dialog; `None` if it could not be made.
        pub fn open() -> Option<FileDialog> {
            // SAFETY: the system's class, in this process.
            let d = unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) };
            d.ok().map(|d| FileDialog::of(Dialog::Open(d)))
        }

        /// A Save dialog; `None` if it could not be made.
        pub fn save() -> Option<FileDialog> {
            // SAFETY: as in `open`.
            let d = unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) };
            d.ok().map(|d| FileDialog::of(Dialog::Save(d)))
        }

        fn of(dialog: Dialog) -> FileDialog {
            FileDialog {
                dialog,
                strings: RefCell::new(Vec::new()),
                specs: RefCell::new(Vec::new()),
            }
        }

        fn get(&self) -> &IFileDialog {
            match &self.dialog {
                Dialog::Open(d) => d,
                Dialog::Save(d) => d,
            }
        }

        /// A NUL-terminated copy of `s`, kept as long as the dialog (its
        /// units never move: each copy is a buffer of its own).
        fn keep(&self, s: &str) -> PCWSTR {
            let mut strings = self.strings.borrow_mut();
            strings.push(wz(s));
            PCWSTR(strings.last().map_or(core::ptr::null(), |v| v.as_ptr()))
        }

        /// Its FOS_* options.
        pub fn options(&self) -> Option<u32> {
            // SAFETY: the dialog this holds.
            unsafe { self.get().GetOptions() }.ok().map(|o| o.0)
        }

        pub fn set_options(&self, options: u32) -> bool {
            // SAFETY: the dialog this holds; flags.
            unsafe { self.get().SetOptions(FILEOPENDIALOGOPTIONS(options)) }.is_ok()
        }

        pub fn set_title(&self, title: &str) -> bool {
            let title = self.keep(title);
            // SAFETY: the dialog this holds; a string kept with it.
            unsafe { self.get().SetTitle(title) }.is_ok()
        }

        /// The text of its OK button ("Open", "Save" by default).
        pub fn set_ok_label(&self, label: &str) -> bool {
            let label = self.keep(label);
            // SAFETY: as in `set_title`.
            unsafe { self.get().SetOkButtonLabel(label) }.is_ok()
        }

        /// The kinds of file it lists, each a name and a pattern
        /// (`("CSV Files (*.csv)", "*.csv")`).
        pub fn set_file_types(&self, kinds: &[(&str, &str)]) -> bool {
            let specs: Vec<COMDLG_FILTERSPEC> = kinds
                .iter()
                .map(|&(name, pattern)| COMDLG_FILTERSPEC {
                    pszName: self.keep(name),
                    pszSpec: self.keep(pattern),
                })
                .collect();
            let mut kept = self.specs.borrow_mut();
            kept.push(specs);
            let specs = kept.last().map_or(&[][..], Vec::as_slice);
            // SAFETY: the dialog this holds; a table whose strings and
            // entries are kept with it.
            unsafe { self.get().SetFileTypes(specs) }.is_ok()
        }

        /// The kind of file listed, from 1.
        pub fn set_file_type_index(&self, index: u32) -> bool {
            // SAFETY: the dialog this holds; a number.
            unsafe { self.get().SetFileTypeIndex(index) }.is_ok()
        }

        /// The kind of file chosen, from 1.
        pub fn file_type_index(&self) -> Option<u32> {
            // SAFETY: the dialog this holds.
            unsafe { self.get().GetFileTypeIndex() }.ok()
        }

        /// The extension a name typed without one gets (no dot).
        pub fn set_default_extension(&self, extension: &str) -> bool {
            let extension = self.keep(extension);
            // SAFETY: as in `set_title`.
            unsafe { self.get().SetDefaultExtension(extension) }.is_ok()
        }

        /// The name in its file name box.
        pub fn set_file_name(&self, name: &str) -> bool {
            let name = self.keep(name);
            // SAFETY: as in `set_title`.
            unsafe { self.get().SetFileName(name) }.is_ok()
        }

        /// The folder it opens in; false if `path` is no item.
        pub fn set_folder(&self, path: &[u16]) -> bool {
            let Some(folder) = ShellItem::from_parsing_name(path) else {
                return false;
            };
            // SAFETY: the dialog this holds; an item it keeps a reference
            // to.
            unsafe { self.get().SetFolder(&folder.0) }.is_ok()
        }

        /// Its added controls (IFileDialogCustomize), for as long as it.
        pub fn customize(&self) -> Option<Customize<'_>> {
            let iface = self.get().cast::<IFileDialogCustomize>().ok()?;
            Some(Customize {
                iface,
                dialog: self,
            })
        }

        /// Shows it modal, owned by `owner`, until it closes; whether an
        /// item was chosen (false when cancelled).
        pub fn show(&self, owner: HWND) -> bool {
            // SAFETY: the dialog this holds; a window handle. It runs a
            // message loop meanwhile, while this wrapper holds no borrow.
            unsafe { self.get().Show(Some(WHWND(owner))) }.is_ok()
        }

        /// The item chosen.
        pub fn result(&self) -> Option<ShellItem> {
            // SAFETY: the dialog this holds; the item it gives is the
            // caller's.
            unsafe { self.get().GetResult() }.ok().map(ShellItem)
        }
    }

    /// The controls added to a [`FileDialog`] (IFileDialogCustomize).
    pub struct Customize<'a> {
        iface: IFileDialogCustomize,
        /// Where the labels are kept.
        dialog: &'a FileDialog,
    }

    impl Customize<'_> {
        /// Adds check box `id`, labelled `label`.
        pub fn add_check_button(&self, id: u32, label: &str, checked: bool) -> bool {
            let label = self.dialog.keep(label);
            // SAFETY: the dialog's interface this holds; a string kept with
            // the dialog.
            unsafe { self.iface.AddCheckButton(id, label, checked) }.is_ok()
        }

        /// Sets control `id`'s CDCS_* state (shown, usable).
        pub fn set_control_state(&self, id: u32, state: u32) -> bool {
            // SAFETY: the dialog's interface this holds; numbers.
            unsafe {
                self.iface
                    .SetControlState(id, CDCONTROLSTATEF(state as i32))
            }
            .is_ok()
        }

        /// Whether check box `id` is ticked (false if it could not be
        /// read).
        pub fn check_button_state(&self, id: u32) -> bool {
            // SAFETY: the dialog's interface this holds; a number.
            unsafe { self.iface.GetCheckButtonState(id) }.is_ok_and(|on| on.as_bool())
        }
    }

    /// A string PROPVARIANT (VT_LPWSTR) holding a copy of `text`, for
    /// `PropVariantClear` to free.
    fn string_variant(text: &str) -> PROPVARIANT {
        let wide = wz(text);
        let mut pv = PROPVARIANT::default();
        // SAFETY: task memory of the string's size, filled with it and
        // given to the value, which `PropVariantClear` frees; the union is
        // written as the VT_LPWSTR value it is then tagged as.
        unsafe {
            let mem = CoTaskMemAlloc(wide.len() * 2).cast::<u16>();
            if mem.is_null() {
                return pv;
            }
            core::ptr::copy_nonoverlapping(wide.as_ptr(), mem, wide.len());
            let inner = &mut *pv.Anonymous.Anonymous;
            inner.vt = VT_LPWSTR;
            inner.Anonymous.pwszVal = PWSTR(mem);
        }
        pv
    }

    /// A property store: a window's (its taskbar identity) or a shortcut's.
    pub struct PropertyStore(IPropertyStore);

    impl PropertyStore {
        /// The window's.
        pub fn for_window(hwnd: HWND) -> Result<PropertyStore> {
            // SAFETY: a window handle.
            unsafe { SHGetPropertyStoreForWindow(WHWND(hwnd)) }.map(PropertyStore)
        }

        /// Sets property `key` to the string `text`.
        pub fn set_string(&self, key: &PROPERTYKEY, text: &str) -> Result<()> {
            let mut pv = string_variant(text);
            // SAFETY: the store this holds; the key and value outlive the
            // call (the store keeps its own copy).
            let r = unsafe { self.0.SetValue(key, &pv) };
            // SAFETY: a value made by `string_variant`, cleared once.
            unsafe {
                let _ = PropVariantClear(&mut pv);
            }
            r
        }

        /// Saves what was set.
        pub fn commit(&self) -> Result<()> {
            // SAFETY: the store this holds.
            unsafe { self.0.Commit() }
        }
    }

    /// A shortcut (IShellLinkW), in memory until saved.
    pub struct ShellLink(IShellLinkW);

    impl ShellLink {
        pub fn new() -> Result<ShellLink> {
            // SAFETY: the system's class, in this process.
            unsafe { CoCreateInstance(&SHELL_LINK, None, CLSCTX_INPROC_SERVER) }.map(ShellLink)
        }

        /// What it starts.
        pub fn set_path(&self, path: &str) -> Result<()> {
            let path = wz(path);
            // SAFETY: the shortcut this holds; a NUL-terminated string read
            // during the call.
            unsafe { self.0.SetPath(PCWSTR(path.as_ptr())) }
        }

        /// The command line it passes.
        pub fn set_arguments(&self, args: &str) -> Result<()> {
            let args = wz(args);
            // SAFETY: as in `set_path`.
            unsafe { self.0.SetArguments(PCWSTR(args.as_ptr())) }
        }

        /// The command line it passes (the first 1023 units).
        pub fn arguments(&self) -> Result<String> {
            let mut buf = [0u16; 1024];
            // SAFETY: the shortcut this holds; a buffer with its length (the
            // slice).
            unsafe { self.0.GetArguments(&mut buf) }?;
            Ok(String::from_utf16_lossy(until_nul(&buf)))
        }

        /// The folder it starts in.
        pub fn set_working_directory(&self, dir: &str) -> Result<()> {
            let dir = wz(dir);
            // SAFETY: as in `set_path`.
            unsafe { self.0.SetWorkingDirectory(PCWSTR(dir.as_ptr())) }
        }

        /// Its tip.
        pub fn set_description(&self, text: &str) -> Result<()> {
            let text = wz(text);
            // SAFETY: as in `set_path`.
            unsafe { self.0.SetDescription(PCWSTR(text.as_ptr())) }
        }

        /// Its icon: icon `index` of `file`.
        pub fn set_icon_location(&self, file: &str, index: i32) -> Result<()> {
            let file = wz(file);
            // SAFETY: as in `set_path`.
            unsafe { self.0.SetIconLocation(PCWSTR(file.as_ptr()), index) }
        }

        /// Its property store (System.Title and the like).
        pub fn property_store(&self) -> Result<PropertyStore> {
            self.0.cast::<IPropertyStore>().map(PropertyStore)
        }

        /// Writes it to the file `path` (IPersistFile).
        pub fn save(&self, path: &str) -> Result<()> {
            let file = self.0.cast::<IPersistFile>()?;
            let path = wz(path);
            // SAFETY: the shortcut's interface; a NUL-terminated path read
            // during the call.
            unsafe { file.Save(PCWSTR(path.as_ptr()), true) }
        }
    }

    /// Objects collected for a jump list's category.
    pub struct ObjectCollection(IObjectCollection);

    impl ObjectCollection {
        pub fn new() -> Result<ObjectCollection> {
            // SAFETY: the system's class, in this process.
            unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }
                .map(ObjectCollection)
        }

        /// Adds `link` (the collection keeps a reference).
        pub fn add(&self, link: &ShellLink) -> Result<()> {
            // SAFETY: the collection this holds; an object it keeps a
            // reference to.
            unsafe { self.0.AddObject(&link.0) }
        }
    }

    /// The items the user took off a jump list ([`JumpList::begin_list`]).
    pub struct ObjectArray(IObjectArray);

    impl ObjectArray {
        pub fn count(&self) -> Result<u32> {
            // SAFETY: the array this holds.
            unsafe { self.0.GetCount() }
        }

        /// Item `i`, if it is a shortcut.
        pub fn link_at(&self, i: u32) -> Result<ShellLink> {
            // SAFETY: the array this holds; a number.
            unsafe { self.0.GetAt::<IShellLinkW>(i) }.map(ShellLink)
        }
    }

    /// The taskbar's jump list of an AppUserModelID (ICustomDestinationList).
    pub struct JumpList(ICustomDestinationList);

    impl JumpList {
        pub fn new() -> Result<JumpList> {
            // SAFETY: the system's class, in this process.
            unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }.map(JumpList)
        }

        /// The AppUserModelID whose list it is.
        pub fn set_app_id(&self, id: &str) -> Result<()> {
            let id = wz(id);
            // SAFETY: the list this holds; a NUL-terminated string read
            // during the call.
            unsafe { self.0.SetAppID(PCWSTR(id.as_ptr())) }
        }

        /// Starts making the list again: the items the user took off it.
        pub fn begin_list(&self) -> Result<ObjectArray> {
            let mut slots = 0u32;
            // SAFETY: the list this holds; an out-value.
            unsafe { self.0.BeginList::<IObjectArray>(&mut slots) }.map(ObjectArray)
        }

        /// Adds category `name` with the objects of `items`.
        pub fn append_category(&self, name: &str, items: &ObjectCollection) -> Result<()> {
            let array = items.0.cast::<IObjectArray>()?;
            let name = wz(name);
            // SAFETY: the list this holds; a NUL-terminated name read during
            // the call; an array it keeps a reference to.
            unsafe { self.0.AppendCategory(PCWSTR(name.as_ptr()), &array) }
        }

        /// Puts the list made since `begin_list` on the taskbar.
        pub fn commit_list(&self) -> Result<()> {
            // SAFETY: the list this holds.
            unsafe { self.0.CommitList() }
        }

        /// Deletes the list of AppUserModelID `id`.
        pub fn delete_list(&self, id: &str) -> Result<()> {
            let id = wz(id);
            // SAFETY: as in `set_app_id`.
            unsafe { self.0.DeleteList(PCWSTR(id.as_ptr())) }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_known_folder_has_a_path() {
            use windows_sys::Win32::UI::Shell::FOLDERID_Windows;
            let path = known_folder(&FOLDERID_Windows).map(|p| String::from_utf16_lossy(&p));
            assert!(path.is_some_and(|p| p.to_lowercase().ends_with("windows")));
        }

        #[test]
        fn a_shortcut_keeps_its_arguments() {
            let _com = crate::win::com::Apartment::sta().unwrap();
            let link = ShellLink::new().unwrap();
            link.set_arguments("-s \"a b\"").unwrap();
            assert_eq!(link.arguments().unwrap(), "-s \"a b\"");
            let item = ShellItem::from_parsing_name(&wz(r"C:\Windows")).unwrap();
            let path = item
                .file_system_path()
                .map(|p| String::from_utf16_lossy(&p));
            assert_eq!(
                path.as_deref().map(str::to_lowercase).as_deref(),
                Some(r"c:\windows")
            );
        }
    }
}
