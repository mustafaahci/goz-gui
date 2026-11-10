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
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DC_BRUSH, DeleteObject, GetDC, GetStockObject, GetTextMetricsW, HFONT,
    InvalidateRect, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow, ReleaseDC,
    SelectObject, SetBkColor, SetDCBrushColor, SetTextColor, TEXTMETRICW,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD, APPCOMMAND_BROWSER_HOME, MK_CONTROL,
};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, GetSystemMetricsForDpi, SystemParametersInfoForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, DragAcceptFiles, HDROP, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::celltip::{self, CellTip};
use crate::client::{BLOCK_ROWS, Failure, Job, Reply, Search, Worker};
use crate::columns::Layout;
use crate::copytext::{self, Field};
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
use crate::wide::{is_wide, window_text, wz};
use crate::{
    dialog, dropdown, format, header, hover, keys, menu, msgbox, paint, places, searchbox, shell,
    status, syntax, theme,
};

pub use crate::columns::{COL_DATE, COL_EXT, COL_NAME, COL_PATH, COL_SIZE, COL_TYPE, COLS};

mod background;
mod bar;
mod bookmarks;
mod cols;
mod commands;
mod context;
mod docks;
mod edit;
mod export;
mod files;
mod live;
mod nav;
mod opts;
mod prefs;
mod reselect;
mod results;
mod runs;
mod searches;
mod session;
pub mod slots;
mod thumbview;
mod typing;
// Wave 1: each feature declares its window-side modules on its own lines
// below, so their changes never meet (FRAMEWORK.md).

// F1 (preview pane).

// F2 (sidebars).

// F3 (tabs and windows).

// F4 (search tools).

// F5 (file tools).

// F6 (integration and the Options pages).
mod uiopts;

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
    /// The message font at `dpi`, `zoom` % its size (View > Zoom).
    fn new(dpi: u32, zoom: u32) -> Fonts {
        // SAFETY: plain out-structs; the font handles are owned by `Fonts`.
        unsafe {
            let mut ncm: NONCLIENTMETRICSW = core::mem::zeroed();
            ncm.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS,
                ncm.cbSize,
                (&raw mut ncm).cast(),
                0,
                dpi,
            );
            let mut lf = ncm.lfMessageFont;
            if zoom != crate::zoom::DEFAULT {
                lf.lfHeight = crate::zoom::scale(lf.lfHeight, 96, zoom);
                lf.lfWidth = crate::zoom::scale(lf.lfWidth, 96, zoom);
            }
            let normal = CreateFontIndirectW(&lf);
            lf.lfWeight = 700;
            let bold = CreateFontIndirectW(&lf);
            let dc = GetDC(core::ptr::null_mut());
            let old = SelectObject(dc, normal);
            let mut tm: TEXTMETRICW = core::mem::zeroed();
            GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            ReleaseDC(core::ptr::null_mut(), dc);
            Fonts {
                normal,
                bold,
                height: tm.tmHeight,
            }
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: owned GDI objects, no longer selected anywhere once
        // replaced.
        unsafe {
            DeleteObject(self.normal);
            DeleteObject(self.bold);
        }
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
    /// The result list's fonts, at View > Zoom's size: its rows, header,
    /// rename box and the status bar.
    pub fonts: RefCell<Fonts>,
    /// The rest of the window's: the search box, its dropdown, the filter
    /// bar, which Zoom leaves as they are.
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
    /// The system's 48 px and 256 px icon lists (HIMAGELIST), once asked.
    big_icons: Cell<[usize; 2]>,
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
    // Wave 1: each feature adds the state it keeps on its own lines below
    // (and in `App::new`), so their changes never meet (FRAMEWORK.md).

    // F1 (preview pane).

    // F2 (sidebars).

    // F3 (tabs and windows).

    // F4 (search tools).

    // F5 (file tools).

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
    /// Started with `-new-window`: the tray icon, the hotkeys and running
    /// in the background belong to the first goz-gui.
    secondary: Cell<bool>,
}

/// How the program that started goz-gui asked for its window to be shown
/// (a shortcut's Run: Normal, Minimized, Maximized; a test's no-activate).
fn startup_show() -> i32 {
    use windows_sys::Win32::System::Threading::{
        GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW,
    };
    // SAFETY: fills a plain out-struct.
    let si = unsafe {
        let mut si: STARTUPINFOW = core::mem::zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        GetStartupInfoW(&mut si);
        si
    };
    if si.dwFlags & STARTF_USESHOWWINDOW != 0 {
        i32::from(si.wShowWindow)
    } else {
        SW_SHOWNORMAL
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
    if l.exit || l.toggle_window || l.startup || l.new_window == Some(false) {
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
    // One goz-gui per data folder (and -instance name): a second start
    // hands its command line over and ends, unless it is to be a window of
    // its own (`-new-window`, or Options > UI's "Create a new window when
    // running goz-gui", `crate::newwin`).
    let key = crate::instance::key(launch.instance.as_deref());
    let first = crate::instance::claim(&key);
    let secondary = !first && (launch.new_window == Some(true) || new_window_on_start(&launch));
    // If the one holding the instance does not answer, start anyway.
    if !first && !secondary && crate::instance::forward(&key, &args) {
        return 0;
    }
    if launch.exit {
        // Nothing running to close.
        return 0;
    }
    let prefs = prefs::Prefs::load();
    let (searches, history) = searches::Searches::load(&prefs.base);
    let sessions = session::Sessions::load(&prefs.base);
    let marks = bookmarks::Marks::load(&prefs.base);
    let runs = runs::Runs::load(&prefs.base);
    // The window starts as the settings say, or as the previous session
    // left it (`session_store_restore_on_first_new_window`).
    let restored = sessions.at_start();
    let mut settings = match &restored {
        Some(s) => session::start_settings(&prefs.base, s),
        // Home's match options that are always on or off apply too.
        None => prefs.base.home_start(),
    };
    apply_launch_settings(&mut settings, &launch);
    if secondary && let Some(w) = settings.window.as_mut() {
        // A new window opens down and right of the first one's place.
        let off = crate::newwin::OFFSET * w.dpi.unwrap_or(96) as i32 / 96;
        w.x += off;
        w.y += off;
    }
    // Popup menus follow the theme from the first one on.
    theme::set_app_mode(settings.theme);
    // Where the window was, on the monitors there are now.
    let start: Option<Rect> = settings
        .window
        .and_then(|w| placement::fit(&w, &placement::monitors()));
    // SAFETY: the window class and windows are created and used on this
    // thread only; `App` is leaked so every pointer to it stays valid until
    // the process exits.
    unsafe {
        let hinst = GetModuleHandleW(core::ptr::null());
        let class = wz("goz-gui");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            // Double-clicks reach the status bar's indicators.
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
            hIcon: app_icon(GetSystemMetrics(SM_CXICON)),
            hIconSm: app_icon(GetSystemMetrics(SM_CXSMICON)),
            lpszClassName: class.as_ptr(),
            ..core::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let menus = menu::menu_bar(prefs.base.history.run_enabled);
        // Made where it opens, so it gets that monitor's DPI from the start
        // and no DPI change rescales the size worked out for it.
        let (x, y, w, h) = match start {
            Some(r) => (r.x, r.y, r.w, r.h),
            None => (CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT),
        };
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wz("goz").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            core::ptr::null_mut(),
            menus.bar,
            hinst,
            core::ptr::null(),
        );
        if hwnd.is_null() {
            return 1;
        }
        let app: &'static App = Box::leak(App::create(
            hwnd,
            hinst,
            menus,
            prefs,
            &settings,
            (searches, history),
            (sessions, marks, runs),
        ));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as *const App as isize);
        APP.set(Some(app));
        app.secondary.set(secondary);
        app.init(start.is_none());
        // A new window leaves the starts handed over, the tray icon and
        // the hotkeys to the first goz-gui.
        if !secondary {
            crate::instance::listen(&key, hwnd, WM_LAUNCH);
        }
        let taken = app.apply_background(prefs_background(app));
        // -startup (signing in to Windows) starts in the background, with
        // no window; not a new window, which has no tray icon to come back
        // from.
        let asked = if launch.startup && !secondary {
            SW_HIDE
        } else {
            startup_show()
        };
        let (show, deferred, min_to_max) = first_show(asked, settings.maximized);
        app.maximize_deferred.set(deferred);
        if min_to_max {
            let mut wp = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..core::mem::zeroed()
            };
            GetWindowPlacement(hwnd, &mut wp);
            wp.flags = WPF_RESTORETOMAXIMIZED;
            wp.showCmd = show as u32;
            SetWindowPlacement(hwnd, &wp);
        } else {
            ShowWindow(hwnd, show);
        }
        app.start_search(false);
        if let Some(s) = &restored {
            // Its navigation entries, and its place in the results once
            // they come.
            app.begin_view(&s.view);
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

        let accel = keys::accelerators();
        let mut msg: MSG = core::mem::zeroed();
        while GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) > 0 {
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
            let ours = msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd) != 0;
            // Esc stops an export first, wherever the keyboard is.
            if ours
                && msg.message == WM_KEYDOWN
                && msg.wParam == usize::from(VK_ESCAPE)
                && app.cancel_export()
            {
                continue;
            }
            if ours && TranslateAcceleratorW(hwnd, accel, &msg) != 0 {
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
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // A copy or move still running finishes first, and so do the
        // opens handed to the shell (Close window on execute closes right
        // after one).
        app.fileops.finish();
        crate::actions::wait_for_launches(Duration::from_secs(10));
        shell::ole_done();
        msg.wParam as i32
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds the leaked `App` once `run` set it, and
    // null before.
    unsafe {
        let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const App;
        if app.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        (*app).handle(msg, w, l)
    }
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
    // SAFETY: loads a resource of this module by its id; a missing one is
    // reported as null, which callers accept.
    unsafe {
        LoadImageW(
            GetModuleHandleW(core::ptr::null()),
            // MAKEINTRESOURCE(1): the id travels as the pointer's address.
            core::ptr::without_provenance(1),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    }
}

fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    // SAFETY: a message to a window of this thread; any pointer in `l` is
    // the caller's and lives across the synchronous call.
    unsafe { SendMessageW(hwnd, msg, w, l) }
}

fn loword(v: usize) -> u16 {
    (v & 0xffff) as u16
}

fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xffff) as u16
}

fn key_down(vk: u16) -> bool {
    // SAFETY: reads this thread's keyboard state.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
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
    unsafe fn create(
        hwnd: HWND,
        hinst: HINSTANCE,
        menus: menu::Bar,
        prefs: prefs::Prefs,
        s: &Settings,
        (searches, history): (searches::Searches, SearchHistory),
        (sessions, marks, runs): (session::Sessions, bookmarks::Marks, runs::Runs),
    ) -> Box<App> {
        // SAFETY: creates the child controls of `hwnd` on its thread.
        unsafe {
            let edit = CreateWindowExW(
                0,
                wz("EDIT").as_ptr(),
                core::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                hwnd,
                ID_EDIT as HMENU,
                hinst,
                core::ptr::null(),
            );
            let list = CreateWindowExW(
                0,
                WC_LISTVIEWW,
                core::ptr::null(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | LVS_REPORT
                    | LVS_OWNERDATA
                    | LVS_OWNERDRAWFIXED
                    | LVS_SHOWSELALWAYS
                    | LVS_EDITLABELS,
                0,
                0,
                0,
                0,
                hwnd,
                ID_LIST as HMENU,
                hinst,
                core::ptr::null(),
            );
            let header = SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND;
            let dpi = GetDpiForWindow(hwnd);
            let mode = s.theme;
            // The search the window starts with (Home's, or the last one),
            // on one line as the box shows it (a session may hold one of
            // several lines). The window's procedure ignores the change
            // this makes: the App is not attached to it yet.
            if !s.search.is_empty() {
                let search = crate::bookmarks::one_line(&s.search);
                SetWindowTextW(edit, wz(&search).as_ptr());
                let end = search.encode_utf16().count();
                SendMessageW(edit, EM_SETSEL, end, end as isize);
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
                fonts: RefCell::new(Fonts::new(dpi, s.zoom)),
                ui_fonts: RefCell::new(Fonts::new(dpi, crate::zoom::DEFAULT)),
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
                }))),
                icons: Icons::new(hwnd, WM_ICON),
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
                big_icons: Cell::new([0; 2]),
                thumb_grid: Cell::new(None),
                view_hot: Cell::new(None),
                docks: RefCell::new(docks::Docks::new(s.docks)),
                strips: RefCell::new([None, None]),
                strip_capture: Cell::new(None),
                regions: Cell::new(Regions::default()),
                panes_shown: Cell::new(false),
                // Wave 1 (FRAMEWORK.md).

                // F1 (preview pane).

                // F2 (sidebars).

                // F3 (tabs and windows).

                // F4 (search tools).

                // F5 (file tools).

                // F6 (integration and the Options pages).
                ui: Cell::new(s.ui),
                search_dirty: Cell::new(false),
                single_click: Cell::new(false),
                click_select: Cell::new(false),
                secondary: Cell::new(false),
            })
        }
    }

    /// Readies the window's controls; `default_size` sizes the window as
    /// goz-gui does when it has no saved place.
    fn init(&'static self, default_size: bool) {
        let me = self as *const App as usize;
        // SAFETY: `self` is leaked, so the subclass data stays valid.
        unsafe {
            SetWindowSubclass(self.edit, Some(edit::edit_proc), 1, me);
            SetWindowSubclass(self.list, Some(list_proc), 1, me);
            SetWindowSubclass(self.header, Some(header::subclass_proc), 1, me);
            // Files dropped on the window or the search box become the
            // search.
            DragAcceptFiles(self.hwnd, 1);
            DragAcceptFiles(self.edit, 1);
        }
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_HEADERDRAGDROP;
        send(
            self.list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            ex as usize,
            ex as isize,
        );
        // Full row select and one-click opening as Options > Results says,
        // sizes as Options > View says.
        self.apply_list_style();
        crate::format::set_size_format(self.ui.get().size_format);
        self.apply_columns(self.layout.get());
        // The dialogs open where they were; one that moved asks for the
        // settings to be written.
        let places = self.prefs.borrow().base.places.clone();
        places::start(places, Box::new(move || self.settings_changed()));
        let cue = wz("Search");
        send(self.edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
        dropdown::create(self);
        self.tooltip.set(searchbox::create_tooltip(self));
        celltip::create(self);
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
        if default_size {
            // SAFETY: sizes the window for its monitor's DPI before it shows.
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    core::ptr::null_mut(),
                    0,
                    0,
                    self.scale(1000),
                    self.scale(640),
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        self.layout();
        // The start search's caret was put at its end while the box had no
        // width, which scrolled the text out of it: from the start again,
        // then the caret at the end, scrolling only if the text is longer
        // than the box.
        // SAFETY: reads the length of this thread's search box.
        let len = unsafe { GetWindowTextLengthW(self.edit) }.max(0) as usize;
        if len > 0 {
            send(self.edit, EM_SETSEL, 0, 0);
            send(self.edit, EM_SETSEL, len, len as isize);
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

    /// The search box's font (Zoom leaves it as it is).
    pub fn ui_font(&self) -> HFONT {
        self.ui_fonts.borrow().normal
    }

    /// A result row: 19 px at 100 % with the usual 9 pt message font, as
    /// in Everything 1.5; never shorter than the row's icon.
    pub fn row_height(&self) -> i32 {
        let text = self.fonts.borrow().height + self.zscale(4);
        // SAFETY: reads metrics.
        let icon = unsafe { GetSystemMetricsForDpi(SM_CYSMICON, self.dpi.get()) };
        text.max(self.zscale(19)).max(icon + self.scale(1))
    }

    /// The header, its top and bottom lines included: 27 px at 100 %.
    pub fn header_height(&self) -> i32 {
        self.fonts.borrow().height + self.zscale(12)
    }

    fn apply_fonts(&self) {
        let ui = self.ui_font();
        send(self.edit, WM_SETFONT, ui as usize, 1);
        let font = self.fonts.borrow().normal;
        send(self.list, WM_SETFONT, font as usize, 1);
        celltip::set_font(self);
    }

    /// The title bar and taskbar icons at the sizes this DPI wants, which
    /// the class's icons (made for the primary monitor) may not be.
    fn set_window_icons(&self) {
        let dpi = self.dpi.get();
        let old = self.window_icons.get();
        // SAFETY: reads metrics; the icons are this module's resource.
        let new = unsafe {
            [
                app_icon(GetSystemMetricsForDpi(SM_CXSMICON, dpi)),
                app_icon(GetSystemMetricsForDpi(SM_CXICON, dpi)),
            ]
        };
        if new.iter().any(|i| i.is_null()) {
            for icon in new.into_iter().filter(|i| !i.is_null()) {
                // SAFETY: an icon made above and given to no one.
                unsafe { DestroyIcon(icon) };
            }
            return;
        }
        send(self.hwnd, WM_SETICON, ICON_SMALL as usize, new[0] as isize);
        send(self.hwnd, WM_SETICON, ICON_BIG as usize, new[1] as isize);
        self.window_icons.set(new);
        for icon in old.into_iter().filter(|i| !i.is_null()) {
            // SAFETY: icons this window no longer uses, made by this function.
            unsafe { DestroyIcon(icon) };
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
        // SAFETY: repaints this thread's window.
        unsafe { InvalidateRect(self.hwnd, core::ptr::null(), 1) };
    }

    /// Shows or hides the status bar; the list takes its room.
    pub fn set_status_bar_visible(&self, visible: bool) {
        self.status_visible.set(visible);
        self.layout();
        self.settings_changed();
    }

    fn apply_theme(&self) {
        let dark = self.theme_mode.get().is_dark();
        let pal = Palette::new(dark);
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
        send(self.list, LVM_SETBKCOLOR, 0, pal.bg as isize);
        send(self.list, LVM_SETTEXTBKCOLOR, 0, pal.bg as isize);
        send(self.list, LVM_SETTEXTCOLOR, 0, pal.fg as isize);
        syntax::apply_theme(dark);
        dialog::set_app_look(self.dialog_look());
        dropdown::apply_theme(self);
        searchbox::theme_tooltip(self, dark);
        celltip::set_theme(self, dark);
        self.docks_theme(dark);
        theme::flush_menus();
        // SAFETY: repaints this thread's windows.
        unsafe {
            DrawMenuBar(self.hwnd);
            RedrawWindow(
                self.hwnd,
                core::ptr::null(),
                core::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
            );
        }
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
        let mut rc = RECT::default();
        // SAFETY: reads this thread's window's size.
        unsafe { GetClientRect(self.hwnd, &mut rc) };
        let (side, above, below) = (self.scale(3), self.scale(5), self.scale(3));
        let fh = self.ui_fonts.borrow().height;
        let search_h = fh + self.scale(8);
        // The status bar is the result list's: it zooms with it.
        let status_h = if self.status_visible.get() {
            self.fonts.borrow().height + self.zscale(8)
        } else {
            0
        };
        let (tabs_h, find_h) = self.strip_heights();
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
        let edit_top = search.top + (search_h - fh) / 2;
        moves.put(
            self.edit,
            RECT {
                left: edit_left,
                top: edit_top,
                right: edit_left + (edit_right - edit_left).max(0),
                bottom: edit_top + fh,
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
        // SAFETY: repaints this thread's window (its children paint
        // themselves).
        unsafe { InvalidateRect(self.hwnd, core::ptr::null(), 0) };
    }

    unsafe fn handle(&self, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: each arm reads `w`/`l` as the message defines them.
        unsafe {
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
                    }
                    self.panes_window_shown(
                        w != SIZE_MINIMIZED as usize && IsWindowVisible(self.hwnd) != 0,
                    );
                    self.settings_changed();
                    0
                }
                WM_SHOWWINDOW => {
                    if w != 0 {
                        self.arm_live();
                    }
                    self.panes_window_shown(w != 0 && IsIconic(self.hwnd) == 0);
                    DefWindowProcW(self.hwnd, msg, w, l)
                }
                WM_MOVE => {
                    dropdown::close(self);
                    self.settings_changed();
                    DefWindowProcW(self.hwnd, msg, w, l)
                }
                WM_ENTERMENULOOP => {
                    dropdown::close(self);
                    self.in_menu.set(true);
                    DefWindowProcW(self.hwnd, msg, w, l)
                }
                WM_EXITMENULOOP => {
                    self.in_menu.set(false);
                    DefWindowProcW(self.hwnd, msg, w, l)
                }
                WM_PAINT => {
                    paint::window(self);
                    0
                }
                WM_ERASEBKGND => 1,
                WM_CTLCOLOREDIT => {
                    let pal = self.pal.get();
                    SetTextColor(w as _, pal.fg);
                    SetBkColor(w as _, pal.bg);
                    // The edit erases with the brush returned, on its own DC.
                    SetDCBrushColor(w as _, pal.bg);
                    GetStockObject(DC_BRUSH) as LRESULT
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
                    } else if !searchbox::button_down(self, pt)
                        && in_rect(&self.search_rc.get(), pt)
                    {
                        // The frame round the text is the search box too,
                        // its text selected as a click in it would.
                        let had = GetFocus() == self.edit;
                        self.focus_box();
                        if !had && self.ui.get().select_on_click && GetFocus() == self.edit {
                            send(self.edit, EM_SETSEL, 0, -1);
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
                        _ => return DefWindowProcW(self.hwnd, msg, w, l),
                    }
                    1
                }
                WM_MEASUREITEM => {
                    let mis = &mut *(l as *mut MEASUREITEMSTRUCT);
                    if mis.CtlType == ODT_LISTVIEW {
                        mis.itemHeight = self.row_height() as u32;
                    }
                    1
                }
                WM_DRAWITEM => {
                    let dis = &*(l as *const DRAWITEMSTRUCT);
                    if dis.CtlType == ODT_LISTVIEW {
                        paint::row(self, dis);
                    }
                    1
                }
                WM_NOTIFY => self.notify(l),
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
                    } else {
                        DefWindowProcW(self.hwnd, msg, w, l)
                    }
                }
                // The window menu (HIWORD of lParam) is the system's own;
                // the shell's submenus are the shell's (above).
                WM_INITMENUPOPUP => {
                    if hiword(l as usize) == 0 && self.shell_menu.borrow().is_none() {
                        self.init_menu_popup(w as HMENU);
                    }
                    0
                }
                WM_SETCURSOR if u32::from(loword(l as usize)) == HTCLIENT && self.busy_cursor() => {
                    1
                }
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
                WM_SERVICE => {
                    if w != 0 {
                        // gozd stopped or (re)started: search again, which
                        // reconnects, or retries until it is up.
                        self.start_search(true);
                    }
                    0
                }
                WM_TIMER => {
                    KillTimer(self.hwnd, w);
                    if w == TIMER_REFRESH || w == TIMER_RETRY {
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
                    } else if slots::is_feature_timer(w) {
                        self.feature_timer(w);
                    }
                    0
                }
                WM_PANE_CLOSE => {
                    if let Some(id) = crate::dock::PaneId::from_code(w) {
                        self.show_pane(id, false);
                    }
                    0
                }
                m if slots::is_feature_message(m) => self
                    .feature_message(m, w, l)
                    .unwrap_or_else(|| DefWindowProcW(self.hwnd, msg, w, l)),
                WM_EXPORT => {
                    if w == crate::export::ENDED {
                        let ended = Box::from_raw(l as *mut crate::export::Ended);
                        self.export_ended(*ended);
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
                    let text = Box::from_raw(l as *mut String);
                    self.store_report(*text, w == prefs::STORE_FAILED);
                    0
                }
                WM_HISTORY => {
                    let written = Box::from_raw(l as *mut crate::history_file::Written);
                    self.history_written(*written);
                    0
                }
                WM_BOOKMARKS => {
                    let note = Box::from_raw(l as *mut Option<String>);
                    self.bookmarks_written(*note);
                    0
                }
                WM_RUNS => {
                    let written = Box::from_raw(l as *mut crate::run_history_file::Written);
                    self.runs_written(*written);
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
                    let done = Box::from_raw(l as *mut fileops::Done);
                    self.file_op_done(*done);
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
                    let reply = Box::from_raw(l as *mut Reply);
                    self.on_reply(*reply);
                    0
                }
                WM_ICON => {
                    let reply = Box::from_raw(l as *mut IconReply);
                    self.on_icon(&reply);
                    0
                }
                WM_THUMB => {
                    let reply = Box::from_raw(l as *mut ThumbReply);
                    self.on_thumb(*reply);
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
                        let focus = GetFocus();
                        // The search box, the results, or a pane's or
                        // strip's window: it has the keyboard back after.
                        if self.frame_of(focus).is_some() {
                            self.last_focus.set(focus);
                        }
                    } else {
                        if to_search {
                            self.last_focus.set(self.edit);
                            PostMessageW(self.hwnd, uiopts::WM_FOCUS_SEARCH, 0, 0);
                        }
                        // A click on the search box that activates the
                        // window selects its text too (select search on
                        // mouse click), though the box had the keyboard.
                        let mut pt = POINT::default();
                        if u32::from(loword(w)) == WA_CLICKACTIVE
                            && self.ui.get().select_on_click
                            && GetCursorPos(&mut pt) != 0
                            && WindowFromPoint(pt) == self.edit
                        {
                            self.click_select.set(true);
                        }
                        self.refresh_if_idle();
                        self.arm_live();
                    }
                    DefWindowProcW(self.hwnd, msg, w, l)
                }
                WM_SETFOCUS => {
                    SetFocus(self.focus_to_restore(self.last_focus.get()));
                    0
                }
                WM_DPICHANGED => {
                    self.on_dpi(u32::from(hiword(w)), &*(l as *const RECT));
                    0
                }
                WM_SETTINGCHANGE => {
                    if self.theme_mode.get() == ThemeMode::UserDefault
                        && l != 0
                        && is_wide(l as *const u16, "ImmersiveColorSet")
                    {
                        self.apply_theme();
                    }
                    DefWindowProcW(self.hwnd, msg, w, l)
                }
                WM_GETMINMAXINFO => {
                    let mmi = &mut *(l as *mut MINMAXINFO);
                    mmi.ptMinTrackSize = POINT {
                        x: self.scale(420),
                        y: self.scale(240),
                    };
                    0
                }
                paint::WM_UAHDRAWMENU if self.pal.get().dark => {
                    paint::menubar(self, &*(l as *const paint::UahMenu));
                    1
                }
                paint::WM_UAHDRAWMENUITEM if self.pal.get().dark => {
                    paint::menubar_item(self, &*(l as *const paint::UahDrawMenuItem));
                    1
                }
                WM_NCPAINT | WM_NCACTIVATE if self.pal.get().dark => {
                    let r = DefWindowProcW(self.hwnd, msg, w, l);
                    paint::menubar_line(self);
                    r
                }
                // A new window (`-new-window`) ends when closed: running in
                // the background and the tray are the first goz-gui's.
                WM_CLOSE if self.background().run_in_background && !self.is_secondary() => {
                    // goz-gui keeps running in the background.
                    self.hide_to_background();
                    0
                }
                WM_SYSCOMMAND
                    if (w & 0xfff0) == SC_MINIMIZE as usize
                        && self.background().minimize_to_tray
                        && self.background().tray_icon()
                        && !self.is_secondary() =>
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
                    let args = Box::from_raw(l as *mut Vec<String>);
                    self.launch(*args);
                    0
                }
                m if m == self.taskbar_created && m != 0 => {
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
                    self.close_all_panes();
                    self.worker.quit();
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(self.hwnd, msg, w, l),
            }
        }
    }

    unsafe fn notify(&self, l: LPARAM) -> LRESULT {
        // SAFETY: `l` points at the notification struct its code names.
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if !hdr.hwndFrom.is_null() && hdr.hwndFrom == self.tooltip.get() {
                if hdr.code == TTN_GETDISPINFOW {
                    searchbox::tooltip_text(self, &mut *(l as *mut NMTTDISPINFOW));
                }
                return 0;
            }
            if hdr.hwndFrom != self.list {
                return 0;
            }
            match hdr.code {
                LVN_GETDISPINFOW => self.disp_info(&mut *(l as *mut NMLVDISPINFOW)),
                LVN_ODCACHEHINT => {
                    let hint = &*(l as *const NMLVCACHEHINT);
                    self.request_rows(hint.iFrom.max(0) as usize, hint.iTo.max(0) as usize);
                }
                LVN_ODFINDITEMW => return -1,
                LVN_BEGINDRAG | LVN_BEGINRDRAG => self.begin_drag(),
                LVN_BEGINLABELEDITW => {
                    let info = &*(l as *const NMLVDISPINFOW);
                    return LRESULT::from(self.begin_rename(info.item.iItem));
                }
                LVN_ENDLABELEDITW => {
                    let info = &*(l as *const NMLVDISPINFOW);
                    self.end_rename(info.item.pszText);
                    // The row changes once the rename is done.
                    return 0;
                }
                LVN_COLUMNCLICK => {
                    let nm = &*(l as *const NMLISTVIEW);
                    if let Some(id) = self.column_id(nm.iSubItem.max(0) as usize) {
                        self.sort_by_column(id);
                    }
                }
                // With one-click opening (Options > Results) the click
                // opens, and the double-click that may follow does not
                // again.
                NM_DBLCLK if !self.single_click_open() => {
                    let nm = &*(l as *const NMITEMACTIVATE);
                    if nm.iItem >= 0 {
                        self.double_click(nm.ptAction);
                    }
                }
                NM_CLICK if self.single_click_open() => {
                    let nm = &*(l as *const NMITEMACTIVATE);
                    // Ctrl and Shift pick rows; a plain click opens.
                    let keys = LVKF_ALT | LVKF_CONTROL | LVKF_SHIFT;
                    if nm.iItem >= 0 && nm.uKeyFlags & keys == 0 {
                        self.double_click(nm.ptAction);
                    }
                }
                // Enter and the list's other keys never reach the list's own
                // handling (list_proc runs them), so no NM_RETURN comes.
                LVN_ITEMCHANGED | LVN_ODSTATECHANGED => {
                    // The user chose other rows: an action waiting for the
                    // rows chosen before is not wanted any more.
                    if !self.restoring.get() && results::selection_touched(l) {
                        self.cancel_fetch();
                    }
                    self.selection_changed();
                    self.invalidate_status();
                    self.pane_event(PaneEvent::SelectionChanged);
                }
                NM_CUSTOMDRAW => {
                    let cd = &*(l as *const NMCUSTOMDRAW);
                    if self.view.get().thumbnails {
                        return self.thumb_custom_draw(cd);
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
    }

    /// Text for accessibility tools (the list's own type-ahead never runs:
    /// `list_proc` keeps the typed characters for goz-gui's).
    fn disp_info(&self, info: &mut NMLVDISPINFOW) {
        if info.item.mask & LVIF_TEXT == 0
            || info.item.pszText.is_null()
            || info.item.cchTextMax <= 0
        {
            return;
        }
        let index = info.item.iItem.max(0) as usize;
        let mut text = Vec::new();
        if let Some(id) = self.column_id(info.item.iSubItem.max(0) as usize) {
            let m = self.model.borrow();
            if let Some(row) = m.row(index) {
                copytext::push(row, Field::of_column(id), &mut text);
            }
        }
        let n = text.len().min(info.item.cchTextMax as usize - 1);
        // SAFETY: the list gave a buffer of `cchTextMax` code units.
        unsafe {
            core::ptr::copy_nonoverlapping(text.as_ptr(), info.item.pszText, n);
            *info.item.pszText.add(n) = 0;
        }
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
            // SAFETY: a timer of this window.
            unsafe { SetTimer(self.hwnd, TIMER_REFRESH, 1000, None) };
            return;
        }
        // SAFETY: timers of this window.
        unsafe {
            KillTimer(self.hwnd, TIMER_REFRESH);
            KillTimer(self.hwnd, TIMER_RETRY);
            if !keep_place {
                // A changed search becomes a navigation entry once it has
                // stayed as it is for a moment.
                SetTimer(self.hwnd, TIMER_NAV, nav::NAV_DELAY_MS, None);
            }
        }
        if !keep_place {
            // An Enter for the search before this one is spent, a note about
            // the rows before these is stale, and so are the files a refresh
            // of them was to select again, and a restored session's place.
            // This search is not made yet.
            self.enter_pending.set(false);
            self.cancel_fetch();
            self.clear_note();
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
        let search = Arc::new(Search {
            query: filter::query(o.filter, o.match_path, &text),
            sort: o.sort,
            match_case: o.match_case,
        });
        let epoch = self.next_epoch.get() + 1;
        self.next_epoch.set(epoch);
        self.model.borrow_mut().pending = Some(Pending {
            epoch,
            search: Arc::clone(&search),
            highlight: self.highlight_for(&text, o.match_case, o.match_path),
            keep_place,
        });
        self.worker.begin(epoch);
        let hidden = self.ui.get().hide_empty && text.trim().is_empty();
        if hidden {
            // Options > Results' "Hide results when the search is empty":
            // gozd is not asked; the empty answer comes as its would, so
            // the window takes it as any other.
            let empty = Box::into_raw(Box::new(Reply::Block {
                epoch,
                block: 0,
                rows: Vec::new(),
                total: 0,
                incomplete: false,
                metadata_pending: false,
                total_size: None,
                elapsed: Duration::ZERO,
            }));
            // SAFETY: posts to this thread's window, which owns the box
            // once the message is queued; if posting fails it is taken
            // back.
            unsafe {
                if PostMessageW(self.hwnd, WM_REPLY, 0, empty as isize) == 0 {
                    drop(Box::from_raw(empty));
                }
            }
        } else {
            self.worker.push(Job::Block {
                epoch,
                block: 0,
                search: Arc::clone(&search),
            });
        }
        if keep_place && !hidden {
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
        let title = if text.is_empty() {
            "goz".to_owned()
        } else {
            format!("{text} - goz")
        };
        // Set only when it changed: a refresh (every live update) would
        // repaint the title bar for nothing.
        if window_text(self.hwnd) != title {
            // SAFETY: a NUL-terminated title for this thread's window.
            unsafe { SetWindowTextW(self.hwnd, wz(&title).as_ptr()) };
        }
        self.apply_on_top();
        if !keep_place {
            // Back and Forward may have somewhere to go now, or not; the
            // filter bar shows the filter, however it was chosen.
            searchbox::invalidate_nav(self);
            self.invalidate_filter_bar();
            self.pane_event(PaneEvent::SearchChanged);
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
                // SAFETY: reads the length of this thread's search box.
                if unsafe { GetWindowTextLengthW(self.edit) } > 0 {
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
            // SAFETY: plays a system sound.
            unsafe { MessageBeep(MB_ICONWARNING) };
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
                let kept = (refresh || keep_selection).then(|| self.note_selection());
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
                        if !keep {
                            // Other files: their thumbnails are not wanted.
                            self.reset_thumbs();
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
                            self.restore_selection(kept, total);
                        } else if self.enter_pending.take() {
                            self.enter_select();
                        } else if let Some(kept) = kept_search {
                            // Another search: the files selected are
                            // selected again where they are in it, from
                            // its top.
                            self.restore_selection(kept.for_new_search(), total);
                            if total > 0 {
                                send(self.list, LVM_ENSUREVISIBLE, 0, 0);
                            }
                        } else {
                            self.clear_selection();
                            if total > 0 {
                                send(self.list, LVM_ENSUREVISIBLE, 0, 0);
                            }
                        }
                        // SAFETY: repaints the list.
                        unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
                        self.pane_event(PaneEvent::ResultsChanged { refresh: keep });
                    }
                    None => {
                        if old_total != total {
                            self.set_count(total, true);
                        }
                        let first = u64::from(block) * u64::from(BLOCK_ROWS);
                        let last = (first + u64::from(BLOCK_ROWS)).min(total).saturating_sub(1);
                        if first <= last {
                            send(self.list, LVM_REDRAWITEMS, first as usize, last as isize);
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
                    // SAFETY: a timer of this window.
                    unsafe { SetTimer(self.hwnd, TIMER_REFRESH, 1000, None) };
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
                    // SAFETY: a timer of this window.
                    unsafe { SetTimer(self.hwnd, TIMER_RETRY, 2000, None) };
                }
                // SAFETY: repaints the list.
                unsafe { InvalidateRect(self.list, core::ptr::null(), 1) };
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
        send(
            self.list,
            LVM_REDRAWITEMS,
            reply.index,
            reply.index as isize,
        );
    }

    fn set_count(&self, total: u64, keep: bool) {
        let flags = if keep {
            LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL
        } else {
            0
        };
        let count = total.min(i32::MAX as u64) as usize;
        send(self.list, LVM_SETITEMCOUNT, count, flags as isize);
    }

    fn set_state(&self, index: usize, state: u32, mask: u32) {
        let item = LVITEMW {
            state,
            stateMask: mask,
            // SAFETY: the rest of LVITEMW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        send(
            self.list,
            LVM_SETITEMSTATE,
            index,
            (&raw const item) as isize,
        );
    }

    /// No row selected or focused, and no anchor for Shift+click: all three
    /// are row numbers, which a new set of rows gives to other files.
    fn clear_selection(&self) {
        self.set_state(usize::MAX, 0, LVIS_SELECTED | LVIS_FOCUSED);
        send(self.list, LVM_SETSELECTIONMARK, 0, -1);
    }

    pub fn focus_list(&self) {
        // SAFETY: focuses a child of this thread's window.
        unsafe { SetFocus(self.list) };
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0);
        let focused = send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        );
        if count > 0 && focused < 0 {
            self.set_state(
                0,
                LVIS_SELECTED | LVIS_FOCUSED,
                LVIS_SELECTED | LVIS_FOCUSED,
            );
            send(self.list, LVM_ENSUREVISIBLE, 0, 0);
        }
    }

    pub fn invalidate_status(&self) {
        let rc = self.status_rc.get();
        // SAFETY: repaints part of this window.
        unsafe { InvalidateRect(self.hwnd, &rc, 0) };
    }

    /// Selected row indices, up to `cap`.
    pub fn selected(&self, cap: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = usize::MAX;
        while out.len() < cap {
            let next = send(self.list, LVM_GETNEXTITEM, at, LVNI_SELECTED as isize);
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
            // gozd sorts by these four only: the sort stays, and the status
            // bar says why for a moment.
            _ => {
                let title = crate::columns::TITLES.get(col).copied().unwrap_or("it");
                self.show_note(format!("goz can't sort by {title} yet"));
                return;
            }
        });
    }

    fn set_sort(&self, sort: SortSpec) {
        let mut o = self.opts.get();
        o.sort = sort;
        self.opts.set(o);
        self.show_sort();
        self.start_search(false);
    }

    /// Repaints what shows the sort: the header's arrow and shaded item
    /// (header.rs draws them, not the header's HDF_SORT* flags), and the
    /// sorted column's cells.
    fn show_sort(&self) {
        // SAFETY: invalidates this thread's windows.
        unsafe {
            InvalidateRect(self.header, core::ptr::null(), 0);
            InvalidateRect(self.list, core::ptr::null(), 0);
        }
    }

    /// The column the results are sorted by.
    pub fn sorted_column(&self) -> usize {
        match self.opts.get().sort.key {
            SortKey::Name => COL_NAME,
            SortKey::Path => COL_PATH,
            SortKey::Size => COL_SIZE,
            SortKey::DateModified => COL_DATE,
        }
    }

    pub fn sort_ascending(&self) -> bool {
        self.opts.get().sort.dir == SortDir::Asc
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
        *self.fonts.borrow_mut() = Fonts::new(dpi, self.zoom.get());
        *self.ui_fonts.borrow_mut() = Fonts::new(dpi, crate::zoom::DEFAULT);
        self.apply_fonts();
        self.rescale_columns();
        // SAFETY: moves this thread's window to the rectangle Windows chose.
        unsafe {
            SetWindowPos(
                self.hwnd,
                core::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.set_window_icons();
        self.docks_dpi(dpi);
        self.layout();
        // A pane's title row follows the font: its content is placed again
        // even where the pane kept its size.
        self.docks_place_all();
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
        let mut order = [0i32; COLS];
        send(
            self.list,
            LVM_GETCOLUMNORDERARRAY,
            n,
            order.as_mut_ptr() as isize,
        );
        let mut out = Cells {
            cells: [(0, RECT::default()); COLS],
            len: 0,
        };
        let mut x = row.left;
        for &index in &order[..n] {
            let index = (index.max(0) as usize).min(n.saturating_sub(1));
            let Some(&id) = cols.ids().get(index) else {
                continue;
            };
            let width = send(self.list, LVM_GETCOLUMNWIDTH, index, 0) as i32;
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
        let mut rc = RECT::default();
        // SAFETY: reads the header's place on the screen.
        unsafe { GetWindowRect(self.header, &mut rc) };
        in_rect(&rc, pt)
    }
}

/// The shown columns of a row: each one's id and rectangle, left to right.
pub struct Cells {
    cells: [(usize, RECT); COLS],
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

unsafe extern "system" fn list_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the leaked `App`; `l` is read as each message defines.
    unsafe {
        let app = &*(data as *const App);
        match msg {
            // The header reports its custom draw to the list, its parent,
            // and the user's drags and resizes of its items.
            WM_NOTIFY => {
                let hdr = &*(l as *const NMHDR);
                if hdr.hwndFrom == app.header {
                    match hdr.code {
                        NM_CUSTOMDRAW => {
                            return header::custom_draw(app, &*(l as *const NMCUSTOMDRAW));
                        }
                        HDN_ENDDRAG | HDN_ENDTRACKW | HDN_ITEMCHANGEDW | HDN_DIVIDERDBLCLICKW => {
                            let r = DefSubclassProc(hwnd, msg, w, l);
                            app.columns_touched();
                            return r;
                        }
                        _ => {}
                    }
                }
            }
            WM_PAINT => {
                let r = DefSubclassProc(hwnd, msg, w, l);
                paint::empty_list(app);
                return r;
            }
            WM_MOUSEMOVE => {
                let pt = POINT {
                    x: (l & 0xffff) as i16 as i32,
                    y: ((l >> 16) & 0xffff) as i16 as i32,
                };
                hover::moved(app, pt);
            }
            WM_MOUSELEAVE => hover::left(app),
            WM_LBUTTONDOWN => {
                celltip::hide(app);
                app.last_click.set(point(l));
            }
            WM_RBUTTONDOWN | WM_MBUTTONDOWN => celltip::hide(app),
            // Rename's box, the list's child, in the list's colours.
            WM_CTLCOLOREDIT => {
                let pal = app.pal.get();
                SetTextColor(w as _, pal.fg);
                SetBkColor(w as _, pal.bg);
                SetDCBrushColor(w as _, pal.bg);
                return GetStockObject(DC_BRUSH) as LRESULT;
            }
            // The list taking the keyboard means the search was the one
            // wanted.
            WM_SETFOCUS => {
                let r = DefSubclassProc(hwnd, msg, w, l);
                app.reset_type_ahead();
                app.record_search();
                return r;
            }
            WM_KEYDOWN => {
                celltip::hide(app);
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
                let r = DefSubclassProc(hwnd, msg, w, l);
                hover::scrolled(app);
                return r;
            }
            // The characters of the keys above (Enter, Esc, Ctrl+A…, and
            // Ctrl+Backspace's DEL) do nothing more.
            WM_CHAR if w < 0x20 || w == 0x7f => return 0,
            // Printable ones go to goz-gui's own type-ahead, never the
            // list's, which would ask for rows it cannot search
            // (LVN_ODFINDITEMW).
            WM_CHAR => {
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
                let r = DefSubclassProc(hwnd, msg, w, l);
                hover::scrolled(app);
                return r;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
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
