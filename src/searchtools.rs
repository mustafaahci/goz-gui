//! The search tools' settings and their pure parts: the find bar's
//! options, fullscreen, View > Window Size's sizes, Edit > Go To's number,
//! and what the search box's menu puts in the search. The window's side is
//! `window::findbar`, `window::tools`, `window::filters` and
//! `window::advsearch`.

use crate::menu::F4_FIRST;

// The search tools' commands, from F4's range (`menu::F4_FIRST`).
/// Edit > Find... (Ctrl+F).
pub const FIND: u16 = F4_FIRST;
/// Edit > Find Next (F3), and Find Previous (Shift+F3, no menu item).
pub const FIND_NEXT: u16 = F4_FIRST + 1;
pub const FIND_PREVIOUS: u16 = F4_FIRST + 2;
/// Edit > Go To... (Ctrl+G).
pub const GO_TO: u16 = F4_FIRST + 3;
/// Edit > Invert Selection.
pub const INVERT_SELECTION: u16 = F4_FIRST + 4;
/// Search > Advanced Search... (F2 in the search box).
pub const ADVANCED_SEARCH: u16 = F4_FIRST + 5;
/// Search > Add to Filters... and Organize Filters... (Ctrl+Shift+K).
pub const ADD_TO_FILTERS: u16 = F4_FIRST + 6;
pub const ORGANIZE_FILTERS: u16 = F4_FIRST + 7;
/// View | Fullscreen (F11, no menu item, as in Everything).
pub const FULLSCREEN: u16 = F4_FIRST + 8;
/// View > Window Size > Small, Medium, Large (Alt+1, 2, 3) and Auto Fit
/// (Alt+4).
pub const WINDOW_SMALL: u16 = F4_FIRST + 10;
pub const WINDOW_MEDIUM: u16 = F4_FIRST + 11;
pub const WINDOW_LARGE: u16 = F4_FIRST + 12;
pub const WINDOW_AUTO_FIT: u16 = F4_FIRST + 13;
// The search box's menu (its items run there, not through the window's
// commands).
pub const BOX_UNDO: u16 = F4_FIRST + 20;
pub const BOX_CUT: u16 = F4_FIRST + 21;
pub const BOX_COPY: u16 = F4_FIRST + 22;
pub const BOX_PASTE: u16 = F4_FIRST + 23;
pub const BOX_DELETE: u16 = F4_FIRST + 24;
pub const BOX_SELECT_ALL: u16 = F4_FIRST + 25;
pub const BOX_AND: u16 = F4_FIRST + 30;
pub const BOX_QUOTE: u16 = F4_FIRST + 31;
pub const BOX_CASE: u16 = F4_FIRST + 32;
pub const BOX_PATH: u16 = F4_FIRST + 33;
pub const BOX_FILES: u16 = F4_FIRST + 34;
pub const BOX_FOLDERS: u16 = F4_FIRST + 35;
/// Search Operator's OR, NOT and Group: only with a gozd that reads them
/// (`client::boolean_syntax`).
pub const BOX_OR: u16 = F4_FIRST + 36;
pub const BOX_NOT: u16 = F4_FIRST + 37;
pub const BOX_GROUP: u16 = F4_FIRST + 38;
pub const BOX_LOCATED_IN: u16 = F4_FIRST + 40;
pub const BOX_DM_BEFORE: u16 = F4_FIRST + 41;
pub const BOX_DM_AFTER: u16 = F4_FIRST + 42;
pub const BOX_DM_TODAY: u16 = F4_FIRST + 43;
pub const BOX_SIZE_SMALLER: u16 = F4_FIRST + 44;
pub const BOX_SIZE_LARGER: u16 = F4_FIRST + 45;
pub const BOX_EXT: u16 = F4_FIRST + 46;

/// What a syntax item of the search box's menu puts in the search, and how.
pub fn box_syntax(id: u16) -> Option<(&'static str, Insert)> {
    Some(match id {
        BOX_AND => ("", Insert::Prefix),
        BOX_QUOTE => ("", Insert::Quote),
        BOX_OR => ("|", Insert::Prefix),
        BOX_NOT => ("!", Insert::Prefix),
        BOX_GROUP => ("", Insert::Group),
        BOX_CASE => ("case:", Insert::Term),
        BOX_PATH => ("path:", Insert::Prefix),
        BOX_FILES => ("file:", Insert::Prefix),
        BOX_FOLDERS => ("folder:", Insert::Prefix),
        BOX_DM_BEFORE => ("dm:<", Insert::Prefix),
        BOX_DM_AFTER => ("dm:>", Insert::Prefix),
        BOX_DM_TODAY => ("dm:today", Insert::Term),
        BOX_SIZE_SMALLER => ("size:<", Insert::Prefix),
        BOX_SIZE_LARGER => ("size:>", Insert::Prefix),
        BOX_EXT => ("ext:", Insert::Prefix),
        _ => return None,
    })
}

/// The search tools' settings, under Everything's keys and with its
/// defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTools {
    /// `findbar_visible`: Edit > Find... left the find bar open.
    pub findbar_visible: bool,
    /// `findbar_highlight_all`: the matches are lit in the rows on screen.
    pub findbar_highlight_all: bool,
    /// `findbar_match_case`.
    pub findbar_match_case: bool,
    /// `fullscreen`: the window was left fullscreen (F11).
    pub fullscreen: bool,
    /// `fullscreen_menu`: the menu bar stays while fullscreen.
    pub fullscreen_menu: bool,
    /// `auto_size_1`, `_2`, `_3`: View > Window Size > Small, Medium and
    /// Large, the window's width at 96 DPI.
    pub auto_size: [i32; 3],
    /// `auto_size_aspect_ratio_x` and `_y`: their height is the width times
    /// y over x.
    pub aspect: (i32, i32),
    /// `search_edit_last_located_in`: the folder the search box's Located
    /// in... and Advanced Search's Browse... last chose.
    pub last_located_in: String,
}

impl Default for SearchTools {
    fn default() -> SearchTools {
        SearchTools {
            findbar_visible: false,
            findbar_highlight_all: true,
            findbar_match_case: false,
            fullscreen: false,
            fullscreen_menu: false,
            auto_size: [512, 640, 768],
            aspect: (9, 7),
            last_located_in: String::new(),
        }
    }
}

fn number(v: Option<&str>) -> Option<i64> {
    v?.trim().parse().ok()
}

fn flag(v: Option<&str>) -> Option<bool> {
    number(v).map(|n| n != 0)
}

impl SearchTools {
    /// The settings in the settings file's keys; what is missing or cannot
    /// be read is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> SearchTools {
        let d = SearchTools::default();
        let size = |key: &str, default: i32| {
            number(get(key))
                .filter(|v| (64..=20_000).contains(v))
                .map_or(default, |v| v as i32)
        };
        let ratio = |key: &str, default: i32| {
            number(get(key))
                .filter(|v| (1..=1000).contains(v))
                .map_or(default, |v| v as i32)
        };
        SearchTools {
            findbar_visible: flag(get("findbar_visible")).unwrap_or(d.findbar_visible),
            findbar_highlight_all: flag(get("findbar_highlight_all"))
                .unwrap_or(d.findbar_highlight_all),
            findbar_match_case: flag(get("findbar_match_case")).unwrap_or(d.findbar_match_case),
            fullscreen: flag(get("fullscreen")).unwrap_or(d.fullscreen),
            fullscreen_menu: flag(get("fullscreen_menu")).unwrap_or(d.fullscreen_menu),
            auto_size: [
                size("auto_size_1", d.auto_size[0]),
                size("auto_size_2", d.auto_size[1]),
                size("auto_size_3", d.auto_size[2]),
            ],
            aspect: (
                ratio("auto_size_aspect_ratio_x", d.aspect.0),
                ratio("auto_size_aspect_ratio_y", d.aspect.1),
            ),
            last_located_in: get("search_edit_last_located_in")
                .unwrap_or_default()
                .to_owned(),
        }
    }

    /// Every key, with its value.
    pub fn to_keys(&self) -> Vec<(String, String)> {
        let bit = |b: bool| u8::from(b).to_string();
        vec![
            ("findbar_visible".into(), bit(self.findbar_visible)),
            (
                "findbar_highlight_all".into(),
                bit(self.findbar_highlight_all),
            ),
            ("findbar_match_case".into(), bit(self.findbar_match_case)),
            ("fullscreen".into(), bit(self.fullscreen)),
            ("fullscreen_menu".into(), bit(self.fullscreen_menu)),
            ("auto_size_1".into(), self.auto_size[0].to_string()),
            ("auto_size_2".into(), self.auto_size[1].to_string()),
            ("auto_size_3".into(), self.auto_size[2].to_string()),
            ("auto_size_aspect_ratio_x".into(), self.aspect.0.to_string()),
            ("auto_size_aspect_ratio_y".into(), self.aspect.1.to_string()),
            (
                "search_edit_last_located_in".into(),
                self.last_located_in.clone(),
            ),
        ]
    }

    /// View > Window Size `which` (0 Small, 1 Medium, 2 Large): the
    /// window's width and height at `dpi`.
    pub fn preset(&self, which: usize, dpi: u32) -> (i32, i32) {
        let width = self.auto_size[which.min(2)];
        let height = aspect_height(width, self.aspect);
        (scale(width, dpi), scale(height, dpi))
    }
}

/// `v` pixels at 96 DPI, at `dpi`.
fn scale(v: i32, dpi: u32) -> i32 {
    (i64::from(v) * i64::from(dpi) / 96) as i32
}

/// The height of a window `width` wide with Everything's aspect ratio
/// (`auto_size_aspect_ratio_x` : `_y`), rounded down: 512 → 398.
pub fn aspect_height(width: i32, (x, y): (i32, i32)) -> i32 {
    (i64::from(width) * i64::from(y.max(1)) / i64::from(x.max(1))) as i32
}

/// A rectangle `(left, top, right, bottom)`.
pub type Rect = (i32, i32, i32, i32);

/// Where a window at `window` goes when it takes `size`: its top left
/// corner stays, moved back inside `work` (the work area of its monitor)
/// as far as it can be, and the size no larger than `work`.
pub fn sized_within(window: Rect, size: (i32, i32), work: Rect) -> Rect {
    let w = size.0.min(work.2 - work.0).max(1);
    let h = size.1.min(work.3 - work.1).max(1);
    let left = window.0.min(work.2 - w).max(work.0);
    let top = window.1.min(work.3 - h).max(work.1);
    (left, top, left + w, top + h)
}

/// View > Window Size > Auto Fit: the window's size for its columns as
/// sized to their text. `columns` is their width, `chrome` what the window
/// has round the list (its frame, the docks, the scroll bar) as (width,
/// height), `rows` the result count and `row` a row's height; the height
/// keeps Everything's aspect ratio unless fewer rows need less, and is at
/// least `min`.
pub fn auto_fit(
    columns: i32,
    chrome: (i32, i32),
    rows: u64,
    row: i32,
    aspect: (i32, i32),
    min: (i32, i32),
) -> (i32, i32) {
    let width = (columns + chrome.0).max(min.0);
    let by_aspect = aspect_height(width, aspect);
    let needed = i64::from(chrome.1) + i64::from(row.max(1)) * rows.min(100_000) as i64;
    let height = (by_aspect as i64).min(needed).max(i64::from(min.1)) as i32;
    (width, height)
}

/// Edit > Go To's result number as typed: digits, perhaps grouped
/// (`1,234`, `1 234`), from 1 to `count`.
pub fn result_number(text: &str, count: u64) -> Option<u64> {
    let digits: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '.' | ' ' | '\u{a0}' | '\u{202f}' | '\''))
        .collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    (1..=count).contains(&n).then_some(n)
}

/// How the search box's menu puts a piece of syntax in the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insert {
    /// A function or modifier its value follows (`ext:`, `size:>`,
    /// `path:`): the selection, if any, becomes its value; the caret after.
    Prefix,
    /// A term of its own (`case:`, `dm:today`, a folder): before the
    /// selection, a space after it.
    Term,
    /// Quotes round the selection, or the caret between two.
    Quote,
    /// `< >` round the selection, or the caret between them.
    Group,
}

/// `text` (UTF-16) with `piece` put in at the selection `start..end` as
/// `how` says, a space before it unless the text has one there (or it
/// starts the text). The new text and the new selection (both ends).
pub fn insert(
    text: &[u16],
    (start, end): (usize, usize),
    piece: &str,
    how: Insert,
) -> (Vec<u16>, (usize, usize)) {
    let (start, end) = (start.min(text.len()), end.min(text.len()));
    let (start, end) = (start.min(end), start.max(end));
    let space = u16::from(b' ');
    let before_space = start == 0 || text[start - 1] == space;
    let piece: Vec<u16> = piece.encode_utf16().collect();
    let selected = &text[start..end];
    let mut mid: Vec<u16> = Vec::new();
    if !before_space {
        mid.push(space);
    }
    let caret;
    match how {
        Insert::Prefix => {
            mid.extend(&piece);
            mid.extend(selected);
            caret = mid.len();
        }
        Insert::Term => {
            mid.extend(&piece);
            mid.push(space);
            caret = mid.len();
            mid.extend(selected);
        }
        Insert::Quote | Insert::Group => {
            let (open, close) = if how == Insert::Quote {
                (b'"', b'"')
            } else {
                (b'<', b'>')
            };
            mid.push(u16::from(open));
            mid.extend(selected);
            caret = mid.len();
            mid.push(u16::from(close));
        }
    }
    let mut out = Vec::with_capacity(text.len() + mid.len());
    out.extend(&text[..start]);
    out.extend(&mid);
    out.extend(&text[end..]);
    let at = start + caret;
    (out, (at, at))
}

/// A folder as a search term: its path with a separator at the end, so it
/// matches only what is in it (`C:\foo\`, not `C:\foobar`), quoted when it
/// holds a space.
pub fn folder_term(path: &str) -> String {
    let mut p = path.trim().replace('/', "\\");
    if !p.ends_with('\\') {
        p.push('\\');
    }
    if p.contains(char::is_whitespace) {
        format!("\"{p}\"")
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn settings_round_trip_with_everythings_defaults() {
        let d = SearchTools::from_keys(|_| None);
        assert_eq!(d, SearchTools::default());
        assert!(d.findbar_highlight_all && !d.findbar_visible && !d.fullscreen_menu);
        let mut s = d.clone();
        s.findbar_visible = true;
        s.findbar_match_case = true;
        s.fullscreen = true;
        s.auto_size = [500, 600, 700];
        s.aspect = (16, 9);
        s.last_located_in = r"C:\Users".into();
        let pairs = s.to_keys();
        let map: HashMap<String, String> = pairs.into_iter().collect();
        assert_eq!(
            SearchTools::from_keys(|k| map.get(k).map(String::as_str)),
            s
        );
        // Out of range: the default.
        let bad: HashMap<&str, &str> = [("auto_size_1", "5"), ("auto_size_aspect_ratio_x", "0")]
            .into_iter()
            .collect();
        let read = SearchTools::from_keys(|k| bad.get(k).copied());
        assert_eq!((read.auto_size[0], read.aspect.0), (512, 9));
    }

    #[test]
    fn window_sizes_are_everythings() {
        let s = SearchTools::default();
        assert_eq!(s.preset(0, 96), (512, 398));
        assert_eq!(s.preset(1, 96), (640, 497));
        assert_eq!(s.preset(2, 96), (768, 597));
        assert_eq!(s.preset(0, 144), (768, 597));
        // Moved back on screen, no larger than it.
        assert_eq!(
            sized_within((1800, 900, 1900, 1000), (512, 398), (0, 0, 1920, 1040)),
            (1408, 642, 1920, 1040)
        );
        assert_eq!(
            sized_within((10, 10, 20, 20), (5000, 5000), (0, 0, 1920, 1040)),
            (0, 0, 1920, 1040)
        );
        assert_eq!(
            sized_within((-500, -20, 0, 0), (512, 398), (-1920, 0, 0, 1040)),
            (-512, 0, 0, 398)
        );
    }

    #[test]
    fn auto_fit_follows_the_columns_and_the_rows() {
        // Many rows: the width's aspect height.
        assert_eq!(
            auto_fit(900, (40, 120), 1_000_000, 19, (9, 7), (420, 240)),
            (940, 731)
        );
        // Three rows: just room for them.
        assert_eq!(
            auto_fit(900, (40, 120), 3, 19, (9, 7), (420, 240)),
            (940, 240)
        );
        assert_eq!(
            auto_fit(900, (40, 120), 10, 19, (9, 7), (420, 240)),
            (940, 310)
        );
        // Never smaller than the window may be.
        assert_eq!(
            auto_fit(100, (40, 120), 0, 19, (9, 7), (420, 240)),
            (420, 240)
        );
    }

    #[test]
    fn result_numbers() {
        assert_eq!(result_number("1", 10), Some(1));
        assert_eq!(result_number(" 10 ", 10), Some(10));
        assert_eq!(result_number("1,234", 5000), Some(1234));
        assert_eq!(result_number("1 234", 5000), Some(1234));
        assert_eq!(result_number("0", 10), None);
        assert_eq!(result_number("11", 10), None);
        assert_eq!(result_number("", 10), None);
        assert_eq!(result_number("-1", 10), None);
        assert_eq!(result_number("1e3", 10_000), None);
        assert_eq!(result_number("5", 0), None);
    }

    fn ins(text: &str, sel: (usize, usize), piece: &str, how: Insert) -> (String, (usize, usize)) {
        let t: Vec<u16> = text.encode_utf16().collect();
        let (out, sel) = insert(&t, sel, piece, how);
        (String::from_utf16_lossy(&out), sel)
    }

    #[test]
    fn syntax_goes_in_at_the_caret() {
        // A prefix at the end of a word: a space first, the caret after.
        assert_eq!(
            ins("report", (6, 6), "ext:", Insert::Prefix),
            ("report ext:".into(), (11, 11))
        );
        // Over a selection: the selection is its value.
        assert_eq!(
            ins("a pdf", (2, 5), "ext:", Insert::Prefix),
            ("a ext:pdf".into(), (9, 9))
        );
        // At the start: no space.
        assert_eq!(
            ins("", (0, 0), "size:>", Insert::Prefix),
            ("size:>".into(), (6, 6))
        );
        // A term: a space after it, before the selection.
        assert_eq!(
            ins("abc", (0, 0), "case:", Insert::Term),
            ("case: abc".into(), (6, 6))
        );
        assert_eq!(
            ins("abc ", (4, 4), "dm:today", Insert::Term),
            ("abc dm:today ".into(), (13, 13))
        );
        // Quotes round the selection, or the caret between two.
        assert_eq!(
            ins("my file", (0, 7), "", Insert::Quote),
            ("\"my file\"".into(), (8, 8))
        );
        assert_eq!(
            ins("x", (1, 1), "", Insert::Quote),
            ("x \"\"".into(), (3, 3))
        );
        // A selection given backwards, or past the end.
        assert_eq!(
            ins("ab", (9, 1), "path:", Insert::Prefix),
            ("a path:b".into(), (8, 8))
        );
    }

    #[test]
    fn the_menus_syntax_is_what_goz_accepts() {
        use goz_core::query::{LocalClock, parse_query_at};
        struct Clock;
        impl LocalClock for Clock {
            fn now(&self) -> i64 {
                133_801_632_000_000_000
            }
            fn to_local(&self, utc: i64) -> Option<i64> {
                Some(utc)
            }
            fn to_utc(&self, local: i64) -> Option<i64> {
                Some(local)
            }
            fn week_start(&self) -> u8 {
                0
            }
            fn date_order(&self) -> goz_core::query::DateOrder {
                goz_core::query::DateOrder::MonthDayYear
            }
        }
        // Each item at the end of a search, then a value typed after a
        // prefix: the search parses.
        let cases = [
            (BOX_AND, "x"),
            (BOX_QUOTE, ""),
            (BOX_CASE, ""),
            (BOX_PATH, "docs"),
            (BOX_FILES, ""),
            (BOX_FOLDERS, ""),
            (BOX_DM_BEFORE, "2024-01-01"),
            (BOX_DM_AFTER, "lastweek"),
            (BOX_DM_TODAY, ""),
            (BOX_SIZE_SMALLER, "10mb"),
            (BOX_SIZE_LARGER, "1kb"),
            (BOX_EXT, "pdf"),
        ];
        for (id, value) in cases {
            let (piece, how) = box_syntax(id).expect("an item");
            let text: Vec<u16> = "report".encode_utf16().collect();
            let (out, (caret, _)) = insert(&text, (6, 6), piece, how);
            let mut s = String::from_utf16_lossy(&out[..caret]);
            s.push_str(value);
            s.push_str(&String::from_utf16_lossy(&out[caret..]));
            assert!(
                parse_query_at(&s, false, &Clock).is_ok(),
                "{id}: {s} does not parse"
            );
        }
        assert_eq!(box_syntax(BOX_UNDO), None);
    }

    /// OR, NOT and Group write the new query language's shapes (goz-core
    /// v0.6.0 cannot parse them, so the shapes are what is tested): `|`
    /// between terms (spaces round it are ignored), `!` right before its
    /// term, `< >` round the selection.
    #[test]
    fn the_new_operators_shapes() {
        let typed = |id, text: &str, sel: (usize, usize), value: &str| {
            let (piece, how) = box_syntax(id).expect("an item");
            let text: Vec<u16> = text.encode_utf16().collect();
            let (out, (caret, _)) = insert(&text, sel, piece, how);
            let mut s = String::from_utf16_lossy(&out[..caret]);
            s.push_str(value);
            s.push_str(&String::from_utf16_lossy(&out[caret..]));
            s
        };
        assert_eq!(
            typed(BOX_OR, "report", (6, 6), "invoice"),
            "report |invoice"
        );
        assert_eq!(typed(BOX_NOT, "report", (6, 6), "draft"), "report !draft");
        assert_eq!(typed(BOX_NOT, "report draft", (7, 12), ""), "report !draft");
        assert_eq!(typed(BOX_GROUP, "a b|c", (0, 3), ""), "<a b>|c");
        assert_eq!(typed(BOX_GROUP, "x", (1, 1), "a|b"), "x <a|b>");
    }

    #[test]
    fn folders_as_terms() {
        assert_eq!(folder_term(r"C:\Windows"), r"C:\Windows\");
        assert_eq!(folder_term(r"C:\"), r"C:\");
        assert_eq!(folder_term(r"C:\Program Files"), r#""C:\Program Files\""#);
        assert_eq!(folder_term("D:/x/y"), r"D:\x\y\");
    }
}
