//! The main window: search field, virtual results list, status bar, and the
//! regions around them (`crate::layout`): the tab strip and the find bar
//! (`crate::strip`), and the docks either side of the results with their
//! panes (`crate::dock`, `crate::pane`, `docks.rs`).
//!
//! Everything here runs on the UI thread. State that the window procedure
//! touches sits in `Cell`/`RefCell`s on one `App` that lives as long as the
//! process. A `RefCell` borrow is never held across a call that can send a
//! message back into the window procedure (most `SendMessageW` calls to the
//! list can), which is what keeps re-entrancy from panicking.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use goz_core::proto::DaemonStatus;
use goz_core::types::{SortDir, SortKey, SortSpec};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DC_BRUSH, HFONT, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE,
};
use windows_sys::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD, APPCOMMAND_BROWSER_HOME, MK_CONTROL,
};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT, VK_TAB,
};
use windows_sys::Win32::UI::Shell::HDROP;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::celltip::{self, CellTip};
use crate::client::{BLOCK_ROWS, Failure, Job, Reply, Search, Worker};
use crate::columns::Layout;
use crate::dropdown::Dropdown;
use crate::fileops::{self, FileOps};
use crate::filter::{self, Filter};
use crate::highlight::Span;
use crate::history::{NavHistory, SearchHistory};
use crate::icons::{GENERIC, IconReply, Icons};
use crate::layout::Regions;
use crate::model::{Model, Pending};
use crate::pane::PaneEvent;
use crate::placement::{self, Rect};
use crate::searchbox::Button;
use crate::settings::{OnTop, Settings};
use crate::status::Indicator;
use crate::store::writer::Writer;
use crate::strip::{Strip, StripSlot};
use crate::theme::{Palette, ThemeMode};
use crate::thumbs::{ThumbReply, Thumbs};
use crate::typeahead::TypeAhead;
use crate::view::View;
use crate::wide::window_text;
use crate::win::class::{self, Class, Create};
use crate::win::ext::b1;
use crate::win::{self, Msg, gdi, lv, parcel, queue, subclass, sys};
use crate::{
    dialog, dropdown, format, header, hover, keys, menu, msgbox, paint, places, searchbox, shell,
    status, syntax, theme,
};

pub use crate::columns::{COL_DATE, COL_EXT, COL_NAME, COL_PATH, COL_SIZE, COL_TYPE};
pub use w9a::WM_INDEX_SET;

mod background;
mod bar;
mod bookmarks;
mod cols;
mod commands;
mod context;
// Wave 4 (G2): customization, and accessibility of the painted parts.
mod access;
mod custom;
mod fonts;
pub use access::pane_object;
mod docks;
mod edit;
mod export;
mod files;
mod live;
mod memory;
mod nav;
mod opts;
mod prefs;
mod reselect;
mod results;
mod runs;
mod searches;
// W6: rows selected one by one, a slice at a time.
mod selecting;
mod session;
pub mod slots;
mod thumbview;
mod typing;
// Wave 1: each feature declares its window-side modules on its own lines
// below, so their changes never meet (FRAMEWORK.md).

// F1 (preview pane).
mod preview;

// F2 (sidebars).
mod bookmarks_pane;
mod filters_pane;
mod folders_pane;
mod sidebars;

// F3 (tabs and windows).
mod newwin;
mod tabs;
mod tabstrip;

// F4 (search tools).
mod custom_filters;
mod findbar;
mod tools;

// F5 (file tools).
mod file_tools;

// F6 (integration and the Options pages).
mod uiopts;

// W4 (G1: data and file tools).
mod datatools;

// W6 (C: file-list slots, the File List Editor's start).
mod lists;

// W9 (B1): the wave 9 columns' values, Options > Indexes, content:.
mod w9a;
mod w9b;

pub const WM_REPLY: u32 = WM_APP + 1;
pub const WM_ICON: u32 = WM_APP + 2;
/// A Tools > goz Service command finished; wParam is 1 if it ran.
pub const WM_SERVICE: u32 = WM_APP + 3;
/// The store thread could not write a file (wParam `prefs::STORE_FAILED`:
/// it is tried again), or wrote it with something to say
/// (`prefs::STORE_NOTE`); lParam is a `Box<String>` saying what.
pub const WM_STORE: u32 = WM_APP + 4;
/// The header's columns were dragged or resized.
pub const WM_COLUMNS: u32 = WM_APP + 5;
/// The store thread wrote the search history; lParam is a
/// `Box<history_file::Written>`.
pub const WM_HISTORY: u32 = WM_APP + 6;
/// The store thread wrote the bookmarks; lParam is a `Box<Option<String>>`,
/// a note when the file could not be read and was moved aside.
pub const WM_BOOKMARKS: u32 = WM_APP + 7;
/// The store thread wrote the run history; lParam is a
/// `Box<run_history_file::Written>`.
pub const WM_RUNS: u32 = WM_APP + 8;
/// A file operation finished; lParam is a `Box<fileops::Done>`.
pub const WM_FILEOP: u32 = WM_APP + 9;
/// A drag of the results ended (after any drop on the window it made).
const WM_DRAG_DONE: u32 = WM_APP + 10;
/// A name typed in Rename's box was refused: say why and open it again.
const WM_RENAME_AGAIN: u32 = WM_APP + 11;
/// The tray icon's clicks and menu (`crate::tray`).
const WM_TRAY: u32 = WM_APP + 12;
/// A command line from a second start: a `Box<Vec<String>>` in lParam.
const WM_LAUNCH: u32 = WM_APP + 13;
/// File > Export's thread: wParam `crate::export::PROGRESS` or `ENDED`
/// (lParam a `Box<crate::export::Ended>`).
const WM_EXPORT: u32 = WM_APP + 14;
/// The types' thread answered (`typeinfo`): the list paints again.
const WM_TYPES: u32 = WM_APP + 15;
/// Posted by a paint that met types not known yet: they are sent to the
/// types' thread, together, once the paint is over.
const WM_TYPES_ASK: u32 = WM_APP + 16;
/// A thumbnail was made: a `Box<thumbs::ThumbReply>` in lParam. Kept apart
/// from the numbers above so others can be added there.
const WM_THUMB: u32 = WM_APP + 30;
/// The framework's messages are `WM_APP + 100..120`; the features' follow
/// (`slots.rs`). A pane's close button was clicked: wParam is the pane's
/// `PaneId::code`.
pub const WM_PANE_CLOSE: u32 = WM_APP + 100;

const ID_EDIT: usize = 100;
const ID_LIST: usize = 101;
const TIMER_REFRESH: usize = 1;
const TIMER_RETRY: usize = 2;
/// The search has stayed the same long enough to be a navigation entry.
const TIMER_NAV: usize = 3;
/// The status bar's note has been there long enough.
const TIMER_NOTE: usize = 4;
/// The settings have stopped changing: time to write them.
const TIMER_SETTINGS: usize = 5;
/// The search history has stopped changing: time to write it.
const TIMER_HISTORY: usize = 6;
/// Time to write the session, if it changed (`session_store_save_delay`).
const TIMER_SESSION: usize = 7;
/// The run history has stopped changing: time to write it.
const TIMER_RUNS: usize = 8;
/// A write failed a while ago: time to try what is still staged again.
const TIMER_STORE_RETRY: usize = 9;
/// Files changed (a delete, paste, drag): time to search again.
const TIMER_SETTLE: usize = 10;
/// Time to search again, so the results follow the files (`live.rs`).
const TIMER_LIVE: usize = 11;
/// The window has rested since thumbnails came or a pane closed: memory
/// not looked at goes (`memory.rs`).
const TIMER_REST: usize = 12;
// The framework's timers are 100..110; the features' follow (`slots.rs`).

/// The search options; the default is History > Home's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Options {
    match_case: bool,
    match_path: bool,
    filter: Filter,
    sort: SortSpec,
}

pub struct Fonts {
    pub normal: HFONT,
    pub bold: HFONT,
    /// Text height in pixels.
    pub height: i32,
}

impl Fonts {
    /// The message font at `dpi` as `adjust` changes it (a face and size
    /// of Options > Fonts and Colors), then `zoom` % its size.
    fn make(
        dpi: u32,
        zoom: u32,
        adjust: impl FnOnce(&mut windows_sys::Win32::Graphics::Gdi::LOGFONTW),
    ) -> Fonts {
        // The font handles are owned by `Fonts`.
        let ncm = sys::non_client_metrics(dpi).unwrap_or_default();
        let mut lf = ncm.lfMessageFont;
        adjust(&mut lf);
        if zoom != crate::zoom::DEFAULT {
            lf.lfHeight = crate::zoom::scale(lf.lfHeight, 96, zoom);
            lf.lfWidth = crate::zoom::scale(lf.lfWidth, 96, zoom);
        }
        let normal = gdi::create_font(&lf);
        lf.lfWeight = 700;
        let bold = gdi::create_font(&lf);
        // The screen's DC, measured in and given back; the old font is put
        // back first.
        let dc = gdi::WindowDc::get(core::ptr::null_mut());
        let old = gdi::Selected::new(dc.hdc(), normal);
        let tm = gdi::text_metrics(dc.hdc());
        drop(old);
        drop(dc);
        Fonts {
            normal,
            bold,
            height: tm.tmHeight,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // Owned GDI objects, no longer selected anywhere once replaced.
        gdi::delete_object(self.normal);
        gdi::delete_object(self.bold);
    }
}

/// Buffers reused by every row draw.
#[derive(Default)]
pub struct Scratch {
    pub folded: Vec<u16>,
    pub name_spans: Vec<Span>,
    pub parent_spans: Vec<Span>,
    pub text: Vec<u16>,
}

pub struct App {
    pub hwnd: HWND,
    pub edit: HWND,
    pub list: HWND,
    pub header: HWND,
    pub dpi: Cell<u32>,
    /// The result list's fonts, at View > Zoom's size: its rows, rename
    /// box and tips, and the header and the status bar while they have no
    /// font of their own (`fonts.rs`).
    pub fonts: RefCell<Fonts>,
    /// The window's own: Fonts and Colors' (Default), which Zoom leaves as
    /// it is; the search box's dropdown, and every other part while it has
    /// no font of its own.
    pub ui_fonts: RefCell<Fonts>,
    /// View > Zoom, in percent.
    zoom: Cell<u32>,
    /// Ctrl+wheel turns not yet a whole zoom step.
    wheel: Cell<i32>,
    /// View > Filter Bar, where it is, and whether its list is open.
    filter_bar: Cell<bool>,
    filter_rc: Cell<RECT>,
    filter_open: Cell<bool>,
    /// A menu is up (the menu bar's, or a popup's): no live update then.
    in_menu: Cell<bool>,
    /// File > Export under way, and the file type it last wrote.
    export: RefCell<Option<crate::export::Running>>,
    export_type: Cell<usize>,
    pub pal: Cell<Palette>,
    theme_mode: Cell<ThemeMode>,
    pub model: RefCell<Model>,
    pub icons: Icons,
    /// W9 (B1): the disk reader of the wave 9 columns, and the content
    /// search running.
    pub(crate) w9: w9a::W9,
    pub scratch: RefCell<Scratch>,
    pub search_rc: Cell<RECT>,
    pub status_rc: Cell<RECT>,
    status_visible: Cell<bool>,
    /// Where the status bar drew each indicator, for double-clicks.
    pub indicators: RefCell<Vec<(Indicator, RECT)>>,
    /// The result row under the mouse.
    pub hot_row: Cell<Option<usize>>,
    /// The header item under the mouse.
    pub hot_header: Cell<Option<usize>>,
    /// The search box button under the mouse, and the one held down.
    pub search_hot: Cell<Option<Button>>,
    pub search_pressed: Cell<Option<Button>>,
    /// The search box buttons' tooltips.
    pub tooltip: Cell<HWND>,
    /// The full text of a result cell cut off by its column.
    pub cell_tip: CellTip,
    /// The letters typed into the list to go to a name.
    type_ahead: RefCell<TypeAhead>,
    /// The search history dropdown.
    pub dropdown: Dropdown,
    /// The searches made, newest first (read from the data folder).
    pub search_history: RefCell<SearchHistory>,
    /// The search history's settings and file.
    searches: searches::Searches,
    /// The session's settings and file, and the previous session.
    sessions: session::Sessions,
    /// Back and Forward buttons left of the search box
    /// (`toolbar_nav_bar_visible`).
    nav_bar: Cell<bool>,
    /// The search box's text is being changed by the user's typing (a key,
    /// a paste, an undo), not by goz-gui: suggestions may open.
    typing: Cell<bool>,
    /// History > Back and Forward.
    nav: RefCell<NavHistory<nav::NavState>>,
    /// The search in the box is in the search history (since its text last
    /// changed).
    recorded: Cell<bool>,
    /// Enter came before the results: select the top one when they come.
    enter_pending: Cell<bool>,
    /// The files a refresh is to select again as their rows arrive.
    reselect: RefCell<Option<reselect::Reselect>>,
    /// The list's selection is being changed by goz-gui, not the user.
    restoring: Cell<bool>,
    /// Select All (Ctrl+A) chose every row, and the user has not chosen
    /// other rows since: a refresh, a later block or the tab shown again
    /// selects every row, however many there are (`reselect.rs`). A
    /// selection that only happens to cover every row (a click on the only
    /// row) is not this.
    select_all: Cell<bool>,
    /// Rows' states are being changed one by one (`in_bulk`): the list is
    /// given no names to measure, and whether it reported a change.
    bulk: Cell<bool>,
    bulk_changed: Cell<bool>,
    /// The list's selection as ranges, kept from its notifications
    /// (`selection.rs`): Invert Selection sets the smaller side.
    sel: RefCell<crate::selection::Selection>,
    /// Rows still to select or deselect one by one, a slice at a time
    /// (`selecting.rs`).
    selecting: RefCell<selecting::Selecting>,
    /// The search on screen counts as made (Enter, an action, the list
    /// taking the keyboard, or the text staying put for a moment), and
    /// whether its refusal by gozd has been beeped for.
    settled: Cell<bool>,
    beeped: Cell<bool>,
    /// The bookmarks, their file and settings.
    bookmarks: bookmarks::Marks,
    /// A bookmark dialog is open (it is modal, but commands can still be
    /// posted to the window).
    bookmark_dialog: Cell<bool>,
    /// The run history, its file and settings, and Enter's action.
    runs: runs::Runs,
    /// A note the status bar shows for a while instead of the counts, such
    /// as an action that took only the rows held of a selection.
    note: RefCell<Option<String>>,
    /// The title bar and taskbar icons made for the window's DPI.
    window_icons: Cell<[HICON; 2]>,
    menus: menu::Bar,
    on_top: Cell<OnTop>,
    opts: Cell<Options>,
    worker: Worker,
    next_epoch: Cell<u64>,
    want_status: Cell<bool>,
    /// gozd could not be reached while the window was hidden or
    /// minimized: the search runs again once it shows, not every 2 s
    /// meanwhile.
    retry_when_shown: Cell<bool>,
    last_focus: Cell<HWND>,
    last_search: Cell<Option<Instant>>,
    /// The settings file: as read, and what was written since.
    prefs: RefCell<prefs::Prefs>,
    /// The store thread, which writes every file goz-gui keeps.
    writer: Writer,
    /// The columns shown, their order and widths (96 DPI).
    layout: Cell<Layout>,
    /// The list's columns by index.
    list_cols: Cell<cols::ListCols>,
    /// A WM_COLUMNS is on its way.
    columns_posted: Cell<bool>,
    /// The settings say maximized but the window was shown without being
    /// activated, which cannot maximize it: kept as maximized until the
    /// user changes the window's state.
    maximize_deferred: Cell<bool>,
    /// The shell's part of the row menu, while it is up: it gets the
    /// menu's messages.
    shell_menu: RefCell<Option<Rc<shell::ContextMenu>>>,
    /// Delete, Paste and Rename's thread.
    fileops: FileOps,
    /// A drag of the results is under way: a drop on the window is ours.
    own_drag: Cell<bool>,
    /// The rename in progress (or asked for), and where its box goes.
    renaming: RefCell<Option<files::Renaming>>,
    rename_rect: Cell<RECT>,
    /// Where the list was last clicked (its coordinates): a slow second
    /// click renames only on the name.
    last_click: Cell<POINT>,
    /// The refreshes still to come after a change to the files.
    settle: Cell<u8>,
    /// The data objects of cuts being pasted, one per paste queued, to be
    /// told once their files are moved.
    cuts: RefCell<Vec<shell::Cut>>,
    /// An action on the selection waiting for its rows to be fetched.
    fetching: RefCell<Option<results::Fetching>>,
    /// The action that waited runs now: it fetches nothing again.
    fetch_bypass: Cell<bool>,
    /// The busy cursor, while goz-gui is busy for the user.
    busy: Cell<results::Busy>,
    /// Background, tray and hotkey settings, and whether the tray icon is
    /// up.
    background: Cell<crate::settings::Background>,
    tray_on: Cell<bool>,
    /// Explorer's "TaskbarCreated" message: the tray icon goes back.
    taskbar_created: u32,
    /// Details or thumbnails, and the thumbnails' size.
    view: Cell<View>,
    /// The thumbnails made, and the thread making them.
    pub thumbs: Thumbs,
    /// The system's icon lists (HIMAGELIST) by `SHIL_*`: 32, 16, 48 and
    /// 256 px (and 16 again, by the system's metrics), once asked.
    big_icons: Cell<[usize; 5]>,
    /// Their icons not loaded yet, loaded on a thread (`bigicons.rs`).
    big_loads: crate::bigicons::BigIcons,
    /// The icon view's layout while the list paints (taken as it starts).
    thumb_grid: Cell<Option<thumbview::Grid>>,
    /// The status bar's view button under the mouse.
    pub view_hot: Cell<Option<status::ViewButton>>,
    /// The panes registered and open, the docks' sizes, a divider being
    /// dragged.
    docks: RefCell<docks::Docks>,
    /// The tab strip and the find bar a feature put in (by `StripSlot`).
    strips: RefCell<[Option<Rc<dyn Strip>>; 2]>,
    /// The strip holding the mouse, which gets every mouse message.
    strip_capture: Cell<Option<StripSlot>>,
    /// Where the regions went in the last layout.
    regions: Cell<Regions>,
    /// The window was on screen when the panes were last told.
    panes_shown: Cell<bool>,
    /// Wave 4 (G2): the search box's font, the color filters, the UI
    /// scale's system DPI, the double-Ctrl hook, the accessible object
    /// (`custom.rs`, `access.rs`).
    custom: custom::Custom,
    // Wave 1: each feature adds the state it keeps on its own lines below
    // (and in `App::new`), so their changes never meet (FRAMEWORK.md).

    // F1 (preview pane).

    // F2 (sidebars).
    /// The Folders pane's folder and the search it scopes (`sidebars.rs`).
    sidebars: sidebars::Sidebars,
    // F3 (tabs and windows).
    /// The window's tabs (`tabs.rs`): the shown one's state is the
    /// window's own; the others are kept as views, and so are the tabs
    /// closed.
    tabs: RefCell<crate::tabs::TabList<crate::session::View>>,
    /// The list is not painting while a tab's rows come.
    list_frozen: Cell<bool>,
    /// Started by File > New Window (`-new-window`): no tray icon,
    /// hotkeys or hand-over; closing it ends it (`newwin.rs`).
    new_window: Cell<bool>,
    /// The `-instance` name and the instance's key, for the windows File >
    /// New Window starts and for Exit to reach them all; and this window's
    /// message-only window for Exit.
    instance: RefCell<Option<String>>,
    instance_key: RefCell<String>,
    exit_window: Cell<HWND>,
    // F4 (search tools).
    /// The find bar, the find under way, fullscreen.
    tools: tools::Tools,
    // F5 (file tools).
    /// The undo record, and the file tools' dialogs (`file_tools.rs`).
    file_tools: file_tools::Tools,
    // F6 (integration and the Options pages).
    /// Options' UI, Search, Results and View settings in force (the
    /// settings file's copy is `prefs.base.ui`; see `uiopts.rs`).
    ui: Cell<crate::ui_options::UiOptions>,
    /// The search box holds text typed and not searched yet (search as you
    /// type is off, or its delay runs).
    search_dirty: Cell<bool>,
    /// One click on a result opens it (`single_click_open`, resolved).
    single_click: Cell<bool>,
    /// A click is putting the keyboard in the search box: its text is
    /// selected once the button is up (`select_search_on_mouse_click`).
    click_select: Cell<bool>,
    // W4 (G1: data and file tools).
    /// The open file list, searched instead of gozd (`datatools.rs`).
    data_tools: datatools::DataTools,
}

/// How the program that started goz-gui asked for its window to be shown
/// (a shortcut's Run: Normal, Minimized, Maximized; a test's no-activate).
fn startup_show() -> i32 {
    match b1::startup_show_window() {
        Some(show) => i32::from(show),
        None => SW_SHOWNORMAL,
    }
}

/// How the window first shows: as its starter asked, maximized when it
/// was last closed so and the starter asked for a normal window. The
/// second value: the window is to be maximized but the starter asked for
/// it not to be activated, which maximizing would do; the third: it
/// starts minimized and restores to maximized.
fn first_show(start: i32, maximized: bool) -> (i32, bool, bool) {
    match start {
        SW_SHOWNORMAL | SW_SHOW | SW_SHOWDEFAULT | SW_RESTORE if maximized => {
            (SW_SHOWMAXIMIZED, false, false)
        }
        SW_SHOWNORMAL | SW_SHOW | SW_SHOWDEFAULT | SW_RESTORE => (SW_SHOWNORMAL, false, false),
        // Hidden, or shown without being activated: as asked; maximizing
        // would activate it.
        SW_SHOWNOACTIVATE | SW_SHOWNA | SW_HIDE => (start, maximized, false),
        SW_SHOWMINIMIZED | SW_SHOWMINNOACTIVE | SW_MINIMIZE | SW_FORCEMINIMIZE => {
            (start, false, maximized)
        }
        other => (other, false, false),
    }
}

/// The search a start's command line asks for, over the settings the
/// window starts with.
fn apply_launch_settings(s: &mut Settings, l: &crate::cmdline::Launch) {
    if let Some(text) = l.search_text() {
        s.search = text;
    }
    if let Some(f) = l.filter {
        s.filter = f;
    }
    if let Some(c) = l.match_case {
        s.match_case = c;
    }
    if let Some(p) = l.match_path {
        s.match_path = p;
    }
    if let Some(key) = l.sort {
        s.sort = SortSpec::default_for(key);
    }
    if let Some(dir) = l.dir {
        s.sort.dir = dir;
    }
    // F1 (preview pane): -preview / -no-preview.
    if let Some(show) = l.preview {
        let id = crate::dock::PaneId::Preview;
        let layout = s.docks.pane(id);
        s.docks.set_pane(
            id,
            crate::dock::PaneLayout {
                visible: show,
                ..layout
            },
        );
    }
    // `-x`, `-y`, `-width`, `-height` (File > New Window's): a normal
    // window there, kept on a monitor as a saved place is.
    if l.places_window() {
        let now = s.window.map(|w| (w.x, w.y, w.width, w.height));
        if let Some((x, y, width, height)) = l.place_over(now) {
            s.window = Some(crate::settings::WindowPos {
                x,
                y,
                width,
                height,
                dpi: None,
            });
            s.maximized = false;
        }
    }
}

fn prefs_background(app: &App) -> crate::settings::Background {
    app.prefs.borrow().base.background
}

/// Whether this start, with another goz-gui running on the data folder,
/// opens a window of its own (Options > UI's "Create a new window when
/// running goz-gui", `allow_multiple_windows`) rather than showing that
/// one. Never for a start that closes, toggles or hides goz-gui, nor with
/// `-no-new-window`. The settings file is only read here: the running
/// goz-gui owns it.
fn new_window_on_start(l: &crate::cmdline::Launch) -> bool {
    if l.exit
        || l.toggle_window
        || l.startup
        || l.new_window == Some(false)
        || (l.editor_only() && l.new_window.is_none())
    {
        return false;
    }
    let Some(path) = crate::store::location::data_file(crate::store::location::SETTINGS_FILE)
    else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    crate::store::ini::Ini::parse(&bytes).is_ok_and(|ini| {
        crate::ui_options::UiOptions::from_keys(|k| ini.get(k)).new_window_on_start
    })
}

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match crate::cmdline::parse(&args) {
        Ok(l) => l,
        Err(why) => {
            crate::msgbox::show(
                core::ptr::null_mut(),
                &format!(
                    "{why}

See Help > Search Syntax and the README for goz-gui's switches."
                ),
                "goz-gui",
                MB_OK | MB_ICONWARNING,
            );
            return 2;
        }
    };
    // W4 (G1): the switches that do their work with no window and end.
    if let Some(code) = datatools::before_window(&launch) {
        return code;
    }
    // One goz-gui per data folder (and -instance name): a second start
    // hands its command line over and ends, unless it is to be a window of
    // its own: File > New Window's (`-new-window`, see `window/newwin.rs`),
    // or any start with Options > UI's "Create a new window when running
    // goz-gui" (`allow_multiple_windows`).
    let key = crate::instance::key(launch.instance.as_deref());
    let first = crate::instance::claim(&key);
    let secondary = !first && (launch.new_window == Some(true) || new_window_on_start(&launch));
    // If the one holding the instance does not answer, start anyway. A
    // file list named relatively is made absolute here, as the running
    // goz-gui runs in another folder.
    let handed = crate::cmdline::absolute_file_lists(&args);
    if !first && !secondary && crate::instance::forward(&key, &handed) {
        return 0;
    }
    if launch.exit {
        // No first window to hand it to: the windows File > New Window
        // opened (if any) close.
        crate::instance::exit_others(&key, core::ptr::null_mut(), false);
        return 0;
    }
    let prefs = prefs::Prefs::load();
    // Wave 4 (G2): the UI scale for everything made from here on, and the
    // instance's own AppUserModelID before its first window.
    crate::custom::set_ui_dpi(prefs.base.custom.dpi);
    crate::jumplist::set_app_id(&key);
    let (searches, history) = searches::Searches::load(&prefs.base);
    let mut sessions = session::Sessions::load(&prefs.base);
    let marks = bookmarks::Marks::load(&prefs.base);
    let runs = runs::Runs::load(&prefs.base);
    // The window starts as the settings say, or as the previous session
    // left it (`session_store_restore_on_first_new_window`): only the
    // first window, as in Everything; not File > New Window's, nor one
    // that Options > UI's "Create a new window when running goz-gui"
    // opened beside a running goz-gui. Decided once, here: what reads the
    // session later (the Folders pane's folder) asks `sessions` again.
    if secondary || launch.new_window == Some(true) {
        sessions.not_first();
    }
    let restored = sessions.at_start();
    let mut settings = match &restored {
        Some(s) => session::start_settings(&prefs.base, s),
        // Home's match options that are always on or off apply too.
        None => prefs.base.home_start(),
    };
    apply_launch_settings(&mut settings, &launch);
    // A window of its own that was not told where to go (Options > UI's
    // "Create a new window when running goz-gui"; File > New Window passes
    // its place) opens down and right of the first one's saved place
    // (`offset_new_window_position`, `_x`, `_y`).
    let offset = settings.tabs.offset_new_window && !launch.places_window();
    if secondary
        && offset
        && let Some(w) = settings.window.as_mut()
    {
        let dpi = w.dpi.unwrap_or(96) as i32;
        w.x += settings.tabs.offset_x * dpi / 96;
        w.y += settings.tabs.offset_y * dpi / 96;
    }
    // Popup menus follow the theme from the first one on.
    theme::set_app_mode(settings.theme);
    // Where the window was, on the monitors there are now.
    let start: Option<Rect> = settings
        .window
        .and_then(|w| placement::fit(&w, &placement::monitors()));
    // The window class and windows are made and used on this thread only;
    // `App` is leaked, so the `&'static App` the window's handler and its
    // subclasses find stays valid until the process exits.
    Class {
        name: "goz-gui",
        // Double-clicks reach the status bar's indicators.
        style: CS_DBLCLKS,
        cursor: sys::system_cursor(IDC_ARROW),
        icon: app_icon(sys::metric(SM_CXICON)),
        icon_small: app_icon(sys::metric(SM_CXSMICON)),
        background: core::ptr::null_mut(),
    }
    .register();
    let menus = menu::menu_bar(prefs.base.history.run_enabled);
    // Made where it opens, so it gets that monitor's DPI from the start
    // and no DPI change rescales the size worked out for it.
    let rect = match start {
        Some(r) => (r.x, r.y, r.w, r.h),
        None => (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT),
    };
    // Its messages go to the `App` once it is attached (`APP`, set
    // right after), and to their default processing before.
    let hwnd = class::create(
        &Create {
            ex_style: 0,
            class: "goz-gui",
            title: "goz",
            style: WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            rect,
            parent: core::ptr::null_mut(),
            menu: menus.bar,
        },
        |m| match app_of(m.hwnd) {
            Some(app) => app.handle(m),
            None => m.def(),
        },
    );
    if hwnd.is_null() {
        return 1;
    }
    let app: &'static App = Box::leak(App::create(
        hwnd,
        menus,
        prefs,
        &settings,
        (searches, history),
        (sessions, marks, runs),
    ));
    APP.set(Some(app));
    // W8: GOZ_GUI_TRACE's timeline of this thread (a test hook).
    crate::trace::start();
    // F3: which window of the instance this is (before the tray and
    // hotkeys, which only the first has).
    app.new_window.set(secondary);
    *app.instance.borrow_mut() = launch.instance.clone();
    *app.instance_key.borrow_mut() = key.clone();
    app.init(start.is_none());
    // A new window leaves the starts handed over, the tray icon and
    // the hotkeys to the first goz-gui.
    if !secondary {
        crate::instance::listen(&key, hwnd, WM_LAUNCH);
    }
    app.listen_for_exit();
    let taken = app.apply_background(prefs_background(app));
    // -startup (signing in to Windows) starts in the background, with
    // no window; not a new window, which has no tray icon to come back
    // from.
    // W6: -edit or -filelist-editor alone shows the editor, not the
    // window.
    let editor_only = launch.editor_only() && !secondary;
    let asked = if (launch.startup || editor_only) && !secondary {
        SW_HIDE
    } else {
        startup_show()
    };
    let (show, deferred, min_to_max) = first_show(asked, settings.maximized);
    app.maximize_deferred.set(deferred);
    if min_to_max {
        let mut wp = win::placement(hwnd).unwrap_or_default();
        wp.flags = WPF_RESTORETOMAXIMIZED;
        wp.showCmd = show as u32;
        win::set_placement(hwnd, &wp);
    } else {
        win::show(hwnd, show);
    }
    // W4 (G1): -filelist (or a bare .efu): the first search is the
    // list's. W5: else the previous session's shown tab's list, if it
    // searched one.
    if let Some(list) = &launch.filelist {
        app.set_file_list(list);
    } else if let Some(list) = restored.as_ref().and_then(|s| s.view().file_list.clone()) {
        app.start_with_file_list(&list);
    }
    // W6: the shown tab's file-list slots, before its first search.
    if let Some(s) = &restored {
        app.show_tab_slots(&s.view().slots);
    }
    app.start_search(false);
    // W6: -edit and -filelist-editor open the File List Editor; alone
    // (nothing else asked), goz-gui's window is not shown and ends
    // with it.
    if launch.edit.is_some() || launch.filelist_editor {
        app.start_list_editor(launch.edit.as_deref(), editor_only);
    }
    if let Some(s) = &restored {
        // Its tabs, the shown one's navigation entries, and its place
        // in the results once they come.
        app.start_tabs(s);
    }
    // The search the window starts with is the first navigation entry,
    // as in Everything.
    app.commit_nav();
    app.session_timer();
    // What could not be read, said once the window is up.
    let notes: Vec<String> = [
        app.prefs.borrow_mut().note.take(),
        app.searches.note.borrow_mut().take(),
        app.sessions.note.borrow_mut().take(),
        app.bookmarks.note.borrow_mut().take(),
        app.runs.note.borrow_mut().take(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !notes.is_empty() {
        app.show_note(notes.join("; "));
    }
    if !taken.is_empty() {
        app.show_note(format!(
            "Another program already uses the {} hotkey",
            taken.join(" and ")
        ));
    }

    while let Some(q) = queue::get(core::ptr::null_mut()) {
        let msg = q.msg();
        dropdown::click_elsewhere(app, &msg);
        // A pane or strip takes the keys its controls keep first.
        if matches!(
            msg.message,
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_CHAR | WM_SYSCHAR
        ) && app.frame_key(&msg)
        {
            continue;
        }
        // Shortcuts are the main window's; another window of the thread
        // (Help > Search Syntax) keeps its keys.
        let ours = msg.hwnd == hwnd || win::is_child(hwnd, msg.hwnd);
        // Esc stops an export first, wherever the keyboard is; W9: then
        // a content search reading.
        if ours
            && msg.message == WM_KEYDOWN
            && msg.wParam == usize::from(VK_ESCAPE)
            && (app.cancel_export() || app.cancel_content())
        {
            continue;
        }
        if ours && queue::translate_accelerator(hwnd, keys::table(), &q) {
            continue;
        }
        // Tab and Shift+Tab go round the tab stops: the search box, the
        // panes that are tab stops, the results (a pane or strip may
        // keep it among its own controls); so does Ctrl+Tab while no
        // shortcut above takes it.
        if msg.message == WM_KEYDOWN
            && msg.wParam == usize::from(VK_TAB)
            && !key_down(VK_MENU)
            && app.tab_key(msg.hwnd, key_down(VK_SHIFT))
        {
            continue;
        }
        queue::translate(&q);
        queue::dispatch(&q);
    }
    crate::trace::finish();
    // W5: the File List Editor is asked to close (it asks about unsaved
    // changes), and goz-gui ends once it has.
    crate::listedit::ui::finish();
    // A copy or move still running finishes first, and so do the
    // opens handed to the shell (Close window on execute closes right
    // after one).
    app.fileops.finish();
    crate::actions::wait_for_launches(Duration::from_secs(10));
    shell::ole_done();
    // WM_QUIT's code: the only one this thread posts is WM_DESTROY's 0.
    0
}

thread_local! {
    /// The window's `App`, once it is attached.
    static APP: Cell<Option<&'static App>> = const { Cell::new(None) };
}

/// The `App` of the main window `main` (a pane's parent), once it is
/// attached.
pub fn app_of(main: HWND) -> Option<&'static App> {
    APP.get().filter(|app| !main.is_null() && app.hwnd == main)
}

/// goz-gui's icon, resource 1 (build.rs embeds it), at `size` pixels; null
/// if the build has none.
pub fn app_icon(size: i32) -> HICON {
    // A resource of this module by its id; a missing one is reported as
    // null, which callers accept.
    b1::module_icon(1, size)
}

// `wide::is_wide` had its one caller here (WM_SETTINGCHANGE, which reads
// its setting with `Msg::setting_is` now). Named, not called, until it goes
// from `wide.rs`, so the build stays free of warnings meanwhile.
const _: unsafe fn(*const u16, &str) -> bool = crate::wide::is_wide;

fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

fn key_down(vk: u16) -> bool {
    sys::key_down(vk)
}

/// The client point a mouse message carries.
fn point(l: LPARAM) -> POINT {
    POINT {
        x: (l & 0xffff) as i16 as i32,
        y: ((l >> 16) & 0xffff) as i16 as i32,
    }
}

fn in_rect(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

impl App {
    /// The window's state, made from `s`, the settings it starts with.
    fn create(
        hwnd: HWND,
        menus: menu::Bar,
        prefs: prefs::Prefs,
        s: &Settings,
        (searches, history): (searches::Searches, SearchHistory),
        (sessions, marks, runs): (session::Sessions, bookmarks::Marks, runs::Runs),
    ) -> Box<App> {
        // The child controls of `hwnd`, on its thread.
        let edit = class::create_plain(&Create {
            ex_style: 0,
            class: "EDIT",
            title: "",
            style: WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            rect: (0, 0, 0, 0),
            parent: hwnd,
            menu: class::id_menu(ID_EDIT),
        });
        let list = class::create_plain(&Create {
            ex_style: 0,
            class: lv::CLASS,
            title: "",
            style: WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | LVS_REPORT
                | LVS_OWNERDATA
                | LVS_OWNERDRAWFIXED
                | LVS_SHOWSELALWAYS
                | LVS_EDITLABELS,
            rect: (0, 0, 0, 0),
            parent: hwnd,
            menu: class::id_menu(ID_LIST),
        });
        let header = lv::header(list);
        // The UI scale (`dpi`, Options > UI) multiplies the monitor's.
        let system_dpi = sys::dpi_for_window(hwnd);
        let dpi = crate::custom::scaled(system_dpi);
        let mode = s.theme;
        // The search the window starts with (Home's, or the last one),
        // on one line as the box shows it (a session may hold one of
        // several lines). The window's procedure ignores the change
        // this makes: the App is not attached to it yet.
        if !s.search.is_empty() {
            let search = crate::bookmarks::one_line(&s.search);
            win::set_text(edit, &search);
            let end = search.encode_utf16().count();
            win::edit::set_sel(edit, end as i32, end as i32);
        }
        let opts = Options {
            match_case: s.match_case,
            match_path: s.match_path,
            filter: s.filter,
            sort: s.sort,
        };
        let (status_bar, on_top, layout) = (s.status_bar, s.on_top, s.columns);
        Box::new(App {
            hwnd,
            edit,
            list,
            header,
            dpi: Cell::new(dpi),
            fonts: RefCell::new(Fonts::with(
                dpi,
                s.zoom,
                &s.custom
                    .colors
                    .resolved_font(crate::colors::Item::ResultList),
            )),
            ui_fonts: RefCell::new(Fonts::with(
                dpi,
                crate::zoom::DEFAULT,
                &s.custom.colors.resolved_font(crate::colors::Item::Default),
            )),
            zoom: Cell::new(s.zoom),
            wheel: Cell::new(0),
            filter_bar: Cell::new(s.filter_bar),
            filter_rc: Cell::new(RECT::default()),
            filter_open: Cell::new(false),
            in_menu: Cell::new(false),
            export: RefCell::new(None),
            export_type: Cell::new(s.export_type),
            pal: Cell::new(Palette::new(mode.is_dark())),
            theme_mode: Cell::new(mode),
            model: RefCell::new(Model::new(Arc::new(Search {
                query: String::new(),
                sort: SortSpec::default(),
                match_case: false,
                filelist: None,
                created_sort: None,
                slots: Default::default(),
                want: Default::default(),
                content: None,
            }))),
            icons: Icons::new(hwnd, WM_ICON, WM_TYPES, WM_TYPES_ASK),
            w9: w9a::W9::new(hwnd),
            scratch: RefCell::new(Scratch::default()),
            search_rc: Cell::new(RECT::default()),
            status_rc: Cell::new(RECT::default()),
            status_visible: Cell::new(status_bar),
            indicators: RefCell::new(Vec::new()),
            hot_row: Cell::new(None),
            hot_header: Cell::new(None),
            search_hot: Cell::new(None),
            search_pressed: Cell::new(None),
            tooltip: Cell::new(core::ptr::null_mut()),
            cell_tip: CellTip::default(),
            type_ahead: RefCell::new(TypeAhead::default()),
            dropdown: Dropdown::new(),
            search_history: RefCell::new(history),
            searches,
            sessions,
            nav_bar: Cell::new(s.nav_bar),
            typing: Cell::new(false),
            nav: RefCell::new(NavHistory::default()),
            recorded: Cell::new(false),
            enter_pending: Cell::new(false),
            reselect: RefCell::new(None),
            restoring: Cell::new(false),
            select_all: Cell::new(false),
            bulk: Cell::new(false),
            bulk_changed: Cell::new(false),
            sel: RefCell::default(),
            selecting: RefCell::default(),
            settled: Cell::new(false),
            beeped: Cell::new(false),
            bookmarks: marks,
            bookmark_dialog: Cell::new(false),
            runs,
            note: RefCell::new(None),
            window_icons: Cell::new([core::ptr::null_mut(); 2]),
            menus,
            on_top: Cell::new(on_top),
            opts: Cell::new(opts),
            worker: Worker::spawn(hwnd, WM_REPLY),
            next_epoch: Cell::new(0),
            want_status: Cell::new(false),
            retry_when_shown: Cell::new(false),
            last_focus: Cell::new(edit),
            last_search: Cell::new(None),
            prefs: RefCell::new(prefs),
            writer: prefs::writer(),
            layout: Cell::new(layout),
            list_cols: Cell::new(cols::ListCols::default()),
            columns_posted: Cell::new(false),
            maximize_deferred: Cell::new(false),
            shell_menu: RefCell::new(None),
            fileops: FileOps::new(hwnd, WM_FILEOP),
            own_drag: Cell::new(false),
            renaming: RefCell::new(None),
            rename_rect: Cell::new(RECT::default()),
            last_click: Cell::new(POINT::default()),
            settle: Cell::new(0),
            cuts: RefCell::new(Vec::new()),
            fetching: RefCell::new(None),
            fetch_bypass: Cell::new(false),
            busy: Cell::new(results::Busy::Idle),
            background: Cell::new(crate::settings::Background::default()),
            tray_on: Cell::new(false),
            taskbar_created: crate::tray::taskbar_created(),
            view: Cell::new(s.view),
            thumbs: Thumbs::new(hwnd, WM_THUMB),
            big_icons: Cell::new([0; 5]),
            big_loads: crate::bigicons::BigIcons::new(hwnd, thumbview::WM_BIG_ICONS),
            thumb_grid: Cell::new(None),
            view_hot: Cell::new(None),
            docks: RefCell::new(docks::Docks::new(s.docks)),
            strips: RefCell::new([None, None]),
            strip_capture: Cell::new(None),
            regions: Cell::new(Regions::default()),
            panes_shown: Cell::new(false),
            custom: custom::Custom::new(&s.custom, system_dpi),
            // Wave 1 (FRAMEWORK.md).

            // F1 (preview pane).

            // F2 (sidebars).
            sidebars: sidebars::Sidebars::new(),
            // F3 (tabs and windows).
            tabs: RefCell::new(crate::tabs::TabList::new(
                crate::session::View::default(),
                s.tabs.max_closed,
            )),
            list_frozen: Cell::new(false),
            new_window: Cell::new(false),
            instance: RefCell::new(None),
            instance_key: RefCell::new(String::new()),
            exit_window: Cell::new(core::ptr::null_mut()),
            // F4 (search tools).
            tools: tools::Tools::new(),
            // F5 (file tools).
            file_tools: file_tools::Tools::new(&s.file_tools),
            // F6 (integration and the Options pages).
            ui: Cell::new(s.ui),
            search_dirty: Cell::new(false),
            single_click: Cell::new(false),
            click_select: Cell::new(false),
            // W4 (G1: data and file tools).
            data_tools: datatools::DataTools::new(),
        })
    }

    /// Readies the window's controls; `default_size` sizes the window as
    /// goz-gui does when it has no saved place.
    fn init(&'static self, default_size: bool) {
        // `self` is leaked, so the subclasses may hold it.
        subclass::set(self.edit, &edit::SUBCLASS, move |m| {
            edit::edit_proc(self, m)
        });
        subclass::set(self.list, &LIST_SUBCLASS, move |m| list_proc(self, m));
        subclass::set(self.header, &header::SUBCLASS, move |m| {
            header::subclass_proc(self, m)
        });
        // Files dropped on the window or the search box become the search.
        b1::drag_accept_files(self.hwnd, true);
        b1::drag_accept_files(self.edit, true);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP;
        lv::set_extended_style(self.list, ex, ex);
        // Full row select and one-click opening as Options > Results says,
        // sizes as Options > View says.
        self.apply_list_style();
        crate::format::set_size_format(self.ui.get().size_format);
        self.apply_columns(self.layout.get());
        self.heal_column_widths();
        // The dialogs open where they were; one that moved asks for the
        // settings to be written.
        let places = self.prefs.borrow().base.places.clone();
        places::start(places, Box::new(move || self.settings_changed()));
        win::edit::set_cue_banner(self.edit, "Search", true);
        dropdown::create(self);
        self.tooltip.set(searchbox::create_tooltip(self));
        celltip::create(self);
        // Wave 6: the parts with a font of their own (`fonts.rs`).
        self.make_part_fonts();
        self.apply_fonts();
        theme::set_app_mode(self.theme_mode.get());
        self.apply_theme();
        self.set_window_icons();
        self.show_sort();
        if self.view.get().thumbnails {
            self.put_view();
        }
        // The panes this build has (and the test hook's), opened as the
        // settings left them.
        self.register_panes();
        // Wave 4 (G2): the keys in force, the double-Ctrl hook, the
        // taskbar.
        self.start_custom();
        if default_size {
            // Sizes the window for its monitor's DPI before it shows.
            win::set_pos(
                self.hwnd,
                core::ptr::null_mut(),
                (0, 0, self.scale(1000), self.scale(640)),
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.layout();
        // The start search's caret was put at its end while the box had no
        // width, which scrolled the text out of it: from the start again,
        // then the caret at the end, scrolling only if the text is longer
        // than the box.
        let len = win::text_len(self.edit);
        if len > 0 {
            win::edit::set_sel(self.edit, 0, 0);
            win::edit::set_sel(self.edit, len as i32, len as i32);
        }
        self.arm_live();
    }

    pub fn scale(&self, v: i32) -> i32 {
        v * self.dpi.get() as i32 / 96
    }

    /// `v` pixels at 96 DPI scaled for the DPI and View > Zoom: the result
    /// list's measures.
    pub fn zscale(&self, v: i32) -> i32 {
        crate::zoom::scale(v, self.dpi.get(), self.zoom.get())
    }

    /// The window's own font: Fonts and Colors' (Default) (Zoom leaves it
    /// as it is). A part with an item of its own draws in
    /// [`App::part_font`].
    pub fn ui_font(&self) -> HFONT {
        self.ui_fonts.borrow().normal
    }

    /// A result row: 19 px at 100 % with the usual 9 pt message font, as
    /// in Everything 1.5; never shorter than the row's icon.
    pub fn row_height(&self) -> i32 {
        let text = self.fonts.borrow().height + self.zscale(4);
        let icon = sys::metric_for_dpi(SM_CYSMICON, self.dpi.get());
        text.max(self.zscale(19)).max(icon + self.scale(1))
    }

    /// The header, its top and bottom lines included: 27 px at 100 % (its
    /// font's height and 12 px).
    pub fn header_height(&self) -> i32 {
        self.part_height(crate::colors::Item::Header) + self.zscale(12)
    }

    fn apply_fonts(&self) {
        let ui = self.search_font();
        win::set_font(self.edit, ui, true);
        let font = self.fonts.borrow().normal;
        win::set_font(self.list, font, true);
        celltip::set_font(self);
    }

    /// The title bar and taskbar icons at the sizes this DPI wants, which
    /// the class's icons (made for the primary monitor) may not be.
    fn set_window_icons(&self) {
        // The monitor's own: the title bar and the taskbar are Windows'.
        let dpi = self.system_dpi();
        let old = self.window_icons.get();
        // The icons are this module's resource.
        let new = [
            app_icon(sys::metric_for_dpi(SM_CXSMICON, dpi)),
            app_icon(sys::metric_for_dpi(SM_CXICON, dpi)),
        ];
        if new.iter().any(|i| i.is_null()) {
            // Icons made above and given to no one.
            for icon in new.into_iter().filter(|i| !i.is_null()) {
                gdi::destroy_icon(icon);
            }
            return;
        }
        win::send_plain(self.hwnd, WM_SETICON, ICON_SMALL as usize, new[0] as isize);
        win::send_plain(self.hwnd, WM_SETICON, ICON_BIG as usize, new[1] as isize);
        self.window_icons.set(new);
        // Icons this window no longer uses, made by this function.
        for icon in old.into_iter().filter(|i| !i.is_null()) {
            gdi::destroy_icon(icon);
        }
    }

    pub fn theme_mode(&self) -> ThemeMode {
        self.theme_mode.get()
    }

    /// Follows the system's theme, or forces light or dark.
    pub fn set_theme_mode(&self, mode: ThemeMode) {
        self.theme_mode.set(mode);
        theme::set_app_mode(mode);
        self.apply_theme();
        // W6: the File List Editor, a window of its own thread, follows.
        crate::listedit::ui::set_look(self.dialog_look());
        self.settings_changed();
    }

    pub fn status_bar_visible(&self) -> bool {
        self.status_visible.get()
    }

    /// Whether Back and Forward show left of the search box.
    pub fn nav_bar_visible(&self) -> bool {
        self.nav_bar.get()
    }

    /// Shows or hides Back and Forward left of the search box.
    pub fn set_nav_bar_visible(&self, visible: bool) {
        self.nav_bar.set(visible);
        self.layout();
        // Repaints this thread's window.
        win::invalidate(self.hwnd, None, true);
    }

    /// Shows or hides the status bar; the list takes its room.
    pub fn set_status_bar_visible(&self, visible: bool) {
        self.status_visible.set(visible);
        self.layout();
        self.settings_changed();
    }

    fn apply_theme(&self) {
        let dark = self.theme_mode.get().is_dark();
        // With Options > Fonts and Colors' result list colours.
        let pal = self.custom_palette(dark);
        self.pal.set(pal);
        theme::set_titlebar(self.hwnd, dark);
        for h in [self.list, self.header, self.edit] {
            theme::allow(h, dark);
        }
        theme::set_class(
            self.list,
            Some(if dark {
                "DarkMode_Explorer"
            } else {
                "Explorer"
            }),
        );
        theme::set_class(
            self.header,
            Some(if dark {
                "DarkMode_ItemsView"
            } else {
                "ItemsView"
            }),
        );
        theme::set_class(self.edit, dark.then_some("DarkMode_CFD"));
        // The search box's style (Fonts and Colors' Search Edit) is the
        // theme's own.
        let current: HFONT = win::font(self.edit);
        if self.search_font() != current {
            win::set_font(self.edit, self.search_font(), true);
        }
        lv::set_bk_color(self.list, pal.list_bg);
        lv::set_text_bk_color(self.list, pal.list_bg);
        lv::set_text_color(self.list, pal.list_fg);
        syntax::apply_theme(dark);
        dialog::set_app_look(self.dialog_look());
        dropdown::apply_theme(self);
        searchbox::theme_tooltip(self, dark);
        celltip::set_theme(self, dark);
        self.docks_theme(dark);
        theme::flush_menus();
        // Repaints this thread's windows.
        b1::draw_menu_bar(self.hwnd);
        win::redraw(
            self.hwnd,
            None,
            RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
        );
    }

    /// Lays the window's regions out (`crate::layout`): the tab strip, the
    /// search row, the docks with their panes either side of the results
    /// and the find bar, and the status bar. At 100 % with the usual 9 pt
    /// message font (15 px) the heights are Everything 1.5's, measured on
    /// its window: 5 px above the search frame, the frame 23 px, 3 px below
    /// it, then the list (header 27 px, rows 19 px), and a 23 px status
    /// bar; 3 px either side of the frame. The nav bar, when shown, takes
    /// two 23 px squares and a 3 px gap left of the frame. The docks are
    /// `left_sidebar_wide` and `right_sidebar_wide` (256 px) with a 6 px
    /// divider each, as Everything's.
    fn layout(&self) {
        let rc = win::client_rect(self.hwnd);
        let (tabs_h, find_h) = self.strip_heights();
        // Under the tab strip, Everything's 3 px above the frame: the 2 px
        // more under the menu bar make up for Everything's taller one, which
        // the strip does when it is there (`tabstrip.rs`).
        let above = self.scale(if tabs_h > 0 { 3 } else { 5 });
        let (side, below) = (self.scale(3), self.scale(3));
        // The search row holds the search box's text and the filter bar's,
        // each in its own font, and is never lower than the window font's.
        let edit_h = self.search_font_height();
        let mut fh = self.ui_fonts.borrow().height.max(edit_h);
        if self.filter_bar.get() {
            fh = fh.max(self.part_height(crate::colors::Item::FilterBar));
        }
        let search_h = fh + self.scale(8);
        // The status bar in its own font, which zooms with the list's.
        let status_h = if self.status_visible.get() {
            self.part_height(crate::colors::Item::StatusBar) + self.zscale(8)
        } else {
            0
        };
        let (left, right) = self.dock_widths();
        let (min_dock, min_center) = self.dock_limits();
        let r = crate::layout::regions(&crate::layout::Inputs {
            width: rc.right,
            height: rc.bottom,
            tabs: tabs_h,
            search: (above, search_h, below),
            status: status_h,
            find: find_h,
            left,
            right,
            divider: self.scale(crate::dock::DIVIDER),
            min_dock,
            min_center,
        });
        self.regions.set(r);
        let nav = if self.nav_bar.get() {
            searchbox::nav_room(search_h, self.scale(searchbox::NAV_GAP))
        } else {
            0
        };
        // The filter bar, `filters_wide` (128 px at 100 %) and a gap, right
        // of the search frame, as long as the frame keeps some room.
        let filter_w = if self.filter_bar.get() {
            let w = self.scale(bar::FILTER_BAR_WIDE);
            let gap = self.scale(searchbox::NAV_GAP);
            let room = rc.right - 2 * side - nav;
            if room - w - gap >= self.scale(64) {
                w + gap
            } else {
                0
            }
        } else {
            0
        };
        let top = r.search.top + above;
        let search = RECT {
            left: side + nav,
            top,
            right: (rc.right - side - filter_w).max(side + nav),
            bottom: top + search_h,
        };
        self.filter_rc.set(if filter_w > 0 {
            RECT {
                left: rc.right - side - self.scale(bar::FILTER_BAR_WIDE),
                right: rc.right - side,
                ..search
            }
        } else {
            RECT::default()
        });
        self.search_rc.set(search);
        self.status_rc.set(r.status);
        let inset = self.scale(7);
        // The text stops short of the clear and history buttons.
        let edit_right = searchbox::frame_of(self).edit_right;
        searchbox::place_tooltip(self);
        // The search box, the results and the panes move together, so
        // dragging a divider repaints them once and in step.
        let mut moves = docks::Defer::new();
        let edit_left = search.left + inset;
        // The box as high as its own text, in the middle of the row.
        let edit_top = search.top + (search_h - edit_h) / 2;
        moves.put(
            self.edit,
            RECT {
                left: edit_left,
                top: edit_top,
                right: edit_left + (edit_right - edit_left).max(0),
                bottom: edit_top + edit_h,
            },
        );
        moves.put(self.list, r.list);
        self.place_panes(&r, &mut moves);
        moves.run();
        for slot in [StripSlot::Tabs, StripSlot::FindBar] {
            if let Some(strip) = self.strip(slot) {
                strip.place(self, self.strip_rect(slot));
            }
        }
        // Repaints this thread's window (its children paint themselves).
        win::invalidate(self.hwnd, None, false);
    }

    /// The main window's messages, from the moment the `App` is attached.
    fn handle(&self, m: &Msg) -> LRESULT {
        let (msg, w, l) = (m.id, m.w, m.l);
        // The shell's menu fills and draws its own items while it is up.
        if matches!(
            msg,
            WM_INITMENUPOPUP | WM_MEASUREITEM | WM_DRAWITEM | WM_MENUCHAR
        ) && let Some(answer) = self.shell_menu_message(msg, w, l)
            && msg != WM_INITMENUPOPUP
        {
            return answer;
        }
        // What a strip's own controls send their parent (this window)
        // goes to the strip.
        if matches!(
            msg,
            WM_COMMAND
                | WM_NOTIFY
                | WM_CTLCOLOREDIT
                | WM_CTLCOLORSTATIC
                | WM_CTLCOLORBTN
                | WM_CTLCOLORLISTBOX
                | WM_DRAWITEM
        ) && let Some(answer) = self.strip_message(msg, w, l)
        {
            return answer;
        }
        // The strips see the mouse over them first (and all of it
        // while one holds it); a button they take goes no further.
        if matches!(
            msg,
            WM_MOUSEMOVE
                | WM_MOUSELEAVE
                | WM_CAPTURECHANGED
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_LBUTTONDBLCLK
                | WM_RBUTTONDOWN
                | WM_RBUTTONUP
                | WM_MBUTTONDOWN
                | WM_MBUTTONUP
        ) && self.strip_mouse(msg, w, point(l))
        {
            return 0;
        }
        match msg {
            WM_SIZE => {
                // The dropdown hangs under the search frame where it was.
                dropdown::close(self);
                self.layout();
                if w == SIZE_MAXIMIZED as usize {
                    // The user maximized it: the state is the window's.
                    self.maximize_deferred.set(false);
                }
                if w != SIZE_MINIMIZED as usize {
                    self.arm_live();
                    self.retry_if_waiting();
                }
                self.panes_window_shown(w != SIZE_MINIMIZED as usize && win::is_visible(self.hwnd));
                self.settings_changed();
                0
            }
            WM_SHOWWINDOW => {
                if w != 0 {
                    self.arm_live();
                    self.retry_if_waiting();
                }
                self.panes_window_shown(w != 0 && !win::is_iconic(self.hwnd));
                m.def()
            }
            WM_MOVE => {
                dropdown::close(self);
                self.settings_changed();
                m.def()
            }
            WM_ENTERMENULOOP => {
                dropdown::close(self);
                self.in_menu.set(true);
                m.def()
            }
            WM_EXITMENULOOP => {
                self.in_menu.set(false);
                m.def()
            }
            WM_PAINT => {
                paint::window(self);
                0
            }
            WM_ERASEBKGND => 1,
            WM_CTLCOLOREDIT => {
                let pal = self.pal.get();
                // The search box in Fonts and Colors' Search Edit.
                let ink = if l as HWND == self.edit {
                    pal.ink(
                        crate::colors::Item::SearchEdit,
                        crate::colors::State::Normal,
                    )
                } else {
                    crate::theme::Ink {
                        fg: pal.fg,
                        bg: pal.bg,
                    }
                };
                let hdc = w as windows_sys::Win32::Graphics::Gdi::HDC;
                gdi::set_text_color(hdc, ink.fg);
                gdi::set_bk_color(hdc, ink.bg);
                // The edit erases with the brush returned, on its own DC.
                gdi::set_dc_brush_color(hdc, ink.bg);
                gdi::stock_object(DC_BRUSH) as LRESULT
            }
            WM_MOUSEMOVE => {
                // A divider being dragged takes the mouse.
                if !self.divider_move(point(l)) {
                    searchbox::mouse_move(self, point(l));
                    status::view_hover(self, Some(point(l)));
                }
                0
            }
            WM_MOUSELEAVE => {
                searchbox::mouse_leave(self);
                status::view_hover(self, None);
                0
            }
            WM_LBUTTONDOWN => {
                let pt = point(l);
                if self.divider_down(pt) {
                    // A divider's drag starts.
                } else if in_rect(&self.filter_rc.get(), pt) {
                    self.filter_bar_click();
                } else if let Some(button) = status::view_button_at(self, pt) {
                    self.view_button(button);
                } else if !searchbox::button_down(self, pt) && in_rect(&self.search_rc.get(), pt) {
                    // The frame round the text is the search box too,
                    // its text selected as a click in it would.
                    let had = win::focus() == self.edit;
                    self.focus_box();
                    if !had && self.ui.get().select_on_click && win::focus() == self.edit {
                        win::edit::select_all(self.edit);
                    }
                }
                0
            }
            WM_LBUTTONUP => {
                if !self.divider_up()
                    && let Some(button) = searchbox::button_up(self, point(l))
                {
                    self.search_button(button);
                }
                0
            }
            WM_CAPTURECHANGED => {
                self.divider_up();
                searchbox::capture_lost(self);
                0
            }
            WM_LBUTTONDBLCLK => {
                let pt = point(l);
                // A quick second click on a button is a click.
                if let Some(button) = status::view_button_at(self, pt) {
                    self.view_button(button);
                } else if !searchbox::button_down(self, pt)
                    && let Some(indicator) = status::indicator_at(self, pt)
                {
                    self.turn_off(indicator);
                }
                0
            }
            // Browser Back, Forward and Home keys, and the mouse's side
            // buttons, which DefWindowProc turns into these.
            WM_APPCOMMAND => {
                match u32::from(hiword(l as usize)) & !FAPPCOMMAND_MASK {
                    APPCOMMAND_BROWSER_BACKWARD => self.go_back(),
                    APPCOMMAND_BROWSER_FORWARD => self.go_forward(),
                    APPCOMMAND_BROWSER_HOME => self.go_home(),
                    _ => return m.def(),
                }
                1
            }
            WM_MEASUREITEM => {
                if let Some(mut mis) = m.measure_item()
                    && mis.CtlType == ODT_LISTVIEW
                {
                    mis.itemHeight = self.row_height() as u32;
                }
                1
            }
            WM_DRAWITEM => {
                if let Some(dis) = m.draw_item()
                    && dis.CtlType == ODT_LISTVIEW
                {
                    paint::row(self, &dis);
                }
                1
            }
            WM_NOTIFY => self.notify(m),
            WM_COMMAND => {
                if l != 0 {
                    if loword(w) as usize == ID_EDIT && u32::from(hiword(w)) == EN_CHANGE {
                        self.text_changed();
                    }
                } else {
                    self.menu_command(loword(w));
                }
                0
            }
            WM_CONTEXTMENU => {
                if (w as HWND == self.list || w as HWND == self.header)
                    && l != -1
                    && self.over_header(point(l))
                {
                    self.header_menu(point(l), header::item_at_screen(self, point(l)));
                    0
                } else if w as HWND == self.list {
                    self.list_menu(l);
                    0
                } else if w as HWND == self.hwnd && self.status_menu(l) {
                    // The status bar's menu (F2).
                    0
                } else {
                    m.def()
                }
            }
            // The window menu (HIWORD of lParam) is the system's own;
            // the shell's submenus are the shell's (above).
            WM_INITMENUPOPUP => {
                if hiword(l as usize) == 0 && self.shell_menu.borrow().is_none() {
                    self.init_menu_popup(w as HMENU);
                    // Options > Keyboard's keys after the items' tabs.
                    self.relabel_menu(w as HMENU);
                }
                0
            }
            WM_SETCURSOR if u32::from(loword(l as usize)) == HTCLIENT && self.busy_cursor() => 1,
            // Over a divider: the resize cursor.
            WM_SETCURSOR
                if w as HWND == self.hwnd
                    && u32::from(loword(l as usize)) == HTCLIENT
                    && self.divider_cursor() =>
            {
                1
            }
            results::WM_FETCHED => {
                self.fetch_done();
                0
            }
            selecting::WM_SELECTING => {
                self.selecting_due();
                0
            }
            // W4 (G1): the data tools' messages.
            id if (crate::datatools::MSG_FIRST..crate::datatools::MSG_END).contains(&id) => {
                self.data_tools_message(id, w, l);
                0
            }
            // W6 (C): the File List Editor closed.
            id if (crate::listedit::WM_EDITOR_CLOSED..crate::listedit::W6_MSG_END)
                .contains(&id) =>
            {
                self.lists_message(id, w, l);
                0
            }
            // W7 (C): a file-list term to edit, a slot's saved results.
            id if (crate::slotedit::MSG_FIRST..crate::slotedit::MSG_END).contains(&id) => {
                self.w7_message(id, w, l);
                0
            }
            // W9 (B1): the disk's column values, the content search.
            id if w9a::is_message(id) => {
                self.w9_message(id, w, l);
                0
            }
            WM_SERVICE => {
                if w != 0 {
                    // gozd stopped or (re)started: search again, which
                    // reconnects, or retries until it is up.
                    self.start_search(true);
                }
                0
            }
            WM_TIMER => {
                win::kill_timer(self.hwnd, w);
                if w == TIMER_RETRY && !self.shown() {
                    // Hidden since: once it shows.
                    self.retry_when_shown.set(true);
                } else if w == TIMER_REFRESH || w == TIMER_RETRY {
                    self.start_search(true);
                } else if w == TIMER_NAV {
                    self.commit_nav();
                    self.settle_search();
                } else if w == TIMER_NOTE {
                    self.clear_note();
                } else if w == TIMER_SETTINGS {
                    self.save_settings();
                } else if w == TIMER_HISTORY {
                    self.save_history();
                } else if w == TIMER_SESSION {
                    self.save_session(false);
                    self.session_timer();
                } else if w == TIMER_RUNS {
                    self.save_runs();
                } else if w == TIMER_STORE_RETRY {
                    self.retry_writes();
                } else if w == TIMER_SETTLE {
                    self.settle_tick();
                } else if w == celltip::TIMER {
                    celltip::timer(self);
                } else if w == TIMER_LIVE {
                    self.live_tick();
                } else if w == TIMER_REST {
                    self.memory_rested();
                } else if w == selecting::TIMER_SELECTING {
                    self.selecting_due();
                } else if slots::is_feature_timer(w) {
                    self.feature_timer(w);
                } else if custom::is_timer(w) {
                    self.custom_timer(w);
                } else if w9a::is_timer(w) {
                    self.w9_timer(w);
                }
                0
            }
            WM_PANE_CLOSE => {
                if let Some(id) = crate::dock::PaneId::from_code(w) {
                    self.show_pane(id, false);
                }
                0
            }
            id if slots::is_feature_message(id) => {
                self.feature_message(id, w, l).unwrap_or_else(|| m.def())
            }
            id if custom::is_message(id) => {
                self.custom_message(id, w, l).unwrap_or_else(|| m.def())
            }
            WM_EXPORT => {
                if w == crate::export::ENDED {
                    if let Some(ended) = parcel::take::<crate::export::Ended>(m) {
                        self.export_ended(ended);
                    }
                } else {
                    self.export_progress();
                }
                0
            }
            WM_COLUMNS => {
                self.sync_columns();
                0
            }
            WM_STORE => {
                if let Some(text) = parcel::take::<String>(m) {
                    self.store_report(text, w == prefs::STORE_FAILED);
                }
                0
            }
            WM_HISTORY => {
                if let Some(written) = parcel::take::<crate::history_file::Written>(m) {
                    self.history_written(written);
                }
                0
            }
            WM_BOOKMARKS => {
                if let Some(note) = parcel::take::<Option<String>>(m) {
                    self.bookmarks_written(note);
                }
                0
            }
            WM_RUNS => {
                if let Some(written) = parcel::take::<crate::run_history_file::Written>(m) {
                    self.runs_written(written);
                }
                0
            }
            // Windows is ending: the settings go out now, as the
            // process may not get another chance. The session's timer
            // keeps running: the end may be called off.
            WM_QUERYENDSESSION => {
                self.save_settings();
                self.save_history();
                self.save_bookmarks();
                self.save_runs();
                self.save_session_now();
                1
            }
            WM_ENDSESSION => {
                if w != 0 {
                    self.save_and_flush();
                } else {
                    // Called off (another program said no): goz-gui
                    // runs on, and its session is written as before.
                    self.session_timer();
                }
                0
            }
            WM_FILEOP => {
                if let Some(done) = parcel::take::<fileops::Done>(m) {
                    self.file_op_done(done);
                }
                0
            }
            WM_DRAG_DONE => {
                self.drag_done();
                0
            }
            WM_RENAME_AGAIN => {
                self.rename_again();
                0
            }
            WM_DROPFILES => {
                self.drop_files(w as HDROP);
                0
            }
            WM_REPLY => {
                if let Some(reply) = parcel::take::<Reply>(m) {
                    self.on_reply(reply);
                }
                0
            }
            WM_ICON => {
                if let Some(reply) = parcel::take::<IconReply>(m) {
                    self.on_icon(&reply);
                }
                0
            }
            WM_TYPES_ASK => {
                self.icons.types.send();
                0
            }
            WM_TYPES => {
                // Rows of those types show their icons and names now
                // (Details' rows and the thumbnail cells alike).
                if self.icons.types.arrived() {
                    win::invalidate(self.list, None, false);
                }
                0
            }
            WM_THUMB => {
                // Only a thumbnail kept puts the rest off: one thrown
                // away keeps nothing, and a hidden window (the
                // thumbnails let go) sets no timer.
                if let Some(reply) = parcel::take::<ThumbReply>(m)
                    && self.on_thumb(reply)
                {
                    self.memory_settles();
                }
                0
            }
            thumbview::WM_BIG_ICONS => {
                self.big_icons_loaded();
                0
            }
            WM_ACTIVATE => {
                // Options > Search's "Focus search on activate": the
                // keyboard goes to the search box, its text selected,
                // once the activation is done (Windows gives the
                // keyboard back to where it was as it ends). Not for
                // a click, whose control takes it where it lands.
                let to_search = u32::from(loword(w)) == WA_ACTIVE
                    && hiword(w) == 0
                    && self.ui.get().focus_on_activate;
                if u32::from(loword(w)) == WA_INACTIVE {
                    dropdown::close(self);
                    let focus = win::focus();
                    // The search box, the results, or a pane's or
                    // strip's window: it has the keyboard back after.
                    if self.frame_of(focus).is_some() {
                        self.last_focus.set(focus);
                    }
                } else {
                    if to_search {
                        self.last_focus.set(self.edit);
                        win::post_app(self.hwnd, uiopts::WM_FOCUS_SEARCH, 0, 0);
                    }
                    // A click on the search box that activates the
                    // window selects its text too (select search on
                    // mouse click), though the box had the keyboard.
                    if u32::from(loword(w)) == WA_CLICKACTIVE
                        && self.ui.get().select_on_click
                        && b1::cursor_pos().is_some_and(|pt| win::from_point(pt) == self.edit)
                    {
                        self.click_select.set(true);
                    }
                    self.refresh_if_idle();
                    self.arm_live();
                }
                m.def()
            }
            WM_SETFOCUS => {
                win::set_focus(self.focus_to_restore(self.last_focus.get()));
                0
            }
            WM_DPICHANGED => {
                let dpi = self.ui_dpi_for(u32::from(hiword(w)));
                if let Some(suggested) = m.dpi_rect() {
                    self.on_dpi(dpi, &suggested);
                }
                0
            }
            // What the window paints itself, for accessibility tools.
            WM_GETOBJECT if crate::a11y::wants_client(l) => {
                self.accessible_object(w).unwrap_or_else(|| m.def())
            }
            WM_SETTINGCHANGE => {
                if m.setting_is("ImmersiveColorSet")
                    && (self.theme_mode.get() == ThemeMode::UserDefault
                            // Windows' accent changed, which Fonts and
                            // Colors' accent of Default follows.
                            || self.custom_palette(self.pal.get().dark).inks
                                != self.pal.get().inks)
                {
                    self.apply_theme();
                }
                m.def()
            }
            WM_GETMINMAXINFO => {
                if let Some(mut mmi) = m.min_max_info() {
                    mmi.ptMinTrackSize = POINT {
                        x: self.scale(420),
                        y: self.scale(240),
                    };
                }
                0
            }
            paint::WM_UAHDRAWMENU if paint::draws_menubar(&self.pal.get()) => {
                if let Some(um) = b1::uah_menu(m) {
                    paint::menubar(self, &um);
                }
                1
            }
            paint::WM_UAHDRAWMENUITEM if paint::draws_menubar(&self.pal.get()) => {
                if let Some(item) = b1::uah_draw_menu_item(m) {
                    paint::menubar_item(self, &item);
                }
                1
            }
            WM_NCPAINT | WM_NCACTIVATE if paint::draws_menubar(&self.pal.get()) => {
                let r = m.def();
                paint::menubar_line(self);
                r
            }
            // A new window (`-new-window`) ends when closed: running in
            // the background and the tray are the first goz-gui's.
            WM_CLOSE if self.background().run_in_background && !self.is_new_window() => {
                // goz-gui keeps running in the background.
                self.hide_to_background();
                0
            }
            // Only with the tray icon up (a New Window's has none).
            WM_SYSCOMMAND
                if (w & 0xfff0) == SC_MINIMIZE as usize
                    && self.background().minimize_to_tray
                    && self.tray_on.get() =>
            {
                self.hide_to_background();
                0
            }
            WM_HOTKEY => {
                self.on_hotkey(w);
                0
            }
            WM_TRAY => {
                self.on_tray(w, l);
                0
            }
            WM_LAUNCH => {
                if let Some(args) = parcel::take::<Vec<String>>(m) {
                    self.launch(args);
                }
                0
            }
            id if id == self.taskbar_created && id != 0 => {
                self.taskbar_restarted();
                0
            }
            WM_DESTROY => {
                self.remove_tray();
                // An export still running stops, and takes its tmp file
                // with it.
                let export = self.export.borrow_mut().take();
                if let Some(export) = export {
                    export.finish();
                }
                // The window is still there to read its placement from.
                self.save_and_flush();
                // The features, then the panes' contents, let go of
                // what they hold; the settings keep the panes open for
                // next time.
                self.exit_features();
                self.exit_custom();
                self.close_all_panes();
                self.worker.quit();
                queue::post_quit(0);
                0
            }
            _ => m.def(),
        }
    }

    fn notify(&self, m: &Msg) -> LRESULT {
        let Some(n) = m.notify() else {
            return 0;
        };
        if !n.from().is_null() && n.from() == self.tooltip.get() {
            if n.code() == TTN_GETDISPINFOW
                && let Some(mut info) = n.get::<NMTTDISPINFOW>()
            {
                searchbox::tooltip_text(self, &mut info);
            }
            return 0;
        }
        if n.from() != self.list {
            return 0;
        }
        match n.code() {
            LVN_GETDISPINFOW => {
                if let Some(mut info) = n.get::<NMLVDISPINFOW>() {
                    self.disp_info(&mut info);
                }
            }
            LVN_ODCACHEHINT => {
                if let Some((from, to)) = n.get::<NMLVCACHEHINT>().map(|h| (h.iFrom, h.iTo)) {
                    self.request_rows(from.max(0) as usize, to.max(0) as usize);
                }
            }
            LVN_ODFINDITEMW => return -1,
            LVN_BEGINDRAG | LVN_BEGINRDRAG => self.begin_drag(),
            LVN_BEGINLABELEDITW => {
                let item = n.get::<NMLVDISPINFOW>().map(|info| info.item.iItem);
                return item.map_or(0, |item| LRESULT::from(self.begin_rename(item)));
            }
            LVN_ENDLABELEDITW => {
                // The text typed (none when the edit was cancelled), as a
                // NUL-terminated copy.
                let text = n
                    .get::<NMLVDISPINFOW>()
                    .and_then(|info| info.text())
                    .map(|t| win::wz_units(&t));
                self.end_rename(text.as_ref().map_or(core::ptr::null(), |t| t.as_ptr()));
                // The row changes once the rename is done.
                return 0;
            }
            LVN_COLUMNCLICK => {
                if let Some(sub) = n.get::<NMLISTVIEW>().map(|nm| nm.iSubItem)
                    && let Some(id) = self.column_id(sub.max(0) as usize)
                {
                    self.sort_by_column(id);
                }
            }
            // With one-click opening (Options > Results) the click
            // opens, and the double-click that may follow does not
            // again.
            NM_DBLCLK if !self.single_click_open() => {
                if let Some(nm) = n.get::<NMITEMACTIVATE>().map(|nm| *nm)
                    && nm.iItem >= 0
                {
                    self.double_click(nm.ptAction);
                }
            }
            NM_CLICK if self.single_click_open() => {
                // Ctrl and Shift pick rows; a plain click opens.
                let keys = LVKF_ALT | LVKF_CONTROL | LVKF_SHIFT;
                if let Some(nm) = n.get::<NMITEMACTIVATE>().map(|nm| *nm)
                    && nm.iItem >= 0
                    && nm.uKeyFlags & keys == 0
                {
                    self.double_click(nm.ptAction);
                }
            }
            // Enter and the list's other keys never reach the list's own
            // handling (list_proc runs them), so no NM_RETURN comes.
            LVN_ITEMCHANGED | LVN_ODSTATECHANGED => {
                // Kept as ranges, whoever changed it (`selection.rs`).
                let change = results::selection_change(&n);
                if let Some((rows, on)) = change.clone() {
                    let mut sel = self.sel.borrow_mut();
                    match rows {
                        Some(rows) => sel.set(rows, on),
                        None => sel.set_all(on),
                    }
                }
                if self.bulk.get() {
                    // Rows changed one by one by goz-gui: reported
                    // once, after the run (`in_bulk`).
                    self.bulk_changed.set(true);
                    return 0;
                }
                // The user chose other rows: an action waiting for the
                // rows chosen before is not wanted any more, and they
                // are not Select All's every row (a move of the focus
                // alone keeps both).
                if !self.restoring.get() && change.is_some() {
                    self.cancel_fetch();
                    self.select_all.set(false);
                }
                self.selection_changed();
                self.invalidate_status();
                self.pane_event(PaneEvent::SelectionChanged);
            }
            NM_CUSTOMDRAW => {
                let Some(cd) = n.get::<NMCUSTOMDRAW>() else {
                    return CDRF_DODEFAULT as LRESULT;
                };
                if self.view.get().thumbnails {
                    return self.thumb_custom_draw(&cd);
                }
                return match cd.dwDrawStage {
                    CDDS_PREPAINT => CDRF_NOTIFYPOSTPAINT as LRESULT,
                    CDDS_POSTPAINT => {
                        paint::below_rows(self, cd.hdc);
                        CDRF_DODEFAULT as LRESULT
                    }
                    _ => CDRF_DODEFAULT as LRESULT,
                };
            }
            _ => {}
        }
        0
    }

    /// Text for accessibility tools (the list's own type-ahead never runs:
    /// `list_proc` keeps the typed characters for goz-gui's), and the names
    /// icon view measures. While rows change state one by one (`in_bulk`)
    /// there is none: the list would only measure it to invalidate the
    /// item, and the list repaints whole after.
    fn disp_info(&self, info: &mut win::View<NMLVDISPINFOW>) {
        if info.item.mask & LVIF_TEXT == 0
            || info.item.pszText.is_null()
            || info.item.cchTextMax <= 0
        {
            return;
        }
        let index = info.item.iItem.max(0) as usize;
        let mut text = Vec::new();
        let column = if self.bulk.get() {
            None
        } else {
            self.column_id(info.item.iSubItem.max(0) as usize)
        };
        if let Some(id) = column {
            let m = self.model.borrow();
            if let Some(row) = m.row(index) {
                // As drawn: the shell is not asked on this thread.
                paint::push_cell(self, row, id, &mut text);
            }
        }
        // Into the list's buffer of `cchTextMax` code units, cut to fit.
        info.set_text(&text);
    }

    /// The blocks holding the rows in view.
    fn view_blocks(&self) -> Range<u32> {
        let items = self.visible_items();
        let (top, end) = (items.start as u32, items.end as u32);
        top / BLOCK_ROWS..end / BLOCK_ROWS + 1
    }

    pub fn request_rows(&self, from: usize, to: usize) {
        let (epoch, search, blocks) = {
            let mut m = self.model.borrow_mut();
            let blocks = m.missing(from, to);
            (m.shown, Arc::clone(&m.search), blocks)
        };
        if blocks.is_empty() {
            return;
        }
        self.trim_requests(epoch);
        for block in blocks {
            self.worker.push(Job::Block {
                epoch,
                block,
                search: Arc::clone(&search),
            });
        }
    }

    /// The blocks of search `epoch` asked for on the way here (a drag of the
    /// scroll box paints many) and not run yet are not wanted any more;
    /// those near the view are, block 0, which carries the result's size,
    /// and those an action waits for (pinned).
    pub(super) fn trim_requests(&self, epoch: u64) {
        let view = self.view_blocks();
        let near = view.start.saturating_sub(1)..view.end.saturating_add(1);
        let dropped = {
            let m = self.model.borrow();
            self.worker
                .trim(epoch, |b| b == 0 || near.contains(&b) || m.pinned(b))
        };
        if !dropped.is_empty() {
            let mut m = self.model.borrow_mut();
            for block in dropped {
                m.forget_request(block);
            }
        }
    }

    /// Sends the search in the box to gozd. `keep_place` re-runs the search
    /// on screen (a refresh), keeping the scroll position and the selected
    /// files.
    fn start_search(&self, keep_place: bool) {
        // A refresh while a changed search is on its way would only ask for
        // what that search brings anyway, and as a refresh it would keep the
        // old results' place and selection for the new ones (and lose an
        // Enter waiting for them).
        if keep_place
            && self
                .model
                .borrow()
                .pending
                .as_ref()
                .is_some_and(|p| !p.keep_place)
        {
            return;
        }
        // Nor while the box holds text typed and not searched yet (search
        // as you type is off, or its delay runs): a refresh would search it
        // as if it were the search on screen.
        if keep_place && self.search_is_dirty() {
            return;
        }
        // Nor while a row is being renamed: the refreshed rows could put
        // another file under its box. It runs once the box is closed. Nor
        // while an action waits for its rows, which are this search's.
        if keep_place && (self.renaming() || self.fetch_waiting()) {
            win::set_timer(self.hwnd, TIMER_REFRESH, 1000);
            return;
        }
        self.retry_when_shown.set(false);
        win::kill_timer(self.hwnd, TIMER_REFRESH);
        win::kill_timer(self.hwnd, TIMER_RETRY);
        if !keep_place {
            // A changed search becomes a navigation entry once it has stayed
            // as it is for a moment.
            win::set_timer(self.hwnd, TIMER_NAV, nav::NAV_DELAY_MS);
        }
        if !keep_place {
            // An Enter for the search before this one is spent, a note about
            // the rows before these is stale, and so are the files a refresh
            // of them was to select again, and a restored session's place.
            // This search is not made yet.
            self.enter_pending.set(false);
            self.cancel_fetch();
            self.clear_note();
            // Select All was of the rows on screen, not of this search's.
            self.select_all.set(false);
            // Files still to be selected again stay sought when a new
            // search keeps the selection (Options > Results).
            if self.ui.get().clear_selection {
                self.forget_selection();
            }
            self.drop_view();
            self.search_started();
            self.settled.set(false);
            self.beeped.set(false);
            // Its options (and its text, when Home keeps the last search)
            // are settings.
            self.settings_changed();
        }
        let text = window_text(self.edit);
        let o = self.opts.get();
        // The Folders pane's folder goes in front of the text (F2). W8: in
        // front of each part `new-search:` starts, with the filter and Match
        // Path, as each is a search of its own.
        let scoped = self.scoped_search(&text);
        let to_gozd = |text: &str| {
            crate::filelist::slots::each_part(text, |part| {
                filter::query(o.filter, o.match_path, &self.scoped_search(part))
            })
        };
        // W9: `content:` terms are read by goz-gui, not gozd: gozd is asked
        // the search without them (or it is refused, and why).
        let planned = self.plan_content(&text, o.match_case);
        let query = match &planned {
            crate::content::plan::Planned::Content(p) => {
                to_gozd(p.queries.first().map_or("", String::as_str))
            }
            _ => to_gozd(&text),
        };
        // W8: an `open-filelist:` term's list is searched for this search
        // instead of the index or the tab's list (or why it cannot be).
        let (filelist, mut refused) = match self.search_file_list(&text) {
            Ok(list) => (list, None),
            Err(why) => (self.filelist(), Some(why)),
        };
        if let crate::content::plan::Planned::Refused(why) = &planned {
            refused.get_or_insert_with(|| why.clone());
        }
        let base = Search {
            query,
            sort: o.sort,
            match_case: o.match_case,
            // W4 (G1): an open file list is searched instead of gozd.
            filelist,
            // W5: sorted by its Date Created, when that is the sort.
            created_sort: self.created_sort(),
            // W6: the tab's file-list slots, for filelistN:.
            slots: self.file_list_slots(),
            // W9: the wave 9 columns shown (stream B2 asks gozd for them).
            want: crate::props::Want::of_columns(self.layout.get().shown()),
            content: None,
        };
        let content = match &planned {
            crate::content::plan::Planned::Content(p) if refused.is_none() => {
                Some(self.content_run(p, &base, keep_place, &to_gozd))
            }
            _ => {
                self.end_content();
                None
            }
        };
        let search = Arc::new(Search { content, ..base });
        let epoch = self.next_epoch.get() + 1;
        self.next_epoch.set(epoch);
        self.model.borrow_mut().pending = Some(Pending {
            epoch,
            search: Arc::clone(&search),
            // W8: the results are the last part's, and so are its words.
            highlight: self.highlight_for(
                crate::filelist::slots::last_part(&text),
                o.match_case,
                o.match_path,
            ),
            keep_place,
        });
        self.worker.begin(epoch);
        // A folder chosen in the Folders pane is a search of its own: the
        // folder's contents show though the box is empty; so does an open
        // file list (W4, G1), as in Everything.
        let hidden =
            self.ui.get().hide_empty && scoped.trim().is_empty() && search.filelist.is_none();
        if let Some(why) = &refused {
            // W8: answered as gozd's refusal would be, without asking.
            let failed = Reply::Failed {
                epoch,
                block: 0,
                failure: Failure::Refused(why.clone()),
            };
            // Posted to this thread's window, which takes it with the
            // message; if posting fails it is dropped.
            parcel::post(self.hwnd, WM_REPLY, 0, failed);
        } else if hidden {
            // Options > Results' "Hide results when the search is empty":
            // gozd is not asked; the empty answer comes as its would, so
            // the window takes it as any other.
            let empty = Reply::Block {
                epoch,
                block: 0,
                rows: Vec::new(),
                total: 0,
                incomplete: false,
                metadata_pending: false,
                total_size: None,
                elapsed: Duration::ZERO,
            };
            // Posted to this thread's window, which takes it with the
            // message; if posting fails it is dropped.
            parcel::post(self.hwnd, WM_REPLY, 0, empty);
        } else {
            self.worker.push(Job::Block {
                epoch,
                block: 0,
                search: Arc::clone(&search),
            });
        }
        if keep_place && !hidden && refused.is_none() {
            // Fetch where the user is looking too, so a refresh repaints in
            // place instead of row by row.
            let top = self.top_index() as u32;
            let block = top / BLOCK_ROWS;
            if block > 0 {
                self.worker.push(Job::Block {
                    epoch,
                    block,
                    search,
                });
            }
        }
        self.last_search.set(Some(Instant::now()));
        // Everything's title names an open file list too (W8: the one this
        // search runs over).
        let list = self.searched_list_name();
        let title = match (text.is_empty(), list) {
            (true, None) => "goz".to_owned(),
            (false, None) => format!("{text} - goz"),
            (true, Some(list)) => format!("{list} - goz"),
            (false, Some(list)) => format!("{text} - {list} - goz"),
        };
        // Set only when it changed: a refresh (every live update) would
        // repaint the title bar for nothing.
        if window_text(self.hwnd) != title {
            win::set_text(self.hwnd, &title);
        }
        self.apply_on_top();
        if !keep_place {
            // Back and Forward may have somewhere to go now, or not; the
            // filter bar shows the filter, however it was chosen.
            searchbox::invalidate_nav(self);
            self.invalidate_filter_bar();
            self.pane_event(PaneEvent::SearchChanged);
            // The shown tab's title is its search.
            self.tabs_search_changed();
        }
    }

    /// The search box's text changed: a new search, not yet in the history;
    /// the clear button may become a magnifier or back. Typed, it may bring
    /// suggestions, and it is searched at once, after the search delay or
    /// on Enter (Options > Search); set by goz-gui (a history entry chosen,
    /// Back), the dropdown closes and it is searched at once.
    fn text_changed(&self) {
        self.recorded.set(false);
        let typed = self.typing.get();
        if typed {
            self.suggest();
        } else {
            dropdown::close(self);
        }
        searchbox::invalidate(self);
        if self.search_now_or_later(typed) {
            self.start_search(false);
        }
    }

    /// A click on a search box button.
    fn search_button(&self, button: Button) {
        match button {
            Button::Clear => {
                if win::text_len(self.edit) > 0 {
                    self.command(menu::CLEAR_SEARCH);
                } else {
                    self.focus_box();
                }
            }
            Button::History => self.toggle_history(),
            Button::Back => self.go_back(),
            Button::Forward => self.go_forward(),
        }
    }

    /// The search on screen counts as made: Enter, an action on its
    /// results, the list taking the keyboard, or its text staying as it is
    /// for a moment.
    pub(super) fn settle_search(&self) {
        self.settled.set(true);
        self.warn_if_refused();
    }

    /// Beeps once for a search gozd refused (bad syntax and the like), once
    /// the search counts as made: not for each refused prefix typed on the
    /// way to a good one (`dm:` and `dm:t` before `dm:today`), and not again
    /// when it is re-run (a refresh, the window coming back). A missing
    /// daemon is retried quietly instead.
    fn warn_if_refused(&self) {
        if !self.settled.get() || self.beeped.get() {
            return;
        }
        let refused = {
            let m = self.model.borrow();
            // While a changed search is on its way, the failure on screen
            // is the search before it.
            m.pending.as_ref().is_none_or(|p| p.keep_place)
                && matches!(m.failure, Some((Failure::Refused(_), _)))
        };
        if refused {
            self.beeped.set(true);
            sys::beep(MB_ICONWARNING);
        }
    }

    /// The window shows again after gozd could not be reached while it
    /// was hidden: the search runs again (and retries every 2 s from now
    /// on, if gozd is still not there).
    fn retry_if_waiting(&self) {
        if self.retry_when_shown.replace(false) {
            self.start_search(true);
        }
    }

    /// Re-runs the search when the window comes back, so files changed
    /// meanwhile show up.
    fn refresh_if_idle(&self) {
        let stale = self
            .last_search
            .get()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(1));
        if stale && self.model.borrow().pending.is_none() {
            self.start_search(true);
        }
    }

    fn on_reply(&self, reply: Reply) {
        match reply {
            Reply::Block {
                epoch,
                block,
                rows,
                total,
                incomplete,
                metadata_pending,
                total_size,
                elapsed,
            } => {
                let (refresh, fresh) = {
                    let m = self.model.borrow();
                    let p = m.pending.as_ref().filter(|p| p.epoch == epoch);
                    (
                        p.is_some_and(|p| p.keep_place),
                        p.is_some_and(|p| !p.keep_place),
                    )
                };
                // What is selected, by path, while the rows it is on are
                // still the ones held: for a refresh, and for a new search
                // when Options > Results keeps the selection.
                let keep_selection = fresh && !self.ui.get().clear_selection;
                let kept = (refresh || keep_selection).then(|| self.note_selection(refresh, total));
                let on_screen = self.view_blocks();
                let mut m = self.model.borrow_mut();
                let mut adopted = None;
                if m.pending.as_ref().is_some_and(|p| p.epoch == epoch) {
                    let p = m.pending.take().expect("checked above");
                    adopted = Some(p.keep_place);
                    let old_size = m.total_size;
                    let keep = p.keep_place;
                    m.adopt(p);
                    // A refresh may answer the block on screen before block
                    // 0, which carries the size: keep showing the old one.
                    if keep {
                        m.total_size = old_size;
                    }
                    m.elapsed = Some(elapsed);
                } else if epoch != m.shown {
                    return;
                }
                let old_total = m.total;
                m.total = total;
                m.incomplete = incomplete;
                m.metadata_pending = metadata_pending;
                if total_size.is_some() {
                    m.total_size = total_size;
                }
                let moved = m.insert(block, rows, on_screen);
                drop(m);
                match adopted {
                    Some(keep) => {
                        self.icons.reset(epoch);
                        // W9: the disk reads asked for the rows before.
                        self.w9.props.reset(epoch);
                        if !keep {
                            // Other files: their thumbnails are not wanted,
                            // and Select All was of the rows before.
                            self.reset_thumbs();
                            self.select_all.set(false);
                        }
                        // Taken first: the count's own selection changes
                        // must not drop it.
                        let view = self.take_view(epoch);
                        self.set_count(total, keep);
                        let (kept_refresh, kept_search) = match kept {
                            Some(k) if keep => (Some(k), None),
                            k => (None, k),
                        };
                        if let Some(view) = view {
                            // A restored session's search: its place and
                            // selection.
                            self.apply_view(view, total);
                        } else if let Some(kept) = kept_refresh {
                            // The list's selection is by row number, which
                            // now may be another file's.
                            self.restore_selection(kept);
                        } else if self.enter_pending.take() {
                            self.enter_select();
                        } else if let Some(kept) = kept_search {
                            // Another search: the files selected are
                            // selected again where they are in it, from
                            // its top.
                            self.restore_selection(kept.for_new_search());
                            if total > 0 {
                                lv::ensure_visible(self.list, 0, false);
                            }
                        } else {
                            self.clear_selection();
                            if total > 0 {
                                lv::ensure_visible(self.list, 0, false);
                            }
                        }
                        // W5: a file list's dates created, found as it is
                        // read, bring the Date Created column.
                        self.refresh_created_column();
                        // W7: a search made (not refreshed) saves its
                        // results to the slots its save-filelistN: names,
                        // once they are all there (a list still being read
                        // or a volume still indexed: at a refresh).
                        if !keep {
                            self.data_tools.save_due.set(true);
                        }
                        if !incomplete && self.data_tools.save_due.replace(false) {
                            self.save_to_slots(total);
                        }
                        // Repaints the list.
                        win::invalidate(self.list, None, false);
                        self.pane_event(PaneEvent::ResultsChanged { refresh: keep });
                    }
                    None => {
                        if old_total != total {
                            // Select All stays every row.
                            self.recount(old_total, total);
                        }
                        let first = u64::from(block) * u64::from(BLOCK_ROWS);
                        let last = (first + u64::from(BLOCK_ROWS)).min(total).saturating_sub(1);
                        if first <= last {
                            lv::redraw_items(self.list, first as i32, last as i32);
                        }
                    }
                }
                if moved {
                    // Before a refresh's files are selected again in it,
                    // by path.
                    self.selection_moved(block);
                }
                self.reselect_block(block);
                if adopted.is_some() {
                    // Other rows now: an action waiting for the old ones
                    // cannot have them.
                    self.cancel_fetch();
                } else {
                    self.fetch_arrived(epoch, block);
                }
                if incomplete || metadata_pending {
                    // A timer of this window.
                    win::set_timer(self.hwnd, TIMER_REFRESH, 1000);
                }
                self.invalidate_status();
            }
            Reply::Failed {
                epoch,
                block,
                failure,
            } => {
                let mut m = self.model.borrow_mut();
                let adopted = m.pending.as_ref().is_some_and(|p| p.epoch == epoch);
                let retry = !matches!(failure, Failure::Refused(_));
                if adopted {
                    let p = m.pending.take().expect("checked above");
                    m.adopt(p);
                } else if epoch != m.shown {
                    return;
                } else if let Failure::Refused(why) = &failure {
                    // A block after the first refused (gozd's time budget
                    // ran out, say). It stays asked for, so the repaints of
                    // its empty rows do not run it again and again; the
                    // next search or refresh asks anew. The status bar says
                    // why the rows are empty.
                    m.partial_failure = Some(why.clone());
                    drop(m);
                    self.invalidate_status();
                    // An action waiting for it acts on what is held.
                    self.fetch_arrived(epoch, block);
                    return;
                } else {
                    m.forget_request(block);
                }
                if adopted || retry {
                    let text = match &failure {
                        Failure::NotRunning => crate::client::not_running_text(),
                        Failure::Untrusted => "Something other than gozd holds goz's pipe; \
                                               goz-gui will not read from it."
                            .into(),
                        Failure::Refused(m) | Failure::Other(m) => m.clone(),
                    };
                    m.failure = Some((failure, text));
                }
                drop(m);
                if adopted {
                    self.icons.reset(epoch);
                    self.forget_selection();
                    self.enter_pending.set(false);
                    self.set_count(0, false);
                    self.warn_if_refused();
                    self.cancel_fetch();
                } else {
                    // An action waiting for it acts on what is held.
                    self.fetch_arrived(epoch, block);
                }
                if retry {
                    if self.shown() {
                        // A timer of this window.
                        win::set_timer(self.hwnd, TIMER_RETRY, 2000);
                    } else {
                        // Hidden or minimized: no busy retries meanwhile;
                        // the search runs again as it shows.
                        self.retry_when_shown.set(true);
                    }
                }
                // Repaints the list.
                win::invalidate(self.list, None, true);
                self.invalidate_status();
            }
            Reply::Status(status) => {
                if self.want_status.replace(false) {
                    let text = match status {
                        Ok(s) => {
                            let mut text = status_text(&s);
                            if let Some(t) = self.model.borrow().elapsed {
                                text.push_str(&format!("\nLast search: {} ms", t.as_millis()));
                            }
                            text
                        }
                        Err(Failure::NotRunning) => crate::client::not_running_text(),
                        Err(Failure::Untrusted) => "gozd could not be verified.".into(),
                        Err(Failure::Refused(m) | Failure::Other(m)) => m,
                    };
                    self.message_box(&text, "Index Status");
                }
            }
            Reply::Syntax(on) => self.syntax_learned(on),
            Reply::Features(features) => self.features_learned(features),
            Reply::Saved { epoch, saves } => self.parts_saved(epoch, saves),
        }
    }

    fn on_icon(&self, reply: &IconReply) {
        {
            let m = self.model.borrow();
            if reply.epoch != m.shown {
                return;
            }
            let Some(row) = m.row(reply.index) else {
                return;
            };
            row.icon
                .set(if reply.icon >= 0 { reply.icon } else { GENERIC });
        }
        lv::redraw_items(self.list, reply.index as i32, reply.index as i32);
    }

    fn set_count(&self, total: u64, keep: bool) {
        let flags = if keep {
            LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL
        } else {
            0
        };
        let count = total.min(i32::MAX as u64) as usize;
        // The list drops the selection of rows past the end without saying
        // so, and reports any change it makes to the others: the kept
        // ranges have the new size before those reports, and are trimmed
        // to it after.
        self.sel.borrow_mut().set_count(count);
        lv::set_item_count(self.list, count, flags);
        self.sel.borrow_mut().set_count(count);
    }

    /// The state bits in `mask` of row `index` (`usize::MAX`: every row).
    fn set_state(&self, index: usize, state: u32, mask: u32) {
        lv::set_item_state(self.list, index as i32, state, mask);
    }

    /// No row selected or focused, and no anchor for Shift+click: all three
    /// are row numbers, which a new set of rows gives to other files.
    fn clear_selection(&self) {
        // Rows still to be set one by one belong to the selection replaced.
        self.drop_selecting();
        self.set_state(usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        lv::set_selection_mark(self.list, -1);
    }

    pub fn focus_list(&self) {
        win::set_focus(self.list);
        let count = lv::item_count(self.list);
        let focused = lv::next_item(self.list, -1, LVNI_FOCUSED);
        if count > 0 && focused < 0 {
            self.set_state(
                0,
                LVIS_SELECTED | LVIS_FOCUSED,
                LVIS_SELECTED | LVIS_FOCUSED,
            );
            lv::ensure_visible(self.list, 0, false);
        }
    }

    pub fn invalidate_status(&self) {
        let rc = self.status_rc.get();
        // Repaints part of this window.
        win::invalidate(self.hwnd, Some(&rc), false);
    }

    /// Selected row indices, up to `cap`.
    pub fn selected(&self, cap: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = usize::MAX;
        while out.len() < cap {
            let next = lv::next_item(self.list, at as i32, LVNI_SELECTED);
            if next < 0 {
                break;
            }
            out.push(next as usize);
            at = next as usize;
        }
        out
    }

    fn sort_by_column(&self, col: usize) {
        self.sort_by_key(match col {
            COL_NAME => SortKey::Name,
            COL_PATH => SortKey::Path,
            COL_SIZE => SortKey::Size,
            COL_DATE => SortKey::DateModified,
            // W5: a file list's Date Created, sorted by the list itself.
            crate::columns::COL_CREATED if self.created_column_wanted() => {
                return self.sort_by_created();
            }
            // W9: gozd's newer keys, when it lists them (`w9b.rs`); the sort
            // stays otherwise, and the status bar says why for a moment.
            _ => return self.sort_by_new_column(col),
        });
    }

    fn set_sort(&self, sort: SortSpec) {
        let mut o = self.opts.get();
        o.sort = sort;
        self.opts.set(o);
        // Another sort than a file list's Date Created (W5).
        self.end_created_sort();
        self.show_sort();
        self.start_search(false);
    }

    /// Repaints what shows the sort: the header's arrow and shaded item
    /// (header.rs draws them, not the header's HDF_SORT* flags), and the
    /// sorted column's cells.
    pub(super) fn show_sort(&self) {
        win::invalidate(self.header, None, false);
        win::invalidate(self.list, None, false);
    }

    /// The column the results are sorted by.
    pub fn sorted_column(&self) -> usize {
        if self.created_sort().is_some() {
            return crate::columns::COL_CREATED;
        }
        match self.opts.get().sort.key {
            SortKey::Name => COL_NAME,
            SortKey::Path => COL_PATH,
            SortKey::Size => COL_SIZE,
            SortKey::DateModified => COL_DATE,
            SortKey::Extension => crate::columns::COL_EXT,
            SortKey::DateCreated => crate::columns::COL_CREATED,
            SortKey::DateAccessed => crate::columns::COL_ACCESSED,
            SortKey::Attributes => crate::columns::COL_ATTRIB,
            SortKey::DateRecentlyChanged => crate::columns::COL_RECENT,
        }
    }

    pub fn sort_ascending(&self) -> bool {
        match self.created_sort() {
            Some(dir) => dir == SortDir::Asc,
            None => self.opts.get().sort.dir == SortDir::Asc,
        }
    }

    /// The search options in force that the status bar shows, left to right.
    pub fn indicators_on(&self) -> Vec<(Indicator, &'static str)> {
        let o = self.opts.get();
        let mut out = Vec::new();
        if o.match_case {
            out.push((Indicator::Case, "CASE"));
        }
        if o.match_path {
            out.push((Indicator::Path, "PATH"));
        }
        if let Some(name) = o.filter.name() {
            out.push((Indicator::Filter, name));
        }
        out
    }

    /// Turns off the option an indicator stands for, and searches again.
    fn turn_off(&self, indicator: Indicator) {
        let mut o = self.opts.get();
        match indicator {
            Indicator::Case => o.match_case = false,
            Indicator::Path => o.match_path = false,
            Indicator::Filter => o.filter = Filter::Everything,
            // The Folders pane's folder (F2), no search option.
            Indicator::Folder => return self.clear_folder(),
            // W4 (G1): the open file list closes.
            Indicator::FileList => return self.close_file_list(),
        }
        self.opts.set(o);
        self.start_search(false);
        self.invalidate_status();
    }

    fn on_dpi(&self, dpi: u32, suggested: &RECT) {
        dropdown::close(self);
        // The user's last widths first, at the old DPI.
        self.sync_columns();
        self.dpi.set(dpi);
        // Every part's font at the new DPI; the old ones go once the panes
        // and strips have theirs (`docks_dpi`).
        let old_fonts = self.remake_fonts();
        self.apply_fonts();
        self.remeasure_rows();
        self.rescale_columns();
        // Moves this thread's window to the rectangle Windows chose.
        win::set_pos(
            self.hwnd,
            core::ptr::null_mut(),
            (
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
            ),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        self.set_window_icons();
        self.docks_dpi(dpi);
        self.layout();
        // A pane's title row follows the font: its content is placed again
        // even where the pane kept its size.
        self.docks_place_all();
        drop(old_fonts);
        if self.view.get().thumbnails {
            // The thumbnails' size and spacing follow the DPI.
            self.put_view();
        }
    }

    /// An information box over the window. Nothing of `App` is borrowed
    /// while it runs its loop.
    fn message_box(&self, text: &str, title: &str) {
        msgbox::show(self.hwnd, text, title, MB_OK | MB_ICONINFORMATION);
    }

    /// The shown columns' ids and rectangles for a row at `row`, left to
    /// right, wherever they were dragged to.
    pub fn column_rects(&self, row: &RECT) -> Cells {
        let cols = self.list_cols.get();
        let n = cols.ids().len();
        // The order array (all 0 if the list could not give it).
        let mut order = [0i32; crate::columns::LIST_COLS];
        let got = lv::column_order(self.list, n);
        if got.len() == n {
            order[..n].copy_from_slice(&got);
        }
        let mut out = Cells {
            cells: [(0, RECT::default()); crate::columns::LIST_COLS],
            len: 0,
        };
        let mut x = row.left;
        for &index in &order[..n] {
            let index = (index.max(0) as usize).min(n.saturating_sub(1));
            let Some(&id) = cols.ids().get(index) else {
                continue;
            };
            let width = lv::column_width(self.list, index as i32);
            out.cells[out.len] = (
                id,
                RECT {
                    left: x,
                    right: x + width,
                    top: row.top,
                    bottom: row.bottom,
                },
            );
            out.len += 1;
            x += width;
        }
        out
    }

    /// Whether `pt` (screen) is on the list's header.
    fn over_header(&self, pt: POINT) -> bool {
        // The header's place on the screen.
        let rc = win::window_rect(self.header);
        in_rect(&rc, pt)
    }
}

/// The shown columns of a row: each one's id and rectangle, left to right.
pub struct Cells {
    cells: [(usize, RECT); crate::columns::LIST_COLS],
    len: usize,
}

impl core::ops::Deref for Cells {
    type Target = [(usize, RECT)];

    fn deref(&self) -> &[(usize, RECT)] {
        &self.cells[..self.len]
    }
}

fn status_text(s: &DaemonStatus) -> String {
    let mut out = String::new();
    for v in &s.volumes {
        let mounts = if v.mounts.is_empty() {
            v.guid.clone()
        } else {
            v.mounts.join(", ")
        };
        out.push_str(&format!(
            "{mounts}    {}    {} entries{}\n",
            v.phase,
            format::grouped(v.entries),
            if v.metadata_pending {
                "    (reading sizes and dates)"
            } else {
                ""
            }
        ));
    }
    if s.volumes.is_empty() {
        out.push_str("No volumes indexed yet.\n");
    }
    if s.process_working_set > 0 {
        out.push_str(&format!(
            "\ngozd memory: {} in use, {} committed",
            format::size_short(s.process_working_set),
            format::size_short(s.process_private_bytes)
        ));
    }
    out
}

/// The results list's subclass ([`LIST_SUBCLASS`]).
static LIST_SUBCLASS: subclass::Key = subclass::Key::new("results list");

/// The results list's subclass: the header's notifications, the mouse over
/// the rows, and the list's keys.
fn list_proc(app: &App, m: &Msg) -> LRESULT {
    let (msg, w, l) = (m.id, m.w, m.l);
    match msg {
        // The header reports its custom draw to the list, its parent,
        // and the user's drags and resizes of its items.
        WM_NOTIFY => {
            if let Some(n) = m.notify()
                && n.from() == app.header
            {
                match n.code() {
                    NM_CUSTOMDRAW => {
                        if let Some(cd) = n.get::<NMCUSTOMDRAW>() {
                            return header::custom_draw(app, &cd);
                        }
                    }
                    HDN_ENDDRAG | HDN_ENDTRACKW | HDN_ITEMCHANGEDW | HDN_DIVIDERDBLCLICKW => {
                        let r = m.def();
                        app.columns_touched();
                        return r;
                    }
                    _ => {}
                }
            }
        }
        WM_PAINT => {
            let r = m.def();
            paint::empty_list(app);
            return r;
        }
        WM_MOUSEMOVE => {
            hover::moved(app, m.point());
        }
        WM_MOUSELEAVE => hover::left(app),
        WM_LBUTTONDOWN => {
            celltip::hide(app);
            app.finish_selecting();
            app.last_click.set(point(l));
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            celltip::hide(app);
            app.finish_selecting();
        }
        // A click acts on the selection as it is to be (`selecting.rs`).
        WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK => app.finish_selecting(),
        // Rename's box, the list's child, in the list's colours.
        WM_CTLCOLOREDIT => {
            let pal = app.pal.get();
            let hdc = w as windows_sys::Win32::Graphics::Gdi::HDC;
            gdi::set_text_color(hdc, pal.list_fg);
            gdi::set_bk_color(hdc, pal.list_bg);
            gdi::set_dc_brush_color(hdc, pal.list_bg);
            return gdi::stock_object(DC_BRUSH) as LRESULT;
        }
        // The list taking the keyboard means the search was the one
        // wanted.
        WM_SETFOCUS => {
            let r = m.def();
            app.reset_type_ahead();
            app.record_search();
            return r;
        }
        WM_KEYDOWN => {
            celltip::hide(app);
            // A key acts on the selection as it is to be.
            app.finish_selecting();
            let cmd = keys::list_command(
                w as u16,
                key_down(VK_CONTROL),
                key_down(VK_SHIFT),
                key_down(VK_MENU),
            );
            if let Some(cmd) = cmd {
                app.command(cmd);
                return 0;
            }
            let r = m.def();
            hover::scrolled(app);
            return r;
        }
        // Alt+key in the list: Alt+Up goes up one folder in the Folders
        // pane (F2), Everything's global Up One Dir (the search box keeps
        // it for its history); Alt+Backspace is Everything's Undo (F5),
        // and its character is no menu's access key: no beep for it.
        WM_SYSKEYDOWN => {
            app.finish_selecting();
            if w == usize::from(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_UP)
                && app.up_one_folder()
            {
                return 0;
            }
            if let Some(cmd) =
                keys::list_alt_command(w as u16, key_down(VK_CONTROL), key_down(VK_SHIFT))
            {
                app.command(cmd);
                return 0;
            }
        }
        WM_SYSCHAR if w == 0x08 => return 0,
        // The characters of the keys above (Enter, Esc, Ctrl+A…, and
        // Ctrl+Backspace's DEL) do nothing more.
        WM_CHAR if w < 0x20 || w == 0x7f => return 0,
        // Printable ones go to goz-gui's own type-ahead, never the
        // list's, which would ask for rows it cannot search
        // (LVN_ODFINDITEMW).
        WM_CHAR => {
            app.finish_selecting();
            app.type_ahead(w as u16);
            return 0;
        }
        // Ctrl+wheel zooms the list (View > Zoom).
        WM_MOUSEWHEEL if (w & MK_CONTROL as usize) != 0 => {
            app.zoom_wheel(hiword(w) as i16 as i32);
            return 0;
        }
        // Rows move under a mouse that stays put.
        WM_MOUSEWHEEL | WM_VSCROLL | WM_HSCROLL => {
            let r = m.def();
            hover::scrolled(app);
            return r;
        }
        _ => {}
    }
    m.def()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_show_follows_the_starter_and_the_saved_state() {
        // A normal start: maximized when the window was.
        assert_eq!(
            first_show(SW_SHOWNORMAL, false),
            (SW_SHOWNORMAL, false, false)
        );
        assert_eq!(
            first_show(SW_SHOWDEFAULT, true),
            (SW_SHOWMAXIMIZED, false, false)
        );
        assert_eq!(
            first_show(SW_SHOWMAXIMIZED, false),
            (SW_SHOWMAXIMIZED, false, false)
        );
        // Not to be activated: as asked; a maximized state is kept for later.
        assert_eq!(
            first_show(SW_SHOWNOACTIVATE, true),
            (SW_SHOWNOACTIVATE, true, false)
        );
        assert_eq!(first_show(SW_SHOWNA, false), (SW_SHOWNA, false, false));
        assert_eq!(first_show(SW_HIDE, true), (SW_HIDE, true, false));
        // Minimized: restores to maximized when it was.
        assert_eq!(
            first_show(SW_SHOWMINNOACTIVE, true),
            (SW_SHOWMINNOACTIVE, false, true)
        );
        assert_eq!(
            first_show(SW_SHOWMINIMIZED, false),
            (SW_SHOWMINIMIZED, false, false)
        );
    }
}
