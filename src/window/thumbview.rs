//! The thumbnail views: the list switched to icon view, each item drawn
//! here at its custom-draw stage (the owner-drawn rows are Details' only),
//! and the view switches of the View menu and the status bar.
//!
//! The list keeps its own geometry: an empty image list of the
//! thumbnail's size and the icon spacing make its icon rectangles, hit
//! tests, keyboard moves and scrolling match the cells drawn here. The
//! view is switched by the list's style bits (LVM_SETVIEW leaves a
//! virtual, owner-drawn list unable to place or draw its icons).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, HDC, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, CLR_DEFAULT, CLR_NONE, ILC_COLOR32, ILD_ASYNC,
    ILD_SCALE, ILD_TRANSPARENT, IMAGELISTDRAWPARAMS, LVA_DEFAULT, LVIR_ICON, LVIS_FOCUSED,
    LVIS_SELECTED, LVNI_FOCUSED, LVS_AUTOARRANGE, LVS_ICON, LVS_OWNERDRAWFIXED, LVS_REPORT,
    LVS_TYPEMASK, LVSIL_NORMAL, NMCUSTOMDRAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

use super::App;
use crate::highlight::Span;
use crate::paint::{self, RowState};
use crate::status::ViewButton;
use crate::thumbs::{Bitmap, Picture, ThumbReply};
use crate::view::{self, View};
use crate::win::dib::Canvas;
use crate::win::imagelist::{ImageList, SystemList};
use crate::win::{self, gdi, imagelist, lv};
use crate::window::Fonts;

/// `SHIL_LARGE` (32 px), `SHIL_SMALL` (16 px), `SHIL_EXTRALARGE` (48 px)
/// and `SHIL_JUMBO` (256 px).
const SHIL_LARGE: i32 = 0;
const SHIL_SMALL: i32 = 1;
const SHIL_EXTRALARGE: i32 = 2;
const SHIL_JUMBO: i32 = 4;

/// The lists a type's icon stands in from while its big one loads, the
/// largest first.
const STAND_INS: [i32; 3] = [SHIL_EXTRALARGE, SHIL_LARGE, SHIL_SMALL];

/// The lists to try for a stand-in when the icon wanted is of list
/// `wanted` (`SHIL_*`): the smaller ones.
fn stand_ins(wanted: i32) -> impl Iterator<Item = i32> {
    let below = if wanted == SHIL_JUMBO {
        0
    } else {
        STAND_INS
            .iter()
            .position(|&l| l == wanted)
            .map_or(STAND_INS.len(), |at| at + 1)
    };
    STAND_INS.into_iter().skip(below)
}

/// The big icons' thread (`bigicons.rs`) loaded icons that were drawn
/// before they were: the thumbnails paint again. Beside `WM_THUMB`.
pub(super) const WM_BIG_ICONS: u32 = WM_APP + 33;

impl App {
    pub fn view(&self) -> View {
        self.view.get()
    }

    /// Switches the results to `view`, keeping the focused item (or the
    /// first one in view) in view and the selection as it is.
    pub(super) fn set_view(&self, view: View) {
        let old = self.view.get();
        if old == view {
            return;
        }
        // What shows now, measured in the view still on the list.
        let shown = self.visible_items();
        self.view.set(view);
        self.settings_changed();
        self.invalidate_status();
        if !old.thumbnails && !view.thumbnails {
            return;
        }
        let focused = lv::next_item(self.list, -1, LVNI_FOCUSED);
        let anchor = usize::try_from(focused)
            .ok()
            .filter(|f| shown.contains(f))
            .unwrap_or(shown.start);
        self.put_view();
        if lv::item_count(self.list) == 0 {
            return;
        }
        if view.thumbnails {
            lv::ensure_visible(self.list, anchor as i32, false);
        } else {
            // The anchor at the top, as the rows' own scroll puts it
            // (ensure-visible right after the switch would leave the list
            // at its old row).
            let top = lv::top_index(self.list).max(0) as usize;
            let rows = anchor as i64 - top as i64;
            let dy = (rows * i64::from(self.row_height())).clamp(-(1 << 30), 1 << 30);
            if dy != 0 {
                lv::scroll(self.list, 0, dy as i32);
            }
        }
    }

    /// Puts the view on the list: icon view with the thumbnails' spacing,
    /// or Details. Also run when the DPI or the font changes.
    pub(super) fn put_view(&self) {
        let v = self.view.get();
        let dpi = self.dpi.get();
        // Another size, or none wanted (their memory is not, in Details);
        // nor are the big icons of the old size still to load.
        self.reset_thumbs();
        self.big_loads.forget_queued();
        self.big_loads.forget_shrunk();
        if v.thumbnails {
            // The shell's icon lists, asked for here rather than in the
            // middle of a paint.
            for which in [SHIL_JUMBO, SHIL_EXTRALARGE, SHIL_LARGE, SHIL_SMALL] {
                self.system_list(which);
            }
            let side = view::side(v.size, dpi);
            let line = self.fonts.borrow().height;
            let (cx, cy) = view::spacing(v.size, dpi, line);
            // An empty image list the list owns from here on; the one it
            // had, made here before, is destroyed (`lv::set_image_list`).
            let images = ImageList::create(side, side, ILC_COLOR32, 0, 1);
            lv::set_image_list(self.list, LVSIL_NORMAL, images);
            win::lv::set_icon_spacing(self.list, cx, cy);
            // Icon view, its items in rows in order (auto-arranged), and not
            // owner-drawn: owner draw is Details' only, and left on it
            // keeps the list from drawing (or placing) its icons.
            self.set_list_style(LVS_ICON | LVS_AUTOARRANGE);
            // Lays the items out on the new spacing.
            lv::arrange(self.list, LVA_DEFAULT);
        } else {
            self.set_list_style(LVS_REPORT | LVS_OWNERDRAWFIXED);
            // The rows are measured again (WM_MEASUREITEM) for their height.
            let font = self.fonts.borrow().normal;
            win::set_font(self.list, font, true);
        }
        win::invalidate(self.list, None, true);
    }

    /// Sets the list's view style bits (the view, its arrangement, owner
    /// draw) to `bits`, which the list takes as a change of view.
    fn set_list_style(&self, bits: u32) {
        let mask = LVS_TYPEMASK | LVS_AUTOARRANGE | LVS_OWNERDRAWFIXED;
        // The list answers with WM_STYLECHANGED.
        let style = win::style(self.list);
        let new = (style & !mask) | bits;
        if new != style {
            win::set_style(self.list, new);
        }
    }

    /// Forgets the thumbnails made, and asks for new ones at the size on
    /// screen (none in Details).
    pub(super) fn reset_thumbs(&self) {
        let v = self.view.get();
        let side = if v.thumbnails {
            view::side(v.size, self.dpi.get())
        } else {
            0
        };
        self.thumbs.reset(side);
    }

    /// A status bar view button was clicked.
    pub(super) fn view_button(&self, button: ViewButton) {
        let v = self.view.get();
        self.set_view(match button {
            ViewButton::Details => v.details(),
            ViewButton::Thumbnails => View::thumbnails(v.size),
        });
    }

    /// The first item in view.
    pub fn top_index(&self) -> usize {
        self.visible_items().start
    }

    /// The items in view, in either view.
    pub fn visible_items(&self) -> Range<usize> {
        let count = lv::item_count(self.list);
        if !self.view.get().thumbnails {
            let top = lv::top_index(self.list).max(0) as usize;
            let page = lv::count_per_page(self.list).max(0) as usize;
            return top.min(count)..(top + page + 1).min(count);
        }
        // While the list paints, the grid it started with: rows fetched
        // for an item drawn ask for it, and measuring it again then would
        // upset the paint.
        match self.thumb_grid.get().or_else(|| self.grid()) {
            Some(g) => g.visible(&self.list_client(), count),
            None => 0..0,
        }
    }

    fn list_client(&self) -> RECT {
        win::client_rect(self.list)
    }

    /// Item `index`'s icon rectangle as the list has it.
    fn icon_rect(&self, index: usize) -> Option<RECT> {
        lv::item_rect(self.list, index as i32, LVIR_ICON as i32)
    }

    /// The icon view's grid as the list laid it out now, scrolled: asked
    /// of the list item by item, which makes it measure their names (and
    /// send custom draws for that), so never while it paints items.
    fn grid(&self) -> Option<Grid> {
        let count = lv::item_count(self.list);
        let first = self.icon_rect(0)?;
        let (cx, cy) = lv::item_spacing(self.list, false);
        let (cx, cy) = (cx.max(1), cy.max(1));
        let client = self.list_client();
        // As many as the width holds, checked against where the list put
        // the end of the first row.
        let top = |i: usize| self.icon_rect(i).map_or(i32::MIN, |r| r.top);
        let mut per_row = (((client.right - client.left) / cx).max(1) as usize).min(count);
        while per_row > 1 && top(per_row - 1) != first.top {
            per_row -= 1;
        }
        while per_row < count && top(per_row) == first.top {
            per_row += 1;
        }
        Some(Grid {
            per_row: per_row.max(1),
            first,
            cx,
            cy,
        })
    }

    /// NM_CUSTOMDRAW of the list in icon view: every item drawn here.
    pub(super) fn thumb_custom_draw(&self, cd: &NMCUSTOMDRAW) -> isize {
        match cd.dwDrawStage {
            // The list measures an item's name with a custom draw on a DC
            // of no size: left to it.
            CDDS_PREPAINT if cd.rc.right <= cd.rc.left => CDRF_DODEFAULT as isize,
            CDDS_PREPAINT => {
                // The grid now, before the items are drawn: asking for it
                // while they are would start the list's measuring custom
                // draws in the middle of its paint.
                let grid = self.grid();
                self.thumb_grid.set(grid);
                let count = lv::item_count(self.list);
                // Requests for items not in view now are dropped.
                let shown = grid.map_or(0..0, |g| g.visible(&self.list_client(), count));
                self.thumbs.set_visible(shown);
                (CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT) as isize
            }
            CDDS_ITEMPREPAINT => {
                if let Some(grid) = self.thumb_grid.get() {
                    self.paint_cell(cd.hdc, cd.dwItemSpec, &grid);
                }
                CDRF_SKIPDEFAULT as isize
            }
            CDDS_POSTPAINT => {
                // Scrolling moves the grid: measured afresh from here on.
                self.thumb_grid.set(None);
                CDRF_DODEFAULT as isize
            }
            _ => CDRF_DODEFAULT as isize,
        }
    }

    /// Where a cell's parts go around the icon rectangle `icon`.
    fn cell_of(&self, icon: &RECT, cx: i32) -> view::Cell {
        let v = self.view.get();
        let line = self.fonts.borrow().height;
        view::cell(icon, v.size, self.dpi.get(), line, cx)
    }

    /// Where item `index`'s cell, thumbnail and name are (the list's
    /// coordinates). Not while the list paints.
    pub(super) fn thumb_cell(&self, index: usize) -> Option<view::Cell> {
        let icon = self.icon_rect(index)?;
        let (cx, _) = lv::item_spacing(self.list, false);
        Some(self.cell_of(&icon, cx))
    }

    /// Draws item `index`: the cell's background as the row's would be,
    /// the thumbnail (the file's icon until it comes), and the name in up
    /// to two lines with its matches in bold.
    fn paint_cell(&self, hdc: HDC, index: usize, grid: &Grid) {
        let geo = self.cell_of(&grid.icon(index), grid.cx);
        // The list is asked first: nothing is borrowed while it may call
        // back.
        let bits = lv::item_state(self.list, index as i32, LVIS_SELECTED | LVIS_FOCUSED);
        let pal = self.pal.get();
        let list_focused = win::focus() == self.list;
        let state = RowState {
            selected: bits & LVIS_SELECTED != 0,
            focused: list_focused,
            hot: self.row_is_hot(index),
            alternate: false,
        };
        // Options > Fonts and Colors' Thumbnail Mouseover for the cell
        // under the mouse; else as the row would be.
        let thumb = pal.ink(
            crate::colors::Item::ResultList,
            crate::colors::State::ThumbnailMouseover,
        );
        let (colour, text) = if state.hot && !state.selected {
            (thumb.bg, thumb.fg)
        } else {
            (
                paint::cell_colour(&pal, state, false),
                paint::cell_text_colour(&pal, state, false).0,
            )
        };
        paint::fill(hdc, &geo.cell, colour);
        // The thumbnail is drawn while the row is borrowed (GDI only); the
        // icon after, as the shell's icon lists can take messages while
        // they load an icon, and those can come back here.
        let (icon, name, spans) = {
            let m = self.model.borrow();
            let Some(row) = m.row(index) else {
                drop(m);
                self.request_rows(index, index);
                return;
            };
            let mut scratch = self.scratch.borrow_mut();
            let scratch = &mut *scratch;
            m.highlight.spans(
                &row.path,
                row.name_start as usize,
                &mut scratch.folded,
                &mut scratch.name_spans,
                &mut scratch.parent_spans,
            );
            // The icon of the system lists to draw, if no image is:
            // `Some(None)` the type's.
            let icon = self.thumbs.with(&row.path, |got| match got {
                Some(Some(Picture::Image(bitmap))) => {
                    draw_bitmap(hdc, bitmap, &geo.thumb);
                    None
                }
                // One a class of files shares (a shortcut to a help file:
                // the .chm type's icon).
                Some(Some(Picture::Shared(icon))) => Some(Some(*icon)),
                // There is no picture for it: its type's icon.
                Some(None) => Some(None),
                None => {
                    self.thumbs.request(index, &row.path);
                    Some(None)
                }
            });
            // The type's icon, never the file's own (`Icons::type_icon`):
            // a program's own icon comes as its thumbnail.
            let icon = icon.map(|shared| shared.unwrap_or_else(|| self.icons.type_icon(row)));
            (icon, row.name().to_vec(), scratch.name_spans.clone())
        };
        if let Some(icon) = icon {
            self.draw_big_icon(hdc, icon, &geo.thumb);
        }
        let fonts = self.fonts.borrow();
        name_lines(hdc, &geo.text, &name, &spans, &fonts, text);
        if bits & LVIS_FOCUSED != 0 && list_focused {
            gdi::draw_focus_rect(hdc, &geo.cell);
        }
    }

    /// The system's big icon `icon` in `square`: the 48 px one where it
    /// fits, else the 256 px one shrunk to the square with a filter (by the
    /// list while that is made; the 48 px one, centred, where the 256 px
    /// one is only that in a corner).
    ///
    /// Only an icon already loaded is drawn: one that is not (an icon's
    /// first draw reads it from its file, up to 9 ms) is loaded on a thread
    /// (`bigicons.rs`), and the list paints again when it is. Meanwhile the
    /// same icon stands in from the largest smaller list that has it loaded
    /// (48, 32 or 16 px, centred: a type shown in Details has its 16 px
    /// one), so the cell is not empty for that moment ([`stand_in`]).
    ///
    /// [`stand_in`]: Self::stand_in
    fn draw_big_icon(&self, hdc: HDC, icon: i32, square: &RECT) {
        if icon < 0 {
            return;
        }
        let side = square.right - square.left;
        let mut jumbo = side >= self.scale(96);
        // Whether the icon drawn is the one wanted, to be loaded if it is
        // not, or a stand-in, drawn only if it is.
        let mut wanted = true;
        if jumbo && let Some(list) = self.big_icons(true) {
            // A type with no icon that big (.cfg, .md, .toml) is in
            // Windows' jumbo list as its 48 px icon in the top left corner
            // of an empty 256 px square: shrunk to the cell, it would be a
            // speck in the corner until the shell's picture came. The 48 px
            // one, centred, as that picture shows it. (Not measured while
            // the thread is still loading it.)
            let known = if self.big_loads.waiting(list, icon) {
                None
            } else {
                jumbo_is_padded(list, icon)
            };
            match known {
                Some(padded) => jumbo = !padded,
                None => {
                    self.big_loads.load(list, icon);
                    jumbo = false;
                    wanted = false;
                }
            }
        }
        let which = if jumbo { SHIL_JUMBO } else { SHIL_EXTRALARGE };
        let Some(list) = self.big_icons(jumbo) else {
            return;
        };
        let (cx, cy) = imagelist::icon_size(&list).unwrap_or((0, 0));
        let (x, y, w, h, style) = if cx <= side && cy <= side {
            let x = square.left + (side - cx) / 2;
            let y = square.top + (side - cy) / 2;
            (x, y, 0, 0, ILD_TRANSPARENT)
        } else {
            // Shrunk with a filter on the big icons' thread, once per icon
            // and size (`bigicons.rs`): the list's own shrink, below, drops
            // lines finer than its step (a text file's page at Large was
            // blank), so it stands in only while that is made.
            if wanted
                && self
                    .big_loads
                    .draw_shrunk(list, icon, side, |b| draw_bitmap(hdc, b, square))
            {
                return;
            }
            // Shrunk by the list itself: an icon made of it and drawn
            // shrunk (ImageList_GetIcon, DrawIconEx) cost 0.7 ms a cell,
            // 45 ms for a window of Large cells; this is 20 times less.
            (
                square.left,
                square.top,
                side,
                side,
                ILD_TRANSPARENT | ILD_SCALE,
            )
        };
        let params = IMAGELISTDRAWPARAMS {
            i: icon,
            hdcDst: hdc,
            x,
            y,
            cx: w,
            cy: h,
            rgbBk: CLR_NONE as u32,
            rgbFg: CLR_DEFAULT as u32,
            fStyle: style,
            ..Default::default()
        };
        if !self.big_loads.draw(list, &params, wanted) {
            self.stand_in(hdc, icon, square, which);
        }
    }

    /// Icon `icon` of the list below `wanted` (`SHIL_*`) that has it loaded
    /// already, the largest first, centred in `square`: drawn while the
    /// one wanted loads. Only a loaded one is drawn (`ILD_ASYNC`); none is
    /// loaded here, which could take the UI thread milliseconds (or, for a
    /// type whose icon handler must be loaded, hundreds).
    fn stand_in(&self, hdc: HDC, icon: i32, square: &RECT, wanted: i32) {
        let side = square.right - square.left;
        for which in stand_ins(wanted) {
            let Some(list) = self.system_list(which) else {
                continue;
            };
            let (cx, cy) = imagelist::icon_size(&list).unwrap_or((0, 0));
            if cx > side || cy > side {
                continue;
            }
            let params = IMAGELISTDRAWPARAMS {
                i: icon,
                hdcDst: hdc,
                x: square.left + (side - cx) / 2,
                y: square.top + (side - cy) / 2,
                rgbBk: CLR_NONE as u32,
                rgbFg: CLR_DEFAULT as u32,
                fStyle: ILD_TRANSPARENT,
                ..Default::default()
            };
            if self.big_loads.draw(list, &params, false) {
                return;
            }
        }
    }

    /// A result row's icon in Details at `square`, whose side is bigger
    /// than the shell's small icons (`paint::row_icon_side`: a UI scale, or
    /// a monitor of more DPI than the system's): from the least larger of
    /// the shell's lists, shrunk with a filter on the big icons' thread as
    /// the thumbnails' are (`bigicons.rs`), never loaded on this thread; the
    /// small icon, centred, stands in meanwhile.
    pub(crate) fn draw_row_icon(&self, hdc: HDC, icon: i32, square: &RECT) {
        let side = square.right - square.left;
        let mut chosen = None;
        for which in [SHIL_SMALL, SHIL_LARGE, SHIL_EXTRALARGE, SHIL_JUMBO] {
            let Some(list) = self.system_list(which) else {
                continue;
            };
            let (cx, _) = imagelist::icon_size(&list).unwrap_or((0, 0));
            if cx >= side {
                chosen = Some((list, cx));
                break;
            }
        }
        let Some((list, cx)) = chosen else {
            return;
        };
        let shrink = cx > side;
        if shrink
            && self
                .big_loads
                .draw_shrunk(list, icon, side, |b| draw_bitmap(hdc, b, square))
        {
            return;
        }
        let params = IMAGELISTDRAWPARAMS {
            i: icon,
            hdcDst: hdc,
            x: square.left,
            y: square.top,
            cx: if shrink { side } else { 0 },
            cy: if shrink { side } else { 0 },
            rgbBk: CLR_NONE as u32,
            rgbFg: CLR_DEFAULT as u32,
            fStyle: if shrink {
                ILD_TRANSPARENT | ILD_SCALE
            } else {
                ILD_TRANSPARENT
            },
            ..Default::default()
        };
        if !self.big_loads.draw(list, &params, true)
            && let Some(small) = self.icons.list
        {
            // The small icon meanwhile, centred, drawn as Details draws it
            // at 100 % (the icons' thread has loaded it already).
            let (cx, cy) = imagelist::icon_size(&small).unwrap_or((0, 0));
            imagelist::draw(
                &small,
                icon,
                hdc,
                (square.left + (side - cx) / 2, square.top + (side - cy) / 2),
                ILD_TRANSPARENT,
            );
        }
    }

    /// Big icons were loaded (or shrunk) on their thread: the view paints
    /// again, with them (the thumbnails, or Details' rows at a UI scale).
    pub(super) fn big_icons_loaded(&self) {
        if self.big_loads.arrived() {
            win::invalidate(self.list, None, false);
        }
    }

    /// The system image list of 48 px icons, or of 256 px ones (`jumbo`),
    /// asked for once.
    fn big_icons(&self, jumbo: bool) -> Option<SystemList> {
        self.system_list(if jumbo { SHIL_JUMBO } else { SHIL_EXTRALARGE })
    }

    /// The system image list `which` (`SHIL_*`), asked of the shell once:
    /// it lives as long as the process (`win::imagelist::system`).
    fn system_list(&self, which: i32) -> Option<SystemList> {
        win::imagelist::system(which)
    }

    /// A thumbnail came from the thread: kept, and its item redrawn if it
    /// still shows that file. False when it was thrown away instead: made
    /// for results, a size or a window no longer shown (`Thumbs::reset`,
    /// `Thumbs::release`), or the view is Details now.
    pub(super) fn on_thumb(&self, reply: ThumbReply) -> bool {
        let (index, path) = (reply.index, reply.path.clone());
        let taken = self.view.get().thumbnails && self.thumbs.accept(reply);
        if !taken {
            return false;
        }
        let same = self
            .model
            .borrow()
            .peek(index)
            .is_some_and(|r| *r.path == *path);
        if same {
            lv::redraw_items(self.list, index as i32, index as i32);
        }
        true
    }

    /// Where Rename's box goes in icon view: over the name's lines.
    pub(super) fn thumb_rename_box(&self, index: usize) -> Option<RECT> {
        let geo = self.thumb_cell(index)?;
        let line = self.fonts.borrow().height;
        Some(RECT {
            bottom: geo.text.top + view::TEXT_LINES * line + 2,
            top: geo.text.top - 1,
            ..geo.text
        })
    }

    /// Whether `pt` is on item `index`'s name in icon view, where a slow
    /// second click renames.
    pub(super) fn on_thumb_name(&self, index: usize, pt: POINT) -> bool {
        self.thumb_cell(index)
            .is_some_and(|g| pt.y >= g.text.top && pt.x >= g.text.left && pt.x < g.text.right)
    }
}

/// The icon view's layout: rows of `per_row` slots `cx` × `cy`, item 0's
/// icon rectangle at `first` (the list's coordinates, scrolled).
#[derive(Clone, Copy)]
pub(super) struct Grid {
    per_row: usize,
    first: RECT,
    cx: i32,
    cy: i32,
}

impl Grid {
    /// Item `index`'s icon rectangle.
    fn icon(&self, index: usize) -> RECT {
        let col = (index % self.per_row) as i32;
        let row = (index / self.per_row).min(i32::MAX as usize / 2) as i32;
        let (dx, dy) = (col * self.cx, row.saturating_mul(self.cy));
        RECT {
            left: self.first.left + dx,
            top: self.first.top.saturating_add(dy),
            right: self.first.right + dx,
            bottom: self.first.bottom.saturating_add(dy),
        }
    }

    /// The items of `count` whose rows show in `client`: those scrolled
    /// off above are skipped; one more row for a part at either edge.
    fn visible(&self, client: &RECT, count: usize) -> Range<usize> {
        let above = ((client.top - self.first.top).max(0) / self.cy) as usize;
        let rows = ((client.bottom - client.top).max(0) / self.cy + 2) as usize;
        let from = (above * self.per_row).min(count);
        from..(from + rows * self.per_row).min(count)
    }
}

/// Draws `bitmap` (premultiplied) fitted into `square`.
fn draw_bitmap(hdc: HDC, bitmap: &Bitmap, square: &RECT) {
    let dst = view::fit(bitmap.width, bitmap.height, square);
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // A memory DC made and deleted here holds the bitmap, which the cache
    // keeps alive across the call and selects nowhere else.
    let mem = gdi::MemDc::compatible(hdc);
    let selected = gdi::Selected::of(mem.hdc(), bitmap.handle());
    gdi::alpha_blend(
        hdc,
        (
            dst.left,
            dst.top,
            dst.right - dst.left,
            dst.bottom - dst.top,
        ),
        mem.hdc(),
        (0, 0, bitmap.width, bitmap.height),
        blend,
    );
    drop(selected);
    drop(mem);
}

/// Whether the system jumbo list's icon `icon` is a small icon in the top
/// left corner of an otherwise empty square; `None` while the icon is not
/// loaded. Worked out once per icon (two draws into a scratch bitmap) and
/// kept: the system list's indexes last as long as the process.
fn jumbo_is_padded(list: SystemList, icon: i32) -> Option<bool> {
    thread_local! {
        static PADDED: RefCell<HashMap<i32, bool>> = RefCell::new(HashMap::new());
    }
    if let Some(known) = PADDED.with(|p| p.borrow().get(&icon).copied()) {
        return Some(known);
    }
    // Not borrowed while drawing: the shell's list can take messages while
    // it draws, and those can paint (and come back here).
    let padded = match drawn_extent(list, icon) {
        Extent::NotLoaded => return None,
        Extent::Unknown => false,
        Extent::Drawn {
            right,
            bottom,
            side,
        } => corner_only(right, bottom, side),
    };
    PADDED.with(|p| p.borrow_mut().insert(icon, padded));
    Some(padded)
}

/// How far an icon draws (`drawn_extent`).
enum Extent {
    /// It draws as far right and down as this, in a square of `side`.
    Drawn { right: i32, bottom: i32, side: i32 },
    /// The list has not loaded it yet (and is not made to here).
    NotLoaded,
    /// It could not be measured.
    Unknown,
}

/// How far right and down icon `icon` of `list` draws anything, and the
/// list's icon side. It is drawn twice into a scratch bitmap, on black and
/// on white, so that no colour of its own can hide one of its pixels; only
/// if it is loaded (`ILD_ASYNC`).
fn drawn_extent(list: SystemList, icon: i32) -> Extent {
    // A DIB section and a memory DC made, used and deleted here
    // (`dib::Canvas`): its pixels are copied out between the draws, once
    // what was drawn is in them.
    let (cx, cy) = imagelist::icon_size(&list).unwrap_or((0, 0));
    if cx <= 0 || cy <= 0 {
        return Extent::Unknown;
    }
    let Some(canvas) = Canvas::new(cx, cy) else {
        return Extent::Unknown;
    };
    let params = IMAGELISTDRAWPARAMS {
        hdcDst: canvas.hdc(),
        i: icon,
        rgbBk: CLR_NONE as u32,
        rgbFg: CLR_DEFAULT as u32,
        fStyle: ILD_TRANSPARENT | ILD_ASYNC,
        ..Default::default()
    };
    let (mut right, mut bottom) = (0, 0);
    let mut loaded = true;
    for back in [0u32, 0x00ff_ffff] {
        canvas.fill(back.to_le_bytes());
        if !imagelist::draw_indirect(&list, &params) {
            loaded = false;
            break;
        }
        let Some(pixels) = canvas.pixels() else {
            return Extent::Unknown;
        };
        for (i, p) in pixels.iter().enumerate() {
            if u32::from_le_bytes(*p) & 0x00ff_ffff != back {
                let (x, y) = (i as i32 % cx, i as i32 / cx);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    drop(canvas);
    if !loaded {
        return Extent::NotLoaded;
    }
    Extent::Drawn {
        right,
        bottom,
        side: cx.min(cy),
    }
}

/// Whether what an icon draws, `right` × `bottom` from the top left of its
/// `side` square, stays in the square's top left quarter: a small icon
/// Windows put in the corner of a jumbo square. An icon that draws nothing
/// is not one.
fn corner_only(right: i32, bottom: i32, side: i32) -> bool {
    right > 0 && bottom > 0 && right <= side / 2 && bottom <= side / 2
}

/// The spans of `spans` inside `from..to`, from `from`.
fn spans_in(spans: &[Span], from: usize, to: usize) -> Vec<Span> {
    spans
        .iter()
        .filter_map(|&(s, e)| {
            let (s, e) = (s.max(from), e.min(to));
            (e > s).then(|| (s - from, e - from))
        })
        .collect()
}

/// The name under a thumbnail: centred, broken between words onto a
/// second line when it is too long for one, and the second line ending in
/// an ellipsis when that is too long too.
fn name_lines(hdc: HDC, rc: &RECT, name: &[u16], spans: &[Span], fonts: &Fonts, colour: u32) {
    let avail = rc.right - rc.left;
    if name.is_empty() || avail <= 0 {
        return;
    }
    // Measures and draws into the list's paint DC; the font set here is
    // put back by `runs` (the normal one, the DC's own).
    gdi::set_bk_mode(hdc, TRANSPARENT as i32);
    gdi::set_text_color(hdc, colour);
    // The bold font where there are matches: what fits in it fits.
    gdi::select(
        hdc,
        if spans.is_empty() {
            fonts.normal
        } else {
            fonts.bold
        },
    );
    let (fit, _) = win::gdi::text_fit(hdc, name, avail);
    let cut = view::line_break(name, fit.max(0) as usize);
    let line = |from: usize, to: usize, top: i32| {
        if from >= to {
            return;
        }
        let part = spans_in(spans, from, to);
        let w = runs_width(hdc, &name[from..to], &part, fonts);
        let left = if w < avail {
            rc.left + (avail - w) / 2
        } else {
            rc.left
        };
        let r = RECT {
            left,
            top,
            right: rc.right,
            bottom: top + fonts.height,
        };
        paint::runs(hdc, &r, &name[from..to], &part, fonts, false);
    };
    line(0, cut, rc.top);
    line(cut, name.len(), rc.top + fonts.height);
}

/// The width of `text` drawn with its `spans` in bold.
fn runs_width(hdc: HDC, text: &[u16], spans: &[Span], fonts: &Fonts) -> i32 {
    let mut total = 0;
    let mut at = 0;
    let mut part = |s: usize, e: usize, bold: bool| {
        if e > s {
            // One of the window's fonts into the DC.
            gdi::select(hdc, if bold { fonts.bold } else { fonts.normal });
            total += paint::width(hdc, &text[s..e]);
        }
    };
    for &(s, e) in spans {
        let (s, e) = (s.min(text.len()), e.min(text.len()));
        part(at, s.max(at), false);
        part(s.max(at), e, true);
        at = at.max(e);
    }
    part(at, text.len(), false);
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_grid_places_items_in_rows() {
        // Large at 100 %: 152 × 180 slots, six a row, scrolled 200 px down.
        let g = Grid {
            per_row: 6,
            first: rect(0, -198, 152, -66),
            cx: 152,
            cy: 180,
        };
        let r = g.icon(7);
        assert_eq!((r.left, r.top, r.right, r.bottom), (152, -18, 304, 114));
        // Row 0 is off the top; rows 1 to 3 show in 520 px, and a part of
        // row 4.
        let client = rect(0, 0, 984, 520);
        assert_eq!(g.visible(&client, 32), 6..30);
        assert_eq!(g.visible(&client, 10), 6..10);
        // Not scrolled: from the first.
        let g = Grid {
            first: rect(0, 2, 152, 134),
            ..g
        };
        assert_eq!(g.visible(&client, 100), 0..24);
        assert_eq!(g.visible(&client, 0), 0..0);
    }

    #[test]
    fn a_small_icon_in_the_jumbo_squares_corner_is_found() {
        // What Windows' jumbo list draws, measured: .cfg, .md and .toml
        // (their 48 px icon, top left), then .txt, .ini and .bat (real
        // 256 px icons, centred or filling the square).
        assert!(corner_only(41, 46, 256));
        assert!(!corner_only(212, 240, 256));
        assert!(!corner_only(212, 256, 256));
        assert!(!corner_only(256, 224, 256));
        // A 128 px icon in the corner still is; one that draws nothing is
        // not (it is drawn as it is).
        assert!(corner_only(128, 128, 256));
        assert!(!corner_only(129, 100, 256));
        assert!(!corner_only(0, 0, 256));
    }

    #[test]
    fn a_loading_icon_stands_in_from_the_smaller_lists_the_largest_first() {
        let all = |wanted| stand_ins(wanted).collect::<Vec<_>>();
        assert_eq!(all(SHIL_JUMBO), [SHIL_EXTRALARGE, SHIL_LARGE, SHIL_SMALL]);
        assert_eq!(all(SHIL_EXTRALARGE), [SHIL_LARGE, SHIL_SMALL]);
        assert_eq!(all(SHIL_LARGE), [SHIL_SMALL]);
        assert!(all(SHIL_SMALL).is_empty());
    }

    #[test]
    fn spans_follow_the_line() {
        let spans = [(0, 3), (5, 12), (14, 16)];
        assert_eq!(spans_in(&spans, 0, 8), vec![(0, 3), (5, 8)]);
        assert_eq!(spans_in(&spans, 8, 20), vec![(0, 4), (6, 8)]);
        assert_eq!(spans_in(&spans, 3, 5), Vec::<Span>::new());
    }
}
