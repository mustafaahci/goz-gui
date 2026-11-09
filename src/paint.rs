//! Drawing: result rows with their matches in bold, the search band, and
//! the dark menu bar.

use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DC_BRUSH, DT_CENTER,
    DT_HIDEPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawFocusRect,
    DrawTextW, ETO_CLIPPED, EndPaint, ExtTextOutW, FillRect, GetDC, GetStockObject,
    GetTextExtentExPointW, GetTextExtentPoint32W, GetWindowDC, HDC, HFONT, MapWindowPoints,
    OffsetRect, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, SetBkMode, SetDCBrushColor,
    SetTextColor, SetViewportOrgEx, TRANSPARENT,
};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ILD_TRANSPARENT, ImageList_Draw, ImageList_GetIconSize, LVIR_BOUNDS,
    LVM_GETITEMCOUNT, LVM_GETITEMRECT, ODS_DISABLED, ODS_FOCUS, ODS_GRAYED, ODS_HOTLIGHT,
    ODS_INACTIVE, ODS_NOACCEL, ODS_SELECTED,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetMenuBarInfo, GetMenuItemInfoW, GetWindowRect, HMENU, MENUBARINFO,
    MENUITEMINFOW, MIIM_STRING, OBJID_MENU, SendMessageW,
};

use crate::format;
use crate::highlight::Span;
use crate::icons::{PENDING, UNSET};
use crate::theme::Palette;
use crate::window::{App, COL_DATE, COL_EXT, COL_NAME, COL_PATH, COL_SIZE, COL_TYPE, Fonts};
use crate::{copytext, filetype, searchbox, status};

/// The space inside a cell before and after its text, at 96 DPI.
const CELL_PAD: i32 = 6;
/// The space between the Name cell's icon and its text, at 96 DPI.
const ICON_GAP: i32 = 5;

/// How far the Name cell's text starts from the cell's left edge: the
/// padding, the icon and the gap after it. Rename's box sits there.
pub fn name_text_offset(app: &App) -> i32 {
    let (mut cx, mut cy) = (0, 0);
    // SAFETY: reads the size of the system image list's icons.
    unsafe { ImageList_GetIconSize(app.icons.list, &mut cx, &mut cy) };
    app.scale(CELL_PAD) + cx + app.scale(ICON_GAP)
}

/// Fills `rc` with `colour` through the DC's own brush, so no brush is made.
pub fn fill(hdc: HDC, rc: &RECT, colour: u32) {
    // SAFETY: the stock DC brush paints in the colour last set on this DC.
    unsafe {
        SetDCBrushColor(hdc, colour);
        FillRect(hdc, rc, GetStockObject(DC_BRUSH));
    }
}

/// How a row's state looks.
#[derive(Clone, Copy)]
pub struct RowState {
    pub selected: bool,
    /// The list has the keyboard focus.
    pub focused: bool,
    /// Under the mouse.
    pub hot: bool,
}

/// The background of one cell. A selection covers the whole row; otherwise
/// the sorted column stands out, and the row under the mouse lights up.
pub fn cell_colour(pal: &Palette, state: RowState, sorted: bool) -> u32 {
    match (state.selected, state.focused, state.hot, sorted) {
        (true, true, _, _) => pal.sel,
        (true, false, _, _) => pal.sel_inactive,
        (false, _, true, true) => pal.hover_sort,
        (false, _, true, false) => pal.hover,
        (false, _, false, true) => pal.sort_bg,
        (false, _, false, false) => pal.bg,
    }
}

/// The results list's rows.
pub fn row(app: &App, dis: &DRAWITEMSTRUCT) {
    let index = dis.itemID as usize;
    let hdc = dis.hDC;
    let rc = dis.rcItem;
    let pal = app.pal.get();
    let cols = app.column_rects(&rc);
    let state = RowState {
        selected: dis.itemState & ODS_SELECTED != 0,
        // SAFETY: reads this thread's focus.
        focused: unsafe { GetFocus() } == app.list,
        hot: app.hot_row.get() == Some(index),
    };
    let sorted = app.sorted_column();
    // The cells, then list background past the last column: a selection
    // spans the columns only.
    let mut span = RECT {
        left: rc.right,
        right: rc.left,
        ..rc
    };
    for &(col, cell) in cols.iter() {
        fill(hdc, &cell, cell_colour(&pal, state, col == sorted));
        span.left = span.left.min(cell.left);
        span.right = span.right.max(cell.right);
    }
    if span.right < rc.right {
        fill(
            hdc,
            &RECT {
                left: span.right,
                ..rc
            },
            pal.bg,
        );
    }
    let m = app.model.borrow();
    let Some(row) = m.row(index) else {
        drop(m);
        app.request_rows(index, index);
        return;
    };
    let fonts = app.fonts.borrow();
    let mut scratch = app.scratch.borrow_mut();
    let scratch = &mut *scratch;
    m.highlight.spans(
        &row.path,
        row.name_start as usize,
        &mut scratch.folded,
        &mut scratch.name_spans,
        &mut scratch.parent_spans,
    );
    let pad = app.scale(CELL_PAD);
    // SAFETY: draws into the list's DC; the old font is put back at the end.
    unsafe {
        let old_font = SelectObject(hdc, fonts.normal);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, pal.fg);
        for &(col, cell) in cols.iter() {
            if cell.right - cell.left <= 2 * pad {
                continue;
            }
            let mut inner = RECT {
                left: cell.left + pad,
                right: cell.right - pad,
                ..cell
            };
            match col {
                COL_NAME => {
                    let (icon, dropped) = app.icons.for_row(row, m.shown, index);
                    // A lookup pushed out of the full queue is asked for
                    // again when its row is drawn again, not left pending.
                    if let Some(other) = dropped.and_then(|i| m.peek(i))
                        && other.icon.get() == PENDING
                    {
                        other.icon.set(UNSET);
                    }
                    let (mut cx, mut cy) = (0, 0);
                    ImageList_GetIconSize(app.icons.list, &mut cx, &mut cy);
                    if inner.right - inner.left > cx {
                        let y = rc.top + (rc.bottom - rc.top - cy) / 2;
                        ImageList_Draw(app.icons.list, icon, hdc, inner.left, y, ILD_TRANSPARENT);
                    }
                    inner.left += cx + app.scale(ICON_GAP);
                    runs(hdc, &inner, row.name(), &scratch.name_spans, &fonts, false);
                }
                COL_PATH => runs(
                    hdc,
                    &inner,
                    row.parent(),
                    &scratch.parent_spans,
                    &fonts,
                    false,
                ),
                COL_SIZE => {
                    if let (false, Some(size)) = (row.is_dir, row.size) {
                        let text: Vec<u16> = format::size_kb(size).encode_utf16().collect();
                        runs(hdc, &inner, &text, &[], &fonts, true);
                    }
                }
                COL_EXT => runs(hdc, &inner, filetype::of_row(row), &[], &fonts, false),
                // Looked up once per extension (`filetype`), then kept.
                COL_TYPE => runs(hdc, &inner, &filetype::name(row), &[], &fonts, false),
                COL_DATE => {
                    if let Some(ft) = row.mtime {
                        scratch.text.clear();
                        format::date(ft, &mut scratch.text);
                        runs(hdc, &inner, &scratch.text, &[], &fonts, false);
                    }
                }
                _ => {}
            }
        }
        if dis.itemState & ODS_FOCUS != 0 && state.focused {
            DrawFocusRect(hdc, &span);
        }
        SelectObject(hdc, old_font);
    }
}

pub fn width(hdc: HDC, text: &[u16]) -> i32 {
    let mut size = SIZE::default();
    // SAFETY: measures `text` in the DC's current font.
    unsafe { GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut size) };
    size.cx
}

/// A cell's text in full: what it says, and where it goes in the list's
/// client coordinates when it shows whole.
pub struct CellText {
    pub text: Vec<u16>,
    /// The text's left edge and top (the row's text line), and its width.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    /// The width the cell needs to show it all, padding (and Name's icon)
    /// included; 0 for an empty cell.
    pub need: i32,
}

/// Measures cell `col` of `row` (`row_rc`, in the list's client
/// coordinates; the cell's left edge at `left`) as `row` draws it: the
/// text with its matches in bold, inside the padding, after Name's icon.
pub fn cell_text(
    app: &App,
    m: &crate::model::Model,
    row: &crate::client::Row,
    col: usize,
    row_rc: &RECT,
    left: i32,
) -> CellText {
    let mut text = Vec::new();
    copytext::push(row, copytext::Field::of_column(col), &mut text);
    let (mut folded, mut name_spans, mut parent_spans) = (Vec::new(), Vec::new(), Vec::new());
    if col == COL_NAME || col == COL_PATH {
        m.highlight.spans(
            &row.path,
            row.name_start as usize,
            &mut folded,
            &mut name_spans,
            &mut parent_spans,
        );
    }
    let spans: &[Span] = match col {
        COL_NAME => &name_spans,
        COL_PATH => &parent_spans,
        _ => &[],
    };
    let fonts = app.fonts.borrow();
    // SAFETY: a DC of this thread's list, measured in and given back; the
    // old font is put back first.
    let w = unsafe {
        let hdc = GetDC(app.list);
        let old = SelectObject(hdc, fonts.normal);
        let w = parts(&text, spans, &fonts)
            .iter()
            .map(|&(s, e, f)| {
                SelectObject(hdc, f);
                width(hdc, &text[s..e])
            })
            .sum::<i32>();
        SelectObject(hdc, old);
        ReleaseDC(app.list, hdc);
        w
    };
    let pad = app.scale(CELL_PAD);
    let lead = if col == COL_NAME {
        let (mut cx, mut cy) = (0, 0);
        // SAFETY: reads the size of the system image list's icons.
        unsafe { ImageList_GetIconSize(app.icons.list, &mut cx, &mut cy) };
        cx + app.scale(ICON_GAP)
    } else {
        0
    };
    CellText {
        x: left + pad + lead,
        y: row_rc.top + (row_rc.bottom - row_rc.top - fonts.height) / 2,
        width: w,
        need: if text.is_empty() {
            0
        } else {
            2 * pad + lead + w
        },
        text,
    }
}

/// `text` cut where `spans` start and end, each part with its font: bold
/// over the spans.
fn parts(text: &[u16], spans: &[Span], fonts: &Fonts) -> Vec<(usize, usize, HFONT)> {
    let mut parts: Vec<(usize, usize, HFONT)> = Vec::with_capacity(spans.len() * 2 + 1);
    let mut at = 0;
    for &(s, e) in spans {
        let (s, e) = (s.min(text.len()), e.min(text.len()));
        if s > at {
            parts.push((at, s, fonts.normal));
        }
        if e > s.max(at) {
            parts.push((s.max(at), e, fonts.bold));
        }
        at = at.max(e);
    }
    if at < text.len() {
        parts.push((at, text.len(), fonts.normal));
    }
    parts
}

/// Draws `text` in `rc`, bold over `spans`, ending in an ellipsis when it
/// does not fit. `right` aligns it to the right edge when it fits.
fn runs(hdc: HDC, rc: &RECT, text: &[u16], spans: &[Span], fonts: &Fonts, right: bool) {
    if text.is_empty() || rc.right <= rc.left {
        return;
    }
    let parts = parts(text, spans, fonts);
    let y = rc.top + (rc.bottom - rc.top - fonts.height) / 2;
    // SAFETY: draws into `hdc`, clipped to `rc`.
    unsafe {
        let draw = |x: i32, s: &[u16]| {
            ExtTextOutW(
                hdc,
                x,
                y,
                ETO_CLIPPED,
                rc,
                s.as_ptr(),
                s.len() as u32,
                core::ptr::null(),
            );
        };
        let widths: Vec<i32> = parts
            .iter()
            .map(|&(s, e, f)| {
                SelectObject(hdc, f);
                width(hdc, &text[s..e])
            })
            .collect();
        let total: i32 = widths.iter().sum();
        let avail = rc.right - rc.left;
        if total <= avail {
            let mut x = if right { rc.right - total } else { rc.left };
            for (&(s, e, f), w) in parts.iter().zip(&widths) {
                SelectObject(hdc, f);
                draw(x, &text[s..e]);
                x += w;
            }
        } else {
            const ELLIPSIS: [u16; 1] = [0x2026];
            SelectObject(hdc, fonts.normal);
            let limit = rc.right - width(hdc, &ELLIPSIS);
            let mut x = rc.left;
            for (&(s, e, f), &w) in parts.iter().zip(&widths) {
                SelectObject(hdc, f);
                if x + w <= limit {
                    draw(x, &text[s..e]);
                    x += w;
                    continue;
                }
                let room = limit - x;
                if room > 0 {
                    let mut fit = 0;
                    let mut size = SIZE::default();
                    GetTextExtentExPointW(
                        hdc,
                        text[s..e].as_ptr(),
                        (e - s) as i32,
                        room,
                        &mut fit,
                        core::ptr::null_mut(),
                        &mut size,
                    );
                    let fit = &text[s..s + fit.max(0) as usize];
                    draw(x, fit);
                    x += width(hdc, fit);
                }
                break;
            }
            SelectObject(hdc, fonts.normal);
            draw(x, &ELLIPSIS);
        }
        SelectObject(hdc, fonts.normal);
    }
}

/// Whether two rectangles share any pixel.
fn overlaps(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// The main window's own surface: the search band and the status bar.
///
/// Only the invalid part is drawn, through a buffer of its size: the status
/// bar repaints on every selection change and block that arrives, and a
/// buffer of the whole client (tens of MB on a large screen) made for each
/// of those would cost milliseconds apiece.
pub fn window(app: &App) {
    // SAFETY: a standard double-buffered WM_PAINT; every GDI object made
    // here is released before returning. The buffer's origin is moved to
    // the invalid rectangle's corner, so drawing stays in client
    // coordinates.
    unsafe {
        let mut ps: PAINTSTRUCT = core::mem::zeroed();
        let hdc = BeginPaint(app.hwnd, &mut ps);
        let dirty = ps.rcPaint;
        let (w, h) = (dirty.right - dirty.left, dirty.bottom - dirty.top);
        if w <= 0 || h <= 0 {
            EndPaint(app.hwnd, &ps);
            return;
        }
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        SetViewportOrgEx(mem, -dirty.left, -dirty.top, core::ptr::null_mut());

        let pal = app.pal.get();
        fill(mem, &dirty, pal.chrome);
        let search = app.search_rc.get();
        if overlaps(&search, &dirty) {
            fill(mem, &search, pal.border);
            let inner = RECT {
                left: search.left + 1,
                top: search.top + 1,
                right: search.right - 1,
                bottom: search.bottom - 1,
            };
            fill(mem, &inner, pal.bg);
            searchbox::paint(app, mem);
        }
        if let Some((back, forward)) = searchbox::nav_of(app)
            && overlaps(
                &RECT {
                    right: forward.right,
                    ..back
                },
                &dirty,
            )
        {
            searchbox::paint_nav(app, mem);
        }
        if overlaps(&app.status_rc.get(), &dirty) {
            status::paint(app, mem);
        }

        BitBlt(
            hdc, dirty.left, dirty.top, w, h, mem, dirty.left, dirty.top, SRCCOPY,
        );
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(app.hwnd, &ps);
    }
}

/// Clears the list below its last row, where the visual style would draw
/// each column's edge on down to the bottom. Called at the list's
/// post-paint stage, so it draws into the list's own paint buffer.
pub fn below_rows(app: &App, hdc: HDC) {
    let mut client = RECT::default();
    let mut header = RECT::default();
    // SAFETY: reads the sizes of this thread's list and header.
    unsafe {
        GetClientRect(app.list, &mut client);
        GetWindowRect(app.header, &mut header);
    }
    let mut top = header.bottom - header.top;
    let count = send(app.list, LVM_GETITEMCOUNT);
    if count > 0 {
        let mut last = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        // SAFETY: LVM_GETITEMRECT fills the RECT passed for an existing row.
        unsafe {
            SendMessageW(
                app.list,
                LVM_GETITEMRECT,
                (count - 1) as usize,
                (&raw mut last) as isize,
            )
        };
        top = top.max(last.bottom);
    }
    if top < client.bottom {
        fill(hdc, &RECT { top, ..client }, app.pal.get().bg);
    }
}

/// When the list is empty because of a failure, says why in the middle of it.
pub fn empty_list(app: &App) {
    if send(app.list, LVM_GETITEMCOUNT) != 0 {
        return;
    }
    let text = match &app.model.borrow().failure {
        Some((_, text)) => text.clone(),
        None => return,
    };
    let mut t: Vec<u16> = text.encode_utf16().collect();
    let fonts = app.fonts.borrow();
    // SAFETY: draws on the list after its own paint; the DC is released.
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(app.list, &mut rc);
        let mut header = RECT::default();
        GetWindowRect(app.header, &mut header);
        rc.top += header.bottom - header.top + app.scale(24);
        rc.left += app.scale(24);
        rc.right -= app.scale(24);
        let hdc = GetDC(app.list);
        let old = SelectObject(hdc, fonts.normal);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.pal.get().dim);
        DrawTextW(
            hdc,
            t.as_mut_ptr(),
            t.len() as i32,
            &mut rc,
            DT_CENTER | DT_WORDBREAK,
        );
        SelectObject(hdc, old);
        ReleaseDC(app.list, hdc);
    }
}

fn send(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32) -> isize {
    // SAFETY: a parameterless query message to a window of this thread.
    unsafe { SendMessageW(hwnd, msg, 0, 0) }
}

// The menu bar has no dark theme of its own. Windows draws it through these
// undocumented messages, which let the app paint the bar and its items; the
// layout of their structs has held since Windows 10 1809.

pub const WM_UAHDRAWMENU: u32 = 0x0091;
pub const WM_UAHDRAWMENUITEM: u32 = 0x0092;

#[repr(C)]
pub struct UahMenu {
    pub hmenu: HMENU,
    pub hdc: HDC,
    pub flags: u32,
}

#[repr(C)]
pub struct UahMenuItem {
    pub position: i32,
    pub metrics: [u32; 8],
    pub popup: [u32; 5],
}

#[repr(C)]
pub struct UahDrawMenuItem {
    pub dis: DRAWITEMSTRUCT,
    pub um: UahMenu,
    pub umi: UahMenuItem,
}

/// The menu bar's rectangle in window coordinates.
fn menubar_rect(app: &App) -> RECT {
    // SAFETY: plain out-structs for this thread's window.
    unsafe {
        let mut mbi: MENUBARINFO = core::mem::zeroed();
        mbi.cbSize = size_of::<MENUBARINFO>() as u32;
        GetMenuBarInfo(app.hwnd, OBJID_MENU, 0, &mut mbi);
        let mut wr = RECT::default();
        GetWindowRect(app.hwnd, &mut wr);
        let mut rc = mbi.rcBar;
        OffsetRect(&mut rc, -wr.left, -wr.top);
        rc
    }
}

pub fn menubar(app: &App, um: &UahMenu) {
    let mut rc = menubar_rect(app);
    rc.top -= 1;
    fill(um.hdc, &rc, app.pal.get().chrome);
}

pub fn menubar_item(app: &App, item: &UahDrawMenuItem) {
    let mut buf = [0u16; 256];
    // SAFETY: reads the item's text into `buf`, whose length is passed; then
    // paints into the DC Windows handed over.
    unsafe {
        let mut mii: MENUITEMINFOW = core::mem::zeroed();
        mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
        mii.fMask = MIIM_STRING;
        mii.dwTypeData = buf.as_mut_ptr();
        mii.cch = buf.len() as u32 - 1;
        GetMenuItemInfoW(item.um.hmenu, item.umi.position as u32, 1, &mut mii);
        let state = item.dis.itemState;
        let pal = app.pal.get();
        let hot = state & (ODS_HOTLIGHT | ODS_SELECTED) != 0;
        let mut rc = item.dis.rcItem;
        fill(item.um.hdc, &rc, if hot { pal.hot } else { pal.chrome });
        let dim = state & (ODS_INACTIVE | ODS_GRAYED | ODS_DISABLED) != 0;
        SetBkMode(item.um.hdc, TRANSPARENT as i32);
        SetTextColor(item.um.hdc, if dim { pal.dim } else { pal.fg });
        let mut flags = DT_CENTER | DT_SINGLELINE | DT_VCENTER;
        if state & ODS_NOACCEL != 0 {
            flags |= DT_HIDEPREFIX;
        }
        DrawTextW(item.um.hdc, buf.as_mut_ptr(), -1, &mut rc, flags);
    }
}

/// Covers the light line Windows draws under the menu bar.
pub fn menubar_line(app: &App) {
    // SAFETY: paints one line of this window's frame; the DC is released.
    unsafe {
        let mut client = RECT::default();
        GetClientRect(app.hwnd, &mut client);
        MapWindowPoints(app.hwnd, core::ptr::null_mut(), (&raw mut client).cast(), 2);
        let mut wr = RECT::default();
        GetWindowRect(app.hwnd, &mut wr);
        OffsetRect(&mut client, -wr.left, -wr.top);
        let line = RECT {
            top: client.top - 1,
            bottom: client.top,
            ..client
        };
        let hdc = GetWindowDC(app.hwnd);
        fill(hdc, &line, app.pal.get().chrome);
        ReleaseDC(app.hwnd, hdc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::rgb;

    fn state(selected: bool, focused: bool, hot: bool) -> RowState {
        RowState {
            selected,
            focused,
            hot,
        }
    }

    #[test]
    fn selection_wins_and_follows_focus() {
        for pal in [Palette::new(true), Palette::new(false)] {
            for sorted in [false, true] {
                for hot in [false, true] {
                    assert_eq!(cell_colour(&pal, state(true, true, hot), sorted), pal.sel);
                    assert_eq!(
                        cell_colour(&pal, state(true, false, hot), sorted),
                        pal.sel_inactive
                    );
                }
            }
        }
    }

    #[test]
    fn sorted_column_and_mouseover() {
        let pal = Palette::new(true);
        assert_eq!(cell_colour(&pal, state(false, true, false), false), pal.bg);
        assert_eq!(
            cell_colour(&pal, state(false, true, false), true),
            pal.sort_bg
        );
        assert_eq!(
            cell_colour(&pal, state(false, false, true), false),
            pal.hover
        );
        assert_eq!(
            cell_colour(&pal, state(false, false, true), true),
            pal.hover_sort
        );
    }

    #[test]
    fn only_what_is_invalid_is_drawn() {
        let rc = |left, top, right, bottom| RECT {
            left,
            top,
            right,
            bottom,
        };
        let status = rc(0, 600, 1000, 630);
        // A status bar update leaves the search band alone, and the reverse.
        assert!(overlaps(&status, &rc(0, 600, 1000, 630)));
        assert!(!overlaps(&rc(6, 6, 994, 34), &status));
        // Edges that only touch do not overlap; one pixel in does.
        assert!(!overlaps(&rc(0, 590, 1000, 600), &status));
        assert!(overlaps(&rc(0, 590, 1000, 601), &status));
        assert!(overlaps(&rc(10, 615, 20, 625), &status));
        assert!(!overlaps(&rc(0, 0, 0, 0), &status), "an empty rectangle");
    }

    #[test]
    fn palettes_match_the_spec() {
        let dark = Palette::new(true);
        assert_eq!(dark.bg, rgb(0, 0, 0));
        assert_eq!(dark.sort_bg, rgb(0x18, 0x18, 0x18));
        assert_eq!(dark.sel, rgb(0x72, 0x72, 0x72));
        assert_eq!(dark.sel_inactive, rgb(0x4a, 0x4a, 0x4a));
        assert_eq!(dark.header_sort, rgb(0x39, 0x39, 0x39));
        let light = Palette::new(false);
        assert_eq!(light.sort_bg, rgb(0xf7, 0xf7, 0xf7));
        assert_eq!(light.sel, rgb(0xcc, 0xe8, 0xff));
        assert_eq!(light.header, rgb(0xff, 0xff, 0xff));
        assert_eq!(light.chrome, rgb(0xf0, 0xf0, 0xf0));
    }
}
