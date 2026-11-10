//! Tabs (Everything 1.5's `tabs`, `tab-context-menu` and `tab-bar-options`):
//! File > New Tab, Close Tab, Ctrl+Tab and Ctrl+Shift+Tab, Reopen Closed
//! Tab, the tab strip's menu (`tabstrip.rs` draws the strip), History >
//! Recently Closed Tabs, and the tabs in the session.
//!
//! Only the shown tab is live: its search, options, view, columns, scroll
//! position, selection and navigation history are the window's own state,
//! and every other tab is kept as a `session::View`, the shape a session
//! keeps a tab in. Showing another tab stores the shown one's state in its
//! place (`current_view`) and shows the other's as a restored session's is
//! shown (`show_view`): its view and columns, then its search, with its
//! place and selection once its rows come. A hidden tab holds no rows and
//! runs no searches. The list does not repaint between the old tab's rows
//! and the new one's: it is frozen until the new rows are in place (at most
//! [`THAW_MS`]).

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{
    RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow,
};
use windows_sys::Win32::UI::Controls::LVM_CANCELEDITLABEL;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyMenu, EnableMenuItem, GetMenuState, HMENU, KillTimer, MF_BYCOMMAND, MF_ENABLED,
    MF_GRAYED, PostMessageW, SetTimer, WM_CLOSE, WM_SETREDRAW,
};

use super::{App, send, slots};
use crate::dock::PaneId;
use crate::menu::{self, Item};
use crate::session::{Search, Session, View};
use crate::strip::StripSlot;
use crate::tabs::{self, Others, TabList, TabSettings};
use crate::wide::window_text;

/// The timer that lets the list paint again if a tab's rows are slow to
/// come (gozd busy or gone).
pub(super) const TIMER_THAW: usize = slots::F3_TIMER;
/// How long the list stays as it was while a tab's rows come.
pub(super) const THAW_MS: u32 = 300;
/// The overflow list's entries (its own menu, which returns the choice,
/// clear of every command the window's menus check or grey).
const OVERFLOW_FIRST: u16 = 0x8000;

impl App {
    /// The tab settings (`tabs_*`, from the settings file).
    pub(super) fn tab_settings(&self) -> TabSettings {
        self.prefs.borrow().base.tabs
    }

    pub(super) fn tab_count(&self) -> usize {
        self.tabs.borrow().len()
    }

    pub(super) fn current_tab(&self) -> usize {
        self.tabs.borrow().current()
    }

    /// Whether the tab strip shows now.
    pub(super) fn tab_strip_shown(&self) -> bool {
        self.tab_settings().strip_shown(self.tab_count())
    }

    /// Each tab's title, left to right: the shown one's as its search box
    /// has it now.
    pub(super) fn tab_titles(&self) -> Vec<String> {
        let live = window_text(self.edit);
        let t = self.tabs.borrow();
        t.tabs()
            .iter()
            .enumerate()
            .map(|(i, v)| {
                tabs::title(if i == t.current() {
                    &live
                } else {
                    &v.search.text
                })
            })
            .collect()
    }

    /// The search in the box changed: the shown tab's title with it.
    pub(super) fn tabs_search_changed(&self) {
        if self.tab_strip_shown() {
            self.invalidate_strip(StripSlot::Tabs);
        }
    }

    /// Keeps the shown tab's state in its place in the list.
    fn store_current_tab(&self) {
        // Read before the list is borrowed: it asks the results list.
        let view = self.current_view();
        let mut t = self.tabs.borrow_mut();
        let i = t.current();
        t.set(i, view);
    }

    /// The shown tab is about to give way: a rename in progress is called
    /// off (its box would sit over another tab's rows), and the cell tip
    /// goes.
    fn leave_tab(&self) {
        if self.renaming() {
            send(self.list, LVM_CANCELEDITLABEL, 0, 0);
        }
        crate::celltip::hide(self);
    }

    /// Shows `view` as the shown tab's, with the list frozen until its
    /// rows are in place.
    fn show_tab(&self, view: &View) {
        self.freeze_list();
        self.show_view(view);
    }

    /// The list stops painting: what it shows stays on screen until the
    /// new tab's rows are in place (`thaw_list`), or [`THAW_MS`] at most.
    fn freeze_list(&self) {
        if self.list_frozen.replace(true) {
            return;
        }
        send(self.list, WM_SETREDRAW, 0, 0);
        // SAFETY: a timer of this window; WM_TIMER kills it.
        unsafe { SetTimer(self.hwnd, TIMER_THAW, THAW_MS, None) };
    }

    /// The list paints again, all of it, once: the new tab's rows are in
    /// place (a restored view applied), or they are slow to come.
    pub(super) fn thaw_list(&self) {
        if !self.list_frozen.replace(false) {
            return;
        }
        // SAFETY: this window's timer, and a repaint of its list.
        unsafe {
            KillTimer(self.hwnd, TIMER_THAW);
            send(self.list, WM_SETREDRAW, 1, 0);
            RedrawWindow(
                self.list,
                core::ptr::null(),
                core::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
            );
        }
    }

    /// The tabs changed: the strip shows or goes (a layout), or repaints.
    fn tabs_changed(&self, was_shown: bool) {
        if self.tab_strip_shown() == was_shown {
            self.invalidate_strip(StripSlot::Tabs);
        } else {
            self.relayout();
        }
    }

    /// A new tab's state: Home's search and options over the shown tab's
    /// (Everything's Options > Home; by default the empty search with the
    /// match options, filter and sort as they are), its view and columns.
    fn home_view(&self) -> View {
        let home = self.prefs.borrow().base.home();
        let text = home.search.unwrap_or_else(|| window_text(self.edit));
        let o = self.opts.get();
        let h = home.options;
        View {
            search: Search {
                text,
                match_case: h.match_case.unwrap_or(o.match_case),
                match_path: h.match_path.unwrap_or(o.match_path),
                filter: h.filter.unwrap_or(o.filter),
                sort: h.sort.unwrap_or(o.sort),
            },
            display: Some(self.view()),
            columns: Some(self.layout.get()),
            ..View::default()
        }
    }

    /// Shows tab `i`.
    pub(super) fn select_tab(&self, i: usize) {
        if i == self.current_tab() || i >= self.tab_count() {
            return;
        }
        let shown = self.tab_strip_shown();
        self.leave_tab();
        self.store_current_tab();
        let view = {
            let mut t = self.tabs.borrow_mut();
            t.select(i);
            t.get(i).cloned()
        };
        if let Some(view) = view {
            self.show_tab(&view);
        }
        self.tabs_changed(shown);
    }

    /// File > New Tab: Home in a new tab, after the last one (or right of
    /// the shown one, `tabs_insert_after_current`), with the keyboard in
    /// the search box.
    pub(super) fn new_tab(&self) {
        let shown = self.tab_strip_shown();
        let view = self.home_view();
        self.leave_tab();
        self.store_current_tab();
        {
            let mut t = self.tabs.borrow_mut();
            let at = if self.tab_settings().insert_after_current {
                t.current() + 1
            } else {
                t.len()
            };
            let i = t.insert(at, view.clone());
            t.select(i);
        }
        self.show_tab(&view);
        self.tabs_changed(shown);
        self.command(menu::FOCUS_SEARCH);
    }

    /// The tab menu's Duplicate Tab: a copy of tab `i` right of it, shown.
    pub(super) fn duplicate_tab(&self, i: usize) {
        let shown = self.tab_strip_shown();
        self.leave_tab();
        self.store_current_tab();
        let view = {
            let mut t = self.tabs.borrow_mut();
            let Some(view) = t.get(i).cloned() else {
                return;
            };
            let j = t.insert(i + 1, view.clone());
            t.select(j);
            view
        };
        self.show_tab(&view);
        self.tabs_changed(shown);
    }

    /// File > Close Tab (tab `i`): the tab right of it shows (the one left
    /// of it for the last); the closed one can be reopened. The window's
    /// only tab closes the window (`tabs_close_window_with_last_tab=1`), or
    /// gives way to a new one.
    pub(super) fn close_tab(&self, i: usize) {
        let n = self.tab_count();
        if i >= n {
            return;
        }
        if n == 1 {
            if self.tab_settings().close_window_with_last_tab {
                self.close_window();
                return;
            }
            let home = self.home_view();
            self.leave_tab();
            self.store_current_tab();
            {
                let mut t = self.tabs.borrow_mut();
                if let Some(old) = t.get(0).cloned() {
                    t.push_closed(0, old);
                }
                t.set(0, home.clone());
            }
            self.show_tab(&home);
            self.invalidate_strip(StripSlot::Tabs);
            return;
        }
        let shown = self.tab_strip_shown();
        let current = self.current_tab();
        if i == current {
            // Kept as it is now, to be reopened so.
            self.leave_tab();
            self.store_current_tab();
        }
        let next = {
            let mut t = self.tabs.borrow_mut();
            t.close(i)
                .filter(|_| i == current)
                .and_then(|next| t.get(next).cloned())
        };
        if let Some(view) = next {
            self.show_tab(&view);
        }
        self.tabs_changed(shown);
    }

    /// Close Multiple Tabs: the tabs left or right of tab `i`, or all but
    /// it; tab `i` shows.
    pub(super) fn close_other_tabs(&self, i: usize, which: Others) {
        let closing = self.tabs.borrow().others(i, which);
        if closing.is_empty() {
            return;
        }
        let shown = self.tab_strip_shown();
        if closing.contains(&self.current_tab()) {
            self.select_tab(i);
        }
        {
            let mut t = self.tabs.borrow_mut();
            // Right to left: each index still holds as the others go.
            for j in closing {
                t.close(j);
            }
        }
        self.tabs_changed(shown);
    }

    /// Reopen Closed Tab (Ctrl+Shift+T), or History > Recently Closed Tabs'
    /// entry `newest` (0 the tab closed last): back where it was, shown.
    pub(super) fn reopen_tab(&self, newest: usize) {
        if newest >= self.tabs.borrow().closed_count() {
            return;
        }
        let shown = self.tab_strip_shown();
        self.leave_tab();
        self.store_current_tab();
        let view = {
            let mut t = self.tabs.borrow_mut();
            let Some(closed) = t.take_closed(newest) else {
                return;
            };
            let view = closed.tab.clone();
            let i = t.insert(closed.index, closed.tab);
            t.select(i);
            view
        };
        self.show_tab(&view);
        self.tabs_changed(shown);
    }

    /// History > Recently Closed Tabs > Reopen All Tabs: every closed tab
    /// back where it was; the one closed last shows.
    pub(super) fn reopen_all_tabs(&self) {
        if self.tabs.borrow().closed_count() == 0 {
            return;
        }
        let shown = self.tab_strip_shown();
        self.leave_tab();
        self.store_current_tab();
        let view = {
            let mut t = self.tabs.borrow_mut();
            let mut last: Option<usize> = None;
            while let Some(closed) = t.take_closed(0) {
                let i = t.insert(closed.index, closed.tab);
                last = Some(match last {
                    // Put in at or left of it: it moved right.
                    Some(l) if i <= l => l + 1,
                    Some(l) => l,
                    None => i,
                });
            }
            last.and_then(|l| {
                t.select(l);
                t.get(l).cloned()
            })
        };
        if let Some(view) = view {
            self.show_tab(&view);
        }
        self.tabs_changed(shown);
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab: the next or previous tab, going round
    /// (`ctrl_tab_action=1`). With one tab, or another action, they move
    /// the keyboard between the window's parts as Tab and Shift+Tab do.
    fn cycle_tab(&self, forward: bool) {
        if self.tab_count() < 2 || !self.tab_settings().ctrl_tab_cycles() {
            // SAFETY: reads this thread's focus.
            let focus = unsafe { GetFocus() };
            self.tab_key(focus, !forward);
            return;
        }
        let next = self.tabs.borrow().step(forward);
        self.select_tab(next);
    }

    /// A drag along the strip moved tab `from` to `to`.
    pub(super) fn move_tab(&self, from: usize, to: usize) {
        self.tabs.borrow_mut().move_tab(from, to);
        self.invalidate_strip(StripSlot::Tabs);
    }

    /// File > Close Window, and closing the last tab: the window closes as
    /// its close box closes it (into the background with
    /// `run_in_background`).
    pub(super) fn close_window(&self) {
        // SAFETY: posts to this thread's window; it closes once the
        // command that asked is done.
        unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
    }

    /// A tab command (`crate::tabs`'s ids); false for any other.
    pub(super) fn tab_command(&self, id: u16) -> bool {
        let current = self.current_tab();
        match id {
            tabs::NEW_TAB => self.new_tab(),
            tabs::CLOSE_TAB => self.close_tab(current),
            tabs::NEXT_TAB => self.cycle_tab(true),
            tabs::PREV_TAB => self.cycle_tab(false),
            tabs::REOPEN_TAB => self.reopen_tab(0),
            tabs::DUPLICATE_TAB => self.duplicate_tab(current),
            tabs::CLOSE_TABS_LEFT => self.close_other_tabs(current, Others::Left),
            tabs::CLOSE_TABS_RIGHT => self.close_other_tabs(current, Others::Right),
            tabs::CLOSE_OTHER_TABS => self.close_other_tabs(current, Others::All),
            tabs::REOPEN_ALL_TABS => self.reopen_all_tabs(),
            tabs::NEW_WINDOW => self.new_window(),
            tabs::CLOSE_WINDOW => self.close_window(),
            _ => {
                let Some(i) = id
                    .checked_sub(tabs::CLOSED_TAB_FIRST)
                    .map(usize::from)
                    .filter(|&i| i < tabs::CLOSED_TABS)
                else {
                    return false;
                };
                self.reopen_tab(i);
            }
        }
        true
    }

    /// A menu of the bar is about to open: File gets the tab and window
    /// commands at its top, as Everything's; History its Recently Closed
    /// Tabs.
    pub(super) fn tab_menu_popup(&self, popup: HMENU) {
        if popup == self.menus.file {
            menu::add_items(
                popup,
                None,
                &[
                    Item::Cmd(tabs::NEW_TAB, "New &Tab\tCtrl+T"),
                    Item::Cmd(tabs::NEW_WINDOW, "&New Window\tCtrl+N"),
                    Item::Cmd(tabs::CLOSE_WINDOW, "Close Win&dow\tCtrl+Shift+W"),
                    Item::Cmd(tabs::CLOSE_TAB, "&Close Tab\tCtrl+W"),
                    Item::Sep,
                ],
            );
        } else if popup == self.menus.history {
            self.add_closed_tabs(popup);
        }
    }

    /// History > Recently Closed Tabs: the closed tabs, newest first, then
    /// Reopen All Tabs; greyed while there are none. It goes with Restore
    /// Previous Session, or after the search history's items without it.
    fn add_closed_tabs(&self, history: HMENU) {
        let labels: Vec<String> = self
            .tabs
            .borrow()
            .closed_newest_first()
            .take(tabs::CLOSED_TABS)
            .map(|v| menu::label(&tabs::title(&v.search.text)))
            .collect();
        const NAME: &str = "Recently Closed Tabs";
        let item = if labels.is_empty() {
            Item::Greyed(NAME)
        } else {
            let mut items: Vec<Item<'_>> = labels
                .iter()
                .enumerate()
                .map(|(i, l)| Item::Cmd(tabs::CLOSED_TAB_FIRST + i as u16, l))
                .collect();
            items.push(Item::Sep);
            items.push(Item::Cmd(tabs::REOPEN_ALL_TABS, "Reopen &All Tabs"));
            Item::Sub(NAME, menu::submenu(&items))
        };
        let after = [
            menu::RESTORE_SESSION,
            menu::ORGANIZE_SEARCH_HISTORY,
            menu::NAV_HOME,
        ]
        .into_iter()
        .find(|&cmd| {
            // SAFETY: reads an item's state by command; all bits set when
            // the menu has no such item.
            let state = unsafe { GetMenuState(history, u32::from(cmd), MF_BYCOMMAND) };
            state != u32::MAX
        });
        match after {
            Some(menu::RESTORE_SESSION) => {
                menu::add_items(history, Some(menu::RESTORE_SESSION), &[item]);
            }
            Some(cmd) => menu::add_items(history, Some(cmd), &[Item::Sep, item]),
            None => menu::add_items(history, None, &[item]),
        }
    }

    /// The strip's menu for tab `target` (none: the empty strip, which acts
    /// for the shown tab), at `pt` (screen), as Everything's: New Tab,
    /// Duplicate Tab, Close Tab, Close Multiple Tabs, Reopen Closed Tab,
    /// then the bars and the panes.
    pub(super) fn tab_menu(&self, target: Option<usize>, pt: POINT) {
        let n = self.tab_count();
        let target = target.filter(|&i| i < n).unwrap_or(self.current_tab());
        let close = menu::submenu(&[
            Item::Cmd(tabs::CLOSE_TABS_LEFT, "Close Tabs to &Left"),
            Item::Cmd(tabs::CLOSE_TABS_RIGHT, "Close Tabs to R&ight"),
            Item::Cmd(tabs::CLOSE_OTHER_TABS, "Close &Other Tabs"),
        ]);
        let mut items = vec![
            Item::Cmd(tabs::NEW_TAB, "New &Tab"),
            Item::Sep,
            Item::Cmd(tabs::DUPLICATE_TAB, "&Duplicate Tab"),
            Item::Sep,
            Item::Cmd(tabs::CLOSE_TAB, "&Close Tab"),
            Item::Sub("Close &Multiple Tabs", close),
            Item::Cmd(tabs::REOPEN_TAB, "Re&open Closed Tab"),
            Item::Sep,
            Item::Cmd(menu::FILTER_BAR, "Filt&er Bar"),
            Item::Cmd(menu::STATUS_BAR, "Status &Bar"),
        ];
        // The panes this build has, as the View menu lists them (without
        // the shortcut, as Everything's tab menu).
        let mut panes: Vec<(u32, u16, &str)> = PaneId::KNOWN
            .into_iter()
            .filter(|&id| self.pane_registered(id))
            .filter_map(|id| {
                id.menu_item().map(|(cmd, label)| {
                    let label = label.split('\t').next().unwrap_or(label);
                    (id.menu_rank(), cmd, label)
                })
            })
            .collect();
        panes.sort_by_key(|&(rank, ..)| rank);
        if !panes.is_empty() {
            items.push(Item::Sep);
            items.extend(panes.iter().map(|&(_, cmd, label)| Item::Cmd(cmd, label)));
        }
        let m = menu::submenu(&items);
        let grey = |cmd: u16, on: bool| {
            // SAFETY: an item of the menu made above, by command.
            unsafe {
                EnableMenuItem(
                    m,
                    u32::from(cmd),
                    MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED },
                )
            };
        };
        grey(tabs::CLOSE_TABS_LEFT, target > 0);
        grey(tabs::CLOSE_TABS_RIGHT, target + 1 < n);
        grey(tabs::CLOSE_OTHER_TABS, n > 1);
        grey(tabs::REOPEN_TAB, self.tabs.borrow().closed_count() > 0);
        menu::check_item(m, menu::FILTER_BAR, self.filter_bar_visible());
        menu::check_item(m, menu::STATUS_BAR, self.status_bar_visible());
        for &(_, cmd, _) in &panes {
            if let Some(id) = PaneId::of_command(cmd) {
                menu::check_item(m, cmd, self.pane_visible(id));
            }
        }
        let cmd = popup_menu(self.hwnd, m, pt, "tab menu");
        match cmd {
            0 => {}
            tabs::DUPLICATE_TAB => self.duplicate_tab(target),
            tabs::CLOSE_TAB => self.close_tab(target),
            tabs::CLOSE_TABS_LEFT => self.close_other_tabs(target, Others::Left),
            tabs::CLOSE_TABS_RIGHT => self.close_other_tabs(target, Others::Right),
            tabs::CLOSE_OTHER_TABS => self.close_other_tabs(target, Others::All),
            other => self.command(other),
        }
    }

    /// The overflow button's list of every tab, the shown one checked, at
    /// `pt` (screen): the one picked shows.
    pub(super) fn tab_overflow_menu(&self, pt: POINT) {
        let titles = self.tab_titles();
        let current = self.current_tab();
        let labels: Vec<String> = titles.iter().map(|t| menu::label(t)).collect();
        let items: Vec<Item<'_>> = labels
            .iter()
            .enumerate()
            .map(|(i, l)| Item::Radio(overflow_id(i), l))
            .collect();
        let m = menu::submenu(&items);
        menu::check_item(m, overflow_id(current), true);
        let cmd = popup_menu(self.hwnd, m, pt, "tab list");
        if let Some(i) = cmd
            .checked_sub(OVERFLOW_FIRST)
            .map(usize::from)
            .filter(|&i| i < titles.len())
        {
            self.select_tab(i);
        }
    }

    // ---- The session ----

    /// The tabs as a session keeps them: every tab (the shown one as it is
    /// now), which one shows, and the closed ones, oldest first.
    pub(super) fn session_tabs(&self) -> (Vec<View>, usize, Vec<View>) {
        let live = self.current_view();
        let t = self.tabs.borrow();
        let mut open: Vec<View> = t.tabs().to_vec();
        if let Some(slot) = open.get_mut(t.current()) {
            *slot = live;
        }
        (open, t.current(), t.closed().cloned().collect())
    }

    /// A window started as the previous session left it
    /// (`session_store_restore_on_first_new_window`): its tabs, and its shown
    /// tab's navigation history, place and selection (the search, view and
    /// columns are the window's already).
    pub(super) fn start_tabs(&self, s: &Session) {
        let max = self.tab_settings().max_closed;
        if let Some(list) = TabList::from_parts(s.tabs.clone(), s.current, s.closed.clone(), max) {
            *self.tabs.borrow_mut() = list;
        }
        self.begin_view(s.view());
        if self.tab_strip_shown() {
            self.relayout();
        }
    }

    /// History > Restore Previous Session: its tabs take the place of the
    /// window's, which go to the closed tabs (Reopen Closed Tab brings them
    /// back); the session's own closed tabs come before them.
    pub(super) fn restore_session_tabs(&self, s: &Session) {
        let shown = self.tab_strip_shown();
        self.leave_tab();
        self.store_current_tab();
        self.tabs
            .borrow_mut()
            .replace(s.tabs.clone(), s.current, s.closed.clone());
        self.show_tab(s.view());
        self.tabs_changed(shown);
    }
}

fn overflow_id(i: usize) -> u16 {
    OVERFLOW_FIRST.saturating_add(u16::try_from(i).unwrap_or(u16::MAX))
}

/// Shows `m` at `pt` (screen) for `owner` and destroys it; the command
/// picked, 0 for none. `what` names it in a test's menu dump
/// (`GOZ_GUI_MENU_DUMP`, `crate::popup`).
fn popup_menu(owner: HWND, m: HMENU, pt: POINT, what: &str) -> u16 {
    // The owner gets its WM_INITMENUPOPUP (the window checks its items as
    // for its own menus); nothing is borrowed while it is up.
    let cmd = crate::popup::track(m, owner, pt, what, &|_| None);
    // SAFETY: the menu made by the caller, no longer shown.
    unsafe { DestroyMenu(m) };
    u16::try_from(cmd).unwrap_or(0)
}
