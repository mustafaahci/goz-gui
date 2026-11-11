//! Options' UI, Search, Results and View settings from the window's side
//! (`crate::ui_options` holds them, under Everything's keys): putting a
//! change in place, the search as it is typed (search as you type, its
//! delay), the search box's text selected by a click or when the window is
//! activated, one-click opening, closing on execute, and new windows.

use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVS_EX_FULLROWSELECT, LVS_EX_TRACKSELECT,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows_sys::Win32::UI::Shell::{SHELLFLAGSTATE, SHGetSettings, SSF_DOUBLECLICKINWEBVIEW};
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer, WM_CLOSE};

use super::{App, send, slots};
use crate::highlight::Highlight;
use crate::ui_options::UiOptions;
use crate::wide::window_text;

/// The search delay's timer (`search_delay`): the text typed is searched
/// when it goes off.
pub(super) const TIMER_SEARCH_DELAY: usize = slots::F6_TIMER;

/// Posted as the window is activated with "Focus search on activate": the
/// keyboard goes to the search box once the activation is done.
pub(super) const WM_FOCUS_SEARCH: u32 = slots::F6_MSG;

/// Explorer's "Single-click to open an item (point to select)" (Folder
/// Options), which Everything's "Use system settings" follows.
fn explorer_single_click() -> bool {
    // SAFETY: fills a plain struct with the flag asked for.
    let state = unsafe {
        let mut s: SHELLFLAGSTATE = core::mem::zeroed();
        SHGetSettings(&mut s, SSF_DOUBLECLICKINWEBVIEW);
        s
    };
    // fDoubleClickInWebView is the sixth bit of the flags (after
    // fShowAllObjects, fShowExtensions, fNoConfirmRecycle, fShowSysFiles
    // and fShowCompColor): set for double-click, Windows' default.
    state._bitfield & (1 << 5) == 0
}

impl App {
    /// The settings in force.
    pub fn ui(&self) -> UiOptions {
        self.ui.get()
    }

    /// Puts `new` in place, as Options' pages apply it, and keeps it for
    /// the settings file.
    pub(super) fn set_ui(&self, new: UiOptions) {
        let old = self.ui.replace(new);
        self.prefs.borrow_mut().base.ui = new;
        if old == new {
            return;
        }
        crate::format::set_size_format(new.size_format);
        if (old.full_row_select, old.click_open) != (new.full_row_select, new.click_open) {
            self.apply_list_style();
        }
        if old.highlight != new.highlight {
            self.rehighlight();
        }
        // The text typed and not searched yet is searched now that typing
        // searches again (or at once).
        if self.search_dirty.get()
            && ((!old.search_as_you_type && new.search_as_you_type)
                || (old.search_delay > 0 && new.search_delay == 0 && new.search_as_you_type))
        {
            self.start_search(false);
        }
        // What the rows, the header and the status bar show.
        // SAFETY: repaints this thread's windows.
        unsafe {
            InvalidateRect(self.list, core::ptr::null(), 0);
        }
        self.invalidate_status();
        self.settings_changed();
    }

    /// The list's extended styles the settings ask for: full row select,
    /// and hover selecting a row while one click opens (as Explorer does in
    /// its single-click mode).
    pub(super) fn apply_list_style(&self) {
        let ui = self.ui.get();
        let single = ui.click_open.single(explorer_single_click());
        self.single_click.set(single);
        let mut ex = 0;
        if ui.full_row_select {
            ex |= LVS_EX_FULLROWSELECT;
        }
        if single {
            ex |= LVS_EX_TRACKSELECT;
        }
        let mask = LVS_EX_FULLROWSELECT | LVS_EX_TRACKSELECT;
        send(
            self.list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            mask as usize,
            ex as isize,
        );
    }

    /// Whether one click on a result opens it (Options > Results).
    pub(super) fn single_click_open(&self) -> bool {
        self.single_click.get()
    }

    /// The highlight of the search on screen (and of one on its way) as
    /// `show_highlighted_search_terms` says now. Text typed and not
    /// searched yet is not what the rows matched: its search, when it
    /// comes, takes the setting.
    fn rehighlight(&self) {
        if self.search_dirty.get() {
            return;
        }
        let text = window_text(self.edit);
        let o = self.opts.get();
        let mut m = self.model.borrow_mut();
        if let Some(p) = m.pending.as_mut() {
            p.highlight = self.highlight_for(&text, o.match_case, o.match_path);
        }
        m.highlight = self.highlight_for(&text, o.match_case, o.match_path);
    }

    /// [`Self::rehighlight`], and the rows drawn again with it (gozd's
    /// syntax changed what the search's terms are).
    pub(super) fn rehighlight_now(&self) {
        self.rehighlight();
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
    }

    /// What a search for `text` highlights: nothing while highlighted
    /// search terms are off.
    pub(super) fn highlight_for(&self, text: &str, case: bool, path: bool) -> Highlight {
        if self.ui.get().highlight {
            Highlight::with_syntax(text, case, path, crate::client::boolean_syntax())
        } else {
            Highlight::new("", case, path)
        }
    }

    /// Whether row `index` is drawn as under the mouse.
    pub fn row_is_hot(&self, index: usize) -> bool {
        self.ui.get().mouseover && self.hot_row.get() == Some(index)
    }

    /// The search box's text changed: whether it is searched now. Typed by
    /// the user, it waits for Enter while search as you type is off, and
    /// for the search delay while one is set; set by goz-gui (a history
    /// entry, Back, a bookmark) it is searched at once.
    pub(super) fn search_now_or_later(&self, typed: bool) -> bool {
        let ui = self.ui.get();
        if !typed || (ui.search_as_you_type && ui.search_delay == 0) {
            return true;
        }
        self.search_dirty.set(true);
        // SAFETY: timers of this window; setting one again restarts it.
        unsafe {
            if ui.search_as_you_type {
                SetTimer(self.hwnd, TIMER_SEARCH_DELAY, ui.search_delay, None);
            } else {
                KillTimer(self.hwnd, TIMER_SEARCH_DELAY);
            }
        }
        false
    }

    /// A search starts: the text typed is searched now.
    pub(super) fn search_started(&self) {
        if self.search_dirty.replace(false) {
            // SAFETY: a timer of this window.
            unsafe { KillTimer(self.hwnd, TIMER_SEARCH_DELAY) };
        }
    }

    /// Whether the search box holds text not searched yet: a refresh would
    /// search it as if it were the search on screen.
    pub(super) fn search_is_dirty(&self) -> bool {
        self.search_dirty.get()
    }

    /// WM_FOCUS_SEARCH: the window is active (still): the keyboard goes to
    /// the search box with its text selected, as Alt+D puts it there.
    pub(super) fn focus_search_activated(&self) {
        // SAFETY: reads this thread's active window.
        if unsafe { GetActiveWindow() } == self.hwnd {
            self.command(crate::menu::FOCUS_SEARCH);
        }
    }

    /// The search delay ran out: the text typed is searched.
    pub(super) fn search_delay_done(&self) {
        if self.search_dirty.get() {
            self.start_search(false);
        }
    }

    /// An open of results was handed to the shell: with close on execute,
    /// the window closes as its close box would (into the background when
    /// goz-gui runs there).
    pub(super) fn executed(&self) {
        if self.ui.get().close_on_execute {
            // SAFETY: posts to this thread's window.
            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
        }
    }
}
