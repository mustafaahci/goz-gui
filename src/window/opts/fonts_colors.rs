//! Options > Fonts and Colors, as Everything's page: a theme (whose colours
//! are being chosen: each theme keeps its own, `dark_...` keys), an item
//! (Result List or Search Edit) with its font and size, and for the result
//! list its item states, each with a foreground colour, a background colour
//! and a font style (Default, or Custom with Bold), with a sample. The keys
//! are Everything's (`crate::colors`). Color Filters... edits the colours
//! rows take from their names (`color_filters`).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, EnumFontFamiliesExW, GetDC,
    InvalidateRect, LOGFONTW, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TEXTMETRICW,
    TRANSPARENT,
};
use windows_sys::Win32::System::SystemServices::{SS_LEFT, SS_OWNERDRAW};
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_GROUPBOX,
    CB_ADDSTRING, CB_ERR, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, CB_RESETCONTENT,
    CB_SELECTSTRING, CB_SETCURSEL, CBN_EDITCHANGE, CBN_SELCHANGE, CBS_DROPDOWN, CBS_DROPDOWNLIST,
    CBS_SORT, LB_ADDSTRING, LB_GETCURSEL, LB_SETCURSEL, LBN_SELCHANGE, LBS_NOINTEGRALHEIGHT,
    LBS_NOTIFY, WM_COMMAND, WM_DRAWITEM, WS_BORDER, WS_GROUP, WS_TABSTOP, WS_VSCROLL,
};

use crate::colors::{Colors, FontSpec, Look, SIZES, State, Weight};
use crate::dialog::Class;
use crate::options::{Invalid, LABEL, Page, Rows, enable, item, order, send, set_text, text};
use crate::paint::fill;
use crate::theme::Palette;
use crate::wide::wz;
use crate::window::App;

/// Everything's control ids.
const THEME: u16 = 1532;
const ITEM: u16 = 1534;
const FONT: u16 = 1142;
const SIZE: u16 = 1143;
const STATES: u16 = 1666;
const GROUP: u16 = 1145;
const FG_DEFAULT: u16 = 1146;
const FG_CUSTOM: u16 = 1147;
const BG_DEFAULT: u16 = 1149;
const BG_CUSTOM: u16 = 1150;
const STYLE_DEFAULT: u16 = 1152;
const STYLE_CUSTOM: u16 = 1153;
const BOLD: u16 = 1154;
const SAMPLE: u16 = 1155;
/// goz-gui's: the colour pickers, their swatches, and Color Filters...
const FG_PICK: u16 = 1180;
const BG_PICK: u16 = 1181;
const FG_SWATCH: u16 = 1182;
const BG_SWATCH: u16 = 1183;
const FILTERS: u16 = 1184;

const THEMES: [(bool, &str); 2] = [(false, "Standard"), (true, "Dark")];
const ITEMS: [&str; 2] = ["Result List", "Search Edit"];
const DEFAULT: &str = "(Default)";

/// The page's working copy, applied at OK or Apply.
pub(super) struct FontsColors {
    colors: RefCell<Colors>,
    dark: Cell<bool>,
    /// 0: the result list, 1: the search box.
    item: Cell<usize>,
    state: Cell<usize>,
    busy: Cell<bool>,
    custom: RefCell<[u32; 16]>,
}

impl FontsColors {
    pub(super) fn new() -> FontsColors {
        FontsColors {
            colors: RefCell::new(Colors::default()),
            dark: Cell::new(false),
            item: Cell::new(0),
            state: Cell::new(0),
            busy: Cell::new(false),
            custom: RefCell::new([0x00ff_ffff; 16]),
        }
    }

    fn quietly(&self, f: impl FnOnce()) {
        let was = self.busy.replace(true);
        f();
        self.busy.set(was);
    }

    fn state(&self) -> State {
        State::ALL[self.state.get().min(State::ALL.len() - 1)]
    }

    fn look(&self) -> Look {
        self.colors.borrow().look(self.dark.get(), self.state())
    }

    fn set_look(&self, page: HWND, look: Look) {
        self.colors
            .borrow_mut()
            .set_look(self.dark.get(), self.state(), look);
        self.repaint(page);
    }

    fn font(&self) -> FontSpec {
        let c = self.colors.borrow();
        if self.item.get() == 0 {
            c.result_list_font.clone()
        } else {
            c.search_edit_font.clone()
        }
    }

    fn set_font(&self, f: impl FnOnce(&mut FontSpec)) {
        let mut c = self.colors.borrow_mut();
        f(if self.item.get() == 0 {
            &mut c.result_list_font
        } else {
            &mut c.search_edit_font
        });
    }

    /// The controls from the working copy: the item's font, the state's
    /// look.
    fn show(&self, page: HWND) {
        let font = self.font();
        let look = self.look();
        let state = self.state();
        let list = self.item.get() == 0;
        self.quietly(|| {
            let face = if font.face.is_empty() {
                DEFAULT.to_owned()
            } else {
                font.face.clone()
            };
            let w = wz(&face);
            if send(page, FONT, CB_SELECTSTRING, usize::MAX, w.as_ptr() as isize) == CB_ERR as isize
            {
                // A face this system has not: listed so it shows.
                send(page, FONT, CB_ADDSTRING, 0, w.as_ptr() as isize);
                send(page, FONT, CB_SELECTSTRING, usize::MAX, w.as_ptr() as isize);
            }
            let size = if font.size == 0 {
                DEFAULT.to_owned()
            } else {
                font.size.to_string()
            };
            set_text(page, SIZE, &size);
            set_text(
                page,
                GROUP,
                &format!("Settings for {} item state", state.label()),
            );
            let radio = |default: u16, custom: u16, on_custom: bool| {
                send(page, default, BM_SETCHECK, usize::from(!on_custom), 0);
                send(page, custom, BM_SETCHECK, usize::from(on_custom), 0);
            };
            radio(FG_DEFAULT, FG_CUSTOM, look.fg.is_some());
            radio(BG_DEFAULT, BG_CUSTOM, look.bg.is_some());
            radio(STYLE_DEFAULT, STYLE_CUSTOM, look.bold != Weight::Default);
            send(
                page,
                BOLD,
                BM_SETCHECK,
                usize::from(look.bold == Weight::Bold),
                0,
            );
        });
        // The states are the result list's.
        for id in [
            STATES,
            FG_DEFAULT,
            FG_CUSTOM,
            BG_DEFAULT,
            BG_CUSTOM,
            STYLE_DEFAULT,
            STYLE_CUSTOM,
        ] {
            enable(page, id, list);
        }
        enable(page, FG_PICK, list && look.fg.is_some());
        enable(page, BG_PICK, list && look.bg.is_some());
        enable(page, BOLD, list && look.bold != Weight::Default);
        self.repaint(page);
    }

    fn repaint(&self, page: HWND) {
        for id in [SAMPLE, FG_SWATCH, BG_SWATCH] {
            // SAFETY: repaints a control of this thread's page.
            unsafe { InvalidateRect(item(page, id), core::ptr::null(), 0) };
        }
    }

    /// The colours the sample and the swatches show: the state's own, or
    /// the theme's.
    fn shown_colours(&self) -> (u32, u32, Option<u32>, Option<u32>, bool) {
        let pal = self.colors.borrow().apply(Palette::new(self.dark.get()));
        let (fg, bg) = match self.state() {
            State::Normal | State::Highlighted => (pal.list_fg, pal.list_bg),
            State::Selected => (pal.sel_fg, pal.sel),
            State::InactiveSelected => (pal.sel_inactive_fg, pal.sel_inactive),
            State::Mouseover => (pal.hover_fg, pal.hover),
            State::CurrentSort => (pal.sort_fg, pal.sort_bg),
        };
        (fg, bg, pal.hl_fg, pal.hl_bg, pal.hl_bold)
    }

    fn pick(&self, page: HWND, foreground: bool) {
        let look = self.look();
        let initial = if foreground { look.fg } else { look.bg };
        let mut custom = *self.custom.borrow();
        let picked = crate::colorfilter_dialog::pick_color(page, initial, &mut custom);
        *self.custom.borrow_mut() = custom;
        if let Some(c) = picked {
            let mut look = self.look();
            if foreground {
                look.fg = Some(c);
            } else {
                look.bg = Some(c);
            }
            self.set_look(page, look);
            self.show(page);
        }
    }

    fn draw(&self, app: &App, dis: &DRAWITEMSTRUCT) {
        let (fg, bg, hl_fg, hl_bg, hl_bold) = self.shown_colours();
        let rc = dis.rcItem;
        let hdc = dis.hDC;
        match dis.CtlID as u16 {
            FG_SWATCH | BG_SWATCH => {
                let look = self.look();
                let c = if dis.CtlID as u16 == FG_SWATCH {
                    look.fg.unwrap_or(fg)
                } else {
                    look.bg.unwrap_or(bg)
                };
                fill(hdc, &rc, 0x0080_8080);
                let inner = RECT {
                    left: rc.left + 1,
                    top: rc.top + 1,
                    right: rc.right - 1,
                    bottom: rc.bottom - 1,
                };
                fill(hdc, &inner, c);
            }
            SAMPLE => {
                fill(hdc, &rc, bg);
                let fonts = app.fonts.borrow();
                let bold_row = self.look().bold == Weight::Bold;
                let base = if bold_row { fonts.bold } else { fonts.normal };
                let matched = if hl_bold { fonts.bold } else { base };
                let mut x = rc.left + 4;
                // SAFETY: draws on the control's own DC for this paint; the
                // old font is put back.
                unsafe {
                    let old = SelectObject(hdc, base);
                    SetBkMode(hdc, TRANSPARENT as i32);
                    for (part, is_match) in
                        [("Annual ", false), ("report", true), (" 2024.pdf", false)]
                    {
                        let mut w: Vec<u16> = part.encode_utf16().collect();
                        SelectObject(hdc, if is_match { matched } else { base });
                        let mut size = windows_sys::Win32::Foundation::SIZE::default();
                        windows_sys::Win32::Graphics::Gdi::GetTextExtentPoint32W(
                            hdc,
                            w.as_ptr(),
                            w.len() as i32,
                            &mut size,
                        );
                        let mut r = RECT {
                            left: x,
                            right: (x + size.cx).min(rc.right),
                            ..rc
                        };
                        if is_match && let Some(b) = hl_bg {
                            fill(hdc, &r, b);
                        }
                        SetTextColor(hdc, if is_match { hl_fg.unwrap_or(fg) } else { fg });
                        DrawTextW(
                            hdc,
                            w.as_mut_ptr(),
                            w.len() as i32,
                            &mut r,
                            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                        );
                        x += size.cx;
                    }
                    SelectObject(hdc, old);
                }
            }
            _ => {}
        }
    }
}

/// The font families this system has, sorted, each once (vertical
/// `@` faces left out).
fn font_families() -> Vec<String> {
    unsafe extern "system" fn each(
        lf: *const LOGFONTW,
        _tm: *const TEXTMETRICW,
        _kind: u32,
        l: LPARAM,
    ) -> i32 {
        // SAFETY: `l` is the vector `font_families` passed, alive for the
        // enumeration; `lf` is the face Windows enumerates.
        unsafe {
            let out = &mut *(l as *mut Vec<String>);
            let face = &(*lf).lfFaceName;
            let n = face.iter().position(|&c| c == 0).unwrap_or(face.len());
            let name = String::from_utf16_lossy(&face[..n]);
            if !name.starts_with('@') && !name.is_empty() {
                out.push(name);
            }
        }
        1
    }
    let mut out: Vec<String> = Vec::new();
    // SAFETY: enumerates the screen DC's fonts into `out`, which outlives
    // the call; the DC is released.
    unsafe {
        let hdc = GetDC(core::ptr::null_mut());
        let lf = LOGFONTW {
            lfCharSet: 1, // DEFAULT_CHARSET: every face once per charset.
            ..core::mem::zeroed()
        };
        EnumFontFamiliesExW(hdc, &lf, Some(each), (&raw mut out) as LPARAM, 0);
        ReleaseDC(core::ptr::null_mut(), hdc);
    }
    out.sort_by_key(|s| s.to_lowercase());
    out.dedup();
    out
}

/// A Size box's text as a size: 0 for the default; `None` if it is none.
pub fn size_of_text(t: &str) -> Option<u32> {
    let t = t.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(DEFAULT) {
        return Some(0);
    }
    t.parse::<u32>().ok().filter(|n| (1..=200).contains(n))
}

fn combo_text(page: HWND, id: u16) -> String {
    let at = send(page, id, CB_GETCURSEL, 0, 0);
    if at < 0 {
        return text(page, id);
    }
    let n = send(page, id, CB_GETLBTEXTLEN, at as usize, 0).max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    send(
        page,
        id,
        CB_GETLBTEXT,
        at as usize,
        buf.as_mut_ptr() as isize,
    );
    String::from_utf16_lossy(&buf[..n])
}

fn checked(page: HWND, id: u16) -> bool {
    send(page, id, BM_GETCHECK, 0, 0) == 1
}

impl Page for FontsColors {
    fn name(&self) -> &'static str {
        "Fonts and Colors"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::FONTS_AND_COLORS
    }

    fn build(&self, rows: &mut Rows<'_>) {
        let w = rows.width();
        rows.combo("&Theme:", THEME, 40, 110)
            .control(Class::Static, "Ite&m:", LABEL, SS_LEFT, 0, (0, 2, 40, 9))
            .control(
                Class::Named("COMBOBOX"),
                "",
                ITEM,
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
                (40, 0, 110, 80),
            )
            .skip(17)
            .control(Class::Static, "&Font:", LABEL, SS_LEFT, 0, (0, 2, 40, 9))
            .control(
                Class::Named("COMBOBOX"),
                "",
                FONT,
                WS_TABSTOP | WS_VSCROLL | (CBS_DROPDOWNLIST | CBS_SORT) as u32,
                0,
                (40, 0, 140, 140),
            )
            .control(Class::Static, "&Size:", LABEL, SS_LEFT, 0, (186, 2, 22, 9))
            .control(
                Class::Named("COMBOBOX"),
                "",
                SIZE,
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32,
                0,
                (208, 0, w - 208, 140),
            )
            .skip(17)
            .control(
                Class::Static,
                "&Item states:",
                LABEL,
                SS_LEFT,
                0,
                (0, 0, w, 9),
            )
            .skip(11)
            .control(
                Class::Named("LISTBOX"),
                "",
                STATES,
                WS_TABSTOP | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
                0,
                (0, 0, w, 52),
            )
            .skip(56)
            .control(
                Class::Button,
                "Settings for Normal item state",
                GROUP,
                BS_GROUPBOX as u32,
                0,
                (0, 0, w, 66),
            );
        let radio = WS_TABSTOP | WS_GROUP | BS_AUTORADIOBUTTON as u32;
        let next = BS_AUTORADIOBUTTON as u32;
        rows.indent(8).skip(12);
        for (label, default, custom, pick, swatch) in [
            (
                "Foreground &color:",
                FG_DEFAULT,
                FG_CUSTOM,
                FG_PICK,
                FG_SWATCH,
            ),
            (
                "&Background color:",
                BG_DEFAULT,
                BG_CUSTOM,
                BG_PICK,
                BG_SWATCH,
            ),
        ] {
            rows.control(Class::Static, label, LABEL, SS_LEFT, 0, (0, 2, 66, 9))
                .control(Class::Button, "Default", default, radio, 0, (66, 1, 40, 10))
                .control(Class::Button, "Custom", custom, next, 0, (108, 1, 40, 10))
                .control(
                    Class::Button,
                    "...",
                    pick,
                    WS_TABSTOP | WS_GROUP,
                    0,
                    (152, 0, 20, 12),
                )
                .control(Class::Static, "", swatch, SS_OWNERDRAW, 0, (176, 1, 24, 10))
                .skip(16);
        }
        rows.control(
            Class::Static,
            "Font st&yle:",
            LABEL,
            SS_LEFT,
            0,
            (0, 2, 66, 9),
        )
        .control(
            Class::Button,
            "Default",
            STYLE_DEFAULT,
            radio,
            0,
            (66, 1, 40, 10),
        )
        .control(
            Class::Button,
            "Custom",
            STYLE_CUSTOM,
            next,
            0,
            (108, 1, 40, 10),
        )
        .control(
            Class::Button,
            "B&old",
            BOLD,
            WS_TABSTOP | WS_GROUP | BS_AUTOCHECKBOX as u32,
            0,
            (152, 1, 50, 10),
        )
        .skip(22)
        .indent(-8);
        rows.control(Class::Static, "Sample:", LABEL, SS_LEFT, 0, (0, 3, 30, 9))
            .control(
                Class::Static,
                "",
                SAMPLE,
                SS_OWNERDRAW,
                0,
                (32, 0, w - 32 - 82, 15),
            )
            .control(
                Class::Button,
                "Color &Filters...",
                FILTERS,
                WS_TABSTOP,
                0,
                (w - 76, 0, 76, 15),
            )
            .skip(19);
    }

    fn fill(&self, app: &App, page: HWND) {
        *self.colors.borrow_mut() = app.custom_settings().colors;
        self.dark.set(app.pal.get().dark);
        self.item.set(0);
        self.state.set(0);
        self.quietly(|| {
            crate::options::fill_combo(page, THEME, &THEMES, self.dark.get());
            for name in ITEMS {
                let w = wz(name);
                send(page, ITEM, CB_ADDSTRING, 0, w.as_ptr() as isize);
            }
            send(page, ITEM, CB_SETCURSEL, 0, 0);
            send(page, FONT, CB_RESETCONTENT, 0, 0);
            let d = wz(DEFAULT);
            send(page, FONT, CB_ADDSTRING, 0, d.as_ptr() as isize);
            for f in font_families() {
                let w = wz(&f);
                send(page, FONT, CB_ADDSTRING, 0, w.as_ptr() as isize);
            }
            send(page, SIZE, CB_ADDSTRING, 0, d.as_ptr() as isize);
            for s in SIZES {
                let w = wz(&s.to_string());
                send(page, SIZE, CB_ADDSTRING, 0, w.as_ptr() as isize);
            }
            for s in State::ALL {
                let w = wz(s.label());
                send(page, STATES, LB_ADDSTRING, 0, w.as_ptr() as isize);
            }
            send(page, STATES, LB_SETCURSEL, 0, 0);
        });
        self.show(page);
    }

    fn changed(&self, _page: HWND, id: u16, code: u32) -> bool {
        matches!(
            (id, code),
            (FONT, CBN_SELCHANGE)
                | (SIZE, CBN_SELCHANGE | CBN_EDITCHANGE)
                | (
                    FG_DEFAULT
                        | FG_CUSTOM
                        | BG_DEFAULT
                        | BG_CUSTOM
                        | STYLE_DEFAULT
                        | STYLE_CUSTOM
                        | BOLD
                        | FG_PICK
                        | BG_PICK,
                    BN_CLICKED
                )
        )
    }

    fn validate(&self, page: HWND) -> Result<(), Invalid> {
        match size_of_text(&text(page, SIZE)) {
            Some(_) => Ok(()),
            None => Err(Invalid {
                control: SIZE,
                message: "Type a font size from 1 to 200 points, or choose (Default).".into(),
            }),
        }
    }

    fn message(&self, app: &App, page: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<isize> {
        if msg == WM_DRAWITEM {
            // SAFETY: WM_DRAWITEM's lParam is the item's DRAWITEMSTRUCT.
            let dis = unsafe { &*(l as *const DRAWITEMSTRUCT) };
            if matches!(dis.CtlID as u16, SAMPLE | FG_SWATCH | BG_SWATCH) {
                self.draw(app, dis);
                return Some(1);
            }
            return None;
        }
        if msg != WM_COMMAND {
            return None;
        }
        let (id, code) = ((w & 0xffff) as u16, ((w >> 16) & 0xffff) as u32);
        if self.busy.get() {
            return Some(0);
        }
        match (id, code) {
            (THEME, CBN_SELCHANGE) => {
                self.dark
                    .set(crate::options::combo_value(page, THEME, &THEMES));
                self.show(page);
                Some(0)
            }
            (ITEM, CBN_SELCHANGE) => {
                self.item
                    .set(send(page, ITEM, CB_GETCURSEL, 0, 0).clamp(0, 1) as usize);
                self.show(page);
                Some(0)
            }
            (STATES, LBN_SELCHANGE) => {
                let at = send(page, STATES, LB_GETCURSEL, 0, 0);
                self.state.set(usize::try_from(at).unwrap_or(0));
                self.show(page);
                Some(0)
            }
            (FONT, CBN_SELCHANGE) => {
                let face = combo_text(page, FONT);
                self.set_font(|f| {
                    f.face = if face == DEFAULT { String::new() } else { face };
                });
                None
            }
            (SIZE, CBN_SELCHANGE) => {
                if let Some(n) = size_of_text(&combo_text(page, SIZE)) {
                    self.set_font(|f| f.size = n);
                }
                None
            }
            (SIZE, CBN_EDITCHANGE) => {
                if let Some(n) = size_of_text(&text(page, SIZE)) {
                    self.set_font(|f| f.size = n);
                }
                None
            }
            (FG_DEFAULT | BG_DEFAULT, BN_CLICKED) => {
                let mut look = self.look();
                if id == FG_DEFAULT {
                    look.fg = None;
                } else {
                    look.bg = None;
                }
                self.set_look(page, look);
                self.show(page);
                None
            }
            (FG_CUSTOM | BG_CUSTOM, BN_CLICKED) => {
                let fg = id == FG_CUSTOM;
                let look = self.look();
                if (if fg { look.fg } else { look.bg }).is_none() {
                    // Custom starts from the colour shown now.
                    let (f, b, ..) = self.shown_colours();
                    let mut look = look;
                    if fg {
                        look.fg = Some(f);
                    } else {
                        look.bg = Some(b);
                    }
                    self.set_look(page, look);
                    self.pick(page, fg);
                }
                self.show(page);
                None
            }
            (FG_PICK | BG_PICK, BN_CLICKED) => {
                self.pick(page, id == FG_PICK);
                None
            }
            (STYLE_DEFAULT | STYLE_CUSTOM | BOLD, BN_CLICKED) => {
                let mut look = self.look();
                look.bold = if checked(page, STYLE_DEFAULT) {
                    Weight::Default
                } else if checked(page, BOLD) {
                    Weight::Bold
                } else {
                    Weight::Normal
                };
                self.set_look(page, look);
                self.show(page);
                None
            }
            (FILTERS, BN_CLICKED) => {
                app.edit_color_filters(page);
                Some(0)
            }
            _ => None,
        }
    }

    fn apply(&self, app: &App, _page: HWND, _problems: &mut Vec<String>) {
        app.set_colors(self.colors.borrow().clone());
    }

    /// Everything's: every font and colour the theme's.
    fn defaults(&self, page: HWND) {
        *self.colors.borrow_mut() = Colors::default();
        self.show(page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_points_or_the_default() {
        assert_eq!(size_of_text("(Default)"), Some(0));
        assert_eq!(size_of_text(""), Some(0));
        assert_eq!(size_of_text(" 11 "), Some(11));
        assert_eq!(size_of_text("0"), None);
        assert_eq!(size_of_text("201"), None);
        assert_eq!(size_of_text("big"), None);
    }
}
