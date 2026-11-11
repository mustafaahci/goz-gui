//! The session (`Session.json`, see `crate::session`): what the window
//! looks like and shows, saved so History > Restore Previous Session can
//! bring it back.
//!
//! - **Saved**: the window's place, theme, On Top and status bar; and each
//!   tab (`tabs.rs`): the search with its options and sort, the view and
//!   columns, the row at the top of the list, the selected and focused
//!   files by path (or that Select All had every row selected), Back and
//!   Forward's entries; and the tabs closed. The
//!   store thread writes it at most every `session_store_save_delay` ms
//!   (15 s) while it changes, and at exit. `session_store=0` turns all of
//!   it off (nothing read or written, no menu item).
//! - **Restored**: the session read at startup (the last one any goz-gui
//!   wrote) on History > Restore Previous Session (its tabs take the place
//!   of the window's, which can be reopened), or as the window starts with
//!   `session_store_restore_on_first_new_window=1`. The shown tab's search
//!   runs again; when its first rows come the list scrolls to the saved row
//!   and the saved files are selected again as their rows arrive (the
//!   refresh's `Reselect`), so a file now elsewhere in the results is still
//!   found; a tab whose rows Select All had all selected has every row
//!   selected again, the files that came since included, as a refresh
//!   keeps it. The other tabs do the same when they are shown.
//! - **Several goz-gui windows**: each writes its own session, so the file
//!   holds the window written last, which is the last one closed.

use std::cell::RefCell;
use std::sync::Arc;

use windows_sys::Win32::UI::Controls::{
    LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETTOPINDEX, LVM_SCROLL,
    LVNI_FOCUSED,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowPlacement, IsIconic, IsWindowVisible, IsZoomed, KillTimer,
    SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOZORDER, SetTimer, SetWindowPlacement, SetWindowPos, ShowWindow, WINDOWPLACEMENT,
    WPF_RESTORETOMAXIMIZED,
};

use super::nav::NavState;
use super::reselect::{self, Reselect};
use super::{App, TIMER_SESSION, prefs, send};
use crate::history::NavHistory;
use crate::placement;
use crate::searchbox;
use crate::session::{self, FILE_NAME, MAX_SELECTION, Session, SessionFile, View};
use crate::settings::{Settings, WindowPos};
use crate::store::location;

/// A restored view waiting for the rows of its search.
#[derive(Debug)]
pub(super) struct PendingView {
    /// The search that brings its rows (or a later run of it).
    epoch: u64,
    top: u64,
    focus: Option<Vec<u16>>,
    selection: Vec<Vec<u16>>,
    /// Every row selected (`View::select_all`).
    all: bool,
}

/// Where the list is and what it has selected, as a tab keeps it.
struct Place {
    top: u64,
    focus: Option<Vec<u16>>,
    selection: Vec<Vec<u16>>,
    all: bool,
}

/// The session's settings and file, and the session read at startup.
pub(super) struct Sessions {
    /// `session_store`, and a data folder to keep it in.
    file: Option<Arc<SessionFile>>,
    /// `session_store_save_delay`.
    delay_ms: u32,
    /// `session_store_restore_on_first_new_window`.
    restore_first: bool,
    /// The session saved last (by any goz-gui) as read at startup: what
    /// Restore Previous Session brings back.
    previous: Option<Session>,
    /// The text last handed to the store thread.
    last: RefCell<Option<String>>,
    /// A restore waiting for its search's rows.
    view: RefCell<Option<PendingView>>,
    /// What to tell the user once the window is up: the file could not be
    /// read.
    pub note: RefCell<Option<String>>,
}

impl Sessions {
    /// Reads the previous session (before the window exists: the file is
    /// small).
    pub(super) fn load(settings: &Settings) -> Sessions {
        let path = settings
            .session_store
            .then(|| location::data_file(FILE_NAME))
            .flatten();
        let (previous, note) = match &path {
            Some(path) => session::load(path),
            None => (None, None),
        };
        Sessions {
            file: path.map(|p| Arc::new(SessionFile::new(p, settings.csv_backup))),
            delay_ms: settings.session_save_delay,
            restore_first: settings.session_restore_first,
            previous,
            last: RefCell::new(None),
            view: RefCell::new(None),
            note: RefCell::new(note),
        }
    }

    /// The session a new window starts as, when it is to start as the
    /// previous one left it.
    pub(super) fn at_start(&self) -> Option<Session> {
        self.restore_first.then(|| self.previous.clone()).flatten()
    }

    /// This window starts as the settings say, whatever
    /// `session_store_restore_on_first_new_window` says: it is not the
    /// first window. (Restore Previous Session still has the session.)
    pub(super) fn not_first(&mut self) {
        self.restore_first = false;
    }
}

/// The settings a window starts with when it starts as `s` left it: its
/// shown tab's search, view and columns.
pub(super) fn start_settings(base: &Settings, s: &Session) -> Settings {
    let tab = s.view();
    let search = &tab.search;
    Settings {
        window: s.window.or(base.window),
        maximized: s.maximized,
        theme: s.theme,
        on_top: s.on_top,
        status_bar: s.status_bar,
        search: search.text.clone(),
        match_case: search.match_case,
        match_path: search.match_path,
        filter: search.filter,
        sort: search.sort,
        view: tab.display.unwrap_or(base.view),
        columns: tab.columns.unwrap_or(base.columns),
        ..base.clone()
    }
}

/// The scroll that brings row `top` to the top of a list showing row
/// `now` there, in pixels of `row` px rows; `total` rows in all.
fn scroll_by(top: u64, now: u64, total: u64, row: i32) -> i32 {
    let top = top.min(total.saturating_sub(1));
    let rows = i128::from(top) - i128::from(now);
    (rows * i128::from(row)).clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
}

impl App {
    /// Starts the clock that writes the session while it changes.
    pub(super) fn session_timer(&self) {
        if self.sessions.file.is_some() {
            // SAFETY: a timer of this window; WM_TIMER kills it, and the
            // handler sets it again.
            unsafe { SetTimer(self.hwnd, TIMER_SESSION, self.sessions.delay_ms, None) };
        }
    }

    /// Hands the session to the store thread: when it changed since the
    /// last time, or always at `exit` (the last window closed is the one
    /// the file keeps; the store thread skips the write when the file
    /// holds the same text).
    pub(super) fn save_session(&self, exit: bool) {
        let Some(file) = self.sessions.file.clone() else {
            return;
        };
        if exit {
            // SAFETY: a timer of this window.
            unsafe { KillTimer(self.hwnd, TIMER_SESSION) };
        }
        let text = self.current_session().to_text();
        {
            let mut last = self.sessions.last.borrow_mut();
            // Unchanged, and no failed write waiting to go out again.
            if !exit && last.as_deref() == Some(text.as_str()) && !file.pending() {
                return;
            }
            *last = Some(text.clone());
        }
        file.stage(text);
        let hwnd = self.hwnd as usize;
        let job = Arc::clone(&file);
        self.writer.submit(
            file.path(),
            Box::new(move || prefs::report_result(hwnd, job.write_now())),
        );
    }

    /// Windows asked whether the session may end: the session is written
    /// now, and its timer keeps running, since the end may yet be called
    /// off (`WM_ENDSESSION` with FALSE), after which goz-gui runs on.
    pub(super) fn save_session_now(&self) {
        self.save_session(true);
        self.session_timer();
    }

    /// The window as it is now.
    fn current_session(&self) -> Session {
        let (window, maximized) = self.placement_now();
        let (tabs, current, closed) = self.session_tabs();
        Session {
            window,
            maximized,
            on_top: self.on_top.get(),
            theme: self.theme_mode(),
            status_bar: self.status_bar_visible(),
            tabs,
            current,
            closed,
        }
    }

    /// The tab shown, as it is now: its search, view and columns, place
    /// and selection, and navigation history.
    pub(super) fn current_view(&self) -> View {
        // The columns as the user last dragged them.
        self.sync_columns();
        let (nav, nav_index) = {
            let nav = self.nav.borrow();
            (
                nav.entries().iter().map(NavState::to_search).collect(),
                nav.current_index().unwrap_or(0),
            )
        };
        let place = self.place_now();
        View {
            search: self.nav_state().to_search(),
            top: place.top,
            focus: place.focus,
            selection: place.selection,
            select_all: place.all,
            nav,
            nav_index,
            display: Some(self.view()),
            columns: Some(self.layout.get()),
            // W5: the file list the tab searches.
            file_list: self.filelist_path(),
        }
    }

    /// The row at the top of the list, and the focused and selected files
    /// by path: those held, and those a refresh or a restore is still to
    /// select again; and whether Select All has every row selected.
    fn place_now(&self) -> Place {
        // The selection as it is to be, rows still being set included.
        self.finish_selecting();
        if let Some(v) = self.sessions.view.borrow().as_ref() {
            // A restore whose rows have not come yet.
            return Place {
                top: v.top,
                focus: v.focus.clone(),
                selection: v.selection.clone(),
                all: v.all,
            };
        }
        if self
            .model
            .borrow()
            .pending
            .as_ref()
            .is_some_and(|p| !p.keep_place)
        {
            // A new search on its way: it shows from the top, with nothing
            // selected.
            return Place {
                top: 0,
                focus: None,
                selection: Vec::new(),
                all: false,
            };
        }
        let top = self.top_index() as u64;
        let focused = usize::try_from(send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .ok();
        let selected = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
        // Only Select All's: a selection that just covers every row (the
        // only row, clicked) is its files, as any other is.
        let all = reselect::all_chosen(self.select_all.get(), selected, self.model.borrow().total);
        // With every row selected, the rows held are noted too: Everything
        // reading the session selects those (and so does goz-gui when the
        // search finds no rows).
        let held = self.selected_held();
        let (mut focus, mut selection) = {
            let m = self.model.borrow();
            let path = |i: usize| m.peek(i).map(|row| row.path.to_vec());
            (
                focused.and_then(path),
                held.iter()
                    .filter_map(|&i| path(i))
                    .take(MAX_SELECTION)
                    .collect::<Vec<_>>(),
            )
        };
        if let Some(r) = self.reselect.borrow().as_ref() {
            let (paths, sought_focus) = r.sought();
            let room = MAX_SELECTION.saturating_sub(selection.len());
            selection.extend(paths.into_iter().take(room));
            focus = focus.or(sought_focus);
        }
        Place {
            top,
            focus,
            selection,
            all,
        }
    }

    /// Whether History > Restore Previous Session is there (`None`: the
    /// session store is off) and has a session to restore.
    pub(super) fn session_menu(&self) -> Option<bool> {
        self.sessions
            .file
            .as_ref()
            .map(|_| self.sessions.previous.is_some())
    }

    /// History > Restore Previous Session.
    pub(super) fn restore_previous_session(&self) {
        if self.bookmark_dialog.get()
            || self.search_history_dialog_open()
            || self.run_history_dialog_open()
        {
            return;
        }
        if let Some(s) = self.sessions.previous.clone() {
            self.restore_session(&s);
        }
    }

    /// Makes the window what `s` describes: its place and look, and its
    /// tabs (`tabs.rs`) with the shown one's search, scroll position,
    /// selection and navigation history.
    fn restore_session(&self, s: &Session) {
        self.restore_placement(s.window, s.maximized);
        if self.theme_mode() != s.theme {
            self.set_theme_mode(s.theme);
        }
        if self.on_top.get() != s.on_top {
            self.set_on_top(s.on_top);
        }
        if self.status_bar_visible() != s.status_bar {
            self.set_status_bar_visible(s.status_bar);
        }
        self.restore_session_tabs(s);
    }

    /// Shows `view` as the window's: its view and columns (a tab that keeps
    /// none, from Everything's session or an older one, the settings'),
    /// then its search as Back shows an entry (no entry is made), its
    /// navigation entries in place of the window's, and its place and
    /// selection once its rows come.
    pub(super) fn show_view(&self, view: &View) {
        let (base_columns, base_display) = {
            let p = self.prefs.borrow();
            (p.base.columns, p.base.view)
        };
        let columns = view.columns.unwrap_or(base_columns);
        self.sync_columns();
        if columns != self.layout.get() {
            self.apply_columns(columns);
            self.settings_changed();
        }
        let old = self.view();
        let display = view.display.unwrap_or(base_display);
        if display != old {
            self.view.set(display);
            // Details stays Details (only the size its button goes back
            // to changes); any thumbnails are put on the list afresh.
            if old.thumbnails || display.thumbnails {
                self.put_view();
            }
            self.settings_changed();
            self.invalidate_status();
        }
        // W5: the tab's file list (or the index) before its search runs.
        self.tab_file_list(view.file_list.as_deref());
        self.restore(&NavState::from_search(&view.search));
        self.begin_view(view);
    }

    /// The search just started (the one in `view`) is to show as `view`
    /// was: its navigation entries now, its place and selection when its
    /// rows come.
    pub(super) fn begin_view(&self, view: &View) {
        // Select All was of the rows on screen; the view's comes with its
        // rows (`apply_view`).
        self.select_all.set(false);
        let entries = view.nav.iter().map(NavState::from_search).collect();
        *self.nav.borrow_mut() = NavHistory::from_parts(entries, view.nav_index);
        *self.sessions.view.borrow_mut() = Some(PendingView {
            epoch: self.next_epoch.get(),
            top: view.top,
            focus: view.focus.clone(),
            selection: view.selection.clone(),
            all: view.select_all,
        });
        searchbox::invalidate_nav(self);
    }

    /// The restored view waiting for the rows of search `epoch`, if any:
    /// it is taken.
    pub(super) fn take_view(&self, epoch: u64) -> Option<PendingView> {
        let mut slot = self.sessions.view.borrow_mut();
        if slot.as_ref().is_some_and(|v| v.epoch <= epoch) {
            slot.take()
        } else {
            None
        }
    }

    /// The view waiting is not wanted any more: another search, or the
    /// user chose rows meanwhile.
    pub(super) fn drop_view(&self) {
        self.sessions.view.borrow_mut().take();
    }

    /// The first rows of a restored view's search came, `total` in all:
    /// the list scrolls to the saved row, and the saved files are selected
    /// again as their rows arrive (every row at once, when every row was).
    pub(super) fn apply_view(&self, view: PendingView, total: u64) {
        // A tab's rows are in: the list, frozen while they came (`tabs.rs`),
        // takes its new size before it scrolls, and paints once this is
        // done.
        self.thaw_list();
        self.quietly(|| self.clear_selection());
        if total > 0 && self.view().thumbnails {
            // Rows of cells, not of the rows' height: the saved item in
            // view.
            let top = view.top.min(total - 1) as usize;
            send(self.list, LVM_ENSUREVISIBLE, top, 0);
        } else if total > 0 {
            let now = send(self.list, LVM_GETTOPINDEX, 0, 0).max(0) as u64;
            let dy = scroll_by(view.top, now, total, self.row_height());
            if dy != 0 {
                send(self.list, LVM_SCROLL, 0, dy as isize);
            }
        }
        let mut reselect = Reselect::sought_again(view.selection, view.focus);
        // Select All had every row selected: every row is again, however
        // many there are now, and it stays in force (`reselect.rs`); the
        // focus is still sought. No rows: its files are sought by path.
        let all = view.all && total > 0;
        if all {
            self.quietly(|| self.select_every_row());
            reselect.every_row_selected();
        }
        self.select_all.set(all);
        if !reselect.is_done() {
            *self.reselect.borrow_mut() = Some(reselect);
        }
        let shown = self.visible_items();
        self.request_rows(shown.start, shown.end.max(shown.start + 1));
    }

    /// Puts the window where `pos` was (on the monitors there are now) and
    /// maximizes it or not, never activating it: a window in the
    /// background that is to be maximized stays as it is and is saved as
    /// maximized until the user changes its state, as at startup.
    pub(super) fn restore_placement(&self, pos: Option<WindowPos>, maximized: bool) {
        let monitors = placement::monitors();
        let rect = pos.and_then(|w| placement::fit(&w, &monitors));
        // SAFETY: reads and places this thread's window; the struct lives
        // across the calls.
        unsafe {
            let hwnd = self.hwnd;
            let visible = IsWindowVisible(hwnd) != 0;
            let active = GetForegroundWindow() == hwnd;
            let iconic = IsIconic(hwnd) != 0;
            if iconic || IsZoomed(hwnd) != 0 {
                // Minimized or maximized: the normal rectangle goes in the
                // placement (workspace coordinates), and the state stays
                // or leaves maximized without the window being activated.
                let mut wp = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..core::mem::zeroed()
                };
                GetWindowPlacement(hwnd, &mut wp);
                if let Some(r) = rect {
                    let ws = placement::screen_to_workspace(r, &monitors);
                    wp.rcNormalPosition = ws.to_win();
                }
                wp.showCmd = if !visible {
                    SW_HIDE
                } else if iconic {
                    SW_SHOWMINNOACTIVE
                } else if maximized {
                    // Stays maximized; the normal rectangle changes.
                    SW_SHOWNA
                } else {
                    SW_SHOWNOACTIVATE
                } as u32;
                if iconic {
                    if maximized {
                        wp.flags |= WPF_RESTORETOMAXIMIZED;
                    } else {
                        wp.flags &= !WPF_RESTORETOMAXIMIZED;
                    }
                }
                SetWindowPlacement(hwnd, &wp);
                self.maximize_deferred.set(maximized && !visible);
            } else {
                if let Some(r) = rect {
                    SetWindowPos(
                        hwnd,
                        core::ptr::null_mut(),
                        r.x,
                        r.y,
                        r.w,
                        r.h,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                if maximized && visible && active {
                    ShowWindow(hwnd, SW_SHOWMAXIMIZED);
                    self.maximize_deferred.set(false);
                } else {
                    self.maximize_deferred.set(maximized);
                }
            }
        }
        self.settings_changed();
    }
}

#[cfg(test)]
mod tests {
    use goz_core::types::{SortDir, SortKey, SortSpec};

    use super::*;
    use crate::filter::Filter;
    use crate::session::Search;
    use crate::settings::OnTop;
    use crate::theme::ThemeMode;

    #[test]
    fn a_window_started_from_a_session_takes_its_state() {
        let base = Settings {
            search: "home".into(),
            window: Some(WindowPos {
                x: 1,
                y: 2,
                width: 300,
                height: 400,
                dpi: None,
            }),
            ..Settings::default()
        };
        let s = Session {
            window: None,
            maximized: true,
            on_top: OnTop::Always,
            theme: ThemeMode::Dark,
            status_bar: false,
            tabs: vec![
                View {
                    search: Search {
                        text: "other tab".into(),
                        ..Search::default()
                    },
                    display: Some(crate::view::View::thumbnails(64)),
                    ..View::default()
                },
                View {
                    search: Search {
                        text: "*.dll".into(),
                        match_case: true,
                        match_path: true,
                        filter: Filter::Audio,
                        sort: SortSpec {
                            key: SortKey::Size,
                            dir: SortDir::Asc,
                        },
                        folder: None,
                    },
                    ..View::default()
                },
            ],
            current: 1,
            closed: Vec::new(),
        };
        let start = start_settings(&base, &s);
        assert_eq!(start.search, "*.dll");
        assert!(start.match_case && start.match_path && start.maximized);
        assert_eq!(start.filter, Filter::Audio);
        assert_eq!(start.sort.key, SortKey::Size);
        assert_eq!(
            (start.theme, start.on_top, start.status_bar),
            (ThemeMode::Dark, OnTop::Always, false)
        );
        // No rectangle in the session: the settings' one.
        assert_eq!(start.window, base.window);
        // What a session does not hold stays the settings' (the shown tab
        // keeps no view or columns here; the other tab's are not taken).
        assert_eq!(start.columns, base.columns);
        assert_eq!(start.view, base.view);
        assert_eq!(start.home, base.home);
        // The shown tab's view and columns when it keeps them.
        let mut columns = crate::columns::Layout::default();
        columns.hide(crate::columns::COL_SIZE);
        let mut kept = s.clone();
        kept.tabs[1].display = Some(crate::view::View::thumbnails(256));
        kept.tabs[1].columns = Some(columns);
        let start = start_settings(&base, &kept);
        assert_eq!(start.view, crate::view::View::thumbnails(256));
        assert_eq!(start.columns, columns);
        let placed = Session {
            window: Some(WindowPos {
                x: 50,
                y: 60,
                width: 700,
                height: 500,
                dpi: Some(96),
            }),
            ..s
        };
        assert_eq!(start_settings(&base, &placed).window, placed.window);
    }

    #[test]
    fn the_list_scrolls_by_whole_rows_to_the_saved_top() {
        assert_eq!(scroll_by(100, 0, 1000, 19), 1900);
        assert_eq!(scroll_by(10, 40, 1000, 19), -570);
        assert_eq!(scroll_by(5, 5, 1000, 19), 0);
        // Fewer rows now: as far as there are.
        assert_eq!(scroll_by(5000, 0, 1000, 19), 999 * 19);
        assert_eq!(scroll_by(5, 0, 0, 19), 0);
        // Far down a huge result: no overflow.
        assert_eq!(scroll_by(u64::MAX, 0, u64::MAX, 19), i32::MAX);
    }
}
