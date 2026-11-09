//! The menu bar, the row context menu, and the command ids they share with
//! the keyboard shortcuts (`keys.rs`).
//!
//! Every item runs a command `App::command` handles. Items whose state
//! changes (checks, radio dots, greyed items, and the File menu's selection
//! part) are set when their menu opens, from a [`State`] the window fills in.

use windows_sys::Win32::UI::WindowsAndMessaging::{
    CheckMenuItem, CreateMenu, CreatePopupMenu, DeleteMenu, EnableMenuItem, GetMenuItemCount,
    HMENU, InsertMenuItemW, MENUITEMINFOW, MF_BYCOMMAND, MF_BYPOSITION, MF_CHECKED, MF_ENABLED,
    MF_GRAYED, MF_UNCHECKED, MFS_GRAYED, MFT_RADIOCHECK, MFT_SEPARATOR, MFT_STRING, MIIM_FTYPE,
    MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SetMenuDefaultItem,
};

use crate::columns::{self, COL_NAME, COLS};
use crate::filter::Filter;
use crate::wide::wz;

// File, and the row context menu.
pub const OPEN: u16 = 1000;
pub const REVEAL: u16 = 1001;
pub const COPY_FULL_PATH: u16 = 1002;
pub const COPY_NAME: u16 = 1003;
/// Copies the Path column: the folder holding the row.
pub const COPY_FOLDER: u16 = 1004;
pub const PROPERTIES: u16 = 1005;
pub const EXIT: u16 = 1006;
pub const COPY_SIZE: u16 = 1007;
pub const COPY_DATE: u16 = 1008;
pub const COPY_TSV: u16 = 1009;
/// Ctrl+Shift+Enter in the list; no menu item.
pub const RUN_AS: u16 = 1010;
/// The row menu's Set Run Count... (while the run history is on).
pub const SET_RUN_COUNT: u16 = 1011;
/// Delete: to the Recycle Bin, or for good with Shift held.
pub const DELETE: u16 = 1012;
/// Rename the focused row in place (F2).
pub const RENAME: u16 = 1013;
/// Shift+Delete in the list; no menu item (File > Delete with Shift held).
pub const DELETE_PERMANENT: u16 = 1014;
/// Copy Property's Extension and Type (while those columns are shown).
pub const COPY_EXT: u16 = 1015;
pub const COPY_TYPE: u16 = 1016;
/// Every result to a file; again while one runs, stops it.
pub const EXPORT: u16 = 1017;

// Edit, and keys with no menu item.
pub const SELECT_ALL: u16 = 1100;
pub const FOCUS_SEARCH: u16 = 1101;
pub const CLEAR_SEARCH: u16 = 1102;
pub const CUT: u16 = 1103;
pub const COPY: u16 = 1104;
pub const NEXT_SELECTED: u16 = 1105;
pub const PREV_SELECTED: u16 = 1106;
/// The clipboard's files into the selected folder (or the selected file's).
pub const PASTE: u16 = 1107;

// View.
pub const REFRESH: u16 = 1200;
pub const STATUS_BAR: u16 = 1201;
pub const FILTER_BAR: u16 = 1202;
pub const ZOOM_IN: u16 = 1203;
pub const ZOOM_OUT: u16 = 1204;
pub const ZOOM_RESET: u16 = 1205;
pub const SORT_NAME: u16 = 1210;
pub const SORT_PATH: u16 = 1211;
pub const SORT_SIZE: u16 = 1212;
pub const SORT_DATE: u16 = 1213;
pub const SORT_ASC: u16 = 1220;
pub const SORT_DESC: u16 = 1221;
pub const THEME_USER_DEFAULT: u16 = 1230;
pub const THEME_STANDARD: u16 = 1231;
pub const THEME_DARK: u16 = 1232;
pub const ON_TOP_NEVER: u16 = 1240;
pub const ON_TOP_ALWAYS: u16 = 1241;
pub const ON_TOP_SEARCHING: u16 = 1242;
/// View > Extra Large, Large and Medium Thumbnails, and Details.
pub const VIEW_EXTRA_LARGE: u16 = 1280;
pub const VIEW_LARGE: u16 = 1281;
pub const VIEW_MEDIUM: u16 = 1282;
pub const VIEW_DETAILS: u16 = 1283;

// The header's menu; each column is COLUMN_FIRST + its id.
pub const COLUMN_FIRST: u16 = 1250;
pub const RESET_COLUMNS: u16 = 1259;
/// The column right-clicked, as wide as its text in view.
pub const SIZE_COLUMN_TO_FIT: u16 = 1260;
pub const SIZE_ALL_COLUMNS_TO_FIT: u16 = 1261;
/// The column right-clicked, off the list (never Name).
pub const REMOVE_COLUMN: u16 = 1262;

// Search; the filters are FILTER_FIRST + their place in `Filter::ALL`.
pub const MATCH_CASE: u16 = 1300;
pub const MATCH_PATH: u16 = 1301;
/// Ctrl+] and Ctrl+[: the next or previous filter; no menu item, as in
/// Everything.
pub const NEXT_FILTER: u16 = 1302;
pub const PREV_FILTER: u16 = 1303;
pub const FILTER_FIRST: u16 = 1310;

// Index.
pub const INDEX_STATUS: u16 = 1400;

// History; its entries are NAV_ENTRY_FIRST + their place in the menu.
pub const NAV_BACK: u16 = 1500;
pub const NAV_FORWARD: u16 = 1501;
pub const NAV_HOME: u16 = 1502;
pub const FIND_SEARCH_HISTORY: u16 = 1503;
pub const ORGANIZE_SEARCH_HISTORY: u16 = 1504;
pub const RESTORE_SESSION: u16 = 1505;
pub const NAV_ENTRY_FIRST: u16 = 1520;
/// The most navigation entries the History menu lists.
pub const NAV_ENTRIES: usize = 20;

// Bookmarks; each bookmark is BOOKMARK_FIRST + its place in the menu.
pub const BOOKMARK_ADD: u16 = 1600;
pub const BOOKMARK_ORGANIZE: u16 = 1601;
pub const BOOKMARK_FIND: u16 = 1602;
pub const BOOKMARK_FIRST: u16 = 2000;
/// The most bookmarks the Bookmarks menu lists.
pub const BOOKMARKS: usize = 1000;

/// The ids the shell's items in the row menu get (`IContextMenu`).
pub const SHELL_FIRST: u16 = 0x7000;
pub const SHELL_LAST: u16 = 0x7fff;

// Tools.
pub const SERVICE_START: u16 = 1700;
pub const SERVICE_STOP: u16 = 1701;
pub const SERVICE_RESTART: u16 = 1702;
pub const RUN_HISTORY: u16 = 1703;
pub const OPTIONS: u16 = 1704;
/// The tray menu's Show Window.
pub const TRAY_SHOW: u16 = 1705;

// Help.
pub const HELP: u16 = 1800;
pub const SEARCH_SYNTAX: u16 = 1801;
pub const WEBSITE: u16 = 1802;
pub const ABOUT: u16 = 1803;

/// The top-level menus, in order, with their access keys.
const TITLES: [&str; 9] = [
    "&File",
    "&Edit",
    "&View",
    "&Search",
    "&Index",
    "Hist&ory",
    "&Bookmarks",
    "&Tools",
    "&Help",
];

/// The access keys of the top-level menus, upper-cased.
#[cfg(test)]
pub fn bar_access_keys() -> Vec<char> {
    TITLES.iter().filter_map(|t| access_key(t)).collect()
}

#[cfg(test)]
fn access_key(text: &str) -> Option<char> {
    let at = text.find('&')?;
    text[at + 1..]
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
}

enum Item<'a> {
    Cmd(u16, &'a str),
    /// One of a group of which exactly one is checked, shown with a dot.
    Radio(u16, &'a str),
    Sep,
    Sub(&'a str, HMENU),
    /// Text that runs nothing, greyed: what an empty folder shows.
    Greyed(&'a str),
}
use Item::{Cmd, Greyed, Radio, Sep, Sub};

fn append(menu: HMENU, item: &Item<'_>) {
    insert(menu, u32::MAX, item);
}

/// Puts `item` at position `pos` of `menu` (`u32::MAX`: at the end).
fn insert(menu: HMENU, pos: u32, item: &Item<'_>) {
    let mut text = Vec::new();
    // SAFETY: the rest of MENUITEMINFOW is plain data.
    let mut mii: MENUITEMINFOW = unsafe { core::mem::zeroed() };
    mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
    mii.fMask = MIIM_FTYPE;
    match *item {
        Cmd(id, label) | Radio(id, label) => {
            text = wz(label);
            mii.fMask |= MIIM_ID | MIIM_STRING;
            mii.fType = if matches!(item, Radio(..)) {
                MFT_STRING | MFT_RADIOCHECK
            } else {
                MFT_STRING
            };
            mii.wID = u32::from(id);
        }
        Sep => mii.fType = MFT_SEPARATOR,
        Greyed(label) => {
            text = wz(label);
            mii.fMask |= MIIM_STRING | MIIM_STATE;
            mii.fType = MFT_STRING;
            mii.fState = MFS_GRAYED;
        }
        Sub(label, sub) => {
            text = wz(label);
            mii.fMask |= MIIM_STRING | MIIM_SUBMENU;
            mii.fType = MFT_STRING;
            mii.hSubMenu = sub;
        }
    }
    if !text.is_empty() {
        mii.dwTypeData = text.as_mut_ptr();
    }
    // SAFETY: appends to a menu this module made; Windows copies the text.
    unsafe { InsertMenuItemW(menu, pos, 1, &mii) };
}

fn popup(items: &[Item<'_>]) -> HMENU {
    // SAFETY: a new, empty menu.
    let menu = unsafe { CreatePopupMenu() };
    for item in items {
        append(menu, item);
    }
    menu
}

/// Deletes the items of `menu` from position `keep` on; submenus go too.
fn truncate(menu: HMENU, keep: usize) {
    // SAFETY: deletes items of a menu of this window by position.
    unsafe {
        let mut n = GetMenuItemCount(menu).max(0) as usize;
        while n > keep {
            n -= 1;
            DeleteMenu(menu, n as u32, MF_BYPOSITION);
        }
    }
}

/// The menu bar, and its popups by name.
#[derive(Clone, Copy)]
pub struct Bar {
    pub bar: HMENU,
    pub file: HMENU,
    pub edit: HMENU,
    pub history: HMENU,
    pub bookmarks: HMENU,
}

/// The History menu's own items, before the entries.
const HISTORY_STATIC: usize = 3;
/// The Bookmarks menu's own items, before the bookmarks.
const BOOKMARKS_STATIC: usize = 3;

/// The menu bar. `run_history`: the run history is on, so Tools lists Run
/// History... first, as Everything's does.
pub fn menu_bar(run_history: bool) -> Bar {
    let file = popup(&[]);
    fill_file(file, false, &[false; COLS]);
    let edit = popup(&[
        Cmd(CUT, "Cu&t\tCtrl+X"),
        Cmd(COPY, "&Copy\tCtrl+C"),
        Cmd(PASTE, "&Paste\tCtrl+V"),
        Sep,
        Cmd(SELECT_ALL, "Select &All\tCtrl+A"),
    ]);
    let sort = sort_menu();
    let theme = popup(&[
        Radio(THEME_USER_DEFAULT, "&User Default"),
        Radio(THEME_STANDARD, "&Standard"),
        Radio(THEME_DARK, "&Dark"),
    ]);
    let on_top = popup(&[
        Radio(ON_TOP_NEVER, "&Never"),
        Radio(ON_TOP_ALWAYS, "&Always"),
        Radio(ON_TOP_SEARCHING, "While &Searching"),
    ]);
    let zoom = popup(&[
        Cmd(ZOOM_IN, "Zoom &In\tCtrl+="),
        Cmd(ZOOM_OUT, "Zoom &Out\tCtrl+-"),
        Sep,
        Cmd(ZOOM_RESET, "&Reset\tCtrl+0"),
    ]);
    let view = popup(&[
        Cmd(FILTER_BAR, "Filt&er Bar"),
        Cmd(STATUS_BAR, "Status &Bar"),
        Sep,
        Radio(VIEW_EXTRA_LARGE, "E&xtra Large Thumbnails\tCtrl+Shift+1"),
        Radio(VIEW_LARGE, "&Large Thumbnails\tCtrl+Shift+2"),
        Radio(VIEW_MEDIUM, "&Medium Thumbnails\tCtrl+Shift+3"),
        Radio(VIEW_DETAILS, "&Details\tCtrl+Shift+6"),
        Sep,
        Sub("&Zoom", zoom),
        Sub("S&ort By", sort),
        Cmd(REFRESH, "&Refresh\tF5"),
        Sep,
        Sub("T&heme", theme),
        Sub("On &Top", on_top),
    ]);
    let search = popup(&[
        Cmd(MATCH_CASE, "Match &Case\tCtrl+I"),
        Cmd(MATCH_PATH, "Match &Path\tCtrl+U"),
        Sep,
    ]);
    for filter in Filter::ALL {
        append(search, &Radio(filter.menu_id(), filter.label()));
    }
    let index = popup(&[Cmd(INDEX_STATUS, "Index &Status...")]);
    let history = popup(&[
        Cmd(NAV_BACK, "Back\tAlt+Left"),
        Cmd(NAV_FORWARD, "Forward\tAlt+Right"),
        Cmd(NAV_HOME, "Home\tAlt+Home"),
    ]);
    let bookmarks = popup(&[
        Cmd(BOOKMARK_ADD, "Add to Bookmarks...\tCtrl+D"),
        Cmd(BOOKMARK_FIND, "Find Bookmark...\tCtrl+B"),
        Cmd(BOOKMARK_ORGANIZE, "Organize Bookmarks...\tCtrl+Shift+B"),
    ]);
    let tools = popup(&[]);
    if run_history {
        // Everything's access key, R, is Restart goz Service's here.
        append(tools, &Cmd(RUN_HISTORY, "Run &History...\tCtrl+J"));
        append(tools, &Sep);
    }
    for item in [
        Cmd(SERVICE_START, "&Start goz Service"),
        Cmd(SERVICE_STOP, "S&top goz Service"),
        Cmd(SERVICE_RESTART, "&Restart goz Service"),
        Sep,
        Cmd(OPTIONS, "&Options...	Ctrl+P"),
    ] {
        append(tools, &item);
    }
    let help = popup(&[
        Cmd(HELP, "goz &Help\tF1"),
        Cmd(SEARCH_SYNTAX, "&Search Syntax"),
        Sep,
        Cmd(WEBSITE, "goz &Website"),
        Sep,
        Cmd(ABOUT, "&About goz-gui\tCtrl+F1"),
    ]);
    // SAFETY: a new, empty menu bar.
    let bar = unsafe { CreateMenu() };
    let menus = [
        file, edit, view, search, index, history, bookmarks, tools, help,
    ];
    for (menu, title) in menus.into_iter().zip(TITLES) {
        append(bar, &Sub(title, menu));
    }
    Bar {
        bar,
        file,
        edit,
        history,
        bookmarks,
    }
}

/// View > Sort By, and the background menu's.
fn sort_menu() -> HMENU {
    popup(&[
        Radio(SORT_NAME, "Name\tCtrl+1"),
        Radio(SORT_PATH, "Path\tCtrl+2"),
        Radio(SORT_SIZE, "Size\tCtrl+3"),
        Radio(SORT_DATE, "Date Modified\tCtrl+6"),
        Sep,
        Radio(SORT_ASC, "&Ascending"),
        Radio(SORT_DESC, "&Descending"),
    ])
}

/// Rebuilds the File menu: Export..., the items that act on the selection
/// only while there is one, then Exit. Copy Property has Extension and
/// Type while `shown` (by column id) says those columns are on the list.
pub fn fill_file(file: HMENU, selection: bool, shown: &[bool; COLS]) {
    truncate(file, 0);
    append(file, &Cmd(EXPORT, "&Export...\tCtrl+S"));
    append(file, &Sep);
    if selection {
        let mut items = vec![
            Cmd(COPY_NAME, "Name"),
            Cmd(COPY_FOLDER, "Path"),
            Cmd(COPY_SIZE, "Size"),
        ];
        if shown[columns::COL_EXT] {
            items.push(Cmd(COPY_EXT, "Extension"));
        }
        if shown[columns::COL_TYPE] {
            items.push(Cmd(COPY_TYPE, "Type"));
        }
        items.extend([
            Cmd(COPY_DATE, "Date Modified"),
            Sep,
            Cmd(COPY_TSV, "Copy as &TSV"),
        ]);
        let copy = popup(&items);
        for item in [
            Cmd(OPEN, "&Open\tEnter"),
            Cmd(REVEAL, "Open Path\tCtrl+Enter"),
            Sep,
            Sub("Copy Property", copy),
            Cmd(COPY_FULL_PATH, "Copy Full Path\tCtrl+Shift+C"),
            Sep,
            Cmd(DELETE, "&Delete\tDel"),
            Cmd(RENAME, "Rena&me\tF2"),
            Cmd(PROPERTIES, "P&roperties\tAlt+Enter"),
            Sep,
        ] {
            append(file, &item);
        }
    }
    append(file, &Cmd(EXIT, "E&xit\tCtrl+Shift+Q"));
}

/// A search or bookmark name as a menu label: `&` shown as itself, an
/// empty search as `(Empty)`.
pub fn label(text: &str) -> String {
    if text.is_empty() {
        return "(Empty)".into();
    }
    text.replace('&', "&&").replace('\t', " ")
}

/// Rebuilds the History menu under Back, Forward and Home: Find and
/// Organize Search History (only while the search history is on, as
/// `search_history` says), Restore Previous Session (only while the session
/// store is on, greyed when `session` says there is none), then the
/// navigation entries, newest first, at most [`NAV_ENTRIES`], as radio
/// items with `current` checked. Each entry runs `NAV_ENTRY_FIRST` + its
/// index in `entries`. Separators between the groups, as in Everything.
pub fn fill_history(
    history: HMENU,
    search_history: bool,
    session: Option<bool>,
    entries: &[String],
    current: Option<usize>,
) {
    truncate(history, HISTORY_STATIC);
    if search_history {
        for item in [
            Sep,
            Cmd(FIND_SEARCH_HISTORY, "Find Search History...	Ctrl+H"),
            Cmd(
                ORGANIZE_SEARCH_HISTORY,
                "Organize Search History...	Ctrl+Shift+H",
            ),
        ] {
            append(history, &item);
        }
    }
    if let Some(saved) = session {
        append(history, &Sep);
        append(history, &Cmd(RESTORE_SESSION, "Restore Previous Session"));
        enable(history, RESTORE_SESSION, saved);
    }
    if entries.is_empty() {
        return;
    }
    append(history, &Sep);
    for (i, entry) in entries.iter().take(NAV_ENTRIES).enumerate() {
        let id = NAV_ENTRY_FIRST + i as u16;
        append(history, &Radio(id, &label(entry)));
        check(history, id, current == Some(i));
    }
}

/// One entry of the Bookmarks menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A bookmark, by its name.
    Bookmark(String),
    Separator,
    /// A folder: its name, and what it holds.
    Folder(String, Vec<Mark>),
}

/// Puts the bookmarks under the Bookmarks menu's own items, in their
/// order: a folder as a submenu ("(Empty)", greyed, when it holds
/// nothing), a separator as one. At most [`BOOKMARKS`] bookmarks; each runs
/// `BOOKMARK_FIRST` + its place among them, depth first.
pub fn fill_bookmarks(bookmarks: HMENU, marks: &[Mark]) {
    truncate(bookmarks, BOOKMARKS_STATIC);
    if marks.is_empty() {
        return;
    }
    append(bookmarks, &Sep);
    let mut next = 0;
    add_marks(bookmarks, marks, &mut next);
}

fn add_marks(menu: HMENU, marks: &[Mark], next: &mut usize) {
    for mark in marks {
        match mark {
            Mark::Bookmark(name) => {
                if *next < BOOKMARKS {
                    let name = label(&name.replace(['\r', '\n'], " "));
                    append(menu, &Cmd(BOOKMARK_FIRST + *next as u16, &name));
                }
                *next += 1;
            }
            Mark::Separator => append(menu, &Sep),
            Mark::Folder(name, inside) => {
                let sub = popup(&[]);
                add_marks(sub, inside, next);
                // SAFETY: counts the items of the menu just made.
                if unsafe { GetMenuItemCount(sub) } <= 0 {
                    append(sub, &Greyed("(Empty)"));
                }
                append(menu, &Sub(&label(&name.replace(['\r', '\n'], " ")), sub));
            }
        }
    }
}

/// What the menus show as checked, chosen or greyed.
pub struct State {
    /// Edit > Cut and Copy have something to act on: selected text while
    /// the search box has the keyboard, selected results otherwise.
    pub can_copy: bool,
    /// Edit > Paste has something to paste: text while the search box has
    /// the keyboard, files (and a folder to put them in) otherwise.
    pub can_paste: bool,
    pub match_case: bool,
    pub match_path: bool,
    pub filter: Filter,
    /// The `SORT_*` id of the sort key.
    pub sort: u16,
    pub ascending: bool,
    pub status_bar: bool,
    pub filter_bar: bool,
    /// The `THEME_*` id of the theme mode.
    pub theme: u16,
    /// The `ON_TOP_*` id of the On Top mode.
    pub on_top: u16,
    /// The `VIEW_*` id of the view.
    pub view: u16,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

fn check(menu: HMENU, id: u16, on: bool) {
    // SAFETY: an item of a menu of this window, by command; a missing one
    // is reported and ignored.
    unsafe {
        CheckMenuItem(
            menu,
            u32::from(id),
            MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED },
        )
    };
}

fn enable(menu: HMENU, id: u16, on: bool) {
    // SAFETY: as in `check`.
    unsafe {
        EnableMenuItem(
            menu,
            u32::from(id),
            MF_BYCOMMAND | if on { MF_ENABLED } else { MF_GRAYED },
        )
    };
}

fn radio(menu: HMENU, ids: &[u16], on: u16) {
    for &id in ids {
        check(menu, id, id == on);
    }
}

/// Sets the checks, dots and greyed items of `menu` and its submenus.
pub fn update(menu: HMENU, s: &State) {
    check(menu, MATCH_CASE, s.match_case);
    check(menu, MATCH_PATH, s.match_path);
    let filters = Filter::ALL.map(Filter::menu_id);
    radio(menu, &filters, s.filter.menu_id());
    radio(menu, &[SORT_NAME, SORT_PATH, SORT_SIZE, SORT_DATE], s.sort);
    radio(
        menu,
        &[SORT_ASC, SORT_DESC],
        if s.ascending { SORT_ASC } else { SORT_DESC },
    );
    check(menu, STATUS_BAR, s.status_bar);
    check(menu, FILTER_BAR, s.filter_bar);
    radio(
        menu,
        &[THEME_USER_DEFAULT, THEME_STANDARD, THEME_DARK],
        s.theme,
    );
    radio(
        menu,
        &[ON_TOP_NEVER, ON_TOP_ALWAYS, ON_TOP_SEARCHING],
        s.on_top,
    );
    radio(
        menu,
        &[VIEW_EXTRA_LARGE, VIEW_LARGE, VIEW_MEDIUM, VIEW_DETAILS],
        s.view,
    );
    enable(menu, COPY, s.can_copy);
    enable(menu, CUT, s.can_copy);
    enable(menu, PASTE, s.can_paste);
    enable(menu, NAV_BACK, s.can_go_back);
    enable(menu, NAV_FORWARD, s.can_go_forward);
}

/// Puts the row menu's own items at the top of `menu`, above the shell's
/// (which go in first: some handlers put theirs at the top whatever they
/// are told): Everything 1.5's Open, Open Path, Copy Name, Copy Full Path,
/// and Set Run Count... while the run history is on (`run_count`), then a
/// separator. Its default item (Open, or Open Path when the click was in
/// the Path column, where a double-click opens the path) is set once the
/// shell's is gone ([`set_default`]).
pub fn add_row_items(menu: HMENU, run_count: bool) {
    let mut items = vec![
        Cmd(OPEN, "&Open"),
        Cmd(REVEAL, "Open &Path"),
        Sep,
        Cmd(COPY_NAME, "Copy &Name"),
        Cmd(COPY_FULL_PATH, "Copy &Full Path"),
        Sep,
    ];
    if run_count {
        items.push(Cmd(SET_RUN_COUNT, "Set Run &Count..."));
        items.push(Sep);
    }
    for (pos, item) in items.iter().enumerate() {
        insert(menu, pos as u32, item);
    }
}

/// A new, empty popup menu.
pub fn empty() -> HMENU {
    popup(&[])
}

/// Makes `id` the menu's default item, in bold.
pub fn set_default(menu: HMENU, id: u16) {
    // SAFETY: an item of a menu of this thread, by command.
    unsafe { SetMenuDefaultItem(menu, u32::from(id), 0) };
}

/// The menu for the list's empty space (or Shift+F10 with nothing
/// selected): Sort By and Refresh.
pub fn background_menu() -> HMENU {
    popup(&[Sub("S&ort By", sort_menu()), Cmd(REFRESH, "&Refresh\tF5")])
}

/// The filter bar's list: every filter, current checked.
pub fn filter_menu(current: Filter) -> HMENU {
    let menu = popup(&[]);
    for filter in Filter::ALL {
        append(menu, &Radio(filter.menu_id(), filter.label()));
    }
    let filters = Filter::ALL.map(Filter::menu_id);
    radio(menu, &filters, current.menu_id());
    menu
}

/// The header's right-click menu, as Everything's: sizing the column
/// right-clicked (`on`, an id; none from the keyboard or past the last
/// column) or all of them, removing it (never Name), a check item per
/// column (Name always on and greyed, as it cannot be hidden), then Reset
/// Columns. `shown[id]` is whether column `id` is on the list.
pub fn header_menu(shown: &[bool; COLS], on: Option<usize>) -> HMENU {
    let menu = popup(&[
        Cmd(SIZE_COLUMN_TO_FIT, "&Size Column to Fit"),
        Cmd(SIZE_ALL_COLUMNS_TO_FIT, "Size &All Columns to Fit"),
        Cmd(REMOVE_COLUMN, "&Remove Column"),
        Sep,
    ]);
    enable(menu, SIZE_COLUMN_TO_FIT, on.is_some());
    enable(menu, REMOVE_COLUMN, on.is_some_and(|id| id != COL_NAME));
    for (id, title) in columns::TITLES.iter().enumerate() {
        let cmd = COLUMN_FIRST + id as u16;
        append(menu, &Cmd(cmd, title));
        check(menu, cmd, shown[id] || id == COL_NAME);
    }
    enable(menu, COLUMN_FIRST + COL_NAME as u16, false);
    append(menu, &Sep);
    append(menu, &Cmd(RESET_COLUMNS, "&Reset Columns"));
    menu
}

/// The column a header menu command shows or hides.
pub fn column_of(cmd: u16) -> Option<usize> {
    let id = usize::from(cmd.checked_sub(COLUMN_FIRST)?);
    (id < COLS).then_some(id)
}

// The id ranges run into nothing else.
const _: () = {
    assert!(ON_TOP_SEARCHING < COLUMN_FIRST);
    assert!(COLUMN_FIRST + COLS as u16 <= RESET_COLUMNS);
    assert!(RESET_COLUMNS < SIZE_COLUMN_TO_FIT);
    assert!(REMOVE_COLUMN < MATCH_CASE);
    assert!(DELETE_PERMANENT < COPY_EXT && COPY_TYPE < SELECT_ALL);
    assert!(PREV_FILTER < FILTER_FIRST);
    assert!(ZOOM_RESET < SORT_NAME);
    assert!(RESET_COLUMNS < VIEW_EXTRA_LARGE);
    assert!(VIEW_DETAILS < MATCH_CASE);
    assert!(FILTER_FIRST + Filter::ALL.len() as u16 <= INDEX_STATUS);
    assert!(NAV_ENTRY_FIRST + NAV_ENTRIES as u16 <= BOOKMARK_ADD);
    assert!(ABOUT < BOOKMARK_FIRST);
    assert!(BOOKMARK_FIRST as usize + BOOKMARKS <= SHELL_FIRST as usize);
    assert!(SHELL_FIRST < SHELL_LAST);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_access_keys_are_distinct() {
        let keys = bar_access_keys();
        assert_eq!(keys, ['F', 'E', 'V', 'S', 'I', 'O', 'B', 'T', 'H']);
    }

    #[test]
    fn header_menu_commands_name_their_columns() {
        for id in 0..COLS {
            assert_eq!(column_of(COLUMN_FIRST + id as u16), Some(id));
        }
        assert_eq!(column_of(RESET_COLUMNS), None);
        assert_eq!(column_of(COLUMN_FIRST - 1), None);
    }

    #[test]
    fn labels_show_text_as_typed() {
        assert_eq!(label(""), "(Empty)");
        assert_eq!(label("R&D notes"), "R&&D notes");
        assert_eq!(label("a\tb"), "a b");
    }
}
