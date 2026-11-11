//! Customizable keyboard shortcuts (Options > Keyboard): the commands a key
//! can run, under Everything 1.5's names and INI keys, and the keys bound
//! to each.
//!
//! Everything keeps a command's keys as `<command>_keys`, a comma-separated
//! list of codes: the virtual key in the low byte, and in the high byte
//! Ctrl 0x01, Alt 0x02, Shift 0x04, Win 0x08, and 0x10 "in the search edit
//! only" or 0x20 "in the result list only" (no scope bit: global). goz-gui
//! reads and writes the same codes, so a list copied from Everything.ini
//! means the same here. A command whose key is absent from the file has
//! goz-gui's defaults: the window's shortcuts (`keys::ACCELERATORS`,
//! global), the results list's own keys (`keys::LIST_KEYS`) and the search
//! box's (`keys::SEARCH_BOX_KEYS`).
//!
//! The bindings in force live in a process-wide table ([`install`]): the
//! window's accelerator table is made from it, the list's and the search
//! box's keys are looked up in it, and the preview pane's thread copies the
//! window's shortcuts from it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

use windows_sys::Win32::UI::WindowsAndMessaging::{FALT, FCONTROL, FSHIFT};

use crate::filetools::cmd;
use crate::{keys, menu, searchtools, tabs};

/// Everything's modifier bits, in a code's high byte.
pub const CTRL: u16 = 0x0100;
pub const ALT: u16 = 0x0200;
pub const SHIFT: u16 = 0x0400;
pub const WIN: u16 = 0x0800;
/// Everything's scope bits: the key works only there.
pub const IN_SEARCH_EDIT: u16 = 0x1000;
pub const IN_RESULT_LIST: u16 = 0x2000;

/// Where a key works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    SearchEdit,
    ResultList,
}

impl Scope {
    /// Everything's name for it (Options > Keyboard).
    pub fn name(self) -> &'static str {
        match self {
            Scope::Global => "Global",
            Scope::SearchEdit => "Search Edit",
            Scope::ResultList => "Result List",
        }
    }

    /// Its bits in a code.
    pub fn bits(self) -> u16 {
        match self {
            Scope::Global => 0,
            Scope::SearchEdit => IN_SEARCH_EDIT,
            Scope::ResultList => IN_RESULT_LIST,
        }
    }

    /// Whether a key in `self` and one in `other` can meet: a global key
    /// works everywhere, the other two never in the same place.
    pub fn overlaps(self, other: Scope) -> bool {
        self == Scope::Global || other == Scope::Global || self == other
    }
}

/// The scope a code names; `None` for both scope bits at once.
pub fn scope(code: u16) -> Option<Scope> {
    match code & (IN_SEARCH_EDIT | IN_RESULT_LIST) {
        0 => Some(Scope::Global),
        IN_SEARCH_EDIT => Some(Scope::SearchEdit),
        IN_RESULT_LIST => Some(Scope::ResultList),
        _ => None,
    }
}

/// The chord of a code: its key and modifiers, without the scope.
pub fn chord(code: u16) -> u16 {
    code & 0x0fff
}

/// A code from an accelerator's `FCONTROL`/`FSHIFT`/`FALT` flags, virtual
/// key and scope.
pub const fn code(accel_mods: u8, vk: u16, scope_bits: u16) -> u16 {
    let mut c = (vk & 0xff) | scope_bits;
    if accel_mods & FCONTROL != 0 {
        c |= CTRL;
    }
    if accel_mods & FALT != 0 {
        c |= ALT;
    }
    if accel_mods & FSHIFT != 0 {
        c |= SHIFT;
    }
    c
}

/// A code's modifiers as an accelerator's flags (without `FVIRTKEY`).
pub fn accel_mods(code: u16) -> u8 {
    let mut m = 0;
    if code & CTRL != 0 {
        m |= FCONTROL;
    }
    if code & ALT != 0 {
        m |= FALT;
    }
    if code & SHIFT != 0 {
        m |= FSHIFT;
    }
    m
}

/// Whether goz-gui can bind a code: a key, one scope, and no Windows key
/// (which only a global hotkey could take).
pub fn usable(code: u16) -> bool {
    code & 0xff != 0 && code & WIN == 0 && scope(code).is_some()
}

/// A command a key can run: Everything's INI key (`<command>_keys`), its
/// name in Options > Keyboard ("Category | Command"), and goz-gui's id.
#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub key: &'static str,
    pub label: &'static str,
    pub cmd: u16,
}

const fn c(key: &'static str, label: &'static str, cmd: u16) -> Command {
    Command { key, label, cmd }
}

/// Every command goz-gui lets a key run, in Everything's order (by its
/// "Category | Command" name). The keys are Everything 1.5's; the labels
/// too, but for goz-gui's own names of the same commands.
pub const COMMANDS: &[Command] = &[
    c(
        "bookmarks_add_to_bookmarks_keys",
        "Bookmarks | Add to Bookmarks...",
        menu::BOOKMARK_ADD,
    ),
    c(
        "bookmarks_find_bookmark_keys",
        "Bookmarks | Find Bookmark...",
        menu::BOOKMARK_FIND,
    ),
    c(
        "bookmarks_organize_bookmarks_keys",
        "Bookmarks | Organize Bookmarks...",
        menu::BOOKMARK_ORGANIZE,
    ),
    c(
        "edit_advanced_advanced_rename_keys",
        "Edit | Advanced | Advanced Rename...",
        cmd::ADVANCED_RENAME,
    ),
    c(
        "edit_advanced_set_file_time_keys",
        "Edit | Advanced | Set File Time...",
        cmd::SET_FILE_TIME,
    ),
    c("edit_copy_keys", "Edit | Copy", menu::COPY),
    c(
        "edit_copy_to_folder_keys",
        "Edit | Copy to Folder...",
        cmd::COPY_TO_FOLDER,
    ),
    c("edit_cut_keys", "Edit | Cut", menu::CUT),
    c(
        "edit_find_next_keys",
        "Edit | Find Next",
        searchtools::FIND_NEXT,
    ),
    c(
        "edit_find_previous_keys",
        "Edit | Find Previous",
        searchtools::FIND_PREVIOUS,
    ),
    c("edit_find_keys", "Edit | Find...", searchtools::FIND),
    c("edit_go_to_keys", "Edit | Go To...", searchtools::GO_TO),
    c(
        "edit_invert_selection_keys",
        "Edit | Invert Selection",
        searchtools::INVERT_SELECTION,
    ),
    c(
        "edit_move_to_folder_keys",
        "Edit | Move to Folder...",
        cmd::MOVE_TO_FOLDER,
    ),
    c("edit_paste_keys", "Edit | Paste", menu::PASTE),
    c("edit_redo_keys", "Edit | Redo", cmd::REDO),
    c(
        "edit_select_all_keys",
        "Edit | Select All",
        menu::SELECT_ALL,
    ),
    c("edit_undo_keys", "Edit | Undo", cmd::UNDO),
    c(
        "edit_undo_history_keys",
        "Edit | Undo History...",
        cmd::UNDO_HISTORY,
    ),
    c("file_close_tab_keys", "File | Close Tab", tabs::CLOSE_TAB),
    c(
        "file_close_window_keys",
        "File | Close Window",
        tabs::CLOSE_WINDOW,
    ),
    c(
        "file_copy_as_tsv_keys",
        "File | Copy as TSV",
        menu::COPY_TSV,
    ),
    c(
        "file_copy_full_path_keys",
        "File | Copy Full Path",
        menu::COPY_FULL_PATH,
    ),
    c("file_copy_name_keys", "File | Copy Name", menu::COPY_NAME),
    c("file_copy_path_keys", "File | Copy Path", menu::COPY_FOLDER),
    c(
        "file_create_shortcut_keys",
        "File | Create Shortcut",
        cmd::CREATE_SHORTCUT,
    ),
    c("file_delete_keys", "File | Delete", menu::DELETE),
    c(
        "file_delete_permanently_keys",
        "File | Delete (Permanently)",
        menu::DELETE_PERMANENT,
    ),
    c(
        "file_duplicate_tab_keys",
        "File | Duplicate Tab",
        tabs::DUPLICATE_TAB,
    ),
    c("file_exit_keys", "File | Exit", menu::EXIT),
    c("file_export_keys", "File | Export...", menu::EXPORT),
    c("file_new_tab_keys", "File | New Tab", tabs::NEW_TAB),
    c(
        "file_new_window_keys",
        "File | New Window",
        tabs::NEW_WINDOW,
    ),
    c("file_next_tab_keys", "File | Next Tab", tabs::NEXT_TAB),
    c("file_open_keys", "File | Open", menu::OPEN),
    c("file_open_path_keys", "File | Open Path", menu::REVEAL),
    c(
        "file_previous_tab_keys",
        "File | Previous Tab",
        tabs::PREV_TAB,
    ),
    c(
        "file_properties_keys",
        "File | Properties",
        menu::PROPERTIES,
    ),
    c("file_rename_keys", "File | Rename", menu::RENAME),
    c("file_run_as_keys", "File | Run As", menu::RUN_AS),
    c(
        "file_set_run_count_keys",
        "File | Set Run Count...",
        menu::SET_RUN_COUNT,
    ),
    c(
        "help_about_everything_keys",
        "Help | About goz-gui",
        menu::ABOUT,
    ),
    c("help_everything_help_keys", "Help | goz Help", menu::HELP),
    c(
        "help_everything_website_keys",
        "Help | goz Website",
        menu::WEBSITE,
    ),
    c(
        "help_search_syntax_keys",
        "Help | Search Syntax",
        menu::SEARCH_SYNTAX,
    ),
    c("history_back_keys", "History | Back", menu::NAV_BACK),
    c(
        "history_find_search_history_keys",
        "History | Find Search History...",
        menu::FIND_SEARCH_HISTORY,
    ),
    c(
        "history_forward_keys",
        "History | Forward",
        menu::NAV_FORWARD,
    ),
    c("history_home_keys", "History | Home", menu::NAV_HOME),
    c(
        "history_organize_search_history_keys",
        "History | Organize Search History...",
        menu::ORGANIZE_SEARCH_HISTORY,
    ),
    c(
        "history_reopen_all_tabs_keys",
        "History | Reopen All Tabs",
        tabs::REOPEN_ALL_TABS,
    ),
    c(
        "history_restore_previous_session_keys",
        "History | Restore Previous Session",
        menu::RESTORE_SESSION,
    ),
    c(
        "history_restore_recently_closed_tab_keys",
        "History | Restore Recently Closed Tab",
        tabs::REOPEN_TAB,
    ),
    c(
        "result_list_focus_next_selected_keys",
        "Result List | Focus next selected",
        menu::NEXT_SELECTED,
    ),
    c(
        "result_list_focus_previous_selected_keys",
        "Result List | Focus previous selected",
        menu::PREV_SELECTED,
    ),
    c(
        "result_list_reset_columns_keys",
        "Result List | Reset Columns",
        menu::RESET_COLUMNS,
    ),
    c(
        "result_list_size_all_columns_to_fit_keys",
        "Result List | Size All Columns to Fit",
        menu::SIZE_ALL_COLUMNS_TO_FIT,
    ),
    c(
        "search_add_to_filters_keys",
        "Search | Add to Filters...",
        searchtools::ADD_TO_FILTERS,
    ),
    c(
        "search_advanced_search_keys",
        "Search | Advanced Search...",
        searchtools::ADVANCED_SEARCH,
    ),
    c(
        "search_match_case_keys",
        "Search | Match Case",
        menu::MATCH_CASE,
    ),
    c(
        "search_match_path_keys",
        "Search | Match Path",
        menu::MATCH_PATH,
    ),
    c(
        "search_next_filter_keys",
        "Search | Next Filter",
        menu::NEXT_FILTER,
    ),
    c(
        "search_organize_filters_keys",
        "Search | Organize Filters...",
        searchtools::ORGANIZE_FILTERS,
    ),
    c(
        "search_previous_filter_keys",
        "Search | Previous Filter",
        menu::PREV_FILTER,
    ),
    c(
        "search_edit_clear_keys",
        "Search Edit | Clear",
        menu::CLEAR_SEARCH,
    ),
    c(
        "search_edit_focus_search_edit_keys",
        "Search Edit | Focus Search Edit",
        menu::FOCUS_SEARCH,
    ),
    c("tools_options_keys", "Tools | Options...", menu::OPTIONS),
    c(
        "tools_run_history_keys",
        "Tools | Run History...",
        menu::RUN_HISTORY,
    ),
    c(
        "view_bookmarks_keys",
        "View | Bookmarks",
        menu::VIEW_PANE_BOOKMARKS,
    ),
    c(
        "view_close_pane_keys",
        "View | Close Pane",
        menu::CLOSE_PANE,
    ),
    c("view_details_keys", "View | Details", menu::VIEW_DETAILS),
    c(
        "view_extra_large_thumbnails_keys",
        "View | Extra Large Thumbnails",
        menu::VIEW_EXTRA_LARGE,
    ),
    c(
        "view_filter_bar_keys",
        "View | Filter Bar",
        menu::FILTER_BAR,
    ),
    c(
        "view_filters_keys",
        "View | Filters",
        menu::VIEW_PANE_FILTERS,
    ),
    c(
        "view_folders_keys",
        "View | Folders",
        menu::VIEW_PANE_FOLDERS,
    ),
    c(
        "view_fullscreen_keys",
        "View | Fullscreen",
        searchtools::FULLSCREEN,
    ),
    c(
        "view_large_thumbnails_keys",
        "View | Large Thumbnails",
        menu::VIEW_LARGE,
    ),
    c(
        "view_medium_thumbnails_keys",
        "View | Medium Thumbnails",
        menu::VIEW_MEDIUM,
    ),
    c(
        "view_on_top_always2_keys",
        "View | On Top | Always",
        menu::ON_TOP_ALWAYS,
    ),
    c(
        "view_on_top_never_keys",
        "View | On Top | Never",
        menu::ON_TOP_NEVER,
    ),
    c(
        "view_on_top_while_searching_keys",
        "View | On Top | While Searching",
        menu::ON_TOP_SEARCHING,
    ),
    c(
        "view_preview_keys",
        "View | Preview",
        menu::VIEW_PANE_PREVIEW,
    ),
    c("view_refresh_keys", "View | Refresh", menu::REFRESH),
    c(
        "view_sort_by_ascending_keys",
        "View | Sort By | Ascending",
        menu::SORT_ASC,
    ),
    c(
        "view_sort_by_date_modified_keys",
        "View | Sort By | Date Modified",
        menu::SORT_DATE,
    ),
    c(
        "view_sort_by_descending_keys",
        "View | Sort By | Descending",
        menu::SORT_DESC,
    ),
    c(
        "view_sort_by_name_keys",
        "View | Sort By | Name",
        menu::SORT_NAME,
    ),
    c(
        "view_sort_by_path_keys",
        "View | Sort By | Path",
        menu::SORT_PATH,
    ),
    c(
        "view_sort_by_size_keys",
        "View | Sort By | Size",
        menu::SORT_SIZE,
    ),
    c(
        "view_status_bar_keys",
        "View | Status Bar",
        menu::STATUS_BAR,
    ),
    c("theme_dark_keys", "View | Theme | Dark", menu::THEME_DARK),
    c(
        "theme_standard_keys",
        "View | Theme | Standard",
        menu::THEME_STANDARD,
    ),
    c(
        "theme_user_default_keys",
        "View | Theme | User Default",
        menu::THEME_USER_DEFAULT,
    ),
    c(
        "view_window_size_auto_fit_keys",
        "View | Window Size | Auto Fit",
        searchtools::WINDOW_AUTO_FIT,
    ),
    c(
        "view_window_size_large_keys",
        "View | Window Size | Large",
        searchtools::WINDOW_LARGE,
    ),
    c(
        "view_window_size_medium_keys",
        "View | Window Size | Medium",
        searchtools::WINDOW_MEDIUM,
    ),
    c(
        "view_window_size_small_keys",
        "View | Window Size | Small",
        searchtools::WINDOW_SMALL,
    ),
    c(
        "view_zoom_reset_keys",
        "View | Zoom | Reset",
        menu::ZOOM_RESET,
    ),
    c(
        "view_zoom_zoom_in_keys",
        "View | Zoom | Zoom In",
        menu::ZOOM_IN,
    ),
    c(
        "view_zoom_zoom_out_keys",
        "View | Zoom | Zoom Out",
        menu::ZOOM_OUT,
    ),
    c(
        "window_next_frame_keys",
        "Window | Next Frame",
        menu::NEXT_FRAME,
    ),
    c(
        "window_previous_frame_keys",
        "Window | Previous Frame",
        menu::PREV_FRAME,
    ),
];

/// The command `cmd` is in [`COMMANDS`], by place.
#[cfg(test)]
pub fn index_of(cmd: u16) -> Option<usize> {
    COMMANDS.iter().position(|c| c.cmd == cmd)
}

/// goz-gui's own keys for `cmd`, as codes: the window's shortcuts
/// (global), the list's and the search box's.
pub fn defaults(cmd: u16) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    let tables: [(KeyTable, u16); 3] = [
        (keys::ACCELERATORS, 0),
        (keys::LIST_KEYS, IN_RESULT_LIST),
        (keys::SEARCH_BOX_KEYS, IN_SEARCH_EDIT),
    ];
    for (table, bits) in tables {
        for &(m, vk, c) in table {
            if c == cmd {
                let code = code(m, vk, bits);
                if !out.contains(&code) {
                    out.push(code);
                }
            }
        }
    }
    out
}

/// A table of keys: accelerator flags, virtual key, command.
type KeyTable = &'static [(u8, u16, u16)];

/// The keys of every command in [`COMMANDS`], by place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings(pub Vec<Vec<u16>>);

impl Default for Bindings {
    fn default() -> Bindings {
        Bindings(COMMANDS.iter().map(|c| defaults(c.cmd)).collect())
    }
}

/// A `<command>_keys` value: its codes, in order, each once; what is not a
/// number is skipped. `Some(empty)` is a command with no key.
pub fn parse_codes(value: &str) -> Vec<u16> {
    let mut out = Vec::new();
    for part in value.split(',') {
        if let Ok(n) = part.trim().parse::<u16>()
            && n & 0xff != 0
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

pub fn format_codes(codes: &[u16]) -> String {
    codes
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

impl Bindings {
    /// The keys `get` holds under Everything's names; a command whose key
    /// is missing keeps its defaults.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Bindings {
        Bindings(
            COMMANDS
                .iter()
                .map(|c| get(c.key).map_or_else(|| defaults(c.cmd), parse_codes))
                .collect(),
        )
    }

    /// Every command's key and value, in [`COMMANDS`]' order.
    pub fn to_keys(&self) -> Vec<(String, String)> {
        COMMANDS
            .iter()
            .zip(&self.0)
            .map(|(c, codes)| (c.key.to_owned(), format_codes(codes)))
            .collect()
    }

    pub fn of(&self, i: usize) -> &[u16] {
        self.0.get(i).map_or(&[], Vec::as_slice)
    }

    /// The command (by place) other than `except` that a key `code` would
    /// meet: the same chord where both work.
    pub fn conflict(&self, code: u16, except: Option<usize>) -> Option<(usize, u16)> {
        let sc = scope(code)?;
        for (i, codes) in self.0.iter().enumerate() {
            if Some(i) == except {
                continue;
            }
            for &other in codes {
                if chord(other) == chord(code) && scope(other).is_some_and(|o| o.overlaps(sc)) {
                    return Some((i, other));
                }
            }
        }
        None
    }

    /// Gives command `i` the key `code`, taking it from any command it
    /// would meet there (Everything unbinds the other on OK). Returns the
    /// commands it was taken from.
    pub fn add(&mut self, i: usize, code: u16) -> Vec<usize> {
        let mut taken = Vec::new();
        while let Some((other, old)) = self.conflict(code, Some(i)) {
            self.0[other].retain(|&k| k != old);
            taken.push(other);
        }
        // The same chord in another scope of this command goes too: one
        // key, one place.
        if let Some(codes) = self.0.get_mut(i) {
            codes.retain(|&k| chord(k) != chord(code));
            codes.push(code);
        }
        taken
    }

    pub fn remove(&mut self, i: usize, code: u16) {
        if let Some(codes) = self.0.get_mut(i) {
            codes.retain(|&k| k != code);
        }
    }

    /// The tables the window works from.
    pub fn effective(&self) -> Effective {
        let ours: Vec<u16> = COMMANDS.iter().map(|c| c.cmd).collect();
        let mut e = Effective::default();
        // The defaults of commands the editor does not list stay as they
        // are.
        let fixed = |table: &[(u8, u16, u16)], out: &mut Vec<(u8, u16, u16)>| {
            for &(m, vk, cmd) in table {
                if !ours.contains(&cmd) {
                    out.push((m, vk, cmd));
                }
            }
        };
        fixed(keys::ACCELERATORS, &mut e.global);
        fixed(keys::LIST_KEYS, &mut e.list);
        fixed(keys::SEARCH_BOX_KEYS, &mut e.edit);
        for (c, codes) in COMMANDS.iter().zip(&self.0) {
            for &code in codes {
                if !usable(code) {
                    continue;
                }
                let entry = (accel_mods(code), code & 0xff, c.cmd);
                let table = match scope(code) {
                    Some(Scope::Global) => &mut e.global,
                    Some(Scope::ResultList) => &mut e.list,
                    Some(Scope::SearchEdit) => &mut e.edit,
                    None => continue,
                };
                // A chord taken twice (a hand-edited file) runs the first.
                if !table.iter().any(|&(m, k, _)| (m, k) == (entry.0, entry.1)) {
                    table.push(entry);
                }
            }
        }
        for (i, (c, codes)) in COMMANDS.iter().zip(&self.0).enumerate() {
            if *codes != defaults(c.cmd) {
                e.changed.push((c.cmd, self.0[i].first().copied()));
            }
        }
        e
    }
}

/// The keys in force: accelerator flags, virtual key, command.
#[derive(Clone, Debug, Default)]
pub struct Effective {
    pub global: Vec<(u8, u16, u16)>,
    pub list: Vec<(u8, u16, u16)>,
    pub edit: Vec<(u8, u16, u16)>,
    /// The commands whose keys are not goz-gui's defaults, with the first
    /// of them: the menus show that one.
    pub changed: Vec<(u16, Option<u16>)>,
}

fn find(table: &[(u8, u16, u16)], mods: u8, vk: u16) -> Option<u16> {
    table
        .iter()
        .find(|&&(m, k, _)| m == mods && k == vk)
        .map(|&(_, _, c)| c)
}

impl Effective {
    /// The command a key pressed in the results list runs, if one of its
    /// own (the window's shortcuts came first).
    pub fn list_command(&self, mods: u8, vk: u16) -> Option<u16> {
        find(&self.list, mods, vk)
    }

    /// The command a key pressed in the search box runs, if one of its own.
    pub fn edit_command(&self, mods: u8, vk: u16) -> Option<u16> {
        find(&self.edit, mods, vk)
    }

    /// What a menu item for `cmd` shows after its tab, when its keys are
    /// not the defaults the menu was written with: `Some(None)` for none.
    pub fn menu_key(&self, cmd: u16) -> Option<Option<String>> {
        self.changed
            .iter()
            .find(|&&(c, _)| c == cmd)
            .map(|&(_, first)| first.map(key_name))
    }
}

static CURRENT: LazyLock<RwLock<Arc<Effective>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Bindings::default().effective())));
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Puts `bindings` in force for every thread of the process.
pub fn install(bindings: &Bindings) {
    let e = Arc::new(bindings.effective());
    match CURRENT.write() {
        Ok(mut g) => *g = e,
        Err(poisoned) => *poisoned.into_inner() = e,
    }
    GENERATION.fetch_add(1, Ordering::Release);
}

/// The keys in force.
pub fn current() -> Arc<Effective> {
    match CURRENT.read() {
        Ok(g) => Arc::clone(&g),
        Err(poisoned) => Arc::clone(&poisoned.into_inner()),
    }
}

/// Changes each time [`install`] runs: a table made from the keys before
/// is out of date.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Acquire)
}

/// A virtual key's name as menus write it (US names for the punctuation
/// keys; [`key_name`] asks the keyboard layout first).
fn vk_name(vk: u8) -> String {
    match vk {
        b'0'..=b'9' | b'A'..=b'Z' => (vk as char).to_string(),
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0d => "Enter".into(),
        0x13 => "Pause".into(),
        0x14 => "Caps Lock".into(),
        0x1b => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2c => "Print Screen".into(),
        0x2d => "Insert".into(),
        0x2e => "Del".into(),
        0x5d => "Menu".into(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x6a => "Num *".into(),
        0x6b => "Num +".into(),
        0x6d => "Num -".into(),
        0x6e => "Num .".into(),
        0x6f => "Num /".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6f),
        0x90 => "Num Lock".into(),
        0x91 => "Scroll Lock".into(),
        0xa6 => "Browser Back".into(),
        0xa7 => "Browser Forward".into(),
        0xac => "Browser Home".into(),
        0xba => ";".into(),
        0xbb => "=".into(),
        0xbc => ",".into(),
        0xbd => "-".into(),
        0xbe => ".".into(),
        0xbf => "/".into(),
        0xc0 => "`".into(),
        0xdb => "[".into(),
        0xdc => "\\".into(),
        0xdd => "]".into(),
        0xde => "'".into(),
        _ => format!("Key {vk:#04x}"),
    }
}

/// A code's keys as Everything writes them: "Ctrl+Shift+T".
pub fn key_name(code: u16) -> String {
    let mut s = String::new();
    for (bit, name) in [
        (CTRL, "Ctrl+"),
        (ALT, "Alt+"),
        (SHIFT, "Shift+"),
        (WIN, "Win+"),
    ] {
        if code & bit != 0 {
            s.push_str(name);
        }
    }
    s.push_str(&vk_name((code & 0xff) as u8));
    s
}

/// A code as Options > Keyboard lists it: "Ctrl+D (Global)".
pub fn binding_name(code: u16) -> String {
    match scope(code) {
        Some(sc) => format!("{} ({})", key_name(code), sc.name()),
        None => format!("{} (?)", key_name(code)),
    }
}

/// Keys that only modify another: no binding on their own.
pub fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5b | 0x5c | 0xa0..=0xa5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everythings_codes_read_and_write_back() {
        // Everything 1.5's defaults as its INI writes them.
        assert_eq!(parse_codes("340"), vec![0x154]);
        assert_eq!(parse_codes("343,371"), vec![343, 371]);
        assert_eq!(parse_codes(" 8538 , 8712 ,x,0"), vec![8538, 8712]);
        assert_eq!(parse_codes(""), Vec::<u16>::new());
        assert_eq!(format_codes(&[343, 371]), "343,371");
        assert_eq!(key_name(340), "Ctrl+T");
        assert_eq!(key_name(1367), "Ctrl+Shift+W");
        assert_eq!(key_name(8712), "Alt+Backspace");
        assert_eq!(binding_name(8538), "Ctrl+Z (Result List)");
        assert_eq!(binding_name(4209), "F2 (Search Edit)");
        assert_eq!(binding_name(324), "Ctrl+D (Global)");
        assert_eq!(key_name(363), "Ctrl+Num +");
        assert_eq!(key_name(477), "Ctrl+]");
    }

    #[test]
    fn goz_guis_defaults_are_everythings_codes() {
        let d = |cmd| defaults(cmd);
        // `file_new_tab_keys=340`, `file_close_tab_keys=343,371`.
        assert_eq!(d(tabs::NEW_TAB), vec![340]);
        assert_eq!(d(tabs::CLOSE_TAB), vec![343, 371]);
        // `edit_undo_keys=8538,8712`, `edit_copy_keys=8515,8493`,
        // `file_open_keys=8205,9229`, `file_delete_keys=8238`,
        // `file_rename_keys=8305`, `edit_select_all_keys=8513`.
        assert_eq!(d(cmd::UNDO), vec![8538, 8712]);
        assert_eq!(d(menu::COPY), vec![8515, 8493]);
        assert_eq!(d(menu::OPEN), vec![8205, 9229]);
        assert_eq!(d(menu::DELETE), vec![8238]);
        assert_eq!(d(menu::RENAME), vec![8305]);
        assert_eq!(d(menu::SELECT_ALL), vec![8513]);
        // `search_advanced_search_keys=4209`: F2 in the search box.
        assert_eq!(d(searchtools::ADVANCED_SEARCH), vec![4209]);
        // `view_zoom_reset_keys=304,352`, `edit_find_keys=326`.
        assert_eq!(d(menu::ZOOM_RESET), vec![304, 352]);
        assert_eq!(d(searchtools::FIND), vec![326]);
    }

    #[test]
    fn every_command_once_with_everythings_key_shape() {
        for (i, a) in COMMANDS.iter().enumerate() {
            assert!(a.key.ends_with("_keys"), "{}", a.key);
            assert!(a.label.contains(" | "), "{}", a.label);
            for b in &COMMANDS[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.cmd, b.cmd, "{} and {}", a.key, b.key);
            }
        }
        // Every window shortcut's command can be rebound, but for those the
        // editor leaves fixed.
        for &(_, _, cmd) in keys::ACCELERATORS {
            assert!(index_of(cmd).is_some(), "shortcut for {cmd} not listed");
        }
    }

    #[test]
    fn defaults_make_the_same_tables_as_before() {
        let e = Bindings::default().effective();
        for &(m, vk, cmd) in keys::ACCELERATORS {
            assert_eq!(find(&e.global, m, vk), Some(cmd));
        }
        for &(m, vk, cmd) in keys::LIST_KEYS {
            assert_eq!(e.list_command(m, vk), Some(cmd));
        }
        for &(m, vk, cmd) in keys::SEARCH_BOX_KEYS {
            assert_eq!(e.edit_command(m, vk), Some(cmd));
        }
        assert_eq!(e.global.len(), keys::ACCELERATORS.len());
        assert_eq!(e.list.len(), keys::LIST_KEYS.len());
        assert!(e.changed.is_empty());
    }

    #[test]
    fn a_file_without_the_key_keeps_the_default_and_an_empty_one_has_none() {
        let b = Bindings::from_keys(|k| (k == "file_new_tab_keys").then_some(""));
        let i = index_of(tabs::NEW_TAB).unwrap();
        assert!(b.of(i).is_empty());
        let j = index_of(tabs::CLOSE_TAB).unwrap();
        assert_eq!(b.of(j), &[343, 371]);
        let e = b.effective();
        assert_eq!(find(&e.global, FCONTROL, u16::from(b'T')), None);
        assert_eq!(e.menu_key(tabs::NEW_TAB), Some(None));
        assert_eq!(e.menu_key(tabs::CLOSE_TAB), None);
        // Round trip.
        let pairs = b.to_keys();
        let again = Bindings::from_keys(|k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        });
        assert_eq!(again, b);
    }

    #[test]
    fn a_key_given_to_another_command_leaves_the_first() {
        let mut b = Bindings::default();
        let new_tab = index_of(tabs::NEW_TAB).unwrap();
        let options = index_of(menu::OPTIONS).unwrap();
        // Ctrl+T (global) on Options...
        assert_eq!(b.conflict(340, Some(options)), Some((new_tab, 340)));
        let taken = b.add(options, 340);
        assert_eq!(taken, vec![new_tab]);
        assert!(b.of(new_tab).is_empty());
        assert_eq!(b.of(options), &[336, 340]);
        let e = b.effective();
        assert_eq!(
            find(&e.global, FCONTROL, u16::from(b'T')),
            Some(menu::OPTIONS)
        );
        assert_eq!(e.menu_key(menu::OPTIONS), Some(Some("Ctrl+P".into())));
        // A list-only key and a search-box-only key of one chord can both
        // be: they never meet. A global one meets both.
        let copy = index_of(menu::COPY).unwrap();
        let find_i = index_of(searchtools::FIND).unwrap();
        assert_eq!(b.conflict(0x1143, Some(find_i)), None, "Ctrl+C in the box");
        assert_eq!(b.conflict(0x0143, Some(find_i)), Some((copy, 8515)));
        b.remove(options, 340);
        assert_eq!(b.of(options), &[336]);
    }

    #[test]
    fn keys_goz_gui_cannot_bind_are_kept_but_not_used() {
        let b = Bindings::from_keys(|k| (k == "tools_options_keys").then_some("2128,336,12624"));
        let i = index_of(menu::OPTIONS).unwrap();
        // Win+P and a key in both scopes are kept for the file.
        assert_eq!(b.of(i), &[0x850, 336, 0x3150]);
        let e = b.effective();
        assert_eq!(
            e.global
                .iter()
                .filter(|&&(_, _, c)| c == menu::OPTIONS)
                .count(),
            1
        );
        assert!(!usable(0x850));
        assert!(!usable(0x3150));
        assert!(usable(336));
    }

    #[test]
    fn accelerator_flags_and_codes_convert_both_ways() {
        for m in 0..8u8 {
            let mods = (if m & 1 != 0 { FCONTROL } else { 0 })
                | (if m & 2 != 0 { FALT } else { 0 })
                | (if m & 4 != 0 { FSHIFT } else { 0 });
            let c = code(mods, 0x41, 0);
            assert_eq!(accel_mods(c), mods);
        }
        assert_eq!(code(FCONTROL | FSHIFT, 0x57, 0), 1367);
        assert_eq!(code(0, 0x71, IN_RESULT_LIST), 8305);
    }
}
