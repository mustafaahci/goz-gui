//! Options > Fonts and Colors, as Everything's page: a theme (whose colours
//! are being chosen: each theme keeps its own, `dark_...` keys), an item
//! (every part of the window goz-gui draws: (Default), the sidebars, the
//! filter and find bars, the header, the menu bar, the panes' title rows,
//! the preview, the result list, the search box, the status bar and the
//! tabs) with its font and size (every item's but the menu bar's, whose
//! items Windows lays out in its menu font) and its accent colour (where
//! it tints a selection), and its item states, each with a foreground
//! colour, a background colour and a font style (Default, or Custom with
//! Bold), with a sample in the item's font. The keys are
//! Everything's (`crate::colors`). Color Filters... edits the colours rows
//! take from their names (`color_filters`).

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, TRANSPARENT,
};
use windows_sys::Win32::System::SystemServices::{SS_LEFT, SS_OWNERDRAW};
use windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_GROUPBOX, CB_ERR, CBN_EDITCHANGE,
    CBN_SELCHANGE, CBS_DROPDOWN, CBS_DROPDOWNLIST, CBS_SORT, LBN_SELCHANGE, LBS_NOINTEGRALHEIGHT,
    LBS_NOTIFY, WM_COMMAND, WM_DRAWITEM, WS_BORDER, WS_GROUP, WS_TABSTOP, WS_VSCROLL,
};

use crate::colors::{Accent, AccentMode, Colors, FontSpec, Item, Look, SIZES, State, Weight};
use crate::dialog::{Class, LIST_BOX};
use crate::options::{Invalid, LABEL, Page, Rows, enable, item, order, set_text, text};
use crate::paint::fill;
use crate::theme::{Ink, Palette};
use crate::win::{self, Msg, combo, dlg, gdi, listbox};
use crate::window::App;

/// Everything's control ids.
const THEME: u16 = 1532;
const ITEM: u16 = 1534;
const FONT: u16 = 1142;
const SIZE: u16 = 1143;
const ACCENT_NONE: u16 = 1398;
const ACCENT_DEFAULT: u16 = 1399;
const ACCENT_CUSTOM: u16 = 1400;
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
const ACCENT_PICK: u16 = 1185;
const ACCENT_SWATCH: u16 = 1186;

const THEMES: [(bool, &str); 2] = [(false, "Standard"), (true, "Dark")];
const DEFAULT: &str = "(Default)";

/// The page's working copy, applied at OK or Apply.
pub(super) struct FontsColors {
    colors: RefCell<Colors>,
    dark: Cell<bool>,
    /// The item chosen (`Item::ALL`) and its state chosen (`Item::states`).
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

    fn item(&self) -> Item {
        Item::ALL[self.item.get().min(Item::ALL.len() - 1)]
    }

    fn state(&self) -> State {
        let states = self.item().states();
        states[self.state.get().min(states.len() - 1)]
    }

    fn look(&self) -> Look {
        self.colors
            .borrow()
            .look(self.dark.get(), self.item(), self.state())
    }

    fn set_look(&self, page: HWND, look: Look) {
        self.colors
            .borrow_mut()
            .set_look(self.dark.get(), self.item(), self.state(), look);
        self.repaint(page);
    }

    fn accent(&self) -> Accent {
        self.colors.borrow().accent(self.dark.get(), self.item())
    }

    fn set_accent(&self, accent: Accent) {
        self.colors
            .borrow_mut()
            .set_accent(self.dark.get(), self.item(), accent);
    }

    /// The colour the accent's swatch shows, and Custom starts from: its
    /// own, else the one in force, else Windows'.
    fn accent_shown(&self) -> u32 {
        let c = self.colors.borrow();
        let a = c.accent(self.dark.get(), self.item());
        a.color
            .or_else(|| c.accent_colour(self.dark.get(), self.item(), &crate::theme::system_accent))
            .unwrap_or_else(crate::theme::system_accent)
    }

    fn font(&self) -> FontSpec {
        self.colors.borrow().font(self.item())
    }

    fn set_font(&self, page: HWND, f: impl FnOnce(&mut FontSpec)) {
        let item = self.item();
        let mut font = self.font();
        f(&mut font);
        self.colors.borrow_mut().set_font(item, font);
        self.repaint(page);
    }

    /// The item's states in the list, the first chosen.
    fn fill_states(&self, page: HWND) {
        self.state.set(0);
        self.quietly(|| {
            let list = item(page, STATES);
            listbox::reset(list);
            for s in self.item().states() {
                listbox::add(list, s.label());
            }
            listbox::set_cur_sel(list, 0);
        });
    }

    /// The controls from the working copy: the item's font and accent, the
    /// state's look.
    fn show(&self, page: HWND) {
        let item = self.item();
        let font = self.font();
        let look = self.look();
        let state = self.state();
        let accent = self.accent();
        let has_accent = item.has_accent();
        let has_bold = item.has_bold(state);
        self.quietly(|| {
            let face = if font.face.is_empty() {
                DEFAULT.to_owned()
            } else {
                font.face.clone()
            };
            let fonts = dlg::item(page, FONT);
            if win::combo::select_string(fonts, -1, &face) == CB_ERR {
                // A face this system has not: listed so it shows.
                combo::add(fonts, &face);
                win::combo::select_string(fonts, -1, &face);
            }
            let size = if font.size == 0 {
                DEFAULT.to_owned()
            } else {
                font.size.to_string()
            };
            set_text(page, SIZE, &size);
            // Setting the box's text selects it, and a combo box's text
            // shows its selection with or without the keyboard: none,
            // until the box is used (as the Font list beside it). A start
            // of -1 takes the selection away.
            combo::set_edit_sel(dlg::item(page, SIZE), -1, 0);
            for (id, mode) in [
                (ACCENT_NONE, AccentMode::None),
                (ACCENT_DEFAULT, AccentMode::Default),
                (ACCENT_CUSTOM, AccentMode::Custom),
            ] {
                let on = has_accent && accent.mode == mode;
                dlg::set_checked(page, id, on);
            }
            set_text(
                page,
                GROUP,
                &format!("Settings for {} item state", state.label()),
            );
            let radio = |default: u16, custom: u16, on_custom: bool| {
                dlg::set_checked(page, default, !on_custom);
                dlg::set_checked(page, custom, on_custom);
            };
            radio(FG_DEFAULT, FG_CUSTOM, look.fg.is_some());
            radio(BG_DEFAULT, BG_CUSTOM, look.bg.is_some());
            radio(
                STYLE_DEFAULT,
                STYLE_CUSTOM,
                has_bold && look.bold != Weight::Default,
            );
            dlg::set_checked(page, BOLD, has_bold && look.bold == Weight::Bold);
        });
        // Only what goz-gui draws can be chosen.
        for id in [FONT, SIZE] {
            enable(page, id, item.has_font());
        }
        for id in [ACCENT_NONE, ACCENT_DEFAULT, ACCENT_CUSTOM] {
            enable(page, id, has_accent);
        }
        enable(
            page,
            ACCENT_PICK,
            has_accent && accent.mode == AccentMode::Custom,
        );
        enable(page, FG_PICK, look.fg.is_some());
        enable(page, BG_PICK, look.bg.is_some());
        for id in [STYLE_DEFAULT, STYLE_CUSTOM] {
            enable(page, id, has_bold);
        }
        enable(page, BOLD, has_bold && look.bold != Weight::Default);
        self.repaint(page);
    }

    fn repaint(&self, page: HWND) {
        for id in [SAMPLE, FG_SWATCH, BG_SWATCH, ACCENT_SWATCH] {
            win::invalidate(item(page, id), None, false);
        }
    }

    /// The palette the working copy makes of the theme chosen.
    fn palette(&self) -> Palette {
        self.colors
            .borrow()
            .apply(Palette::new(self.dark.get()), &crate::theme::system_accent)
    }

    /// The colours the sample and the swatches show: the state's (its own,
    /// or what it takes).
    fn shown_colours(&self) -> Ink {
        let pal = self.palette();
        match (self.item(), self.state()) {
            (item, State::Highlighted) => pal.ink(item, State::Normal),
            (item, state) => pal.ink(item, state),
        }
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

    fn pick_accent(&self, page: HWND) {
        let mut custom = *self.custom.borrow();
        let picked =
            crate::colorfilter_dialog::pick_color(page, Some(self.accent_shown()), &mut custom);
        *self.custom.borrow_mut() = custom;
        if let Some(c) = picked {
            self.set_accent(Accent {
                mode: AccentMode::Custom,
                color: Some(c),
            });
        }
        self.show(page);
    }

    fn draw(&self, app: &App, dis: &DRAWITEMSTRUCT) {
        let pal = self.palette();
        let Ink { fg, bg } = self.shown_colours();
        let rc = dis.rcItem;
        let hdc = dis.hDC;
        let swatch = |c: u32| {
            fill(hdc, &rc, 0x0080_8080);
            let inner = RECT {
                left: rc.left + 1,
                top: rc.top + 1,
                right: rc.right - 1,
                bottom: rc.bottom - 1,
            };
            fill(hdc, &inner, c);
        };
        match dis.CtlID as u16 {
            FG_SWATCH => swatch(self.look().fg.unwrap_or(fg)),
            BG_SWATCH => swatch(self.look().bg.unwrap_or(bg)),
            ACCENT_SWATCH => {
                if self.item().has_accent() {
                    swatch(self.accent_shown());
                } else {
                    fill(hdc, &rc, 0x0080_8080);
                }
            }
            SAMPLE => {
                fill(hdc, &rc, bg);
                let item = self.item();
                let list = item == Item::ResultList;
                // The item's font as chosen on the page (the menu bar's is
                // Windows'), at the window's DPI.
                let spec = if item.has_font() {
                    self.colors.borrow().resolved_font(item)
                } else {
                    FontSpec::default()
                };
                let fonts = crate::window::Fonts::with(app.dpi.get(), crate::zoom::DEFAULT, &spec);
                let state = self.state();
                let bold_row = pal.is_bold(item, state)
                    || state == State::Highlighted && pal.is_bold(item, State::Normal);
                let base = if bold_row { fonts.bold } else { fonts.normal };
                let matched = if pal.hl_bold { fonts.bold } else { base };
                let parts: &[(&str, bool)] = if list {
                    &[("Annual ", false), ("report", true), (" 2024.pdf", false)]
                } else {
                    &[("Annual report 2024.pdf", false)]
                };
                let mut x = rc.left + 4;
                // On the control's own DC for this paint; the old font is
                // put back.
                let old = gdi::select(hdc, base);
                gdi::set_bk_mode(hdc, TRANSPARENT as i32);
                for &(part, is_match) in parts {
                    let w: Vec<u16> = part.encode_utf16().collect();
                    gdi::select(hdc, if is_match { matched } else { base });
                    let size = gdi::text_extent(hdc, &w);
                    let mut r = RECT {
                        left: x,
                        right: (x + size.cx).min(rc.right),
                        ..rc
                    };
                    if is_match && let Some(b) = pal.hl_bg {
                        fill(hdc, &r, b);
                    }
                    gdi::set_text_color(
                        hdc,
                        if is_match {
                            pal.hl_fg.unwrap_or(fg)
                        } else {
                            fg
                        },
                    );
                    gdi::draw_text(
                        hdc,
                        &w,
                        &mut r,
                        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                    );
                    x += size.cx;
                }
                old.restore(hdc);
            }
            _ => {}
        }
    }
}

/// The font families this system has, sorted, each once (vertical
/// `@` faces left out).
fn font_families() -> Vec<String> {
    // DEFAULT_CHARSET: every face once per charset.
    let mut out: Vec<String> = win::gdi::font_faces(1)
        .into_iter()
        .filter(|name| !name.starts_with('@') && !name.is_empty())
        .collect();
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
    let list = item(page, id);
    let at = combo::cur_sel(list);
    if at < 0 {
        return text(page, id);
    }
    combo::text(list, at)
}

fn checked(page: HWND, id: u16) -> bool {
    dlg::is_checked(page, id)
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
        let radio = WS_TABSTOP | WS_GROUP | BS_AUTORADIOBUTTON as u32;
        let next = BS_AUTORADIOBUTTON as u32;
        rows.combo("&Theme:", THEME, 40, 110)
            .control(Class::Static, "Ite&m:", LABEL, SS_LEFT, 0, (0, 2, 40, 9))
            .control(
                Class::Named("COMBOBOX"),
                "",
                ITEM,
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
                (40, 0, 110, 160),
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
                "Acce&nt color:",
                LABEL,
                SS_LEFT,
                0,
                (0, 2, 50, 9),
            )
            .control(
                Class::Button,
                "None",
                ACCENT_NONE,
                radio,
                0,
                (50, 1, 32, 10),
            )
            .control(
                Class::Button,
                "Default",
                ACCENT_DEFAULT,
                next,
                0,
                (84, 1, 40, 10),
            )
            .control(
                Class::Button,
                "Custom",
                ACCENT_CUSTOM,
                next,
                0,
                (126, 1, 38, 10),
            )
            .control(
                Class::Button,
                "...",
                ACCENT_PICK,
                WS_TABSTOP | WS_GROUP,
                0,
                (166, 0, 20, 12),
            )
            .control(
                Class::Static,
                "",
                ACCENT_SWATCH,
                SS_OWNERDRAW,
                0,
                (190, 1, 24, 10),
            )
            .skip(16)
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
                WS_TABSTOP
                    | WS_VSCROLL
                    | WS_BORDER
                    | LIST_BOX
                    | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
                0,
                (0, 0, w, 48),
            )
            .skip(52)
            .control(
                Class::Button,
                "Settings for Normal item state",
                GROUP,
                BS_GROUPBOX as u32,
                0,
                (0, 0, w, 66),
            );
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
        self.quietly(|| {
            crate::options::fill_combo(page, THEME, &THEMES, self.dark.get());
            let items = item(page, ITEM);
            combo::reset(items);
            for i in Item::ALL {
                combo::add(items, i.label());
            }
            combo::set_cur_sel(items, 0);
            let fonts = item(page, FONT);
            combo::reset(fonts);
            combo::add(fonts, DEFAULT);
            for f in font_families() {
                combo::add(fonts, &f);
            }
            let sizes = item(page, SIZE);
            combo::reset(sizes);
            combo::add(sizes, DEFAULT);
            for s in SIZES {
                combo::add(sizes, &s.to_string());
            }
        });
        self.fill_states(page);
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
                        | BG_PICK
                        | ACCENT_NONE
                        | ACCENT_DEFAULT
                        | ACCENT_CUSTOM
                        | ACCENT_PICK,
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

    fn message(&self, app: &App, page: HWND, m: &Msg) -> Option<isize> {
        if m.id == WM_DRAWITEM {
            let dis = m.draw_item()?;
            if matches!(
                dis.CtlID as u16,
                SAMPLE | FG_SWATCH | BG_SWATCH | ACCENT_SWATCH
            ) {
                self.draw(app, &dis);
                return Some(1);
            }
            return None;
        }
        if m.id != WM_COMMAND {
            return None;
        }
        let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
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
                let at = combo::cur_sel(item(page, ITEM));
                self.item
                    .set(usize::try_from(at).unwrap_or(0).min(Item::ALL.len() - 1));
                self.fill_states(page);
                self.show(page);
                Some(0)
            }
            (STATES, LBN_SELCHANGE) => {
                let at = listbox::cur_sel(item(page, STATES));
                self.state.set(usize::try_from(at).unwrap_or(0));
                self.show(page);
                Some(0)
            }
            (FONT, CBN_SELCHANGE) => {
                let face = combo_text(page, FONT);
                self.set_font(page, |f| {
                    f.face = if face == DEFAULT { String::new() } else { face };
                });
                None
            }
            (SIZE, CBN_SELCHANGE) => {
                if let Some(n) = size_of_text(&combo_text(page, SIZE)) {
                    self.set_font(page, |f| f.size = n);
                }
                None
            }
            (SIZE, CBN_EDITCHANGE) => {
                if let Some(n) = size_of_text(&text(page, SIZE)) {
                    self.set_font(page, |f| f.size = n);
                }
                None
            }
            (ACCENT_NONE | ACCENT_DEFAULT, BN_CLICKED) => {
                let mut a = self.accent();
                a.mode = if id == ACCENT_NONE {
                    AccentMode::None
                } else {
                    AccentMode::Default
                };
                self.set_accent(a);
                self.show(page);
                None
            }
            (ACCENT_CUSTOM, BN_CLICKED) => {
                let a = self.accent();
                if a.color.is_none() {
                    // Custom starts from the colour shown now.
                    self.pick_accent(page);
                } else {
                    self.set_accent(Accent {
                        mode: AccentMode::Custom,
                        ..a
                    });
                    self.show(page);
                }
                None
            }
            (ACCENT_PICK, BN_CLICKED) => {
                self.pick_accent(page);
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
                    let shown = self.shown_colours();
                    let mut look = look;
                    if fg {
                        look.fg = Some(shown.fg);
                    } else {
                        look.bg = Some(shown.bg);
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

    /// Everything's: every font, colour and accent the theme's.
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
