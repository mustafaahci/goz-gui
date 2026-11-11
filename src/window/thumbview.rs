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
    AC_SRC_ALPHA, AC_SRC_OVER, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, DrawFocusRect,
    GdiFlush, GetTextExtentExPointW, HDC, InvalidateRect, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_POSTPAINT, CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_NOTIFYPOSTPAINT, CDRF_SKIPDEFAULT, CLR_DEFAULT, CLR_NONE, HIMAGELIST, ILC_COLOR32,
    ILD_ASYNC, ILD_SCALE, ILD_TRANSPARENT, IMAGELISTDRAWPARAMS, ImageList_Create,
    ImageList_Destroy, ImageList_DrawIndirect, ImageList_GetIconSize, LVA_DEFAULT, LVIR_ICON,
    LVIS_FOCUSED, LVIS_SELECTED, LVM_ARRANGE, LVM_ENSUREVISIBLE, LVM_GETCOUNTPERPAGE,
    LVM_GETITEMCOUNT, LVM_GETITEMRECT, LVM_GETITEMSPACING, LVM_GETITEMSTATE, LVM_GETNEXTITEM,
    LVM_GETTOPINDEX, LVM_REDRAWITEMS, LVM_SCROLL, LVM_SETICONSPACING, LVM_SETIMAGELIST,
    LVNI_FOCUSED, LVS_AUTOARRANGE, LVS_ICON, LVS_OWNERDRAWFIXED, LVS_REPORT, LVS_TYPEMASK,
    LVSIL_NORMAL, NMCUSTOMDRAW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::Shell::SHGetImageList;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetClientRect, GetWindowLongPtrW, SetWindowLongPtrW, WM_APP, WM_SETFONT,
};
use windows_sys::core::GUID;

use super::{App, send};
use crate::highlight::Span;
use crate::paint::{self, RowState};
use crate::status::ViewButton;
use crate::thumbs::{Bitmap, Picture, ThumbReply};
use crate::view::{self, View};
use crate::window::Fonts;

/// `IID_IImageList`, which `SHGetImageList` hands the system lists out as.
const IID_IMAGE_LIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);
/// `SHIL_EXTRALARGE` (48 px) and `SHIL_JUMBO` (256 px).
const SHIL_EXTRALARGE: i32 = 2;
const SHIL_JUMBO: i32 = 4;

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
        let focused = send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        );
        let anchor = usize::try_from(focused)
            .ok()
            .filter(|f| shown.contains(f))
            .unwrap_or(shown.start);
        self.put_view();
        if send(self.list, LVM_GETITEMCOUNT, 0, 0) <= 0 {
            return;
        }
        if view.thumbnails {
            send(self.list, LVM_ENSUREVISIBLE, anchor, 0);
        } else {
            // The anchor at the top, as the rows' own scroll puts it
            // (ensure-visible right after the switch would leave the list
            // at its old row).
            let top = send(self.list, LVM_GETTOPINDEX, 0, 0).max(0) as usize;
            let rows = anchor as i64 - top as i64;
            let dy = (rows * i64::from(self.row_height())).clamp(-(1 << 30), 1 << 30);
            if dy != 0 {
                send(self.list, LVM_SCROLL, 0, dy as isize);
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
            // The shell's big icon lists, asked for here rather than in
            // the middle of a paint.
            self.big_icons(false);
            self.big_icons(true);
            let side = view::side(v.size, dpi);
            let line = self.fonts.borrow().height;
            let (cx, cy) = view::spacing(v.size, dpi, line);
            // SAFETY: an empty image list the list owns from here on; the
            // one it gives back was made here before and is not used.
            unsafe {
                let images = ImageList_Create(side, side, ILC_COLOR32, 0, 1);
                let old = send(
                    self.list,
                    LVM_SETIMAGELIST,
                    LVSIL_NORMAL as usize,
                    images as isize,
                );
                if old != 0 {
                    ImageList_Destroy(old as HIMAGELIST);
                }
            }
            send(
                self.list,
                LVM_SETICONSPACING,
                0,
                (cx as u16 as isize) | ((cy as u16 as isize) << 16),
            );
            // Icon view, its items in rows in order (auto-arranged), and not
            // owner-drawn: owner draw is Details' only, and left on it
            // keeps the list from drawing (or placing) its icons.
            self.set_list_style(LVS_ICON | LVS_AUTOARRANGE);
            // Lays the items out on the new spacing.
            send(self.list, LVM_ARRANGE, LVA_DEFAULT as usize, 0);
        } else {
            self.set_list_style(LVS_REPORT | LVS_OWNERDRAWFIXED);
            // The rows are measured again (WM_MEASUREITEM) for their height.
            let font = self.fonts.borrow().normal;
            send(self.list, WM_SETFONT, font as usize, 1);
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(self.list, core::ptr::null(), 1) };
    }

    /// Sets the list's view style bits (the view, its arrangement, owner
    /// draw) to `bits`, which the list takes as a change of view.
    fn set_list_style(&self, bits: u32) {
        let mask = (LVS_TYPEMASK | LVS_AUTOARRANGE | LVS_OWNERDRAWFIXED) as isize;
        // SAFETY: changes the style of this thread's list, which answers
        // with WM_STYLECHANGED.
        unsafe {
            let style = GetWindowLongPtrW(self.list, GWL_STYLE);
            let new = (style & !mask) | bits as isize;
            if new != style {
                SetWindowLongPtrW(self.list, GWL_STYLE, new);
            }
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
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        if !self.view.get().thumbnails {
            let top = send(self.list, LVM_GETTOPINDEX, 0, 0).max(0) as usize;
            let page = send(self.list, LVM_GETCOUNTPERPAGE, 0, 0).max(0) as usize;
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
        let mut client = RECT::default();
        // SAFETY: reads the size of this thread's list.
        unsafe { GetClientRect(self.list, &mut client) };
        client
    }

    /// Item `index`'s icon rectangle as the list has it.
    fn icon_rect(&self, index: usize) -> Option<RECT> {
        let mut icon = RECT {
            left: LVIR_ICON as i32,
            ..RECT::default()
        };
        (send(self.list, LVM_GETITEMRECT, index, (&raw mut icon) as isize) != 0).then_some(icon)
    }

    /// The icon view's grid as the list laid it out now, scrolled: asked
    /// of the list item by item, which makes it measure their names (and
    /// send custom draws for that), so never while it paints items.
    fn grid(&self) -> Option<Grid> {
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        let first = self.icon_rect(0)?;
        let spacing = send(self.list, LVM_GETITEMSPACING, 0, 0) as usize;
        let cx = ((spacing & 0xffff) as i32).max(1);
        let cy = (((spacing >> 16) & 0xffff) as i32).max(1);
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
                let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
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
        let spacing = send(self.list, LVM_GETITEMSPACING, 0, 0) as usize;
        Some(self.cell_of(&icon, (spacing & 0xffff) as i32))
    }

    /// Draws item `index`: the cell's background as the row's would be,
    /// the thumbnail (the file's icon until it comes), and the name in up
    /// to two lines with its matches in bold.
    fn paint_cell(&self, hdc: HDC, index: usize, grid: &Grid) {
        let geo = self.cell_of(&grid.icon(index), grid.cx);
        // The list is asked first: nothing is borrowed while it may call
        // back.
        let bits = send(
            self.list,
            LVM_GETITEMSTATE,
            index,
            (LVIS_SELECTED | LVIS_FOCUSED) as isize,
        ) as u32;
        let pal = self.pal.get();
        // SAFETY: reads this thread's focus.
        let list_focused = unsafe { GetFocus() } == self.list;
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
            // SAFETY: draws into the list's paint DC.
            unsafe { DrawFocusRect(hdc, &geo.cell) };
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
    /// 48 px one stands in for the 256 px one if it is loaded; else the cell
    /// shows no icon for that moment.
    fn draw_big_icon(&self, hdc: HDC, icon: i32, square: &RECT) {
        if icon < 0 {
            return;
        }
        let side = square.right - square.left;
        let mut jumbo = side >= self.scale(96);
        // Whether the icon drawn is the one wanted, to be loaded if it is
        // not, or a stand-in, drawn only if it is.
        let mut wanted = true;
        if jumbo {
            let list = self.big_icons(true);
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
        let list = self.big_icons(jumbo);
        if list == 0 {
            return;
        }
        let (mut cx, mut cy) = (0, 0);
        // SAFETY: reads the size of a system image list.
        unsafe { ImageList_GetIconSize(list, &mut cx, &mut cy) };
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
            cbSize: size_of::<IMAGELISTDRAWPARAMS>() as u32,
            himl: list,
            i: icon,
            hdcDst: hdc,
            x,
            y,
            cx: w,
            cy: h,
            rgbBk: CLR_NONE as u32,
            rgbFg: CLR_DEFAULT as u32,
            fStyle: style,
            // SAFETY: the rest of the struct is plain data.
            ..unsafe { core::mem::zeroed() }
        };
        self.big_loads.draw(&params, wanted);
    }

    /// Big icons were loaded on their thread: the thumbnails paint again,
    /// with them.
    pub(super) fn big_icons_loaded(&self) {
        if self.big_loads.arrived() && self.view.get().thumbnails {
            // SAFETY: repaints this thread's list.
            unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
        }
    }

    /// The system image list of 48 px icons, or of 256 px ones (`jumbo`),
    /// asked for once.
    fn big_icons(&self, jumbo: bool) -> HIMAGELIST {
        let mut lists = self.big_icons.get();
        let slot = usize::from(jumbo);
        if lists[slot] == 0 {
            let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
            let which = if jumbo { SHIL_JUMBO } else { SHIL_EXTRALARGE };
            // SAFETY: the shell hands out its own image list, which lives as
            // long as the process; it is kept, never released.
            if unsafe { SHGetImageList(which, &IID_IMAGE_LIST, &mut ppv) } >= 0 {
                lists[slot] = ppv as usize;
                self.big_icons.set(lists);
            }
        }
        lists[slot] as HIMAGELIST
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
            send(self.list, LVM_REDRAWITEMS, index, index as isize);
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
    // SAFETY: a memory DC made and deleted here holds the bitmap, which the
    // cache keeps alive across the call and selects nowhere else.
    unsafe {
        let mem = CreateCompatibleDC(hdc);
        let old = SelectObject(mem, bitmap.handle());
        AlphaBlend(
            hdc,
            dst.left,
            dst.top,
            dst.right - dst.left,
            dst.bottom - dst.top,
            mem,
            0,
            0,
            bitmap.width,
            bitmap.height,
            blend,
        );
        SelectObject(mem, old);
        DeleteDC(mem);
    }
}

/// Whether the system jumbo list's icon `icon` is a small icon in the top
/// left corner of an otherwise empty square; `None` while the icon is not
/// loaded. Worked out once per icon (two draws into a scratch bitmap) and
/// kept: the system list's indexes last as long as the process.
fn jumbo_is_padded(list: HIMAGELIST, icon: i32) -> Option<bool> {
    thread_local! {
        static PADDED: RefCell<HashMap<i32, bool>> = RefCell::new(HashMap::new());
    }
    if list == 0 {
        return Some(false);
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
fn drawn_extent(list: HIMAGELIST, icon: i32) -> Extent {
    let (mut cx, mut cy) = (0, 0);
    // SAFETY: a DIB section and a memory DC made, used and deleted here.
    // The pixels are touched only through slices made between the draws
    // (after GdiFlush), each covering exactly the section's cx × cy 32-bit
    // pixels.
    unsafe {
        ImageList_GetIconSize(list, &mut cx, &mut cy);
        if cx <= 0 || cy <= 0 {
            return Extent::Unknown;
        }
        let mut bi: BITMAPINFO = core::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: cx,
            // Top-down.
            biHeight: -cy,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..core::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let dib = CreateDIBSection(
            core::ptr::null_mut(),
            &bi,
            DIB_RGB_COLORS,
            &mut bits,
            core::ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            if !dib.is_null() {
                DeleteObject(dib);
            }
            return Extent::Unknown;
        }
        let count = (cx * cy) as usize;
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        let old = SelectObject(dc, dib);
        let params = IMAGELISTDRAWPARAMS {
            cbSize: size_of::<IMAGELISTDRAWPARAMS>() as u32,
            himl: list,
            i: icon,
            hdcDst: dc,
            rgbBk: CLR_NONE as u32,
            rgbFg: CLR_DEFAULT as u32,
            fStyle: ILD_TRANSPARENT | ILD_ASYNC,
            ..core::mem::zeroed()
        };
        let (mut right, mut bottom) = (0, 0);
        let mut loaded = true;
        for back in [0u32, 0x00ff_ffff] {
            core::slice::from_raw_parts_mut(bits.cast::<u32>(), count).fill(back);
            if ImageList_DrawIndirect(&params) == 0 {
                loaded = false;
                break;
            }
            GdiFlush();
            let pixels = core::slice::from_raw_parts(bits.cast::<u32>(), count);
            for (i, p) in pixels.iter().enumerate() {
                if p & 0x00ff_ffff != back {
                    let (x, y) = (i as i32 % cx, i as i32 / cx);
                    right = right.max(x + 1);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        SelectObject(dc, old);
        DeleteDC(dc);
        DeleteObject(dib);
        if !loaded {
            return Extent::NotLoaded;
        }
        Extent::Drawn {
            right,
            bottom,
            side: cx.min(cy),
        }
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
    // SAFETY: measures and draws into the list's paint DC; the font set
    // here is put back by `runs` (the normal one, the DC's own).
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, colour);
        // The bold font where there are matches: what fits in it fits.
        SelectObject(
            hdc,
            if spans.is_empty() {
                fonts.normal
            } else {
                fonts.bold
            },
        );
        let mut fit = 0;
        let mut size = Default::default();
        GetTextExtentExPointW(
            hdc,
            name.as_ptr(),
            name.len() as i32,
            avail,
            &mut fit,
            core::ptr::null_mut(),
            &mut size,
        );
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
}

/// The width of `text` drawn with its `spans` in bold.
fn runs_width(hdc: HDC, text: &[u16], spans: &[Span], fonts: &Fonts) -> i32 {
    let mut total = 0;
    let mut at = 0;
    let mut part = |s: usize, e: usize, bold: bool| {
        if e > s {
            // SAFETY: selects one of the window's fonts into the DC.
            unsafe { SelectObject(hdc, if bold { fonts.bold } else { fonts.normal }) };
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
    fn spans_follow_the_line() {
        let spans = [(0, 3), (5, 12), (14, 16)];
        assert_eq!(spans_in(&spans, 0, 8), vec![(0, 3), (5, 8)]);
        assert_eq!(spans_in(&spans, 8, 20), vec![(0, 4), (6, 8)]);
        assert_eq!(spans_in(&spans, 3, 5), Vec::<Span>::new());
    }
}
