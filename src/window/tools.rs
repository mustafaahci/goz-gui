//! The search tools' window side (F4): their menu items and commands;
//! Invert Selection and Go To... (Edit menu); Window Size and Fullscreen
//! (View menu, F11); the search box's own menu; Advanced Search... (Search
//! menu). The find bar is `findbar.rs`, the user's filters
//! `custom_filters.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    EM_CANUNDO, EM_GETSEL, EM_REPLACESEL, EM_SETSEL, EM_UNDO, LVIS_SELECTED, LVM_GETCOLUMNWIDTH,
    LVM_GETITEMCOUNT, LVM_GETITEMSTATE, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVNI_FOCUSED,
    LVNI_SELECTED,
};
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetActiveWindow, SetFocus, VK_CONTROL, VK_F2, VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CheckMenuItem, DestroyMenu, EnableMenuItem, GWL_STYLE, GetCaretPos, GetClientRect,
    GetMenuItemCount, GetMenuItemID, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
    GetWindowTextLengthW, HMENU, IsZoomed, MB_OK, MF_BYCOMMAND, MF_BYPOSITION, MF_CHECKED,
    MF_ENABLED, MF_GRAYED, MF_UNCHECKED, PostMessageW, SM_CXVSCROLL, SW_RESTORE, SW_SHOWMAXIMIZED,
    SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetMenu, SetWindowLongPtrW, SetWindowPlacement,
    SetWindowPos, ShowWindow, WINDOWPLACEMENT, WM_CLEAR, WM_CONTEXTMENU, WM_COPY, WM_CUT,
    WM_KEYDOWN, WM_PASTE, WS_CAPTION, WS_THICKFRAME,
};

use super::findbar::{FindBar, Finding, Report, TIMER_TYPED, WM_FIND};
use super::{App, key_down, send, slots};
use crate::menu::{self, Item};
use crate::searchtools::{self as st, Insert};
use crate::settings::WindowPos;
use crate::strip::StripSlot;
use crate::wide::window_text;
use crate::{advsearch, clipboard, goto, placement};

/// Enters fullscreen once the window has shown, as it was left
/// (`fullscreen=1`).
pub(super) const WM_FULLSCREEN_AT_START: u32 = slots::F4_MSG + 1;

/// The window as it was before it went fullscreen.
struct Full {
    placement: WINDOWPLACEMENT,
    style: isize,
    /// The menu bar was taken away (`fullscreen_menu=0`).
    menu_hidden: bool,
}

/// What the search tools keep on the window.
pub(super) struct Tools {
    pub(super) find_bar: Rc<FindBar>,
    pub(super) finding: Finding,
    full: RefCell<Option<Full>>,
}

impl Tools {
    pub(super) fn new() -> Tools {
        Tools {
            find_bar: Rc::new(FindBar::new()),
            finding: Finding::default(),
            full: RefCell::new(None),
        }
    }
}

fn enable(menu: HMENU, id: u16, on: bool) {
    // SAFETY: an item of a menu of this window, by command; a missing one
    // is reported and ignored.
    unsafe {
        EnableMenuItem(
            menu,
            u32::from(id),
            MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED },
        )
    };
}

fn check(menu: HMENU, id: u16, on: bool) {
    // SAFETY: as in `enable`.
    unsafe {
        CheckMenuItem(
            menu,
            u32::from(id),
            MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED },
        )
    };
}

/// The position of the item that runs `cmd` in `menu`.
fn position_of(menu: HMENU, cmd: u16) -> Option<u32> {
    // SAFETY: reads the items of a menu of this thread.
    unsafe {
        let n = GetMenuItemCount(menu).max(0) as u32;
        (0..n).find(|&i| GetMenuItemID(menu, i as i32) == u32::from(cmd))
    }
}

impl App {
    /// The window starts: the find bar's row, the menu items, the user's
    /// filters in the Search menu, the find bar and fullscreen as they
    /// were left.
    pub(super) fn tools_start(&self) {
        let bar: Rc<dyn crate::strip::Strip> = self.tools.find_bar.clone();
        self.set_strip(StripSlot::FindBar, Some(bar));
        let m = self.menus;
        menu::add_items(
            m.edit,
            Some(menu::SELECT_ALL),
            &[
                Item::Cmd(st::INVERT_SELECTION, "&Invert Selection"),
                Item::Sep,
                Item::Cmd(st::FIND, "&Find...\tCtrl+F"),
                Item::Cmd(st::FIND_NEXT, "Find &Next\tF3"),
                Item::Cmd(st::GO_TO, "&Go To...\tCtrl+G"),
            ],
        );
        // View: Window Size before Zoom, in Details' group's end.
        let sizes = menu::submenu(&[
            Item::Cmd(st::WINDOW_SMALL, "&Small\tAlt+1"),
            Item::Cmd(st::WINDOW_MEDIUM, "&Medium\tAlt+2"),
            Item::Cmd(st::WINDOW_LARGE, "&Large\tAlt+3"),
            Item::Cmd(st::WINDOW_AUTO_FIT, "&Auto Fit\tAlt+4"),
        ]);
        menu::add_items(
            m.view,
            Some(menu::VIEW_DETAILS),
            &[Item::Sep, Item::Sub("&Window Size", sizes)],
        );
        // The separator that came after Details now follows Window Size:
        // it goes, so Window Size and Zoom are one group, as Everything's.
        if let Some(at) = position_of(m.view, menu::VIEW_DETAILS) {
            // SAFETY: deletes one item of this window's View menu by
            // position, the separator two after Window Size's own.
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DeleteMenu, GetMenuItemInfoW, MENUITEMINFOW, MFT_SEPARATOR, MIIM_FTYPE,
                };
                let mut mii: MENUITEMINFOW = core::mem::zeroed();
                mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
                mii.fMask = MIIM_FTYPE;
                if GetMenuItemInfoW(m.view, at + 3, 1, &mut mii) != 0
                    && mii.fType & MFT_SEPARATOR != 0
                {
                    DeleteMenu(m.view, at + 3, MF_BYPOSITION);
                }
            }
        }
        menu::add_items(
            m.search,
            Some(menu::MATCH_PATH),
            &[
                Item::Sep,
                Item::Cmd(st::ADVANCED_SEARCH, "Advanced &Search..."),
                Item::Sep,
                Item::Cmd(st::ADD_TO_FILTERS, "Add to Filters..."),
                Item::Cmd(st::ORGANIZE_FILTERS, "Organize Filters...\tCtrl+Shift+K"),
            ],
        );
        self.filters_menu();
        if let Some(note) = crate::filters::take_note() {
            self.show_note(note);
        }
        self.restore_find();
        if self.prefs.borrow().base.search_tools.fullscreen {
            // SAFETY: posts to this thread's window: after it has shown.
            unsafe { PostMessageW(self.hwnd, WM_FULLSCREEN_AT_START, 0, 0) };
        }
    }

    /// The window is ending: a find under way stops.
    pub(super) fn tools_exit(&self) {
        self.cancel_find();
    }

    /// A menu of the bar opens: what acts on the results greys without
    /// them; the user's filters are read again if another program changed
    /// them.
    pub(super) fn tools_menu_popup(&self, popup: HMENU) {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0);
        if popup == self.menus.edit {
            enable(popup, st::INVERT_SELECTION, count > 0);
            enable(popup, st::GO_TO, count > 0);
            enable(popup, st::FIND_NEXT, count > 0);
        } else if popup == self.menus.search {
            self.refresh_filters();
            // The filters' radio dots (the window's own update ran before
            // the user's were in place on a refresh).
            let filter = self.opts.get().filter;
            for f in crate::filter::Filter::all() {
                check(popup, f.menu_id(), f == filter);
            }
        }
    }

    /// A command of F4's: true when it is one.
    pub(super) fn tools_command(&self, id: u16) -> bool {
        match id {
            st::FIND => self.show_find(),
            st::FIND_NEXT => self.find_next(true),
            st::FIND_PREVIOUS => self.find_next(false),
            st::GO_TO => self.go_to(),
            st::INVERT_SELECTION => self.invert_selection(),
            st::ADVANCED_SEARCH => self.advanced_search(),
            st::ADD_TO_FILTERS => self.add_to_filters(),
            st::ORGANIZE_FILTERS => self.organize_filters(),
            st::FULLSCREEN => self.toggle_fullscreen(),
            st::WINDOW_SMALL => self.window_size(0),
            st::WINDOW_MEDIUM => self.window_size(1),
            st::WINDOW_LARGE => self.window_size(2),
            st::WINDOW_AUTO_FIT => self.auto_fit(),
            _ => return false,
        }
        true
    }

    /// F4's WM_APP messages.
    pub(super) fn tools_message(&self, msg: u32, _w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_FIND => {
                // SAFETY: a find's thread posted this box and gave it up.
                let report = unsafe { Box::from_raw(l as *mut Report) };
                self.find_report(*report);
                Some(0)
            }
            WM_FULLSCREEN_AT_START => {
                if self.tools.full.borrow().is_none() {
                    self.enter_fullscreen();
                }
                Some(0)
            }
            _ => None,
        }
    }

    /// F4's main-window timers.
    pub(super) fn tools_timer(&self, id: usize) -> bool {
        if id == TIMER_TYPED {
            self.find_typed();
            return true;
        }
        false
    }

    // ---- Edit > Invert Selection ----

    /// Selects the rows not selected and deselects the others, keeping the
    /// focus. With millions of rows the list holds the selection as ranges,
    /// but takes it only one row at a time, or all rows at once: every row
    /// is selected (or none), then the rows of the smaller side are set
    /// one by one. Which rows those are comes from the selection kept from
    /// the list's notifications (`selection.rs`), so inverting all rows but
    /// a few costs those few, not the millions selected.
    fn invert_selection(&self) {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        if count == 0 {
            return;
        }
        let selected = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as usize;
        // All rows selected first (and the ones selected now deselected
        // after), or none (and the ones not selected now selected after).
        let (all, rows) = match self.kept_selection(count, selected) {
            Some(sel) if selected <= count.saturating_sub(selected) => {
                (true, sel.ranges().collect())
            }
            Some(sel) => (false, sel.gaps()),
            None if selected >= count => (false, Vec::new()),
            None => (true, self.selected_scanned()),
        };
        // The user chose other rows: an action waiting for the old ones is
        // not wanted; the rows change unseen, then repaint once, and the
        // status bar and the panes hear of it once (`in_bulk`).
        self.cancel_fetch();
        self.in_bulk(|| {
            let (first, then) = if all {
                (LVIS_SELECTED, 0)
            } else {
                (0, LVIS_SELECTED)
            };
            self.set_state(usize::MAX, first, LVIS_SELECTED);
            for i in rows.into_iter().flatten() {
                self.set_state(i, then, LVIS_SELECTED);
            }
        });
        self.selection_changed();
    }

    /// The selection kept from the list's notifications, if it agrees with
    /// the list: as many rows of as many selected, and the list has the
    /// edges of its first ranges (and the rows beside them) as it does.
    fn kept_selection(&self, count: usize, selected: usize) -> Option<crate::selection::Selection> {
        let (samples, sel) = {
            let sel = self.sel.borrow();
            if sel.count() != count || sel.len() != selected {
                return None;
            }
            (sel.samples(64), sel.clone())
        };
        let agrees = samples.into_iter().all(|(row, on)| {
            let bits = send(self.list, LVM_GETITEMSTATE, row, LVIS_SELECTED as isize) as u32;
            (bits & LVIS_SELECTED != 0) == on
        });
        agrees.then_some(sel)
    }

    /// Every selected row, one range each, asked of the list row by row:
    /// when the kept selection does not agree with it.
    fn selected_scanned(&self) -> Vec<core::ops::Range<usize>> {
        let mut out = Vec::new();
        let mut at = usize::MAX;
        loop {
            let next = send(self.list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize);
            if next < 0 {
                break;
            }
            out.push(next as usize..next as usize + 1);
            at = next as usize;
        }
        out
    }

    // ---- Edit > Go To... ----

    /// Asks for a result number and selects that result alone, the
    /// keyboard in the results.
    fn go_to(&self) {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as u64;
        if count == 0 {
            // SAFETY: plays a system sound.
            unsafe { MessageBeep(MB_OK) };
            return;
        }
        let focused = usize::try_from(send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .map_or(1, |i| i as u64 + 1);
        let chosen = {
            let _quiet = crate::dialog::Quiet::begin();
            goto::ask(self.hwnd, count, focused, self.dialog_look())
        };
        let Some(n) = chosen else { return };
        // The rows may have gone meanwhile (a live update).
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as u64;
        if n == 0 || n > count {
            return;
        }
        self.select_row((n - 1) as usize);
        // SAFETY: reads which window is active.
        if unsafe { GetActiveWindow() } == self.hwnd {
            // SAFETY: focuses a child of this thread's window.
            unsafe { SetFocus(self.list) };
        } else {
            self.last_focus.set(self.list);
        }
    }

    // ---- View > Window Size and Fullscreen ----

    /// The work area of the window's monitor.
    fn work_area(&self) -> RECT {
        // SAFETY: plain out-structs; MonitorFromWindow never fails with
        // DEFAULTTONEAREST.
        unsafe {
            let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..core::mem::zeroed()
            };
            GetMonitorInfoW(mon, &mut mi);
            mi.rcWork
        }
    }

    /// Puts the window back to its normal size first: from fullscreen,
    /// from maximized.
    fn to_normal(&self) {
        if self.tools.full.borrow().is_some() {
            self.leave_fullscreen();
        }
        // SAFETY: reads and restores this thread's window.
        unsafe {
            if IsZoomed(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_RESTORE);
            }
        }
    }

    /// Sizes the window as `size` (pixels), its top left corner where it
    /// is, kept on its monitor.
    fn resize_window(&self, size: (i32, i32)) {
        let mut rc = RECT::default();
        // SAFETY: reads this thread's window.
        unsafe { GetWindowRect(self.hwnd, &mut rc) };
        let work = self.work_area();
        let (l, t, r, b) = st::sized_within(
            (rc.left, rc.top, rc.right, rc.bottom),
            size,
            (work.left, work.top, work.right, work.bottom),
        );
        // SAFETY: moves and sizes this thread's window.
        unsafe {
            SetWindowPos(
                self.hwnd,
                core::ptr::null_mut(),
                l,
                t,
                r - l,
                b - t,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// View > Window Size > Small, Medium or Large (`which` 0, 1, 2):
    /// Everything's `auto_size_*` widths at its aspect ratio.
    fn window_size(&self, which: usize) {
        self.to_normal();
        let size = self
            .prefs
            .borrow()
            .base
            .search_tools
            .preset(which, self.dpi.get());
        self.resize_window(size);
    }

    /// View > Window Size > Auto Fit: the columns sized to their text, and
    /// the window to the columns (and to the rows, when few).
    fn auto_fit(&self) {
        self.to_normal();
        self.size_all_columns_to_fit();
        let shown = self.list_cols.get().ids().len();
        let columns: i32 = (0..shown)
            .map(|i| send(self.list, LVM_GETCOLUMNWIDTH, i, 0) as i32)
            .sum();
        let (mut window, mut list) = (RECT::default(), RECT::default());
        // SAFETY: reads this thread's windows and a metric.
        let scroll = unsafe {
            GetWindowRect(self.hwnd, &mut window);
            GetClientRect(self.list, &mut list);
            GetSystemMetricsForDpi(SM_CXVSCROLL, self.dpi.get())
        };
        let chrome = (
            (window.right - window.left) - (list.right - list.left) + scroll + self.scale(2),
            (window.bottom - window.top) - (list.bottom - list.top) + self.header_height(),
        );
        let aspect = self.prefs.borrow().base.search_tools.aspect;
        let total = self.model.borrow().total;
        let size = st::auto_fit(
            columns,
            chrome,
            total,
            self.row_height(),
            aspect,
            (self.scale(420), self.scale(240)),
        );
        self.resize_window(size);
    }

    /// F11: fullscreen, or back.
    fn toggle_fullscreen(&self) {
        if self.tools.full.borrow().is_some() {
            self.leave_fullscreen();
        } else {
            self.enter_fullscreen();
        }
    }

    /// The window covers its monitor, without its frame (and without its
    /// menu bar, unless `fullscreen_menu=1`).
    fn enter_fullscreen(&self) {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            // SAFETY: the rest of WINDOWPLACEMENT is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        // SAFETY: reads this thread's window.
        let (style, ok) = unsafe {
            (
                GetWindowLongPtrW(self.hwnd, GWL_STYLE),
                GetWindowPlacement(self.hwnd, &mut placement) != 0,
            )
        };
        if !ok {
            return;
        }
        let keep_menu = self.prefs.borrow().base.search_tools.fullscreen_menu;
        *self.tools.full.borrow_mut() = Some(Full {
            placement,
            style,
            menu_hidden: !keep_menu,
        });
        // SAFETY: plain out-structs; MonitorFromWindow never fails with
        // DEFAULTTONEAREST.
        let monitor = unsafe {
            let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..core::mem::zeroed()
            };
            GetMonitorInfoW(mon, &mut mi);
            mi.rcMonitor
        };
        // SAFETY: changes this thread's window: its menu, its frame, then
        // its place, which sends WM_SIZE (no borrow is held).
        unsafe {
            if !keep_menu {
                SetMenu(self.hwnd, core::ptr::null_mut());
            }
            let frame = (WS_CAPTION | WS_THICKFRAME) as isize;
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, style & !frame);
            SetWindowPos(
                self.hwnd,
                core::ptr::null_mut(),
                monitor.left,
                monitor.top,
                monitor.right - monitor.left,
                monitor.bottom - monitor.top,
                SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE,
            );
        }
        self.set_fullscreen_setting(true);
    }

    /// Back from fullscreen: the frame, the menu bar and the place the
    /// window had.
    fn leave_fullscreen(&self) {
        let Some(full) = self.tools.full.borrow_mut().take() else {
            return;
        };
        let mut placement = full.placement;
        // Not activated by going back (it is the active window when the
        // user asks; a test's window is not); maximized comes back as it
        // was, which only an active window can be.
        if placement.showCmd == SW_SHOWNORMAL as u32 {
            placement.showCmd = SW_SHOWNOACTIVATE as u32;
        }
        // SAFETY: changes this thread's window back as it was read.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, full.style);
            if full.menu_hidden {
                SetMenu(self.hwnd, self.menus.bar);
            }
            SetWindowPlacement(self.hwnd, &placement);
            SetWindowPos(
                self.hwnd,
                core::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER
                    | SWP_NOOWNERZORDER
                    | SWP_FRAMECHANGED
                    | SWP_NOACTIVATE,
            );
            if placement.showCmd == SW_SHOWMAXIMIZED as u32 && GetActiveWindow() != self.hwnd {
                self.maximize_deferred.set(true);
            }
        }
        self.set_fullscreen_setting(false);
    }

    fn set_fullscreen_setting(&self, on: bool) {
        self.prefs.borrow_mut().base.search_tools.fullscreen = on;
        self.settings_changed();
    }

    /// While fullscreen, the window's place as the settings keep it: the
    /// one it goes back to.
    pub(super) fn fullscreen_place(&self) -> Option<(Option<WindowPos>, bool)> {
        let full = self.tools.full.borrow();
        let full = full.as_ref()?;
        Some(match placement::place_of(&full.placement) {
            Some((pos, maximized)) => (Some(pos), maximized),
            None => (None, false),
        })
    }

    // ---- The search box's menu, and F2 in it ----

    /// The search box's messages F4 takes: its context menu, and F2
    /// (Advanced Search..., Everything's key there).
    pub(super) fn search_box_message(&self, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_CONTEXTMENU => {
                self.search_box_menu(l);
                Some(0)
            }
            WM_KEYDOWN
                if w as u16 == VK_F2
                    && !key_down(VK_CONTROL)
                    && !key_down(VK_SHIFT)
                    && !key_down(VK_MENU) =>
            {
                self.advanced_search();
                Some(0)
            }
            _ => None,
        }
    }

    /// The search box's menu, at `l` (screen), or at the caret from the
    /// keyboard: Everything's editing items, then the syntax goz accepts,
    /// put in at the caret, and Advanced Search...
    fn search_box_menu(&self, l: LPARAM) {
        let edit = self.edit;
        let (mut start, mut end) = (0u32, 0u32);
        send(
            edit,
            EM_GETSEL,
            (&raw mut start) as usize,
            (&raw mut end) as isize,
        );
        let selection = start != end;
        // SAFETY: reads this thread's search box.
        let has_text = unsafe { GetWindowTextLengthW(edit) } > 0;
        let operator = menu::submenu(&[
            Item::Cmd(st::BOX_AND, "AND\tspace"),
            Item::Cmd(st::BOX_QUOTE, "Quote\t\" \""),
        ]);
        let modifier = menu::submenu(&[
            Item::Cmd(st::BOX_CASE, "Match Case\tcase:"),
            Item::Cmd(st::BOX_PATH, "Match Path\tpath:"),
            Item::Sep,
            Item::Cmd(st::BOX_FILES, "Match Files Only\tfile:"),
            Item::Cmd(st::BOX_FOLDERS, "Match Folders Only\tfolder:"),
        ]);
        let function = menu::submenu(&[
            Item::Cmd(st::BOX_LOCATED_IN, "Located in..."),
            Item::Cmd(st::BOX_DM_BEFORE, "Date Modified Before\tdm:<"),
            Item::Cmd(st::BOX_DM_AFTER, "Date Modified After\tdm:>"),
            Item::Cmd(st::BOX_DM_TODAY, "Date Modified Today\tdm:today"),
            Item::Cmd(st::BOX_SIZE_SMALLER, "Size Smaller Than\tsize:<"),
            Item::Cmd(st::BOX_SIZE_LARGER, "Size Larger Than\tsize:>"),
            Item::Cmd(st::BOX_EXT, "Extension\text:"),
        ]);
        let popup = menu::submenu(&[
            Item::Cmd(st::BOX_UNDO, "&Undo"),
            Item::Sep,
            Item::Cmd(st::BOX_CUT, "Cu&t"),
            Item::Cmd(st::BOX_COPY, "&Copy"),
            Item::Cmd(st::BOX_PASTE, "&Paste"),
            Item::Cmd(st::BOX_DELETE, "&Delete"),
            Item::Sep,
            Item::Cmd(st::BOX_SELECT_ALL, "Select &All"),
            Item::Sep,
            Item::Sub("Search Operator", operator),
            Item::Sub("Search Modifier", modifier),
            Item::Sub("Search Function", function),
            Item::Cmd(st::ADVANCED_SEARCH, "Advanced Search..."),
        ]);
        enable(popup, st::BOX_UNDO, send(edit, EM_CANUNDO, 0, 0) != 0);
        enable(popup, st::BOX_CUT, selection);
        enable(popup, st::BOX_COPY, selection);
        enable(popup, st::BOX_DELETE, selection);
        enable(popup, st::BOX_PASTE, clipboard::has_text());
        enable(popup, st::BOX_SELECT_ALL, has_text);
        let pt = if l == -1 {
            // From the keyboard: under the caret.
            let mut p = POINT::default();
            // SAFETY: reads the caret of this thread and maps it to the
            // screen from the box.
            unsafe {
                GetCaretPos(&mut p);
                let h = self.ui_fonts.borrow().height;
                p.y += h;
                ClientToScreen(edit, &mut p);
            }
            p
        } else {
            POINT {
                x: (l & 0xffff) as i16 as i32,
                y: ((l >> 16) & 0xffff) as i16 as i32,
            }
        };
        let id = crate::popup::track(popup, self.hwnd, pt, "search box menu", &|_| None);
        // SAFETY: the menu this function made, its submenus with it.
        unsafe { DestroyMenu(popup) };
        let id = id as u16;
        match id {
            st::BOX_UNDO => {
                send(edit, EM_UNDO, 0, 0);
            }
            st::BOX_CUT => {
                send(edit, WM_CUT, 0, 0);
            }
            st::BOX_COPY => {
                send(edit, WM_COPY, 0, 0);
            }
            st::BOX_PASTE => {
                send(edit, WM_PASTE, 0, 0);
            }
            st::BOX_DELETE => {
                send(edit, WM_CLEAR, 0, 0);
            }
            st::BOX_SELECT_ALL => {
                send(edit, EM_SETSEL, 0, -1);
            }
            st::BOX_LOCATED_IN => {
                if let Some(folder) = self.pick_folder() {
                    self.put_in_search(&st::folder_term(&folder), Insert::Term);
                }
            }
            st::ADVANCED_SEARCH => self.advanced_search(),
            other => {
                if let Some((piece, how)) = st::box_syntax(other) {
                    self.put_in_search(piece, how);
                }
            }
        }
    }

    /// Puts `piece` in the search box at its selection (`st::insert`), as
    /// typing would (it can be undone), the keyboard there.
    fn put_in_search(&self, piece: &str, how: Insert) {
        let edit = self.edit;
        let (mut start, mut end) = (0u32, 0u32);
        send(
            edit,
            EM_GETSEL,
            (&raw mut start) as usize,
            (&raw mut end) as isize,
        );
        let text: Vec<u16> = window_text(edit).encode_utf16().collect();
        let (start, end) = (start as usize, end as usize);
        let (out, (caret, _)) = st::insert(&text, (start, end), piece, how);
        let (lo, hi) = (
            start.min(end).min(text.len()),
            start.max(end).min(text.len()),
        );
        let tail = text.len() - hi;
        let mut middle: Vec<u16> = out[lo..out.len() - tail].to_vec();
        middle.push(0);
        send(edit, EM_SETSEL, lo, hi as isize);
        // The box's EN_CHANGE searches again.
        send(edit, EM_REPLACESEL, 1, middle.as_ptr() as isize);
        send(edit, EM_SETSEL, caret, caret as isize);
        self.focus_box();
    }

    /// Asks for a folder (Located in..., Browse...), starting from the one
    /// chosen last, which is kept (`search_edit_last_located_in`).
    pub(super) fn pick_folder(&self) -> Option<String> {
        let last = self
            .prefs
            .borrow()
            .base
            .search_tools
            .last_located_in
            .clone();
        let chosen = {
            let _quiet = crate::dialog::Quiet::begin();
            advsearch::pick_folder(self.hwnd, &last)
        }?;
        self.prefs.borrow_mut().base.search_tools.last_located_in = chosen.clone();
        self.settings_changed();
        Some(chosen)
    }

    // ---- Search > Advanced Search... ----

    /// Builds a search from the dialog's fields; it replaces the one in
    /// the box, with the filter chosen there.
    fn advanced_search(&self) {
        let filters = crate::filter::Filter::all();
        let current = self.opts.get().filter;
        let last = self
            .prefs
            .borrow()
            .base
            .search_tools
            .last_located_in
            .clone();
        let result = {
            let _quiet = crate::dialog::Quiet::begin();
            advsearch::ask(
                self.hwnd,
                &advsearch::Start {
                    filters: filters.iter().map(|f| f.title().to_owned()).collect(),
                    filter: filters.iter().position(|&f| f == current).unwrap_or(0),
                    last_folder: last,
                },
                self.dialog_look(),
            )
        };
        let Some(done) = result else { return };
        if let Some(folder) = done.browsed {
            self.prefs.borrow_mut().base.search_tools.last_located_in = folder;
            self.settings_changed();
        }
        if let Some(&filter) = filters.get(done.filter) {
            let mut o = self.opts.get();
            o.filter = filter;
            self.opts.set(o);
            self.invalidate_status();
        }
        self.set_search_text(&done.query);
        self.focus_box();
    }
}
