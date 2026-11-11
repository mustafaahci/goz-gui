//! Options > Fonts and Colors: the colours of every part of the window
//! goz-gui draws, in each of its states, in the light and the dark theme;
//! the accent colour; and the result list's and the search box's fonts,
//! under Everything 1.5's INI keys.
//!
//! Everything keeps an item state's look as `<item><state>_foreground_color`,
//! `<item><state>_background_color` (`#RRGGBB`, empty for the theme's own)
//! and `<item><state>_bold` (empty or 1 the default style, 0 normal, 2
//! bold), the dark theme's under the same names with `dark_` in front
//! (`selected_background_color`, `dark_treeview_selected_background_color`);
//! an item's accent as `<item>_accent` (0 None, 1 Default, 2 Custom; the
//! dark theme's `<item>_dark_accent`) with `<item>_accent_color`, the
//! (Default) item's as plain `accent` and `accent_color`; the fonts as
//! `result_list_font` / `search_edit_font` (a face name, empty for the
//! default) and `..._font_size` (points, empty for the default).
//!
//! Only the items and states goz-gui draws are here (no Toolbar, no Hot
//! or Drop Target rows, no per-state Highlighted variants): each one's
//! colours reach the drawing code through [`Colors::apply`], which fills
//! the palette's [`Ink`]s.

use std::collections::BTreeMap;

use crate::theme::{Ink, Palette};

/// A font chosen for a part of the window: empty face and 0 size are the
/// default's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontSpec {
    pub face: String,
    pub size: u32,
}

/// The sizes Everything's Size box lists.
pub const SIZES: [u32; 16] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 22, 24, 26, 28, 36, 48, 72];
/// The largest size read from the file.
const SIZE_MAX: u32 = 200;

/// A font style of an item state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Weight {
    /// The item's own: in the result list matches bold and other text
    /// normal; elsewhere normal.
    #[default]
    Default,
    Normal,
    Bold,
}

impl Weight {
    fn from_value(v: &str) -> Weight {
        match v.trim() {
            "0" => Weight::Normal,
            "2" => Weight::Bold,
            _ => Weight::Default,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Weight::Default => "",
            Weight::Normal => "0",
            Weight::Bold => "2",
        }
    }
}

/// One item state's look: `None` is the theme's colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: Weight,
}

/// The parts of the window Fonts and Colors sets, in Everything's order
/// of its Item box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Item {
    /// What every item's Normal state takes where it has no colour of its
    /// own; its accent is every item's that says Default.
    Default,
    BookmarksSidebar,
    FilterBar,
    FiltersSidebar,
    FindBar,
    FoldersSidebar,
    Header,
    /// The menu bar (goz-gui draws it in the dark theme, and in the light
    /// one once it has a colour of its own).
    Menu,
    /// A pane's title row.
    Pane,
    Preview,
    ResultList,
    SearchEdit,
    StatusBar,
    Tabs,
}

impl Item {
    pub const ALL: [Item; 14] = [
        Item::Default,
        Item::BookmarksSidebar,
        Item::FilterBar,
        Item::FiltersSidebar,
        Item::FindBar,
        Item::FoldersSidebar,
        Item::Header,
        Item::Menu,
        Item::Pane,
        Item::Preview,
        Item::ResultList,
        Item::SearchEdit,
        Item::StatusBar,
        Item::Tabs,
    ];

    /// Everything's name in the Item box.
    pub fn label(self) -> &'static str {
        match self {
            Item::Default => "(Default)",
            Item::BookmarksSidebar => "Bookmarks Sidebar",
            Item::FilterBar => "Filter Bar",
            Item::FiltersSidebar => "Filters Sidebar",
            Item::FindBar => "Find Bar",
            Item::FoldersSidebar => "Folders Sidebar",
            Item::Header => "Header",
            Item::Menu => "Menu",
            Item::Pane => "Pane",
            Item::Preview => "Preview",
            Item::ResultList => "Result List",
            Item::SearchEdit => "Search Edit",
            Item::StatusBar => "Status Bar",
            Item::Tabs => "Tabs",
        }
    }

    /// The front of the item's state keys (`treeview_` +
    /// `selected_background_color`).
    fn prefix(self) -> &'static str {
        match self {
            Item::Default => "default_",
            Item::BookmarksSidebar => "bookmark_sidebar_",
            Item::FilterBar => "filterbar_",
            Item::FiltersSidebar => "filter_sidebar_",
            Item::FindBar => "findbar_",
            Item::FoldersSidebar => "treeview_",
            Item::Header => "header_",
            Item::Menu => "menu_",
            Item::Pane => "pane_",
            Item::Preview => "preview_",
            Item::ResultList => "",
            Item::SearchEdit => "search_",
            Item::StatusBar => "statusbar_",
            Item::Tabs => "tabs_",
        }
    }

    /// The stem of the item's font and accent keys (`result_list_font`,
    /// `treeview_accent`); the (Default) item's are bare (`accent`).
    fn stem(self) -> &'static str {
        match self {
            Item::Default => "",
            Item::BookmarksSidebar => "bookmark_sidebar",
            Item::FilterBar => "filter_bar",
            Item::FiltersSidebar => "filter_sidebar",
            Item::FindBar => "findbar",
            Item::FoldersSidebar => "treeview",
            Item::Header => "header",
            Item::Menu => "menu",
            Item::Pane => "pane",
            Item::Preview => "preview",
            Item::ResultList => "result_list",
            Item::SearchEdit => "search_edit",
            Item::StatusBar => "status_bar",
            Item::Tabs => "tabs",
        }
    }

    /// The item's states goz-gui draws, in Everything's order.
    pub fn states(self) -> &'static [State] {
        use State::*;
        match self {
            Item::ResultList => &[
                Normal,
                Highlighted,
                Selected,
                InactiveSelected,
                Mouseover,
                AlternateRow,
                CurrentSort,
                CurrentSortMouseover,
                CurrentSortAlternateRow,
                ThumbnailMouseover,
            ],
            Item::BookmarksSidebar | Item::FiltersSidebar | Item::FoldersSidebar => {
                &[Normal, Selected, InactiveSelected, Mouseover]
            }
            Item::Header => &[Normal, CurrentSort],
            Item::Tabs => &[Normal, Selected],
            _ => &[Normal],
        }
    }

    /// goz-gui sets this item's font and size (the others keep the
    /// window's).
    pub fn has_font(self) -> bool {
        matches!(self, Item::ResultList | Item::SearchEdit)
    }

    /// The accent colour tints this item's Selected and Mouseover rows
    /// ((Default): every such item's that says Default).
    pub fn has_accent(self) -> bool {
        matches!(
            self,
            Item::Default
                | Item::ResultList
                | Item::FoldersSidebar
                | Item::FiltersSidebar
                | Item::BookmarksSidebar
        )
    }

    /// goz-gui draws this item state's text bold when its style says so
    /// (the menu bar's items are measured by Windows; the find bar, the
    /// preview's text, the thumbnail names and (Default) have no style of
    /// their own).
    pub fn has_bold(self, state: State) -> bool {
        !matches!(
            self,
            Item::Default | Item::Menu | Item::FindBar | Item::Preview
        ) && state != State::ThumbnailMouseover
    }

    /// The item's first slot in [`SLOTS`] order.
    fn first_slot(self) -> usize {
        let mut n = 0;
        for i in Item::ALL {
            if i == self {
                break;
            }
            n += i.states().len();
        }
        n
    }
}

/// An item state, with Everything's names and key stems.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    Normal,
    /// Matched text in the result list.
    Highlighted,
    Selected,
    InactiveSelected,
    Mouseover,
    AlternateRow,
    CurrentSort,
    CurrentSortMouseover,
    CurrentSortAlternateRow,
    /// The cell under the mouse in the thumbnail views.
    ThumbnailMouseover,
}

impl State {
    /// Everything's name in the Item states list.
    pub fn label(self) -> &'static str {
        match self {
            State::Normal => "Normal",
            State::Highlighted => "Normal (Highlighted)",
            State::Selected => "Selected",
            State::InactiveSelected => "Inactive Selected",
            State::Mouseover => "Mouseover Row",
            State::AlternateRow => "Alternate Row",
            State::CurrentSort => "Current Sort",
            State::CurrentSortMouseover => "Current Sort Mouseover Row",
            State::CurrentSortAlternateRow => "Current Sort Alternate Row",
            State::ThumbnailMouseover => "Thumbnail Mouseover",
        }
    }

    /// A sidebar row's state: selected (with the focus or without), under
    /// the mouse, or neither.
    pub fn of_row(selected: bool, focused: bool, hot: bool) -> State {
        match (selected, focused, hot) {
            (true, true, _) => State::Selected,
            (true, false, _) => State::InactiveSelected,
            (false, _, true) => State::Mouseover,
            (false, _, false) => State::Normal,
        }
    }

    fn stem(self) -> &'static str {
        match self {
            State::Normal => "normal",
            State::Highlighted => "highlighted",
            State::Selected => "selected",
            State::InactiveSelected => "selected_inactive",
            State::Mouseover => "mouseover",
            State::AlternateRow => "alternate_row",
            State::CurrentSort => "current_sort",
            State::CurrentSortMouseover => "mouseover_current_sort",
            State::CurrentSortAlternateRow => "current_sort_alternate_row",
            State::ThumbnailMouseover => "thumbnail_mouseover",
        }
    }
}

/// How many item states there are, every item's together.
pub const SLOTS: usize = 34;

/// An item state's place in the palette's inks (`Palette::inks`).
pub fn slot(item: Item, state: State) -> usize {
    let at = item.states().iter().position(|&s| s == state);
    debug_assert!(at.is_some(), "{state:?} is not a state of {item:?}");
    item.first_slot() + at.unwrap_or(0)
}

/// Every item state, in slot order.
pub fn all_slots() -> impl Iterator<Item = (Item, State)> {
    Item::ALL
        .into_iter()
        .flat_map(|i| i.states().iter().map(move |&s| (i, s)))
}

/// What an item's accent is: Everything's `accent` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentMode {
    /// No accent: the theme's greys (and the light theme's blues).
    None,
    /// (Default): Windows' accent colour; any other item: (Default)'s.
    Default,
    Custom,
}

/// An item's accent: its mode, and the colour Custom uses (kept while
/// another mode is chosen, as Everything keeps `accent_color`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accent {
    pub mode: AccentMode,
    pub color: Option<u32>,
}

impl Accent {
    /// Everything's: None for (Default), Default (as (Default)) for the
    /// others.
    pub fn default_for(item: Item) -> Accent {
        Accent {
            mode: if item == Item::Default {
                AccentMode::None
            } else {
                AccentMode::Default
            },
            color: None,
        }
    }
}

/// `#RRGGBB` as a COLORREF (0x00BBGGRR).
pub fn parse_color(v: &str) -> Option<u32> {
    let hex = v.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    let (r, g, b) = (n >> 16, (n >> 8) & 0xff, n & 0xff);
    Some(r | g << 8 | b << 16)
}

/// A COLORREF as `#RRGGBB`.
pub fn format_color(c: u32) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        c & 0xff,
        (c >> 8) & 0xff,
        (c >> 16) & 0xff
    )
}

/// The fonts, every item state's colours and style, and the accents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Colors {
    pub result_list_font: FontSpec,
    pub search_edit_font: FontSpec,
    /// The item states with a look of their own, by theme (dark) and slot;
    /// an all-default look is never kept, so equal settings compare equal.
    looks: BTreeMap<(bool, usize), Look>,
    /// The items whose accent is not their default, by theme and item.
    accents: BTreeMap<(bool, Item), Accent>,
}

fn state_key(dark: bool, item: Item, state: State, part: &str) -> String {
    format!(
        "{}{}{}_{part}",
        if dark { "dark_" } else { "" },
        item.prefix(),
        state.stem()
    )
}

/// `accent`, `dark_accent`, `treeview_accent`, `treeview_dark_accent`;
/// with `_color` after for the colour.
fn accent_key(dark: bool, item: Item, color: bool) -> String {
    let stem = item.stem();
    let mut k = String::new();
    if !stem.is_empty() {
        k.push_str(stem);
        k.push('_');
    }
    if dark {
        k.push_str("dark_");
    }
    k.push_str("accent");
    if color {
        k.push_str("_color");
    }
    k
}

fn font_from<'a>(get: &impl Fn(&str) -> Option<&'a str>, stem: &str) -> FontSpec {
    FontSpec {
        face: get(&format!("{stem}_font"))
            .map(|v| v.trim().chars().take(31).collect())
            .unwrap_or_default(),
        size: get(&format!("{stem}_font_size"))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&n| n <= SIZE_MAX)
            .unwrap_or(0),
    }
}

/// `a` moved `t` (0 to 1) of the way to `b`, per channel.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let one = |s: u32| {
        let v = ch(a, s) + (ch(b, s) - ch(a, s)) * t;
        (v.round().clamp(0.0, 255.0) as u32) << s
    };
    one(0) | one(8) | one(16)
}

/// How far an accent tints the Selected and the Mouseover rows' background
/// towards it, in the light and the dark theme (goz-gui's amounts: about
/// Explorer's light blues for Windows' default accent).
const ACCENT_SELECTED: [f32; 2] = [0.25, 0.5];
const ACCENT_MOUSEOVER: [f32; 2] = [0.1, 0.25];

impl Colors {
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Colors {
        let mut c = Colors {
            result_list_font: font_from(&get, "result_list"),
            search_edit_font: font_from(&get, "search_edit"),
            ..Colors::default()
        };
        for dark in [false, true] {
            for (item, state) in all_slots() {
                let k = |part| state_key(dark, item, state, part);
                c.set_look(
                    dark,
                    item,
                    state,
                    Look {
                        fg: get(&k("foreground_color")).and_then(parse_color),
                        bg: get(&k("background_color")).and_then(parse_color),
                        bold: get(&k("bold")).map_or(Weight::Default, Weight::from_value),
                    },
                );
            }
            for item in Item::ALL.into_iter().filter(|i| i.has_accent()) {
                let mut a = Accent::default_for(item);
                if let Some(v) = get(&accent_key(dark, item, false)) {
                    a.mode = match v.trim() {
                        "0" => AccentMode::None,
                        "1" => AccentMode::Default,
                        "2" => AccentMode::Custom,
                        _ => a.mode,
                    };
                }
                a.color = get(&accent_key(dark, item, true)).and_then(parse_color);
                c.set_accent(dark, item, a);
            }
        }
        c
    }

    pub fn to_keys(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (stem, f) in [
            ("result_list", &self.result_list_font),
            ("search_edit", &self.search_edit_font),
        ] {
            out.push((format!("{stem}_font"), f.face.clone()));
            out.push((
                format!("{stem}_font_size"),
                if f.size == 0 {
                    String::new()
                } else {
                    f.size.to_string()
                },
            ));
        }
        for dark in [false, true] {
            for item in Item::ALL.into_iter().filter(|i| i.has_accent()) {
                let a = self.accent(dark, item);
                let mode = match a.mode {
                    AccentMode::None => "0",
                    AccentMode::Default => "1",
                    AccentMode::Custom => "2",
                };
                out.push((accent_key(dark, item, false), mode.to_owned()));
                out.push((
                    accent_key(dark, item, true),
                    a.color.map(format_color).unwrap_or_default(),
                ));
            }
            for (item, state) in all_slots() {
                let l = self.look(dark, item, state);
                let c = |c: Option<u32>| c.map(format_color).unwrap_or_default();
                out.push((state_key(dark, item, state, "foreground_color"), c(l.fg)));
                out.push((state_key(dark, item, state, "background_color"), c(l.bg)));
                out.push((
                    state_key(dark, item, state, "bold"),
                    l.bold.value().to_owned(),
                ));
            }
        }
        out
    }

    pub fn look(&self, dark: bool, item: Item, state: State) -> Look {
        self.looks
            .get(&(dark, slot(item, state)))
            .copied()
            .unwrap_or_default()
    }

    pub fn set_look(&mut self, dark: bool, item: Item, state: State, look: Look) {
        let key = (dark, slot(item, state));
        if look == Look::default() {
            self.looks.remove(&key);
        } else {
            self.looks.insert(key, look);
        }
    }

    pub fn accent(&self, dark: bool, item: Item) -> Accent {
        self.accents
            .get(&(dark, item))
            .copied()
            .unwrap_or_else(|| Accent::default_for(item))
    }

    pub fn set_accent(&mut self, dark: bool, item: Item, accent: Accent) {
        if accent == Accent::default_for(item) {
            self.accents.remove(&(dark, item));
        } else {
            self.accents.insert((dark, item), accent);
        }
    }

    /// The colour `item`'s accent tints with in the theme, if any:
    /// `windows` gives Windows' accent colour when it is asked for.
    pub fn accent_colour(&self, dark: bool, item: Item, windows: &dyn Fn() -> u32) -> Option<u32> {
        if !item.has_accent() {
            return None;
        }
        let a = self.accent(dark, item);
        match a.mode {
            AccentMode::None => None,
            AccentMode::Custom => a.color,
            AccentMode::Default if item == Item::Default => Some(windows()),
            AccentMode::Default => self.accent_colour(dark, Item::Default, windows),
        }
    }

    /// `pal` with every item state's colours of its theme put in: the
    /// state's own, else (for Normal) (Default)'s, else what it follows
    /// (the accent's tint, the Normal row's shade, the Mouseover row's
    /// colour), else the theme's. Text follows the item's Normal text
    /// unless the state has its own. `windows` gives Windows' accent
    /// colour if an accent of Default needs it.
    pub fn apply(&self, mut pal: Palette, windows: &dyn Fn() -> u32) -> Palette {
        let dark = pal.dark;
        let theme = pal.inks;
        let mut inks = theme;
        let mut own = 0u64;
        // The states whose background is not the theme's.
        let mut own_bg = 0u64;
        let mut bold = 0u64;
        let base = self.look(dark, Item::Default, State::Normal);
        let k = usize::from(dark);
        for (item, state) in all_slots() {
            let at = slot(item, state);
            let look = self.look(dark, item, state);
            let t = theme[at];
            let resolved = |s: State, inks: &[Ink; SLOTS]| inks[slot(item, s)];
            let is_own = |s: State, own_bg: u64| own_bg & 1 << slot(item, s) != 0;
            let accent = self.accent_colour(dark, item, windows);
            let (fg, bg) = match state {
                State::Normal => (
                    look.fg.or(base.fg).unwrap_or(t.fg),
                    look.bg.or(base.bg).unwrap_or(t.bg),
                ),
                _ => {
                    let normal = resolved(State::Normal, &inks);
                    // The state whose text and background this one takes
                    // when it has none of its own and is not tinted.
                    let follows = match state {
                        State::CurrentSortMouseover | State::ThumbnailMouseover => {
                            Some(State::Mouseover)
                        }
                        _ => None,
                    };
                    let fg = look.fg.unwrap_or(match state {
                        State::CurrentSortMouseover | State::ThumbnailMouseover => {
                            resolved(State::Mouseover, &inks).fg
                        }
                        State::CurrentSortAlternateRow => resolved(State::CurrentSort, &inks).fg,
                        _ => normal.fg,
                    });
                    let bg = match (look.bg, state, accent) {
                        (Some(bg), ..) => bg,
                        (None, State::Selected, Some(a)) => mix(normal.bg, a, ACCENT_SELECTED[k]),
                        (None, State::Mouseover, Some(a)) => mix(normal.bg, a, ACCENT_MOUSEOVER[k]),
                        (None, State::AlternateRow, _) if is_own(State::Normal, own_bg) => {
                            shade(normal.bg)
                        }
                        (None, State::CurrentSortAlternateRow, _)
                            if is_own(State::CurrentSort, own_bg) =>
                        {
                            shade(resolved(State::CurrentSort, &inks).bg)
                        }
                        (None, ..) => match follows {
                            Some(s) if is_own(s, own_bg) => resolved(s, &inks).bg,
                            _ => t.bg,
                        },
                    };
                    (fg, bg)
                }
            };
            inks[at] = Ink { fg, bg };
            if inks[at] != t {
                own |= 1 << at;
            }
            if bg != t.bg {
                own_bg |= 1 << at;
            }
            if look.bold == Weight::Bold && item.has_bold(state) {
                bold |= 1 << at;
            }
        }
        pal.inks = inks;
        pal.own = own;
        pal.bold = bold;
        // The result list's, where the drawing code reads them.
        let ink = |s: State| inks[slot(Item::ResultList, s)];
        pal.list_bg = ink(State::Normal).bg;
        pal.list_fg = ink(State::Normal).fg;
        pal.sel = ink(State::Selected).bg;
        pal.sel_fg = ink(State::Selected).fg;
        pal.sel_inactive = ink(State::InactiveSelected).bg;
        pal.sel_inactive_fg = ink(State::InactiveSelected).fg;
        pal.hover = ink(State::Mouseover).bg;
        pal.hover_fg = ink(State::Mouseover).fg;
        pal.alt_bg = ink(State::AlternateRow).bg;
        pal.alt_fg = ink(State::AlternateRow).fg;
        pal.sort_bg = ink(State::CurrentSort).bg;
        pal.sort_fg = ink(State::CurrentSort).fg;
        pal.hover_sort = ink(State::CurrentSortMouseover).bg;
        pal.hover_sort_fg = ink(State::CurrentSortMouseover).fg;
        pal.alt_sort_bg = ink(State::CurrentSortAlternateRow).bg;
        pal.alt_sort_fg = ink(State::CurrentSortAlternateRow).fg;
        let header = |s: State| inks[slot(Item::Header, s)];
        pal.header = header(State::Normal).bg;
        pal.header_sort = header(State::CurrentSort).bg;
        if !dark {
            // Light Everything draws no edge round the sorted item.
            pal.header_sort_edge = pal.header_sort;
        }
        // (Default)'s colours are also the window's own where no item is
        // drawn: round the search box, between the docks, the search
        // history's list.
        if let Some(bg) = base.bg {
            pal.bg = bg;
            pal.chrome = bg;
            pal.popup_bg = bg;
        }
        if let Some(fg) = base.fg {
            pal.fg = fg;
        }
        let hl = self.look(dark, Item::ResultList, State::Highlighted);
        pal.hl_fg = hl.fg;
        pal.hl_bg = hl.bg;
        pal.hl_bold = hl.bold != Weight::Normal;
        pal
    }
}

/// The theme's colours of every item state, from the palette's sampled
/// colours: what each part of the window drew before Fonts and Colors.
pub fn theme_inks(p: &Palette) -> [Ink; SLOTS] {
    let ink = |fg: u32, bg: u32| Ink { fg, bg };
    let mut out = [ink(p.fg, p.bg); SLOTS];
    for (item, state) in all_slots() {
        let bg = match (item, state) {
            (Item::ResultList, State::Normal | State::Highlighted) => p.list_bg,
            (_, State::Selected) if item == Item::Tabs => {
                // The shown tab: lit in the dark strip, white in the light.
                if p.dark { p.header_sort } else { p.bg }
            }
            (_, State::Selected) => p.sel,
            (_, State::InactiveSelected) => p.sel_inactive,
            (_, State::Mouseover | State::ThumbnailMouseover) => p.hover,
            (_, State::AlternateRow) => p.alt_bg,
            (Item::Header, State::CurrentSort) => p.header_sort,
            (_, State::CurrentSort) => p.sort_bg,
            (_, State::CurrentSortMouseover) => p.hover_sort,
            (_, State::CurrentSortAlternateRow) => p.alt_sort_bg,
            (Item::Header | Item::Pane, _) => p.header,
            (Item::FindBar | Item::Menu | Item::StatusBar | Item::Tabs, _) => p.chrome,
            _ => p.bg,
        };
        out[slot(item, state)] = ink(p.fg, bg);
    }
    out
}

/// A row shade a little off `c` (Everything's Alternate Row): darker on a
/// light colour, lighter on a dark one.
pub fn shade(c: u32) -> u32 {
    let ch = |shift: u32| (c >> shift) & 0xff;
    let (r, g, b) = (ch(0), ch(8), ch(16));
    let light = r * 299 + g * 587 + b * 114 > 128_000;
    let f = |v: u32| {
        if light {
            v.saturating_sub(11)
        } else {
            (v + 15).min(255)
        }
    };
    f(r) | f(g) << 8 | f(b) << 16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::rgb;

    const NO_ACCENT: &dyn Fn() -> u32 = &|| panic!("Windows' accent was not wanted");

    fn look(fg: Option<u32>, bg: Option<u32>, bold: Weight) -> Look {
        Look { fg, bg, bold }
    }

    #[test]
    fn colours_are_everythings_hex() {
        assert_eq!(parse_color("#ff0000"), Some(rgb(0xff, 0, 0)));
        assert_eq!(parse_color(" #0080FF "), Some(rgb(0, 0x80, 0xff)));
        assert_eq!(parse_color("ff0000"), None);
        assert_eq!(parse_color("#ff00"), None);
        assert_eq!(parse_color("#gg0000"), None);
        assert_eq!(parse_color(""), None);
        assert_eq!(format_color(rgb(0x12, 0x34, 0x56)), "#123456");
    }

    #[test]
    fn slots_cover_every_item_state_once() {
        let all: Vec<_> = all_slots().collect();
        assert_eq!(all.len(), SLOTS);
        for (i, &(item, state)) in all.iter().enumerate() {
            assert_eq!(slot(item, state), i);
        }
        // `own` and `bold` are u64 masks.
        const { assert!(SLOTS <= 64) };
    }

    /// Every key is one of Everything 1.5's (from its generated INI), and
    /// none is written twice.
    #[test]
    fn keys_are_everythings_names() {
        let keys: Vec<String> = Colors::default()
            .to_keys()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let mut seen = std::collections::HashSet::new();
        for k in &keys {
            assert!(seen.insert(k.clone()), "{k} twice");
        }
        for k in [
            "result_list_font",
            "search_edit_font_size",
            "accent",
            "dark_accent_color",
            "result_list_accent",
            "treeview_dark_accent",
            "bookmark_sidebar_accent_color",
            "default_normal_foreground_color",
            "dark_default_normal_bold",
            "header_current_sort_background_color",
            "dark_header_normal_foreground_color",
            "statusbar_normal_background_color",
            "search_normal_foreground_color",
            "filterbar_normal_bold",
            "findbar_normal_background_color",
            "tabs_selected_foreground_color",
            "dark_tabs_normal_background_color",
            "treeview_selected_inactive_background_color",
            "filter_sidebar_mouseover_foreground_color",
            "bookmark_sidebar_selected_bold",
            "pane_normal_background_color",
            "preview_normal_foreground_color",
            "menu_normal_background_color",
            "alternate_row_background_color",
            "mouseover_current_sort_foreground_color",
            "dark_current_sort_alternate_row_background_color",
            "thumbnail_mouseover_background_color",
            "selected_inactive_bold",
        ] {
            assert!(seen.contains(k), "{k} missing");
        }
        // 2 fonts x 2, 5 accents x 2 x 2 themes, 34 states x 3 x 2 themes.
        assert_eq!(keys.len(), 4 + 20 + SLOTS * 6);
    }

    #[test]
    fn everythings_keys_read_and_write_back() {
        let pairs = [
            ("result_list_font", "Consolas"),
            ("result_list_font_size", "11"),
            ("search_edit_font_size", "14"),
            ("selected_background_color", "#336699"),
            ("dark_selected_foreground_color", "#ffffff"),
            ("highlighted_bold", "0"),
            ("dark_current_sort_bold", "2"),
            ("normal_foreground_color", "nonsense"),
            ("treeview_selected_background_color", "#102030"),
            ("dark_tabs_selected_foreground_color", "#ff8800"),
            ("statusbar_normal_bold", "2"),
            ("accent", "2"),
            ("accent_color", "#0078d4"),
            ("treeview_dark_accent", "0"),
        ];
        let c = Colors::from_keys(|k| pairs.iter().find(|(p, _)| *p == k).map(|(_, v)| *v));
        assert_eq!(c.result_list_font.face, "Consolas");
        assert_eq!(c.result_list_font.size, 11);
        assert_eq!(
            c.search_edit_font,
            FontSpec {
                face: String::new(),
                size: 14
            }
        );
        let rl = Item::ResultList;
        assert_eq!(
            c.look(false, rl, State::Selected).bg,
            Some(rgb(0x33, 0x66, 0x99))
        );
        assert_eq!(
            c.look(true, rl, State::Selected).fg,
            Some(rgb(0xff, 0xff, 0xff))
        );
        assert_eq!(c.look(true, rl, State::Selected).bg, None);
        assert_eq!(c.look(false, rl, State::Highlighted).bold, Weight::Normal);
        assert_eq!(c.look(true, rl, State::CurrentSort).bold, Weight::Bold);
        assert_eq!(c.look(false, rl, State::Normal).fg, None);
        assert_eq!(
            c.look(false, Item::FoldersSidebar, State::Selected).bg,
            Some(rgb(0x10, 0x20, 0x30))
        );
        assert_eq!(
            c.look(true, Item::Tabs, State::Selected).fg,
            Some(rgb(0xff, 0x88, 0))
        );
        assert_eq!(
            c.look(false, Item::StatusBar, State::Normal).bold,
            Weight::Bold
        );
        assert_eq!(
            c.accent(false, Item::Default),
            Accent {
                mode: AccentMode::Custom,
                color: Some(rgb(0, 0x78, 0xd4))
            }
        );
        assert_eq!(c.accent(true, Item::FoldersSidebar).mode, AccentMode::None);
        assert_eq!(
            c.accent(true, Item::ResultList),
            Accent::default_for(Item::ResultList)
        );
        let out = c.to_keys();
        let get = |k: &str| out.iter().find(|(p, _)| p == k).map(|(_, v)| v.as_str());
        assert_eq!(get("selected_background_color"), Some("#336699"));
        assert_eq!(get("dark_selected_inactive_bold"), Some(""));
        assert_eq!(get("mouseover_foreground_color"), Some(""));
        assert_eq!(get("dark_current_sort_bold"), Some("2"));
        assert_eq!(get("search_edit_font"), Some(""));
        assert_eq!(get("accent"), Some("2"));
        assert_eq!(get("dark_accent"), Some("0"));
        assert_eq!(get("result_list_accent"), Some("1"));
        assert_eq!(get("treeview_dark_accent"), Some("0"));
        assert_eq!(Colors::from_keys(get), c);
        // The defaults write Everything's defaults: no colour, the accents
        // None for (Default) and Default for the others.
        for (k, v) in Colors::default().to_keys() {
            let accent_mode = k.ends_with("accent");
            if accent_mode {
                assert_eq!(
                    v,
                    if k == "accent" || k == "dark_accent" {
                        "0"
                    } else {
                        "1"
                    },
                    "{k}"
                );
            } else {
                assert!(v.is_empty(), "{k}={v}");
            }
        }
    }

    #[test]
    fn a_state_with_a_colour_paints_with_it_the_rest_as_the_theme() {
        let mut c = Colors::default();
        let rl = Item::ResultList;
        c.set_look(
            false,
            rl,
            State::Normal,
            look(
                Some(rgb(1, 2, 3)),
                Some(rgb(250, 250, 250)),
                Weight::Default,
            ),
        );
        c.set_look(
            false,
            rl,
            State::Highlighted,
            look(Some(rgb(200, 0, 0)), None, Weight::Normal),
        );
        c.set_look(
            false,
            rl,
            State::Selected,
            look(None, Some(rgb(0, 0, 200)), Weight::Bold),
        );
        let base = Palette::new(false);
        let p = c.apply(base, NO_ACCENT);
        assert_eq!(p.list_bg, rgb(250, 250, 250));
        assert_eq!(p.alt_bg, shade(rgb(250, 250, 250)));
        assert_eq!(p.list_fg, rgb(1, 2, 3));
        // Selected text follows Normal's; so do the alternate rows'.
        assert_eq!(p.sel_fg, rgb(1, 2, 3));
        assert_eq!(p.alt_fg, rgb(1, 2, 3));
        assert_eq!(p.sel, rgb(0, 0, 200));
        assert_eq!(p.sel_inactive, base.sel_inactive);
        assert_eq!(p.hl_fg, Some(rgb(200, 0, 0)));
        assert!(!p.hl_bold);
        assert!(p.is_bold(rl, State::Selected));
        assert!(!p.is_bold(rl, State::Normal));
        assert!(p.is_own(rl, State::AlternateRow));
        // The sorted cells keep the theme's background, with Normal's text.
        assert_eq!(p.sort_bg, base.sort_bg);
        assert_eq!(p.sort_fg, rgb(1, 2, 3));
        // The other items keep the theme's.
        assert_eq!(p.bg, base.bg);
        assert_eq!(p.fg, base.fg);
        assert_eq!(
            p.ink(Item::StatusBar, State::Normal),
            base.ink(Item::StatusBar, State::Normal)
        );
        // The dark theme has its own keys: none set, nothing changes.
        let d = c.apply(Palette::new(true), NO_ACCENT);
        assert_eq!(d.list_bg, Palette::new(true).list_bg);
        assert!(d.hl_bold);
        assert_eq!(d.bold, 0);
        assert_eq!(d.own, 0);
        assert!(d.inks == Palette::new(true).inks);
    }

    #[test]
    fn the_default_item_is_every_normal_states_fallback() {
        let mut c = Colors::default();
        let red = rgb(0x80, 0, 0);
        let white = rgb(0xff, 0xff, 0xff);
        c.set_look(
            true,
            Item::Default,
            State::Normal,
            look(Some(white), Some(red), Weight::Default),
        );
        let blue = rgb(0, 0, 0x80);
        c.set_look(
            true,
            Item::StatusBar,
            State::Normal,
            look(None, Some(blue), Weight::Default),
        );
        let base = Palette::new(true);
        let p = c.apply(base, NO_ACCENT);
        for item in Item::ALL {
            let n = p.ink(item, State::Normal);
            assert_eq!(n.fg, white, "{item:?}");
            assert_eq!(
                n.bg,
                if item == Item::StatusBar { blue } else { red },
                "{item:?}"
            );
        }
        // Other states keep the theme's background, with Normal's text.
        let sel = p.ink(Item::FoldersSidebar, State::Selected);
        assert_eq!(sel.bg, base.sel);
        assert_eq!(sel.fg, white);
        assert_eq!(p.header, red);
        assert_eq!(p.header_sort, base.header_sort);
    }

    #[test]
    fn mouseover_states_follow_the_mouseover_row() {
        let mut c = Colors::default();
        let rl = Item::ResultList;
        let hot = rgb(0x20, 0x40, 0x60);
        let text = rgb(0xee, 0xdd, 0xcc);
        c.set_look(
            false,
            rl,
            State::Mouseover,
            look(Some(text), Some(hot), Weight::Default),
        );
        let p = c.apply(Palette::new(false), NO_ACCENT);
        assert_eq!(p.hover, hot);
        assert_eq!(p.hover_sort, hot);
        assert_eq!(p.hover_sort_fg, text);
        assert_eq!(
            p.ink(rl, State::ThumbnailMouseover),
            Ink { fg: text, bg: hot }
        );
        // Its own colour wins.
        let thumb = rgb(1, 1, 1);
        c.set_look(
            false,
            rl,
            State::ThumbnailMouseover,
            look(None, Some(thumb), Weight::Default),
        );
        c.set_look(
            false,
            rl,
            State::CurrentSort,
            look(None, Some(rgb(0x40, 0x40, 0x40)), Weight::Default),
        );
        let p = c.apply(Palette::new(false), NO_ACCENT);
        assert_eq!(p.ink(rl, State::ThumbnailMouseover).bg, thumb);
        assert_eq!(p.alt_sort_bg, shade(rgb(0x40, 0x40, 0x40)));
        assert_eq!(p.hover_sort, hot);
    }

    #[test]
    fn accents_tint_selection_and_mouseover() {
        let mut c = Colors::default();
        let accent = rgb(0, 0x78, 0xd4);
        let windows: &dyn Fn() -> u32 = &|| accent;
        // None by default: the theme's colours, Windows not asked.
        let base = Palette::new(false);
        assert!(c.apply(base, NO_ACCENT).inks == base.inks);
        c.set_accent(
            false,
            Item::Default,
            Accent {
                mode: AccentMode::Default,
                color: None,
            },
        );
        let p = c.apply(base, windows);
        let rl = Item::ResultList;
        assert_eq!(p.sel, mix(base.list_bg, accent, ACCENT_SELECTED[0]));
        assert_eq!(p.hover, mix(base.list_bg, accent, ACCENT_MOUSEOVER[0]));
        // The sorted cell under the mouse follows the Mouseover row.
        assert_eq!(p.hover_sort, p.hover);
        assert_eq!(p.sel_inactive, base.sel_inactive);
        let folders = p.ink(Item::FoldersSidebar, State::Selected);
        assert_eq!(folders.bg, p.sel);
        assert!(p.is_own(Item::FoldersSidebar, State::Mouseover));
        // An item's None, an item's Custom.
        c.set_accent(
            false,
            Item::FoldersSidebar,
            Accent {
                mode: AccentMode::None,
                color: None,
            },
        );
        let green = rgb(0, 0x80, 0);
        c.set_accent(
            false,
            rl,
            Accent {
                mode: AccentMode::Custom,
                color: Some(green),
            },
        );
        let p = c.apply(base, windows);
        assert_eq!(p.ink(Item::FoldersSidebar, State::Selected).bg, base.sel);
        assert_eq!(p.sel, mix(base.list_bg, green, ACCENT_SELECTED[0]));
        assert_eq!(
            p.ink(Item::FiltersSidebar, State::Selected).bg,
            mix(base.bg, accent, ACCENT_SELECTED[0])
        );
        // A colour of the state's own beats the accent.
        c.set_look(
            false,
            rl,
            State::Selected,
            look(None, Some(rgb(9, 9, 9)), Weight::Default),
        );
        assert_eq!(c.apply(base, windows).sel, rgb(9, 9, 9));
        // Each set back to its default leaves nothing behind.
        c.set_look(false, rl, State::Selected, Look::default());
        for item in [Item::Default, Item::FoldersSidebar, rl] {
            c.set_accent(false, item, Accent::default_for(item));
        }
        assert_eq!(c, Colors::default());
    }

    #[test]
    fn only_drawn_styles_count() {
        let mut c = Colors::default();
        c.set_look(
            false,
            Item::Menu,
            State::Normal,
            look(None, None, Weight::Bold),
        );
        c.set_look(
            false,
            Item::Tabs,
            State::Selected,
            look(None, None, Weight::Bold),
        );
        let p = c.apply(Palette::new(false), NO_ACCENT);
        assert!(!p.is_bold(Item::Menu, State::Normal));
        assert!(p.is_bold(Item::Tabs, State::Selected));
    }

    #[test]
    fn shades_move_away_from_the_end_they_are_near() {
        assert!(shade(rgb(255, 255, 255)) < rgb(255, 255, 255));
        assert_eq!(shade(rgb(0, 0, 0)), rgb(15, 15, 15));
        assert_eq!(mix(rgb(0, 0, 0), rgb(200, 100, 50), 0.5), rgb(100, 50, 25));
    }
}
