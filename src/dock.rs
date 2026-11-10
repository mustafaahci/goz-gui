//! The docks either side of the results and the panes in them, as Everything
//! 1.5 has them (`pane-system`): a pane has a title row with its name and a
//! close button, and sits in the left or the right dock; the panes of a dock
//! stack top to bottom by their order, with a 6 px divider between them that
//! drags, and a 6 px divider between the dock and the results that drags the
//! dock's width. The first pane of a dock takes the height the others leave.
//!
//! This is the arithmetic and the settings, kept under Everything's INI keys
//! (`left_sidebar_wide`, `right_sidebar_wide`, `<pane>_visible`, `_side`,
//! `_order`, `_high`, `_tab_stop`, `pane_title`, `pane_min_wide`); widths and
//! heights at 96 DPI, as the columns' are. The panes' windows are
//! `crate::pane`'s; the window's side is `window/docks.rs`.

use windows_sys::Win32::Foundation::RECT;

use crate::menu;

/// Everything's `left_sidebar_wide`, `right_sidebar_wide` and `<pane>_high`
/// default, at 96 DPI.
pub const DEFAULT_WIDE: i32 = 256;
pub const DEFAULT_HIGH: i32 = 256;
/// Everything's `pane_min_wide`: no dock is narrower.
pub const PANE_MIN_WIDE: i32 = 32;
/// The dividers' thickness at 96 DPI (`vdivider_wide`, `hdivider_high`;
/// 6 px measured on Everything's window).
pub const DIVIDER: i32 = 6;
/// The results keep at least this much width (96 DPI) while a dock is
/// dragged wider or the window narrows.
pub const MIN_CENTER: i32 = 64;
/// A pane's least height below its title row, unless it asks for more.
pub const MIN_HIGH: i32 = 32;
/// The widest or tallest a kept width or height is read as.
const MAX_SIZE: i64 = 100_000;

/// Which dock a pane is in (`<pane>_side`: 0 left, 1 right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn number(self) -> u8 {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }

    fn from_number(n: i64) -> Option<Side> {
        match n {
            0 => Some(Side::Left),
            1 => Some(Side::Right),
            _ => None,
        }
    }
}

/// The panes goz-gui can have. The first four are Everything's, whose
/// layout the settings file keeps under its keys; the test panes
/// (`GOZ_GUI_TEST_PANES`) are kept nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneId {
    Folders,
    Filters,
    Bookmarks,
    Preview,
    /// A test pane, 0 to 3.
    Test(u8),
}

impl PaneId {
    /// Everything's panes, whose layout is kept.
    pub const KNOWN: [PaneId; 4] = [
        PaneId::Folders,
        PaneId::Filters,
        PaneId::Bookmarks,
        PaneId::Preview,
    ];

    /// What its settings keys start with (`preview_visible`, ...); none
    /// for a test pane.
    pub fn key(self) -> Option<&'static str> {
        match self {
            PaneId::Folders => Some("folders_sidebar"),
            PaneId::Filters => Some("filters_sidebar"),
            PaneId::Bookmarks => Some("bookmarks_sidebar"),
            PaneId::Preview => Some("preview"),
            PaneId::Test(_) => None,
        }
    }

    /// Its name, as its title row shows it (Everything's).
    pub fn title(self) -> &'static str {
        match self {
            PaneId::Folders => "Folders",
            PaneId::Filters => "Filters",
            PaneId::Bookmarks => "Bookmarks",
            PaneId::Preview => "Preview",
            PaneId::Test(0) => "Test Pane 1",
            PaneId::Test(1) => "Test Pane 2",
            PaneId::Test(2) => "Test Pane 3",
            PaneId::Test(_) => "Test Pane 4",
        }
    }

    /// Everything's defaults: hidden, the sidebars left and the preview
    /// right, order 0, 256 px high; only Folders is a tab stop.
    pub fn default_layout(self) -> PaneLayout {
        PaneLayout {
            visible: false,
            side: if self == PaneId::Preview {
                Side::Right
            } else {
                Side::Left
            },
            order: 0,
            high: DEFAULT_HIGH,
            tab_stop: self == PaneId::Folders,
        }
    }

    /// Where it goes among panes of the same order: Everything stacks
    /// Folders, Filters, Bookmarks.
    pub fn rank(self) -> u32 {
        match self {
            PaneId::Folders => 0,
            PaneId::Filters => 1,
            PaneId::Bookmarks => 2,
            PaneId::Preview => 3,
            PaneId::Test(n) => 10 + u32::from(n),
        }
    }

    /// Its View menu item, Everything's command label: the command id
    /// (`menu::VIEW_PANE_*`) and the label. Test panes have none.
    pub fn menu_item(self) -> Option<(u16, &'static str)> {
        match self {
            PaneId::Folders => Some((menu::VIEW_PANE_FOLDERS, "&Folders")),
            PaneId::Bookmarks => Some((menu::VIEW_PANE_BOOKMARKS, "Bookm&arks")),
            PaneId::Filters => Some((menu::VIEW_PANE_FILTERS, "Filter&s")),
            PaneId::Preview => Some((menu::VIEW_PANE_PREVIEW, "&Preview\tAlt+P")),
            PaneId::Test(_) => None,
        }
    }

    /// The pane a View menu command shows or hides.
    pub fn of_command(cmd: u16) -> Option<PaneId> {
        PaneId::KNOWN
            .into_iter()
            .find(|p| p.menu_item().is_some_and(|(id, _)| id == cmd))
    }

    /// The View menu's order (Everything's): Folders, Bookmarks, Filters,
    /// Preview.
    pub fn menu_rank(self) -> u32 {
        match self {
            PaneId::Folders => 0,
            PaneId::Bookmarks => 1,
            PaneId::Filters => 2,
            PaneId::Preview => 3,
            PaneId::Test(n) => 10 + u32::from(n),
        }
    }

    /// A number standing for it in a window's data or a message.
    pub fn code(self) -> usize {
        match self {
            PaneId::Folders => 1,
            PaneId::Filters => 2,
            PaneId::Bookmarks => 3,
            PaneId::Preview => 4,
            PaneId::Test(n) => 16 + usize::from(n.min(3)),
        }
    }

    pub fn from_code(code: usize) -> Option<PaneId> {
        Some(match code {
            1 => PaneId::Folders,
            2 => PaneId::Filters,
            3 => PaneId::Bookmarks,
            4 => PaneId::Preview,
            16..=19 => PaneId::Test((code - 16) as u8),
            _ => return None,
        })
    }

    fn known_index(self) -> Option<usize> {
        PaneId::KNOWN.iter().position(|&p| p == self)
    }
}

/// A pane's place as the settings keep it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneLayout {
    /// Shown (`<pane>_visible`); a pane this build has not got stays as
    /// the file says.
    pub visible: bool,
    pub side: Side,
    /// Its place among its dock's panes (`<pane>_order`), lowest at the
    /// top; equal orders go by `PaneId::rank`.
    pub order: i32,
    /// Its height, title row included, at 96 DPI (`<pane>_high`). The
    /// dock's first pane takes what the others leave instead.
    pub high: i32,
    /// Tab and Shift+Tab stop at it (`<pane>_tab_stop`); F6 always does.
    pub tab_stop: bool,
}

/// The docks as the settings keep them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockSettings {
    /// `left_sidebar_wide`, `right_sidebar_wide`, at 96 DPI.
    pub left_wide: i32,
    pub right_wide: i32,
    /// `pane_title`: the panes have their title rows.
    pub title: bool,
    /// `pane_min_wide`, at 96 DPI.
    pub min_wide: i32,
    /// Everything's panes, in `PaneId::KNOWN`'s order.
    pub panes: [PaneLayout; 4],
}

impl Default for DockSettings {
    fn default() -> DockSettings {
        DockSettings {
            left_wide: DEFAULT_WIDE,
            right_wide: DEFAULT_WIDE,
            title: true,
            min_wide: PANE_MIN_WIDE,
            panes: PaneId::KNOWN.map(PaneId::default_layout),
        }
    }
}

fn number(v: Option<&str>) -> Option<i64> {
    v?.trim().parse().ok()
}

fn flag(v: Option<&str>) -> Option<bool> {
    match v?.trim() {
        "1" => Some(true),
        "0" => Some(false),
        v if v.eq_ignore_ascii_case("true") => Some(true),
        v if v.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

impl DockSettings {
    /// The layout of a pane: its kept one, or for a test pane the defaults.
    pub fn pane(&self, id: PaneId) -> PaneLayout {
        id.known_index()
            .map_or_else(|| id.default_layout(), |i| self.panes[i])
    }

    /// Keeps `layout` for `id` (nothing for a test pane).
    pub fn set_pane(&mut self, id: PaneId, layout: PaneLayout) {
        if let Some(i) = id.known_index() {
            self.panes[i] = layout;
        }
    }

    /// The docks `get` holds under Everything's keys; what is missing or
    /// unreadable is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> DockSettings {
        let d = DockSettings::default();
        let size = |key: &str, default: i32| {
            number(get(key))
                .filter(|v| (1..=MAX_SIZE).contains(v))
                .map_or(default, |v| v as i32)
        };
        let min_wide = size("pane_min_wide", d.min_wide);
        let mut panes = d.panes;
        for (layout, id) in panes.iter_mut().zip(PaneId::KNOWN) {
            let key = id.key().expect("Everything's panes have keys");
            let k = |name: &str| format!("{key}_{name}");
            let def = id.default_layout();
            *layout = PaneLayout {
                visible: flag(get(&k("visible"))).unwrap_or(def.visible),
                side: number(get(&k("side")))
                    .and_then(Side::from_number)
                    .unwrap_or(def.side),
                order: number(get(&k("order")))
                    .filter(|v| v.abs() <= MAX_SIZE)
                    .map_or(def.order, |v| v as i32),
                high: size(&k("high"), def.high),
                tab_stop: flag(get(&k("tab_stop"))).unwrap_or(def.tab_stop),
            };
        }
        DockSettings {
            left_wide: size("left_sidebar_wide", d.left_wide).max(min_wide),
            right_wide: size("right_sidebar_wide", d.right_wide).max(min_wide),
            title: flag(get("pane_title")).unwrap_or(d.title),
            min_wide,
            panes,
        }
    }

    /// Every key, with its value, in Everything's order.
    pub fn to_keys(self) -> Vec<(String, String)> {
        let bit = |b: bool| u8::from(b).to_string();
        let mut out = vec![
            ("pane_title".to_owned(), bit(self.title)),
            ("pane_min_wide".to_owned(), self.min_wide.to_string()),
        ];
        for (layout, id) in self.panes.iter().zip(PaneId::KNOWN) {
            let key = id.key().expect("Everything's panes have keys");
            out.push((format!("{key}_visible"), bit(layout.visible)));
            out.push((format!("{key}_tab_stop"), bit(layout.tab_stop)));
            out.push((format!("{key}_side"), layout.side.number().to_string()));
            out.push((format!("{key}_order"), layout.order.to_string()));
            out.push((format!("{key}_high"), layout.high.to_string()));
        }
        out.push(("left_sidebar_wide".into(), self.left_wide.to_string()));
        out.push(("right_sidebar_wide".into(), self.right_wide.to_string()));
        out
    }
}

/// The panes' heights in a dock `total` pixels tall, dividers left out:
/// each but the first as `wanted` (at least its least, `mins`), the first
/// taking the rest. When that leaves the first less than its least, the
/// others give way from the bottom up, down to their least, then to
/// nothing in a dock too short for them all: the heights always add up to
/// `total`.
pub fn heights(total: i32, wanted: &[i32], mins: &[i32]) -> Vec<i32> {
    let n = wanted.len();
    if n == 0 {
        return Vec::new();
    }
    let total = total.max(0);
    let min = |i: usize| mins.get(i).copied().unwrap_or(0).max(0);
    let mut out: Vec<i32> = (0..n)
        .map(|i| if i == 0 { 0 } else { wanted[i].max(min(i)) })
        .collect();
    let rest = |out: &[i32]| total - out[1..].iter().sum::<i32>();
    // The others give way to the first's least: down to their own least,
    // then (a dock too short for them all) to nothing.
    for floor in [true, false] {
        let mut need = min(0) - rest(&out);
        for i in (1..n).rev() {
            if need <= 0 {
                break;
            }
            let least = if floor { min(i) } else { 0 };
            let give = need.min(out[i] - least).max(0);
            out[i] -= give;
            need -= give;
        }
    }
    out[0] = rest(&out).max(0);
    out
}

/// The panes of the dock `rc`, top to bottom, and the dividers between
/// them, `divider` thick: as `heights` shares the dock out.
pub fn stack(rc: RECT, wanted: &[i32], mins: &[i32], divider: i32) -> (Vec<RECT>, Vec<RECT>) {
    let n = wanted.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    let dividers = divider * (n as i32 - 1);
    let hs = heights(rc.bottom - rc.top - dividers, wanted, mins);
    let mut panes = Vec::with_capacity(n);
    let mut splits = Vec::with_capacity(n - 1);
    let mut y = rc.top;
    for (i, h) in hs.into_iter().enumerate() {
        let bottom = (y + h).min(rc.bottom);
        panes.push(RECT {
            top: y.min(rc.bottom),
            bottom,
            ..rc
        });
        y = bottom;
        if i + 1 < n {
            let bottom = (y + divider).min(rc.bottom);
            splits.push(RECT {
                top: y,
                bottom,
                ..rc
            });
            y = bottom;
        }
    }
    (panes, splits)
}

/// The panes' heights after the divider under pane `i` moved `dy` pixels
/// (down positive) from where it was with the panes `shown` high: pane `i`
/// grows by it and pane `i + 1` shrinks, neither below its least.
pub fn drag_split(shown: &[i32], mins: &[i32], i: usize, dy: i32) -> Vec<i32> {
    let mut out = shown.to_vec();
    if i + 1 >= out.len() {
        return out;
    }
    let min = |k: usize| mins.get(k).copied().unwrap_or(0).max(0);
    let up = (out[i] - min(i)).max(0);
    let down = (out[i + 1] - min(i + 1)).max(0);
    let dy = dy.clamp(-up, down);
    out[i] += dy;
    out[i + 1] -= dy;
    out
}

/// A dock's width after its divider moved `dx` pixels (right positive) from
/// where it was with the dock `start` wide: the left dock follows the
/// divider, the right dock the other way; within `min` and `max`.
pub fn drag_width(side: Side, start: i32, dx: i32, min: i32, max: i32) -> i32 {
    let w = match side {
        Side::Left => start + dx,
        Side::Right => start - dx,
    };
    w.min(max).max(min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn read(pairs: &[(&str, &str)]) -> DockSettings {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        DockSettings::from_keys(|k| map.get(k).copied())
    }

    #[test]
    fn everythings_keys_and_defaults() {
        // As Everything 1.5.0.1424b writes them.
        let d = read(&[
            ("pane_title", "1"),
            ("pane_min_wide", "32"),
            ("preview_visible", "0"),
            ("preview_side", "1"),
            ("preview_order", "0"),
            ("preview_high", "256"),
            ("folders_sidebar_visible", "0"),
            ("folders_sidebar_side", "0"),
            ("folders_sidebar_order", "0"),
            ("folders_sidebar_high", "256"),
            ("folders_sidebar_tab_stop", "1"),
            ("filters_sidebar_tab_stop", "0"),
            ("left_sidebar_wide", "256"),
            ("right_sidebar_wide", "256"),
        ]);
        assert_eq!(d, DockSettings::default());
        assert_eq!(read(&[]), DockSettings::default());
        assert_eq!(d.pane(PaneId::Preview).side, Side::Right);
        assert_eq!(d.pane(PaneId::Bookmarks).side, Side::Left);
        assert!(d.pane(PaneId::Folders).tab_stop && !d.pane(PaneId::Filters).tab_stop);
        // What is written reads back the same.
        let mut s = DockSettings {
            left_wide: 300,
            right_wide: 410,
            title: false,
            min_wide: 40,
            ..DockSettings::default()
        };
        s.set_pane(
            PaneId::Bookmarks,
            PaneLayout {
                visible: true,
                side: Side::Right,
                order: -2,
                high: 180,
                tab_stop: true,
            },
        );
        s.set_pane(
            PaneId::Test(0),
            PaneLayout {
                visible: true,
                ..PaneId::Test(0).default_layout()
            },
        );
        let pairs = s.to_keys();
        let map: HashMap<&str, &str> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(DockSettings::from_keys(|k| map.get(k).copied()), s);
        assert_eq!(map.get("bookmarks_sidebar_side"), Some(&"1"));
        assert_eq!(map.get("left_sidebar_wide"), Some(&"300"));
        // A test pane is kept nowhere.
        assert!(!pairs.iter().any(|(k, _)| k.contains("test")));
        assert!(!s.pane(PaneId::Test(0)).visible);
    }

    #[test]
    fn unreadable_values_are_the_defaults() {
        let d = read(&[
            ("left_sidebar_wide", "0"),
            ("right_sidebar_wide", "x"),
            ("preview_side", "2"),
            ("preview_high", "-4"),
            ("filters_sidebar_order", "99999999"),
            ("folders_sidebar_visible", "yes"),
            ("pane_min_wide", "0"),
        ]);
        assert_eq!(d, DockSettings::default());
        // A width under the least is the least.
        let d = read(&[("left_sidebar_wide", "10"), ("pane_min_wide", "48")]);
        assert_eq!((d.left_wide, d.min_wide), (48, 48));
    }

    #[test]
    fn the_first_pane_takes_what_the_others_leave() {
        // Everything's three left panes in its 736 px dock: 724 without
        // the two dividers; Filters and Bookmarks keep 256, Folders 212.
        assert_eq!(heights(724, &[256, 256, 256], &[58; 3]), [212, 256, 256]);
        // Room to spare: the first grows.
        assert_eq!(heights(1000, &[256, 256], &[58, 58]), [744, 256]);
        // One pane: all of it.
        assert_eq!(heights(500, &[256], &[58]), [500]);
        assert_eq!(heights(0, &[], &[]), Vec::<i32>::new());
    }

    #[test]
    fn short_docks_squeeze_from_the_bottom_up() {
        // The first at its least: the last gives way first.
        assert_eq!(heights(400, &[256, 256, 256], &[58; 3]), [58, 256, 86]);
        assert_eq!(heights(200, &[256, 256, 256], &[58; 3]), [58, 84, 58]);
        // Not even the least fits: the bottom ones go to nothing, the
        // heights still add up to the dock.
        let h = heights(100, &[256, 256, 256], &[58; 3]);
        assert_eq!(h, [58, 42, 0]);
        assert_eq!(h.iter().sum::<i32>(), 100);
        assert_eq!(heights(30, &[256, 256], &[58, 58]), [30, 0]);
        // A wanted height below the least is the least.
        assert_eq!(heights(500, &[256, 10], &[58, 58]), [442, 58]);
    }

    #[test]
    fn panes_and_dividers_tile_the_dock() {
        let dock = RECT {
            left: 0,
            top: 60,
            right: 256,
            bottom: 796,
        };
        let (panes, splits) = stack(dock, &[256, 256, 256], &[58; 3], 6);
        let tops: Vec<(i32, i32)> = panes.iter().map(|r| (r.top, r.bottom)).collect();
        assert_eq!(tops, [(60, 272), (278, 534), (540, 796)]);
        let between: Vec<(i32, i32)> = splits.iter().map(|r| (r.top, r.bottom)).collect();
        assert_eq!(between, [(272, 278), (534, 540)]);
        assert!(panes.iter().all(|r| r.left == 0 && r.right == 256));
        // A dock shorter than its dividers: nothing reaches past it.
        let tiny = RECT { bottom: 65, ..dock };
        let (panes, splits) = stack(tiny, &[256, 256, 256], &[58; 3], 6);
        for r in panes.iter().chain(&splits) {
            assert!(r.top >= 60 && r.bottom <= 65 && r.top <= r.bottom);
        }
    }

    #[test]
    fn dragging_a_divider_trades_height_between_its_neighbours() {
        let shown = [212, 256, 256];
        let mins = [58; 3];
        assert_eq!(drag_split(&shown, &mins, 0, 40), [252, 216, 256]);
        assert_eq!(drag_split(&shown, &mins, 1, -100), [212, 156, 356]);
        // Neither below its least.
        assert_eq!(drag_split(&shown, &mins, 0, 1000), [410, 58, 256]);
        assert_eq!(drag_split(&shown, &mins, 0, -1000), [58, 410, 256]);
        // No pane under the last.
        assert_eq!(drag_split(&shown, &mins, 2, 10), shown);
    }

    #[test]
    fn dragging_a_docks_divider() {
        assert_eq!(drag_width(Side::Left, 256, 40, 32, 900), 296);
        assert_eq!(drag_width(Side::Right, 256, 40, 32, 900), 216);
        assert_eq!(drag_width(Side::Left, 256, -500, 32, 900), 32);
        assert_eq!(drag_width(Side::Right, 256, -2000, 32, 900), 900);
    }

    #[test]
    fn ids_and_commands_round_trip() {
        for id in PaneId::KNOWN.into_iter().chain((0..4).map(PaneId::Test)) {
            assert_eq!(PaneId::from_code(id.code()), Some(id));
        }
        assert_eq!(PaneId::from_code(0), None);
        for id in PaneId::KNOWN {
            let (cmd, label) = id.menu_item().expect("a View menu item");
            assert_eq!(PaneId::of_command(cmd), Some(id));
            assert!(label.contains('&'));
        }
        assert_eq!(PaneId::of_command(menu::REFRESH), None);
        assert_eq!(PaneId::Test(2).menu_item(), None);
    }
}
