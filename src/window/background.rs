//! Running in the background: the tray icon and its menu, closing or
//! minimizing to the tray, the global hotkeys, and command lines handed over
//! by a second start (`crate::instance`).

use windows_sys::Win32::Foundation::{POINT, WPARAM};
use windows_sys::Win32::UI::Controls::{HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_SHIFT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use windows_sys::Win32::UI::Shell::NIN_SELECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONWARNING, MB_OK, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_NULL,
};

use super::{App, Options, WM_TRAY};
use crate::cmdline::{self, Launch};
use crate::settings::Background;
use crate::win::{self, Msg};
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
        // A window File > New Window started leaves the tray icon and the
        // hotkeys to the first window.
        if self.is_new_window() {
            return Vec::new();
        }
        if b.tray_icon() {
            if !self.tray_on.get() {
                let icon = self.window_icons.borrow()[0]
                    .as_ref()
                    .map_or(core::ptr::null_mut(), win::icon::Icon::handle);
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
            // This window's own hotkey ids; unregistering one not
            // registered is harmless.
            win::unregister_hotkey(self.hwnd, id);
            if let Some((mods, vk)) = hotkey_parts(word)
                && !win::register_hotkey(self.hwnd, id, mods, vk)
            {
                taken.push(name);
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
    pub(super) fn shown(&self) -> bool {
        win::is_visible(self.hwnd) && !win::is_iconic(self.hwnd)
    }

    /// The window closes but goz-gui keeps running (run in background), or
    /// it minimizes to the tray: everything is saved as at exit, then the
    /// window hides.
    pub(super) fn hide_to_background(&self) {
        self.save_and_flush();
        crate::dropdown::close(self);
        win::show(self.hwnd, SW_HIDE);
    }

    /// Brings the window up from the tray, a hotkey or a second start, with
    /// the keyboard in the search box and its text selected.
    pub(super) fn show_window(&self) {
        // Shows and activates this thread's window; Windows lets it come
        // forward from a hotkey, a tray click, or a process that allowed it.
        if win::is_iconic(self.hwnd) {
            win::show(self.hwnd, SW_RESTORE);
        } else {
            win::show(self.hwnd, SW_SHOW);
        }
        win::set_foreground(self.hwnd);
        self.command(menu::FOCUS_SEARCH);
    }

    /// Shows the window, or puts it away when it is in front already: into
    /// the tray if there is one or goz-gui runs in the background, else
    /// minimized.
    pub(super) fn toggle_window(&self) {
        let in_front = self.shown() && win::foreground() == self.hwnd;
        if !in_front {
            self.show_window();
            return;
        }
        let b = self.background.get();
        if b.run_in_background || b.tray_icon() {
            self.hide_to_background();
        } else {
            win::show(self.hwnd, SW_MINIMIZE);
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
    pub(super) fn on_tray(&self, m: &Msg) {
        let (w, event) = (m.w, (m.l as u32) & 0xffff);
        match event {
            WM_CONTEXTMENU => {
                let pt = POINT {
                    x: (w & 0xffff) as i16 as i32,
                    y: ((w >> 16) & 0xffff) as i16 as i32,
                };
                self.tray_menu(pt);
            }
            NIN_SELECT | NIN_KEYSELECT if self.background.get().single_click_tray => {
                self.tray_open(true)
            }
            WM_LBUTTONDBLCLK if !self.background.get().single_click_tray => self.tray_open(false),
            _ => {}
        }
    }

    /// The tray icon was clicked to open goz-gui (`toggle`: a single click,
    /// which puts a window in front away again): another window with
    /// Options > UI's "Create a new window from the system tray", else
    /// this one.
    fn tray_open(&self, toggle: bool) {
        if self.ui().new_window_from_tray {
            self.open_new_window(true);
        } else if toggle {
            self.toggle_window();
        } else {
            self.show_window();
        }
    }

    fn tray_menu(&self, pt: POINT) {
        // A popup menu this function makes, shows and destroys; the window
        // comes forward first so the menu closes when the user clicks
        // elsewhere (and WM_NULL after, the documented pairing).
        let cmd = {
            use win::menu::{By, NewItem};
            let m = win::menu::create_popup();
            let command = |id: u16, text| NewItem::command(u32::from(id), text);
            win::menu::append(m, &command(menu::TRAY_SHOW, "&Show Window"));
            win::menu::append(m, &NewItem::separator());
            win::menu::append(m, &command(menu::OPTIONS, "&Options..."));
            win::menu::append(m, &NewItem::separator());
            win::menu::append(m, &command(menu::EXIT, "E&xit"));
            win::menu::set_default(m, By::Command(u32::from(menu::TRAY_SHOW)));
            win::set_foreground(self.hwnd);
            let cmd = win::menu::track(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt, self.hwnd);
            win::post_plain(self.hwnd, WM_NULL, 0, 0);
            win::menu::destroy(m);
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
            // Every window of this goz-gui (File > New Window's) closes.
            self.exit_other_windows();
            // Closes this thread's window for good.
            win::destroy(self.hwnd);
            return;
        }
        // W4 (G1): -filelist (or a bare .efu) opens a file list.
        if let Some(list) = &l.filelist {
            self.open_file_list(list);
        }
        if l.changes_search() {
            self.apply_launch(&l);
        }
        // F1 (preview pane): -preview / -no-preview.
        if let Some(show) = l.preview {
            self.show_pane(crate::dock::PaneId::Preview, show);
        }
        if l.places_window() {
            self.place_window(&l);
        }
        // W6: -edit and -filelist-editor: the File List Editor; alone,
        // the window stays as it is.
        if l.edit.is_some() || l.filelist_editor {
            self.open_list_editor(l.edit.as_deref());
            if l.editor_only() && !l.startup {
                return;
            }
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
            .unwrap_or_else(|| crate::win::text(self.edit));
        if l.path.is_some() {
            // A folder of its own (Explorer's "Search goz..."): the Folders
            // pane's goes, or the search would be inside both (F2).
            self.navigate_to(&text, o, None);
        } else {
            self.navigate(&text, o);
        }
    }
}
