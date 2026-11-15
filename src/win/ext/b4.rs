//! Batch 4 of the move onto `win`.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::UI::Controls::SetWindowTheme;

use crate::win::wz;

// ---- The registry ----

/// A registry hive goz-gui reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn key(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// The DWORD `value` of key `key` (a path under `hive`); `None` if it is
/// not there or is not a DWORD.
pub fn reg_dword(hive: Hive, key: &str, value: &str) -> Option<u32> {
    let key = wz(key);
    let value = wz(value);
    let mut data: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: a predefined key; NUL-terminated names read during the call;
    // a DWORD out (RRF_RT_REG_DWORD: nothing else is written) whose size is
    // passed.
    let rc = unsafe {
        RegGetValueW(
            hive.key(),
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    (rc == ERROR_SUCCESS).then_some(data)
}

/// The string `value` of key `key` (a path under `hive`), if it is there,
/// is a REG_SZ and fits in `cap` UTF-16 units with its NUL.
pub fn reg_string(hive: Hive, key: &str, value: &str, cap: usize) -> Option<String> {
    let key = wz(key);
    let value = wz(value);
    let mut buf = vec![0u16; cap];
    let mut size = u32::try_from(buf.len() * 2).unwrap_or(u32::MAX & !1);
    // SAFETY: a predefined key; NUL-terminated names read during the call;
    // the buffer, whose size in bytes is passed (at most its own), written
    // during the call.
    let rc = unsafe {
        RegGetValueW(
            hive.key(),
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            core::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    // `size` is the bytes written, its NUL included.
    buf.truncate((size as usize / 2).saturating_sub(1));
    Some(String::from_utf16_lossy(&buf))
}

/// This process's memory counters (K32GetProcessMemoryInfo).
pub fn process_memory()
-> Option<windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: an out-struct whose size is passed, for this process (the
    // pseudo-handle GetCurrentProcess gives, which needs no closing).
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
            c.cb,
        )
    };
    (ok != 0).then_some(c)
}

// ---- Visual styles and DWM ----

/// SetWindowTheme: the visual style class (`app`) and id list a control
/// draws with; `None` for both puts its default back.
pub fn set_window_theme(hwnd: HWND, app: Option<&str>, ids: Option<&str>) -> bool {
    let app = app.map(wz);
    let ids = ids.map(wz);
    let ptr = |s: &Option<Vec<u16>>| s.as_ref().map_or(core::ptr::null(), |v| v.as_ptr());
    // SAFETY: NUL-terminated strings or null, read during the call.
    unsafe { SetWindowTheme(hwnd, ptr(&app), ptr(&ids)) >= 0 }
}

/// DwmSetWindowAttribute of a 4-byte value (a BOOL, a COLORREF, an enum):
/// DWMWA_USE_IMMERSIVE_DARK_MODE (20) and the like. False if DWM refused
/// it (a window that is not there, an attribute that takes another size).
pub fn dwm_set_attribute_i32(hwnd: HWND, attribute: u32, value: i32) -> bool {
    // SAFETY: DWM reads `cbAttribute` bytes from the pointer, here the 4 of
    // `value`, during the call; no window attribute's value is a pointer.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&raw const value).cast(),
            size_of::<i32>() as u32,
        ) >= 0
    }
}

/// uxtheme's PreferredAppMode: how the app's popup menus (and the windows
/// that allow it) follow the system's dark mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    Default = 0,
    /// Follow the system's app mode.
    AllowDark = 1,
    ForceDark = 2,
    ForceLight = 3,
}

/// The dark-mode functions uxtheme has exported by ordinal only since
/// Windows 10 1809; each `None` on a Windows without it.
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

fn dark_mode() -> &'static DarkMode {
    static API: OnceLock<DarkMode> = OnceLock::new();
    API.get_or_init(|| {
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
            return DarkMode {
                allow_for_window: None,
                set_preferred_app_mode: None,
                flush_menu_themes: None,
            };
        }
        // An export by ordinal: the number travels as the name's address
        // (MAKEINTRESOURCEA), below 0x10000.
        // SAFETY: GetProcAddress of the module just loaded; an ordinal, not
        // a string.
        let ordinal = |n: usize| unsafe { GetProcAddress(dll, core::ptr::without_provenance(n)) };
        // SAFETY: each export is cast to the signature uxtheme has given it
        // at that ordinal since 1809 (named on the fields of `DarkMode`). A
        // C++ bool is one byte, taken and read here as a u8, so no value
        // it gives is an invalid Rust bool.
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

// ---- Message boxes, menus, icons, frames ----

/// Windows' own message box (MessageBoxW) over `owner`, modal; its answer
/// (IDOK, IDCANCEL...), 0 if it could not be shown.
pub fn message_box(
    owner: HWND,
    text: &str,
    title: &str,
    style: windows_sys::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) -> i32 {
    let text = wz(text);
    let title = wz(title);
    // SAFETY: NUL-terminated strings that outlive the modal call.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            owner,
            text.as_ptr(),
            title.as_ptr(),
            style,
        )
    }
}

/// The window's system menu (GetSystemMenu, not reverted); null if it has
/// none.
pub fn system_menu(hwnd: HWND) -> windows_sys::Win32::UI::WindowsAndMessaging::HMENU {
    // SAFETY: no pointers.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMenu(hwnd, 0) }
}

/// EnableMenuItem: enables, greys or disables item `item` of `menu` (by
/// command or position, as `flags` say); its state before, -1 if there is
/// no such item.
pub fn enable_menu_item(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    item: u32,
    flags: windows_sys::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS,
) -> i32 {
    // SAFETY: no pointers; user32 checks the menu's handle.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::EnableMenuItem(menu, item, flags) }
}

/// AdjustWindowRectExForDpi: the window rectangle whose client area is
/// `rc`, for these styles at `dpi`.
pub fn adjust_window_rect_for_dpi(
    rc: windows_sys::Win32::Foundation::RECT,
    style: u32,
    menu: bool,
    ex_style: u32,
    dpi: u32,
) -> windows_sys::Win32::Foundation::RECT {
    let mut r = rc;
    // SAFETY: an in-out struct.
    unsafe {
        windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut r,
            style,
            menu.into(),
            ex_style,
            dpi,
        )
    };
    r
}

/// An icon this code loaded, destroyed when it drops.
pub struct Icon(windows_sys::Win32::UI::WindowsAndMessaging::HICON);

impl Icon {
    /// One of the system's icons (IDI_ERROR, IDI_WARNING... as a number)
    /// at `cx` × `cy`, scaled down from a larger one where it has one
    /// (LoadIconWithScaleDown); `None` if it could not be loaded.
    pub fn system(id: usize, cx: i32, cy: i32) -> Option<Icon> {
        // A system icon's id travels as the pointer's address
        // (MAKEINTRESOURCE), below 0x10000; anything else would be read as
        // a name.
        if id >= 0x10000 {
            return None;
        }
        let mut icon = core::ptr::null_mut();
        // SAFETY: an id, not a string (above), of the system's icons; an
        // out-handle.
        let hr = unsafe {
            windows_sys::Win32::UI::Controls::LoadIconWithScaleDown(
                core::ptr::null_mut(),
                core::ptr::without_provenance(id),
                cx,
                cy,
                &mut icon,
            )
        };
        (hr >= 0 && !icon.is_null()).then_some(Icon(icon))
    }

    /// The icon's handle, to draw with (it stays this icon's).
    pub fn handle(&self) -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
        self.0
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: the icon this owns (LoadIconWithScaleDown's is the
        // caller's to destroy), destroyed once.
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(self.0) };
    }
}

/// Destroys a menu (and its submenus); false if it could not (user32
/// checks the menu's handle).
pub fn destroy_menu(menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU) -> bool {
    // SAFETY: no pointers.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyMenu(menu) != 0 }
}

/// Windows' colour picker (ChooseColorW), modal over `owner`, starting at
/// `initial` with CC_RGBINIT; its 16 custom colours are `custom`'s, kept
/// there. The colour chosen, `None` if the user cancelled. `flags` are
/// CC_* without a hook or a template (those are ignored).
pub fn choose_color(
    owner: HWND,
    initial: u32,
    custom: &mut [u32; 16],
    flags: windows_sys::Win32::UI::Controls::Dialogs::CHOOSECOLOR_FLAGS,
) -> Option<u32> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CC_ENABLEHOOK, CC_ENABLETEMPLATE, CC_ENABLETEMPLATEHANDLE, CHOOSECOLORW, ChooseColorW,
    };
    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: initial,
        lpCustColors: custom.as_mut_ptr(),
        Flags: flags & !(CC_ENABLEHOOK | CC_ENABLETEMPLATE | CC_ENABLETEMPLATEHANDLE),
        ..Default::default()
    };
    // SAFETY: a complete struct with its size; the custom colours are 16
    // COLORREFs, read and written during the modal call; no hook, no
    // template (their flags are masked off, their fields null).
    (unsafe { ChooseColorW(&mut cc) } != 0).then_some(cc.rgbResult)
}

/// Whether this module has registered a window class `name`
/// (GetClassInfoExW).
pub fn class_registered(name: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassInfoExW, WNDCLASSEXW};
    let z = wz(name);
    let mut wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        ..Default::default()
    };
    // SAFETY: a NUL-terminated name read during the call; an out-struct
    // with its size (what it is given back is not kept).
    unsafe { GetClassInfoExW(crate::win::class::module(), z.as_ptr(), &mut wc) != 0 }
}

/// SetPixelV: paints one pixel of `hdc` in colour `c`.
pub fn set_pixel(
    hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    x: i32,
    y: i32,
    c: windows_sys::Win32::Foundation::COLORREF,
) -> bool {
    // SAFETY: no pointers; GDI checks the DC.
    unsafe { windows_sys::Win32::Graphics::Gdi::SetPixelV(hdc, x, y, c) != 0 }
}
