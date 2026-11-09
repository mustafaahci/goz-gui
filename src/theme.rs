//! Light and dark palettes, and the switches that make Win32 chrome follow
//! the system's app theme.
//!
//! The title bar uses the documented DWM attribute. Popup menus, scroll bars
//! and the list header use the dark mode uxtheme has carried since Windows 10
//! 1809 but only exports by ordinal; every call checks the export exists, so
//! a Windows without it just stays light.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::Controls::SetWindowTheme;

use crate::wide::wz;

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// Which palette the window uses: the system's app theme, or one forced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow Windows' "app mode" setting, live.
    UserDefault,
    /// Always light.
    Standard,
    Dark,
}

impl ThemeMode {
    pub fn is_dark(self) -> bool {
        match self {
            ThemeMode::UserDefault => system_dark(),
            ThemeMode::Standard => false,
            ThemeMode::Dark => true,
        }
    }
}

/// Colours sampled from Everything 1.5's dark theme and its light
/// "Standard" one, so goz-gui sits beside it without looking foreign.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// The list and the search field.
    pub bg: u32,
    /// Cells of the sorted column ("Current Sort").
    pub sort_bg: u32,
    pub fg: u32,
    pub dim: u32,
    /// Selected rows while the list has the focus, and while it has not.
    pub sel: u32,
    pub sel_inactive: u32,
    /// The row under the mouse, and its sorted cell.
    pub hover: u32,
    pub hover_sort: u32,
    /// Menu bar, search band and status bar.
    pub chrome: u32,
    /// The list header; its sorted item, which also gets an edge; items
    /// under the mouse; the item held down.
    pub header: u32,
    pub header_sort: u32,
    pub header_sort_edge: u32,
    pub header_hot: u32,
    pub header_sort_hot: u32,
    pub header_pressed: u32,
    /// Lines between header items, and the sort arrow.
    pub divider: u32,
    pub arrow: u32,
    /// The lines along the header and the top of the status bar.
    pub line: u32,
    /// The search frame and the status bar's separators.
    pub border: u32,
    /// A menu bar item under the mouse, where goz-gui draws the bar (dark).
    pub hot: u32,
    /// The search box's clear and history glyphs, and their hover state.
    pub glyph: u32,
    pub glyph_hot: u32,
    pub glyph_hot_bg: u32,
    /// The search history dropdown: its background, edge, and the entry
    /// under the mouse or chosen with the arrow keys.
    pub popup_bg: u32,
    pub popup_border: u32,
    pub popup_hot: u32,
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        if dark {
            Palette {
                dark,
                bg: rgb(0x00, 0x00, 0x00),
                sort_bg: rgb(0x18, 0x18, 0x18),
                fg: rgb(0xff, 0xff, 0xff),
                dim: rgb(0xa0, 0xa0, 0xa0),
                sel: rgb(0x72, 0x72, 0x72),
                sel_inactive: rgb(0x4a, 0x4a, 0x4a),
                hover: rgb(0x1e, 0x1e, 0x1e),
                hover_sort: rgb(0x2a, 0x2a, 0x2a),
                chrome: rgb(0x20, 0x20, 0x20),
                header: rgb(0x20, 0x20, 0x20),
                header_sort: rgb(0x39, 0x39, 0x39),
                header_sort_edge: rgb(0x52, 0x52, 0x52),
                header_hot: rgb(0x2d, 0x2d, 0x2d),
                header_sort_hot: rgb(0x45, 0x45, 0x45),
                header_pressed: rgb(0x52, 0x52, 0x52),
                divider: rgb(0x47, 0x47, 0x47),
                arrow: rgb(0xbc, 0xbc, 0xbc),
                line: rgb(0x30, 0x30, 0x30),
                border: rgb(0x5f, 0x5f, 0x5f),
                hot: rgb(0x3d, 0x3d, 0x3d),
                glyph: rgb(0xb4, 0xb4, 0xb4),
                glyph_hot: rgb(0xff, 0xff, 0xff),
                glyph_hot_bg: rgb(0x3a, 0x3a, 0x3a),
                // Sampled from Everything's dark history dropdown.
                popup_bg: rgb(0x2b, 0x2b, 0x2b),
                popup_border: rgb(0x80, 0x80, 0x80),
                popup_hot: rgb(0x45, 0x45, 0x45),
            }
        } else {
            Palette {
                dark,
                bg: rgb(0xff, 0xff, 0xff),
                sort_bg: rgb(0xf7, 0xf7, 0xf7),
                fg: rgb(0x00, 0x00, 0x00),
                dim: rgb(0x5f, 0x5f, 0x5f),
                sel: rgb(0xcc, 0xe8, 0xff),
                sel_inactive: rgb(0xd9, 0xd9, 0xd9),
                hover: rgb(0xe5, 0xf3, 0xff),
                hover_sort: rgb(0xdc, 0xed, 0xfc),
                chrome: rgb(0xf0, 0xf0, 0xf0),
                header: rgb(0xff, 0xff, 0xff),
                header_sort: rgb(0xe8, 0xe8, 0xe8),
                // Light Everything draws no edge round the sorted item.
                header_sort_edge: rgb(0xe8, 0xe8, 0xe8),
                header_hot: rgb(0xd9, 0xeb, 0xf9),
                header_sort_hot: rgb(0xd9, 0xeb, 0xf9),
                header_pressed: rgb(0xbc, 0xdc, 0xf4),
                divider: rgb(0xe5, 0xe5, 0xe5),
                arrow: rgb(0x70, 0x70, 0x70),
                line: rgb(0xe5, 0xe5, 0xe5),
                border: rgb(0xc8, 0xc8, 0xc8),
                hot: rgb(0xe5, 0xf3, 0xff),
                glyph: rgb(0x60, 0x60, 0x60),
                glyph_hot: rgb(0x60, 0x60, 0x60),
                glyph_hot_bg: rgb(0xe5, 0xe5, 0xe5),
                popup_bg: rgb(0xff, 0xff, 0xff),
                popup_border: rgb(0x80, 0x80, 0x80),
                popup_hot: rgb(0xcc, 0xe8, 0xff),
            }
        }
    }
}

/// Whether the user picked the dark app theme.
pub fn system_dark() -> bool {
    let key = wz(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wz("AppsUseLightTheme");
    let mut data: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into `data`, whose size is passed.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    rc == ERROR_SUCCESS && data == 0
}

struct Uxtheme {
    allow_for_window: Option<unsafe extern "system" fn(HWND, bool) -> bool>,
    set_preferred_app_mode: Option<unsafe extern "system" fn(i32) -> i32>,
    flush_menu_themes: Option<unsafe extern "system" fn()>,
}

fn uxtheme() -> &'static Uxtheme {
    static API: OnceLock<Uxtheme> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: loads a system DLL and looks up exports by ordinal. Each
        // pointer is cast to the signature uxtheme has had since 1809.
        unsafe {
            let dll = LoadLibraryExW(
                wz("uxtheme.dll").as_ptr(),
                core::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if dll.is_null() {
                return Uxtheme {
                    allow_for_window: None,
                    set_preferred_app_mode: None,
                    flush_menu_themes: None,
                };
            }
            let ordinal = |n: usize| GetProcAddress(dll, n as *const u8);
            Uxtheme {
                allow_for_window: ordinal(133).map(|f| core::mem::transmute(f)),
                set_preferred_app_mode: ordinal(135).map(|f| core::mem::transmute(f)),
                flush_menu_themes: ordinal(136).map(|f| core::mem::transmute(f)),
            }
        }
    })
}

/// Lets the app's popup menus go dark with the system. Call before any
/// window is created.
pub fn init() {
    set_app_mode(ThemeMode::UserDefault);
}

/// Makes popup menus follow `mode`: the system's setting, or forced light or
/// dark. Menus opened after [`flush_menus`] use it.
pub fn set_app_mode(mode: ThemeMode) {
    // uxtheme's PreferredAppMode: AllowDark follows the system.
    let value = match mode {
        ThemeMode::UserDefault => 1,
        ThemeMode::Dark => 2,
        ThemeMode::Standard => 3,
    };
    if let Some(f) = uxtheme().set_preferred_app_mode {
        // SAFETY: the export takes the mode as an int.
        unsafe { f(value) };
    }
}

/// Applies `dark` to a top-level window's title bar.
pub fn set_titlebar(hwnd: HWND, dark: bool) {
    allow(hwnd, dark);
    let value: i32 = dark.into();
    // SAFETY: DWMWA_USE_IMMERSIVE_DARK_MODE (20) takes a BOOL.
    unsafe { DwmSetWindowAttribute(hwnd, 20, (&raw const value).cast(), size_of::<i32>() as u32) };
}

/// Lets `hwnd`'s themed parts (scroll bars, header) draw dark.
pub fn allow(hwnd: HWND, dark: bool) {
    if let Some(f) = uxtheme().allow_for_window {
        // SAFETY: the export takes a window and a C++ bool.
        unsafe { f(hwnd, dark) };
    }
}

/// Sets a control's visual style class, or its default with `None`.
pub fn set_class(hwnd: HWND, class: Option<&str>) {
    let class = class.map(wz);
    // SAFETY: the string, if any, is NUL-terminated and outlives the call.
    unsafe {
        SetWindowTheme(
            hwnd,
            class.as_ref().map_or(core::ptr::null(), |c| c.as_ptr()),
            core::ptr::null(),
        )
    };
}

/// Makes menus opened from now on use the current theme.
pub fn flush_menus() {
    if let Some(f) = uxtheme().flush_menu_themes {
        // SAFETY: takes no arguments.
        unsafe { f() };
    }
}
