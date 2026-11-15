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
//!
//! Its rows are Everything's (17 px at 100 %: the font's height and 2),
//! not a list's own (the font's and 4): the list is owner-drawn with fixed
//! rows, which it measures with the pane (`WM_MEASUREITEM`) whenever its
//! size changes. The rows are painted in its custom draw as before.

use std::any::Any;
use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDIS_HOT, CDRF_DODEFAULT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, LVCF_WIDTH, LVCOLUMNW, LVIF_PARAM,
    LVIF_STATE, LVIR_BOUNDS, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVN_ITEMCHANGED, LVNI_SELECTED,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_NOCOLUMNHEADER, LVS_OWNERDRAWFIXED, LVS_REPORT,
    LVS_SHOWSELALWAYS, LVS_SINGLESEL, NM_CUSTOMDRAW, NMLISTVIEW, NMLVCUSTOMDRAW, ODT_LISTVIEW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_RETURN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WM_APP, WM_CONTEXTMENU, WM_DRAWITEM, WM_KEYDOWN,
    WM_MEASUREITEM, WM_NOTIFY, WM_SYSKEYDOWN, WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use super::{App, point};
use crate::colors::{Item as ColorItem, State as ColorState};
use crate::dock::PaneId;
use crate::filter::Filter;
use crate::paint::fill;
use crate::pane::{PaneContent, PaneEvent};
use crate::theme::{self, Ink};
use crate::win::class::{self, Create};
use crate::win::gdi::{self, Selected};
use crate::win::{self, Msg, lv};
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

    fn quietly(&self, f: impl FnOnce()) {
        let was = self.syncing.replace(true);
        f();
        self.syncing.set(was);
    }

    /// The command of entry `index`.
    fn command_of(&self, index: usize) -> Option<u16> {
        let item = LVITEMW {
            mask: LVIF_PARAM,
            iItem: index as i32,
            ..Default::default()
        };
        lv::get_item(self.list.get(), item, 0).and_then(|(item, _)| u16::try_from(item.lParam).ok())
    }

    /// The chosen entry.
    fn chosen(&self) -> Option<usize> {
        usize::try_from(lv::next_item(self.list.get(), -1, LVNI_SELECTED)).ok()
    }

    /// Fills the list with the filters.
    fn fill(&self, app: &App) {
        let entries = entries();
        *self.shown.borrow_mut() = entries.clone();
        let list = self.list.get();
        self.quietly(|| {
            lv::delete_all_items(list);
            for (i, (name, cmd)) in entries.into_iter().enumerate() {
                // The name goes in as its text (LVIF_TEXT).
                let item = LVITEMW {
                    mask: LVIF_PARAM,
                    iItem: i as i32,
                    lParam: cmd as isize,
                    ..Default::default()
                };
                lv::insert_item(list, item, Some(&name));
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
        let list = self.list.get();
        let count = lv::item_count(list);
        let at = (0..count).find(|&i| self.command_of(i) == Some(want));
        if at.is_some() && at == self.chosen() {
            return;
        }
        self.quietly(|| {
            // Item -1: every item.
            let set = |index: i32, on: bool| {
                let state = if on { LVIS_SELECTED | LVIS_FOCUSED } else { 0 };
                lv::set_item_state(list, index, state, LVIS_SELECTED | LVIS_FOCUSED);
            };
            set(-1, false);
            if let Some(i) = at {
                set(i as i32, true);
                lv::ensure_visible(list, i as i32, false);
            }
        });
    }

    /// The rows' height: Everything's 17 px at 100 % (the sidebar's font's
    /// 15 and 2), where a list's own would be the font's and 4.
    fn row_height(app: &App) -> i32 {
        app.part_height(ColorItem::FiltersSidebar) + app.scale(2)
    }

    /// The list measures its rows again (`WM_MEASUREITEM`), which it does
    /// as its size changes: after a change of font or DPI that left its
    /// size alone.
    fn remeasure(&self) {
        let list = self.list.get();
        // Its size, then one pixel taller and back, each a
        // WM_WINDOWPOSCHANGED to it.
        let rc = win::client_rect(list);
        let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        let (w, h) = (rc.right, rc.bottom);
        win::set_pos(list, core::ptr::null_mut(), (0, 0, w, h + 1), flags);
        win::set_pos(list, core::ptr::null_mut(), (0, 0, w, h), flags);
    }

    /// Paints entry `index` into `hdc` (`hot`: under the mouse) in the
    /// window's colours, as the results paint theirs: the one in force in
    /// the selection's colour (brighter while the list has the keyboard),
    /// the one under the mouse lit. The list's own look would leave the
    /// choice unseen once the list lost the keyboard, in the dark theme.
    fn paint_row(&self, app: &App, hdc: HDC, index: usize, hot: bool) {
        let list = self.list.get();
        let rc = lv::item_rect(list, index as i32, LVIR_BOUNDS as i32).unwrap_or_default();
        let selected = lv::item_state(list, index as i32, LVIS_SELECTED) & LVIS_SELECTED != 0;
        let focused = win::focus() == list;
        let pal = app.pal.get();
        // Options > Fonts and Colors' Filters Sidebar, in the row's state.
        let state = ColorState::of_row(selected, focused, hot);
        let ink = pal.ink(ColorItem::FiltersSidebar, state);
        fill(hdc, &rc, ink.bg);
        let Some(name) = self.shown.borrow().get(index).map(|(name, _)| name.clone()) else {
            return;
        };
        let text: Vec<u16> = name.encode_utf16().collect();
        let mut text_rc = RECT {
            left: rc.left + app.scale(4),
            right: rc.right - app.scale(2),
            ..rc
        };
        // Drawn into the list's paint DC; the old font goes back as this
        // drops.
        let font = app.part_font(
            ColorItem::FiltersSidebar,
            pal.is_bold(ColorItem::FiltersSidebar, state),
        );
        let _font = Selected::new(hdc, font);
        gdi::set_bk_mode(hdc, TRANSPARENT as i32);
        gdi::set_text_color(hdc, ink.fg);
        gdi::draw_text(
            hdc,
            &text,
            &mut text_rc,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }

    /// Clears the list below its last row, where the visual style draws the
    /// column's edge on down to the bottom (as the results' `paint::
    /// below_rows` does), into the list's own paint buffer.
    fn below_rows(&self, app: &App, hdc: HDC) {
        let list = self.list.get();
        let count = lv::item_count(list);
        let client = win::client_rect(list);
        let top = if count > 0 {
            lv::item_rect(list, (count - 1) as i32, LVIR_BOUNDS as i32)
                .unwrap_or_default()
                .bottom
        } else {
            0
        };
        if top < client.bottom {
            fill(hdc, &RECT { top, ..client }, filters_ink(app).bg);
        }
    }

    /// Fits the one column to the list's width.
    fn fit_column(&self) {
        let rc = win::client_rect(self.list.get());
        lv::set_column_width(self.list.get(), 0, rc.right.max(0));
    }

    fn apply_theme(&self, app: &App) {
        let pal = app.pal.get();
        let ink = filters_ink(app);
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
        lv::set_bk_color(list, ink.bg);
        lv::set_text_bk_color(list, ink.bg);
        lv::set_text_color(list, ink.fg);
    }

    /// The filter list as a menu at `l` (screen), or under the chosen entry
    /// from the keyboard (-1); the one picked is put in force.
    fn menu(&self, app: &App, l: LPARAM) {
        let pt = if l == -1 {
            let list = self.list.get();
            let rc = lv::item_rect(list, self.chosen().unwrap_or(0) as i32, LVIR_BOUNDS as i32)
                .unwrap_or_default();
            win::client_to_screen(
                list,
                POINT {
                    x: rc.left + app.scale(12),
                    y: rc.bottom,
                },
            )
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
        // The menu made above.
        win::menu::destroy(popup);
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

/// The list's colours: Options > Fonts and Colors' Filters Sidebar.
fn filters_ink(app: &App) -> Ink {
    app.pal
        .get()
        .ink(ColorItem::FiltersSidebar, ColorState::Normal)
}

impl PaneContent for FiltersPane {
    fn colours(&self, app: &App) -> Ink {
        filters_ink(app)
    }

    fn create(&self, app: &App, pane: HWND) {
        self.pane.set(pane);
        // A child of this thread's pane window.
        let list = class::create_plain(&Create {
            ex_style: 0,
            class: lv::CLASS,
            title: "",
            style: WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | LVS_REPORT
                | LVS_NOCOLUMNHEADER
                | LVS_SINGLESEL
                | LVS_SHOWSELALWAYS
                | LVS_OWNERDRAWFIXED,
            rect: (0, 0, 0, 0),
            parent: pane,
            menu: class::id_menu(LIST_ID),
        });
        self.list.set(list);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        lv::set_extended_style(list, ex, ex);
        let column = LVCOLUMNW {
            mask: LVCF_WIDTH,
            cx: 100,
            ..Default::default()
        };
        lv::insert_column(list, 0, column, None);
        win::set_font(list, app.part_font(ColorItem::FiltersSidebar, false), true);
        self.remeasure();
        self.apply_theme(app);
        self.fill(app);
    }

    fn place(&self, _app: &App, rc: RECT) {
        win::move_window(
            self.list.get(),
            rc.left,
            rc.top,
            rc.right - rc.left,
            rc.bottom - rc.top,
            true,
        );
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

    fn handle(&self, app: &App, _pane: HWND, m: &Msg) -> Option<LRESULT> {
        let (w, l) = (m.w, m.l);
        match m.id {
            WM_NOTIFY => {
                let n = m.notify()?;
                if n.from() != self.list.get() {
                    return None;
                }
                if n.code() == NM_CUSTOMDRAW {
                    // What the stage needs, copied out of the list's
                    // NMLVCUSTOMDRAW before the rows are painted (which
                    // sends the list more).
                    let cd = n.get::<NMLVCUSTOMDRAW>()?.nmcd;
                    return Some(match cd.dwDrawStage {
                        CDDS_PREPAINT => (CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT) as LRESULT,
                        CDDS_ITEMPREPAINT => {
                            let hot = cd.uItemState & CDIS_HOT != 0;
                            self.paint_row(app, cd.hdc, cd.dwItemSpec, hot);
                            // Painted: no WM_DRAWITEM follows.
                            CDRF_SKIPDEFAULT as LRESULT
                        }
                        CDDS_POSTPAINT => {
                            self.below_rows(app, cd.hdc);
                            CDRF_DODEFAULT as LRESULT
                        }
                        _ => CDRF_DODEFAULT as LRESULT,
                    });
                }
                if n.code() != LVN_ITEMCHANGED {
                    return None;
                }
                if self.syncing.get() {
                    return Some(0);
                }
                let nm = *n.get::<NMLISTVIEW>()?;
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
                win::post_app(self.pane.get(), WM_FOLLOW, 0, 0);
                Some(0)
            }
            // The rows' height, asked as the list is made (before
            // `self.list` is known) and whenever its size changes.
            WM_MEASUREITEM => {
                let mut mi = m.measure_item()?;
                if mi.CtlType != ODT_LISTVIEW || mi.CtlID as usize != LIST_ID {
                    return None;
                }
                mi.itemHeight = Self::row_height(app).max(1) as u32;
                Some(1)
            }
            // An owner-drawn row the custom draw did not paint.
            WM_DRAWITEM => {
                // Copied out before the row is painted.
                let di = *m.draw_item()?;
                if di.CtlType != ODT_LISTVIEW || di.hwndItem != self.list.get() {
                    return None;
                }
                let hot = win::lv::hot_item(self.list.get()) as isize == di.itemID as isize;
                self.paint_row(app, di.hDC, di.itemID as usize, hot);
                Some(1)
            }
            WM_FOLLOW => {
                self.follow(app);
                Some(0)
            }
            // wParam is the window clicked.
            WM_CONTEXTMENU if w as HWND == self.list.get() => {
                self.menu(app, l);
                Some(0)
            }
            _ => None,
        }
    }

    fn theme(&self, app: &App, _dark: bool) {
        self.apply_theme(app);
        win::invalidate(self.list.get(), None, true);
    }

    fn dpi(&self, app: &App, _dpi: u32) {
        win::set_font(
            self.list.get(),
            app.part_font(ColorItem::FiltersSidebar, false),
            true,
        );
        self.remeasure();
    }

    /// One row in the sidebar's font, and the list's edges.
    fn least_high(&self, app: &App) -> i32 {
        Self::row_height(app) + app.scale(4)
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
