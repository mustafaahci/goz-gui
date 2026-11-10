//! Options > Home: what a new window starts with and History > Home (Alt+Home)
//! brings back, each "(Use last value)" or a fixed one, with Everything's
//! labels, ids and keys (`home_match_case`, `home_match_path`,
//! `home_search`, `home_filter`, `home_sort`, `home_view`; a fixed value is
//! the plain key's, see `crate::settings`). Everything's other Home rows
//! (diacritics, whole words, prefix, suffix, punctuation, white-space,
//! regular expressions, columns, index) are options goz has not: not here.

use std::cell::RefCell;

use goz_core::types::{SortDir, SortKey, SortSpec};
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CBN_SELCHANGE, CBS_DROPDOWNLIST, ES_AUTOHSCROLL, WM_COMMAND, WS_EX_CLIENTEDGE, WS_TABSTOP,
    WS_VSCROLL,
};

use crate::bookmarks::{SORT_KEYS, one_line, sort_name};
use crate::dialog::Class;
use crate::filter::Filter;
use crate::options::{
    LABEL, Page, Rows, combo_value, enable, fill_combo, order, select_combo, set_text, text,
};
use crate::settings::{HomeFixed, HomeOption, Settings};
use crate::view::{self, View};
use crate::window::App;

/// Everything's control ids.
const MATCH_CASE: u16 = 1100;
const MATCH_PATH: u16 = 1102;
const SEARCH: u16 = 1105;
const SEARCH_TEXT: u16 = 1106;
const FILTER: u16 = 1107;
const SORT: u16 = 1108;
const SORT_DIR: u16 = 1369;
const VIEW: u16 = 1109;

/// The labels' width, and the lists', in dialog units.
const LABEL_W: i16 = 62;
const LIST_W: i16 = 80;

const LAST: &str = "(Use last value)";

/// Match Case and Match Path: Everything's three (`0`, `2`, `3`).
const MATCH: [(HomeOption, &str); 3] = [
    (HomeOption::Last, LAST),
    (HomeOption::On, "Enabled"),
    (HomeOption::Off, "Disabled"),
];

/// The search: the last one, or the text beside the list.
const SEARCHES: [(bool, &str); 2] = [(false, LAST), (true, "Custom")];

const DIRS: [(SortDir, &str); 2] = [(SortDir::Asc, "Ascending"), (SortDir::Desc, "Descending")];

/// The views goz-gui has, as Everything's list names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HomeView {
    Last,
    Details,
    Thumbnails(u32),
}

const VIEWS: [(HomeView, &str); 5] = [
    (HomeView::Last, LAST),
    (HomeView::Details, "Details"),
    (
        HomeView::Thumbnails(view::MEDIUM),
        "Thumbnails (64 x 64) (Medium)",
    ),
    (
        HomeView::Thumbnails(view::LARGE),
        "Thumbnails (128 x 128) (Large)",
    ),
    (
        HomeView::Thumbnails(view::EXTRA_LARGE),
        "Thumbnails (256 x 256) (Extra Large)",
    ),
];

/// The filters, after "(Use last value)": the built-in ones, then the
/// user's, as the Search menu and the filter bar list them.
fn filters() -> Vec<(Option<Filter>, &'static str)> {
    let mut out = vec![(None, LAST)];
    out.extend(Filter::all().into_iter().map(|f| (Some(f), f.title())));
    out
}

/// The sorts gozd has, after "(Use last value)".
fn sorts() -> Vec<(Option<SortKey>, &'static str)> {
    let mut out = vec![(None, LAST)];
    out.extend(SORT_KEYS.iter().map(|&k| (Some(k), sort_name(k))));
    out
}

/// What a match option's list shows for `option`: a fixed one (`1`, the
/// plain key's) as the value it fixes.
fn match_choice(option: HomeOption, plain: bool) -> HomeOption {
    match option {
        HomeOption::Custom if plain => HomeOption::On,
        HomeOption::Custom => HomeOption::Off,
        o => o,
    }
}

/// The view a Home view list shows for settings `s`: a size between
/// Everything's three (from a hand-edited file) as the nearest.
fn view_choice(s: &Settings) -> HomeView {
    if !s.home.view {
        HomeView::Last
    } else if !s.view.thumbnails {
        HomeView::Details
    } else {
        let size = s.view.size;
        let nearest = [view::MEDIUM, view::LARGE, view::EXTRA_LARGE]
            .into_iter()
            .min_by_key(|&v| v.abs_diff(size))
            .unwrap_or(view::LARGE);
        HomeView::Thumbnails(nearest)
    }
}

/// What the page shows, read from settings `s` (or the defaults).
struct Shown {
    case: HomeOption,
    path: HomeOption,
    search: bool,
    text: String,
    filter: Option<Filter>,
    sort: Option<SortKey>,
    dir: SortDir,
    view: HomeView,
}

impl Shown {
    fn of(s: &Settings) -> Shown {
        let h = &s.home;
        Shown {
            case: match_choice(h.match_case, s.match_case),
            path: match_choice(h.match_path, s.match_path),
            search: h.search,
            text: if h.search {
                one_line(&s.search)
            } else {
                String::new()
            },
            filter: h.filter.then_some(s.filter),
            sort: h.sort.then_some(s.sort.key),
            dir: s.sort.dir,
            view: view_choice(s),
        }
    }

    /// Shows these on the page, whose Filter list holds `filters`.
    fn put(&self, page: HWND, filters: &[(Option<Filter>, &'static str)]) {
        select_combo(page, MATCH_CASE, &MATCH, self.case);
        select_combo(page, MATCH_PATH, &MATCH, self.path);
        select_combo(page, SEARCH, &SEARCHES, self.search);
        set_text(page, SEARCH_TEXT, &self.text);
        select_combo(page, FILTER, filters, self.filter);
        select_combo(page, SORT, &sorts(), self.sort);
        select_combo(page, SORT_DIR, &DIRS, self.dir);
        select_combo(page, VIEW, &VIEWS, self.view);
    }
}

/// Sets `s` (the settings as read, which hold Home's fixed values) to what
/// the page shows.
fn set_home(s: &mut Settings, shown: &Shown) {
    let fixed = |o: HomeOption| match o {
        HomeOption::On | HomeOption::Off => o,
        _ => HomeOption::Last,
    };
    s.home.match_case = fixed(shown.case);
    s.home.match_path = fixed(shown.path);
    s.home.search = shown.search;
    if shown.search {
        s.search = one_line(&shown.text);
    }
    s.home.filter = shown.filter.is_some();
    if let Some(f) = shown.filter {
        s.filter = f;
    }
    s.home.sort = shown.sort.is_some();
    if let Some(key) = shown.sort {
        s.sort = SortSpec {
            key,
            dir: shown.dir,
        };
    }
    match shown.view {
        HomeView::Last => s.home.view = false,
        HomeView::Details => {
            s.home.view = true;
            s.view = s.view.details();
        }
        HomeView::Thumbnails(size) => {
            s.home.view = true;
            s.view = View::thumbnails(size);
        }
    }
}

pub(super) struct Home {
    /// The Filter list's filters as it was filled: what is read back by
    /// their place in it names what it showed, even if the user's filters
    /// change meanwhile (another goz-gui's Organize Filters).
    filters: RefCell<Vec<(Option<Filter>, &'static str)>>,
}

impl Home {
    pub(super) fn new() -> Home {
        Home {
            filters: RefCell::new(filters()),
        }
    }
}

/// A label `LABEL_W` wide and a drop-down list `w` wide right of it, at
/// `x` from the row's start; the row does not move on.
fn list(rows: &mut Rows<'_>, label: Option<&str>, id: u16, x: i16, w: i16) {
    if let Some(label) = label {
        rows.control(Class::Static, label, LABEL, SS_LEFT, 0, (0, 2, LABEL_W, 9));
    }
    rows.control(
        Class::Named("COMBOBOX"),
        "",
        id,
        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        0,
        (x, 0, w, 120),
    );
}

impl Page for Home {
    fn name(&self) -> &'static str {
        "Home"
    }

    fn parent(&self) -> Option<&'static str> {
        Some("General")
    }

    fn order(&self) -> u32 {
        order::HOME
    }

    fn build(&self, rows: &mut Rows<'_>) {
        rows.combo("Match &case:", MATCH_CASE, LABEL_W, LIST_W)
            .combo("Match &path:", MATCH_PATH, LABEL_W, LIST_W);
        let rest = rows.width() - LABEL_W - LIST_W - 4;
        list(rows, Some("&Search:"), SEARCH, LABEL_W, LIST_W);
        rows.control(
            Class::Edit,
            "",
            SEARCH_TEXT,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (LABEL_W + LIST_W + 4, 0, rest, 13),
        )
        .skip(17);
        rows.combo("&Filter:", FILTER, LABEL_W, LIST_W);
        list(rows, Some("Sor&t:"), SORT, LABEL_W, LIST_W);
        list(rows, None, SORT_DIR, LABEL_W + LIST_W + 4, 64);
        rows.skip(17)
            .combo("&View:", VIEW, LABEL_W, LIST_W + 4 + rest);
    }

    fn fill(&self, app: &App, page: HWND) {
        fill_combo(page, MATCH_CASE, &MATCH, HomeOption::Last);
        fill_combo(page, MATCH_PATH, &MATCH, HomeOption::Last);
        fill_combo(page, SEARCH, &SEARCHES, false);
        let filters = filters();
        fill_combo(page, FILTER, &filters, None);
        fill_combo(page, SORT, &sorts(), None);
        fill_combo(page, SORT_DIR, &DIRS, SortDir::Asc);
        fill_combo(page, VIEW, &VIEWS, HomeView::Last);
        let shown = Shown::of(&app.prefs.borrow().base);
        shown.put(page, &filters);
        *self.filters.borrow_mut() = filters;
    }

    /// The search's text with "Custom", the direction with a sort.
    fn update_enabled(&self, page: HWND) {
        enable(page, SEARCH_TEXT, combo_value(page, SEARCH, &SEARCHES));
        enable(page, SORT_DIR, combo_value(page, SORT, &sorts()).is_some());
    }

    /// A sort chosen starts in its own direction (names A to Z, sizes and
    /// dates largest and newest first), as a click on its column would.
    fn message(&self, app: &App, page: HWND, msg: u32, w: WPARAM, _l: LPARAM) -> Option<isize> {
        if msg == WM_COMMAND
            && (w & 0xffff) as u16 == SORT
            && ((w >> 16) & 0xffff) as u32 == CBN_SELCHANGE
            && let Some(key) = combo_value(page, SORT, &sorts())
        {
            select_combo(page, SORT_DIR, &DIRS, app.first_sort(key).dir);
        }
        None
    }

    fn apply(&self, app: &App, page: HWND, _problems: &mut Vec<String>) {
        let shown = Shown {
            case: combo_value(page, MATCH_CASE, &MATCH),
            path: combo_value(page, MATCH_PATH, &MATCH),
            search: combo_value(page, SEARCH, &SEARCHES),
            text: text(page, SEARCH_TEXT),
            filter: combo_value(page, FILTER, &self.filters.borrow()),
            sort: combo_value(page, SORT, &sorts()),
            dir: combo_value(page, SORT_DIR, &DIRS),
            view: combo_value(page, VIEW, &VIEWS),
        };
        set_home(&mut app.prefs.borrow_mut().base, &shown);
    }

    /// Everything's: the empty search, the rest the last value.
    fn defaults(&self, page: HWND) {
        debug_assert_eq!(Settings::default().home, HomeFixed::default());
        Shown::of(&Settings::default()).put(page, &self.filters.borrow());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ini::Ini;

    fn read(text: &str) -> Settings {
        Settings::from_ini(&Ini::from_text(text))
    }

    #[test]
    fn the_page_shows_what_the_keys_say() {
        let s = read(
            "home_match_case=1\r\nmatch_case=1\r\nhome_match_path=3\r\nhome_search=1\r\n\
             search=*.mp3\r\nhome_filter=1\r\nfilter=Audio\r\nhome_sort=1\r\nsort=Size\r\n\
             sort_ascending=1\r\nhome_view=1\r\nview=1\r\nthumbnail_size=120\r\n",
        );
        let shown = Shown::of(&s);
        // 1 (the plain key's value) shows as the value it fixes.
        assert_eq!(shown.case, HomeOption::On);
        assert_eq!(shown.path, HomeOption::Off);
        assert!(shown.search);
        assert_eq!(shown.text, "*.mp3");
        assert_eq!(shown.filter, Some(Filter::Audio));
        assert_eq!((shown.sort, shown.dir), (Some(SortKey::Size), SortDir::Asc));
        assert_eq!(shown.view, HomeView::Thumbnails(view::LARGE));
        // Everything's defaults.
        let d = Shown::of(&read(""));
        assert_eq!((d.case, d.path), (HomeOption::Last, HomeOption::Last));
        assert!(d.search && d.text.is_empty());
        assert_eq!((d.filter, d.sort, d.view), (None, None, HomeView::Last));
    }

    #[test]
    fn what_the_page_sets_reads_back() {
        let shown = Shown {
            case: HomeOption::On,
            path: HomeOption::Last,
            search: true,
            text: "report".into(),
            filter: Some(Filter::Document),
            sort: Some(SortKey::DateModified),
            dir: SortDir::Desc,
            view: HomeView::Details,
        };
        let mut s = read("view=1\r\nthumbnail_size=256\r\n");
        set_home(&mut s, &shown);
        let mut ini = Ini::default();
        for (k, v) in s.to_pairs() {
            ini.set(&k, &v);
        }
        let back = Settings::from_ini(&ini);
        let again = Shown::of(&back);
        assert_eq!(again.case, HomeOption::On);
        assert_eq!(
            back.home.match_case,
            HomeOption::On,
            "written as Everything's 2"
        );
        assert_eq!(again.path, HomeOption::Last);
        assert_eq!(again.text, "report");
        assert_eq!(again.filter, Some(Filter::Document));
        assert_eq!(
            (again.sort, again.dir),
            (Some(SortKey::DateModified), SortDir::Desc)
        );
        assert_eq!(again.view, HomeView::Details);
        // Details keeps the thumbnails' size for the status bar's button.
        assert_eq!(back.view.size, view::EXTRA_LARGE);
        // History > Home then brings them back.
        let home = back.home();
        assert_eq!(home.search.as_deref(), Some("report"));
        assert_eq!(home.options.match_case, Some(true));
        assert_eq!(home.options.match_path, None);
        assert_eq!(home.view, Some(back.view.details()));
        // "(Use last value)" for all: nothing fixed.
        let none = Shown {
            case: HomeOption::Last,
            path: HomeOption::Last,
            search: false,
            text: String::new(),
            filter: None,
            sort: None,
            dir: SortDir::Asc,
            view: HomeView::Last,
        };
        set_home(&mut s, &none);
        for key in ["search", "filter", "sort", "match_case", "view"] {
            assert!(!s.is_fixed(key), "{key}");
        }
    }
}
