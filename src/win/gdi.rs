//! GDI: device contexts and the objects drawn with them. GDI checks every
//! handle it is given, so handles are plain values here; what a call reads
//! or writes through a pointer is a Rust reference or slice.

use windows_sys::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    AlphaBlend, BITMAP, BLENDFUNCTION, BeginPaint, BitBlt, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontIndirectW, CreatePen, CreateSolidBrush, DRAW_TEXT_FORMAT,
    DT_MODIFYSTRING, DeleteDC, DeleteObject, DrawFocusRect, DrawTextW, ETO_OPTIONS, EndPaint,
    ExcludeClipRect, ExtTextOutW, FillRect, FrameRect, GET_STOCK_OBJECT_FLAGS, GdiFlush, GetDC,
    GetDeviceCaps, GetObjectW, GetStockObject, GetSysColor, GetSysColorBrush,
    GetTextExtentPoint32W, GetTextMetricsW, GetWindowDC, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
    HPEN, IntersectClipRect, LOGFONTW, LineTo, MoveToEx, PAINTSTRUCT, PEN_STYLE, PatBlt, ROP_CODE,
    Rectangle, ReleaseDC, RestoreDC, SYS_COLOR_INDEX, SaveDC, SelectObject, SetBkColor, SetBkMode,
    SetDCBrushColor, SetTextColor, SetViewportOrgEx, StretchBlt, TEXTMETRICW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DI_FLAGS, DestroyIcon, DrawIconEx, HICON};

/// A window's painting, from BeginPaint to EndPaint (when this drops).
pub struct Paint {
    hwnd: HWND,
    ps: PAINTSTRUCT,
}

impl Paint {
    /// Starts painting the window's invalid area (WM_PAINT).
    pub fn begin(hwnd: HWND) -> Paint {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: an out-struct, kept until EndPaint.
        unsafe { BeginPaint(hwnd, &mut ps) };
        Paint { hwnd, ps }
    }

    /// The DC to paint with (null if BeginPaint failed).
    pub fn hdc(&self) -> HDC {
        self.ps.hdc
    }

    /// The area to paint.
    pub fn area(&self) -> RECT {
        self.ps.rcPaint
    }

    /// Whether the background is to be erased.
    pub fn erase(&self) -> bool {
        self.ps.fErase != 0
    }
}

impl Drop for Paint {
    fn drop(&mut self) {
        // SAFETY: the PAINTSTRUCT BeginPaint filled.
        unsafe { EndPaint(self.hwnd, &self.ps) };
    }
}

/// A window's client-area DC (GetDC), released when this drops.
pub struct WindowDc {
    hwnd: HWND,
    hdc: HDC,
}

impl WindowDc {
    pub fn get(hwnd: HWND) -> WindowDc {
        WindowDc {
            hwnd,
            hdc: get_dc(hwnd),
        }
    }

    /// The whole window's DC, frame included (GetWindowDC).
    pub fn whole(hwnd: HWND) -> WindowDc {
        WindowDc {
            hwnd,
            hdc: window_dc(hwnd),
        }
    }

    pub fn hdc(&self) -> HDC {
        self.hdc
    }
}

impl Drop for WindowDc {
    fn drop(&mut self) {
        release_dc(self.hwnd, self.hdc);
    }
}

/// A memory DC (CreateCompatibleDC), deleted when this drops.
pub struct MemDc(HDC);

impl MemDc {
    pub fn compatible(hdc: HDC) -> MemDc {
        MemDc(create_compatible_dc(hdc))
    }

    pub fn hdc(&self) -> HDC {
        self.0
    }
}

impl Drop for MemDc {
    fn drop(&mut self) {
        delete_dc(self.0);
    }
}

/// A GDI object (font, brush, pen, bitmap) deleted when this drops.
pub struct Owned(HGDIOBJ);

impl Owned {
    pub fn new(obj: HGDIOBJ) -> Owned {
        Owned(obj)
    }

    pub fn get(&self) -> HGDIOBJ {
        self.0
    }

    /// Gives the object up without deleting it.
    pub fn into_raw(self) -> HGDIOBJ {
        let obj = self.0;
        core::mem::forget(self);
        obj
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        delete_object(self.0);
    }
}

/// An object selected into a DC until this drops, when the one it replaced
/// goes back.
pub struct Selected {
    hdc: HDC,
    old: HGDIOBJ,
}

impl Selected {
    pub fn new(hdc: HDC, obj: HGDIOBJ) -> Selected {
        Selected {
            hdc,
            old: select(hdc, obj),
        }
    }
}

impl Drop for Selected {
    fn drop(&mut self) {
        if !self.old.is_null() {
            select(self.hdc, self.old);
        }
    }
}

pub fn get_dc(hwnd: HWND) -> HDC {
    // SAFETY: no pointers.
    unsafe { GetDC(hwnd) }
}

pub fn window_dc(hwnd: HWND) -> HDC {
    // SAFETY: no pointers.
    unsafe { GetWindowDC(hwnd) }
}

pub fn release_dc(hwnd: HWND, hdc: HDC) {
    // SAFETY: no pointers.
    unsafe { ReleaseDC(hwnd, hdc) };
}

pub fn create_compatible_dc(hdc: HDC) -> HDC {
    // SAFETY: no pointers.
    unsafe { CreateCompatibleDC(hdc) }
}

pub fn delete_dc(hdc: HDC) {
    // SAFETY: no pointers.
    unsafe { DeleteDC(hdc) };
}

pub fn create_compatible_bitmap(hdc: HDC, cx: i32, cy: i32) -> HBITMAP {
    // SAFETY: no pointers.
    unsafe { CreateCompatibleBitmap(hdc, cx, cy) }
}

/// Selects `obj` into `hdc`; the object it replaced.
pub fn select(hdc: HDC, obj: HGDIOBJ) -> HGDIOBJ {
    // SAFETY: no pointers.
    unsafe { SelectObject(hdc, obj) }
}

/// Deletes a GDI object (GDI refuses one still selected into a DC).
pub fn delete_object(obj: HGDIOBJ) -> bool {
    // SAFETY: no pointers.
    unsafe { DeleteObject(obj) != 0 }
}

pub fn stock_object(which: GET_STOCK_OBJECT_FLAGS) -> HGDIOBJ {
    // SAFETY: no pointers.
    unsafe { GetStockObject(which) }
}

pub fn sys_color(index: SYS_COLOR_INDEX) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { GetSysColor(index) }
}

/// The system's brush of a colour (not to be deleted).
pub fn sys_color_brush(index: SYS_COLOR_INDEX) -> HBRUSH {
    // SAFETY: no pointers.
    unsafe { GetSysColorBrush(index) }
}

pub fn set_text_color(hdc: HDC, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { SetTextColor(hdc, c) }
}

pub fn set_bk_color(hdc: HDC, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { SetBkColor(hdc, c) }
}

/// TRANSPARENT or OPAQUE; the mode before.
pub fn set_bk_mode(hdc: HDC, mode: i32) -> i32 {
    // SAFETY: no pointers.
    unsafe { SetBkMode(hdc, mode) }
}

/// The colour of the DC's DC_BRUSH stock brush; the one before.
pub fn set_dc_brush_color(hdc: HDC, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { SetDCBrushColor(hdc, c) }
}

/// DrawTextW of `text` (no NUL needed) in `rc`; the text's height. With
/// DT_CALCRECT, `rc` comes back as the text's extent. DT_MODIFYSTRING is
/// ignored: `text` is not written to.
pub fn draw_text(hdc: HDC, text: &[u16], rc: &mut RECT, format: DRAW_TEXT_FORMAT) -> i32 {
    let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
    // SAFETY: `len` units of `text`, read only (DT_MODIFYSTRING is masked
    // off); the rectangle is in-out.
    unsafe { DrawTextW(hdc, text.as_ptr(), len, rc, format & !DT_MODIFYSTRING) }
}

/// `draw_text` of a `&str`.
pub fn draw_str(hdc: HDC, text: &str, rc: &mut RECT, format: DRAW_TEXT_FORMAT) -> i32 {
    let units: Vec<u16> = text.encode_utf16().collect();
    draw_text(hdc, &units, rc, format)
}

/// ExtTextOutW of `text` at `x, y`, clipped or filled to `rc` as `options`
/// say.
pub fn ext_text_out(
    hdc: HDC,
    (x, y): (i32, i32),
    options: ETO_OPTIONS,
    rc: Option<&RECT>,
    text: &[u16],
) -> bool {
    let rc = rc.map_or(core::ptr::null(), |r| r as *const RECT);
    let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
    // SAFETY: `len` units of `text` and a rectangle or null, read during the
    // call; no spacing array.
    unsafe {
        ExtTextOutW(
            hdc,
            x,
            y,
            options,
            rc,
            text.as_ptr(),
            len,
            core::ptr::null(),
        ) != 0
    }
}

/// The size of `text` in the DC's font.
pub fn text_extent(hdc: HDC, text: &[u16]) -> SIZE {
    let mut size = SIZE::default();
    let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
    // SAFETY: `len` units of `text`; an out-struct.
    unsafe { GetTextExtentPoint32W(hdc, text.as_ptr(), len, &mut size) };
    size
}

/// The DC's font's metrics.
pub fn text_metrics(hdc: HDC) -> TEXTMETRICW {
    let mut tm = TEXTMETRICW::default();
    // SAFETY: an out-struct.
    unsafe { GetTextMetricsW(hdc, &mut tm) };
    tm
}

pub fn fill_rect(hdc: HDC, rc: &RECT, brush: HBRUSH) {
    // SAFETY: an in-struct.
    unsafe { FillRect(hdc, rc, brush) };
}

pub fn frame_rect(hdc: HDC, rc: &RECT, brush: HBRUSH) {
    // SAFETY: an in-struct.
    unsafe { FrameRect(hdc, rc, brush) };
}

pub fn draw_focus_rect(hdc: HDC, rc: &RECT) {
    // SAFETY: an in-struct.
    unsafe { DrawFocusRect(hdc, rc) };
}

pub fn rectangle(hdc: HDC, rc: &RECT) {
    // SAFETY: no pointers.
    unsafe { Rectangle(hdc, rc.left, rc.top, rc.right, rc.bottom) };
}

pub fn pat_blt(hdc: HDC, rc: &RECT, rop: ROP_CODE) {
    // SAFETY: no pointers.
    unsafe {
        PatBlt(
            hdc,
            rc.left,
            rc.top,
            rc.right - rc.left,
            rc.bottom - rc.top,
            rop,
        )
    };
}

/// BitBlt of `cx` × `cy` from `src` at `sx, sy` to `dst` at `x, y`.
pub fn bit_blt(
    dst: HDC,
    (x, y, cx, cy): (i32, i32, i32, i32),
    src: HDC,
    (sx, sy): (i32, i32),
    rop: ROP_CODE,
) -> bool {
    // SAFETY: no pointers.
    unsafe { BitBlt(dst, x, y, cx, cy, src, sx, sy, rop) != 0 }
}

pub fn stretch_blt(
    dst: HDC,
    (x, y, cx, cy): (i32, i32, i32, i32),
    src: HDC,
    (sx, sy, scx, scy): (i32, i32, i32, i32),
    rop: ROP_CODE,
) -> bool {
    // SAFETY: no pointers.
    unsafe { StretchBlt(dst, x, y, cx, cy, src, sx, sy, scx, scy, rop) != 0 }
}

pub fn alpha_blend(
    dst: HDC,
    (x, y, cx, cy): (i32, i32, i32, i32),
    src: HDC,
    (sx, sy, scx, scy): (i32, i32, i32, i32),
    blend: BLENDFUNCTION,
) -> bool {
    // SAFETY: no pointers.
    unsafe { AlphaBlend(dst, x, y, cx, cy, src, sx, sy, scx, scy, blend) != 0 }
}

/// Moves the DC's viewport origin; the one before.
pub fn set_viewport_org(hdc: HDC, x: i32, y: i32) -> POINT {
    let mut old = POINT::default();
    // SAFETY: an out-struct.
    unsafe { SetViewportOrgEx(hdc, x, y, &mut old) };
    old
}

pub fn move_to(hdc: HDC, x: i32, y: i32) {
    // SAFETY: no out-point asked for.
    unsafe { MoveToEx(hdc, x, y, core::ptr::null_mut()) };
}

pub fn line_to(hdc: HDC, x: i32, y: i32) {
    // SAFETY: no pointers.
    unsafe { LineTo(hdc, x, y) };
}

pub fn intersect_clip(hdc: HDC, rc: &RECT) {
    // SAFETY: no pointers.
    unsafe { IntersectClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom) };
}

pub fn exclude_clip(hdc: HDC, rc: &RECT) {
    // SAFETY: no pointers.
    unsafe { ExcludeClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom) };
}

pub fn save_dc(hdc: HDC) -> i32 {
    // SAFETY: no pointers.
    unsafe { SaveDC(hdc) }
}

pub fn restore_dc(hdc: HDC, saved: i32) {
    // SAFETY: no pointers.
    unsafe { RestoreDC(hdc, saved) };
}

pub fn device_caps(hdc: HDC, index: i32) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetDeviceCaps(hdc, index) }
}

pub fn create_font(lf: &LOGFONTW) -> HFONT {
    // SAFETY: an in-struct (the face name is read within its array).
    unsafe { CreateFontIndirectW(lf) }
}

pub fn create_solid_brush(c: COLORREF) -> HBRUSH {
    // SAFETY: no pointers.
    unsafe { CreateSolidBrush(c) }
}

pub fn create_pen(style: PEN_STYLE, width: i32, c: COLORREF) -> HPEN {
    // SAFETY: no pointers.
    unsafe { CreatePen(style, width, c) }
}

/// A font's LOGFONTW.
pub fn font_info(font: HFONT) -> Option<LOGFONTW> {
    let mut lf = LOGFONTW::default();
    // SAFETY: an out-struct and its size.
    let n = unsafe { GetObjectW(font, size_of::<LOGFONTW>() as i32, (&raw mut lf).cast()) };
    (n != 0).then_some(lf)
}

/// A bitmap's BITMAP (its size and format; `bmBits` is not to be read).
pub fn bitmap_info(bmp: HBITMAP) -> Option<BITMAP> {
    let mut bm = BITMAP::default();
    // SAFETY: an out-struct and its size.
    let n = unsafe { GetObjectW(bmp, size_of::<BITMAP>() as i32, (&raw mut bm).cast()) };
    (n != 0).then_some(bm)
}

/// Waits for this thread's GDI calls to finish.
pub fn flush() {
    // SAFETY: no pointers.
    unsafe { GdiFlush() };
}

/// DrawIconEx of `icon` at `x, y`, `cx` × `cy`.
pub fn draw_icon(
    hdc: HDC,
    (x, y, cx, cy): (i32, i32, i32, i32),
    icon: HICON,
    flags: DI_FLAGS,
) -> bool {
    // SAFETY: no pointers.
    unsafe { DrawIconEx(hdc, x, y, icon, cx, cy, 0, core::ptr::null_mut(), flags) != 0 }
}

pub fn destroy_icon(icon: HICON) {
    // SAFETY: no pointers.
    unsafe { DestroyIcon(icon) };
}
