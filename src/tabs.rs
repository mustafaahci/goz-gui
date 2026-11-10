//! Tabs and windows (Everything 1.5's `tabs`, `tab-bar-options` and
//! `multi-window`), the parts that need no window: the tab commands' ids,
//! the settings under Everything's INI keys, the list of tabs with the
//! ones closed, where the tab strip puts each tab and button, and the
//! command line File > New Window starts goz-gui with.
//!
//! Only one tab's results are live: the window's own state is the shown
//! tab's, and every other tab is kept as `crate::session::View` (its search
//! and options, view, columns, scroll position, selection by path and
//! navigation history), so a tab costs no rows and no searches while
//! another is shown (`window/tabs.rs`).

use crate::menu;

// The commands (F3's range, `menu::F3_FIRST`).
/// File > New Tab (Ctrl+T).
pub const NEW_TAB: u16 = menu::F3_FIRST;
/// File > Close Tab (Ctrl+W, Ctrl+F4).
pub const CLOSE_TAB: u16 = menu::F3_FIRST + 1;
/// Ctrl+Tab and Ctrl+Shift+Tab (`ctrl_tab_action=1`): the next or
/// previous tab, going round.
pub const NEXT_TAB: u16 = menu::F3_FIRST + 2;
pub const PREV_TAB: u16 = menu::F3_FIRST + 3;
/// History's Restore Recently Closed Tab (Ctrl+Shift+T), the tab menu's
/// Reopen Closed Tab.
pub const REOPEN_TAB: u16 = menu::F3_FIRST + 4;
/// The tab menu's Duplicate Tab, Close Tabs to Left / to Right, Close
/// Other Tabs, and History > Recently Closed Tabs > Reopen All Tabs.
pub const DUPLICATE_TAB: u16 = menu::F3_FIRST + 5;
pub const CLOSE_TABS_LEFT: u16 = menu::F3_FIRST + 6;
pub const CLOSE_TABS_RIGHT: u16 = menu::F3_FIRST + 7;
pub const CLOSE_OTHER_TABS: u16 = menu::F3_FIRST + 8;
pub const REOPEN_ALL_TABS: u16 = menu::F3_FIRST + 9;
/// File > New Window (Ctrl+N) and Close Window (Ctrl+Shift+W).
pub const NEW_WINDOW: u16 = menu::F3_FIRST + 10;
pub const CLOSE_WINDOW: u16 = menu::F3_FIRST + 11;
/// History > Recently Closed Tabs' entries, newest first, one each.
pub const CLOSED_TAB_FIRST: u16 = menu::TAB_ENTRY_FIRST;
pub const CLOSED_TABS: usize = menu::TAB_ENTRIES;

/// A tab's title: its search on one line, or "New Tab" for none.
pub fn title(search: &str) -> String {
    if search.trim().is_empty() {
        "New Tab".to_owned()
    } else {
        crate::bookmarks::one_line(search)
    }
}

/// The tabs' settings, under Everything 1.5's keys with its defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabSettings {
    /// `tabs_show`: the tab strip shows at all (0 hides it; the tabs still
    /// work from the keyboard and the menus).
    pub show: bool,
    /// `tabs_always_show`: the strip shows with a single tab too (by
    /// default only with two or more).
    pub always_show: bool,
    /// `tabs_min_wide` and `tabs_max_wide`: a tab's width at 96 DPI.
    pub min_wide: i32,
    pub max_wide: i32,
    /// `tabs_close_button`: each tab has its ×.
    pub close_button: bool,
    /// `tabs_insert_after_current`: a new tab goes right of the current one
    /// instead of after the last.
    pub insert_after_current: bool,
    /// `tabs_double_click_action`: 1 closes the tab double-clicked; 0 (the
    /// default) does nothing more than the click.
    pub double_click_action: u8,
    /// `tabs_close_window_with_last_tab`: closing the last tab closes the
    /// window; off, it leaves a new tab.
    pub close_window_with_last_tab: bool,
    /// `ctrl_tab_action`: 1 (the default) Ctrl+Tab goes round the tabs;
    /// otherwise it moves the keyboard between frames, as Tab does.
    pub ctrl_tab_action: u8,
    /// `session_store_max_tabs_undo`: how many closed tabs Reopen Closed Tab
    /// can bring back.
    pub max_closed: usize,
    /// `offset_new_window_position` with `_x` and `_y`: a new window opens
    /// that far (96-DPI pixels) right of and below the window it came from.
    pub offset_new_window: bool,
    pub offset_x: i32,
    pub offset_y: i32,
}

impl Default for TabSettings {
    fn default() -> TabSettings {
        TabSettings {
            show: true,
            always_show: false,
            min_wide: 76,
            max_wide: 224,
            close_button: true,
            insert_after_current: false,
            double_click_action: 0,
            close_window_with_last_tab: true,
            ctrl_tab_action: 1,
            max_closed: 24,
            offset_new_window: true,
            offset_x: 22,
            offset_y: 22,
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

impl TabSettings {
    /// The settings `get` holds under Everything's keys; what is missing or
    /// unreadable (or out of any sane range) is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> TabSettings {
        let d = TabSettings::default();
        let int = |key: &str, range: core::ops::RangeInclusive<i64>, default: i64| {
            number(get(key))
                .filter(|v| range.contains(v))
                .unwrap_or(default)
        };
        let min_wide = int("tabs_min_wide", 16..=1024, d.min_wide.into()) as i32;
        let max_wide = int("tabs_max_wide", 16..=4096, d.max_wide.into()) as i32;
        TabSettings {
            show: flag(get("tabs_show")).unwrap_or(d.show),
            always_show: flag(get("tabs_always_show")).unwrap_or(d.always_show),
            min_wide,
            // Never narrower than the least.
            max_wide: max_wide.max(min_wide),
            close_button: flag(get("tabs_close_button")).unwrap_or(d.close_button),
            insert_after_current: flag(get("tabs_insert_after_current"))
                .unwrap_or(d.insert_after_current),
            double_click_action: int(
                "tabs_double_click_action",
                0..=255,
                d.double_click_action.into(),
            ) as u8,
            close_window_with_last_tab: flag(get("tabs_close_window_with_last_tab"))
                .unwrap_or(d.close_window_with_last_tab),
            ctrl_tab_action: int("ctrl_tab_action", 0..=255, d.ctrl_tab_action.into()) as u8,
            max_closed: int(
                "session_store_max_tabs_undo",
                0..=CLOSED_TABS as i64,
                d.max_closed as i64,
            ) as usize,
            offset_new_window: flag(get("offset_new_window_position"))
                .unwrap_or(d.offset_new_window),
            offset_x: int(
                "offset_new_window_position_x",
                -4096..=4096,
                d.offset_x.into(),
            ) as i32,
            offset_y: int(
                "offset_new_window_position_y",
                -4096..=4096,
                d.offset_y.into(),
            ) as i32,
        }
    }

    /// Every key, with its value.
    pub fn to_keys(self) -> Vec<(String, String)> {
        let bit = |b: bool| u8::from(b).to_string();
        vec![
            ("tabs_show".into(), bit(self.show)),
            ("tabs_always_show".into(), bit(self.always_show)),
            ("tabs_min_wide".into(), self.min_wide.to_string()),
            ("tabs_max_wide".into(), self.max_wide.to_string()),
            ("tabs_close_button".into(), bit(self.close_button)),
            (
                "tabs_insert_after_current".into(),
                bit(self.insert_after_current),
            ),
            (
                "tabs_double_click_action".into(),
                self.double_click_action.to_string(),
            ),
            (
                "tabs_close_window_with_last_tab".into(),
                bit(self.close_window_with_last_tab),
            ),
            ("ctrl_tab_action".into(), self.ctrl_tab_action.to_string()),
            (
                "session_store_max_tabs_undo".into(),
                self.max_closed.to_string(),
            ),
            (
                "offset_new_window_position".into(),
                bit(self.offset_new_window),
            ),
            (
                "offset_new_window_position_x".into(),
                self.offset_x.to_string(),
            ),
            (
                "offset_new_window_position_y".into(),
                self.offset_y.to_string(),
            ),
        ]
    }

    /// Whether the strip shows with `count` tabs.
    pub fn strip_shown(&self, count: usize) -> bool {
        self.show && (count >= 2 || self.always_show)
    }

    /// Whether a double-click on a tab closes it.
    pub fn double_click_closes(&self) -> bool {
        self.double_click_action == 1
    }

    /// Whether Ctrl+Tab goes round the tabs.
    pub fn ctrl_tab_cycles(&self) -> bool {
        self.ctrl_tab_action == 1
    }
}

/// Which tabs Close Multiple Tabs closes, beside the one it was asked on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Others {
    Left,
    Right,
    All,
}

/// A closed tab and the place it had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed<T> {
    pub index: usize,
    pub tab: T,
}

/// The tabs of a window, left to right, with the one shown, and the tabs
/// closed (oldest first, at most `max_closed`). There is always at least
/// one tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabList<T> {
    tabs: Vec<T>,
    current: usize,
    closed: Vec<Closed<T>>,
    max_closed: usize,
}

impl<T> TabList<T> {
    /// One tab.
    pub fn new(first: T, max_closed: usize) -> TabList<T> {
        TabList {
            tabs: vec![first],
            current: 0,
            closed: Vec::new(),
            max_closed,
        }
    }

    /// `tabs` with `current` shown and `closed` (oldest first) to reopen,
    /// as a saved session holds them; `None` without a tab. A current past
    /// the end is the last tab; past `max_closed`, the oldest closed go.
    pub fn from_parts(
        tabs: Vec<T>,
        current: usize,
        closed: Vec<T>,
        max_closed: usize,
    ) -> Option<TabList<T>> {
        if tabs.is_empty() {
            return None;
        }
        let at_end = tabs.len();
        let mut list = TabList {
            current: current.min(tabs.len() - 1),
            tabs,
            closed: closed
                .into_iter()
                .map(|tab| Closed { index: at_end, tab })
                .collect(),
            max_closed,
        };
        list.trim_closed();
        Some(list)
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn tabs(&self) -> &[T] {
        &self.tabs
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        self.tabs.get(i)
    }

    /// Puts `tab` in place of tab `i`.
    pub fn set(&mut self, i: usize, tab: T) {
        if let Some(slot) = self.tabs.get_mut(i) {
            *slot = tab;
        }
    }

    /// Shows tab `i`; false when there is no such tab.
    pub fn select(&mut self, i: usize) -> bool {
        if i < self.tabs.len() {
            self.current = i;
            true
        } else {
            false
        }
    }

    /// Puts `tab` at `at` (at most the end); the tab shown stays shown.
    /// Its index.
    pub fn insert(&mut self, at: usize, tab: T) -> usize {
        let at = at.min(self.tabs.len());
        self.tabs.insert(at, tab);
        if at <= self.current && self.tabs.len() > 1 {
            self.current += 1;
        }
        at
    }

    /// The tab after (or before) the one shown, going round.
    pub fn step(&self, forward: bool) -> usize {
        let n = self.tabs.len();
        if forward {
            (self.current + 1) % n
        } else {
            (self.current + n - 1) % n
        }
    }

    /// The tab shown once tab `i` is closed: the one shown now if it is not
    /// `i`; else the tab right of it, or left of it when it is the last.
    /// Its index after the close.
    pub fn after_close(&self, i: usize) -> usize {
        if i == self.current {
            i.min(self.tabs.len().saturating_sub(2))
        } else if i < self.current {
            self.current - 1
        } else {
            self.current
        }
    }

    /// Closes tab `i`, kept to be reopened; never the only tab (`None`).
    /// The shown tab becomes `after_close(i)`.
    pub fn close(&mut self, i: usize) -> Option<usize> {
        if self.tabs.len() < 2 || i >= self.tabs.len() {
            return None;
        }
        let next = self.after_close(i);
        let tab = self.tabs.remove(i);
        self.current = next;
        self.closed.push(Closed { index: i, tab });
        self.trim_closed();
        Some(next)
    }

    /// Keeps `tab` as closed, as if it had been at `index` (the window's
    /// last tab, which closing replaced).
    pub fn push_closed(&mut self, index: usize, tab: T) {
        self.closed.push(Closed { index, tab });
        self.trim_closed();
    }

    fn trim_closed(&mut self) {
        let cut = self.closed.len().saturating_sub(self.max_closed);
        self.closed.drain(..cut);
    }

    /// The closed tabs, newest first.
    pub fn closed_newest_first(&self) -> impl Iterator<Item = &T> {
        self.closed.iter().rev().map(|c| &c.tab)
    }

    pub fn closed_count(&self) -> usize {
        self.closed.len()
    }

    /// Takes the closed tab `newest` places from the newest (0 is the one
    /// closed last).
    pub fn take_closed(&mut self, newest: usize) -> Option<Closed<T>> {
        let at = self.closed.len().checked_sub(newest + 1)?;
        Some(self.closed.remove(at))
    }

    /// Every closed tab, oldest first, to keep in a session.
    pub fn closed(&self) -> impl Iterator<Item = &T> {
        self.closed.iter().map(|c| &c.tab)
    }

    /// The tabs Close Multiple Tabs closes for tab `i`, right to left (so
    /// each index still holds as the ones after it go).
    pub fn others(&self, i: usize, which: Others) -> Vec<usize> {
        let n = self.tabs.len();
        if i >= n {
            return Vec::new();
        }
        let mut out: Vec<usize> = match which {
            Others::Left => (0..i).collect(),
            Others::Right => (i + 1..n).collect(),
            Others::All => (0..n).filter(|&j| j != i).collect(),
        };
        out.reverse();
        out
    }

    /// Moves tab `from` to `to` (a drag along the strip); the tab shown
    /// stays shown wherever it goes.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        let n = self.tabs.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.current = if self.current == from {
            to
        } else if from < self.current && to >= self.current {
            self.current - 1
        } else if from > self.current && to <= self.current {
            self.current + 1
        } else {
            self.current
        };
    }

    /// Replaces every tab with `tabs` (showing `current`), as a restored
    /// session does; `older` (closed tabs, oldest first) go before the
    /// closed ones, and the tabs there were after them, so Reopen Closed
    /// Tab brings those back first. Nothing changes for no tabs.
    pub fn replace(&mut self, tabs: Vec<T>, current: usize, older: Vec<T>) {
        if tabs.is_empty() {
            return;
        }
        let end = tabs.len();
        let old = core::mem::replace(&mut self.tabs, tabs);
        let mut closed: Vec<Closed<T>> = older
            .into_iter()
            .map(|tab| Closed { index: end, tab })
            .collect();
        closed.append(&mut self.closed);
        closed.extend(
            old.into_iter()
                .enumerate()
                .map(|(index, tab)| Closed { index, tab }),
        );
        self.closed = closed;
        self.trim_closed();
        self.current = current.min(self.tabs.len() - 1);
    }
}

/// What the tab strip is laid out with, in pixels at the window's DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    /// Where the tabs start and how far the strip's buttons may reach
    /// (Everything's `tabs_margin_left` and `_right` in from the edges).
    pub left: i32,
    pub right: i32,
    /// The tabs' rows (`tabs_margin_top` down from the strip's top).
    pub top: i32,
    pub bottom: i32,
    /// `tabs_min_wide` and `tabs_max_wide`.
    pub min: i32,
    pub max: i32,
    /// The "+" and overflow buttons' width.
    pub button: i32,
    /// A tab's × button: its side (0: none, `tabs_close_button=0`) and its
    /// centre's distance from the tab's right edge.
    pub close: i32,
    pub close_inset: i32,
}

/// Where the strip put each tab and button.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    /// The tabs in view: index, left, right (exclusive).
    pub tabs: Vec<(usize, i32, i32)>,
    /// The "+" after the last tab in view: left, right.
    pub plus: (i32, i32),
    /// The list of every tab, at the right end, while some are out of view.
    pub overflow: Option<(i32, i32)>,
    /// The first tab in view (the strip scrolls to keep the shown tab in
    /// view).
    pub first: usize,
}

/// Lays out `n` tabs, the shown one `current`, scrolled to `first` when
/// they do not all fit. The tabs share the room equally, each between
/// `min` and `max` wide, as Everything's do; a "+" follows the last.
/// When even `min` leaves too little room, as many as fit show at `min`
/// (or a little more, to fill the room), the shown one always among
/// them, and an overflow button at the right end lists them all.
pub fn place(n: usize, current: usize, first: usize, m: &Metrics) -> Placed {
    if n == 0 {
        return Placed::default();
    }
    let room = (m.right - m.left).max(0);
    let count = n as i32;
    let each = (room - m.button).max(0) / count;
    if each >= m.min {
        let w = each.min(m.max);
        let tabs = (0..n)
            .map(|i| {
                let l = m.left + i as i32 * w;
                (i, l, l + w)
            })
            .collect();
        let end = m.left + count * w;
        return Placed {
            tabs,
            plus: (end, end + m.button),
            overflow: None,
            first: 0,
        };
    }
    // Some out of view: the "+" and the overflow button keep their room.
    let room = (room - 2 * m.button).max(1);
    let fit = ((room / m.min.max(1)).max(1) as usize).min(n);
    let w = (room / fit as i32).clamp(1, m.max.max(1));
    let current = current.min(n - 1);
    let mut first = first.min(n - fit);
    if current < first {
        first = current;
    } else if current >= first + fit {
        first = current + 1 - fit;
    }
    let tabs = (0..fit)
        .map(|k| {
            let l = m.left + k as i32 * w;
            (first + k, l, l + w)
        })
        .collect();
    let end = m.left + fit as i32 * w;
    Placed {
        tabs,
        plus: (end, end + m.button),
        overflow: Some((m.right - m.button, m.right)),
        first,
    }
}

/// A tab's × button: (left, top, right, bottom), centred `close_inset`
/// from the tab's right edge and in its row; empty without close buttons.
pub fn close_box(right: i32, m: &Metrics) -> (i32, i32, i32, i32) {
    if m.close <= 0 {
        return (0, 0, 0, 0);
    }
    let cx = right - m.close_inset;
    let cy = (m.top + m.bottom) / 2;
    let l = cx - m.close / 2;
    let t = cy - m.close / 2;
    (l, t, l + m.close, t + m.close)
}

/// What is at a point of the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Close(usize),
    Plus,
    Overflow,
    /// The strip, past the tabs and buttons.
    Empty,
}

fn within(v: i32, (a, b): (i32, i32)) -> bool {
    v >= a && v < b
}

/// What is at (`x`, `y`), strip coordinates as `place` used.
pub fn hit(p: &Placed, m: &Metrics, x: i32, y: i32) -> Hit {
    if !within(y, (m.top, m.bottom)) {
        return Hit::Empty;
    }
    for &(i, l, r) in &p.tabs {
        if within(x, (l, r)) {
            let (cl, ct, cr, cb) = close_box(r, m);
            if within(x, (cl, cr)) && within(y, (ct, cb)) {
                return Hit::Close(i);
            }
            return Hit::Tab(i);
        }
    }
    if within(x, p.plus) {
        return Hit::Plus;
    }
    if p.overflow.is_some_and(|o| within(x, o)) {
        return Hit::Overflow;
    }
    Hit::Empty
}

/// The tab a drag at `x` is over: the one in view under it, or the first
/// or last in view when `x` is past them. `None` with no tab in view.
pub fn drop_slot(p: &Placed, x: i32) -> Option<usize> {
    let &(first, l, _) = p.tabs.first()?;
    let &(last, _, r) = p.tabs.last()?;
    if x < l {
        return Some(first);
    }
    if x >= r {
        return Some(last);
    }
    p.tabs
        .iter()
        .find(|&&(_, l, r)| within(x, (l, r)))
        .map(|&(i, ..)| i)
}

/// `arg` as one argument of a Windows command line, quoted so the C
/// runtime (and `CommandLineToArgvW`) reads it back as it was.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{0b}', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                // Each backslash before a quote doubled, and the quote
                // escaped.
                out.extend(core::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.extend(core::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    // Backslashes before the closing quote, doubled.
    out.extend(core::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// The switches File > New Window starts goz-gui with: a window of its
/// own (`-new-window`) at `rect` (x, y, width, height in screen pixels; none
/// leaves it to the settings), of the same named instance.
pub fn new_window_args(rect: Option<(i32, i32, i32, i32)>, instance: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["-new-window".into()];
    if let Some((x, y, w, h)) = rect {
        for (switch, v) in [("-x", x), ("-y", y), ("-width", w), ("-height", h)] {
            args.push(switch.into());
            args.push(v.to_string());
        }
    }
    if let Some(name) = instance {
        args.push("-instance".into());
        args.push(name.to_owned());
    }
    args
}

/// The command line for `exe` with `args`, each quoted.
pub fn command_line(exe: &str, args: &[String]) -> String {
    core::iter::once(quote(exe))
        .chain(args.iter().map(|a| quote(a)))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize) -> TabList<usize> {
        let mut l = TabList::new(0, 24);
        for i in 1..n {
            l.insert(i, i);
        }
        l
    }

    #[test]
    fn titles_are_the_search_or_new_tab() {
        assert_eq!(title(""), "New Tab");
        assert_eq!(title("  "), "New Tab");
        assert_eq!(title("*.png|*.jpg"), "*.png|*.jpg");
        assert_eq!(title("a\r\nb"), "a b");
    }

    #[test]
    fn settings_read_back_as_written_with_everythings_defaults() {
        let d = TabSettings::default();
        assert_eq!(TabSettings::from_keys(|_| None), d);
        let keys = d.to_keys();
        let get = |k: &str| {
            keys.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(TabSettings::from_keys(get), d);
        for (key, value) in [
            ("tabs_show", "1"),
            ("tabs_always_show", "0"),
            ("tabs_min_wide", "76"),
            ("tabs_max_wide", "224"),
            ("tabs_close_button", "1"),
            ("tabs_insert_after_current", "0"),
            ("tabs_double_click_action", "0"),
            ("tabs_close_window_with_last_tab", "1"),
            ("ctrl_tab_action", "1"),
            ("session_store_max_tabs_undo", "24"),
            ("offset_new_window_position", "1"),
            ("offset_new_window_position_x", "22"),
            ("offset_new_window_position_y", "22"),
        ] {
            assert_eq!(get(key), Some(value), "{key}");
        }
        let changed = TabSettings {
            always_show: true,
            min_wide: 100,
            max_wide: 150,
            close_button: false,
            insert_after_current: true,
            double_click_action: 1,
            close_window_with_last_tab: false,
            ctrl_tab_action: 2,
            max_closed: 3,
            offset_x: -10,
            ..d
        };
        let keys = changed.to_keys();
        let get = |k: &str| {
            keys.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        let back = TabSettings::from_keys(get);
        assert_eq!(back, changed);
        assert!(back.double_click_closes() && !back.ctrl_tab_cycles());
    }

    #[test]
    fn settings_out_of_range_take_the_default() {
        let bad = [
            ("tabs_min_wide", "0"),
            ("tabs_max_wide", "50"),
            ("session_store_max_tabs_undo", "100000"),
            ("tabs_show", "maybe"),
            ("offset_new_window_position_x", "99999"),
        ];
        let get = |k: &str| bad.iter().find(|(key, _)| *key == k).map(|(_, v)| *v);
        let s = TabSettings::from_keys(get);
        let d = TabSettings::default();
        assert_eq!(
            (s.min_wide, s.show, s.offset_x),
            (d.min_wide, d.show, d.offset_x)
        );
        assert_eq!(s.max_closed, d.max_closed);
        // A maximum under the minimum is the minimum.
        assert_eq!(s.max_wide, d.min_wide);
    }

    #[test]
    fn the_strip_shows_with_two_tabs_or_when_asked() {
        let d = TabSettings::default();
        assert!(!d.strip_shown(1));
        assert!(d.strip_shown(2));
        let always = TabSettings {
            always_show: true,
            ..d
        };
        assert!(always.strip_shown(1));
        let hidden = TabSettings {
            show: false,
            always_show: true,
            ..d
        };
        assert!(!hidden.strip_shown(5));
    }

    #[test]
    fn inserting_keeps_the_shown_tab() {
        let mut l = TabList::new("a", 24);
        assert_eq!(l.insert(9, "b"), 1, "at the end");
        assert_eq!(l.current(), 0);
        l.select(1);
        assert_eq!(l.insert(0, "z"), 0);
        assert_eq!(l.current(), 2, "b moved right");
        assert_eq!(l.tabs(), ["z", "a", "b"]);
        assert!(!l.select(3));
        assert_eq!(l.get(2), Some(&"b"));
        l.set(2, "B");
        assert_eq!(l.tabs()[2], "B");
    }

    #[test]
    fn stepping_goes_round() {
        let mut l = list(3);
        assert_eq!(l.step(true), 1);
        assert_eq!(l.step(false), 2);
        l.select(2);
        assert_eq!(l.step(true), 0);
        let one = TabList::new(0, 24);
        assert_eq!((one.step(true), one.step(false)), (0, 0));
    }

    #[test]
    fn closing_shows_the_neighbour_and_keeps_the_tab() {
        let mut l = list(4);
        l.select(1);
        // The shown tab: the one right of it takes its place.
        assert_eq!(l.close(1), Some(1));
        assert_eq!(l.tabs(), [0, 2, 3]);
        // The last, shown: the one left of it.
        l.select(2);
        assert_eq!(l.close(2), Some(1));
        assert_eq!(l.tabs(), [0, 2]);
        // One left of the shown: the shown one moves left.
        assert_eq!(l.close(0), Some(0));
        assert_eq!((l.tabs(), l.current()), (&[2][..], 0));
        // Never the only tab.
        assert_eq!(l.close(0), None);
        // Newest first: 0, then 3, then 1.
        assert_eq!(
            l.closed_newest_first().copied().collect::<Vec<_>>(),
            [0, 3, 1]
        );
        let back = l.take_closed(0).expect("closed");
        assert_eq!((back.index, back.tab), (0, 0));
        assert_eq!(l.take_closed(5), None);
        assert_eq!(l.closed_count(), 2);
    }

    #[test]
    fn closing_a_tab_beside_the_shown_one_keeps_it_shown() {
        let mut l = list(3);
        l.select(0);
        assert_eq!(l.close(2), Some(0));
        assert_eq!(l.after_close(1), 0);
        l.select(1);
        assert_eq!(l.after_close(0), 0);
    }

    #[test]
    fn only_so_many_closed_tabs_are_kept() {
        let mut l = TabList::new(0, 2);
        for i in 1..5 {
            l.insert(i, i);
        }
        for _ in 0..3 {
            l.close(0);
        }
        assert_eq!(l.closed_newest_first().copied().collect::<Vec<_>>(), [2, 1]);
        let mut none = TabList::new(0, 0);
        none.insert(1, 1);
        none.close(1);
        assert_eq!(none.closed_count(), 0);
        none.push_closed(0, 7);
        assert_eq!(none.closed_count(), 0);
    }

    #[test]
    fn close_multiple_tabs_picks_the_others() {
        let l = list(5);
        assert_eq!(l.others(2, Others::Left), [1, 0]);
        assert_eq!(l.others(2, Others::Right), [4, 3]);
        assert_eq!(l.others(2, Others::All), [4, 3, 1, 0]);
        assert!(l.others(0, Others::Left).is_empty());
        assert!(l.others(4, Others::Right).is_empty());
        assert!(l.others(9, Others::All).is_empty());
        // Right to left, each closed in turn leaves the rest where they are.
        let mut m = list(5);
        for i in m.others(2, Others::All) {
            m.close(i);
        }
        assert_eq!(m.tabs(), [2]);
    }

    #[test]
    fn a_dragged_tab_moves_and_the_shown_one_follows() {
        let mut l = list(4);
        l.select(1);
        l.move_tab(1, 3);
        assert_eq!((l.tabs(), l.current()), (&[0, 2, 3, 1][..], 3));
        // Another tab dragged past the shown one.
        l.move_tab(0, 3);
        assert_eq!((l.tabs(), l.current()), (&[2, 3, 1, 0][..], 2));
        l.move_tab(3, 0);
        assert_eq!((l.tabs(), l.current()), (&[0, 2, 3, 1][..], 3));
        // Out of range or in place: nothing.
        l.move_tab(0, 9);
        l.move_tab(2, 2);
        assert_eq!(l.tabs(), [0, 2, 3, 1]);
    }

    #[test]
    fn a_session_replaces_the_tabs_which_can_be_reopened() {
        let mut l = list(3);
        l.close(2);
        l.replace(vec![10, 11, 12], 7, vec![20, 21]);
        assert_eq!((l.tabs(), l.current()), (&[10, 11, 12][..], 2));
        // The window's tabs first, then what it had closed, then the
        // session's own.
        assert_eq!(
            l.closed_newest_first().copied().collect::<Vec<_>>(),
            [1, 0, 2, 21, 20]
        );
        l.replace(Vec::new(), 0, vec![30]);
        assert_eq!(l.len(), 3);
        assert_eq!(l.closed_count(), 5);
        let parts = TabList::from_parts(vec![1, 2], 5, vec![7, 8, 9], 2).expect("tabs");
        assert_eq!((parts.tabs(), parts.current()), (&[1, 2][..], 1));
        assert_eq!(parts.closed().copied().collect::<Vec<_>>(), [8, 9]);
        assert!(TabList::<u8>::from_parts(Vec::new(), 0, Vec::new(), 2).is_none());
    }

    fn metrics(width: i32) -> Metrics {
        Metrics {
            left: 3,
            right: width - 3,
            top: 3,
            bottom: 29,
            min: 76,
            max: 224,
            button: 26,
            close: 16,
            close_inset: 13,
        }
    }

    #[test]
    fn tabs_share_the_room_between_the_least_and_most_width() {
        // Everything's window, 1384 px: two tabs at 224, "+" after them.
        let p = place(2, 1, 0, &metrics(1384));
        assert_eq!(p.tabs, [(0, 3, 227), (1, 227, 451)]);
        assert_eq!((p.plus, p.overflow, p.first), ((451, 477), None, 0));
        // Ten tabs in 1000 px: (994 - 26) / 10 = 96 each.
        let p = place(10, 0, 0, &metrics(1000));
        assert!(p.tabs.iter().all(|&(_, l, r)| r - l == 96));
        assert_eq!(p.plus, (963, 989));
        assert_eq!(p.overflow, None);
    }

    #[test]
    fn too_many_tabs_scroll_with_the_shown_one_in_view() {
        // 20 tabs in 800 px: (794 - 52) / 76 = 9 fit, at 82 each.
        let m = metrics(800);
        let p = place(20, 0, 0, &m);
        assert_eq!(p.tabs.len(), 9);
        assert!(p.tabs.iter().all(|&(_, l, r)| r - l == 82));
        assert_eq!((p.tabs[0].0, p.first), (0, 0));
        assert_eq!(p.overflow, Some((771, 797)));
        assert_eq!(p.plus, (3 + 9 * 82, 3 + 9 * 82 + 26));
        // The shown tab far right: it is the last in view.
        let p = place(20, 15, 0, &m);
        assert_eq!((p.first, p.tabs.last().map(|t| t.0)), (7, Some(15)));
        // Scrolled past it to the left: it is the first in view.
        let p = place(20, 3, 7, &m);
        assert_eq!(p.first, 3);
        // A scroll past the end comes back.
        let p = place(20, 19, 50, &m);
        assert_eq!((p.first, p.tabs.last().map(|t| t.0)), (11, Some(19)));
        // A window too narrow for even one: one shows.
        let p = place(3, 2, 0, &metrics(60));
        assert_eq!(p.tabs.len(), 1);
        assert_eq!(p.tabs[0].0, 2);
        assert_eq!(place(0, 0, 0, &m), Placed::default());
    }

    #[test]
    fn the_strip_says_what_is_under_the_mouse() {
        let m = metrics(1384);
        let p = place(2, 0, 0, &m);
        assert_eq!(hit(&p, &m, 10, 15), Hit::Tab(0));
        assert_eq!(hit(&p, &m, 300, 15), Hit::Tab(1));
        // The × 13 px in from the right, 16 px square, in the row's middle.
        let (l, t, r, b) = close_box(227, &m);
        assert_eq!((l, t, r, b), (206, 8, 222, 24));
        assert_eq!(hit(&p, &m, 214, 16), Hit::Close(0));
        assert_eq!(hit(&p, &m, 214, 5), Hit::Tab(0));
        assert_eq!(hit(&p, &m, 460, 16), Hit::Plus);
        assert_eq!(hit(&p, &m, 900, 16), Hit::Empty);
        // Above the tabs: the strip's margin.
        assert_eq!(hit(&p, &m, 10, 1), Hit::Empty);
        let none = Metrics { close: 0, ..m };
        assert_eq!(hit(&p, &none, 214, 16), Hit::Tab(0));
        let p = place(20, 0, 0, &metrics(800));
        assert_eq!(hit(&p, &m, 780, 16), Hit::Overflow);
    }

    #[test]
    fn a_drag_drops_on_the_tab_under_it() {
        let m = metrics(1384);
        let p = place(3, 0, 0, &m);
        assert_eq!(drop_slot(&p, 0), Some(0));
        assert_eq!(drop_slot(&p, 300), Some(1));
        assert_eq!(drop_slot(&p, 1300), Some(2));
        assert_eq!(drop_slot(&Placed::default(), 5), None);
    }

    #[test]
    fn arguments_are_quoted_as_the_c_runtime_reads_them() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("two words"), "\"two words\"");
        assert_eq!(
            quote(r"C:\Program Files\goz\"),
            r#""C:\Program Files\goz\\""#
        );
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(quote(r"a\b c"), r#""a\b c""#);
    }

    #[test]
    fn a_new_window_starts_where_it_is_asked_to() {
        let args = new_window_args(Some((122, -5, 1000, 640)), Some("work x"));
        assert_eq!(
            args,
            [
                "-new-window",
                "-x",
                "122",
                "-y",
                "-5",
                "-width",
                "1000",
                "-height",
                "640",
                "-instance",
                "work x"
            ]
        );
        assert_eq!(
            command_line(r"C:\a b\goz-gui.exe", &args[..3]),
            r#""C:\a b\goz-gui.exe" -new-window -x 122"#
        );
        assert_eq!(new_window_args(Some((0, 0, 1, 1)), None).len(), 9);
        assert_eq!(new_window_args(None, None), ["-new-window"]);
        // What the window reads back.
        let l = crate::cmdline::parse(&args).expect("parsed");
        assert!(l.new_window);
        assert_eq!(l.place_over(None), Some((122, -5, 1000, 640)));
        assert_eq!(l.instance.as_deref(), Some("work x"));
    }
}
