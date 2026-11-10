//! goz-gui's settings as the settings file (`goz-gui.ini`) holds them.
//!
//! Keys are Everything 1.5's wherever the meaning is the same, with its
//! values and defaults (from the INI Everything writes), so a user who
//! knows one knows the other. goz-gui's own keys (`window_dpi`) do not
//! collide with any of Everything's. A value goz-gui cannot read (an
//! Everything filter goz-gui lacks, a sort by Extension) reads as the
//! default and is left in the file as it is unless the user changes that
//! setting here.
//!
//! **Home** (`home_*`) works as Everything's does: `0` uses the last value,
//! `1` a fixed ("custom") one. The fixed value is the plain key's
//! (`home_match_case=1` with `match_case=1` makes Home match case), so the
//! window does not write that key while it is fixed (Options > Home does,
//! from the settings read: see `window/prefs.rs`). `home_match_case` and
//! `home_match_path` also take Everything's `2` (always on) and `3` (always
//! off), Options > Home's "Enabled" and "Disabled", which fix the option
//! without the plain key: that one still holds the last value and is
//! written as it changes. Any other number uses the last value. A window
//! that does not start from a session starts from Home: the plain keys,
//! with `2` and `3` applied ([`Settings::home_start`]); History > Home sets
//! the fixed values and leaves the others as they are. `home_view` fixes
//! the view (`view`, `thumbnail_size`) the same way.

use goz_core::types::{SortDir, SortKey, SortSpec};

use crate::bookmarks::{self, sort_key, sort_name};
use crate::columns::Layout;
use crate::dock::DockSettings;
use crate::filter::Filter;
use crate::menu;
use crate::places::Places;
use crate::store::ini::Ini;
use crate::theme::ThemeMode;
use crate::ui_options::UiOptions;
use crate::view::View;

/// View > On Top (`ontop`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnTop {
    #[default]
    Never,
    Always,
    /// Only while the search box has text.
    WhileSearching,
}

impl OnTop {
    pub fn menu_id(self) -> u16 {
        match self {
            OnTop::Never => menu::ON_TOP_NEVER,
            OnTop::Always => menu::ON_TOP_ALWAYS,
            OnTop::WhileSearching => menu::ON_TOP_SEARCHING,
        }
    }

    /// Whether the window stays above other windows; `searching` is whether
    /// the search box has text.
    pub fn topmost(self, searching: bool) -> bool {
        match self {
            OnTop::Never => false,
            OnTop::Always => true,
            OnTop::WhileSearching => searching,
        }
    }

    pub(crate) fn from_number(n: i64) -> Option<OnTop> {
        match n {
            0 => Some(OnTop::Never),
            1 => Some(OnTop::Always),
            2 => Some(OnTop::WhileSearching),
            _ => None,
        }
    }

    pub(crate) fn number(self) -> u8 {
        match self {
            OnTop::Never => 0,
            OnTop::Always => 1,
            OnTop::WhileSearching => 2,
        }
    }
}

/// `theme`: 0 User Default, 1 Standard, 2 Dark (Everything's Theme list).
pub(crate) fn theme_from(n: i64) -> Option<ThemeMode> {
    match n {
        0 => Some(ThemeMode::UserDefault),
        1 => Some(ThemeMode::Standard),
        2 => Some(ThemeMode::Dark),
        _ => None,
    }
}

pub(crate) fn theme_number(mode: ThemeMode) -> u8 {
    match mode {
        ThemeMode::UserDefault => 0,
        ThemeMode::Standard => 1,
        ThemeMode::Dark => 2,
    }
}

/// The window's last normal (not minimized or maximized) rectangle, in
/// screen pixels, as Everything keeps it, and the DPI it had then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPos {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// `window_dpi`; `None` for a file without it (Everything's own): the
    /// size is taken as it is.
    pub dpi: Option<u32>,
}

/// The history settings (Options > History in Everything), for the
/// search and run histories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryOptions {
    pub search_enabled: bool,
    pub search_days_to_keep: u32,
    pub search_keep_forever: bool,
    pub search_always_suggest: bool,
    pub run_enabled: bool,
    pub run_days_to_keep: u32,
    pub run_keep_forever: bool,
    /// `run_history_increase_run_count_on_open_path`: Open Path counts as
    /// a run of the items whose folders it opens.
    pub run_on_open_path: bool,
}

impl Default for HistoryOptions {
    fn default() -> HistoryOptions {
        HistoryOptions {
            search_enabled: true,
            search_days_to_keep: 90,
            search_keep_forever: true,
            search_always_suggest: false,
            run_enabled: true,
            run_days_to_keep: 90,
            run_keep_forever: true,
            run_on_open_path: false,
        }
    }
}

/// What the first Enter in the search box does after the search changed
/// (`search_edit_enter_action`, Everything's list in its order); the next
/// Enter opens what it selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnterAction {
    /// Nothing but count the search as made.
    None,
    /// Select the held result run most (Everything's default); with none
    /// ever run, the top one.
    #[default]
    MostRun,
    /// Select the held result run last; with none ever run, the top one.
    LastRun,
    /// Select the focused result (the top one after a new search).
    Result,
    /// Select the top result.
    Top,
    /// Select the bottom result.
    Bottom,
    /// Select the search box's text.
    SelectSearch,
}

impl EnterAction {
    const ALL: [EnterAction; 7] = [
        EnterAction::None,
        EnterAction::MostRun,
        EnterAction::LastRun,
        EnterAction::Result,
        EnterAction::Top,
        EnterAction::Bottom,
        EnterAction::SelectSearch,
    ];

    fn from_number(n: i64) -> Option<EnterAction> {
        usize::try_from(n)
            .ok()
            .and_then(|i| EnterAction::ALL.get(i).copied())
    }

    fn number(self) -> usize {
        EnterAction::ALL
            .iter()
            .position(|&a| a == self)
            .unwrap_or(1)
    }
}

/// What History > Home sets: the fixed values; `None` keeps the value in
/// force ("use last value"). The options are a bookmark's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Home {
    pub search: Option<String>,
    pub options: bookmarks::Settings,
    /// Details, or thumbnails at a size (`home_view=1`).
    pub view: Option<View>,
}

/// What `home_match_case` or `home_match_path` says Home does with the
/// option (Everything's numbers, as its Options > Home lists them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HomeOption {
    /// `0` (and any number Everything does not use): the last value.
    #[default]
    Last,
    /// `1`: the plain key's value (`match_case`), which is then not written.
    Custom,
    /// `2`: on.
    On,
    /// `3`: off.
    Off,
}

impl HomeOption {
    fn from_number(n: i64) -> HomeOption {
        match n {
            1 => HomeOption::Custom,
            2 => HomeOption::On,
            3 => HomeOption::Off,
            _ => HomeOption::Last,
        }
    }

    fn number(self) -> u8 {
        match self {
            HomeOption::Last => 0,
            HomeOption::Custom => 1,
            HomeOption::On => 2,
            HomeOption::Off => 3,
        }
    }

    /// What Home sets the option to, `plain` being the plain key's value;
    /// `None` keeps the value in force.
    fn value(self, plain: bool) -> Option<bool> {
        match self {
            HomeOption::Last => None,
            HomeOption::Custom => Some(plain),
            HomeOption::On => Some(true),
            HomeOption::Off => Some(false),
        }
    }
}

/// What the `home_*` keys say: the match options' choice, and for the
/// others whether they are `1` (a fixed value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeFixed {
    pub match_case: HomeOption,
    pub match_path: HomeOption,
    pub search: bool,
    pub filter: bool,
    pub sort: bool,
    pub view: bool,
}

impl Default for HomeFixed {
    /// Everything's: the search is fixed (to an empty one), the rest use the
    /// last value.
    fn default() -> HomeFixed {
        HomeFixed {
            match_case: HomeOption::Last,
            match_path: HomeOption::Last,
            search: true,
            filter: false,
            sort: false,
            view: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub window: Option<WindowPos>,
    pub maximized: bool,
    pub match_case: bool,
    pub match_path: bool,
    pub search: String,
    pub filter: Filter,
    pub sort: SortSpec,
    pub theme: ThemeMode,
    pub on_top: OnTop,
    pub status_bar: bool,
    pub columns: Layout,
    pub history: HistoryOptions,
    pub session_store: bool,
    /// `session_store_save_delay`: how often, at most, the session is
    /// written while it changes (ms).
    pub session_save_delay: u32,
    /// `session_store_restore_on_first_new_window`: a new window starts as
    /// the previous session left it.
    pub session_restore_first: bool,
    pub nav_bar: bool,
    /// `search_edit_enter_action`.
    pub enter_action: EnterAction,
    pub home: HomeFixed,
    /// `bookmark_remember_*`: what a new bookmark takes from the window.
    pub bookmark_remember: bookmarks::Remember,
    pub ini_backup: bool,
    pub csv_backup: bool,
    /// Where the dialogs and the Search Syntax window open (see
    /// `crate::places`).
    pub places: Places,
    /// The window and the tray: Everything's keys of the same names.
    pub background: Background,
    /// `filters_visible`: View > Filter Bar.
    pub filter_bar: bool,
    /// `zoom`: the result list's View > Zoom, in percent.
    pub zoom: u32,
    /// `live_update_interval_ms` (goz-gui's own): how often the results
    /// of a window on screen are searched again; 0 for never.
    pub live_update_ms: u32,
    /// `utf8_bom`: an export starts with a UTF-8 byte order mark.
    pub utf8_bom: bool,
    /// `last_export_type`: File > Export's file type (0 EFU, 1 CSV, 2 TXT).
    pub export_type: usize,
    /// Details or thumbnails (`view`, `thumbnail_size`,
    /// `statusbar_view_thumbnail_size`).
    pub view: View,
    /// The docks and their panes (`left_sidebar_wide`, `<pane>_visible`,
    /// ..., see `crate::dock`).
    pub docks: DockSettings,
    /// `options_last_page`: the Options page shown last, by name.
    pub options_page: String,
    // Wave 1: each feature adds its settings on its own lines below (here,
    // in `Default`, `from_ini` and `to_pairs`), so their changes never meet
    // (FRAMEWORK.md).

    // F1 (preview pane).
    /// The preview pane's keys (`preview_fill`, `preview_load_size`,
    /// `show_preview_handlers_in_preview_pane`, ..., see
    /// `crate::preview`).
    pub preview: crate::preview::PreviewSettings,
    // F2 (sidebars).
    /// The sidebars' own keys (`folders_sidebar_labels`,
    /// `bookmarks_sidebar_expanded`, ..., see `crate::sidebars`).
    pub sidebars: crate::sidebars::Settings,
    // F3 (tabs and windows).
    /// The tab strip, the tabs' keys and New Window's offset
    /// (`tabs_always_show`, `tabs_min_wide`, ..., see `crate::tabs`).
    pub tabs: crate::tabs::TabSettings,
    // F4 (search tools).
    /// The find bar, fullscreen, View > Window Size, Located in...
    /// (`crate::searchtools`).
    pub search_tools: crate::searchtools::SearchTools,
    // F5 (file tools).

    // F6 (integration and the Options pages).
    /// What Options' UI, Search, Results and View pages set
    /// (`allow_multiple_windows`, `search_as_you_type`, `size_format`, ...;
    /// see `crate::ui_options`).
    pub ui: UiOptions,
}

/// Running in the background, the tray icon and the global hotkeys, under
/// Everything's key names and with its defaults (all off).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Background {
    /// `run_in_background`: closing the window hides it; goz-gui keeps
    /// running until File > Exit.
    pub run_in_background: bool,
    /// `show_tray_icon`.
    pub show_tray_icon: bool,
    /// `minimize_to_tray`: minimizing hides the window to the tray icon.
    pub minimize_to_tray: bool,
    /// `single_click_tray`: one click on the tray icon shows the window
    /// (otherwise a double-click).
    pub single_click_tray: bool,
    /// `toggle_window_key` and `show_window_key`: global hotkeys, as a
    /// hotkey control holds them (virtual key in the low byte, `HOTKEYF_*`
    /// in the high byte); 0 for none.
    pub toggle_window_key: u16,
    pub show_window_key: u16,
}

impl Background {
    /// Whether the tray icon shows: asked for, or needed to reach a window
    /// that runs in the background.
    pub fn tray_icon(&self) -> bool {
        self.show_tray_icon
    }
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            window: None,
            maximized: false,
            match_case: false,
            match_path: false,
            search: String::new(),
            filter: Filter::Everything,
            sort: SortSpec {
                key: SortKey::Name,
                dir: SortDir::Asc,
            },
            theme: ThemeMode::UserDefault,
            on_top: OnTop::Never,
            status_bar: true,
            columns: Layout::default(),
            history: HistoryOptions::default(),
            session_store: true,
            session_save_delay: SESSION_SAVE_DELAY,
            session_restore_first: false,
            nav_bar: false,
            enter_action: EnterAction::MostRun,
            home: HomeFixed::default(),
            bookmark_remember: bookmarks::Remember::default(),
            ini_backup: true,
            csv_backup: true,
            places: Places::default(),
            background: Background::default(),
            filter_bar: false,
            zoom: crate::zoom::DEFAULT,
            live_update_ms: LIVE_UPDATE_MS,
            utf8_bom: false,
            export_type: 0,
            view: View::default(),
            docks: DockSettings::default(),
            options_page: String::new(),
            // Wave 1 (FRAMEWORK.md).

            // F1 (preview pane).
            preview: crate::preview::PreviewSettings::default(),
            // F2 (sidebars).
            sidebars: crate::sidebars::Settings::default(),
            // F3 (tabs and windows).
            tabs: crate::tabs::TabSettings::default(),
            // F4 (search tools).
            search_tools: crate::searchtools::SearchTools::default(),
            // F5 (file tools).

            // F6 (integration and the Options pages).
            ui: UiOptions::default(),
        }
    }
}

fn number(ini: &Ini, key: &str) -> Option<i64> {
    ini.get(key)?.trim().parse().ok()
}

fn flag(ini: &Ini, key: &str) -> Option<bool> {
    match ini.get(key)?.trim() {
        "1" => Some(true),
        "0" => Some(false),
        v if v.eq_ignore_ascii_case("true") => Some(true),
        v if v.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// A hotkey key's value: a hotkey control's word, 0 (none) if unreadable.
fn hotkey(ini: &Ini, key: &str) -> u16 {
    number(ini, key)
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or(0)
}

fn bit(on: bool) -> String {
    u8::from(on).to_string()
}

/// Everything's `session_store_save_delay`: the session is written at most
/// every 15 s while it changes.
pub const SESSION_SAVE_DELAY: u32 = 15_000;
/// The shortest delay taken from the file, so a typo cannot make goz-gui
/// write the session many times a second.
const SESSION_SAVE_DELAY_MIN: u32 = 1000;

/// How often a window on screen searches again, by default (ms).
pub const LIVE_UPDATE_MS: u32 = 2000;
/// The shortest interval taken from the file (0 aside, which is off), so a
/// typo cannot make goz-gui search many times a second.
pub const LIVE_UPDATE_MIN: u32 = 500;

/// The keys Home can fix, each with the plain key(s) holding its value.
const HOME_KEYS: [(&str, &[&str]); 6] = [
    ("home_match_case", &["match_case"]),
    ("home_match_path", &["match_path"]),
    ("home_search", &["search"]),
    ("home_filter", &["filter"]),
    ("home_sort", &["sort", "sort_ascending"]),
    ("home_view", &["view", "thumbnail_size"]),
];

impl Settings {
    /// The settings in `ini`; what it lacks or has unreadable is the
    /// default.
    pub fn from_ini(ini: &Ini) -> Settings {
        let d = Settings::default();
        let window = match (
            number(ini, "window_x"),
            number(ini, "window_y"),
            number(ini, "window_wide"),
            number(ini, "window_high"),
        ) {
            (Some(x), Some(y), Some(w), Some(h))
                if (1..=100_000).contains(&w)
                    && (1..=100_000).contains(&h)
                    && x.abs() <= 100_000
                    && y.abs() <= 100_000 =>
            {
                Some(WindowPos {
                    x: x as i32,
                    y: y as i32,
                    width: w as i32,
                    height: h as i32,
                    dpi: number(ini, "window_dpi")
                        .filter(|d| (48..=1920).contains(d))
                        .map(|d| d as u32),
                })
            }
            _ => None,
        };
        let sort = ini
            .get("sort")
            .and_then(sort_key)
            .map_or(d.sort, |key| SortSpec {
                key,
                dir: match flag(ini, "sort_ascending") {
                    Some(true) => SortDir::Asc,
                    Some(false) => SortDir::Desc,
                    None => SortSpec::default_for(key).dir,
                },
            });
        let h = d.history;
        let days = |key: &str, default: u32| {
            number(ini, key)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(default)
        };
        let fixed = |key: &str, default: bool| number(ini, key).map_or(default, |n| n == 1);
        let option = |key: &str, default: HomeOption| {
            number(ini, key).map_or(default, HomeOption::from_number)
        };
        let r = d.bookmark_remember;
        let remember = |key: &str, default: bool| flag(ini, key).unwrap_or(default);
        Settings {
            window,
            maximized: flag(ini, "maximized").unwrap_or(d.maximized),
            match_case: flag(ini, "match_case").unwrap_or(d.match_case),
            match_path: flag(ini, "match_path").unwrap_or(d.match_path),
            search: ini.get("search").unwrap_or_default().to_owned(),
            // Empty is Everything; a filter goz-gui does not have too.
            filter: ini
                .get("filter")
                .and_then(Filter::from_title)
                .unwrap_or(d.filter),
            sort,
            theme: number(ini, "theme").and_then(theme_from).unwrap_or(d.theme),
            on_top: number(ini, "ontop")
                .and_then(OnTop::from_number)
                .unwrap_or(d.on_top),
            status_bar: flag(ini, "status_bar_visible").unwrap_or(d.status_bar),
            columns: Layout::from_keys(|k| ini.get(k)),
            history: HistoryOptions {
                search_enabled: flag(ini, "search_history_enabled").unwrap_or(h.search_enabled),
                search_days_to_keep: days("search_history_days_to_keep", h.search_days_to_keep),
                search_keep_forever: flag(ini, "search_history_keep_forever")
                    .unwrap_or(h.search_keep_forever),
                search_always_suggest: flag(ini, "search_history_always_suggest")
                    .unwrap_or(h.search_always_suggest),
                run_enabled: flag(ini, "run_history_enabled").unwrap_or(h.run_enabled),
                run_days_to_keep: days("run_history_days_to_keep", h.run_days_to_keep),
                run_keep_forever: flag(ini, "run_history_keep_forever")
                    .unwrap_or(h.run_keep_forever),
                run_on_open_path: flag(ini, "run_history_increase_run_count_on_open_path")
                    .unwrap_or(h.run_on_open_path),
            },
            session_store: flag(ini, "session_store").unwrap_or(d.session_store),
            session_save_delay: number(ini, "session_store_save_delay")
                .and_then(|n| u32::try_from(n).ok())
                .map_or(d.session_save_delay, |n| n.max(SESSION_SAVE_DELAY_MIN)),
            session_restore_first: flag(ini, "session_store_restore_on_first_new_window")
                .unwrap_or(d.session_restore_first),
            nav_bar: flag(ini, "toolbar_nav_bar_visible").unwrap_or(d.nav_bar),
            enter_action: number(ini, "search_edit_enter_action")
                .and_then(EnterAction::from_number)
                .unwrap_or(d.enter_action),
            home: HomeFixed {
                match_case: option("home_match_case", d.home.match_case),
                match_path: option("home_match_path", d.home.match_path),
                search: fixed("home_search", d.home.search),
                filter: fixed("home_filter", d.home.filter),
                sort: fixed("home_sort", d.home.sort),
                view: fixed("home_view", d.home.view),
            },
            bookmark_remember: bookmarks::Remember {
                case: remember("bookmark_remember_case", r.case),
                path: remember("bookmark_remember_path", r.path),
                filter: remember("bookmark_remember_filter", r.filter),
                sort: remember("bookmark_remember_sort", r.sort),
                search: remember("bookmark_remember_search", r.search),
            },
            ini_backup: flag(ini, "ini_backup").unwrap_or(d.ini_backup),
            csv_backup: flag(ini, "csv_backup").unwrap_or(d.csv_backup),
            places: Places::from_keys(|k| ini.get(k)),
            background: Background {
                run_in_background: flag(ini, "run_in_background")
                    .unwrap_or(d.background.run_in_background),
                show_tray_icon: flag(ini, "show_tray_icon").unwrap_or(d.background.show_tray_icon),
                minimize_to_tray: flag(ini, "minimize_to_tray")
                    .unwrap_or(d.background.minimize_to_tray),
                single_click_tray: flag(ini, "single_click_tray")
                    .unwrap_or(d.background.single_click_tray),
                toggle_window_key: hotkey(ini, "toggle_window_key"),
                show_window_key: hotkey(ini, "show_window_key"),
            },
            filter_bar: flag(ini, "filters_visible").unwrap_or(d.filter_bar),
            zoom: number(ini, "zoom").map_or(d.zoom, crate::zoom::clamp),
            live_update_ms: number(ini, "live_update_interval_ms")
                .and_then(|n| u32::try_from(n).ok())
                .map_or(d.live_update_ms, |n| {
                    if n == 0 { 0 } else { n.max(LIVE_UPDATE_MIN) }
                }),
            utf8_bom: flag(ini, "utf8_bom").unwrap_or(d.utf8_bom),
            export_type: number(ini, "last_export_type")
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n < 3)
                .unwrap_or(d.export_type),
            view: View::from_keys(|k| ini.get(k)),
            docks: DockSettings::from_keys(|k| ini.get(k)),
            options_page: ini.get("options_last_page").unwrap_or_default().to_owned(),
            // Wave 1 (FRAMEWORK.md).

            // F1 (preview pane).
            preview: crate::preview::PreviewSettings::from_keys(|k| ini.get(k)),
            // F2 (sidebars).
            sidebars: crate::sidebars::Settings::from_keys(|k| ini.get(k)),
            // F3 (tabs and windows).
            tabs: crate::tabs::TabSettings::from_keys(|k| ini.get(k)),
            // F4 (search tools).
            search_tools: crate::searchtools::SearchTools::from_keys(|k| ini.get(k)),
            // F5 (file tools).

            // F6 (integration and the Options pages).
            ui: UiOptions::from_keys(|k| ini.get(k)),
        }
    }

    /// Every key goz-gui keeps, with its value, in a fixed order.
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut put = |k: &str, v: String| out.push((k.to_owned(), v));
        if let Some(w) = self.window {
            put("window_x", w.x.to_string());
            put("window_y", w.y.to_string());
            put("window_wide", w.width.to_string());
            put("window_high", w.height.to_string());
            if let Some(dpi) = w.dpi {
                put("window_dpi", dpi.to_string());
            }
        }
        put("maximized", bit(self.maximized));
        put("ontop", self.on_top.number().to_string());
        put("match_path", bit(self.match_path));
        put("match_case", bit(self.match_case));
        put("status_bar_visible", bit(self.status_bar));
        put("filters_visible", bit(self.filter_bar));
        put("zoom", self.zoom.to_string());
        put("theme", theme_number(self.theme).to_string());
        put("search", self.search.clone());
        put(
            "filter",
            match self.filter {
                Filter::Everything => String::new(),
                f => f.title().to_owned(),
            },
        );
        put("sort", sort_name(self.sort.key).to_owned());
        put("sort_ascending", bit(self.sort.dir == SortDir::Asc));
        let h = &self.history;
        put("search_history_enabled", bit(h.search_enabled));
        put("run_history_enabled", bit(h.run_enabled));
        put(
            "search_history_days_to_keep",
            h.search_days_to_keep.to_string(),
        );
        put("run_history_days_to_keep", h.run_days_to_keep.to_string());
        put("search_history_keep_forever", bit(h.search_keep_forever));
        put("run_history_keep_forever", bit(h.run_keep_forever));
        put(
            "run_history_increase_run_count_on_open_path",
            bit(h.run_on_open_path),
        );
        put(
            "search_history_always_suggest",
            bit(h.search_always_suggest),
        );
        put("session_store", bit(self.session_store));
        put(
            "session_store_save_delay",
            self.session_save_delay.to_string(),
        );
        put(
            "session_store_restore_on_first_new_window",
            bit(self.session_restore_first),
        );
        put("toolbar_nav_bar_visible", bit(self.nav_bar));
        put(
            "search_edit_enter_action",
            self.enter_action.number().to_string(),
        );
        let f = &self.home;
        put("home_match_case", f.match_case.number().to_string());
        put("home_match_path", f.match_path.number().to_string());
        put("home_search", bit(f.search));
        put("home_filter", bit(f.filter));
        put("home_sort", bit(f.sort));
        put("home_view", bit(f.view));
        // Everything's order.
        let r = &self.bookmark_remember;
        put("bookmark_remember_case", bit(r.case));
        put("bookmark_remember_path", bit(r.path));
        put("bookmark_remember_sort", bit(r.sort));
        put("bookmark_remember_filter", bit(r.filter));
        put("bookmark_remember_search", bit(r.search));
        let b = &self.background;
        put("run_in_background", bit(b.run_in_background));
        put("show_tray_icon", bit(b.show_tray_icon));
        put("minimize_to_tray", bit(b.minimize_to_tray));
        put("single_click_tray", bit(b.single_click_tray));
        put("toggle_window_key", b.toggle_window_key.to_string());
        put("show_window_key", b.show_window_key.to_string());
        put("ini_backup", bit(self.ini_backup));
        put("csv_backup", bit(self.csv_backup));
        put("utf8_bom", bit(self.utf8_bom));
        put("last_export_type", self.export_type.to_string());
        put("live_update_interval_ms", self.live_update_ms.to_string());
        out.extend(self.columns.to_keys());
        out.extend(self.places.to_keys());
        out.extend(self.view.to_keys());
        out.extend(self.docks.to_keys());
        out.push(("options_last_page".into(), self.options_page.clone()));
        // Wave 1 (FRAMEWORK.md).

        // F1 (preview pane).
        out.extend(self.preview.to_keys());

        // F2 (sidebars).
        out.extend(self.sidebars.to_keys());

        // F3 (tabs and windows).
        out.extend(self.tabs.to_keys());

        // F4 (search tools).
        out.extend(self.search_tools.to_keys());

        // F5 (file tools).

        // F6 (integration and the Options pages).
        out.extend(self.ui.to_keys());

        out
    }

    /// Whether `key` holds a Home value that is fixed: goz-gui then leaves
    /// it as it is in the file, whatever the window does.
    pub fn is_fixed(&self, key: &str) -> bool {
        let f = &self.home;
        // Only `1` takes the plain key as Home's value; with `2` or `3` it
        // still holds the last value, written as it changes.
        let on = [
            f.match_case == HomeOption::Custom,
            f.match_path == HomeOption::Custom,
            f.search,
            f.filter,
            f.sort,
            f.view,
        ];
        HOME_KEYS
            .iter()
            .zip(on)
            .any(|((_, keys), fixed)| fixed && keys.contains(&key))
    }

    /// What History > Home does with these settings as the fixed values.
    pub fn home(&self) -> Home {
        let f = &self.home;
        Home {
            search: f.search.then(|| self.search.clone()),
            options: bookmarks::Settings {
                match_case: f.match_case.value(self.match_case),
                match_path: f.match_path.value(self.match_path),
                filter: f.filter.then_some(self.filter),
                sort: f.sort.then_some(self.sort),
            },
            view: f.view.then_some(self.view),
        }
    }

    /// The settings a window starts with when it starts from Home rather
    /// than a session: the plain keys (which hold Home's fixed values),
    /// with the match options Home holds always on or off.
    pub fn home_start(&self) -> Settings {
        let f = &self.home;
        Settings {
            match_case: f
                .match_case
                .value(self.match_case)
                .unwrap_or(self.match_case),
            match_path: f
                .match_path
                .value(self.match_path)
                .unwrap_or(self.match_path),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::{COL_DATE, COL_NAME, COL_PATH};

    /// A slice of the INI Everything 1.5.0.1424b writes with its defaults.
    const EVERYTHING: &str = "; Please make sure Everything is not running before modifying this file.\r\n\
        [Everything]\r\n\
        window_x=200\r\nwindow_y=150\r\nwindow_wide=1400\r\nwindow_high=850\r\nmaximized=0\r\n\
        ontop=0\r\nmatch_path=0\r\nmatch_case=0\r\nsearch=\r\nhome_match_case=0\r\n\
        home_match_path=0\r\nhome_search=1\r\nhome_filter=0\r\nhome_sort=0\r\ntheme=0\r\n\
        ini_backup=1\r\ncsv_backup=1\r\nfilter=\r\nsort=Name\r\nsort_ascending=1\r\n\
        status_bar_visible=1\r\nsearch_history_enabled=1\r\nrun_history_enabled=1\r\n\
        search_history_days_to_keep=90\r\nrun_history_days_to_keep=90\r\n\
        search_history_keep_forever=1\r\nrun_history_keep_forever=1\r\n\
        run_history_increase_run_count_on_open_path=0\r\nsearch_edit_enter_action=1\r\n\
        search_history_always_suggest=0\r\nsession_store=1\r\nsession_store_save_delay=15000\r\n\
        session_store_restore_on_first_new_window=0\r\ntoolbar_nav_bar_visible=0\r\n\
        bookmark_remember_case=1\r\nbookmark_remember_path=1\r\nbookmark_remember_sort=0\r\n\
        bookmark_remember_filter=0\r\nbookmark_remember_search=1\r\n\
        name_column_pos=0\r\nname_column_width=256\r\npath_column_visible=1\r\n\
        path_column_pos=1\r\npath_column_width=256\r\nsize_column_visible=1\r\n\
        size_column_pos=2\r\nsize_column_width=96\r\nlast_write_time_column_visible=1\r\n\
        last_write_time_column_pos=3\r\nlast_write_time_column_width=153\r\n";

    #[test]
    fn everythings_defaults_are_goz_guis() {
        // The same keys under goz-gui's section header.
        let ini = Ini::from_text(&EVERYTHING.replace("[Everything]", "[goz-gui]"));
        let s = Settings::from_ini(&ini);
        let d = Settings::default();
        assert_eq!(
            s.window,
            Some(WindowPos {
                x: 200,
                y: 150,
                width: 1400,
                height: 850,
                dpi: None
            })
        );
        assert_eq!(Settings { window: None, ..s }, d);
        // And an empty file is the defaults too.
        assert_eq!(Settings::from_ini(&Ini::default()), d);
    }

    #[test]
    fn written_pairs_read_back_the_same() {
        let s = Settings {
            window: Some(WindowPos {
                x: -1900,
                y: 40,
                width: 1200,
                height: 700,
                dpi: Some(144),
            }),
            maximized: true,
            match_case: true,
            match_path: true,
            search: "a=b ; c".into(),
            filter: Filter::Executable,
            sort: SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Asc,
            },
            theme: ThemeMode::Dark,
            on_top: OnTop::WhileSearching,
            status_bar: false,
            columns: {
                let mut c = Layout::default();
                c.hide(crate::columns::COL_SIZE);
                c.set_order(&[COL_DATE, COL_NAME, COL_PATH]);
                c.set_width(COL_PATH, 400);
                c
            },
            history: HistoryOptions {
                search_enabled: false,
                search_days_to_keep: 7,
                search_keep_forever: false,
                search_always_suggest: true,
                run_enabled: false,
                run_days_to_keep: 30,
                run_keep_forever: false,
                run_on_open_path: true,
            },
            session_store: false,
            session_save_delay: 2500,
            session_restore_first: true,
            nav_bar: true,
            enter_action: EnterAction::Bottom,
            home: HomeFixed {
                match_case: HomeOption::Custom,
                match_path: HomeOption::Off,
                search: false,
                filter: true,
                sort: true,
                view: true,
            },
            bookmark_remember: bookmarks::Remember {
                case: false,
                path: true,
                filter: true,
                sort: true,
                search: false,
            },
            ini_backup: false,
            csv_backup: false,
            places: {
                let mut p = Places::default();
                p.set(
                    crate::places::Which::BookmarkEdit,
                    crate::places::Place {
                        width: 600,
                        height: 450,
                        dpi: Some(120),
                        ..crate::places::Place::default()
                    },
                );
                p
            },
            background: Background {
                run_in_background: true,
                show_tray_icon: true,
                minimize_to_tray: true,
                single_click_tray: true,
                toggle_window_key: 0x0247,
                show_window_key: 0x0647,
            },
            filter_bar: true,
            zoom: 150,
            live_update_ms: 0,
            utf8_bom: true,
            export_type: 2,
            view: View::thumbnails(crate::view::EXTRA_LARGE),
            docks: {
                let mut d = DockSettings {
                    left_wide: 320,
                    right_wide: 200,
                    ..DockSettings::default()
                };
                let id = crate::dock::PaneId::Preview;
                d.set_pane(
                    id,
                    crate::dock::PaneLayout {
                        visible: true,
                        high: 300,
                        ..id.default_layout()
                    },
                );
                d
            },
            options_page: "History".into(),
            // F1 (preview pane).
            preview: crate::preview::PreviewSettings {
                fill: true,
                load_size: 2048,
                ..crate::preview::PreviewSettings::default()
            },
            // F3 (tabs and windows).
            tabs: crate::tabs::TabSettings {
                always_show: true,
                min_wide: 90,
                max_closed: 5,
                offset_new_window: false,
                ..crate::tabs::TabSettings::default()
            },
            // F2 (sidebars).
            sidebars: crate::sidebars::Settings {
                clear_on_close: true,
                match_only_when_shown: false,
                labels: false,
                bookmarks_expanded: vec!["Music".into(), "Music/Rock".into()],
            },
            // F6 (integration and the Options pages).
            ui: UiOptions {
                alternate_rows: true,
                size_format: crate::ui_options::SizeFormat::Auto,
                rename_stem: crate::ui_options::Tri::Off,
                ..UiOptions::default()
            },
            // F4 (search tools).
            search_tools: crate::searchtools::SearchTools {
                findbar_visible: true,
                fullscreen: true,
                ..crate::searchtools::SearchTools::default()
            },
        };
        let mut ini = Ini::default();
        for (k, v) in s.to_pairs() {
            ini.set(&k, &v);
        }
        assert_eq!(Settings::from_ini(&ini), s);
        let text = ini.to_text();
        for line in [
            "theme=2",
            "ontop=2",
            "filter=Executable",
            "sort=Date Modified",
            "sort_ascending=1",
            "window_dpi=144",
            "status_bar_visible=0",
            "view=1",
            "thumbnail_size=256",
            "statusbar_view_thumbnail_size=256",
            "maximized=1",
            "session_store_save_delay=2500",
            "session_store_restore_on_first_new_window=1",
            "bookmark_remember_case=0",
            "bookmark_remember_filter=1",
            "bookmark_remember_search=0",
            "run_history_increase_run_count_on_open_path=1",
            "search_edit_enter_action=5",
            "bookmark_edit_wide=600",
            "bookmark_edit_dpi=120",
            "search_history_maximized=0",
            "filters_visible=1",
            "zoom=150",
            "live_update_interval_ms=0",
            "utf8_bom=1",
            "last_export_type=2",
            "left_sidebar_wide=320",
            "findbar_visible=1",
            "fullscreen=1",
            "right_sidebar_wide=200",
            "preview_visible=1",
            "preview_side=1",
            "preview_high=300",
            "folders_sidebar_visible=0",
            "folders_sidebar_tab_stop=1",
            "pane_title=1",
            "pane_min_wide=32",
            "options_last_page=History",
            "home_view=1",
            "alternate_row_color=1",
            "size_format=0",
            "rename_select_filepart_only=2",
            "show_mouseover=1",
        ] {
            assert!(text.contains(&format!("{line}\r\n")), "{line} in\n{text}");
        }
    }

    #[test]
    fn zoom_and_live_updates_read_within_bounds() {
        let read =
            |text: &str| Settings::from_ini(&Ini::from_text(&format!("[goz-gui]\r\n{text}")));
        let d = Settings::default();
        assert_eq!((d.zoom, d.live_update_ms, d.filter_bar), (100, 2000, false));
        assert_eq!(read("zoom=9999\r\n").zoom, 500);
        assert_eq!(read("zoom=x\r\n").zoom, 100);
        assert_eq!(read("live_update_interval_ms=0\r\n").live_update_ms, 0);
        assert_eq!(read("live_update_interval_ms=10\r\n").live_update_ms, 500);
        assert_eq!(read("live_update_interval_ms=-1\r\n").live_update_ms, 2000);
        assert_eq!(read("last_export_type=7\r\n").export_type, 0);
        assert!(read("filters_visible=1\r\n").filter_bar);
    }

    #[test]
    fn unreadable_values_are_the_defaults() {
        let ini = Ini::from_text(
            "[goz-gui]\r\ntheme=7\r\nontop=x\r\nfilter=Pictures\r\nsort=Extension\r\n\
             sort_ascending=0\r\nmatch_case=yes\r\nwindow_x=1\r\nwindow_y=2\r\nwindow_wide=0\r\n\
             window_high=10\r\nsearch_history_days_to_keep=-3\r\n",
        );
        let s = Settings::from_ini(&ini);
        assert_eq!(s, Settings::default());
        // Enter's action: Everything's numbers; one past its list is the
        // default.
        for (text, action) in [
            ("search_edit_enter_action=0\r\n", EnterAction::None),
            ("search_edit_enter_action=2\r\n", EnterAction::LastRun),
            ("search_edit_enter_action=3\r\n", EnterAction::Result),
            ("search_edit_enter_action=4\r\n", EnterAction::Top),
            ("search_edit_enter_action=6\r\n", EnterAction::SelectSearch),
            ("search_edit_enter_action=7\r\n", EnterAction::MostRun),
            ("search_edit_enter_action=-1\r\n", EnterAction::MostRun),
            ("search_edit_enter_action=x\r\n", EnterAction::MostRun),
        ] {
            assert_eq!(
                Settings::from_ini(&Ini::from_text(text)).enter_action,
                action,
                "{text}"
            );
        }
        for action in EnterAction::ALL {
            assert_eq!(
                EnterAction::from_number(action.number() as i64),
                Some(action)
            );
        }
        // A sort without a direction takes the key's own.
        let s = Settings::from_ini(&Ini::from_text("sort=size\r\n"));
        assert_eq!(s.sort, SortSpec::default_for(SortKey::Size));
        // The session's delay: a bad one is the default, a tiny one 1 s.
        for (text, delay) in [
            ("session_store_save_delay=-1\r\n", SESSION_SAVE_DELAY),
            ("session_store_save_delay=x\r\n", SESSION_SAVE_DELAY),
            ("session_store_save_delay=10\r\n", 1000),
            ("session_store_save_delay=60000\r\n", 60_000),
        ] {
            assert_eq!(
                Settings::from_ini(&Ini::from_text(text)).session_save_delay,
                delay
            );
        }
    }

    #[test]
    fn home_fixes_what_its_keys_say() {
        let s = Settings::from_ini(&Ini::default());
        // Everything's defaults: the empty search, the rest as they are.
        assert_eq!(
            s.home(),
            Home {
                search: Some(String::new()),
                options: bookmarks::Settings::default(),
                view: None,
            }
        );
        assert!(s.is_fixed("search"));
        assert!(!s.is_fixed("match_case") && !s.is_fixed("sort"));
        let s = Settings::from_ini(&Ini::from_text(
            "home_match_case=1\r\nmatch_case=1\r\nhome_search=0\r\nsearch=old\r\n\
             home_sort=1\r\nsort=Size\r\nsort_ascending=1\r\nhome_filter=2\r\nfilter=Audio\r\n",
        ));
        let home = s.home();
        assert_eq!(home.search, None, "the last search stays");
        assert_eq!(home.options.match_case, Some(true));
        assert_eq!(home.options.match_path, None);
        assert_eq!(home.options.filter, None, "only 1 fixes it");
        assert_eq!(
            home.options.sort,
            Some(SortSpec {
                key: SortKey::Size,
                dir: SortDir::Asc
            })
        );
        for key in ["match_case", "sort", "sort_ascending"] {
            assert!(s.is_fixed(key), "{key}");
        }
        for key in ["search", "filter", "match_path", "theme"] {
            assert!(!s.is_fixed(key), "{key}");
        }
    }

    #[test]
    fn home_match_options_take_everythings_on_and_off() {
        // 2 always on, 3 always off, whatever the plain key says; the plain
        // key keeps the last value and is written as it changes.
        let s = Settings::from_ini(&Ini::from_text(
            "home_match_case=2\r\nmatch_case=0\r\nhome_match_path=3\r\nmatch_path=1\r\n",
        ));
        assert_eq!(s.home.match_case, HomeOption::On);
        assert_eq!(s.home.match_path, HomeOption::Off);
        let home = s.home();
        assert_eq!(home.options.match_case, Some(true));
        assert_eq!(home.options.match_path, Some(false));
        assert!(!s.is_fixed("match_case") && !s.is_fixed("match_path"));
        // Written back as they were read.
        let pairs = s.to_pairs();
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("home_match_case"), Some("2"));
        assert_eq!(get("home_match_path"), Some("3"));
        // 1: the plain key's value, not written; 0 or any other number:
        // the last value.
        for (text, home, fixed) in [
            ("home_match_case=1\r\nmatch_case=1\r\n", Some(true), true),
            ("home_match_case=1\r\nmatch_case=0\r\n", Some(false), true),
            ("home_match_case=0\r\nmatch_case=1\r\n", None, false),
            ("home_match_case=4\r\nmatch_case=1\r\n", None, false),
            ("home_match_case=x\r\n", None, false),
        ] {
            let s = Settings::from_ini(&Ini::from_text(text));
            assert_eq!(s.home().options.match_case, home, "{text}");
            assert_eq!(s.is_fixed("match_case"), fixed, "{text}");
        }
    }

    #[test]
    fn a_window_starts_from_home() {
        // Always on and always off apply at the start; the last value and a
        // fixed one are the plain key's already.
        for (text, case, path) in [
            (
                "home_match_case=2\r\nmatch_case=0\r\nhome_match_path=3\r\nmatch_path=1\r\n",
                true,
                false,
            ),
            (
                "home_match_case=0\r\nmatch_case=1\r\nhome_match_path=1\r\nmatch_path=1\r\n",
                true,
                true,
            ),
            ("match_case=0\r\nmatch_path=0\r\n", false, false),
        ] {
            let s = Settings::from_ini(&Ini::from_text(text)).home_start();
            assert_eq!((s.match_case, s.match_path), (case, path), "{text}");
        }
        // The rest is as read.
        let s = Settings::from_ini(&Ini::from_text("search=x\r\nhome_search=0\r\n"));
        assert_eq!(s.home_start().search, "x");
    }

    #[test]
    fn home_can_fix_the_view() {
        let s = Settings::from_ini(&Ini::from_text(
            "home_view=1\r\nview=1\r\nthumbnail_size=64\r\n",
        ));
        assert_eq!(s.home().view, Some(View::thumbnails(crate::view::MEDIUM)));
        assert!(s.is_fixed("view") && s.is_fixed("thumbnail_size"));
        assert!(!s.is_fixed("statusbar_view_thumbnail_size"));
        let s = Settings::from_ini(&Ini::from_text("view=1\r\n"));
        assert_eq!(s.home().view, None);
        assert!(!s.is_fixed("view"));
    }
}
