//! Visual styles (uxtheme): a class's theme data, owned and closed when it
//! drops, and drawing its parts; the visual style a control draws with; and
//! the dark mode uxtheme has but does not document.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::HDC;
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::UI::Controls::{
    CloseThemeData, DrawThemeBackground, DrawThemeParentBackground, EnableThemeDialogTexture,
    GetThemePartSize, HTHEME, OpenThemeData, SetWindowTheme, THEMESIZE,
};
use windows_sys::Win32::UI::HiDpi::OpenThemeDataForDpi;

use super::wz;

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

    /// The theme data of `classes` for `hwnd` at the DPI the system picks
    /// for that window (OpenThemeData); `None` without visual styles.
    pub fn open_for_window(hwnd: HWND, classes: &str) -> Option<Theme> {
        let z = super::wz(classes);
        // SAFETY: a NUL-terminated class list read during the call.
        let t = unsafe { OpenThemeData(hwnd, z.as_ptr()) };
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

/// SetWindowTheme: the visual style class (`app`) and id list a control
/// draws with; `None` for both puts its default back.
pub fn set_window_theme(hwnd: HWND, app: Option<&str>, ids: Option<&str>) -> bool {
    let app = app.map(wz);
    let ids = ids.map(wz);
    let ptr = |s: &Option<Vec<u16>>| s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr());
    // SAFETY: NUL-terminated strings or null, read during the call.
    unsafe { SetWindowTheme(hwnd, ptr(&app), ptr(&ids)) >= 0 }
}

/// EnableThemeDialogTexture: whether a dialog draws the tab's face behind
/// its controls (ETDT_*).
pub fn enable_dialog_texture(hwnd: HWND, flags: u32) -> bool {
    // SAFETY: no pointers.
    unsafe { EnableThemeDialogTexture(hwnd, flags) >= 0 }
}

/// uxtheme's PreferredAppMode: how the app's popup menus (and the windows
/// that allow it) follow the system's dark mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    /// Follow the system's app mode.
    AllowDark = 1,
    /// Dark whatever the system's mode.
    ForceDark = 2,
    /// Light whatever the system's mode.
    ForceLight = 3,
}

/// The dark-mode functions uxtheme has exported by ordinal only since
/// Windows 10 1809 (build 17763); each `None` on a Windows without it,
/// and on any build before that one, where the ordinals may name other
/// functions.
struct DarkMode {
    /// Ordinal 133: `bool AllowDarkModeForWindow(HWND, bool)`.
    allow_for_window: Option<unsafe extern "system" fn(HWND, u8) -> u8>,
    /// Ordinal 135: `PreferredAppMode SetPreferredAppMode(PreferredAppMode)`
    /// (1903 on; 1809's `bool AllowDarkModeForApp(bool)` there takes and
    /// gives as well an integer in the same register).
    set_preferred_app_mode: Option<unsafe extern "system" fn(i32) -> i32>,
    /// Ordinal 136: `void FlushMenuThemes()`.
    flush_menu_themes: Option<unsafe extern "system" fn()>,
}

/// Windows 10 1809's build, the first whose uxtheme has the dark mode's
/// exports at these ordinals.
const FIRST_BUILD: u32 = 17763;

fn dark_mode() -> &'static DarkMode {
    static API: OnceLock<DarkMode> = OnceLock::new();
    API.get_or_init(|| {
        let none = DarkMode {
            allow_for_window: None,
            set_preferred_app_mode: None,
            flush_menu_themes: None,
        };
        if super::sys::windows_build() < FIRST_BUILD {
            return none;
        }
        let name = wz("uxtheme.dll");
        // SAFETY: loads a system DLL by a NUL-terminated name (kept loaded
        // for the process, as the pointers below are kept).
        let dll = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                core::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if dll.is_null() {
            return none;
        }
        // An export by ordinal: the number travels as the name's address
        // (MAKEINTRESOURCEA), below 0x10000.
        // SAFETY: GetProcAddress of the module just loaded; an ordinal, not
        // a string.
        let ordinal = |n: usize| unsafe { GetProcAddress(dll, core::ptr::without_provenance(n)) };
        // SAFETY: each export is cast to the signature uxtheme has given it
        // at that ordinal since 1809 (named on the fields of `DarkMode`),
        // this Windows being that build or later (checked above): an
        // assumption of the trust model (`win`'s documentation), as the
        // exports are undocumented. A C++ bool is one byte, taken and read
        // here as a u8, so no value it gives is an invalid Rust bool.
        unsafe {
            DarkMode {
                allow_for_window: ordinal(133).map(|f| core::mem::transmute(f)),
                set_preferred_app_mode: ordinal(135).map(|f| core::mem::transmute(f)),
                flush_menu_themes: ordinal(136).map(|f| core::mem::transmute(f)),
            }
        }
    })
}

/// Lets `hwnd`'s themed parts (scroll bars, a list's header) draw dark,
/// or not; nothing on a Windows without uxtheme's dark mode.
pub fn allow_dark_mode_for_window(hwnd: HWND, allow: bool) {
    if let Some(f) = dark_mode().allow_for_window {
        // SAFETY: the export's signature (see `DarkMode`); it records a
        // flag on the window, which the system checks the handle of.
        unsafe { f(hwnd, u8::from(allow)) };
    }
}

/// Sets the app's preferred mode (popup menus follow it once flushed);
/// nothing on a Windows without uxtheme's dark mode.
pub fn set_preferred_app_mode(mode: AppMode) {
    if let Some(f) = dark_mode().set_preferred_app_mode {
        // SAFETY: the export's signature (see `DarkMode`), given one of the
        // mode's values.
        unsafe { f(mode as i32) };
    }
}

/// Makes the popup menus opened from now on use the preferred mode;
/// nothing on a Windows without uxtheme's dark mode.
pub fn flush_menu_themes() {
    if let Some(f) = dark_mode().flush_menu_themes {
        // SAFETY: the export's signature (see `DarkMode`): no arguments.
        unsafe { f() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dark_mode_is_asked_for_only_where_it_is() {
        let build = crate::win::sys::windows_build();
        assert!(build > 0, "RtlGetVersion says the build");
        if build < FIRST_BUILD {
            assert!(dark_mode().allow_for_window.is_none());
        }
    }
}
