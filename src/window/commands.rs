//! What the menus and shortcuts do: `App::command`, and the state the menus
//! show as they open. The navigation history is `nav.rs`; the result
//! actions are `results.rs`; the bookmarks are `bookmarks.rs`.

use goz_core::types::{SortDir, SortKey, SortSpec};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::Controls::{
    EM_GETSEL, EM_SETSEL, LVIS_FOCUSED, LVIS_SELECTED, LVM_ENSUREVISIBLE, LVM_GETEDITCONTROL,
    LVM_GETITEMCOUNT, LVM_GETITEMSTATE, LVM_GETNEXTITEM, LVM_SETSELECTIONMARK, LVNI_FOCUSED,
    LVNI_SELECTED,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowTextLengthW, HMENU, HWND_NOTOPMOST,
    HWND_TOPMOST, MB_OK, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, SetWindowTextW,
    WM_COPY, WM_CUT, WM_PASTE, WS_EX_TOPMOST,
};

use super::{App, WM_SERVICE, key_down, send};
use crate::actions::{self, Service};
use crate::client::Job;
use crate::clipboard::{self, Effect};
use crate::copytext::Field;
use crate::filter::Filter;
use crate::settings::OnTop;
use crate::theme::ThemeMode;
use crate::wide::wz;
use crate::{menu, syntax};

fn sort_key(id: u16) -> SortKey {
    match id {
        menu::SORT_NAME => SortKey::Name,
        menu::SORT_PATH => SortKey::Path,
        menu::SORT_SIZE => SortKey::Size,
        _ => SortKey::DateModified,
    }
}

fn sort_id(key: SortKey) -> u16 {
    match key {
        SortKey::Name => menu::SORT_NAME,
        SortKey::Path => menu::SORT_PATH,
        SortKey::Size => menu::SORT_SIZE,
        SortKey::DateModified => menu::SORT_DATE,
    }
}

/// Commands that act on the selected results, which makes the search the
/// one wanted: it goes into the search history.
const ACTIONS: &[u16] = &[
    menu::OPEN,
    menu::REVEAL,
    menu::RUN_AS,
    menu::COPY_FULL_PATH,
    menu::COPY_NAME,
    menu::COPY_FOLDER,
    menu::COPY_SIZE,
    menu::COPY_DATE,
    menu::COPY_EXT,
    menu::COPY_TYPE,
    menu::COPY_TSV,
    menu::COPY,
    menu::CUT,
    menu::PROPERTIES,
    menu::SET_RUN_COUNT,
    menu::DELETE,
    menu::DELETE_PERMANENT,
    menu::RENAME,
    menu::PASTE,
];

/// `id`'s place in the `len` ids from `first`.
fn index_in(id: u16, first: u16, len: usize) -> Option<usize> {
    let i = usize::from(id.checked_sub(first)?);
    (i < len).then_some(i)
}

/// The last selected row before row `before`. It walks back from there and
/// forward from the top in step, so it costs the smaller of the distance to
/// that row and the number of selected rows above it; the list's own
/// backward search (LVNI_PREVIOUS) finds nothing in an owner-data list.
fn previous_selected(
    before: usize,
    is_selected: impl Fn(usize) -> bool,
    next_selected: impl Fn(Option<usize>) -> Option<usize>,
) -> Option<usize> {
    let mut back = before;
    let mut forward = None;
    loop {
        if back == 0 {
            return None;
        }
        back -= 1;
        if is_selected(back) {
            return Some(back);
        }
        match next_selected(forward) {
            Some(i) if i < before => forward = Some(i),
            _ => return forward,
        }
    }
}

/// The message that does Edit > Cut, Copy or Paste (`id`) in a text box
/// (the search box, Rename's); `None` for every other command.
fn edit_message(id: u16) -> Option<u32> {
    match id {
        menu::CUT => Some(WM_CUT),
        menu::COPY => Some(WM_COPY),
        menu::PASTE => Some(WM_PASTE),
        _ => None,
    }
}

impl App {
    /// The text box that has the keyboard, if one does: the search box, or
    /// the box of a rename in progress (the list's own edit control).
    fn text_box(&self) -> Option<HWND> {
        // SAFETY: reads this thread's focus.
        let focus = unsafe { GetFocus() };
        let rename = send(self.list, LVM_GETEDITCONTROL, 0, 0) as HWND;
        (!focus.is_null() && (focus == self.edit || focus == rename)).then_some(focus)
    }

    /// A command from the menu bar or the accelerator table. Edit > Cut,
    /// Copy and Paste act on files from the list only: while the keyboard
    /// is in a text box (the search box, or Rename's) they cut, copy or
    /// paste its text, as the shortcuts they show do there (a Cut meant for
    /// the text must never become a move of the selected files at the next
    /// paste, nor a Paste meant for a name a copy of files). Select All is
    /// the results', as in Everything. The row menu and the list's own keys
    /// call `command` directly: theirs are the results' commands.
    pub(super) fn menu_command(&self, id: u16) {
        if let Some(text) = self.text_box()
            && let Some(msg) = edit_message(id)
        {
            send(text, msg, 0, 0);
            return;
        }
        self.command(id);
    }

    /// Runs a menu command or shortcut.
    pub fn command(&self, id: u16) {
        if ACTIONS.contains(&id) && !self.selected(1).is_empty() {
            self.record_search();
        }
        let mut o = self.opts.get();
        match id {
            menu::OPEN => self.open_selected(false),
            menu::REVEAL => self.open_selected(true),
            menu::RUN_AS => self.run_as_selected(),
            menu::COPY_FULL_PATH => self.copy(Field::FullPath),
            menu::COPY_NAME => self.copy(Field::Name),
            menu::COPY_FOLDER => self.copy(Field::Path),
            menu::COPY_SIZE => self.copy(Field::Size),
            menu::COPY_DATE => self.copy(Field::DateModified),
            menu::COPY_EXT => self.copy(Field::Extension),
            menu::COPY_TYPE => self.copy(Field::Type),
            menu::COPY_TSV => self.copy(Field::Tsv(self.layout.get())),
            menu::COPY => self.copy_files(Effect::Copy),
            menu::CUT => self.copy_files(Effect::Move),
            menu::PROPERTIES => self.properties_selected(),
            menu::SET_RUN_COUNT => self.set_run_count(),
            // Shift held as File > Delete is chosen: for good, as in
            // Explorer.
            menu::DELETE => self.delete_selected(key_down(VK_SHIFT)),
            menu::DELETE_PERMANENT => self.delete_selected(true),
            menu::RENAME => self.rename_focused(),
            menu::PASTE => self.paste(),
            // SAFETY: destroys this thread's window.
            menu::EXIT => unsafe {
                DestroyWindow(self.hwnd);
            },
            menu::SELECT_ALL => {
                if send(self.list, LVM_GETITEMCOUNT, 0, 0) > 0 {
                    self.set_state(usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
                }
            }
            menu::FOCUS_SEARCH => {
                // SAFETY: focuses a child of this thread's window.
                unsafe { SetFocus(self.edit) };
                send(self.edit, EM_SETSEL, 0, -1);
            }
            // The clear button.
            menu::CLEAR_SEARCH => {
                // SAFETY: clears the search field; EN_CHANGE searches again.
                unsafe { SetWindowTextW(self.edit, wz("").as_ptr()) };
                self.focus_box();
            }
            menu::NEXT_SELECTED => self.jump_selected(true),
            menu::PREV_SELECTED => self.jump_selected(false),
            menu::REFRESH => self.start_search(true),
            menu::SIZE_ALL_COLUMNS_TO_FIT => self.size_all_columns_to_fit(),
            menu::SORT_NAME | menu::SORT_PATH | menu::SORT_SIZE | menu::SORT_DATE => {
                self.sort_by_key(sort_key(id));
            }
            menu::SORT_ASC | menu::SORT_DESC => {
                o.sort.dir = if id == menu::SORT_ASC {
                    SortDir::Asc
                } else {
                    SortDir::Desc
                };
                self.set_sort(o.sort);
            }
            menu::STATUS_BAR => self.set_status_bar_visible(!self.status_bar_visible()),
            menu::FILTER_BAR => self.set_filter_bar_visible(!self.filter_bar_visible()),
            menu::ZOOM_IN => self.zoom_step(Some(true)),
            menu::ZOOM_OUT => self.zoom_step(Some(false)),
            menu::ZOOM_RESET => self.zoom_step(None),
            menu::NEXT_FILTER => self.set_filter(o.filter.cycle(true)),
            menu::PREV_FILTER => self.set_filter(o.filter.cycle(false)),
            menu::EXPORT => self.export(),
            menu::THEME_USER_DEFAULT => self.set_theme_mode(ThemeMode::UserDefault),
            menu::THEME_STANDARD => self.set_theme_mode(ThemeMode::Standard),
            menu::THEME_DARK => self.set_theme_mode(ThemeMode::Dark),
            menu::ON_TOP_NEVER => self.set_on_top(OnTop::Never),
            menu::ON_TOP_ALWAYS => self.set_on_top(OnTop::Always),
            menu::ON_TOP_SEARCHING => self.set_on_top(OnTop::WhileSearching),
            menu::MATCH_CASE | menu::MATCH_PATH => {
                if id == menu::MATCH_CASE {
                    o.match_case = !o.match_case;
                } else {
                    o.match_path = !o.match_path;
                }
                self.opts.set(o);
                self.start_search(false);
                self.invalidate_status();
            }
            menu::INDEX_STATUS => {
                self.want_status.set(true);
                self.worker.push(Job::Status);
            }
            menu::NAV_BACK => self.go_back(),
            menu::NAV_FORWARD => self.go_forward(),
            menu::NAV_HOME => self.go_home(),
            menu::FIND_SEARCH_HISTORY => self.find_search_history(),
            menu::ORGANIZE_SEARCH_HISTORY => self.organize_search_history(),
            menu::RESTORE_SESSION => self.restore_previous_session(),
            menu::BOOKMARK_ADD => self.add_bookmark(),
            menu::BOOKMARK_FIND => self.find_bookmark(),
            menu::BOOKMARK_ORGANIZE => self.organize_bookmarks(),
            menu::SERVICE_START => actions::service(self.hwnd, Service::Start, WM_SERVICE),
            menu::SERVICE_STOP => actions::service(self.hwnd, Service::Stop, WM_SERVICE),
            menu::SERVICE_RESTART => actions::service(self.hwnd, Service::Restart, WM_SERVICE),
            menu::RUN_HISTORY => self.run_history_dialog(),
            menu::OPTIONS => self.options_dialog(),
            menu::HELP => actions::open_url(self.hwnd, actions::HELP_URL),
            menu::SEARCH_SYNTAX => syntax::show(self.hwnd, self.pal.get().dark),
            menu::WEBSITE => actions::open_url(self.hwnd, actions::WEBSITE_URL),
            menu::ABOUT => self.message_box(
                concat!(
                    "goz-gui ",
                    env!("CARGO_PKG_VERSION"),
                    "\n\nA native Windows front end for goz, the instant NTFS file search.\n",
                    "https://github.com/mustafaahci/goz"
                ),
                "About goz-gui",
            ),
            _ => {
                if let Some(filter) = Filter::from_menu_id(id) {
                    self.set_filter(filter);
                } else if let Some(i) = index_in(id, menu::NAV_ENTRY_FIRST, menu::NAV_ENTRIES) {
                    self.go_to_history(i);
                } else if let Some(i) = index_in(id, menu::BOOKMARK_FIRST, menu::BOOKMARKS) {
                    self.open_bookmark(i);
                }
            }
        }
    }

    /// Readies a menu about to open: rebuilds the parts that depend on the
    /// moment (the File menu's selection items, the history entries, the
    /// bookmarks), then sets its checks and greyed items.
    pub(super) fn init_menu_popup(&self, popup: HMENU) {
        let selection = !self.selected(1).is_empty();
        // Only the menu bar's Edit menu acts on a text box's text while it
        // has the keyboard (`menu_command`); the row menu's Cut and Copy are
        // the results' commands wherever the keyboard is, so they follow the
        // selection, as its other items do.
        let text = if popup == self.menus.edit {
            self.text_box()
        } else {
            None
        };
        let can_paste = match text {
            Some(_) => clipboard::has_text(),
            None => self.can_paste(),
        };
        let can_copy = match text {
            Some(text) => {
                let (mut start, mut end) = (0u32, 0u32);
                send(
                    text,
                    EM_GETSEL,
                    (&raw mut start) as usize,
                    (&raw mut end) as isize,
                );
                start != end
            }
            None => selection,
        };
        if popup == self.menus.file {
            let layout = self.layout.get();
            let shown = core::array::from_fn(|id| layout.is_shown(id));
            menu::fill_file(popup, selection, &shown);
        } else if popup == self.menus.history {
            // The search on screen is listed, and stays where it is while
            // the menu is open.
            self.commit_nav();
            let (entries, current) = self.history_entries();
            menu::fill_history(
                popup,
                self.search_history_enabled(),
                self.session_menu(),
                &entries,
                current,
            );
        } else if popup == self.menus.bookmarks {
            self.fill_bookmarks_menu(popup);
        }
        menu::update(popup, &self.menu_state(can_copy, can_paste));
    }

    fn menu_state(&self, can_copy: bool, can_paste: bool) -> menu::State {
        let o = self.opts.get();
        menu::State {
            can_copy,
            can_paste,
            match_case: o.match_case,
            match_path: o.match_path,
            filter: o.filter,
            sort: sort_id(o.sort.key),
            ascending: o.sort.dir == SortDir::Asc,
            status_bar: self.status_bar_visible(),
            filter_bar: self.filter_bar_visible(),
            theme: match self.theme_mode() {
                ThemeMode::UserDefault => menu::THEME_USER_DEFAULT,
                ThemeMode::Standard => menu::THEME_STANDARD,
                ThemeMode::Dark => menu::THEME_DARK,
            },
            on_top: self.on_top.get().menu_id(),
            can_go_back: self.can_go_back(),
            can_go_forward: self.can_go_forward(),
        }
    }

    pub(super) fn set_on_top(&self, mode: OnTop) {
        self.on_top.set(mode);
        self.apply_on_top();
        self.settings_changed();
    }

    /// Keeps the window above others or not, as View > On Top says; the
    /// search calls it on every change of the search text.
    pub(super) fn apply_on_top(&self) {
        // SAFETY: reads this thread's windows.
        let (searching, topmost) = unsafe {
            (
                GetWindowTextLengthW(self.edit) > 0,
                GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0,
            )
        };
        let want = self.on_top.get().topmost(searching);
        if want != topmost {
            // SAFETY: changes only the z-order of this thread's window.
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    if want { HWND_TOPMOST } else { HWND_NOTOPMOST },
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// Sorts by `key`, or reverses the order when the results are sorted by
    /// it already, as a click on its column header does.
    pub(super) fn sort_by_key(&self, key: SortKey) {
        let cur = self.opts.get().sort;
        let sort = if cur.key == key {
            SortSpec {
                key,
                dir: match cur.dir {
                    SortDir::Asc => SortDir::Desc,
                    SortDir::Desc => SortDir::Asc,
                },
            }
        } else {
            SortSpec::default_for(key)
        };
        self.set_sort(sort);
    }

    /// F4 / Shift+F4: moves the focus to the next or previous selected row,
    /// keeping the selection; past the last one it wraps round, with a beep.
    fn jump_selected(&self, forward: bool) {
        let list = self.list;
        let count = send(list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        let focus = usize::try_from(send(
            list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .ok();
        // GETNEXTITEM looks after the row given; -1 (usize::MAX) is the top.
        let next_selected = |after: Option<usize>| {
            let at = after.unwrap_or(usize::MAX);
            usize::try_from(send(list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize)).ok()
        };
        let is_selected = |i: usize| {
            send(list, LVM_GETITEMSTATE, i, LVIS_SELECTED as isize) as u32 & LVIS_SELECTED != 0
        };
        let found = if forward {
            next_selected(focus)
        } else {
            previous_selected(focus.unwrap_or(count), is_selected, next_selected)
        };
        let target = match found {
            Some(row) => row,
            None => {
                let wrapped = if forward {
                    next_selected(None)
                } else {
                    previous_selected(count, is_selected, next_selected)
                };
                // Nothing selected: nowhere to go.
                let Some(row) = wrapped else { return };
                if focus.is_some() {
                    // SAFETY: plays a system sound.
                    unsafe { MessageBeep(MB_OK) };
                }
                row
            }
        };
        self.set_state(target, LVIS_FOCUSED, LVIS_FOCUSED);
        send(list, LVM_SETSELECTIONMARK, 0, target as isize);
        send(list, LVM_ENSUREVISIBLE, target, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prev(selected: &[usize], before: usize) -> Option<usize> {
        previous_selected(
            before,
            |i| selected.contains(&i),
            |after| {
                selected
                    .iter()
                    .copied()
                    .find(|&i| after.is_none_or(|a| i > a))
            },
        )
    }

    #[test]
    fn previous_selected_row() {
        let sel = [2, 5, 9];
        assert_eq!(prev(&sel, 9), Some(5));
        assert_eq!(prev(&sel, 7), Some(5));
        assert_eq!(prev(&sel, 5), Some(2));
        assert_eq!(prev(&sel, 3), Some(2));
        assert_eq!(prev(&sel, 2), None);
        assert_eq!(prev(&sel, 0), None);
        // From past the end: the last one.
        assert_eq!(prev(&sel, 100), Some(9));
        assert_eq!(prev(&[], 100), None);
        // Far apart, either walk finds it.
        assert_eq!(prev(&[0, 1, 2, 3, 900], 1000), Some(900));
        assert_eq!(prev(&[3], 1_000_000), Some(3));
    }

    #[test]
    fn edit_menu_in_the_search_box() {
        assert_eq!(edit_message(menu::CUT), Some(WM_CUT));
        assert_eq!(edit_message(menu::COPY), Some(WM_COPY));
        assert_eq!(edit_message(menu::PASTE), Some(WM_PASTE));
        // The results' own commands stay theirs.
        for id in [
            menu::SELECT_ALL,
            menu::COPY_FULL_PATH,
            menu::COPY_NAME,
            menu::OPEN,
            menu::PROPERTIES,
            menu::DELETE,
            menu::RENAME,
        ] {
            assert_eq!(edit_message(id), None);
        }
    }

    #[test]
    fn on_top_modes() {
        assert!(!OnTop::Never.topmost(true));
        assert!(OnTop::Always.topmost(false));
        assert!(OnTop::WhileSearching.topmost(true));
        assert!(!OnTop::WhileSearching.topmost(false));
    }

    #[test]
    fn entry_ids() {
        assert_eq!(
            index_in(menu::NAV_ENTRY_FIRST, menu::NAV_ENTRY_FIRST, 20),
            Some(0)
        );
        assert_eq!(
            index_in(menu::NAV_ENTRY_FIRST + 19, menu::NAV_ENTRY_FIRST, 20),
            Some(19)
        );
        assert_eq!(
            index_in(menu::NAV_ENTRY_FIRST + 20, menu::NAV_ENTRY_FIRST, 20),
            None
        );
        assert_eq!(
            index_in(menu::NAV_ENTRY_FIRST - 1, menu::NAV_ENTRY_FIRST, 20),
            None
        );
        for id in [
            menu::SORT_NAME,
            menu::SORT_PATH,
            menu::SORT_SIZE,
            menu::SORT_DATE,
        ] {
            assert_eq!(sort_id(sort_key(id)), id);
        }
    }
}
