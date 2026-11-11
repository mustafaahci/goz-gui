//! Where the wave-1 features (F1 to F6, see FRAMEWORK.md in the wave's
//! notes) hook into the window: what they set up as it starts (panes,
//! strips, menu items) and let go of as it ends, their menus about to
//! open, their commands, their WM_APP messages and their main-window
//! timers, each by a range of its own. Each feature fills only its own slot
//! below, so their changes merge without touching each other's lines. Ids
//! outside every range never get here.

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{HMENU, WM_APP};

use super::App;

/// The WM_APP messages each feature may post to the main window: 20 each,
/// from `WM_APP + 120` (the framework's own are `WM_APP + 100..120`).
pub const F1_MSG: u32 = WM_APP + 120;
pub const F2_MSG: u32 = WM_APP + 140;
pub const F3_MSG: u32 = WM_APP + 160;
pub const F4_MSG: u32 = WM_APP + 180;
pub const F5_MSG: u32 = WM_APP + 200;
pub const F6_MSG: u32 = WM_APP + 220;
pub const MSG_RANGE: u32 = 20;

/// The main window's timer ids each feature may use: 10 each, from 110
/// (the framework's own are 100..110). A pane's or strip's own window has
/// timers of its own and needs none of these.
pub const F1_TIMER: usize = 110;
pub const F2_TIMER: usize = 120;
pub const F3_TIMER: usize = 130;
pub const F4_TIMER: usize = 140;
pub const F5_TIMER: usize = 150;
pub const F6_TIMER: usize = 160;
pub const TIMER_RANGE: usize = 10;

impl App {
    /// The window starts (its controls are made and themed, it is not
    /// shown yet): the features register their panes (`register_pane`),
    /// put in their strips (`set_strip`) and add their menu items
    /// (`menu::add_items`).
    pub(super) fn start_features(&self) {
        // F1 (preview pane).
        let preview = self.prefs.borrow().base.preview;
        crate::preview::register(self, preview);

        // F2 (sidebars).
        self.start_sidebars();

        // F3 (tabs and windows).
        self.set_strip(
            crate::strip::StripSlot::Tabs,
            Some(std::rc::Rc::new(super::tabstrip::TabStrip::new())),
        );

        // F4 (search tools).
        self.tools_start();

        // F5 (file tools).
        self.start_file_tools();

        // F6 (integration and the Options pages).

        // W4 (G1: data and file tools).
        self.data_tools_start();
    }

    /// The window is ending (its settings are saved, its panes still
    /// open): the features stop their threads and let go of what they
    /// hold.
    pub(super) fn exit_features(&self) {
        // F1 (preview pane).
        crate::preview::exit(self);

        // F2 (sidebars).
        self.exit_sidebars();

        // F3 (tabs and windows).

        // F4 (search tools).
        self.tools_exit();

        // F5 (file tools).

        // F6 (integration and the Options pages).

        // W4 (G1: data and file tools).
        self.data_tools_exit();
    }

    /// A menu of the bar (or a submenu) is about to open, after the
    /// window readied its own items: the features check, grey or rebuild
    /// theirs.
    pub(super) fn feature_menu_popup(&self, popup: HMENU) {
        // F1 (preview pane).

        // F2 (sidebars).

        // F3 (tabs and windows).
        self.tab_menu_popup(popup);

        // F4 (search tools).
        self.tools_menu_popup(popup);

        // F5 (file tools).
        self.file_tools_menu_popup(popup);

        // F6 (integration and the Options pages).

        // W4 (G1: data and file tools).
        self.data_tools_menu_popup(popup);
    }

    /// A command in a feature's range (`menu::F1_FIRST`, ...): true when
    /// the feature ran it.
    pub(super) fn feature_command(&self, id: u16) -> bool {
        match id {
            // F1: 3000..=3099.
            3000..=3099 => self.preview_command(id),

            // F2: 3100..=3199.
            3100..=3199 => self.sidebar_command(id),

            // F3: 3200..=3299, and its tab entries 4000..=4255.
            3200..=3299 | 4000..=4255 => self.tab_command(id),

            // F4: 3300..=3399, and its custom filters 3800..=3999.
            // (The custom filters' items run through `Filter::from_menu_id`
            // before they get here.)
            3300..=3399 | 3800..=3999 => self.tools_command(id),

            // F5: 3400..=3499.
            3400..=3499 => self.file_tools_command(id),

            // F6: 3500..=3599.
            3500..=3599 => false,

            // W4 (G1): 4600..=4699.
            4600..=4699 => self.data_tools_command(id),

            // W5 (C, the File List Editor): 4750..=4799.
            4750..=4799 => self.list_editor_command(id),

            // W6 (C, file-list slots): 4850..=4899.
            4850..=4899 => self.lists_command(id),

            _ => false,
        }
    }

    /// A feature's WM_APP message (its range above): the answer, or `None`
    /// for DefWindowProc.
    pub(super) fn feature_message(&self, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            // F1.
            m if (F1_MSG..F1_MSG + MSG_RANGE).contains(&m) => self.preview_message(m, w, l),

            // F2.
            m if (F2_MSG..F2_MSG + MSG_RANGE).contains(&m) => None,

            // F3.
            m if (F3_MSG..F3_MSG + MSG_RANGE).contains(&m) => self.window_message(m, w, l),

            // F4.
            m if (F4_MSG..F4_MSG + MSG_RANGE).contains(&m) => self.tools_message(m, w, l),

            // F5.
            m if (F5_MSG..F5_MSG + MSG_RANGE).contains(&m) => None,

            // F6: the keyboard to the search box after an activation.
            super::uiopts::WM_FOCUS_SEARCH => {
                self.focus_search_activated();
                Some(0)
            }
            m if (F6_MSG..F6_MSG + MSG_RANGE).contains(&m) => None,

            _ => None,
        }
    }

    /// A feature's main-window timer (its range above) went off; the timer
    /// is already killed, as the window's own are. True when one took it.
    pub(super) fn feature_timer(&self, id: usize) -> bool {
        match id {
            // F1.
            t if (F1_TIMER..F1_TIMER + TIMER_RANGE).contains(&t) => false,

            // F2.
            t if (F2_TIMER..F2_TIMER + TIMER_RANGE).contains(&t) => false,

            // F3.
            super::tabs::TIMER_THAW => {
                self.thaw_list();
                true
            }
            t if (F3_TIMER..F3_TIMER + TIMER_RANGE).contains(&t) => false,

            // F4.
            t if (F4_TIMER..F4_TIMER + TIMER_RANGE).contains(&t) => self.tools_timer(t),

            // F5.
            t if (F5_TIMER..F5_TIMER + TIMER_RANGE).contains(&t) => self.file_tools_timer(t),

            // F6: the search delay (Options > Search).
            super::uiopts::TIMER_SEARCH_DELAY => {
                self.search_delay_done();
                true
            }
            t if (F6_TIMER..F6_TIMER + TIMER_RANGE).contains(&t) => false,

            _ => false,
        }
    }
}

/// Whether `msg` is one of the features' WM_APP messages.
pub fn is_feature_message(msg: u32) -> bool {
    (F1_MSG..F6_MSG + MSG_RANGE).contains(&msg)
}

/// Whether `id` is one of the features' main-window timers.
pub fn is_feature_timer(id: usize) -> bool {
    (F1_TIMER..F6_TIMER + TIMER_RANGE).contains(&id)
}

// The ranges follow each other and stay clear of the window's own.
const _: () = {
    assert!(F2_MSG == F1_MSG + MSG_RANGE && F3_MSG == F2_MSG + MSG_RANGE);
    assert!(F4_MSG == F3_MSG + MSG_RANGE && F5_MSG == F4_MSG + MSG_RANGE);
    assert!(F6_MSG == F5_MSG + MSG_RANGE);
    assert!(super::WM_PANE_CLOSE < F1_MSG);
    assert!(F2_TIMER == F1_TIMER + TIMER_RANGE && F3_TIMER == F2_TIMER + TIMER_RANGE);
    assert!(F4_TIMER == F3_TIMER + TIMER_RANGE && F5_TIMER == F4_TIMER + TIMER_RANGE);
    assert!(F6_TIMER == F5_TIMER + TIMER_RANGE);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filetools::cmd;
    use crate::filter::Filter;
    use crate::window::{
        datatools, file_tools, findbar, newwin, results, sidebars, tabs as tabs_w, tools, uiopts,
    };
    use crate::{columns, menu, searchtools, tabs};

    /// `(name, value)` of each constant, so a failure says which.
    macro_rules! named {
        ($($id:path),* $(,)?) => {
            vec![$((stringify!($id), $id)),*]
        };
    }

    /// Every command the main window runs (menu items of the bar and of
    /// the context menus, accelerators, the panes' and strips' menus), by
    /// name: the window's own and each wave-1 feature's.
    fn commands() -> Vec<(&'static str, u16)> {
        named![
            menu::OPEN,
            menu::REVEAL,
            menu::COPY_FULL_PATH,
            menu::COPY_NAME,
            menu::COPY_FOLDER,
            menu::PROPERTIES,
            menu::EXIT,
            menu::COPY_SIZE,
            menu::COPY_DATE,
            menu::COPY_TSV,
            menu::RUN_AS,
            menu::SET_RUN_COUNT,
            menu::DELETE,
            menu::RENAME,
            menu::DELETE_PERMANENT,
            menu::COPY_EXT,
            menu::COPY_TYPE,
            menu::EXPORT,
            menu::SELECT_ALL,
            menu::FOCUS_SEARCH,
            menu::CLEAR_SEARCH,
            menu::CUT,
            menu::COPY,
            menu::NEXT_SELECTED,
            menu::PREV_SELECTED,
            menu::PASTE,
            menu::REFRESH,
            menu::STATUS_BAR,
            menu::FILTER_BAR,
            menu::ZOOM_IN,
            menu::ZOOM_OUT,
            menu::ZOOM_RESET,
            menu::SORT_NAME,
            menu::SORT_PATH,
            menu::SORT_SIZE,
            menu::SORT_DATE,
            menu::SORT_ASC,
            menu::SORT_DESC,
            menu::THEME_USER_DEFAULT,
            menu::THEME_STANDARD,
            menu::THEME_DARK,
            menu::ON_TOP_NEVER,
            menu::ON_TOP_ALWAYS,
            menu::ON_TOP_SEARCHING,
            menu::VIEW_EXTRA_LARGE,
            menu::VIEW_LARGE,
            menu::VIEW_MEDIUM,
            menu::VIEW_DETAILS,
            menu::RESET_COLUMNS,
            menu::SIZE_COLUMN_TO_FIT,
            menu::SIZE_ALL_COLUMNS_TO_FIT,
            menu::REMOVE_COLUMN,
            menu::MATCH_CASE,
            menu::MATCH_PATH,
            menu::NEXT_FILTER,
            menu::PREV_FILTER,
            menu::INDEX_STATUS,
            menu::NAV_BACK,
            menu::NAV_FORWARD,
            menu::NAV_HOME,
            menu::FIND_SEARCH_HISTORY,
            menu::ORGANIZE_SEARCH_HISTORY,
            menu::RESTORE_SESSION,
            menu::BOOKMARK_ADD,
            menu::BOOKMARK_ORGANIZE,
            menu::BOOKMARK_FIND,
            menu::VIEW_PANE_FOLDERS,
            menu::VIEW_PANE_BOOKMARKS,
            menu::VIEW_PANE_FILTERS,
            menu::VIEW_PANE_PREVIEW,
            menu::CLOSE_PANE,
            menu::NEXT_FRAME,
            menu::PREV_FRAME,
            menu::SERVICE_START,
            menu::SERVICE_STOP,
            menu::SERVICE_RESTART,
            menu::RUN_HISTORY,
            menu::OPTIONS,
            menu::TRAY_SHOW,
            menu::HELP,
            menu::SEARCH_SYNTAX,
            menu::WEBSITE,
            menu::ABOUT,
            searchtools::FIND,
            searchtools::FIND_NEXT,
            searchtools::FIND_PREVIOUS,
            searchtools::GO_TO,
            searchtools::INVERT_SELECTION,
            searchtools::ADVANCED_SEARCH,
            searchtools::ADD_TO_FILTERS,
            searchtools::ORGANIZE_FILTERS,
            searchtools::FULLSCREEN,
            searchtools::WINDOW_SMALL,
            searchtools::WINDOW_MEDIUM,
            searchtools::WINDOW_LARGE,
            searchtools::WINDOW_AUTO_FIT,
            searchtools::BOX_UNDO,
            searchtools::BOX_CUT,
            searchtools::BOX_COPY,
            searchtools::BOX_PASTE,
            searchtools::BOX_DELETE,
            searchtools::BOX_SELECT_ALL,
            searchtools::BOX_AND,
            searchtools::BOX_QUOTE,
            searchtools::BOX_CASE,
            searchtools::BOX_PATH,
            searchtools::BOX_FILES,
            searchtools::BOX_FOLDERS,
            searchtools::BOX_LOCATED_IN,
            searchtools::BOX_DM_BEFORE,
            searchtools::BOX_DM_AFTER,
            searchtools::BOX_DM_TODAY,
            searchtools::BOX_SIZE_SMALLER,
            searchtools::BOX_SIZE_LARGER,
            searchtools::BOX_EXT,
            tabs::NEW_TAB,
            tabs::CLOSE_TAB,
            tabs::NEXT_TAB,
            tabs::PREV_TAB,
            tabs::REOPEN_TAB,
            tabs::DUPLICATE_TAB,
            tabs::CLOSE_TABS_LEFT,
            tabs::CLOSE_TABS_RIGHT,
            tabs::CLOSE_OTHER_TABS,
            tabs::REOPEN_ALL_TABS,
            tabs::NEW_WINDOW,
            tabs::CLOSE_WINDOW,
            cmd::UNDO,
            cmd::REDO,
            cmd::UNDO_HISTORY,
            cmd::COPY_TO_FOLDER,
            cmd::MOVE_TO_FOLDER,
            cmd::ADVANCED_RENAME,
            cmd::SET_FILE_TIME,
            cmd::NEW_FOLDER,
            cmd::CREATE_SHORTCUT,
            sidebars::CLEAR_SELECTION,
            sidebars::UP_ONE_FOLDER,
            sidebars::FOLDER_EXPAND,
            sidebars::FOLDER_OPEN,
            sidebars::FOLDER_OPEN_PATH,
            sidebars::FOLDER_COPY_PATH,
            sidebars::MARK_OPEN,
            sidebars::MARK_EDIT,
            sidebars::MARK_DELETE,
            sidebars::MARK_EXPAND,
            crate::preview::TAB_OUT,
            crate::preview::TAB_BACK_OUT,
            crate::datatools::cmd::OPEN_FILE_LIST,
            crate::datatools::cmd::CLOSE_FILE_LIST,
            crate::datatools::cmd::CLEAR_RECENT_FILE_LISTS,
            crate::datatools::cmd::ADVANCED_COPY_TO,
            crate::datatools::cmd::ADVANCED_MOVE_TO,
            crate::datatools::cmd::IMPORT_EXPORT_SETTINGS,
            crate::datatools::cmd::TROUBLESHOOTING,
            crate::listedit::cmd::FILE_LIST_EDITOR,
        ]
    }

    /// The ranges of commands made one per entry: `(name, first, count)`.
    fn ranges() -> Vec<(&'static str, u16, usize)> {
        vec![
            ("columns", menu::COLUMN_FIRST, columns::COLS),
            ("filters", menu::FILTER_FIRST, Filter::ALL.len()),
            ("history entries", menu::NAV_ENTRY_FIRST, menu::NAV_ENTRIES),
            ("bookmarks", menu::BOOKMARK_FIRST, menu::BOOKMARKS),
            (
                "custom filters",
                menu::CUSTOM_FILTER_FIRST,
                menu::CUSTOM_FILTERS,
            ),
            ("closed tabs", menu::TAB_ENTRY_FIRST, menu::TAB_ENTRIES),
            (
                "recent file lists",
                crate::datatools::cmd::RECENT_FIRST,
                crate::filelist::settings::MENU_MAX,
            ),
            (
                "shell items",
                menu::SHELL_FIRST,
                usize::from(menu::SHELL_LAST - menu::SHELL_FIRST) + 1,
            ),
            // W6 (C): File | Set File List Slot 0..9.
            (
                "file-list slots",
                crate::filelist::slots::cmd::SET_SLOT_FIRST,
                crate::filelist::slots::COUNT,
            ),
        ]
    }

    /// Every command the keyboard editor lists (`keymap::COMMANDS`) is one
    /// the window runs: named above or in one of the ranges.
    #[test]
    fn every_keyboard_command_is_a_window_command() {
        let all = commands();
        let ranges = ranges();
        for c in crate::keymap::COMMANDS {
            let named = all.iter().any(|&(_, id)| id == c.cmd);
            let ranged = ranges.iter().any(|&(_, first, n)| {
                (usize::from(first)..usize::from(first) + n).contains(&usize::from(c.cmd))
            });
            assert!(
                named || ranged,
                "{} ({}) is not a window command",
                c.key,
                c.cmd
            );
        }
        // The slots' range is routed to them (4850..=4899), past the File
        // List Editor's (4750..=4799).
        const {
            use crate::filelist::slots::cmd as slots;
            assert!(crate::listedit::cmd::END <= slots::SET_SLOT_FIRST);
            assert!(slots::SET_SLOT_FIRST == 4850 && slots::END == 4900);
        }
    }

    #[test]
    fn every_command_has_an_id_of_its_own() {
        let all = commands();
        for (i, &(a, id)) in all.iter().enumerate() {
            assert!(id != 0, "{a} has no id");
            for &(b, other) in &all[i + 1..] {
                assert!(id != other, "{a} and {b} are both {id}");
            }
            for (range, first, n) in ranges() {
                let inside =
                    (usize::from(first)..usize::from(first) + n).contains(&usize::from(id));
                assert!(!inside, "{a} ({id}) is in the {range}' ids");
            }
        }
        let ranges = ranges();
        for (i, &(a, first, n)) in ranges.iter().enumerate() {
            for &(b, other, m) in &ranges[i + 1..] {
                let (a0, a1) = (usize::from(first), usize::from(first) + n);
                let (b0, b1) = (usize::from(other), usize::from(other) + m);
                assert!(a1 <= b0 || b1 <= a0, "the {a}' and the {b}' ids meet");
            }
        }
    }

    #[test]
    fn each_feature_keeps_to_its_command_range() {
        let range = |first: u16| first..first + menu::FEATURE_RANGE;
        for (name, id) in commands() {
            let feature = if name.starts_with("crate::preview::") {
                Some(menu::F1_FIRST)
            } else if name.starts_with("sidebars::") {
                Some(menu::F2_FIRST)
            } else if name.starts_with("tabs::") {
                Some(menu::F3_FIRST)
            } else if name.starts_with("searchtools::") {
                Some(menu::F4_FIRST)
            } else if name.starts_with("cmd::") {
                Some(menu::F5_FIRST)
            } else if name.starts_with("crate::datatools::") {
                // W4 (G1): 4600..=4699.
                Some(crate::datatools::cmd::FIRST)
            } else if name.starts_with("crate::listedit::") {
                // W5 (C): 4750..=4799.
                Some(crate::listedit::cmd::FIRST)
            } else {
                None
            };
            match feature {
                Some(first) => assert!(range(first).contains(&id), "{name} ({id})"),
                // The window's own: below the features', or the framework's.
                None => assert!(
                    id < menu::F1_FIRST || range(menu::FRAMEWORK_FIRST).contains(&id),
                    "{name} ({id})"
                ),
            }
        }
    }

    #[test]
    fn the_main_windows_messages_and_timers_are_each_its_own() {
        let messages: Vec<(&str, u32)> = named![
            super::super::WM_REPLY,
            super::super::WM_ICON,
            super::super::WM_SERVICE,
            super::super::WM_STORE,
            super::super::WM_COLUMNS,
            super::super::WM_HISTORY,
            super::super::WM_BOOKMARKS,
            super::super::WM_RUNS,
            super::super::WM_FILEOP,
            super::super::WM_DRAG_DONE,
            super::super::WM_RENAME_AGAIN,
            super::super::WM_TRAY,
            super::super::WM_LAUNCH,
            super::super::WM_EXPORT,
            super::super::WM_THUMB,
            results::WM_FETCHED,
            super::super::WM_PANE_CLOSE,
            crate::preview::WM_RETIRE,
            crate::preview::WM_REPLIES,
            newwin::WM_EXIT_WINDOW,
            newwin::WM_NEW_WINDOW_FAILED,
            findbar::WM_FIND,
            tools::WM_FULLSCREEN_AT_START,
            uiopts::WM_FOCUS_SEARCH,
            // Wave 4 (G1).
            datatools::WM_FILELIST,
            datatools::WM_OTHERS_ENDED,
            // Wave 4 (G2).
            super::super::custom::WM_DOUBLE_CTRL,
            // W6: rows selected a slice at a time.
            super::super::selecting::WM_SELECTING,
            // W6 (C): the File List Editor closed.
            crate::listedit::WM_EDITOR_CLOSED,
        ];
        for (i, &(a, m)) in messages.iter().enumerate() {
            for &(b, other) in &messages[i + 1..] {
                assert!(m != other, "{a} and {b} are both WM_APP + {}", m - WM_APP);
            }
            // A feature's own sit in its range.
            if is_feature_message(m) {
                assert!(
                    !a.starts_with("super::super::") && !a.starts_with("results::"),
                    "{a} is in the features' range"
                );
            }
            // Wave 4's streams route their own ranges (G1 WM_APP + 250..300,
            // G2 + 300..320): nothing else may sit in them.
            let in_g1 = (crate::datatools::MSG_FIRST..crate::datatools::MSG_END).contains(&m);
            assert_eq!(in_g1, a.starts_with("datatools::"), "{a} and G1's range");
            let in_g2 = super::super::custom::is_message(m);
            assert_eq!(
                in_g2,
                a.starts_with("super::super::custom::"),
                "{a} and G2's range"
            );
            // W6 (C) routes WM_APP + 360..370 to `lists_message`: only
            // the File List Editor's sit there.
            let in_w6c =
                (crate::listedit::WM_EDITOR_CLOSED..crate::listedit::W6_MSG_END).contains(&m);
            assert_eq!(
                in_w6c,
                a.starts_with("crate::listedit::"),
                "{a} and W6 C's range"
            );
        }
        assert!(!is_feature_message(crate::datatools::MSG_FIRST));
        assert!(!is_feature_message(crate::datatools::MSG_END - 1));
        assert!(!super::super::custom::is_message(
            crate::datatools::MSG_END - 1
        ));
        let timers: Vec<(&str, usize)> = named![
            super::super::TIMER_REFRESH,
            super::super::TIMER_RETRY,
            super::super::TIMER_NAV,
            super::super::TIMER_NOTE,
            super::super::TIMER_SETTINGS,
            super::super::TIMER_HISTORY,
            super::super::TIMER_SESSION,
            super::super::TIMER_RUNS,
            super::super::TIMER_STORE_RETRY,
            super::super::TIMER_SETTLE,
            super::super::TIMER_LIVE,
            crate::celltip::TIMER,
            tabs_w::TIMER_THAW,
            findbar::TIMER_TYPED,
            file_tools::TIMER_NEW_FOLDER,
            uiopts::TIMER_SEARCH_DELAY,
            // Wave 4 (G2).
            super::super::custom::TIMER_JUMP_LIST,
            // W6.
            super::super::selecting::TIMER_SELECTING,
        ];
        for (i, &(a, t)) in timers.iter().enumerate() {
            for &(b, other) in &timers[i + 1..] {
                assert!(t != other, "{a} and {b} are both timer {t}");
            }
        }
        // Each feature's own in its range.
        assert!((F3_TIMER..F3_TIMER + TIMER_RANGE).contains(&tabs_w::TIMER_THAW));
        assert!((F4_TIMER..F4_TIMER + TIMER_RANGE).contains(&findbar::TIMER_TYPED));
        assert!((F5_TIMER..F5_TIMER + TIMER_RANGE).contains(&file_tools::TIMER_NEW_FOLDER));
        assert!((F6_TIMER..F6_TIMER + TIMER_RANGE).contains(&uiopts::TIMER_SEARCH_DELAY));
        assert!(!is_feature_timer(crate::celltip::TIMER));
        assert!(!is_feature_timer(super::super::selecting::TIMER_SELECTING));
        // G2's timers (300..310) route as a range: only its own sit there.
        for &(a, t) in &timers {
            let in_g2 = super::super::custom::is_timer(t);
            assert_eq!(
                in_g2,
                a.starts_with("super::super::custom::"),
                "{a} and G2's timers"
            );
            assert!(
                !in_g2 || !is_feature_timer(t),
                "{a} is in the features' timers"
            );
        }
    }
}
