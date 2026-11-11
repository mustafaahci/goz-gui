//! Options > Fonts and Colors: the result list's and the search box's
//! fonts, and the result list's colours for each item state, in the light
//! and the dark theme, under Everything 1.5's INI keys.
//!
//! Everything keeps an item state's look as `<state>_foreground_color`,
//! `<state>_background_color` (`#RRGGBB`, empty for the theme's own) and
//! `<state>_bold` (empty or 1 the default style, 0 normal, 2 bold), the
//! dark theme's under the same names with `dark_` in front; the fonts as
//! `result_list_font` / `search_edit_font` (a face name, empty for the
//! default) and `..._font_size` (points, empty for the default).

use crate::theme::Palette;

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
    /// The item's own: matches bold, other text normal.
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

/// The result list's item states goz-gui draws, with Everything's names
/// and key stems.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Normal,
    Highlighted,
    Selected,
    InactiveSelected,
    Mouseover,
    CurrentSort,
}

impl State {
    pub const ALL: [State; 6] = [
        State::Normal,
        State::Highlighted,
        State::Selected,
        State::InactiveSelected,
        State::Mouseover,
        State::CurrentSort,
    ];

    /// Everything's name in the Item states list.
    pub fn label(self) -> &'static str {
        match self {
            State::Normal => "Normal",
            State::Highlighted => "Normal (Highlighted)",
            State::Selected => "Selected",
            State::InactiveSelected => "Inactive Selected",
            State::Mouseover => "Mouseover Row",
            State::CurrentSort => "Current Sort",
        }
    }

    fn stem(self) -> &'static str {
        match self {
            State::Normal => "normal",
            State::Highlighted => "highlighted",
            State::Selected => "selected",
            State::InactiveSelected => "selected_inactive",
            State::Mouseover => "mouseover",
            State::CurrentSort => "current_sort",
        }
    }

    fn index(self) -> usize {
        State::ALL.iter().position(|&s| s == self).unwrap_or(0)
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

/// The fonts and the result list's colours.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Colors {
    pub result_list_font: FontSpec,
    pub search_edit_font: FontSpec,
    pub light: [Look; 6],
    pub dark: [Look; 6],
}

fn key(dark: bool, state: State, part: &str) -> String {
    format!("{}{}_{part}", if dark { "dark_" } else { "" }, state.stem())
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

impl Colors {
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Colors {
        let looks = |dark: bool| {
            State::ALL.map(|s| Look {
                fg: get(&key(dark, s, "foreground_color")).and_then(parse_color),
                bg: get(&key(dark, s, "background_color")).and_then(parse_color),
                bold: get(&key(dark, s, "bold")).map_or(Weight::Default, Weight::from_value),
            })
        };
        Colors {
            result_list_font: font_from(&get, "result_list"),
            search_edit_font: font_from(&get, "search_edit"),
            light: looks(false),
            dark: looks(true),
        }
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
            let looks = if dark { &self.dark } else { &self.light };
            for s in State::ALL {
                let l = looks[s.index()];
                let c = |c: Option<u32>| c.map(format_color).unwrap_or_default();
                out.push((key(dark, s, "foreground_color"), c(l.fg)));
                out.push((key(dark, s, "background_color"), c(l.bg)));
                out.push((key(dark, s, "bold"), l.bold.value().to_owned()));
            }
        }
        out
    }

    pub fn look(&self, dark: bool, state: State) -> Look {
        (if dark { &self.dark } else { &self.light })[state.index()]
    }

    pub fn set_look(&mut self, dark: bool, state: State, look: Look) {
        (if dark {
            &mut self.dark
        } else {
            &mut self.light
        })[state.index()] = look;
    }

    /// `pal` with the result list's colours of its theme put in.
    pub fn apply(&self, mut pal: Palette) -> Palette {
        let look = |s| self.look(pal.dark, s);
        let normal = look(State::Normal);
        if let Some(bg) = normal.bg {
            pal.list_bg = bg;
            pal.alt_bg = shade(bg);
        }
        if let Some(fg) = normal.fg {
            pal.list_fg = fg;
        }
        // Each state's text is the normal text unless it has its own.
        let fg_of = |s: State, pal: &Palette| look(s).fg.unwrap_or(pal.list_fg);
        pal.sel_fg = fg_of(State::Selected, &pal);
        pal.sel_inactive_fg = fg_of(State::InactiveSelected, &pal);
        pal.hover_fg = fg_of(State::Mouseover, &pal);
        pal.sort_fg = fg_of(State::CurrentSort, &pal);
        if let Some(bg) = look(State::Selected).bg {
            pal.sel = bg;
        }
        if let Some(bg) = look(State::InactiveSelected).bg {
            pal.sel_inactive = bg;
        }
        if let Some(bg) = look(State::Mouseover).bg {
            pal.hover = bg;
            pal.hover_sort = bg;
        }
        if let Some(bg) = look(State::CurrentSort).bg {
            pal.sort_bg = bg;
            pal.alt_sort_bg = shade(bg);
        }
        let hl = look(State::Highlighted);
        pal.hl_fg = hl.fg;
        pal.hl_bg = hl.bg;
        pal.hl_bold = hl.bold != Weight::Normal;
        pal.bold_rows = [
            State::Normal,
            State::Selected,
            State::InactiveSelected,
            State::Mouseover,
            State::CurrentSort,
        ]
        .iter()
        .enumerate()
        .filter(|&(_, &s)| look(s).bold == Weight::Bold)
        .fold(0u8, |m, (i, _)| m | 1 << i);
        pal
    }
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

/// The bit of `bold_rows` for each row state (`Palette::bold_rows`).
pub const BOLD_NORMAL: u8 = 1;
pub const BOLD_SELECTED: u8 = 2;
pub const BOLD_INACTIVE: u8 = 4;
pub const BOLD_MOUSEOVER: u8 = 8;
pub const BOLD_SORT: u8 = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::rgb;

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
        assert_eq!(
            c.look(false, State::Selected).bg,
            Some(rgb(0x33, 0x66, 0x99))
        );
        assert_eq!(
            c.look(true, State::Selected).fg,
            Some(rgb(0xff, 0xff, 0xff))
        );
        assert_eq!(c.look(true, State::Selected).bg, None);
        assert_eq!(c.look(false, State::Highlighted).bold, Weight::Normal);
        assert_eq!(c.look(true, State::CurrentSort).bold, Weight::Bold);
        assert_eq!(c.look(false, State::Normal).fg, None);
        let out = c.to_keys();
        let get = |k: &str| out.iter().find(|(p, _)| p == k).map(|(_, v)| v.as_str());
        assert_eq!(get("selected_background_color"), Some("#336699"));
        assert_eq!(get("dark_selected_inactive_bold"), Some(""));
        assert_eq!(get("mouseover_foreground_color"), Some(""));
        assert_eq!(get("dark_current_sort_bold"), Some("2"));
        assert_eq!(get("search_edit_font"), Some(""));
        assert_eq!(Colors::from_keys(get), c);
        // Every key once, 6 states x 3 parts x 2 themes and 4 font keys.
        assert_eq!(out.len(), 6 * 3 * 2 + 4);
        assert!(
            Colors::default()
                .to_keys()
                .iter()
                .all(|(_, v)| v.is_empty())
        );
    }

    #[test]
    fn a_state_with_a_colour_paints_with_it_the_rest_as_the_theme() {
        let mut c = Colors::default();
        c.set_look(
            false,
            State::Normal,
            Look {
                fg: Some(rgb(1, 2, 3)),
                bg: Some(rgb(250, 250, 250)),
                bold: Weight::Default,
            },
        );
        c.set_look(
            false,
            State::Highlighted,
            Look {
                fg: Some(rgb(200, 0, 0)),
                bg: None,
                bold: Weight::Normal,
            },
        );
        c.set_look(
            false,
            State::Selected,
            Look {
                fg: None,
                bg: Some(rgb(0, 0, 200)),
                bold: Weight::Bold,
            },
        );
        let base = Palette::new(false);
        let p = c.apply(base);
        assert_eq!(p.list_bg, rgb(250, 250, 250));
        assert_ne!(p.alt_bg, p.list_bg);
        assert_eq!(p.list_fg, rgb(1, 2, 3));
        // Selected text follows Normal's.
        assert_eq!(p.sel_fg, rgb(1, 2, 3));
        assert_eq!(p.sel, rgb(0, 0, 200));
        assert_eq!(p.sel_inactive, base.sel_inactive);
        assert_eq!(p.hl_fg, Some(rgb(200, 0, 0)));
        assert!(!p.hl_bold);
        assert_eq!(p.bold_rows, BOLD_SELECTED);
        // The search box and the panes keep the theme's.
        assert_eq!(p.bg, base.bg);
        assert_eq!(p.fg, base.fg);
        // The dark theme has its own keys: none set, nothing changes.
        let d = c.apply(Palette::new(true));
        assert_eq!(d.list_bg, Palette::new(true).list_bg);
        assert!(d.hl_bold);
        assert_eq!(d.bold_rows, 0);
    }

    #[test]
    fn shades_move_away_from_the_end_they_are_near() {
        assert!(shade(rgb(255, 255, 255)) < rgb(255, 255, 255));
        assert_eq!(shade(rgb(0, 0, 0)), rgb(15, 15, 15));
    }
}
