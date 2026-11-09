//! Bookmarks > Find Bookmark... (Ctrl+B), "Select Bookmark" as Everything
//! calls it: every bookmark in one list, `Name | Search`, a bookmark in a
//! folder named with the folder's path (`Music/Rock/Queen`), in the order
//! the Bookmarks menu shows them. The box above filters the list as it is
//! typed (any part of the name or the search, any case); Up, Down, Page Up
//! and Page Down in it move in the list. Open (Enter, a double-click) opens
//! the bookmark and closes; Copy Search puts its search on the clipboard.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW,
    LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVM_SETSELECTIONMARK,
    LVN_GETDISPINFOW, LVN_ITEMCHANGED, LVN_KEYDOWN, LVN_ODFINDITEMW, LVN_ODSTATECHANGED,
    LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP, LVS_OWNERDATA,
    LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, LVSCW_AUTOSIZE_USEHEADER, NM_CUSTOMDRAW,
    NM_DBLCLK, NMHDR, NMITEMACTIVATE, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVKEYDOWN,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows_sys::Win32::UI::Shell::SetWindowSubclass;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, DM_SETDEFID, DWLP_MSGRESULT, EN_CHANGE, ES_AUTOHSCROLL, EndDialog,
    GWLP_USERDATA, GetClientRect, GetWindowRect, IDCANCEL, IDOK, MINMAXINFO, MapDialogRect,
    MoveWindow, SetWindowLongPtrW, WM_COMMAND, WM_DESTROY, WM_GETMINMAXINFO, WM_INITDIALOG,
    WM_NOTIFY, WM_SIZE, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX, WS_TABSTOP, WS_THICKFRAME,
};

use super::{Host, WM_REFRESH, enable, item, send};
use crate::bookmarks::tree::{TOP, Tree};
use crate::bookmarks::{Bookmarks, Settings, one_line};
use crate::dialog::{
    Class, Look, Template, apply_theme, center_on_owner, common, list_custom_draw, state,
};
use crate::history_dialogs::filter_proc;
use crate::places::{self, Which};
use crate::wide::{window_text, wz};

/// What a bookmark opens: its search (`None`: the one in the box stays)
/// and its options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub search: Option<String>,
    pub settings: Settings,
}

/// A bookmark as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its folders' names and its own, joined by `/`.
    pub label: String,
    /// Its search, on one line.
    pub search: String,
    pub chosen: Chosen,
}

/// Every bookmark of `list`, in the menu's order.
pub fn entries(list: &Bookmarks) -> Vec<Entry> {
    let tree = Tree::build(list);
    tree.bookmarks_below(list, TOP)
        .into_iter()
        .map(|r| {
            let row = &list.rows()[r];
            let mut path = row.folder();
            path.push(one_line(row.name()));
            Entry {
                label: path.join("/"),
                search: one_line(row.search()),
                chosen: Chosen {
                    search: row.search_set().then(|| row.search().to_owned()),
                    settings: row.settings(),
                },
            }
        })
        .collect()
}

/// The entries whose name (with its folders) or search holds `filter`, in
/// any case.
pub fn view(all: &[Entry], filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    (0..all.len())
        .filter(|&i| {
            all[i].label.to_lowercase().contains(&needle)
                || all[i].search.to_lowercase().contains(&needle)
        })
        .collect()
}

const ID_FILTER: u16 = 100;
const ID_LIST: u16 = 101;
const ID_OPEN: u16 = 102;
const ID_COPY: u16 = 103;
const STATIC: u16 = 0xffff;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;
/// The columns: title and width at 96 DPI.
const COLUMNS: [(&str, i32); 2] = [("Name", 180), ("Search", 256)];

struct Find<'a> {
    host: &'a dyn Host,
    look: Cell<Look>,
    generation: Cell<u64>,
    all: RefCell<Vec<Entry>>,
    shown: RefCell<Vec<usize>>,
    /// The list's selected row, as last seen.
    selected_row: Cell<Option<usize>>,
    min: Cell<(i32, i32)>,
    chosen: RefCell<Option<Chosen>>,
}

/// Shows Find Bookmark over `owner`; what Open chose.
pub fn find(owner: HWND, host: &dyn Host, look: Look) -> Option<Chosen> {
    let state = Find {
        host,
        look: Cell::new(look),
        generation: Cell::new(u64::MAX),
        all: RefCell::new(Vec::new()),
        shown: RefCell::new(Vec::new()),
        selected_row: Cell::new(None),
        min: Cell::new((0, 0)),
        chosen: RefCell::new(None),
    };
    let (cx, cy) = (300, 220);
    let mut t = Template::new("Select Bookmark", WS_THICKFRAME | WS_MAXIMIZEBOX, cx, cy);
    t.control(Class::Static, "&Search:", STATIC, 0, 0, (7, 9, 28, 8))
        .control(
            Class::Edit,
            "",
            ID_FILTER,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (36, 7, cx - 43, 12),
        )
        .control(
            Class::Named("SysListView32"),
            "",
            ID_LIST,
            WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SINGLESEL,
            WS_EX_CLIENTEDGE,
            (7, 24, cx - 14, cy - 52),
        );
    for (id, text, style) in [
        (ID_OPEN, "&Open", WS_TABSTOP | BS_DEFPUSHBUTTON as u32),
        (ID_COPY, "&Copy Search", WS_TABSTOP),
        (CANCEL, "Close", WS_TABSTOP),
    ] {
        t.control(Class::Button, text, id, style, 0, (0, 0, 50, 14));
    }
    t.run(owner, Some(find_proc), (&raw const state) as LPARAM);
    state.chosen.into_inner()
}

impl Find<'_> {
    fn list(dlg: HWND) -> HWND {
        item(dlg, ID_LIST)
    }

    fn init(&self, dlg: HWND) {
        let list = Find::list(dlg);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        // SAFETY: reads this thread's dialog's DPI.
        let dpi = unsafe { GetDpiForWindow(dlg) }.max(96) as i32;
        for (i, (title, width)) in COLUMNS.into_iter().enumerate() {
            let mut text = wz(title);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: width * dpi / 96,
                pszText: text.as_mut_ptr(),
                // SAFETY: the rest of LVCOLUMNW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(list, LVM_INSERTCOLUMNW, i, (&raw const col) as isize);
        }
        // SAFETY: subclasses this thread's filter box; the list outlives it
        // (both go with the dialog), and the subclass removes itself.
        unsafe { SetWindowSubclass(item(dlg, ID_FILTER), Some(filter_proc), 1, list as usize) };
        apply_theme(dlg, self.look.get().dark);
        center_on_owner(dlg);
        let mut wr = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetWindowRect(dlg, &mut wr) };
        self.min.set((wr.right - wr.left, wr.bottom - wr.top));
        places::restore(dlg, Which::BookmarkSelect, list, self.min.get());
        self.layout(dlg);
        send(dlg, DM_SETDEFID, usize::from(ID_OPEN), 0);
        self.host.showing(dlg);
        self.reload(dlg);
    }

    fn layout(&self, dlg: HWND) {
        let px = |x: i32, y: i32| {
            let mut r = RECT {
                left: 0,
                top: 0,
                right: x,
                bottom: y,
            };
            // SAFETY: converts within this thread's dialog.
            unsafe { MapDialogRect(dlg, &mut r) };
            (r.right, r.bottom)
        };
        let mut rc = RECT::default();
        // SAFETY: reads this thread's dialog.
        unsafe { GetClientRect(dlg, &mut rc) };
        let (w, h) = (rc.right, rc.bottom);
        let (m, _) = px(7, 7);
        let (_, my) = px(0, 7);
        let (bw, bh) = px(50, 14);
        let (wide, _) = px(60, 0);
        let (gap, _) = px(4, 0);
        let (label, top) = px(29, 24);
        let (_, edit_h) = px(0, 12);
        let place = |id: u16, x: i32, y: i32, cw: i32, ch: i32| {
            // SAFETY: moves a control of this thread's dialog.
            unsafe { MoveWindow(item(dlg, id), x, y, cw.max(0), ch.max(0), 1) };
        };
        place(ID_FILTER, m + label, my, w - 2 * m - label, edit_h);
        let by = h - my - bh;
        place(ID_LIST, m, top, w - 2 * m, by - my - top);
        // The Search column takes what the Name column leaves.
        send(
            Find::list(dlg),
            LVM_SETCOLUMNWIDTH,
            1,
            LVSCW_AUTOSIZE_USEHEADER as isize,
        );
        let mut x = w - m;
        for (id, width) in [(CANCEL, bw), (ID_COPY, wide), (ID_OPEN, bw)] {
            x -= width;
            place(id, x, by, width, bh);
            x -= gap;
        }
    }

    /// Reads the bookmarks again, keeping the selected one.
    fn reload(&self, dlg: HWND) {
        let keep = self.current().map(|i| self.all.borrow()[i].label.clone());
        let (list, generation) = self.host.bookmarks();
        self.generation.set(generation);
        *self.all.borrow_mut() = entries(&list);
        self.reshow(dlg, keep.as_deref());
    }

    fn reshow(&self, dlg: HWND, keep: Option<&str>) {
        let filter = window_text(item(dlg, ID_FILTER));
        let (n, select) = {
            let all = self.all.borrow();
            let rows = view(&all, &filter);
            let select = keep
                .and_then(|k| rows.iter().position(|&i| all[i].label == k))
                .or((!rows.is_empty()).then_some(0));
            let n = rows.len();
            *self.shown.borrow_mut() = rows;
            (n, select)
        };
        // The list's notifications come back into this dialog: no borrow
        // is held from here on.
        let list = Find::list(dlg);
        send(list, LVM_SETITEMCOUNT, n, 0);
        let state = |row: usize, on: u32| {
            let it = LVITEMW {
                state: on,
                stateMask: LVIS_SELECTED | LVIS_FOCUSED,
                // SAFETY: the rest of LVITEMW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(list, LVM_SETITEMSTATE, row, (&raw const it) as isize);
        };
        state(usize::MAX, 0);
        if let Some(i) = select {
            state(i, LVIS_SELECTED | LVIS_FOCUSED);
            send(list, LVM_SETSELECTIONMARK, 0, i as isize);
            send(list, LVM_ENSUREVISIBLE, i, 0);
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(list, core::ptr::null(), 1) };
        self.update_buttons(dlg);
    }

    /// The selected entry.
    fn current(&self) -> Option<usize> {
        self.selected_row
            .get()
            .and_then(|row| self.shown.borrow().get(row).copied())
    }

    fn update_buttons(&self, dlg: HWND) {
        let row = send(
            Find::list(dlg),
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        );
        self.selected_row.set(usize::try_from(row).ok());
        let any = self.current().is_some();
        enable(dlg, ID_OPEN, any);
        enable(dlg, ID_COPY, any);
    }

    fn open(&self, dlg: HWND) {
        let Some(i) = self.current() else { return };
        *self.chosen.borrow_mut() = Some(self.all.borrow()[i].chosen.clone());
        // SAFETY: ends this modal dialog.
        unsafe { EndDialog(dlg, 1) };
    }

    fn copy(&self) {
        let Some(i) = self.current() else { return };
        let all = self.all.borrow();
        let c = &all[i].chosen;
        let text = c.search.clone().unwrap_or_default();
        drop(all);
        // The host says so when the clipboard is busy.
        self.host.copy(&text);
    }

    fn disp_info(&self, info: &mut NMLVDISPINFOW) {
        if info.item.mask & LVIF_TEXT == 0
            || info.item.pszText.is_null()
            || info.item.cchTextMax <= 0
        {
            return;
        }
        let text: Vec<u16> = {
            let shown = self.shown.borrow();
            let all = self.all.borrow();
            let entry = usize::try_from(info.item.iItem)
                .ok()
                .and_then(|row| shown.get(row))
                .and_then(|&i| all.get(i));
            match (entry, info.item.iSubItem) {
                (Some(e), 0) => e.label.encode_utf16().collect(),
                (Some(e), 1) => e.search.encode_utf16().collect(),
                _ => Vec::new(),
            }
        };
        let n = text.len().min(info.item.cchTextMax as usize - 1);
        // SAFETY: the list gave a buffer of `cchTextMax` code units.
        unsafe {
            core::ptr::copy_nonoverlapping(text.as_ptr(), info.item.pszText, n);
            *info.item.pszText.add(n) = 0;
        }
    }

    /// Answers the list's notifications; `Some` is DWLP_MSGRESULT's value.
    ///
    /// # Safety
    /// `l` is a WM_NOTIFY's lParam from the list.
    unsafe fn notify(&self, dlg: HWND, l: LPARAM) -> Option<isize> {
        // SAFETY: as the caller promises; each code's struct is read as
        // the list sends it.
        unsafe {
            let hdr = &*(l as *const NMHDR);
            match hdr.code {
                NM_CUSTOMDRAW => {
                    let cd = &*(l as *const NMLVCUSTOMDRAW);
                    return Some(list_custom_draw(hdr.hwndFrom, cd, self.look.get().dark));
                }
                LVN_GETDISPINFOW => self.disp_info(&mut *(l as *mut NMLVDISPINFOW)),
                // No type-ahead: the filter box is for finding.
                LVN_ODFINDITEMW => return Some(-1),
                LVN_ITEMCHANGED | LVN_ODSTATECHANGED => self.update_buttons(dlg),
                NM_DBLCLK => {
                    if (*(l as *const NMITEMACTIVATE)).iItem >= 0 {
                        self.open(dlg);
                    }
                }
                LVN_KEYDOWN => {
                    let ctrl = GetKeyState(i32::from(VK_CONTROL)) < 0;
                    if ctrl && (*(l as *const NMLVKEYDOWN)).wVKey == 0x43 {
                        self.copy();
                    }
                }
                _ => {}
            }
            None
        }
    }
}

unsafe extern "system" fn find_proc(dlg: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: `l` is the `Find` that `find` keeps alive until the
        // dialog has ended.
        unsafe { SetWindowLongPtrW(dlg, GWLP_USERDATA, l) };
    }
    // SAFETY: GWLP_USERDATA is the `Find` from `find`, or null before.
    let Some(s) = (unsafe { state::<Find<'_>>(dlg) }) else {
        return 0;
    };
    if let Some(answer) = common(dlg, msg, w, l, &s.look) {
        return answer;
    }
    match msg {
        WM_INITDIALOG => {
            s.init(dlg);
            // The dialog manager puts the keyboard in the filter box.
            1
        }
        WM_SIZE => {
            s.layout(dlg);
            1
        }
        WM_GETMINMAXINFO => {
            let (w, h) = s.min.get();
            // SAFETY: WM_GETMINMAXINFO's lParam is a MINMAXINFO to fill.
            let mmi = unsafe { &mut *(l as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = w;
            mmi.ptMinTrackSize.y = h;
            1
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lParam is an NMHDR.
            if unsafe { (*(l as *const NMHDR)).idFrom } != usize::from(ID_LIST) {
                return 0;
            }
            // SAFETY: a notification from the list.
            if let Some(answer) = unsafe { s.notify(dlg, l) } {
                // SAFETY: the answer of this dialog's WM_NOTIFY.
                unsafe { SetWindowLongPtrW(dlg, DWLP_MSGRESULT as i32, answer) };
            }
            1
        }
        WM_COMMAND => {
            let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
            match id {
                // Enter is Open, the default button.
                ID_OPEN | OK => s.open(dlg),
                ID_COPY => s.copy(),
                ID_FILTER if code == EN_CHANGE => {
                    let keep = s.current().map(|i| s.all.borrow()[i].label.clone());
                    s.reshow(dlg, keep.as_deref());
                }
                // SAFETY: ends this modal dialog.
                CANCEL => unsafe {
                    EndDialog(dlg, 0);
                },
                _ => return 0,
            }
            1
        }
        WM_REFRESH => {
            if s.host.bookmarks().1 != s.generation.get() {
                s.reload(dlg);
            }
            1
        }
        WM_DESTROY => {
            places::keep(dlg, Which::BookmarkSelect, Find::list(dlg));
            s.host.showing(core::ptr::null_mut());
            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::tree::tests::list;

    #[test]
    fn every_bookmark_with_its_folders_and_a_filter() {
        let l = list(&[
            ("b", "", "Notes"),
            ("f", "", "Music"),
            ("b", "Music/Rock", "Queen"),
            ("-", "", ""),
            ("b", "Music", "Abba"),
        ]);
        let all = entries(&l);
        let labels: Vec<&str> = all.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["Notes", "Music/Rock/Queen", "Music/Abba"]);
        assert_eq!(all[1].chosen.search.as_deref(), Some("Queen"));
        // Name with its folders, or search; any case.
        assert_eq!(view(&all, "music"), [1, 2]);
        assert_eq!(view(&all, "QUEEN"), [1]);
        assert_eq!(view(&all, "rock/q"), [1]);
        assert!(view(&all, "zzz").is_empty());
        assert_eq!(view(&all, ""), [0, 1, 2]);
    }
}
