//! The Filters pane (Everything's Filters sidebar, View > Filters): the
//! filters as a list, the one in force chosen. Choosing another (a click,
//! the arrow keys) puts it in force, as the Search menu, the filter bar or
//! Ctrl+] would; the list follows the filter however it changes. Its menu is
//! Everything's: Add to Filters... and Organize Filters... (F4's), then the
//! filter list of the filter bar.
//!
//! The list's entries are [`entries`]: the filters goz-gui has, the
//! built-in ones and the user's own (F4's `Filters.csv`), each with the
//! command that puts it in force (the Search menu's), so a filter added
//! there shows here too.

use std::any::Any;
use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, HDC,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDIS_HOT, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, LVCF_WIDTH, LVCOLUMNW, LVIF_PARAM,
    LVIF_STATE, LVIF_TEXT, LVIR_BOUNDS, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS,
    LVM_ENSUREVISIBLE, LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETITEMW,
    LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETBKCOLOR, LVM_SETCOLUMNWIDTH,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR,
    LVN_ITEMCHANGED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_NOCOLUMNHEADER,
    LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, NMLISTVIEW,
    NMLVCUSTOMDRAW, WC_LISTVIEWW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, VK_RETURN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyMenu, GetClientRect, HMENU, MSG, MoveWindow, PostMessageW,
    SendMessageW, WM_APP, WM_CONTEXTMENU, WM_KEYDOWN, WM_NOTIFY, WM_SETFONT, WM_SYSKEYDOWN,
    WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use super::{App, point};
use crate::dock::PaneId;
use crate::filter::Filter;
use crate::paint::fill;
use crate::pane::{PaneContent, PaneEvent};
use crate::theme;
use crate::wide::nul;
use crate::{menu, searchtools};

/// The list's choice is to be made to match the filter in force again.
const WM_FOLLOW: u32 = WM_APP + 1;
/// The list's child id in the pane.
const LIST_ID: usize = 1;

/// The filters the pane lists, in the Search menu's order: each one's name
/// and the command that puts it in force.
fn entries() -> Vec<(String, u16)> {
    Filter::all()
        .into_iter()
        // Past the most the menus list, a filter has no command.
        .filter(|f| f.menu_id() != 0)
        .map(|f| (f.title().to_owned(), f.menu_id()))
        .collect()
}

/// The command of the filter in force.
fn current(app: &App) -> u16 {
    app.opts.get().filter.menu_id()
}

pub(super) struct FiltersPane {
    list: Cell<HWND>,
    pane: Cell<HWND>,
    /// The entries shown (each name and command), as the rows paint them.
    shown: RefCell<Vec<(String, u16)>>,
    /// The list is being filled or its choice set by goz-gui.
    syncing: Cell<bool>,
}

impl FiltersPane {
    pub(super) fn new() -> FiltersPane {
        FiltersPane {
            list: Cell::new(core::ptr::null_mut()),
            pane: Cell::new(core::ptr::null_mut()),
            shown: RefCell::new(Vec::new()),
            syncing: Cell::new(false),
        }
    }

    fn send(&self, msg: u32, w: usize, l: isize) -> isize {
        // SAFETY: a message to this pane's list; any pointer in `l` is the
        // caller's and lives across the call.
        unsafe { SendMessageW(self.list.get(), msg, w, l) }
    }

    fn quietly(&self, f: impl FnOnce()) {
        let was = self.syncing.replace(true);
        f();
        self.syncing.set(was);
    }

    /// The command of entry `index`.
    fn command_of(&self, index: usize) -> Option<u16> {
        let mut item = LVITEMW {
            mask: LVIF_PARAM,
            iItem: index as i32,
            // SAFETY: the rest of LVITEMW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        (self.send(LVM_GETITEMW, 0, (&raw mut item) as isize) != 0)
            .then(|| u16::try_from(item.lParam).ok())
            .flatten()
    }

    /// The chosen entry.
    fn chosen(&self) -> Option<usize> {
        usize::try_from(self.send(LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize)).ok()
    }

    /// Fills the list with the filters.
    fn fill(&self, app: &App) {
        let entries = entries();
        *self.shown.borrow_mut() = entries.clone();
        self.quietly(|| {
            self.send(LVM_DELETEALLITEMS, 0, 0);
            for (i, (name, cmd)) in entries.into_iter().enumerate() {
                let mut text = nul(&name.encode_utf16().collect::<Vec<u16>>());
                let item = LVITEMW {
                    mask: LVIF_TEXT | LVIF_PARAM,
                    iItem: i as i32,
                    pszText: text.as_mut_ptr(),
                    lParam: cmd as isize,
                    // SAFETY: the rest of LVITEMW is plain data.
                    ..unsafe { core::mem::zeroed() }
                };
                self.send(LVM_INSERTITEMW, 0, (&raw const item) as isize);
            }
        });
        self.follow(app);
    }

    /// The list shows the filters there are now (they can change), and its
    /// choice is the filter in force.
    fn follow(&self, app: &App) {
        let changed = *self.shown.borrow() != entries();
        if changed {
            self.fill(app);
            return;
        }
        let want = current(app);
        let count = usize::try_from(self.send(LVM_GETITEMCOUNT, 0, 0)).unwrap_or(0);
        let at = (0..count).find(|&i| self.command_of(i) == Some(want));
        if at.is_some() && at == self.chosen() {
            return;
        }
        self.quietly(|| {
            let set = |index: usize, on: bool| {
                let state = if on { LVIS_SELECTED | LVIS_FOCUSED } else { 0 };
                let item = LVITEMW {
                    mask: LVIF_STATE,
                    state,
                    stateMask: LVIS_SELECTED | LVIS_FOCUSED,
                    // SAFETY: the rest of LVITEMW is plain data.
                    ..unsafe { core::mem::zeroed() }
                };
                self.send(LVM_SETITEMSTATE, index, (&raw const item) as isize);
            };
            set(usize::MAX, false);
            if let Some(i) = at {
                set(i, true);
                self.send(LVM_ENSUREVISIBLE, i, 0);
            }
        });
    }

    /// Paints entry `cd.dwItemSpec` in the window's colours, as the results
    /// paint theirs: the one in force in the selection's colour (brighter
    /// while the list has the keyboard), the one under the mouse lit. The
    /// list's own look would leave the choice unseen once the list lost the
    /// keyboard, in the dark theme.
    fn paint_row(&self, app: &App, cd: &NMCUSTOMDRAW) {
        let list = self.list.get();
        let index = cd.dwItemSpec;
        let mut rc = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        self.send(LVM_GETITEMRECT, index, (&raw mut rc) as isize);
        let selected =
            self.send(LVM_GETITEMSTATE, index, LVIS_SELECTED as isize) as u32 & LVIS_SELECTED != 0;
        // SAFETY: reads this thread's focus.
        let focused = unsafe { GetFocus() } == list;
        let pal = app.pal.get();
        let bg = match (selected, focused) {
            (true, true) => pal.sel,
            (true, false) => pal.sel_inactive,
            _ if cd.uItemState & CDIS_HOT != 0 => pal.hover,
            _ => pal.bg,
        };
        fill(cd.hdc, &rc, bg);
        let Some(name) = self.shown.borrow().get(index).map(|(name, _)| name.clone()) else {
            return;
        };
        let mut text: Vec<u16> = name.encode_utf16().collect();
        let mut text_rc = RECT {
            left: rc.left + app.scale(4),
            right: rc.right - app.scale(2),
            ..rc
        };
        // SAFETY: draws into the list's paint DC; the old font is put back.
        unsafe {
            let old = SelectObject(cd.hdc, app.ui_font());
            SetBkMode(cd.hdc, TRANSPARENT as i32);
            SetTextColor(cd.hdc, pal.fg);
            DrawTextW(
                cd.hdc,
                text.as_mut_ptr(),
                text.len() as i32,
                &mut text_rc,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            SelectObject(cd.hdc, old);
        }
    }

    /// Clears the list below its last row, where the visual style draws the
    /// column's edge on down to the bottom (as the results' `paint::
    /// below_rows` does), into the list's own paint buffer.
    fn below_rows(&self, app: &App, hdc: HDC) {
        let count = self.send(LVM_GETITEMCOUNT, 0, 0);
        let mut client = RECT::default();
        // SAFETY: reads this pane's list's size.
        unsafe { GetClientRect(self.list.get(), &mut client) };
        let top = if count > 0 {
            let mut rc = RECT {
                left: LVIR_BOUNDS as i32,
                ..RECT::default()
            };
            self.send(
                LVM_GETITEMRECT,
                (count - 1) as usize,
                (&raw mut rc) as isize,
            );
            rc.bottom
        } else {
            0
        };
        if top < client.bottom {
            fill(hdc, &RECT { top, ..client }, app.pal.get().bg);
        }
    }

    /// Fits the one column to the list's width.
    fn fit_column(&self) {
        let mut rc = RECT::default();
        // SAFETY: reads this pane's list's size.
        unsafe { GetClientRect(self.list.get(), &mut rc) };
        self.send(LVM_SETCOLUMNWIDTH, 0, rc.right.max(0) as isize);
    }

    fn apply_theme(&self, app: &App) {
        let pal = app.pal.get();
        let list = self.list.get();
        theme::allow(list, pal.dark);
        theme::set_class(
            list,
            Some(if pal.dark {
                "DarkMode_Explorer"
            } else {
                "Explorer"
            }),
        );
        self.send(LVM_SETBKCOLOR, 0, pal.bg as isize);
        self.send(LVM_SETTEXTBKCOLOR, 0, pal.bg as isize);
        self.send(LVM_SETTEXTCOLOR, 0, pal.fg as isize);
    }

    /// The filter list as a menu at `l` (screen), or under the chosen entry
    /// from the keyboard (-1); the one picked is put in force.
    fn menu(&self, app: &App, l: LPARAM) {
        let pt = if l == -1 {
            let mut rc = RECT {
                left: LVIR_BOUNDS as i32,
                ..RECT::default()
            };
            self.send(
                LVM_GETITEMRECT,
                self.chosen().unwrap_or(0),
                (&raw mut rc) as isize,
            );
            let mut pt = POINT {
                x: rc.left + app.scale(12),
                y: rc.bottom,
            };
            // SAFETY: converts a point of this pane's list.
            unsafe { ClientToScreen(self.list.get(), &mut pt) };
            pt
        } else {
            point(l)
        };
        // Everything's: the filters' own commands, then the list.
        let popup = menu::filter_menu(app.opts.get().filter);
        menu::add_items(
            popup,
            None,
            &[
                menu::Item::Cmd(searchtools::ADD_TO_FILTERS, "Add to Filters..."),
                menu::Item::Cmd(searchtools::ORGANIZE_FILTERS, "Organize Filters..."),
                menu::Item::Sep,
            ],
        );
        let id = app.track_with_shell(popup, pt, "filters menu", None);
        // SAFETY: the menu made above.
        unsafe { DestroyMenu(popup) };
        if id > 0 {
            app.command(id as u16);
        }
    }
}

impl App {
    /// The filters changed without a new search (F4's Organize Filters...
    /// and Add to Filters...): the Filters pane, if open, lists them again.
    pub(super) fn filters_pane_follow(&self) {
        if let Some(content) = self.pane_content(PaneId::Filters)
            && let Some(pane) = content.as_any().downcast_ref::<FiltersPane>()
        {
            pane.follow(self);
        }
    }
}

impl PaneContent for FiltersPane {
    fn create(&self, app: &App, pane: HWND) {
        self.pane.set(pane);
        // SAFETY: a child of this thread's pane window.
        let list = unsafe {
            CreateWindowExW(
                0,
                WC_LISTVIEWW,
                core::ptr::null(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WS_VSCROLL
                    | LVS_REPORT
                    | LVS_NOCOLUMNHEADER
                    | LVS_SINGLESEL
                    | LVS_SHOWSELALWAYS,
                0,
                0,
                0,
                0,
                pane,
                LIST_ID as HMENU,
                GetModuleHandleW(core::ptr::null()),
                core::ptr::null(),
            )
        };
        self.list.set(list);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        self.send(LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        let column = LVCOLUMNW {
            mask: LVCF_WIDTH,
            cx: 100,
            // SAFETY: the rest of LVCOLUMNW is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        self.send(LVM_INSERTCOLUMNW, 0, (&raw const column) as isize);
        self.send(WM_SETFONT, app.ui_font() as usize, 1);
        self.apply_theme(app);
        self.fill(app);
    }

    fn place(&self, _app: &App, rc: RECT) {
        // SAFETY: moves this pane's list.
        unsafe {
            MoveWindow(
                self.list.get(),
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top,
                1,
            );
        }
        self.fit_column();
    }

    fn focus(&self) -> Option<HWND> {
        Some(self.list.get())
    }

    fn key(&self, app: &App, msg: &MSG) -> bool {
        if msg.hwnd != self.list.get() {
            return false;
        }
        let vk = msg.wParam as u16;
        match msg.message {
            // The filter is in force already: Enter goes to its results.
            WM_KEYDOWN if vk == VK_RETURN => {
                app.focus_list();
                true
            }
            // Alt+Up, Everything's global Up One Dir.
            WM_SYSKEYDOWN if vk == VK_UP => {
                app.up_one_folder();
                true
            }
            _ => false,
        }
    }

    fn message(&self, app: &App, _pane: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_NOTIFY => {
                // SAFETY: a control's notification carries an NMHDR; the
                // list's LVN_ITEMCHANGED an NMLISTVIEW, its NM_CUSTOMDRAW an
                // NMLVCUSTOMDRAW.
                let hdr = unsafe { &*(l as *const NMHDR) };
                if hdr.hwndFrom != self.list.get() {
                    return None;
                }
                if hdr.code == NM_CUSTOMDRAW {
                    // SAFETY: as above.
                    let cd = unsafe { &*(l as *const NMLVCUSTOMDRAW) };
                    return Some(match cd.nmcd.dwDrawStage {
                        CDDS_PREPAINT => (CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT) as LRESULT,
                        CDDS_ITEMPREPAINT => {
                            self.paint_row(app, &cd.nmcd);
                            CDRF_SKIPDEFAULT as LRESULT
                        }
                        CDDS_POSTPAINT => {
                            self.below_rows(app, cd.nmcd.hdc);
                            CDRF_DODEFAULT as LRESULT
                        }
                        _ => CDRF_DODEFAULT as LRESULT,
                    });
                }
                if hdr.code != LVN_ITEMCHANGED {
                    return None;
                }
                if self.syncing.get() {
                    return Some(0);
                }
                // SAFETY: as above.
                let nm = unsafe { &*(l as *const NMLISTVIEW) };
                let chosen = nm.uChanged & LVIF_STATE != 0
                    && nm.uNewState & LVIS_SELECTED != 0
                    && nm.uOldState & LVIS_SELECTED == 0;
                if chosen
                    && let Some(cmd) = self.command_of(nm.iItem.max(0) as usize)
                    && cmd != current(app)
                {
                    app.command(cmd);
                }
                // A click on the empty space leaves nothing chosen: the
                // filter in force shows chosen again, once the list is done
                // with the click.
                // SAFETY: posts to this pane's own window.
                unsafe { PostMessageW(self.pane.get(), WM_FOLLOW, 0, 0) };
                Some(0)
            }
            WM_FOLLOW => {
                self.follow(app);
                Some(0)
            }
            WM_CONTEXTMENU if w as HWND == self.list.get() => {
                self.menu(app, l);
                Some(0)
            }
            _ => None,
        }
    }

    fn theme(&self, app: &App, _dark: bool) {
        self.apply_theme(app);
        // SAFETY: repaints this pane's list.
        unsafe { InvalidateRect(self.list.get(), core::ptr::null(), 1) };
    }

    fn dpi(&self, app: &App, _dpi: u32) {
        self.send(WM_SETFONT, app.ui_font() as usize, 1);
    }

    fn event(&self, app: &App, event: PaneEvent) {
        if event == PaneEvent::SearchChanged {
            self.follow(app);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
