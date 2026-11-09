//! Running in the background: the tray icon and its menu, closing or
//! minimizing to the tray, the global hotkeys, and command lines handed over
//! by a second start (`crate::instance`).

use windows_sys::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows_sys::Win32::UI::Controls::{HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_SHIFT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
};
use windows_sys::Win32::UI::Shell::NIN_SELECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, DestroyWindow, GetForegroundWindow, IsIconic,
    IsWindowVisible, MB_ICONWARNING, MB_OK, MF_SEPARATOR, MF_STRING, PostMessageW, SW_HIDE,
    SW_MINIMIZE, SW_RESTORE, SW_SHOW, SetForegroundWindow, SetMenuDefaultItem, ShowWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_NULL,
};

use super::{App, Options, WM_TRAY};
use crate::cmdline::{self, Launch};
use crate::settings::Background;
use crate::wide::wz;
use crate::{menu, msgbox, tray};

/// The tray icon chosen with the keyboard (NIN_SELECT | NINF_KEY).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

/// The global hotkeys' ids.
const HOTKEY_TOGGLE: i32 = 1;
const HOTKEY_SHOW: i32 = 2;

/// A hotkey control's word (virtual key, `HOTKEYF_*` above it) as
/// `RegisterHotKey` takes it; `None` for no key.
pub fn hotkey_parts(word: u16) -> Option<(u32, u32)> {
    let vk = u32::from(word & 0xff);
    if vk == 0 {
        return None;
    }
    let f = (word >> 8) as u8 as u32;
    let mut mods = MOD_NOREPEAT;
    if f & HOTKEYF_SHIFT != 0 {
        mods |= MOD_SHIFT;
    }
    if f & HOTKEYF_CONTROL != 0 {
        mods |= MOD_CONTROL;
    }
    if f & HOTKEYF_ALT != 0 {
        mods |= MOD_ALT;
    }
    Some((mods, vk))
}

impl App {
    pub(super) fn background(&self) -> Background {
        self.background.get()
    }

    /// Puts the tray icon and the hotkeys as `b` says. Returns the hotkeys
    /// that another program holds already, by name, for the user.
    pub(super) fn apply_background(&self, b: Background) -> Vec<&'static str> {
        self.background.set(b);
        if b.tray_icon() {
            if !self.tray_on.get() {
                let icon = self.window_icons.get()[0];
                self.tray_on.set(tray::add(self.hwnd, WM_TRAY, icon, "goz"));
            }
        } else if self.tray_on.replace(false) {
            tray::remove(self.hwnd);
        }
        let mut taken = Vec::new();
        for (id, word, name) in [
            (HOTKEY_TOGGLE, b.toggle_window_key, "Toggle window"),
            (HOTKEY_SHOW, b.show_window_key, "Show window"),
        ] {
            // SAFETY: this window's own hotkey ids; unregistering one not
            // registered is harmless.
            unsafe { UnregisterHotKey(self.hwnd, id) };
            if let Some((mods, vk)) = hotkey_parts(word) {
                // SAFETY: as above.
                if unsafe { RegisterHotKey(self.hwnd, id, mods, vk) } == 0 {
                    taken.push(name);
                }
            }
        }
        taken
    }

    /// Explorer restarted: the tray icon is gone, add it again.
    pub(super) fn taskbar_restarted(&self) {
        if self.tray_on.replace(false) {
            self.apply_background(self.background.get());
        }
    }

    pub(super) fn remove_tray(&self) {
        if self.tray_on.replace(false) {
            tray::remove(self.hwnd);
        }
    }

    /// Whether the window is on screen and not minimized.
    fn shown(&self) -> bool {
        // SAFETY: reads this thread's window's state.
        unsafe { IsWindowVisible(self.hwnd) != 0 && IsIconic(self.hwnd) == 0 }
    }

    /// The window closes but goz-gui keeps running (run in background), or
    /// it minimizes to the tray: everything is saved as at exit, then the
    /// window hides.
    pub(super) fn hide_to_background(&self) {
        self.save_and_flush();
        crate::dropdown::close(self);
        // SAFETY: hides this thread's window.
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    /// Brings the window up from the tray, a hotkey or a second start, with
    /// the keyboard in the search box and its text selected.
    pub(super) fn show_window(&self) {
        // SAFETY: shows and activates this thread's window; Windows lets
        // it come forward from a hotkey, a tray click, or a process that
        // allowed it.
        unsafe {
            if IsIconic(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_RESTORE);
            } else {
                ShowWindow(self.hwnd, SW_SHOW);
            }
            SetForegroundWindow(self.hwnd);
        }
        self.command(menu::FOCUS_SEARCH);
    }

    /// Shows the window, or puts it away when it is in front already: into
    /// the tray if there is one or goz-gui runs in the background, else
    /// minimized.
    pub(super) fn toggle_window(&self) {
        // SAFETY: reads the foreground window.
        let in_front = self.shown() && unsafe { GetForegroundWindow() } == self.hwnd;
        if !in_front {
            self.show_window();
            return;
        }
        let b = self.background.get();
        if b.run_in_background || b.tray_icon() {
            self.hide_to_background();
        } else {
            // SAFETY: minimizes this thread's window.
            unsafe { ShowWindow(self.hwnd, SW_MINIMIZE) };
        }
    }

    pub(super) fn on_hotkey(&self, id: WPARAM) {
        match id as i32 {
            HOTKEY_TOGGLE => self.toggle_window(),
            HOTKEY_SHOW => self.show_window(),
            _ => {}
        }
    }

    /// A tray icon event (`NOTIFYICON_VERSION_4`: the event in lParam's low
    /// word, the anchor point in wParam).
    pub(super) fn on_tray(&self, w: WPARAM, l: LPARAM) {
        let event = (l as u32) & 0xffff;
        match event {
            WM_CONTEXTMENU => {
                let pt = POINT {
                    x: (w & 0xffff) as i16 as i32,
                    y: ((w >> 16) & 0xffff) as i16 as i32,
                };
                self.tray_menu(pt);
            }
            NIN_SELECT | NIN_KEYSELECT if self.background.get().single_click_tray => {
                self.toggle_window()
            }
            WM_LBUTTONDBLCLK if !self.background.get().single_click_tray => self.show_window(),
            _ => {}
        }
    }

    fn tray_menu(&self, pt: POINT) {
        // SAFETY: a popup menu this function makes, shows and destroys; the
        // window comes forward first so the menu closes when the user
        // clicks elsewhere (and WM_NULL after, the documented pairing).
        let cmd = unsafe {
            let m = CreatePopupMenu();
            AppendMenuW(
                m,
                MF_STRING,
                usize::from(menu::TRAY_SHOW),
                wz("&Show Window").as_ptr(),
            );
            AppendMenuW(m, MF_SEPARATOR, 0, core::ptr::null());
            AppendMenuW(
                m,
                MF_STRING,
                usize::from(menu::OPTIONS),
                wz("&Options...").as_ptr(),
            );
            AppendMenuW(m, MF_SEPARATOR, 0, core::ptr::null());
            AppendMenuW(m, MF_STRING, usize::from(menu::EXIT), wz("E&xit").as_ptr());
            SetMenuDefaultItem(m, u32::from(menu::TRAY_SHOW), 0);
            SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenuEx(
                m,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                self.hwnd,
                core::ptr::null(),
            );
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            DestroyMenu(m);
            cmd
        };
        match cmd as u16 {
            0 => {}
            menu::TRAY_SHOW => self.show_window(),
            menu::OPTIONS => {
                self.show_window();
                self.command(menu::OPTIONS);
            }
            id => self.command(id),
        }
    }

    /// A command line handed over by a second start of goz-gui.
    pub(super) fn launch(&self, args: Vec<String>) {
        let l = match cmdline::parse(&args) {
            Ok(l) => l,
            Err(why) => {
                msgbox::show(self.hwnd, &why, "goz-gui", MB_OK | MB_ICONWARNING);
                return;
            }
        };
        if l.exit {
            // SAFETY: closes this thread's window for good.
            unsafe { DestroyWindow(self.hwnd) };
            return;
        }
        if l.changes_search() {
            self.apply_launch(&l);
        }
        if l.toggle_window {
            self.toggle_window();
        } else if !l.startup {
            self.show_window();
        }
    }

    /// The search a command line asks for, as a new navigation entry.
    pub(super) fn apply_launch(&self, l: &Launch) {
        let mut o: Options = self.opts.get();
        if let Some(f) = l.filter {
            o.filter = f;
        }
        if let Some(c) = l.match_case {
            o.match_case = c;
        }
        if let Some(p) = l.match_path {
            o.match_path = p;
        }
        if let Some(key) = l.sort {
            o.sort = goz_core::types::SortSpec::default_for(key);
        }
        if let Some(dir) = l.dir {
            o.sort.dir = dir;
        }
        let text = l
            .search_text()
            .unwrap_or_else(|| crate::wide::window_text(self.edit));
        self.navigate(&text, o);
    }
}
