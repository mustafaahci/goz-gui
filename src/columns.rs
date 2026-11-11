//! The result list's columns: which are shown, in what order, and how wide,
//! kept in the settings file under Everything's own keys
//! (`name_column_width`, `path_column_visible`, `path_column_pos`, ...), so
//! the layout reads the same in both. Widths are kept at 96 DPI and scaled
//! to the window's DPI when shown.
//!
//! Name is always shown (Everything has no `name_column_visible`); the
//! others can be removed and shown again. Extension and Type start hidden,
//! as in Everything.

/// The columns, by id. An id is also the column's place in Everything's
/// default order.
pub const COL_NAME: usize = 0;
pub const COL_PATH: usize = 1;
pub const COL_SIZE: usize = 2;
pub const COL_EXT: usize = 3;
pub const COL_TYPE: usize = 4;
pub const COL_DATE: usize = 5;
pub const COLS: usize = 6;

pub const TITLES: [&str; COLS] = ["Name", "Path", "Size", "Extension", "Type", "Date Modified"];

/// The settings file's name for each column: `<key>_column_width` and so on.
/// Date Modified is Everything's `last_write_time`.
const KEYS: [&str; COLS] = [
    "name",
    "path",
    "size",
    "extension",
    "type",
    "last_write_time",
];

/// Everything's default widths, at 96 DPI.
pub const DEFAULT_WIDTHS: [i32; COLS] = [256, 256, 96, 96, 153, 153];

/// Whether each column is shown when the file does not say.
const DEFAULT_SHOWN: [bool; COLS] = [true, true, true, false, false, true];

/// The widest a column is kept; wider values in the file are taken as this.
const MAX_WIDTH: i32 = 8192;
/// The narrowest a column is kept, at 96 DPI. A saved width below it (0
/// from a damaged or hand-edited file, or one recorded while the list could
/// not say) is not a choice anyone made: the column takes its default.
pub const MIN_WIDTH: i32 = 16;

/// A width read from a file or session for column `id`: as saved, or the
/// default when it is too narrow to be a real one.
fn read_width(id: usize, width: i64) -> i32 {
    if width < i64::from(MIN_WIDTH) {
        DEFAULT_WIDTHS[id]
    } else {
        width.min(i64::from(MAX_WIDTH)) as i32
    }
}

/// The shown columns, left to right, and every column's width.
#[derive(Clone, Copy, Debug, Eq)]
pub struct Layout {
    /// Each column's width at 96 DPI, by id; kept while it is hidden.
    widths: [i32; COLS],
    /// The ids of the shown columns, left to right; `shown` of them.
    order: [usize; COLS],
    shown: usize,
}

/// Two layouts are the same when they show the same columns in the same
/// order at the same widths; the order's unused slots do not count.
impl PartialEq for Layout {
    fn eq(&self, other: &Layout) -> bool {
        self.widths == other.widths && self.shown() == other.shown()
    }
}

impl Default for Layout {
    /// Everything's default: Name, Path, Size and Date Modified.
    fn default() -> Layout {
        Layout {
            widths: DEFAULT_WIDTHS,
            order: [COL_NAME, COL_PATH, COL_SIZE, COL_DATE, COL_EXT, COL_TYPE],
            shown: 4,
        }
    }
}

fn number(text: Option<&str>) -> Option<i32> {
    text?.trim().parse().ok()
}

impl Layout {
    /// Reads the layout from the settings' keys; `get` looks one up. A key
    /// missing or unreadable takes Everything's default (so a file written
    /// before a column existed has it as Everything would).
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Layout {
        let mut layout = Layout::default();
        let visible: [bool; COLS] = core::array::from_fn(|id| {
            id == COL_NAME
                || number(get(&format!("{}_column_visible", KEYS[id])))
                    .map_or(DEFAULT_SHOWN[id], |v| v != 0)
        });
        let mut shown: Vec<(i32, usize)> = Vec::new();
        for (id, key) in KEYS.into_iter().enumerate() {
            if let Some(w) = number(get(&format!("{key}_column_width"))) {
                layout.widths[id] = read_width(id, w.into());
            }
            if visible[id] {
                // Without one, its place among the shown columns in the
                // default order, as Everything writes it.
                let place = visible[..id].iter().filter(|&&v| v).count() as i32;
                let pos = number(get(&format!("{key}_column_pos"))).unwrap_or(place);
                shown.push((pos, id));
            }
        }
        // By position; a tie (or a position Everything gave a hidden
        // column) keeps the default order.
        shown.sort();
        layout.shown = shown.len();
        for (slot, (_, id)) in layout.order.iter_mut().zip(shown) {
            *slot = id;
        }
        layout
    }

    /// The layout as the settings' keys and values. The shown columns get
    /// their places from 0; the hidden ones the places after them.
    pub fn to_keys(self) -> Vec<(String, String)> {
        let mut pos = [0usize; COLS];
        let mut next = 0;
        for &id in self.shown() {
            pos[id] = next;
            next += 1;
        }
        for id in (0..COLS).filter(|&id| !self.is_shown(id)) {
            pos[id] = next;
            next += 1;
        }
        let mut out = Vec::new();
        for (id, key) in KEYS.into_iter().enumerate() {
            if id != COL_NAME {
                out.push((
                    format!("{key}_column_visible"),
                    u8::from(self.is_shown(id)).to_string(),
                ));
            }
            out.push((format!("{key}_column_pos"), pos[id].to_string()));
            out.push((format!("{key}_column_width"), self.widths[id].to_string()));
        }
        out
    }

    /// The shown columns' ids, left to right.
    pub fn shown(&self) -> &[usize] {
        &self.order[..self.shown]
    }

    pub fn is_shown(&self, id: usize) -> bool {
        self.shown().contains(&id)
    }

    /// The column's width at 96 DPI.
    pub fn width(&self, id: usize) -> i32 {
        self.widths[id.min(COLS - 1)]
    }

    pub fn set_width(&mut self, id: usize, width: i32) {
        if id < COLS {
            self.widths[id] = width.clamp(MIN_WIDTH, MAX_WIDTH);
        }
    }

    /// Puts the shown columns in `order` (ids, left to right). An order that
    /// is not exactly the shown columns is ignored.
    pub fn set_order(&mut self, order: &[usize]) {
        let mut a = order.to_vec();
        let mut b = self.shown().to_vec();
        a.sort_unstable();
        b.sort_unstable();
        if a == b {
            self.order[..order.len()].copy_from_slice(order);
        }
    }

    /// Takes a column off the list; never Name. False when nothing changed.
    pub fn hide(&mut self, id: usize) -> bool {
        let Some(at) = self.shown().iter().position(|&c| c == id) else {
            return false;
        };
        if id == COL_NAME {
            return false;
        }
        self.order.copy_within(at + 1..self.shown, at);
        self.shown -= 1;
        true
    }

    /// The layout a tab of Everything's session keeps (its `columns`): the
    /// shown columns left to right, each with its width at 96 DPI. A column
    /// listed twice counts once; Name shows first when it is not listed (it
    /// is always shown); the columns not listed are hidden, with `base`'s
    /// widths.
    pub fn from_shown(base: Layout, shown: &[(usize, i32)]) -> Layout {
        let mut layout = Layout {
            widths: base.widths,
            order: [0; COLS],
            shown: 0,
        };
        if !shown.iter().any(|&(id, _)| id == COL_NAME) {
            layout.order[0] = COL_NAME;
            layout.shown = 1;
        }
        for &(id, width) in shown {
            if id >= COLS || layout.shown().contains(&id) {
                continue;
            }
            layout.widths[id] = read_width(id, width.into());
            layout.order[layout.shown] = id;
            layout.shown += 1;
        }
        // The hidden ones fill the order's unused slots.
        let mut at = layout.shown;
        for id in 0..COLS {
            if !layout.shown().contains(&id) {
                layout.order[at] = id;
                at += 1;
            }
        }
        layout
    }

    /// The shown columns, left to right, each with its width at 96 DPI: what
    /// a tab of a session keeps.
    pub fn shown_widths(&self) -> Vec<(usize, i32)> {
        self.shown()
            .iter()
            .map(|&id| (id, self.width(id)))
            .collect()
    }

    /// Shows a hidden column, after the shown column that comes last before
    /// it in the default order (or first, when none does).
    pub fn show(&mut self, id: usize) -> bool {
        if id >= COLS || self.is_shown(id) {
            return false;
        }
        let at = self
            .shown()
            .iter()
            .rposition(|&c| c < id)
            .map_or(0, |i| i + 1);
        self.order.copy_within(at..self.shown, at + 1);
        self.order[at] = id;
        self.shown += 1;
        true
    }
}

/// The width a column sized to fit gets: as wide as its title (`title`, the
/// width it needs) and every cell measured (`cells`; an empty one needs 0),
/// and never wider than a column is kept.
pub fn fit_width(title: i32, cells: impl IntoIterator<Item = i32>) -> i32 {
    cells.into_iter().fold(title, i32::max).clamp(0, MAX_WIDTH)
}

/// `width` pixels at `dpi` as a 96-DPI width, rounded to the nearest.
pub fn unscale(width: i32, dpi: u32) -> i32 {
    let dpi = dpi.max(1) as i32;
    (width * 96 + dpi / 2).div_euclid(dpi)
}

/// A 96-DPI width at `dpi`, rounded to the nearest pixel, so that
/// `unscale` gives the same width back at every DPI.
pub fn scale(width: i32, dpi: u32) -> i32 {
    (width * dpi as i32 + 48).div_euclid(96)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn read(pairs: &[(&str, &str)]) -> Layout {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        Layout::from_keys(|k| map.get(k).copied())
    }

    #[test]
    fn a_sessions_columns_read_back_as_kept() {
        let mut layout = Layout::default();
        layout.show(COL_EXT);
        layout.set_width(COL_PATH, 300);
        layout.set_order(&[COL_PATH, COL_NAME, COL_EXT, COL_SIZE, COL_DATE]);
        let kept = layout.shown_widths();
        assert_eq!(
            kept,
            [
                (COL_PATH, 300),
                (COL_NAME, 256),
                (COL_EXT, 96),
                (COL_SIZE, 96),
                (COL_DATE, 153)
            ]
        );
        assert_eq!(Layout::from_shown(Layout::default(), &kept), layout);
        // Name always shows (first when not listed); twice or unknown
        // counts once or not at all; the hidden keep the base's widths.
        let mut base = Layout::default();
        base.set_width(COL_TYPE, 77);
        let l = Layout::from_shown(base, &[(COL_SIZE, 50), (COL_SIZE, 60), (99, 10)]);
        assert_eq!(l.shown(), [COL_NAME, COL_SIZE]);
        assert_eq!((l.width(COL_SIZE), l.width(COL_TYPE)), (50, 77));
        assert!(!l.is_shown(COL_TYPE));
        // Every id is still in the order once, so showing one works.
        let mut l = l;
        assert!(l.show(COL_TYPE));
        assert_eq!(l.shown(), [COL_NAME, COL_SIZE, COL_TYPE]);
    }

    #[test]
    fn nothing_in_the_file_is_everythings_default() {
        let layout = read(&[]);
        assert_eq!(layout, Layout::default());
        assert_eq!(layout.shown(), [COL_NAME, COL_PATH, COL_SIZE, COL_DATE]);
        assert_eq!(layout.width(COL_DATE), 153);
    }

    #[test]
    fn everythings_own_keys_are_read() {
        // As Everything 1.5 writes them; Extension and Type, not given, stay
        // hidden.
        let layout = read(&[
            ("name_column_pos", "0"),
            ("name_column_width", "300"),
            ("path_column_visible", "1"),
            ("path_column_pos", "2"),
            ("path_column_width", "200"),
            ("size_column_visible", "0"),
            ("size_column_pos", "1"),
            ("size_column_width", "80"),
            ("last_write_time_column_visible", "1"),
            ("last_write_time_column_pos", "1"),
            ("last_write_time_column_width", "x"),
        ]);
        assert_eq!(layout.shown(), [COL_NAME, COL_DATE, COL_PATH]);
        assert_eq!(layout.width(COL_NAME), 300);
        assert_eq!(layout.width(COL_SIZE), 80, "kept while hidden");
        assert_eq!(layout.width(COL_DATE), 153, "unreadable: the default");
    }

    #[test]
    fn name_cannot_be_hidden_even_by_the_file() {
        let layout = read(&[("name_column_visible", "0"), ("name_column_pos", "5")]);
        assert_eq!(layout.shown(), [COL_PATH, COL_SIZE, COL_DATE, COL_NAME]);
        let mut layout = Layout::default();
        assert!(!layout.hide(COL_NAME));
        assert!(layout.hide(COL_PATH));
        assert!(!layout.hide(COL_PATH));
        assert_eq!(layout.shown(), [COL_NAME, COL_SIZE, COL_DATE]);
    }

    #[test]
    fn written_keys_read_back_the_same() {
        let mut layout = Layout::default();
        layout.set_order(&[COL_DATE, COL_NAME, COL_SIZE, COL_PATH]);
        layout.hide(COL_SIZE);
        layout.set_width(COL_PATH, 123);
        let keys = layout.to_keys();
        let pairs: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(read(&pairs), layout);
        let get = |k: &str| {
            keys.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("last_write_time_column_pos"), Some("0"));
        assert_eq!(get("name_column_pos"), Some("1"));
        assert_eq!(get("path_column_pos"), Some("2"));
        assert_eq!(get("size_column_visible"), Some("0"));
        assert_eq!(get("size_column_pos"), Some("3"));
        assert_eq!(get("name_column_visible"), None, "Everything has none");
        assert_eq!(keys.len(), 17);
    }

    #[test]
    fn extension_and_type_use_everythings_keys() {
        // Everything 1.5's own defaults, as its file has them.
        let defaults = read(&[
            ("size_column_pos", "2"),
            ("extension_column_visible", "0"),
            ("extension_column_pos", "3"),
            ("extension_column_width", "96"),
            ("type_column_visible", "0"),
            ("type_column_pos", "4"),
            ("type_column_width", "153"),
            ("last_write_time_column_visible", "1"),
            ("last_write_time_column_pos", "3"),
        ]);
        assert_eq!(defaults, Layout::default());
        // A file from before they existed leaves them hidden.
        assert_eq!(read(&[("size_column_pos", "2")]), Layout::default());
        // Shown with no places: in the default order.
        let both = read(&[
            ("extension_column_visible", "1"),
            ("type_column_visible", "1"),
        ]);
        assert_eq!(
            both.shown(),
            [COL_NAME, COL_PATH, COL_SIZE, COL_EXT, COL_TYPE, COL_DATE]
        );
        // Shown, they go where Everything puts them, and read back.
        let mut layout = Layout::default();
        assert!(layout.show(COL_TYPE));
        assert!(layout.show(COL_EXT));
        layout.set_width(COL_TYPE, 200);
        assert_eq!(
            layout.shown(),
            [COL_NAME, COL_PATH, COL_SIZE, COL_EXT, COL_TYPE, COL_DATE]
        );
        let keys = layout.to_keys();
        let pairs: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert!(pairs.contains(&("extension_column_visible", "1")));
        assert!(pairs.contains(&("type_column_pos", "4")));
        assert!(pairs.contains(&("type_column_width", "200")));
        assert_eq!(read(&pairs), layout);
        assert_eq!(read(&pairs).width(COL_TYPE), 200);
    }

    #[test]
    fn a_shown_column_comes_back_in_its_default_place() {
        let mut layout = Layout::default();
        layout.hide(COL_SIZE);
        layout.hide(COL_PATH);
        assert!(layout.show(COL_SIZE));
        assert_eq!(layout.shown(), [COL_NAME, COL_SIZE, COL_DATE]);
        assert!(!layout.show(COL_SIZE));
        // After a drag, it goes after the last column before it by id.
        layout.set_order(&[COL_DATE, COL_SIZE, COL_NAME]);
        assert!(layout.show(COL_PATH));
        assert_eq!(layout.shown(), [COL_DATE, COL_SIZE, COL_NAME, COL_PATH]);
    }

    #[test]
    fn an_order_that_is_not_the_shown_columns_is_ignored() {
        let mut layout = Layout::default();
        layout.hide(COL_DATE);
        layout.set_order(&[COL_NAME, COL_DATE, COL_PATH]);
        assert_eq!(layout.shown(), [COL_NAME, COL_PATH, COL_SIZE]);
        layout.set_order(&[COL_SIZE, COL_PATH, COL_NAME]);
        assert_eq!(layout.shown(), [COL_SIZE, COL_PATH, COL_NAME]);
    }

    #[test]
    fn a_fitted_column_takes_its_widest_text() {
        assert_eq!(fit_width(60, [120, 0, 90]), 120);
        // Short cells (or none in view): the title's width.
        assert_eq!(fit_width(60, [10, 0]), 60);
        assert_eq!(fit_width(60, []), 60);
        assert_eq!(fit_width(60, [100_000]), MAX_WIDTH);
    }

    #[test]
    fn widths_survive_scaling_round_trips() {
        for dpi in [96, 110, 120, 144, 168, 192, 288] {
            for w in (0..1200).chain([4096, 8192]) {
                assert_eq!(unscale(scale(w, dpi), dpi), w, "{w} at {dpi}");
            }
        }
        assert_eq!(unscale(230, 144), 153);
    }

    /// A file (or session) whose widths were saved as 0 shows its columns at
    /// their defaults, not zero pixels wide.
    #[test]
    fn zero_widths_read_as_defaults() {
        let l = read(&[
            ("name_column_width", "0"),
            ("path_column_width", "3"),
            ("size_column_width", "120"),
            ("last_write_time_column_width", "-5"),
        ]);
        assert_eq!(l.width(COL_NAME), DEFAULT_WIDTHS[COL_NAME]);
        assert_eq!(l.width(COL_PATH), DEFAULT_WIDTHS[COL_PATH]);
        assert_eq!(l.width(COL_SIZE), 120);
        assert_eq!(l.width(COL_DATE), DEFAULT_WIDTHS[COL_DATE]);
        let tab = Layout::from_shown(Layout::default(), &[(COL_NAME, 0), (COL_PATH, 300)]);
        assert_eq!(tab.width(COL_NAME), DEFAULT_WIDTHS[COL_NAME]);
        assert_eq!(tab.width(COL_PATH), 300);
        let mut l = Layout::default();
        l.set_width(COL_SIZE, 0);
        assert_eq!(l.width(COL_SIZE), MIN_WIDTH);
    }
}
