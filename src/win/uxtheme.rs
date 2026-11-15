//! Visual styles (uxtheme): a class's theme data, owned and closed when it
//! drops, and drawing its parts.

use windows_sys::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{DRAW_TEXT_FORMAT, DT_MODIFYSTRING, HDC};
use windows_sys::Win32::UI::Controls::{
    CloseThemeData, DrawThemeBackground, DrawThemeParentBackground, DrawThemeText, GetThemeColor,
    GetThemePartSize, HTHEME, THEMESIZE,
};
use windows_sys::Win32::UI::HiDpi::OpenThemeDataForDpi;

/// A window class's theme data (OpenThemeDataForDpi), closed when this
/// drops. Only made here, so the handle is always one uxtheme gave.
pub struct Theme(HTHEME);

impl Theme {
    /// The theme data of `classes` ("Button", "ListView;TreeView") for
    /// `hwnd` at `dpi`; `None` without visual styles.
    pub fn open(hwnd: HWND, classes: &str, dpi: u32) -> Option<Theme> {
        let z = super::wz(classes);
        // SAFETY: a NUL-terminated class list read during the call.
        let t = unsafe { OpenThemeDataForDpi(hwnd, z.as_ptr(), dpi) };
        (t != 0).then_some(Theme(t))
    }

    /// The size of a part in a state (TS_MIN, TS_TRUE, TS_DRAW).
    pub fn part_size(&self, hdc: HDC, part: i32, state: i32, size: THEMESIZE) -> Option<SIZE> {
        let mut s = SIZE::default();
        // SAFETY: theme data this owns; no bounding rectangle; an out-struct.
        let hr =
            unsafe { GetThemePartSize(self.0, hdc, part, state, core::ptr::null(), size, &mut s) };
        (hr >= 0).then_some(s)
    }

    /// Draws a part's background in `rc` (clipped to `clip`).
    pub fn draw_background(&self, hdc: HDC, part: i32, state: i32, rc: &RECT, clip: Option<&RECT>) {
        let clip = clip.map_or(core::ptr::null(), |c| c as *const RECT);
        // SAFETY: theme data this owns; rectangles read during the call.
        unsafe { DrawThemeBackground(self.0, hdc, part, state, rc, clip) };
    }

    /// Draws `text` as a part's text in `rc`.
    pub fn draw_text(
        &self,
        hdc: HDC,
        (part, state): (i32, i32),
        text: &[u16],
        format: DRAW_TEXT_FORMAT,
        rc: &RECT,
    ) {
        let len = i32::try_from(text.len()).unwrap_or(i32::MAX);
        // SAFETY: theme data this owns; `len` units of `text`, read only
        // (DT_MODIFYSTRING masked off).
        unsafe {
            DrawThemeText(
                self.0,
                hdc,
                part,
                state,
                text.as_ptr(),
                len,
                format & !DT_MODIFYSTRING,
                0,
                rc,
            )
        };
    }

    /// A colour property (TMT_*) of a part in a state.
    pub fn color(&self, part: i32, state: i32, prop: i32) -> Option<COLORREF> {
        let mut c = 0;
        // SAFETY: theme data this owns; an out-value.
        let hr = unsafe { GetThemeColor(self.0, part, state, prop, &mut c) };
        (hr >= 0).then_some(c)
    }
}

impl Drop for Theme {
    fn drop(&mut self) {
        // SAFETY: theme data this owns, closed once.
        unsafe { CloseThemeData(self.0) };
    }
}

/// Draws what `hwnd`'s parent shows behind `rc` of it (a see-through
/// control's background).
pub fn draw_parent_background(hwnd: HWND, hdc: HDC, rc: &RECT) {
    // SAFETY: a rectangle read during the call.
    unsafe { DrawThemeParentBackground(hwnd, hdc, rc) };
}
