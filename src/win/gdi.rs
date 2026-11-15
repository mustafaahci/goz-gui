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
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, EnumFontFamiliesExW, GetPixel, GetTextColor, GetTextExtentExPointW, RoundRect,
    STRETCH_BLT_MODE, SetBrushOrgEx, SetDCPenColor, SetPixelV, SetStretchBltMode,
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
    /// The window's client-area DC (null: the screen's).
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

    /// The DC, to draw with (it stays this one's).
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
    /// A memory DC compatible with `hdc` (null: the screen).
    pub fn compatible(hdc: HDC) -> MemDc {
        MemDc(create_compatible_dc(hdc))
    }

    /// The DC, to draw with (it stays this one's).
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
    /// Takes `obj`, a GDI object made for the caller, to delete when this drops.
    pub fn new(obj: HGDIOBJ) -> Owned {
        Owned(obj)
    }

    /// The object, to draw with (it stays this one's).
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
    /// Selects `obj` into `hdc` until this drops.
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

/// The window's client-area DC (GetDC), to give back with [`release_dc`].
pub fn get_dc(hwnd: HWND) -> HDC {
    // SAFETY: no pointers.
    unsafe { GetDC(hwnd) }
}

/// The whole window's DC (GetWindowDC), to give back with [`release_dc`].
pub fn window_dc(hwnd: HWND) -> HDC {
    // SAFETY: no pointers.
    unsafe { GetWindowDC(hwnd) }
}

/// Gives back a DC [`get_dc`] or [`window_dc`] gave.
pub fn release_dc(hwnd: HWND, hdc: HDC) {
    // SAFETY: no pointers.
    unsafe { ReleaseDC(hwnd, hdc) };
}

/// A memory DC compatible with `hdc` (null: the screen), to delete with [`delete_dc`].
pub fn create_compatible_dc(hdc: HDC) -> HDC {
    // SAFETY: no pointers.
    unsafe { CreateCompatibleDC(hdc) }
}

/// Deletes a memory DC.
pub fn delete_dc(hdc: HDC) {
    // SAFETY: no pointers.
    unsafe { DeleteDC(hdc) };
}

/// A `cx` × `cy` bitmap of `hdc`'s format (null if it could not be made).
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

/// One of the stock objects (DC_BRUSH, DEFAULT_GUI_FONT…), never deleted.
pub fn stock_object(which: GET_STOCK_OBJECT_FLAGS) -> HGDIOBJ {
    // SAFETY: no pointers.
    unsafe { GetStockObject(which) }
}

/// A system colour (COLOR_*).
pub fn sys_color(index: SYS_COLOR_INDEX) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { GetSysColor(index) }
}

/// The system's brush of a colour (not to be deleted).
pub fn sys_color_brush(index: SYS_COLOR_INDEX) -> HBRUSH {
    // SAFETY: no pointers.
    unsafe { GetSysColorBrush(index) }
}

/// The DC's text colour; the one before.
pub fn set_text_color(hdc: HDC, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { SetTextColor(hdc, c) }
}

/// The DC's background colour; the one before.
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

/// DrawTextW of `text` in `rc` with DT_MODIFYSTRING added to `format`: the
/// text as drawn, with the ellipsis DT_END_ELLIPSIS or DT_PATH_ELLIPSIS
/// put in, written in a buffer of its own. With DT_CALCRECT nothing is
/// drawn and `rc` comes back as the text's extent.
pub fn draw_text_modified(
    hdc: HDC,
    text: &[u16],
    rc: &mut RECT,
    format: DRAW_TEXT_FORMAT,
) -> Vec<u16> {
    // DT_MODIFYSTRING may write up to four units more than the text, and a
    // NUL after them.
    let mut buf = text.to_vec();
    buf.extend_from_slice(&[0; 5]);
    let len = super::len_i32(text.len());
    // SAFETY: `len` units of the buffer, which has five more after them, as
    // DT_MODIFYSTRING needs; the rectangle is in-out.
    unsafe { DrawTextW(hdc, buf.as_mut_ptr(), len, rc, format | DT_MODIFYSTRING) };
    let end = super::until_nul(&buf).len();
    buf.truncate(end);
    buf
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

/// GetTextExtentExPointW of `text` in the DC's font: how many of its units
/// fit in `max` pixels, and the size of all of it.
pub fn text_fit(hdc: HDC, text: &[u16], max: i32) -> (i32, SIZE) {
    let (mut fit, mut size) = (0, SIZE::default());
    let len = super::len_i32(text.len());
    // SAFETY: `len` units of `text`, read; two out-values; no array of
    // partial extents.
    unsafe {
        GetTextExtentExPointW(
            hdc,
            text.as_ptr(),
            len,
            max,
            &mut fit,
            core::ptr::null_mut(),
            &mut size,
        )
    };
    (fit, size)
}

/// The DC's text colour.
pub fn text_color(hdc: HDC) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { GetTextColor(hdc) }
}

/// The colour of the DC's DC_PEN stock pen; the one before.
pub fn set_dc_pen_color(hdc: HDC, c: COLORREF) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { SetDCPenColor(hdc, c) }
}

/// The colour of the pixel at `x, y` of the DC (CLR_INVALID outside its
/// clip).
pub fn get_pixel(hdc: HDC, x: i32, y: i32) -> COLORREF {
    // SAFETY: no pointers.
    unsafe { GetPixel(hdc, x, y) }
}

/// SetPixelV: paints one pixel of `hdc` in colour `c`.
pub fn set_pixel(hdc: HDC, x: i32, y: i32, c: COLORREF) -> bool {
    // SAFETY: no pointers; GDI checks the DC.
    unsafe { SetPixelV(hdc, x, y, c) != 0 }
}

/// A rectangle with corners rounded by an ellipse of `cx` × `cy`, outlined
/// in the DC's pen and filled with its brush.
pub fn round_rect(hdc: HDC, rc: &RECT, cx: i32, cy: i32) {
    // SAFETY: no pointers.
    unsafe { RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, cx, cy) };
}

/// A new `cx` × `cy` bitmap of `planes` planes of `bits_per_pixel`, its
/// pixels not set (null if it could not be made). The caller deletes it.
pub fn create_bitmap(cx: i32, cy: i32, planes: u32, bits_per_pixel: u32) -> HBITMAP {
    // SAFETY: no initial pixels (null).
    unsafe { CreateBitmap(cx, cy, planes, bits_per_pixel, core::ptr::null()) }
}

/// The font faces the screen has, as EnumFontFamiliesExW gives them for a
/// LOGFONTW of `charset` and no face (DEFAULT_CHARSET: each face once per
/// charset it has), in its order.
pub fn font_faces(charset: u8) -> Vec<String> {
    use windows_sys::Win32::Foundation::LPARAM;

    /// Adds each face to the vector `l` points at.
    unsafe extern "system" fn each(
        lf: *const LOGFONTW,
        _tm: *const TEXTMETRICW,
        _kind: u32,
        l: LPARAM,
    ) -> i32 {
        // SAFETY: `l` is `font_faces`' vector, alive and borrowed by nothing
        // else for the enumeration; `lf` the face Windows enumerates, read
        // during the call (its name copied out).
        unsafe {
            let out = &mut *(l as *mut Vec<String>);
            let face = &(*lf).lfFaceName;
            out.push(String::from_utf16_lossy(super::until_nul(face)));
        }
        1
    }

    let mut out: Vec<String> = Vec::new();
    let dc = WindowDc::get(core::ptr::null_mut());
    let lf = LOGFONTW {
        lfCharSet: charset,
        ..Default::default()
    };
    // SAFETY: the screen's DC (released when `dc` drops); the callback
    // above with `out`, which outlives the call and is not touched during
    // it but through the callback.
    unsafe { EnumFontFamiliesExW(dc.hdc(), &lf, Some(each), (&raw mut out) as LPARAM, 0) };
    out
}

/// The DC's font's metrics.
pub fn text_metrics(hdc: HDC) -> TEXTMETRICW {
    let mut tm = TEXTMETRICW::default();
    // SAFETY: an out-struct.
    unsafe { GetTextMetricsW(hdc, &mut tm) };
    tm
}

/// Fills `rc` with `brush`.
pub fn fill_rect(hdc: HDC, rc: &RECT, brush: HBRUSH) {
    // SAFETY: an in-struct.
    unsafe { FillRect(hdc, rc, brush) };
}

/// Draws a one-pixel frame round `rc` with `brush`.
pub fn frame_rect(hdc: HDC, rc: &RECT, brush: HBRUSH) {
    // SAFETY: an in-struct.
    unsafe { FrameRect(hdc, rc, brush) };
}

/// Draws (or, again, takes away) the dotted focus rectangle.
pub fn draw_focus_rect(hdc: HDC, rc: &RECT) {
    // SAFETY: an in-struct.
    unsafe { DrawFocusRect(hdc, rc) };
}

/// A rectangle outlined in the DC's pen and filled with its brush.
pub fn rectangle(hdc: HDC, rc: &RECT) {
    // SAFETY: no pointers.
    unsafe { Rectangle(hdc, rc.left, rc.top, rc.right, rc.bottom) };
}

/// Paints `rc` with the DC's brush as `rop` says (PATCOPY…).
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

/// Copies `src`'s rectangle into `dst`'s, stretched to fit, as `rop` says.
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

/// Blends `src`'s rectangle onto `dst`'s (AlphaBlend) as `blend` says.
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

/// Moves the DC's pen position.
pub fn move_to(hdc: HDC, x: i32, y: i32) {
    // SAFETY: no out-point asked for.
    unsafe { MoveToEx(hdc, x, y, core::ptr::null_mut()) };
}

/// Draws a line from the pen position to `x, y` in the DC's pen.
pub fn line_to(hdc: HDC, x: i32, y: i32) {
    // SAFETY: no pointers.
    unsafe { LineTo(hdc, x, y) };
}

/// Clips drawing to `rc` (and what was clipped to before).
pub fn intersect_clip(hdc: HDC, rc: &RECT) {
    // SAFETY: no pointers.
    unsafe { IntersectClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom) };
}

/// Clips `rc` out of what is drawn.
pub fn exclude_clip(hdc: HDC, rc: &RECT) {
    // SAFETY: no pointers.
    unsafe { ExcludeClipRect(hdc, rc.left, rc.top, rc.right, rc.bottom) };
}

/// Saves the DC's state; what [`restore_dc`] takes.
pub fn save_dc(hdc: HDC) -> i32 {
    // SAFETY: no pointers.
    unsafe { SaveDC(hdc) }
}

/// Puts back the state [`save_dc`] saved.
pub fn restore_dc(hdc: HDC, saved: i32) {
    // SAFETY: no pointers.
    unsafe { RestoreDC(hdc, saved) };
}

/// A device capability of the DC (LOGPIXELSY…).
pub fn device_caps(hdc: HDC, index: i32) -> i32 {
    // SAFETY: no pointers.
    unsafe { GetDeviceCaps(hdc, index) }
}

/// A font as `lf` describes it, to delete (null if it could not be made).
pub fn create_font(lf: &LOGFONTW) -> HFONT {
    // SAFETY: an in-struct (the face name is read within its array).
    unsafe { CreateFontIndirectW(lf) }
}

/// A brush of colour `c`, to delete.
pub fn create_solid_brush(c: COLORREF) -> HBRUSH {
    // SAFETY: no pointers.
    unsafe { CreateSolidBrush(c) }
}

/// A pen of `style`, `width` and colour `c`, to delete.
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

/// How the DC stretches bitmaps (HALFTONE, COLORONCOLOR…); the mode
/// before.
pub fn set_stretch_blt_mode(hdc: HDC, mode: STRETCH_BLT_MODE) -> i32 {
    // SAFETY: no pointers.
    unsafe { SetStretchBltMode(hdc, mode) }
}

/// Moves the DC's brush origin (the one before is not asked for).
pub fn set_brush_org(hdc: HDC, x: i32, y: i32) {
    // SAFETY: no out-point asked for.
    unsafe { SetBrushOrgEx(hdc, x, y, core::ptr::null_mut()) };
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

/// Destroys an icon made for the caller.
pub fn destroy_icon(icon: HICON) {
    // SAFETY: no pointers.
    unsafe { DestroyIcon(icon) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_path_is_shortened_in_its_own_buffer() {
        use windows_sys::Win32::Graphics::Gdi::{
            DT_CALCRECT, DT_NOPREFIX, DT_PATH_ELLIPSIS, DT_SINGLELINE,
        };
        let dc = MemDc::compatible(core::ptr::null_mut());
        let path: Vec<u16> = r"C:\Users\someone\Documents\Projects\goz\src\window.rs"
            .encode_utf16()
            .collect();
        let (all, size) = text_fit(dc.hdc(), &path, i32::MAX);
        assert_eq!(all, path.len() as i32);
        assert_eq!(size.cx, text_extent(dc.hdc(), &path).cx);
        let mut rc = RECT {
            right: size.cx / 2,
            bottom: 1,
            ..Default::default()
        };
        let format = DT_SINGLELINE | DT_NOPREFIX | DT_PATH_ELLIPSIS | DT_CALCRECT;
        let short = draw_text_modified(dc.hdc(), &path, &mut rc, format);
        assert!(short.len() < path.len());
        let dots = [0x2e; 3];
        assert!(
            short.windows(3).any(|w| w == dots),
            "an ellipsis in the middle"
        );
        assert!(short.ends_with(&path[path.len() - 9..]), "the name kept");
    }

    #[test]
    fn the_screen_has_fonts() {
        assert!(!font_faces(1).is_empty());
    }
}
