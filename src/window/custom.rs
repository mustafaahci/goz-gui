//! Wave 4 (G2), customization, from the window's side: the fonts and
//! colours of Options > Fonts and Colors put in place, the keys of the
//! Keyboard page put in force (and the menus' shortcut text following
//! them), the UI scale, the color filters, the double-Ctrl action, and the
//! taskbar's jump list.
//!
//! G2's main-window messages are `WM_APP + 300..320` and its timers
//! `300..310` (wave 4's G1 has `WM_APP + 250..300` and `250..300`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{HFONT, InvalidateRect};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMenuItemCount, GetMenuItemInfoW, GetWindowRect, HMENU, MENUITEMINFOW, MFT_SEPARATOR,
    MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU, SetMenuItemInfoW, SetTimer, WM_APP,
};

use super::{App, Fonts};
use crate::a11y::Server;
use crate::colorfilter::{ColorFilter, Matcher};
use crate::colors::{Colors, FontSpec};
use crate::custom::DoubleCtrl;
use crate::keymap::{self, Bindings};
use crate::theme::Palette;

/// Posted by the double-Ctrl hook (`crate::dblctrl`): Ctrl was pressed
/// twice.
pub(super) const WM_DOUBLE_CTRL: u32 = WM_APP + 300;
const MSG_END: u32 = WM_APP + 320;

/// The jump list is made again a while after the searches or bookmarks
/// changed (and once after the start), so typing never waits for it.
pub(super) const TIMER_JUMP_LIST: usize = 300;
const TIMER_END: usize = 310;
const JUMP_LIST_DELAY_MS: u32 = 5000;

pub(super) fn is_message(msg: u32) -> bool {
    (WM_DOUBLE_CTRL..MSG_END).contains(&msg)
}

pub(super) fn is_timer(id: usize) -> bool {
    (TIMER_JUMP_LIST..TIMER_END).contains(&id)
}

/// G2's state in the window.
pub(super) struct Custom {
    /// The fonts of the parts Fonts and Colors gives a font of their own
    /// (`fonts.rs`).
    pub(super) part_fonts: RefCell<Vec<(crate::colors::Item, Fonts)>>,
    /// The color filters, ready to match rows as they are drawn.
    filters: RefCell<Matcher>,
    /// The monitor's DPI as Windows gives it: the window's `dpi` is it
    /// times the UI scale.
    system_dpi: Cell<u32>,
    /// The double-Ctrl hook is in.
    hooked: Cell<bool>,
    /// The menu items whose shortcut text Options > Keyboard's keys
    /// rewrote, with what they said before (see [`relabel`]).
    menu_texts: RefCell<Originals>,
    /// The window's accessible object (`crate::a11y`), made when first
    /// asked for.
    pub(super) accessible: RefCell<Option<Server>>,
    /// The status bar's texts and where they were drawn last.
    status_parts: RefCell<Vec<(String, RECT)>>,
}

impl Custom {
    pub(super) fn set_status_parts(&self, parts: Vec<(String, RECT)>) {
        *self.status_parts.borrow_mut() = parts;
    }

    pub(super) fn status_parts(&self) -> Vec<(String, RECT)> {
        self.status_parts.borrow().clone()
    }

    pub(super) fn new(c: &crate::custom::Custom, system_dpi: u32) -> Custom {
        Custom {
            part_fonts: RefCell::new(Vec::new()),
            filters: RefCell::new(Matcher::new(&c.color_filters)),
            system_dpi: Cell::new(system_dpi),
            hooked: Cell::new(false),
            menu_texts: RefCell::new(HashMap::new()),
            accessible: RefCell::new(None),
            status_parts: RefCell::new(Vec::new()),
        }
    }
}

impl Fonts {
    /// [`Fonts::new`] with a face and size of its own (`spec`; empty face
    /// and 0 size are the message font's).
    pub fn with(dpi: u32, zoom: u32, spec: &FontSpec) -> Fonts {
        Fonts::make(dpi, zoom, |lf| {
            if !spec.face.is_empty() {
                let face: Vec<u16> = spec.face.encode_utf16().take(31).collect();
                lf.lfFaceName = [0; 32];
                lf.lfFaceName[..face.len()].copy_from_slice(&face);
                lf.lfCharSet = 1; // DEFAULT_CHARSET
            }
            if spec.size > 0 {
                // Points at the DPI; Zoom scales it after.
                lf.lfHeight = -(((spec.size * dpi + 36) / 72).max(1) as i32);
            }
        })
    }
}

impl App {
    /// G2's settings as they are now.
    pub(super) fn custom_settings(&self) -> crate::custom::Custom {
        self.prefs.borrow().base.custom.clone()
    }

    /// The palette of the theme with Fonts and Colors' colours.
    pub(super) fn custom_palette(&self, dark: bool) -> Palette {
        self.prefs
            .borrow()
            .base
            .custom
            .colors
            .apply(Palette::new(dark), &crate::theme::system_accent)
    }

    /// The search box's font: Fonts and Colors' Search Edit, bold when its
    /// style says so.
    pub(super) fn search_font(&self) -> HFONT {
        let bold = self.pal.get().is_bold(
            crate::colors::Item::SearchEdit,
            crate::colors::State::Normal,
        );
        self.part_font(crate::colors::Item::SearchEdit, bold)
    }

    /// The search box's text height.
    pub(super) fn search_font_height(&self) -> i32 {
        self.part_height(crate::colors::Item::SearchEdit)
    }

    /// Options > Fonts and Colors applied: the colours in place, and with
    /// another font for any part, every part in its font, measured and
    /// laid out again (`fonts.rs`).
    pub(super) fn set_colors(&self, colors: Colors) {
        let fonts_changed = {
            let mut prefs = self.prefs.borrow_mut();
            let old = &prefs.base.custom.colors;
            if *old == colors {
                return;
            }
            let changed = old.fonts_differ(&colors);
            prefs.base.custom.colors = colors;
            changed
        };
        if fonts_changed {
            self.refont();
        }
        self.apply_theme();
        self.settings_changed();
    }

    /// The color filters' colour for a row's text, if one matches.
    pub fn color_filter_colour(&self, row: &crate::client::Row) -> Option<u32> {
        let m = self.custom.filters.borrow();
        if m.is_empty() {
            return None;
        }
        m.color(
            &row.path,
            row.name_start as usize,
            row.is_dir,
            self.pal.get().dark,
        )
    }

    /// Options > Fonts and Colors' Color Filters...: the editor, and what it
    /// gives in place at once.
    pub(super) fn edit_color_filters(&self, owner: windows_sys::Win32::Foundation::HWND) {
        let rules = self.prefs.borrow().base.custom.color_filters.clone();
        if let Some(new) = crate::colorfilter_dialog::edit(owner, rules, self.dialog_look()) {
            self.set_color_filters(new);
        }
    }

    /// Puts new color filters in place (Options > Advanced's editor).
    pub(super) fn set_color_filters(&self, rules: Vec<ColorFilter>) {
        *self.custom.filters.borrow_mut() = Matcher::new(&rules);
        {
            let mut prefs = self.prefs.borrow_mut();
            prefs.base.custom.color_filters = rules;
            prefs.base.custom.color_filters_raw = None;
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
        self.settings_changed();
    }

    /// Options > Keyboard applied: the keys in force for every thread, the
    /// accelerator table made again as the next key comes.
    pub(super) fn set_bindings(&self, keys: Bindings) {
        keymap::install(&keys);
        self.prefs.borrow_mut().base.custom.keys = keys;
        self.settings_changed();
    }

    /// The UI scale (`dpi`) changed: the window is laid out at the scaled
    /// DPI now; the dialogs take it as they open.
    pub(super) fn set_ui_scale(&self, setting: u32) {
        crate::custom::set_ui_dpi(setting);
        self.prefs.borrow_mut().base.custom.dpi = setting;
        let dpi = crate::custom::scaled_dpi(self.custom.system_dpi.get(), setting);
        if dpi != self.dpi.get() {
            let mut rc = RECT::default();
            // SAFETY: reads this thread's window's place.
            unsafe { GetWindowRect(self.hwnd, &mut rc) };
            self.on_dpi(dpi, &rc);
        }
        self.settings_changed();
    }

    /// The monitor's DPI as Windows gives it (without the UI scale).
    pub(super) fn system_dpi(&self) -> u32 {
        self.custom.system_dpi.get()
    }

    /// WM_DPICHANGED: the monitor's DPI, scaled by the UI scale.
    pub(super) fn ui_dpi_for(&self, system: u32) -> u32 {
        self.custom.system_dpi.set(system);
        crate::custom::scaled(system)
    }

    /// The double-Ctrl action changed: the keyboard hook is in only while
    /// one is chosen, and only in the first window (a New Window's leaves
    /// the hotkeys to it).
    pub(super) fn set_double_ctrl(&self, action: DoubleCtrl) {
        self.prefs.borrow_mut().base.custom.double_ctrl = action;
        self.hook_double_ctrl();
        self.settings_changed();
    }

    fn hook_double_ctrl(&self) {
        let want = self.prefs.borrow().base.custom.double_ctrl != DoubleCtrl::None
            && !self.is_new_window();
        if want && !self.custom.hooked.get() {
            self.custom
                .hooked
                .set(crate::dblctrl::install(self.hwnd, WM_DOUBLE_CTRL));
        } else if !want && self.custom.hooked.replace(false) {
            crate::dblctrl::uninstall();
        }
    }

    /// Ctrl was pressed twice: Everything's `double_ctrl_action`.
    fn double_ctrl(&self) {
        match self.prefs.borrow().base.custom.double_ctrl {
            DoubleCtrl::None => {}
            DoubleCtrl::Show => self.show_window(),
            DoubleCtrl::Toggle => self.toggle_window(),
            DoubleCtrl::NewWindow => {
                self.command(crate::tabs::NEW_WINDOW);
            }
        }
    }

    /// The window is up: the keys in force, the double-Ctrl hook, the
    /// taskbar's relaunch command and, a little later, the jump list.
    pub(super) fn start_custom(&self) {
        keymap::install(&self.prefs.borrow().base.custom.keys);
        self.hook_double_ctrl();
        if !self.is_new_window() {
            crate::jumplist::set_relaunch(self.hwnd, self.instance.borrow().as_deref());
            self.jump_list_later();
        }
    }

    /// The window is ending.
    pub(super) fn exit_custom(&self) {
        if self.custom.hooked.replace(false) {
            crate::dblctrl::uninstall();
        }
        // Tools holding the window's object find no window behind it now.
        self.custom.accessible.borrow_mut().take();
        crate::jumplist::delete_for_test();
    }

    /// The searches or the bookmarks changed: the jump list follows.
    pub(super) fn jump_list_later(&self) {
        if self.is_new_window() {
            return;
        }
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_JUMP_LIST, JUMP_LIST_DELAY_MS, None) };
    }

    /// The jump list's entries: recent searches, then the bookmarks.
    fn jump_list_now(&self) {
        let max = crate::jumplist::MAX_ITEMS;
        let searches: Vec<String> = if self.prefs.borrow().base.history.search_enabled {
            self.search_history
                .borrow()
                .entries()
                .iter()
                .take(max)
                .map(|s| s.search.clone())
                .collect()
        } else {
            Vec::new()
        };
        let marks = self.bookmark_launches(max);
        crate::jumplist::update(searches, marks, self.instance.borrow().clone());
    }

    /// The first `max` bookmarks with a search, by name, each with the
    /// switches that open it (`-s`, `-filter`, `-sort`, `-case`, ...).
    fn bookmark_launches(&self, max: usize) -> Vec<(String, Vec<String>)> {
        use crate::bookmarks::Kind;
        let list = self.bookmark_list();
        let mut out = Vec::new();
        for b in list.rows() {
            if out.len() >= max {
                break;
            }
            if b.kind() != Kind::Bookmark || !b.search_set() {
                continue;
            }
            let mut args = vec!["-s".to_owned(), b.search().to_owned()];
            let s = b.settings();
            if let Some(f) = s.filter {
                args.push("-filter".into());
                args.push(f.title().to_owned());
            }
            if let Some(sort) = s.sort {
                args.push("-sort".into());
                args.push(crate::bookmarks::sort_name(sort.key).to_owned());
                args.push(
                    if sort.dir == goz_core::types::SortDir::Asc {
                        "-sort-ascending"
                    } else {
                        "-sort-descending"
                    }
                    .into(),
                );
            }
            if let Some(case) = s.match_case {
                args.push(if case { "-case" } else { "-nocase" }.into());
            }
            if let Some(path) = s.match_path {
                args.push(if path { "-matchpath" } else { "-nomatchpath" }.into());
            }
            out.push((b.name().to_owned(), args));
        }
        out
    }

    pub(super) fn custom_timer(&self, id: usize) {
        if id == TIMER_JUMP_LIST {
            self.jump_list_now();
        }
    }

    pub(super) fn custom_message(&self, msg: u32, _w: WPARAM, _l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_DOUBLE_CTRL => {
                self.double_ctrl();
                Some(0)
            }
            _ => None,
        }
    }

    /// A menu is about to open: an item whose command has keys other than
    /// goz-gui's defaults shows its first (or none) after its tab, and one
    /// rewritten so before whose keys are the defaults again shows what it
    /// showed then.
    pub(super) fn relabel_menu(&self, menu: HMENU) {
        let keys = keymap::current();
        // Out of its cell while the menu calls run.
        let mut texts = self.custom.menu_texts.take();
        if !keys.changed.is_empty() || !texts.is_empty() {
            relabel(menu, &|cmd| keys.menu_key(cmd), &mut texts);
        }
        *self.custom.menu_texts.borrow_mut() = texts;
    }
}

/// What the menu items [`relabel`] rewrote said before, by menu and
/// command: the label and the text after the tab (none without a tab).
type Originals = HashMap<(usize, u16), (String, Option<String>)>;

/// Rewrites the shortcut text of `menu`'s items (not its submenus', which
/// get theirs as they open) as `key` says: `Some` for the keys to show,
/// `None` for a command whose keys are the defaults, whose item gets back
/// what it said before it was first rewritten (kept in `originals`).
fn relabel(menu: HMENU, key: &dyn Fn(u16) -> Option<Option<String>>, originals: &mut Originals) {
    // SAFETY: reads and sets this thread's menu items; the buffers outlive
    // each call.
    unsafe {
        let n = GetMenuItemCount(menu);
        for i in 0..n.max(0) as u32 {
            let mut buf = [0u16; 256];
            let mut mii: MENUITEMINFOW = core::mem::zeroed();
            mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
            mii.fMask = MIIM_FTYPE | MIIM_ID | MIIM_STRING | MIIM_SUBMENU;
            mii.dwTypeData = buf.as_mut_ptr();
            mii.cch = buf.len() as u32;
            if GetMenuItemInfoW(menu, i, 1, &mut mii) == 0
                || mii.fType & MFT_SEPARATOR != 0
                || !mii.hSubMenu.is_null()
                || mii.wID > u32::from(u16::MAX)
            {
                continue;
            }
            let cmd = mii.wID as u16;
            let text = String::from_utf16_lossy(&buf[..mii.cch as usize]);
            let (label, tail) = match text.split_once('\t') {
                Some((label, tail)) => (label, Some(tail)),
                None => (text.as_str(), None),
            };
            let slot = (menu as usize, cmd);
            let wanted = match key(cmd) {
                Some(new_key) => {
                    // The first rewrite keeps what the item said; a record
                    // of another item (a handle used again) gives way.
                    if originals.get(&slot).is_none_or(|(l, _)| l != label) {
                        originals.insert(slot, (label.to_owned(), tail.map(str::to_owned)));
                    }
                    match new_key {
                        Some(k) => format!("{label}\t{k}"),
                        None => label.to_owned(),
                    }
                }
                None => match originals.remove(&slot) {
                    Some((l, t)) if l == label => match t {
                        Some(t) => format!("{label}\t{t}"),
                        None => l,
                    },
                    _ => continue,
                },
            };
            if wanted == text {
                continue;
            }
            let mut wide: Vec<u16> = wanted.encode_utf16().chain([0]).collect();
            let mut set: MENUITEMINFOW = core::mem::zeroed();
            set.cbSize = size_of::<MENUITEMINFOW>() as u32;
            set.fMask = MIIM_STRING;
            set.dwTypeData = wide.as_mut_ptr();
            SetMenuItemInfoW(menu, i, 1, &set);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetMenuStringW, MF_BYCOMMAND, MF_STRING,
    };

    fn text(menu: HMENU, cmd: u16) -> String {
        let mut buf = [0u16; 128];
        // SAFETY: reads an item of a menu this test made into its buffer.
        let n =
            unsafe { GetMenuStringW(menu, u32::from(cmd), buf.as_mut_ptr(), 128, MF_BYCOMMAND) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// Keys back to the defaults put back the shortcut text a menu built
    /// once (View > Zoom) was written with.
    #[test]
    fn menus_get_their_shortcut_text_back_with_the_default_keys() {
        // SAFETY: a menu of this thread's, destroyed at the end.
        let menu = unsafe { CreatePopupMenu() };
        assert!(!menu.is_null());
        for (cmd, label) in [(1u16, "Zoom &In\tCtrl+="), (2, "&Status Bar")] {
            let w: Vec<u16> = label.encode_utf16().chain([0]).collect();
            // SAFETY: appends an item to the menu made above.
            unsafe { AppendMenuW(menu, MF_STRING, usize::from(cmd), w.as_ptr()) };
        }
        let mut originals = Originals::new();
        let f7 = |cmd: u16| (cmd == 1).then(|| Some("F7".to_owned()));
        relabel(menu, &f7, &mut originals);
        assert_eq!(text(menu, 1), "Zoom &In\tF7");
        assert_eq!(text(menu, 2), "&Status Bar");
        // Again, and with no key for the other: what each said first is
        // what is kept, not the F7.
        relabel(menu, &f7, &mut originals);
        let f7_and_none = |cmd: u16| match cmd {
            1 => Some(Some("F7".to_owned())),
            _ => Some(None),
        };
        relabel(menu, &f7_and_none, &mut originals);
        assert_eq!(text(menu, 1), "Zoom &In\tF7");
        assert_eq!(text(menu, 2), "&Status Bar");
        let defaults = |_: u16| None;
        relabel(menu, &defaults, &mut originals);
        assert_eq!(text(menu, 1), "Zoom &In\tCtrl+=");
        assert_eq!(text(menu, 2), "&Status Bar");
        assert!(originals.is_empty());
        // A key with no tab before: the tab goes again.
        let ctrl_b = |cmd: u16| (cmd == 2).then(|| Some("Ctrl+B".to_owned()));
        relabel(menu, &ctrl_b, &mut originals);
        assert_eq!(text(menu, 2), "&Status Bar\tCtrl+B");
        relabel(menu, &defaults, &mut originals);
        assert_eq!(text(menu, 2), "&Status Bar");
        // SAFETY: the menu made above, destroyed once.
        unsafe { DestroyMenu(menu) };
    }
}
