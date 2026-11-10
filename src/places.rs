//! Where goz-gui's resizable dialogs and the Search Syntax window open: the
//! size, place and column widths each had when it last closed, kept in the
//! settings file under Everything's keys (`search_history_x`, `_y`,
//! `_wide`, `_high`, `_maximized`, `search_history_column_search_wide`,
//! `..._order`, ...). A dialog Everything keeps no place for (its Select
//! dialogs, the bookmark editor) opens centred on its owner at the size it
//! had. goz-gui adds `<dialog>_dpi`, as `window_dpi` for the window, so a
//! size kept on one monitor is scaled for another.
//!
//! A place comes back clamped to the monitors there are now
//! (`placement::fit`); one now off every monitor opens centred on the
//! owner instead. Column widths are kept at 96 DPI, as the window's are.
//!
//! The window hands the places read from the file to [`start`]; the
//! dialogs [`restore`] theirs as they open and [`keep`] it as they close,
//! which asks the window to write the settings.

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Controls::{
    HDM_GETITEMCOUNT, LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_GETHEADER,
    LVM_SETCOLUMNORDERARRAY, LVM_SETCOLUMNWIDTH,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SW_SHOWMAXIMIZED, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
    SetWindowPos, ShowWindow,
};

use crate::columns::{scale, unscale};
use crate::dialog::center_on_owner;
use crate::placement::{self, Monitor, Rect};
use crate::settings::WindowPos;

/// The windows whose place is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// Organize Search History.
    SearchHistory,
    /// Find Search History (Everything's "Select Search History").
    SearchHistorySelect,
    RunHistory,
    BookmarkOrganize,
    /// Find Bookmark (Everything's "Select Bookmark").
    BookmarkSelect,
    /// The bookmark editor.
    BookmarkEdit,
    /// Help > Search Syntax.
    SearchSyntax,
    /// Tools > Options.
    Options,
    /// Search > Organize Filters.
    FilterOrganize,
    /// F5: Edit > Advanced > Advanced Rename (Everything's `rename_*`).
    Rename,
    /// F5: Edit > Undo History.
    UndoHistory,
}

/// What Everything keeps for a dialog, under which keys.
struct Keys {
    prefix: &'static str,
    /// `<prefix>_x` and `_y`: the dialog opens where it was (else centred
    /// on its owner).
    pos: bool,
    /// `<prefix>_maximized`.
    max: bool,
    /// Before each column's name in its keys.
    column: &'static str,
    /// goz-gui's columns, in their order in the dialog, by Everything's
    /// names; each has `<column><name>_wide`.
    names: &'static [&'static str],
    /// And `<column><name>_order`: the columns can be dragged into another
    /// order.
    order: bool,
}

impl Which {
    pub const ALL: [Which; 11] = [
        Which::SearchHistory,
        Which::SearchHistorySelect,
        Which::RunHistory,
        Which::BookmarkOrganize,
        Which::BookmarkSelect,
        Which::BookmarkEdit,
        Which::SearchSyntax,
        Which::Options,
        Which::FilterOrganize,
        Which::Rename,
        Which::UndoHistory,
    ];

    fn keys(self) -> Keys {
        const SEARCHES: &[&str] = &["search", "count", "date"];
        let k = |prefix, pos, max, column, names, order| Keys {
            prefix,
            pos,
            max,
            column,
            names,
            order,
        };
        match self {
            Which::SearchHistory => k(
                "search_history",
                true,
                true,
                "search_history_column_",
                SEARCHES,
                true,
            ),
            Which::SearchHistorySelect => k(
                "search_history_select",
                false,
                true,
                "search_history_select_column_",
                SEARCHES,
                true,
            ),
            Which::RunHistory => k(
                "run_history",
                true,
                true,
                "run_history_column_",
                &["name", "path", "count", "date"],
                true,
            ),
            // goz-gui's bookmark lists have Everything's first two columns
            // (not Macro and Key, whose keys are left as they are), the
            // second filling the list.
            Which::BookmarkOrganize => k(
                "bookmark_organize",
                true,
                true,
                "bookmark_organize_",
                &["name", "search"],
                false,
            ),
            Which::BookmarkSelect => k(
                "bookmark_select",
                false,
                true,
                "bookmark_select_",
                &["name", "search"],
                false,
            ),
            Which::BookmarkEdit => k("bookmark_edit", false, false, "", &[], false),
            Which::SearchSyntax => k("help_search_syntax", true, true, "", &[], false),
            Which::Options => k("options", true, true, "", &[], false),
            // Everything's first two columns, as the bookmark lists' (not
            // Macro and Key, which goz does not use; their keys are left as
            // they are), the second filling the list.
            Which::FilterOrganize => k(
                "filter_organize",
                true,
                true,
                "filter_organize_",
                &["name", "search"],
                false,
            ),
            // Everything keeps the renamer's place and size, no columns
            // (its lists are text boxes).
            Which::Rename => k("rename", true, true, "", &[], false),
            Which::UndoHistory => k(
                "undo_history",
                true,
                true,
                "undo_history_",
                &["date", "action", "name", "path", "new_name", "new_path"],
                true,
            ),
        }
    }
}

/// A column as kept: its width at 96 DPI (0: the dialog's own) and its
/// place from the left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Column {
    pub width: i32,
    pub order: i32,
}

/// A window's place as kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Place {
    /// The normal (not maximized) rectangle in screen pixels; a width or
    /// height of 0 (Everything's default) is none kept.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    /// The DPI the size was kept at; `None` (Everything's file) takes it as
    /// it is.
    pub dpi: Option<u32>,
    /// One per column of the dialog, in its order there.
    pub columns: Vec<Column>,
}

/// The widest column kept, at 96 DPI.
const MAX_WIDTH: i32 = 10_000;

impl Place {
    /// Whether a size is kept.
    pub fn sized(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// Every window's place, as the settings file holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places([Place; Which::ALL.len()]);

impl Default for Places {
    fn default() -> Places {
        Places(Which::ALL.map(|w| {
            Place {
                columns: (0..w.keys().names.len())
                    .map(|i| Column {
                        width: 0,
                        order: i as i32,
                    })
                    .collect(),
                ..Place::default()
            }
        }))
    }
}

fn number(v: Option<&str>) -> Option<i64> {
    v?.trim().parse().ok()
}

impl Places {
    pub fn get(&self, which: Which) -> &Place {
        &self.0[which as usize]
    }

    pub fn set(&mut self, which: Which, place: Place) {
        self.0[which as usize] = place;
    }

    /// The places in the settings file's keys; what is missing or cannot
    /// be read is none kept (a column: the dialog's own width, its place).
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Places {
        let mut out = Places::default();
        for which in Which::ALL {
            let k = which.keys();
            let p = &mut out.0[which as usize];
            let n = |key: &str| number(get(&format!("{}_{key}", k.prefix)));
            let size = |key: &str| n(key).filter(|v| (0..=100_000).contains(v));
            if let (Some(w), Some(h)) = (size("wide"), size("high")) {
                p.width = w as i32;
                p.height = h as i32;
            }
            if k.pos {
                let at = |key: &str| n(key).filter(|v| v.abs() <= 100_000).unwrap_or(0) as i32;
                p.x = at("x");
                p.y = at("y");
            }
            if k.max {
                p.maximized = n("maximized") == Some(1);
            }
            p.dpi = n("dpi")
                .filter(|d| (48..=1920).contains(d))
                .map(|d| d as u32);
            for (i, name) in k.names.iter().enumerate() {
                let c = |key: &str| number(get(&format!("{}{name}_{key}", k.column)));
                if let Some(w) = c("wide").filter(|w| (0..=i64::from(MAX_WIDTH)).contains(w)) {
                    p.columns[i].width = w as i32;
                }
                if k.order
                    && let Some(o) = c("order").filter(|o| (0..k.names.len() as i64).contains(o))
                {
                    p.columns[i].order = o as i32;
                }
            }
        }
        out
    }

    /// Every key, with its value, in Everything's order.
    pub fn to_keys(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for which in Which::ALL {
            let k = which.keys();
            let p = self.get(which);
            let mut put = |key: String, v: String| out.push((key, v));
            if k.pos {
                put(format!("{}_x", k.prefix), p.x.to_string());
                put(format!("{}_y", k.prefix), p.y.to_string());
            }
            put(format!("{}_wide", k.prefix), p.width.to_string());
            put(format!("{}_high", k.prefix), p.height.to_string());
            if k.max {
                put(
                    format!("{}_maximized", k.prefix),
                    u8::from(p.maximized).to_string(),
                );
            }
            if let Some(dpi) = p.dpi {
                put(format!("{}_dpi", k.prefix), dpi.to_string());
            }
            for (name, c) in k.names.iter().zip(&p.columns) {
                put(format!("{}{name}_wide", k.column), c.width.to_string());
                if k.order {
                    put(format!("{}{name}_order", k.column), c.order.to_string());
                }
            }
        }
        out
    }
}

/// The columns' order as a list view takes it (the column at each place,
/// left to right); `None` unless each place has one column.
fn order_array(columns: &[Column]) -> Option<Vec<i32>> {
    let mut at = vec![-1; columns.len()];
    for (i, c) in columns.iter().enumerate() {
        let slot = at.get_mut(usize::try_from(c.order).ok()?)?;
        if *slot >= 0 {
            return None;
        }
        *slot = i as i32;
    }
    Some(at)
}

/// Where a window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// At this rectangle, screen pixels.
    At(Rect),
    /// This size, centred on the owner.
    Centred(i32, i32),
}

/// Where a window kept as `place` opens, `pos` saying whether it opens
/// where it was. `here` is the DPI where the window is before it moves and
/// that monitor's work area, if it is on one; `min` its smallest size.
/// `None` when no size is kept.
///
/// Where it was, clamped to the monitors (`placement::fit`) when at least
/// half of it is on them; else (or without a place) centred on the owner,
/// at the size scaled to the DPI here, within the work area here.
pub fn opening(
    place: &Place,
    pos: bool,
    monitors: &[Monitor],
    here: (u32, Option<Rect>),
    min: (i32, i32),
) -> Option<Opening> {
    if !place.sized() {
        return None;
    }
    let saved = WindowPos {
        x: place.x,
        y: place.y,
        width: place.width,
        height: place.height,
        dpi: place.dpi,
    };
    let rect = Rect {
        x: place.x,
        y: place.y,
        w: place.width,
        h: place.height,
    };
    let on_screen: i64 = monitors
        .iter()
        .map(|m| {
            let w = (rect.x + rect.w).min(m.area.x + m.area.w) - rect.x.max(m.area.x);
            let h = (rect.y + rect.h).min(m.area.y + m.area.h) - rect.y.max(m.area.y);
            i64::from(w.max(0)) * i64::from(h.max(0))
        })
        .sum();
    let visible = on_screen * 2 >= i64::from(rect.w) * i64::from(rect.h);
    if pos
        && visible
        && let Some(mut r) = placement::fit(&saved, monitors)
    {
        r.w = r.w.max(min.0);
        r.h = r.h.max(min.1);
        return Some(Opening::At(r));
    }
    let (dpi, work) = here;
    let scaled = |v: i32| match place.dpi {
        Some(d) if d != dpi => (i64::from(v) * i64::from(dpi) / i64::from(d.max(1))) as i32,
        _ => v,
    };
    let (mut w, mut h) = (
        scaled(place.width).max(min.0),
        scaled(place.height).max(min.1),
    );
    if let Some(work) = work {
        w = w.min(work.w.max(min.0));
        h = h.min(work.h.max(min.1));
    }
    Some(Opening::Centred(w, h))
}

thread_local! {
    /// The places this thread's window keeps.
    static PLACES: RefCell<Places> = RefCell::new(Places::default());
    /// Asks the window to write its settings.
    static CHANGED: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// The window's places as read from the settings file, and how to ask the
/// window to write them once one changes.
pub fn start(places: Places, changed: Box<dyn Fn()>) {
    PLACES.set(places);
    CHANGED.set(Some(changed));
}

/// The places now, for the settings file.
pub fn all() -> Places {
    PLACES.with_borrow(Places::clone)
}

fn get(which: Which) -> Place {
    PLACES.with_borrow(|p| p.get(which).clone())
}

/// Whether a width is kept for any of `which`'s columns (else its list
/// shows the dialog's own).
pub fn columns_kept(which: Which) -> bool {
    PLACES.with_borrow(|p| p.get(which).columns.iter().any(|c| c.width > 0))
}

/// Keeps `place`; the window writes it a little later if it changed.
fn put(which: Which, place: Place) {
    let changed = PLACES.with_borrow_mut(|p| {
        let new = *p.get(which) != place;
        p.set(which, place);
        new
    });
    if changed {
        CHANGED.with_borrow(|f| {
            if let Some(f) = f {
                f();
            }
        });
    }
}

/// Opens `window` where it was kept, at its size (at least `min`): for a
/// dialog, from its WM_INITDIALOG once it has its template's size and is
/// centred on its owner. `list` (or null) gets its columns' widths and
/// order back. A window kept maximized is shown maximized.
pub fn restore(window: HWND, which: Which, list: HWND, min: (i32, i32)) {
    let place = get(which);
    let k = which.keys();
    let mut now = RECT::default();
    // SAFETY: reads this thread's window and its DPI.
    let dpi = unsafe {
        GetWindowRect(window, &mut now);
        GetDpiForWindow(window).max(96)
    };
    let now = Rect::from_win(&now);
    let monitors = placement::monitors();
    let work = placement::monitor_of(&now, &monitors).map(|i| monitors[i].work);
    match opening(&place, k.pos, &monitors, (dpi, work), min) {
        Some(Opening::At(r)) => {
            // SAFETY: moves this thread's window: first onto the monitor
            // (which may scale it for that monitor's DPI), then sizes it
            // for the DPI there.
            unsafe {
                SetWindowPos(
                    window,
                    core::ptr::null_mut(),
                    r.x,
                    r.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                SetWindowPos(
                    window,
                    core::ptr::null_mut(),
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        Some(Opening::Centred(w, h)) => {
            // SAFETY: sizes this thread's window.
            unsafe {
                SetWindowPos(
                    window,
                    core::ptr::null_mut(),
                    now.x,
                    now.y,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            center_on_owner(window);
        }
        None => {}
    }
    if !list.is_null() {
        restore_columns(list, &place.columns, k.order);
    }
    if place.sized() && place.maximized && k.max {
        // SAFETY: shows this thread's window maximized; its normal
        // rectangle is the one just set.
        unsafe { ShowWindow(window, SW_SHOWMAXIMIZED) };
    }
}

/// Gives a list its columns' kept widths (at its DPI) and order.
fn restore_columns(list: HWND, columns: &[Column], order: bool) {
    // SAFETY: reads this thread's list's DPI.
    let dpi = unsafe { GetDpiForWindow(list) }.max(96);
    for (i, c) in columns.iter().enumerate() {
        if c.width > 0 {
            // SAFETY: a message to this thread's list.
            unsafe { SendMessageW(list, LVM_SETCOLUMNWIDTH, i, scale(c.width, dpi) as isize) };
        }
    }
    if order && let Some(at) = order_array(columns) {
        // SAFETY: the array of the list's column count outlives the call.
        unsafe {
            SendMessageW(
                list,
                LVM_SETCOLUMNORDERARRAY,
                at.len(),
                at.as_ptr() as isize,
            )
        };
    }
}

/// Keeps where `window` is, its size and whether it is maximized, and
/// `list`'s (or null) columns' widths and order: as it closes
/// (WM_DESTROY), or as the settings are written while it is open.
pub fn keep(window: HWND, which: Which, list: HWND) {
    let mut place = get(which);
    if let Some((pos, maximized)) = placement::normal_place(window) {
        place.x = pos.x;
        place.y = pos.y;
        place.width = pos.width;
        place.height = pos.height;
        place.dpi = pos.dpi;
        place.maximized = maximized && which.keys().max;
    }
    if !list.is_null() {
        // SAFETY: reads this thread's list: its DPI, its header's column
        // count, each width, and the order into an array of that count.
        unsafe {
            let dpi = GetDpiForWindow(list).max(96);
            let header = SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND;
            let n = SendMessageW(header, HDM_GETITEMCOUNT, 0, 0).max(0) as usize;
            let n = n.min(place.columns.len());
            let mut at = vec![0i32; n];
            let ordered =
                SendMessageW(list, LVM_GETCOLUMNORDERARRAY, n, at.as_mut_ptr() as isize) != 0;
            for i in 0..n {
                let width = SendMessageW(list, LVM_GETCOLUMNWIDTH, i, 0) as i32;
                place.columns[i].width = unscale(width, dpi).clamp(0, MAX_WIDTH);
            }
            if ordered {
                for (slot, &col) in at.iter().enumerate() {
                    if let Some(c) = usize::try_from(col)
                        .ok()
                        .and_then(|col| place.columns.get_mut(col))
                    {
                        c.order = slot as i32;
                    }
                }
            }
        }
    }
    put(which, place);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn read(pairs: &[(&str, &str)]) -> Places {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        Places::from_keys(|k| map.get(k).copied())
    }

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn options_keeps_everythings_keys() {
        // As Everything 1.5.0.1424b writes them.
        let p = read(&[
            ("options_x", "604"),
            ("options_y", "300"),
            ("options_wide", "592"),
            ("options_high", "551"),
            ("options_maximized", "1"),
        ]);
        let o = p.get(Which::Options);
        assert_eq!((o.x, o.y, o.width, o.height), (604, 300, 592, 551));
        assert!(o.maximized && o.columns.is_empty());
        let keys: Vec<String> = p.to_keys().into_iter().map(|(k, _)| k).collect();
        for k in [
            "options_x",
            "options_y",
            "options_wide",
            "options_high",
            "options_maximized",
        ] {
            assert!(keys.iter().any(|have| have == k), "{k}");
        }
        // Its tree's width (`options_tree_wide`) is not goz-gui's to write.
        assert!(!keys.iter().any(|k| k.starts_with("options_tree")));
    }

    #[test]
    fn everythings_keys_read_and_write_back() {
        // As Everything 1.5.0.1424b writes them.
        let p = read(&[
            ("search_history_x", "589"),
            ("search_history_y", "316"),
            ("search_history_wide", "622"),
            ("search_history_high", "519"),
            ("search_history_maximized", "0"),
            ("search_history_column_search_wide", "256"),
            ("search_history_column_search_order", "0"),
            ("search_history_column_count_wide", "96"),
            ("search_history_column_count_order", "2"),
            ("search_history_column_date_wide", "128"),
            ("search_history_column_date_order", "1"),
            ("bookmark_select_wide", "0"),
            ("bookmark_select_high", "0"),
            ("bookmark_edit_wide", "496"),
            ("bookmark_edit_high", "665"),
            ("help_search_syntax_maximized", "1"),
        ]);
        let sh = p.get(Which::SearchHistory);
        assert_eq!((sh.x, sh.y, sh.width, sh.height), (589, 316, 622, 519));
        assert!(!sh.maximized && sh.dpi.is_none() && sh.sized());
        assert_eq!(
            sh.columns,
            [
                Column {
                    width: 256,
                    order: 0
                },
                Column {
                    width: 96,
                    order: 2
                },
                Column {
                    width: 128,
                    order: 1
                }
            ]
        );
        assert!(!p.get(Which::BookmarkSelect).sized());
        assert_eq!(p.get(Which::BookmarkEdit).width, 496);
        // Maximized with no size kept: nothing to restore.
        assert!(!p.get(Which::SearchSyntax).sized());
        let keys = p.to_keys();
        let get = |k: &str| {
            keys.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("search_history_x"), Some("589"));
        assert_eq!(get("search_history_column_count_order"), Some("2"));
        assert_eq!(get("run_history_column_path_wide"), Some("0"));
        assert_eq!(get("run_history_column_date_order"), Some("3"));
        // No place for the Select dialogs and the editor, as Everything's.
        assert_eq!(get("bookmark_select_x"), None);
        assert_eq!(get("search_history_select_y"), None);
        assert_eq!(get("bookmark_edit_maximized"), None);
        // goz-gui's bookmark lists cannot be reordered: no order keys.
        assert_eq!(get("bookmark_organize_name_wide"), Some("0"));
        assert_eq!(get("bookmark_organize_name_order"), None);
        assert_eq!(get("bookmark_organize_macro_wide"), None);
        assert_eq!(get("help_search_syntax_dpi"), None);
        // What goz-gui writes reads back the same.
        let mut q = p.clone();
        q.set(
            Which::RunHistory,
            Place {
                x: -1800,
                y: 40,
                width: 900,
                height: 600,
                maximized: true,
                dpi: Some(144),
                columns: vec![
                    Column {
                        width: 200,
                        order: 3,
                    },
                    Column {
                        width: 300,
                        order: 0,
                    },
                    Column {
                        width: 90,
                        order: 1,
                    },
                    Column {
                        width: 140,
                        order: 2,
                    },
                ],
            },
        );
        let pairs = q.to_keys();
        let map: HashMap<&str, &str> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(Places::from_keys(|k| map.get(k).copied()), q);
        // An empty file: none kept.
        assert_eq!(read(&[]), Places::default());
    }

    #[test]
    fn the_file_tools_keep_everythings_keys() {
        // As Everything 1.5.0.1424b writes them.
        let p = read(&[
            ("rename_x", "1016"),
            ("rename_y", "435"),
            ("rename_wide", "528"),
            ("rename_high", "498"),
            ("undo_history_x", "485"),
            ("undo_history_wide", "830"),
            ("undo_history_high", "519"),
            ("undo_history_date_wide", "112"),
            ("undo_history_action_wide", "64"),
            ("undo_history_new_path_order", "5"),
        ]);
        let r = p.get(Which::Rename);
        assert_eq!((r.x, r.y, r.width, r.height), (1016, 435, 528, 498));
        let u = p.get(Which::UndoHistory);
        assert_eq!((u.x, u.width, u.height), (485, 830, 519));
        assert_eq!(u.columns.len(), 6);
        assert_eq!(u.columns[0].width, 112);
        assert_eq!(u.columns[1].width, 64);
        assert_eq!(u.columns[5].order, 5);
        let keys: Vec<String> = p.to_keys().into_iter().map(|(k, _)| k).collect();
        for k in [
            "rename_maximized",
            "undo_history_maximized",
            "undo_history_name_wide",
            "undo_history_new_name_order",
        ] {
            assert!(keys.iter().any(|have| have == k), "{k}");
        }
    }

    #[test]
    fn unreadable_values_are_none_kept() {
        let p = read(&[
            ("run_history_wide", "-5"),
            ("run_history_high", "400"),
            ("run_history_dpi", "7"),
            ("run_history_column_name_wide", "abc"),
            ("run_history_column_name_order", "9"),
        ]);
        let rh = p.get(Which::RunHistory);
        assert!(!rh.sized());
        assert_eq!(rh.dpi, None);
        assert_eq!(rh.columns[0], Column { width: 0, order: 0 });
    }

    #[test]
    fn the_order_is_used_only_when_each_place_has_a_column() {
        let c = |order| Column { width: 0, order };
        assert_eq!(order_array(&[c(0), c(1), c(2)]), Some(vec![0, 1, 2]));
        assert_eq!(order_array(&[c(2), c(0), c(1)]), Some(vec![1, 2, 0]));
        assert_eq!(order_array(&[c(0), c(0), c(1)]), None);
        assert_eq!(order_array(&[c(0), c(3), c(1)]), None);
    }

    fn monitors() -> Vec<Monitor> {
        vec![
            Monitor {
                area: r(0, 0, 1920, 1080),
                work: r(0, 0, 1920, 1032),
                dpi: 96,
            },
            Monitor {
                area: r(-2560, 0, 2560, 1440),
                work: r(-2560, 0, 2560, 1380),
                dpi: 144,
            },
        ]
    }

    fn place(x: i32, y: i32, w: i32, h: i32, dpi: Option<u32>) -> Place {
        Place {
            x,
            y,
            width: w,
            height: h,
            dpi,
            ..Place::default()
        }
    }

    #[test]
    fn a_dialog_opens_where_it_was_within_the_monitors() {
        let m = monitors();
        let here = (96, Some(m[0].work));
        let min = (500, 400);
        // Nothing kept: as it opened.
        assert_eq!(opening(&Place::default(), true, &m, here, min), None);
        assert_eq!(
            opening(&place(100, 80, 700, 500, Some(96)), true, &m, here, min),
            Some(Opening::At(r(100, 80, 700, 500)))
        );
        // Kept at 100 % on the monitor now at 150 %: scaled.
        assert_eq!(
            opening(&place(-2000, 80, 700, 500, Some(96)), true, &m, here, min),
            Some(Opening::At(r(-2000, 80, 1050, 750)))
        );
        // Its title bar above the top of the screen: brought down.
        assert_eq!(
            opening(&place(100, -100, 700, 500, Some(96)), true, &m, here, min),
            Some(Opening::At(r(100, 0, 700, 500)))
        );
        // Never smaller than its template.
        assert_eq!(
            opening(&place(100, 80, 300, 200, Some(96)), true, &m, here, min),
            Some(Opening::At(r(100, 80, 500, 400)))
        );
    }

    #[test]
    fn a_dialog_off_every_monitor_or_without_a_place_opens_centred() {
        let m = monitors();
        let min = (500, 400);
        // Its monitor is gone: centred on the owner, scaled to the DPI
        // there.
        assert_eq!(
            opening(
                &place(5000, 80, 900, 600, Some(144)),
                true,
                &m,
                (96, Some(m[0].work)),
                min
            ),
            Some(Opening::Centred(600, 400))
        );
        // A Select dialog keeps only its size; within the work area.
        assert_eq!(
            opening(
                &place(0, 0, 3000, 2000, None),
                false,
                &m,
                (96, Some(m[0].work)),
                min
            ),
            Some(Opening::Centred(1920, 1032))
        );
        // The owner on no monitor (off-screen): the size alone.
        assert_eq!(
            opening(
                &place(0, 0, 800, 600, Some(96)),
                false,
                &m,
                (144, None),
                min
            ),
            Some(Opening::Centred(1200, 900))
        );
    }
}
