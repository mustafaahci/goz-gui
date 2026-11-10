//! When a search counts as made: the search history (what the search box's
//! dropdown lists, kept in `Search History.csv` by `searches.rs`) and the
//! navigation history (History > Back, Forward, Home and the History menu's
//! entries).
//!
//! A search goes into the search history when Enter is pressed in the box,
//! when a result is acted on, or when the results list gets the keyboard
//! focus; once per change of the text, so the count is of searches made, not
//! of keys pressed. The same moments, and the search staying unchanged for
//! [`NAV_DELAY_MS`], make the search on screen (its text, options, filter
//! and sort) a navigation entry. Going back, forward or to an entry restores
//! that state without adding one.

use goz_core::types::{SortDir, SortSpec};
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW;

use super::{App, Options, send};
use crate::bookmarks::{one_line, sort_name};
use crate::session::Search;
use crate::wide::{window_text, wz};
use crate::{dropdown, menu, searchbox};

/// How long a search must stay as it is to become a navigation entry.
pub(super) const NAV_DELAY_MS: u32 = 1500;

/// A search as Back and Forward bring it back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NavState {
    text: String,
    opts: Options,
    /// The Folders sidebar's folder (`sidebars.rs`), which Back and Forward
    /// bring back with the search.
    folder: Option<String>,
}

impl NavState {
    /// As a saved session holds it.
    pub(super) fn to_search(&self) -> Search {
        let o = self.opts;
        Search {
            text: self.text.clone(),
            match_case: o.match_case,
            match_path: o.match_path,
            filter: o.filter,
            sort: o.sort,
            folder: self.folder.clone(),
        }
    }

    pub(super) fn from_search(s: &Search) -> NavState {
        NavState {
            text: s.text.clone(),
            opts: Options {
                match_case: s.match_case,
                match_path: s.match_path,
                filter: s.filter,
                sort: s.sort,
            },
            folder: s.folder.clone(),
        }
    }
}

impl App {
    /// The search on screen.
    pub(super) fn nav_state(&self) -> NavState {
        NavState {
            text: window_text(self.edit),
            opts: self.opts.get(),
            folder: self.folder_selection(),
        }
    }

    /// Notes the search in the box in the search history, once per change of
    /// its text (an empty one never), and makes it a navigation entry; the
    /// search counts as made.
    pub(super) fn record_search(&self) {
        // Text typed and not searched yet (Options > Search) is not the
        // search on screen: it counts once it is searched.
        if self.search_is_dirty() {
            return;
        }
        if !self.recorded.replace(true) {
            self.record_in_history(&window_text(self.edit));
        }
        self.commit_nav();
        self.settle_search();
    }

    /// Makes the search on screen a navigation entry, unless it is the
    /// current entry already.
    pub(super) fn commit_nav(&self) {
        let state = self.nav_state();
        if self.nav.borrow_mut().commit(state) {
            searchbox::invalidate_nav(self);
        }
    }

    /// The navigation entries' labels for the History menu, newest first,
    /// and which one is current.
    pub(super) fn history_entries(&self) -> (Vec<String>, Option<usize>) {
        let nav = self.nav.borrow();
        let (entries, current) = nav.newest_first(menu::NAV_ENTRIES);
        (entry_labels(&entries), current)
    }

    /// Back is possible to an older entry, or to the current one from a
    /// search changed since it (which Back first makes an entry).
    pub(super) fn can_go_back(&self) -> bool {
        let state = self.nav_state();
        let nav = self.nav.borrow();
        nav.can_back() || nav.current().is_some_and(|c| *c != state)
    }

    pub(super) fn can_go_forward(&self) -> bool {
        let state = self.nav_state();
        let nav = self.nav.borrow();
        nav.can_forward() && nav.current() == Some(&state)
    }

    /// Whether the nav bar's Back (`back`) or Forward button has somewhere
    /// to go.
    pub fn can_navigate(&self, back: bool) -> bool {
        if back {
            self.can_go_back()
        } else {
            self.can_go_forward()
        }
    }

    pub(super) fn go_back(&self) {
        // A search not yet an entry becomes one, so Forward comes back to it.
        self.commit_nav();
        let state = self.nav.borrow_mut().back().cloned();
        if let Some(state) = state {
            self.restore(&state);
        }
    }

    pub(super) fn go_forward(&self) {
        if !self.can_go_forward() {
            return;
        }
        let state = self.nav.borrow_mut().forward().cloned();
        if let Some(state) = state {
            self.restore(&state);
        }
    }

    /// History menu entry `index` of `history_entries`; the menu made the
    /// search on screen an entry as it opened, so the indices hold.
    pub(super) fn go_to_history(&self, index: usize) {
        let state = self.nav.borrow_mut().go_newest_first(index).cloned();
        if let Some(state) = state {
            self.restore(&state);
        }
    }

    /// History > Home: the Home search and options (Everything's Options >
    /// Home, from the settings file); each one Home leaves to the last
    /// value stays as it is. By default that is the empty search, with
    /// match case, match path, the filter and the sort unchanged.
    pub(super) fn go_home(&self) {
        let home = self.prefs.borrow().base.home();
        let text = home.search.unwrap_or_else(|| window_text(self.edit));
        let o = self.opts.get();
        let h = home.options;
        self.navigate(
            &text,
            Options {
                match_case: h.match_case.unwrap_or(o.match_case),
                match_path: h.match_path.unwrap_or(o.match_path),
                filter: h.filter.unwrap_or(o.filter),
                sort: h.sort.unwrap_or(o.sort),
            },
        );
        // Options > Home's View: Details, or thumbnails at a size.
        if let Some(view) = home.view
            && view != self.view()
        {
            self.set_view(view);
        }
    }

    /// Shows `text` with `opts` as a new navigation entry after the search on
    /// screen, which becomes one first, so Back returns to it. Home uses it;
    /// so can a bookmark (M1 WP5).
    pub(super) fn navigate(&self, text: &str, opts: Options) {
        // The Folders sidebar's folder stays as it is.
        self.navigate_to(text, opts, self.folder_selection());
    }

    /// [`navigate`](Self::navigate) with the Folders sidebar's `folder`
    /// (`None`: none), for a search that names its own folder (`-path`).
    pub(super) fn navigate_to(&self, text: &str, opts: Options, folder: Option<String>) {
        self.commit_nav();
        self.restore(&NavState {
            text: text.to_owned(),
            opts,
            folder,
        });
        self.commit_nav();
    }

    /// Shows `state`'s search, options, filter, sort and folder.
    pub(super) fn restore(&self, state: &NavState) {
        dropdown::close(self);
        self.restore_folder(state.folder.clone());
        self.opts.set(state.opts);
        self.show_sort();
        self.invalidate_status();
        self.set_search_text(&state.text);
    }

    /// Puts `text` in the search box with the caret at its end, and searches
    /// for it (with the options in force) even when the text is unchanged.
    /// A search of several lines (one of Everything's, from an imported
    /// history or session) goes in as one line, its lines searched as one,
    /// as a bookmark's are: the box shows one line.
    pub(super) fn set_search_text(&self, text: &str) {
        let text = one_line(text);
        if window_text(self.edit) == text {
            self.start_search(false);
        } else {
            // SAFETY: a NUL-terminated string for this thread's search box;
            // the change searches through EN_CHANGE.
            unsafe { SetWindowTextW(self.edit, wz(&text).as_ptr()) };
        }
        let end = text.encode_utf16().count();
        send(self.edit, EM_SETSEL, end, end as isize);
    }
}

/// The sort as a History menu label shows it: `Size ↓`.
fn sort_label(sort: SortSpec) -> String {
    let arrow = match sort.dir {
        SortDir::Asc => '↑',
        SortDir::Desc => '↓',
    };
    format!("{} {arrow}", sort_name(sort.key))
}

/// The History menu's label of each entry: its search (`(Empty)` for none),
/// and, when another listed entry has the same search with other options,
/// what tells them apart: the sort when the sorts differ, and Match Case,
/// Match Path, the filter or the Folders sidebar's folder when they are on
/// and differ (`*.exe  (Size ↓)`, `*.exe  (C:\Windows)`).
fn entry_labels(entries: &[&NavState]) -> Vec<String> {
    entries
        .iter()
        .map(|e| {
            let same: Vec<&NavState> = entries
                .iter()
                .copied()
                .filter(|o| o.text == e.text)
                .collect();
            let o = e.opts;
            let mut parts = Vec::new();
            if same.iter().any(|s| s.opts.sort != o.sort) {
                parts.push(sort_label(o.sort));
            }
            if o.match_case && same.iter().any(|s| !s.opts.match_case) {
                parts.push("Match Case".to_owned());
            }
            if o.match_path && same.iter().any(|s| !s.opts.match_path) {
                parts.push("Match Path".to_owned());
            }
            if let Some(name) = o.filter.name()
                && same.iter().any(|s| s.opts.filter != o.filter)
            {
                parts.push(name.to_owned());
            }
            if let Some(folder) = &e.folder
                && same.iter().any(|s| s.folder != e.folder)
            {
                parts.push(folder.clone());
            }
            let text = if e.text.is_empty() {
                "(Empty)"
            } else {
                &e.text
            };
            if parts.is_empty() {
                text.to_owned()
            } else {
                format!("{text}  ({})", parts.join(", "))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use goz_core::types::SortKey;

    use super::*;
    use crate::filter::Filter;
    use crate::history::NavHistory;

    #[test]
    fn every_option_makes_another_entry() {
        let base = NavState {
            text: "foo".into(),
            opts: Options::default(),
            folder: None,
        };
        let mut n = NavHistory::default();
        assert!(n.commit(base.clone()));
        assert!(!n.commit(base.clone()));
        let variants = [
            Options {
                match_case: true,
                ..Options::default()
            },
            Options {
                match_path: true,
                ..Options::default()
            },
            Options {
                filter: Filter::Audio,
                ..Options::default()
            },
            Options {
                sort: SortSpec::default_for(SortKey::Size),
                ..Options::default()
            },
        ];
        for opts in variants {
            assert!(n.commit(NavState {
                opts,
                ..base.clone()
            }));
        }
        // So does the Folders sidebar's folder.
        assert!(n.commit(NavState {
            folder: Some(r"C:\Windows".into()),
            ..base.clone()
        }));
        // Home is the empty search with the default options.
        let home = NavState::default();
        assert_eq!(home.text, "");
        assert_eq!(home.opts, Options::default());
    }

    fn state(text: &str, opts: Options) -> NavState {
        NavState {
            text: text.into(),
            opts,
            folder: None,
        }
    }

    #[test]
    fn the_folder_goes_to_a_session_and_back_and_tells_entries_apart() {
        let s = NavState {
            folder: Some(r"D:\Music".into()),
            ..state("live", Options::default())
        };
        let search = s.to_search();
        assert_eq!(search.folder.as_deref(), Some(r"D:\Music"));
        assert_eq!(NavState::from_search(&search), s);
        let entries = [s.clone(), state("live", Options::default())];
        let refs: Vec<&NavState> = entries.iter().collect();
        assert_eq!(entry_labels(&refs), [r"live  (D:\Music)", "live"]);
    }

    #[test]
    fn entries_go_to_a_session_and_back() {
        let s = state(
            "*.mp3",
            Options {
                match_case: true,
                match_path: false,
                filter: Filter::Audio,
                sort: SortSpec {
                    key: SortKey::Size,
                    dir: SortDir::Asc,
                },
            },
        );
        let search = s.to_search();
        assert_eq!(search.text, "*.mp3");
        assert!(search.match_case && !search.match_path);
        assert_eq!((search.filter, search.sort), (Filter::Audio, s.opts.sort));
        assert_eq!(NavState::from_search(&search), s);
        assert_eq!(
            NavState::from_search(&Search::default()),
            NavState::default()
        );
    }

    #[test]
    fn entries_alike_but_for_their_options_are_told_apart() {
        let size = SortSpec {
            key: SortKey::Size,
            dir: SortDir::Desc,
        };
        let entries = [
            state("*.exe", Options::default()),
            state(
                "*.exe",
                Options {
                    sort: size,
                    ..Options::default()
                },
            ),
            state(
                "*.exe",
                Options {
                    sort: size,
                    match_case: true,
                    ..Options::default()
                },
            ),
            state("notes", Options::default()),
            state(
                "",
                Options {
                    filter: Filter::Audio,
                    ..Options::default()
                },
            ),
            state("", Options::default()),
        ];
        let refs: Vec<&NavState> = entries.iter().collect();
        assert_eq!(
            entry_labels(&refs),
            [
                "*.exe  (Name ↑)",
                "*.exe  (Size ↓)",
                "*.exe  (Size ↓, Match Case)",
                "notes",
                "(Empty)  (Audio)",
                "(Empty)",
            ]
        );
        // Alone, or the same search with the same options: the search only.
        let one = [state("*.exe", Options::default())];
        assert_eq!(entry_labels(&[&one[0], &one[0]]), ["*.exe", "*.exe"]);
    }

    #[test]
    fn every_listed_label_differs_when_the_entries_do() {
        // All combinations of the options for one search.
        let mut entries = Vec::new();
        for case in [false, true] {
            for path in [false, true] {
                for filter in [Filter::Everything, Filter::Audio, Filter::Image] {
                    for key in [SortKey::Name, SortKey::Size] {
                        entries.push(state(
                            "x",
                            Options {
                                match_case: case,
                                match_path: path,
                                filter,
                                sort: SortSpec::default_for(key),
                            },
                        ));
                    }
                }
            }
        }
        let refs: Vec<&NavState> = entries.iter().collect();
        let labels = entry_labels(&refs);
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "{labels:?}");
    }
}
