//! Options > Advanced, as Everything's page: every setting goz-gui keeps
//! (its INI key) with its value, "Show only modified settings", "Show
//! settings containing", and the chosen one's value to edit, with Reset to
//! its default; a double-click turns a 0/1 setting over, and
//! `color_filters` opens its editor.
//!
//! A value is checked by reading it back as goz-gui reads the file: one
//! that reads as the setting already was (not a value the key takes) is
//! refused; one read a little differently (a number out of range, `true`
//! for 1) is taken as read. The window's own state (its place, the search,
//! the sort, the columns, the panes, the dialogs' places) is shown but
//! changed only where it is changed: the window, the View menu.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_STATE, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW,
    LVM_DELETEALLITEMS, LVM_ENSUREVISIBLE, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVN_ITEMCHANGED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_NOSORTHEADER, LVS_REPORT, LVS_SHOWSELALWAYS,
    LVS_SINGLESEL, NM_DBLCLK, NMHDR, NMLISTVIEW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_GROUPBOX, EN_CHANGE, ES_AUTOHSCROLL, WM_COMMAND, WM_NOTIFY, WS_BORDER,
    WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::Class;
use crate::options::{
    Invalid, LABEL, Page, Rows, checked, enable, item, order, send, set_check, set_text, text,
};
use crate::settings::Settings;
use crate::store::ini::Ini;
use crate::wide::wz;
use crate::window::App;

/// Everything's control ids.
const MODIFIED: u16 = 1634;
const FILTER: u16 = 1636;
const LIST: u16 = 1637;
const GROUP: u16 = 1640;
const RESET: u16 = 1647;
/// goz-gui's: the value's box, Edit... (color filters), and a note.
const VALUE: u16 = 1643;
const EDIT: u16 = 1650;
const NOTE: u16 = 1651;

/// One setting as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    /// As the window has it now (and as the file will).
    pub was: String,
    pub value: String,
    /// The default, if the key has one (a window's place has none).
    pub default: Option<String>,
    /// Changed by the window only.
    pub read_only: bool,
    /// What the user made or used, not a setting (see [`is_data`]).
    pub data: bool,
}

/// Whether `key` holds what the user made or used rather than a setting:
/// the rename presets, the format histories, the recent file lists, the
/// folders and the list opened last. Restore Defaults leaves these alone
/// (their defaults are empty, so it would throw them away); the row's own
/// Reset still empties one.
pub fn is_data(key: &str) -> bool {
    key.ends_with("_format_history")
        || matches!(
            key,
            "rename_presets"
                | "file_list_history"
                | "last_open_filelist_filename"
                | "rename_last_browse_path"
                | "search_edit_last_located_in"
        )
}

/// The keys of the window's own state, which only the window changes.
pub fn window_keys(s: &Settings) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = [
        "window_x",
        "window_y",
        "window_wide",
        "window_high",
        "window_dpi",
        "maximized",
        "match_case",
        "match_path",
        "search",
        "filter",
        "sort",
        "sort_ascending",
        "last_export_type",
        "options_last_page",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for pairs in [
        s.columns.to_keys(),
        s.view.to_keys(),
        s.docks.to_keys(),
        s.places.to_keys(),
    ] {
        out.extend(pairs.into_iter().map(|(k, _)| k));
    }
    out
}

/// Every setting: its value now, its default, whether only the window
/// changes it; by key.
pub fn rows_of(current: &Settings) -> Vec<Row> {
    let defaults: BTreeMap<String, String> = Settings::default().to_pairs().into_iter().collect();
    let own = window_keys(current);
    let mut rows: Vec<Row> = current
        .to_pairs()
        .into_iter()
        .map(|(key, value)| Row {
            default: defaults.get(&key).cloned(),
            read_only: own.contains(&key),
            data: is_data(&key),
            was: value.clone(),
            value,
            key,
        })
        .collect();
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    rows.dedup_by(|a, b| a.key == b.key);
    rows
}

/// The settings `rows` hold, as goz-gui reads them.
fn read(rows: &[Row]) -> Settings {
    let mut ini = Ini::default();
    for r in rows {
        ini.set(&r.key, &r.value);
    }
    Settings::from_ini(&ini)
}

fn value_of(s: &Settings, key: &str) -> Option<String> {
    s.to_pairs()
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Why [`refused`] refuses a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// It reads as the setting already was: what it reads as.
    ReadsAs(String),
    /// goz-gui cannot read it at all (the reader keeps such text as it is,
    /// so it reads back as typed, but nothing could use it): the reader's
    /// words.
    Unreadable(String),
}

/// The first changed row whose value is not one its key takes: its key and
/// why.
pub fn refused(rows: &[Row]) -> Option<(String, Why)> {
    let changed: Vec<&Row> = rows.iter().filter(|r| r.value != r.was).collect();
    if changed.is_empty() {
        return None;
    }
    let now = read(rows);
    for r in changed {
        if r.key == "color_filters"
            && let Err(e) = crate::colorfilter::parse(&r.value)
        {
            return Some((r.key.clone(), Why::Unreadable(e)));
        }
        let got = value_of(&now, &r.key).unwrap_or_default();
        if got == r.value.trim() {
            continue;
        }
        // Read differently: taken if it still changes the setting.
        let mut alone: Vec<Row> = rows.to_vec();
        for a in alone.iter_mut() {
            if a.key == r.key {
                a.value = a.was.clone();
            }
        }
        let before = value_of(&read(&alone), &r.key).unwrap_or_default();
        if got == before {
            return Some((r.key.clone(), Why::ReadsAs(got)));
        }
    }
    None
}

/// Whether row `i` is a 0/1 switch a double-click turns over: it holds 0
/// or 1, and 2 is no value it takes (as it is for a number).
fn is_switch(rows: &[Row], i: usize) -> bool {
    let Some(r) = rows.get(i) else { return false };
    if !matches!(r.value.trim(), "0" | "1") {
        return false;
    }
    let mut two = rows.to_vec();
    two[i].value = "2".into();
    value_of(&read(&two), &r.key).as_deref() == Some(r.value.trim())
}

pub(super) struct Advanced {
    rows: RefCell<Vec<Row>>,
    /// The rows the filters leave, by place in `rows`.
    shown: RefCell<Vec<usize>>,
    current: Cell<Option<usize>>,
    busy: Cell<bool>,
}

impl Advanced {
    pub(super) fn new() -> Advanced {
        Advanced {
            rows: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
            current: Cell::new(None),
            busy: Cell::new(false),
        }
    }

    fn quietly(&self, f: impl FnOnce()) {
        let was = self.busy.replace(true);
        f();
        self.busy.set(was);
    }

    fn list_rows(&self, page: HWND) {
        let only_modified = checked(page, MODIFIED);
        let words: Vec<String> = text(page, FILTER)
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let shown: Vec<usize> = self
            .rows
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                (!only_modified || r.default.as_ref() != Some(&r.value))
                    && words.iter().all(|w| r.key.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        let rows = self.rows.borrow().clone();
        let list = item(page, LIST);
        let keep = self
            .current
            .get()
            .and_then(|c| shown.iter().position(|&i| i == c));
        self.quietly(|| {
            send(page, LIST, LVM_DELETEALLITEMS, 0, 0);
            for (at, &i) in shown.iter().enumerate() {
                let r = &rows[i];
                let mut key = wz(&r.key);
                let mut lv = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: at as i32,
                    pszText: key.as_mut_ptr(),
                    ..Default::default()
                };
                send(page, LIST, LVM_INSERTITEMW, 0, (&raw mut lv) as isize);
                set_cell(list, at, &r.value);
            }
        });
        *self.shown.borrow_mut() = shown;
        // The one chosen before, or the first: the box below shows a value.
        match keep.or((!self.shown.borrow().is_empty()).then_some(0)) {
            Some(at) => {
                if keep.is_none() {
                    self.current.set(self.shown.borrow().first().copied());
                    self.show_value(page);
                }
                self.select_at(page, at);
            }
            None => {
                self.current.set(None);
                self.show_value(page);
            }
        }
    }

    fn select_at(&self, page: HWND, at: usize) {
        let state = LVIS_SELECTED | LVIS_FOCUSED;
        let mut lv = LVITEMW {
            mask: LVIF_STATE,
            state,
            stateMask: state,
            ..Default::default()
        };
        send(page, LIST, LVM_SETITEMSTATE, at, (&raw mut lv) as isize);
        send(page, LIST, LVM_ENSUREVISIBLE, at, 0);
    }

    /// The chosen setting's value, in the box below the list.
    fn show_value(&self, page: HWND) {
        let row = self
            .current
            .get()
            .and_then(|i| self.rows.borrow().get(i).cloned());
        self.quietly(|| match &row {
            Some(r) => {
                set_text(page, GROUP, &format!("Value for {}", r.key));
                set_text(page, VALUE, &r.value);
                let note = if r.read_only {
                    "The window keeps this one: change it there (the View menu, the window's size, ...)."
                        .to_owned()
                } else {
                    match &r.default {
                        Some(d) if d.is_empty() => "Default: (empty)".to_owned(),
                        Some(d) => format!("Default: {d}"),
                        None => String::new(),
                    }
                };
                set_text(page, NOTE, &note);
            }
            None => {
                set_text(page, GROUP, "Value");
                set_text(page, VALUE, "");
                set_text(page, NOTE, "");
            }
        });
        let editable = row.as_ref().is_some_and(|r| !r.read_only);
        enable(page, VALUE, editable);
        enable(
            page,
            RESET,
            editable
                && row
                    .as_ref()
                    .is_some_and(|r| r.default.is_some() && r.default.as_ref() != Some(&r.value)),
        );
        enable(
            page,
            EDIT,
            row.as_ref().is_some_and(|r| r.key == "color_filters"),
        );
    }

    /// Sets the chosen row's value (typed, reset, turned over) and its
    /// cell in the list.
    fn set_value(&self, page: HWND, value: String) {
        let Some(i) = self.current.get() else { return };
        if let Some(r) = self.rows.borrow_mut().get_mut(i) {
            r.value = value.clone();
        }
        if let Some(at) = self.shown.borrow().iter().position(|&s| s == i) {
            set_cell(item(page, LIST), at, &value);
        }
    }
}

fn set_cell(list: HWND, at: usize, value: &str) {
    let mut v = wz(value);
    let mut lv = LVITEMW {
        iSubItem: 1,
        pszText: v.as_mut_ptr(),
        ..Default::default()
    };
    // SAFETY: a message to this thread's list; the text outlives it.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
            list,
            LVM_SETITEMTEXTW,
            at,
            (&raw mut lv) as isize,
        )
    };
}

impl Page for Advanced {
    fn name(&self) -> &'static str {
        "Advanced"
    }

    fn order(&self) -> u32 {
        order::ADVANCED
    }

    fn build(&self, rows: &mut Rows<'_>) {
        let w = rows.width();
        rows.check(MODIFIED, "Show only &modified settings")
            .control(
                Class::Static,
                "&Show settings containing:",
                LABEL,
                SS_LEFT,
                0,
                (0, 2, 90, 9),
            )
            .control(
                Class::Edit,
                "",
                FILTER,
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
                (90, 0, w - 90, 13),
            )
            .skip(17)
            .control(
                Class::Named("SysListView32"),
                "",
                LIST,
                WS_TABSTOP
                    | WS_BORDER
                    | (LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER),
                0,
                (0, 0, w, 120),
            )
            .skip(124)
            .control(
                Class::Button,
                "Value",
                GROUP,
                BS_GROUPBOX as u32,
                0,
                (0, 0, w, 48),
            )
            .indent(8)
            .skip(12)
            .control(
                Class::Edit,
                "",
                VALUE,
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
                (0, 0, w - 16 - 104, 13),
            )
            .control(
                Class::Button,
                "&Edit...",
                EDIT,
                WS_TABSTOP,
                0,
                (w - 16 - 100, 0, 48, 14),
            )
            .control(
                Class::Button,
                "R&eset",
                RESET,
                WS_TABSTOP,
                0,
                (w - 16 - 48, 0, 48, 14),
            )
            .skip(18)
            .control(Class::Static, "", NOTE, SS_LEFT, 0, (0, 0, w - 16, 9))
            .skip(14)
            .indent(-8);
    }

    fn fill(&self, app: &App, page: HWND) {
        *self.rows.borrow_mut() = rows_of(&app.current_settings());
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        send(
            page,
            LIST,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            ex as usize,
            ex as isize,
        );
        // The name three fifths of the list, the value the rest (less a
        // scroll bar's room).
        let mut rc = windows_sys::Win32::Foundation::RECT::default();
        // SAFETY: reads the size of this page's list.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(item(page, LIST), &mut rc)
        };
        let dpi = crate::custom::window_dpi(page) as i32;
        let room = (rc.right - rc.left - 20 * dpi / 96).max(100);
        let name_w = room * 3 / 5;
        for (i, (name, width)) in [("Name", name_w), ("Value", room - name_w)]
            .into_iter()
            .enumerate()
        {
            let mut t = wz(name);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: width,
                pszText: t.as_mut_ptr(),
                ..Default::default()
            };
            send(page, LIST, LVM_INSERTCOLUMNW, i, (&raw const col) as isize);
        }
        set_check(page, MODIFIED, false);
        self.list_rows(page);
    }

    /// A value typed, Reset, a 0/1 turned over: a change. (The filters,
    /// the list and Edit... are none.)
    fn changed(&self, _page: HWND, id: u16, code: u32) -> bool {
        (id == VALUE && code == EN_CHANGE) || (id == RESET && code == BN_CLICKED)
    }

    fn validate(&self, _page: HWND) -> Result<(), Invalid> {
        match refused(&self.rows.borrow()) {
            None => Ok(()),
            Some((key, why)) => Err(Invalid {
                control: VALUE,
                message: match why {
                    Why::ReadsAs(reads) => format!(
                        "{key} does not take that value (goz-gui reads it as {}).",
                        if reads.is_empty() {
                            "(empty)".to_owned()
                        } else {
                            reads
                        }
                    ),
                    Why::Unreadable(e) => {
                        format!("{key} does not take that value: goz-gui cannot read it ({e}).")
                    }
                },
            }),
        }
    }

    fn message(&self, _app: &App, page: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<isize> {
        if msg == WM_NOTIFY {
            // SAFETY: WM_NOTIFY's lParam is an NMHDR, an NMLISTVIEW for the
            // list's item changes.
            let hdr = unsafe { &*(l as *const NMHDR) };
            if hdr.idFrom != usize::from(LIST) {
                return None;
            }
            if hdr.code == LVN_ITEMCHANGED && !self.busy.get() {
                // SAFETY: as above.
                let nm = unsafe { &*(l as *const NMLISTVIEW) };
                if nm.uNewState & LVIS_SELECTED != 0 && nm.uOldState & LVIS_SELECTED == 0 {
                    let i = self.shown.borrow().get(nm.iItem.max(0) as usize).copied();
                    self.current.set(i);
                    self.show_value(page);
                }
                return Some(0);
            }
            if hdr.code == NM_DBLCLK {
                let Some(i) = self.current.get() else {
                    return Some(0);
                };
                let row = self.rows.borrow().get(i).cloned();
                if let Some(r) = row.filter(|r| !r.read_only) {
                    if r.key == "color_filters" {
                        self.edit_filters(page);
                    } else if is_switch(&self.rows.borrow(), i) {
                        // Through the box, so it counts as a change.
                        set_text(page, VALUE, if r.value == "1" { "0" } else { "1" });
                    }
                }
                return Some(0);
            }
            return None;
        }
        if msg != WM_COMMAND {
            return None;
        }
        let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
        if self.busy.get() {
            return Some(0);
        }
        match (id, code) {
            (MODIFIED, BN_CLICKED) | (FILTER, EN_CHANGE) => {
                self.list_rows(page);
                Some(0)
            }
            (VALUE, EN_CHANGE) => {
                self.set_value(page, text(page, VALUE));
                self.show_value_buttons(page);
                None
            }
            (RESET, BN_CLICKED) => {
                let d = self
                    .current
                    .get()
                    .and_then(|i| self.rows.borrow().get(i).and_then(|r| r.default.clone()));
                if let Some(d) = d {
                    set_text(page, VALUE, &d);
                }
                None
            }
            (EDIT, BN_CLICKED) => {
                self.edit_filters(page);
                Some(0)
            }
            _ => None,
        }
    }

    fn apply(&self, app: &App, page: HWND, problems: &mut Vec<String>) {
        let changed: Vec<(String, String)> = self
            .rows
            .borrow()
            .iter()
            .filter(|r| r.value != r.was && !r.read_only)
            .map(|r| (r.key.clone(), r.value.clone()))
            .collect();
        if changed.is_empty() {
            return;
        }
        app.apply_advanced(&changed, problems);
        // What the window has now (as read, and as the other pages just
        // applied it) is what the next change starts from.
        *self.rows.borrow_mut() = rows_of(&app.current_settings());
        self.list_rows(page);
    }

    /// Everything's Restore Defaults: every setting that has a default and
    /// is not the window's own back to it; the user's data (presets,
    /// histories) stays.
    fn defaults(&self, page: HWND) {
        for r in self.rows.borrow_mut().iter_mut() {
            restore_default(r);
        }
        self.list_rows(page);
    }
}

/// Restore Defaults for one row: a setting that has a default and is not
/// the window's own nor the user's data goes back to it.
fn restore_default(r: &mut Row) {
    if let (false, false, Some(d)) = (r.read_only, r.data, &r.default) {
        r.value = d.clone();
    }
}

impl Advanced {
    fn show_value_buttons(&self, page: HWND) {
        let row = self
            .current
            .get()
            .and_then(|i| self.rows.borrow().get(i).cloned());
        enable(
            page,
            RESET,
            row.is_some_and(|r| {
                !r.read_only && r.default.is_some() && r.default.as_ref() != Some(&r.value)
            }),
        );
    }

    /// `color_filters`' editor; what it gives goes in the box (a change).
    fn edit_filters(&self, page: HWND) {
        let Some(i) = self.current.get() else { return };
        let value = self
            .rows
            .borrow()
            .get(i)
            .map(|r| r.value.clone())
            .unwrap_or_default();
        let rules = crate::colorfilter::parse(&value).unwrap_or_default();
        let look = crate::dialog::app_look();
        if let Some(new) = crate::colorfilter_dialog::edit(page, rules, look) {
            set_text(page, VALUE, &crate::colorfilter::format(&new));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        rows_of(&Settings::default())
    }

    fn set(rows: &mut [Row], key: &str, value: &str) {
        rows.iter_mut().find(|r| r.key == key).unwrap().value = value.into();
    }

    #[test]
    fn every_key_once_with_its_default() {
        let r = rows();
        let sd = r.iter().find(|r| r.key == "search_delay").unwrap();
        assert_eq!(sd.default.as_deref(), Some("0"));
        assert!(!sd.read_only);
        assert!(
            r.iter()
                .any(|r| r.key == "file_new_tab_keys" && r.value == "340")
        );
        assert!(r.iter().any(|r| r.key == "dpi" && r.value == "96"));
        assert!(r.iter().find(|r| r.key == "sort").unwrap().read_only);
        let mut keys: Vec<&str> = r.iter().map(|r| r.key.as_str()).collect();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn values_a_key_takes_pass_others_are_refused() {
        let mut r = rows();
        set(&mut r, "search_delay", "250");
        assert_eq!(refused(&r), None);
        // Clamped to its range: still a change, taken as read.
        set(&mut r, "search_delay", "999999");
        assert_eq!(refused(&r), None);
        // Not a number: reads as it was.
        set(&mut r, "search_delay", "soon");
        assert_eq!(
            refused(&r),
            Some(("search_delay".into(), Why::ReadsAs("0".into())))
        );
        set(&mut r, "search_delay", "0");
        // `true` for a switch reads as 1.
        set(&mut r, "grid_lines", "true");
        assert_eq!(refused(&r), None);
        set(&mut r, "grid_lines", "maybe");
        assert!(refused(&r).is_some());
        set(&mut r, "grid_lines", "0");
        set(&mut r, "normal_foreground_color", "#12345");
        assert!(refused(&r).is_some());
        set(&mut r, "normal_foreground_color", "#123456");
        assert_eq!(refused(&r), None);
    }

    /// A color_filters value goz-gui cannot read reads back as typed (the
    /// reader keeps it) but would drop every filter: it is refused, with
    /// the reader's words.
    #[test]
    fn unreadable_color_filters_are_refused() {
        let mut r = rows();
        let good = r##"[{"filter":"*.dll","include_files":1,"include_folders":0,"foreground_color":"#ff0000","foreground_dark_color":""}]"##;
        set(&mut r, "color_filters", good);
        assert_eq!(refused(&r), None);
        set(&mut r, "color_filters", r#"[{"filter":"*.dll""#);
        assert!(matches!(
            refused(&r),
            Some((k, Why::Unreadable(_))) if k == "color_filters"
        ));
        set(&mut r, "color_filters", "");
        assert_eq!(refused(&r), None);
    }

    /// Restore Defaults leaves the presets, histories and remembered
    /// folders and lists: their defaults are empty.
    #[test]
    fn restore_defaults_keeps_the_users_data() {
        let mut r = rows();
        for (k, v) in [
            ("rename_presets", r#""Swap","a%1","b%1""#),
            ("file_list_history", r"C:\a.efu"),
            ("last_open_filelist_filename", r"C:\a.efu"),
            ("rename_last_browse_path", r"C:\x"),
            ("search_edit_last_located_in", r"C:\y"),
            ("rename_old_format_history", "%1"),
            ("rename_new_format_history", "%1"),
            ("advanced_copy_to_old_format_history", "%1"),
            ("advanced_move_to_new_format_history", "%1"),
            ("search_delay", "250"),
        ] {
            set(&mut r, k, v);
        }
        for row in r.iter_mut() {
            restore_default(row);
        }
        let value = |k: &str| r.iter().find(|r| r.key == k).unwrap().value.clone();
        assert_eq!(value("search_delay"), "0");
        assert_eq!(value("rename_presets"), r#""Swap","a%1","b%1""#);
        assert_eq!(value("file_list_history"), r"C:\a.efu");
        assert_eq!(value("advanced_copy_to_old_format_history"), "%1");
        assert_eq!(value("search_edit_last_located_in"), r"C:\y");
        assert!(r.iter().filter(|r| r.data).count() >= 10);
    }

    #[test]
    fn switches_turn_over_on_a_double_click() {
        let r = rows();
        let at = |k: &str| r.iter().position(|r| r.key == k).unwrap();
        assert!(is_switch(&r, at("grid_lines")));
        assert!(!is_switch(&r, at("search_delay")));
        // Three-way: 0, 1 and 2 are all values.
        assert!(!is_switch(&r, at("date_descending_first")));
    }
}
