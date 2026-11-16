//! The search tools' window side (F4): their menu items and commands;
//! Invert Selection and Go To... (Edit menu); Window Size and Fullscreen
//! (View menu, F11); the search box's own menu; Advanced Search... (Search
//! menu). The find bar is `findbar.rs`, the user's filters
//! `custom_filters.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL};
use windows_sys::Win32::UI::Controls::{LVIS_SELECTED, LVNI_FOCUSED, LVNI_SELECTED};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HMENU, MB_OK, SM_CXVSCROLL, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
    WINDOWPLACEMENT, WM_CLEAR, WM_CONTEXTMENU, WM_COPY, WM_CUT, WM_PASTE, WS_CAPTION,
    WS_THICKFRAME,
};

use super::findbar::{FindBar, Finding, Report, TIMER_TYPED, WM_FIND};
use super::selecting::Step;
use super::{App, slots};
use crate::menu::{self, Item};
use crate::searchtools::{self as st, Insert};
use crate::settings::WindowPos;
use crate::strip::StripSlot;
use crate::win::menu::By;
use crate::win::{self, Msg, edit, lv, parcel, sys};
use crate::{advsearch, clipboard, goto, placement};

/// Enters fullscreen once the window has shown, as it was left
/// (`fullscreen=1`).
pub(super) const WM_FULLSCREEN_AT_START: u32 = slots::F4_MSG + 1;

/// The window as it was before it went fullscreen.
struct Full {
    placement: WINDOWPLACEMENT,
    /// Its rectangle on the screen, if it was neither maximized nor
    /// minimized: put back exactly, as `SetWindowPlacement` alone moves a
    /// window that was partly or wholly off every monitor onto one.
    rect: Option<RECT>,
    /// That rectangle was on a monitor then (W5R-4): it is put back
    /// exactly only while it still is, else the placement alone puts the
    /// window on one (a monitor unplugged while fullscreen).
    rect_on_monitor: bool,
    style: u32,
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
    // By command; a missing one is reported and ignored.
    win::menu::enable(menu, By::Command(u32::from(id)), on);
}

fn check(menu: HMENU, id: u16, on: bool) {
    // As in `enable`.
    win::menu::check(menu, By::Command(u32::from(id)), on);
}

/// The position of the item that runs `cmd` in `menu`.
fn position_of(menu: HMENU, cmd: u16) -> Option<u32> {
    let n = win::menu::count(menu).max(0) as u32;
    (0..n).find(|&i| win::menu::item_id(menu, i as i32) == u32::from(cmd))
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
            // One item of this window's View menu, by position: the
            // separator two after Window Size's own.
            if win::menu::is_separator(m.view, By::Position(at + 3)) {
                win::menu::delete(m.view, By::Position(at + 3));
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
            // After the window has shown.
            win::post_app(self.hwnd, WM_FULLSCREEN_AT_START, 0, 0);
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
        let count = lv::item_count(self.list);
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
    pub(super) fn tools_message(&self, m: &Msg) -> Option<LRESULT> {
        match m.id {
            WM_FIND => {
                // What a find's thread posted.
                if let Some(report) = parcel::take::<Report>(m) {
                    self.find_report(report);
                }
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
        // The selection as it is to be, rows still being set included.
        self.finish_selecting();
        let count = lv::item_count(self.list);
        if count == 0 {
            return;
        }
        let selected = lv::selected_count(self.list);
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
        // not wanted, and they are not Select All's every row (inverting
        // none selects every row as it is now, as a click on each would).
        // The rows of the smaller side are set a slice at a time
        // (`selecting.rs`): a few thousand of them cost the list tens of
        // milliseconds; the status bar and the panes hear of it once, when
        // all are set.
        self.cancel_fetch();
        self.select_all.set(false);
        self.select_rows(
            std::iter::once(Step::All(all)).chain(rows.into_iter().map(|r| Step::Rows(r, !all))),
        );
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
            let bits = lv::item_state(self.list, row as i32, LVIS_SELECTED);
            (bits & LVIS_SELECTED != 0) == on
        });
        agrees.then_some(sel)
    }

    /// Every selected row, one range each, asked of the list row by row:
    /// when the kept selection does not agree with it.
    fn selected_scanned(&self) -> Vec<core::ops::Range<usize>> {
        let mut out = Vec::new();
        let mut at = -1;
        loop {
            let next = lv::next_item(self.list, at, LVNI_SELECTED);
            if next < 0 {
                break;
            }
            out.push(next as usize..next as usize + 1);
            at = next;
        }
        out
    }

    // ---- Edit > Go To... ----

    /// Asks for a result number and selects that result alone, the
    /// keyboard in the results.
    fn go_to(&self) {
        let count = lv::item_count(self.list) as u64;
        if count == 0 {
            sys::beep(MB_OK);
            return;
        }
        let focused =
            usize::try_from(lv::next_item(self.list, -1, LVNI_FOCUSED)).map_or(1, |i| i as u64 + 1);
        let chosen = {
            let _quiet = crate::dialog::Quiet::begin();
            goto::ask(self.hwnd, count, focused, self.dialog_look())
        };
        let Some(n) = chosen else { return };
        // The rows may have gone meanwhile (a live update).
        let count = lv::item_count(self.list) as u64;
        if n == 0 || n > count {
            return;
        }
        self.select_row((n - 1) as usize);
        if win::active() == self.hwnd {
            win::set_focus(self.list);
        } else {
            self.last_focus.set(self.list);
        }
    }

    // ---- View > Window Size and Fullscreen ----

    /// The work area of the window's monitor.
    fn work_area(&self) -> RECT {
        // MonitorFromWindow never fails with DEFAULTTONEAREST.
        let mon = sys::monitor_from_window(self.hwnd, MONITOR_DEFAULTTONEAREST);
        sys::monitor_info(mon).map_or_else(RECT::default, |mi| mi.rcWork)
    }

    /// Puts the window back to its normal size first: from fullscreen,
    /// from maximized.
    fn to_normal(&self) {
        if self.tools.full.borrow().is_some() {
            self.leave_fullscreen();
        }
        if win::is_zoomed(self.hwnd) {
            win::show(self.hwnd, SW_RESTORE);
        }
    }

    /// Sizes the window as `size` (pixels), its top left corner where it
    /// is, kept on its monitor.
    fn resize_window(&self, size: (i32, i32)) {
        let rc = win::window_rect(self.hwnd);
        let work = self.work_area();
        let (l, t, r, b) = st::sized_within(
            (rc.left, rc.top, rc.right, rc.bottom),
            size,
            (work.left, work.top, work.right, work.bottom),
        );
        win::set_pos(
            self.hwnd,
            core::ptr::null_mut(),
            (l, t, r - l, b - t),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
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
            .map(|i| lv::column_width(self.list, i as i32))
            .sum();
        let window = win::window_rect(self.hwnd);
        let list = win::client_rect(self.list);
        let scroll = sys::metric_for_dpi(SM_CXVSCROLL, self.dpi.get());
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
        let style = win::style(self.hwnd);
        let Some(placement) = win::placement(self.hwnd) else {
            return;
        };
        let rect = (!win::is_zoomed(self.hwnd) && !win::is_iconic(self.hwnd))
            .then(|| win::window_rect(self.hwnd));
        let keep_menu = self.prefs.borrow().base.search_tools.fullscreen_menu;
        *self.tools.full.borrow_mut() = Some(Full {
            placement,
            rect,
            rect_on_monitor: rect.as_ref().is_some_and(on_a_monitor),
            style,
            menu_hidden: !keep_menu,
        });
        // MonitorFromWindow never fails with DEFAULTTONEAREST.
        let mon = sys::monitor_from_window(self.hwnd, MONITOR_DEFAULTTONEAREST);
        let monitor = sys::monitor_info(mon).map_or_else(RECT::default, |mi| mi.rcMonitor);
        // The window's menu, its frame, then its place, which sends WM_SIZE
        // (no borrow is held).
        if !keep_menu {
            win::menu::set_for_window(self.hwnd, core::ptr::null_mut());
        }
        let frame = WS_CAPTION | WS_THICKFRAME;
        win::set_style(self.hwnd, style & !frame);
        win::set_pos(
            self.hwnd,
            core::ptr::null_mut(),
            (
                monitor.left,
                monitor.top,
                monitor.right - monitor.left,
                monitor.bottom - monitor.top,
            ),
            SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE,
        );
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
        // The window goes back as it was read.
        win::set_style(self.hwnd, full.style);
        if full.menu_hidden {
            win::menu::set_for_window(self.hwnd, self.menus.bar);
        }
        win::set_placement(self.hwnd, &placement);
        let flags = SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED | SWP_NOACTIVATE;
        match full
            .rect
            .filter(|rc| exactly_back(full.rect_on_monitor, on_a_monitor(rc)))
        {
            // Exactly where it was, on a monitor or not (the placement
            // alone brings one that was off every monitor onto one);
            // not where no monitor is any more.
            Some(rc) => win::set_pos(
                self.hwnd,
                core::ptr::null_mut(),
                (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top),
                flags,
            ),
            None => win::set_pos(
                self.hwnd,
                core::ptr::null_mut(),
                (0, 0, 0, 0),
                SWP_NOMOVE | SWP_NOSIZE | flags,
            ),
        };
        if placement.showCmd == SW_SHOWMAXIMIZED as u32 && win::active() != self.hwnd {
            self.maximize_deferred.set(true);
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

    /// The search box's messages F4 takes: its context menu. (F2, Advanced
    /// Search... there, is one of the box's keys in `keys::SEARCH_BOX_KEYS`,
    /// which Options > Keyboard can rebind.)
    pub(super) fn search_box_message(&self, m: &Msg) -> Option<LRESULT> {
        match m.id {
            WM_CONTEXTMENU => {
                self.search_box_menu(m.l);
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
        let (start, end) = edit::sel(edit);
        let selection = start != end;
        let has_text = win::text_len(edit) > 0;
        // OR, NOT and Group with a gozd that reads them.
        let operator = if crate::client::boolean_syntax() {
            menu::submenu(&[
                Item::Cmd(st::BOX_AND, "AND\tspace"),
                Item::Cmd(st::BOX_OR, "OR\t|"),
                Item::Cmd(st::BOX_NOT, "NOT\t!"),
                Item::Cmd(st::BOX_GROUP, "Group\t< >"),
                Item::Cmd(st::BOX_QUOTE, "Quote\t\" \""),
            ])
        } else {
            menu::submenu(&[
                Item::Cmd(st::BOX_AND, "AND\tspace"),
                Item::Cmd(st::BOX_QUOTE, "Quote\t\" \""),
            ])
        };
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
        // W7: on a file-list term (under the mouse, or the caret from the
        // keyboard), Everything's "Edit File List Slot..." comes first ("Edit
        // File List..." for a term's own names).
        let at = if l == -1 {
            Some(start as usize)
        } else {
            let p = POINT {
                x: (l & 0xffff) as i16 as i32,
                y: ((l >> 16) & 0xffff) as i16 as i32,
            };
            let p = win::screen_to_client(edit, p);
            // -1 (both words all ones) for a point outside the box.
            let (ch, line) = edit::char_from_pos(edit, p);
            ((ch, line) != (u16::MAX, u16::MAX)).then_some(usize::from(ch))
        };
        // W8: not on `filelist-filename:` (nothing to edit), nor on a term
        // whose file Ctrl+click and F2 pick (`include-file-list:`,
        // `open-filelist:`: no label of Everything's is known for a menu).
        let term = at
            .and_then(|u| crate::filelist::slots::term_at(&win::text(edit), u))
            .filter(|t| t.func.editable() && !t.func.picks_file());
        let mut items = Vec::with_capacity(16);
        if let Some(t) = &term {
            items.push(match t.slot {
                Some(_) => Item::Cmd(crate::slotedit::cmd::EDIT_SLOT, "Edit File List Slot..."),
                None => Item::Cmd(crate::slotedit::cmd::EDIT_LIST, "Edit File List..."),
            });
            items.push(Item::Sep);
        }
        items.extend([
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
        let popup = menu::submenu(&items);
        enable(popup, st::BOX_UNDO, edit::can_undo(edit));
        enable(popup, st::BOX_CUT, selection);
        enable(popup, st::BOX_COPY, selection);
        enable(popup, st::BOX_DELETE, selection);
        enable(popup, st::BOX_PASTE, clipboard::has_text());
        enable(popup, st::BOX_SELECT_ALL, has_text);
        let pt = if l == -1 {
            // From the keyboard: under the caret.
            // This thread's caret, mapped to the screen from the box.
            let mut p = win::sys::caret_pos();
            let h = self.search_font_height();
            p.y += h;
            win::client_to_screen(edit, p)
        } else {
            POINT {
                x: (l & 0xffff) as i16 as i32,
                y: ((l >> 16) & 0xffff) as i16 as i32,
            }
        };
        let id = crate::popup::track(popup, self.hwnd, pt, "search box menu", &|_| None);
        // The menu this function made, its submenus with it.
        win::menu::destroy(popup);
        let id = id as u16;
        match id {
            st::BOX_UNDO => {
                edit::undo(edit);
            }
            st::BOX_CUT => {
                win::send_plain(edit, WM_CUT, 0, 0);
            }
            st::BOX_COPY => {
                win::send_plain(edit, WM_COPY, 0, 0);
            }
            st::BOX_PASTE => {
                win::send_plain(edit, WM_PASTE, 0, 0);
            }
            st::BOX_DELETE => {
                win::send_plain(edit, WM_CLEAR, 0, 0);
            }
            st::BOX_SELECT_ALL => {
                edit::set_sel(edit, 0, -1);
            }
            st::BOX_LOCATED_IN => {
                if let Some(folder) = self.pick_folder() {
                    self.put_in_search(&st::folder_term(&folder), Insert::Term);
                }
            }
            st::ADVANCED_SEARCH => self.advanced_search(),
            crate::slotedit::cmd::EDIT_SLOT | crate::slotedit::cmd::EDIT_LIST => {
                if let Some(t) = term {
                    self.edit_file_list_term(t);
                }
            }
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
        let (start, end) = edit::sel(edit);
        let text: Vec<u16> = win::text(edit).encode_utf16().collect();
        let (start, end) = (start as usize, end as usize);
        let (out, (caret, _)) = st::insert(&text, (start, end), piece, how);
        let (lo, hi) = (
            start.min(end).min(text.len()),
            start.max(end).min(text.len()),
        );
        let tail = text.len() - hi;
        let mut middle: Vec<u16> = out[lo..out.len() - tail].to_vec();
        middle.push(0);
        edit::set_sel(edit, lo as i32, hi as i32);
        // The box's EN_CHANGE searches again.
        win::edit::replace_sel_wide(edit, &middle, true);
        edit::set_sel(edit, caret as i32, caret as i32);
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
                    boolean_syntax: crate::client::boolean_syntax(),
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

/// Whether any of `rc` is on a monitor.
fn on_a_monitor(rc: &RECT) -> bool {
    // DEFAULTTONULL gives null for none.
    !sys::monitor_from_rect(rc, MONITOR_DEFAULTTONULL).is_null()
}

/// Whether leaving fullscreen puts the window back at its exact old
/// rectangle: while that is on a monitor (`on_now`), or when it was off
/// every monitor already before (a test's window, kept off screen);
/// otherwise the placement alone, which Windows brings onto a monitor.
fn exactly_back(was_on: bool, on_now: bool) -> bool {
    on_now || !was_on
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_goes_back_where_a_monitor_is() {
        // W5R-4: on a monitor at F11, on none at the second F11 (it was
        // unplugged meanwhile): the placement alone, not the old place.
        assert!(!exactly_back(true, false));
        assert!(exactly_back(true, true));
        // Off every monitor already at F11: back there exactly.
        assert!(exactly_back(false, false));
        assert!(exactly_back(false, true));
        let far = RECT {
            left: -2_000_000,
            top: -2_000_000,
            right: -1_999_000,
            bottom: -1_999_500,
        };
        assert!(!on_a_monitor(&far));
    }
}
