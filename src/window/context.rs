//! The list's context menus: the row menu (Everything's own items, then the
//! Explorer menu of the selection, `crate::shell`), the menu of the list's
//! empty space, and the messages the shell's menu needs while it is up.
//!
//! The shell's menu is made from the selected rows, fetched first when
//! some are not held (`results::Fetching`; then the menu shows once they
//! are here): one folder's items through that folder, items spread over
//! several through the desktop with their full ID lists (`shell::Items`). Its default item
//! (what a double-click in Explorer does) is the same as goz-gui's Open, so
//! it is taken out: Open stays first and bold, and counts as a run. Of the
//! shell's verbs, Properties and Run as administrator run goz-gui's own
//! (one sheet for items in several folders; a test's dry run), and Delete,
//! Rename and Paste goz-gui's (`files.rs`: the list follows them; Rename is
//! in place, which the shell's cannot do outside Explorer); the others run
//! in the shell, which shows its own questions.

use std::rc::Rc;

use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    LVHITTESTINFO, LVIR_BOUNDS, LVM_GETITEMRECT, LVM_GETNEXTITEM, LVM_HITTEST, LVM_SUBITEMHITTEST,
    LVNI_FOCUSED, ODT_MENU,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DeleteMenu, DestroyMenu, GetMenuDefaultItem, GetMenuItemCount, GetMenuItemInfoW, HMENU,
    MB_ICONWARNING, MENUITEMINFOW, MF_BYCOMMAND, MF_BYPOSITION, MFT_SEPARATOR, MIIM_FTYPE,
    WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
};

use super::files::renamable;
use super::results::{Act, Busy, Then};
use super::{App, COL_PATH, key_down, point, send};
use crate::dialog::Quiet;
use crate::shell::{self, ContextMenu};
use crate::store::location;
use crate::{menu, popup, run_count};

/// Whether item `pos` of `menu` is a separator.
fn is_separator(menu: HMENU, pos: u32) -> bool {
    // SAFETY: the rest of MENUITEMINFOW is plain data.
    let mut mii: MENUITEMINFOW = unsafe { core::mem::zeroed() };
    mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
    mii.fMask = MIIM_FTYPE;
    // SAFETY: reads an item of a menu of this thread.
    unsafe { GetMenuItemInfoW(menu, pos, 1, &mut mii) != 0 && mii.fType & MFT_SEPARATOR != 0 }
}

/// No separator first, last, or after another: what taking items out of
/// the shell's part can leave.
fn tidy_separators(menu: HMENU) {
    // SAFETY: counts and deletes items of a menu of this thread.
    unsafe {
        let mut pos = 0u32;
        let mut after_separator = true;
        while (pos as i32) < GetMenuItemCount(menu) {
            let sep = is_separator(menu, pos);
            if sep && after_separator {
                DeleteMenu(menu, pos, MF_BYPOSITION);
                continue;
            }
            after_separator = sep;
            pos += 1;
        }
        let n = GetMenuItemCount(menu);
        if n > 0 && is_separator(menu, n as u32 - 1) {
            DeleteMenu(menu, n as u32 - 1, MF_BYPOSITION);
        }
    }
}

impl App {
    /// WM_CONTEXTMENU from the list, off its header: `l` is the mouse's
    /// screen point, or -1 from the keyboard (Shift+F10, the Apps key).
    /// On a row (or from the keyboard with rows selected) the row menu;
    /// on the empty space (or with nothing selected) the background menu.
    pub(super) fn list_menu(&self, l: LPARAM) {
        if l == -1 {
            if self.selected(1).is_empty() {
                let mut pt = POINT {
                    x: self.scale(24),
                    y: self.scale(24),
                };
                // SAFETY: converts a point of the list.
                unsafe { ClientToScreen(self.list, &mut pt) };
                self.background_menu(pt);
            } else {
                self.row_menu(None);
            }
            return;
        }
        let pt = point(l);
        let mut at = pt;
        // SAFETY: converts a screen point to the list's.
        unsafe { ScreenToClient(self.list, &mut at) };
        if self.hit_row(at).is_some() {
            self.row_menu(Some(pt));
        } else {
            self.background_menu(pt);
        }
    }

    /// The row and column under `pt` (the list's client coordinates).
    pub(super) fn hit_row(&self, pt: POINT) -> Option<(usize, i32)> {
        let mut hit = LVHITTESTINFO {
            pt,
            flags: 0,
            iItem: -1,
            iSubItem: -1,
            iGroup: 0,
        };
        // Icon view has no columns: the item, as the name's column.
        let msg = if self.view().thumbnails {
            LVM_HITTEST
        } else {
            LVM_SUBITEMHITTEST
        };
        let item = send(self.list, msg, 0, (&raw mut hit) as isize);
        (item >= 0).then_some((item as usize, hit.iSubItem.max(0)))
    }

    /// The row menu at the mouse (`Some`, screen) or, from the keyboard,
    /// under the focused row: Open (bold; Open Path in the Path column,
    /// where a double-click opens the path), Open Path, Copy Name, Copy
    /// Full Path, Set Run Count... (with the run history on), then the
    /// Explorer menu of the selection; Shift adds the extended verbs. When
    /// selected rows are not held, it shows once they are fetched.
    fn row_menu(&self, mouse: Option<POINT>) {
        let mut default = menu::OPEN;
        if let Some(pt) = mouse {
            let mut at = pt;
            // SAFETY: converts a screen point to the list's.
            unsafe { ScreenToClient(self.list, &mut at) };
            if self
                .hit_row(at)
                .and_then(|(_, sub)| self.column_id(sub.max(0) as usize))
                == Some(COL_PATH)
            {
                default = menu::REVEAL;
            }
        }
        let extended = key_down(VK_SHIFT);
        let then = Then::RowMenu {
            at: mouse,
            default,
            extended,
        };
        if !self.fetch_first(then) {
            self.show_row_menu(mouse, default, extended);
        }
    }

    /// [`row_menu`](Self::row_menu) on the selected rows held, with
    /// `default` bold and `extended` the extended verbs. The shell makes its
    /// part under the wait cursor: its handlers take their time over a big
    /// selection (see `shell::Items`).
    pub(super) fn show_row_menu(&self, mouse: Option<POINT>, default: u16, extended: bool) {
        let (paths, selected) = self.picked_paths();
        if paths.is_empty() {
            if selected > 0 {
                // None of them could be had: say so, not nothing.
                self.note_partial(Act::Command, 0, selected);
                // SAFETY: plays a system sound.
                unsafe { MessageBeep(MB_ICONWARNING) };
            }
            return;
        }
        let pt = mouse.unwrap_or_else(|| self.focused_row_point());
        let popup = menu::empty();
        let was = self.busy.get();
        self.set_busy(Busy::Wait);
        let shell_part = self.shell_part(popup, &paths, extended);
        self.set_busy(was);
        let (shell, took) = match shell_part {
            Some((cm, took)) => (Some(cm), took),
            None => (None, 0),
        };
        menu::add_row_items(popup, self.run_history_enabled());
        tidy_separators(popup);
        menu::set_default(popup, default);
        let id = self.track_with_shell(popup, pt, "row menu", shell.as_ref());
        match &shell {
            Some(cm) if cm.owns(id, u32::from(menu::SHELL_LAST)) => {
                self.run_shell_item(cm, id, pt, took, selected);
            }
            _ if id > 0 => self.command(id as u16),
            _ => {}
        }
        // SAFETY: the menu this function made; the shell's handlers are
        // done with it once its command ran.
        unsafe { DestroyMenu(popup) };
    }

    /// Where the keyboard's row menu opens: under the focused row, a little
    /// in from its left edge.
    fn focused_row_point(&self) -> POINT {
        let focused = send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        );
        let mut rc = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        send(
            self.list,
            LVM_GETITEMRECT,
            focused.max(0) as usize,
            (&raw mut rc) as isize,
        );
        let mut pt = POINT {
            x: rc.left + self.scale(24),
            y: rc.bottom,
        };
        // SAFETY: converts a point of the list.
        unsafe { ClientToScreen(self.list, &mut pt) };
        pt
    }

    /// Fills the empty `popup` with the Explorer menu of `paths`, without
    /// its default item (goz-gui's Open), and says how many of them it is
    /// for (the shell finds no gone file). `None` when the shell has none
    /// (the files are gone): the menu is goz-gui's alone.
    fn shell_part(
        &self,
        popup: HMENU,
        paths: &[Vec<u16>],
        extended: bool,
    ) -> Option<(Rc<ContextMenu>, usize)> {
        // The shell's Copy and Cut put their data object on the clipboard
        // through OLE; without it the rest of the menu still works.
        shell::ole_ready();
        let items = shell::Items::of_paths(paths)?;
        let cm = items.context_menu(self.hwnd)?;
        // goz-gui renames in place: the shell adds its Rename, except for
        // a drive (its label is not renamed in place).
        let flags = shell::query_flags(extended, paths.iter().all(|p| renamable(p)));
        if !cm.fill(
            popup,
            0,
            u32::from(menu::SHELL_FIRST),
            u32::from(menu::SHELL_LAST),
            flags,
        ) {
            return None;
        }
        // SAFETY: reads and deletes an item of a menu of this thread.
        unsafe {
            let default = GetMenuDefaultItem(popup, 0, 0);
            if cm.owns(default, u32::from(menu::SHELL_LAST)) {
                DeleteMenu(popup, default, MF_BYCOMMAND);
            }
        }
        Some((Rc::new(cm), items.len()))
    }

    /// Shows `popup` at `pt` (screen) with `shell`'s menu getting the
    /// messages it needs while it is up; the id chosen (0 for none).
    fn track_with_shell(
        &self,
        popup: HMENU,
        pt: POINT,
        what: &str,
        shell: Option<&Rc<ContextMenu>>,
    ) -> u32 {
        *self.shell_menu.borrow_mut() = shell.cloned();
        let last = u32::from(menu::SHELL_LAST);
        let verb = |id: u32| {
            shell
                .filter(|cm| cm.owns(id, last))
                .and_then(|cm| cm.verb(id))
        };
        let id = popup::track(popup, self.hwnd, pt, what, &verb);
        self.shell_menu.borrow_mut().take();
        id
    }

    /// Runs the shell's item `id`, chosen at `pt`, for the `took` items
    /// the shell found of the `selected` rows. goz-gui's own commands say
    /// themselves when they took only part of a selection; for the shell's
    /// the status bar says so here.
    fn run_shell_item(&self, cm: &ContextMenu, id: u32, pt: POINT, took: usize, selected: usize) {
        // Acting on the results makes the search the one wanted.
        self.record_search();
        let verb = cm.verb(id).unwrap_or_default().to_ascii_lowercase();
        match verb.as_str() {
            "properties" => self.command(menu::PROPERTIES),
            "runas" => self.command(menu::RUN_AS),
            // As the Delete key does (Shift: for good), F2 and Ctrl+V.
            "delete" if key_down(VK_SHIFT) => self.command(menu::DELETE_PERMANENT),
            "delete" => self.command(menu::DELETE),
            "rename" => self.command(menu::RENAME),
            "paste" => self.command(menu::PASTE),
            // Create shortcut, as File > Create Shortcut: beside the items
            // (or on the Desktop, asked), and undoable.
            "link" => self.command(crate::filetools::cmd::CREATE_SHORTCUT),
            _ => {
                let mask = shell::invoke_mask(
                    key_down(VK_CONTROL),
                    key_down(VK_SHIFT),
                    location::fileop_silent(),
                );
                if cm.invoke(self.hwnd, id, pt, mask) {
                    let act = match verb.as_str() {
                        "copy" => Act::Copy,
                        "cut" => Act::Cut,
                        _ => Act::Command,
                    };
                    self.note_partial(act, took, selected);
                }
            }
        }
    }

    /// The menu of the list's empty space at `pt` (screen): Sort By and
    /// Refresh, and New Folder (`tools.rs`).
    fn background_menu(&self, pt: POINT) {
        let popup = menu::background_menu();
        self.add_background_items(popup);
        let id = self.track_with_shell(popup, pt, "background menu", None);
        // SAFETY: the menu this function made.
        unsafe { DestroyMenu(popup) };
        if id > 0 {
            self.command(id as u16);
        }
    }

    /// A message of a menu while the shell's is up, passed to it: it fills
    /// its submenus as they open and draws its own items. `Some` is the
    /// window's answer when the shell took the message; for
    /// WM_INITMENUPOPUP the window readies its own menus too.
    pub(super) fn shell_menu_message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        let owner_drawn = |l: isize| {
            // SAFETY: WM_MEASUREITEM's and WM_DRAWITEM's lParam starts with
            // the control type (MEASUREITEMSTRUCT, DRAWITEMSTRUCT).
            unsafe { *(l as *const u32) == ODT_MENU }
        };
        let take = match msg {
            WM_INITMENUPOPUP | WM_MENUCHAR => true,
            WM_MEASUREITEM | WM_DRAWITEM => l != 0 && owner_drawn(l),
            _ => false,
        };
        if !take {
            return None;
        }
        // Out of the cell before the call: the shell's handler may send
        // the window more messages.
        let cm = self.shell_menu.borrow().clone()?;
        let answer = cm.message(msg, w, l);
        match msg {
            WM_MEASUREITEM | WM_DRAWITEM => answer.map(|_| 1),
            _ => answer,
        }
    }

    /// Set Run Count... on the selection: asks for the count, starting
    /// from the first selected item's.
    pub(super) fn set_run_count(&self) {
        if !self.run_history_enabled() || self.fetch_first(Then::RunCount) {
            return;
        }
        let (paths, _) = self.picked_paths();
        let files: Vec<String> = paths
            .iter()
            .filter_map(|p| String::from_utf16(p).ok())
            .collect();
        let Some(first) = files.first() else {
            return;
        };
        let initial = self.run_count(first);
        let count = {
            // Opened by a command posted while goz-gui is in the
            // background: shown, but not brought to the front.
            let _quiet = Quiet::begin();
            run_count::ask(self.hwnd, initial, self.dialog_look())
        };
        if let Some(count) = count {
            self.set_run_counts(&files, count);
        }
    }
}
