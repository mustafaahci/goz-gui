//! A refresh keeps the selection on the same files.
//!
//! The list is virtual (owner data), so it keeps its selection, focus and
//! anchor as row numbers. A refresh (F5, the window coming back, the timer
//! while gozd indexes) replaces every row, and a file added or removed above
//! a selected one moves it: the same numbers would then point at other
//! files, and Enter, Cut or Properties would act on those. So just before a
//! refresh's rows replace the ones on screen, the selected rows that are
//! held are noted by path; the list's selection is cleared, and each of
//! those files is selected again as the block holding it arrives. A file
//! that is gone stays unselected. Selected rows that are not held cannot be
//! noted (nothing knows their paths), but no action takes them either.
//!
//! Select All (Ctrl+A) stays every row: after the refresh every row is
//! selected, the files that came with it included, whatever the result's
//! new size, and only the focus and the anchor are sought by path.
//! Selecting every row is one message to the list whatever their number,
//! so a refresh of millions of selected rows costs what one of a few does.
//! The same holds when a later block brings a larger total, and for a tab
//! shown again (`session.rs`). It lasts until the user chooses other rows
//! or another search shows, or a refresh finds no rows: what was selected
//! is then sought by path, as any selection is.
//!
//! A selection that only covers every row (a click on the only row, the
//! top row Enter selects, Shift+click over all of a few) is the user's
//! files, not "every row": it stays every row only while the result keeps
//! its size, and is otherwise sought by path, so a file that comes later
//! (Word's `~$` owner file beside the document opened) is never selected
//! with it.
//!
//! As soon as the user changes the selection, whatever has not come back
//! yet is forgotten, so it never adds itself to a selection made since.
//!
//! A restored session (`session.rs`) selects its files again the same way.

use std::collections::HashSet;

use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    LVIS_FOCUSED, LVIS_SELECTED, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETSELECTIONMARK,
    LVM_SETSELECTIONMARK, LVNI_FOCUSED,
};

use super::{App, send};
use crate::pane::PaneEvent;

/// The files a refresh is to select again, by path.
#[derive(Debug, Default)]
pub(super) struct Reselect {
    paths: HashSet<Box<[u16]>>,
    /// The focused row's file.
    focus: Option<Box<[u16]>>,
    /// The anchor of Shift+click and Shift+arrow.
    mark: Option<Box<[u16]>>,
}

/// Where the files of a [`Reselect`] are in a block that arrived.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Found {
    pub selected: Vec<usize>,
    pub focus: Option<usize>,
    pub mark: Option<usize>,
}

impl Found {
    fn is_empty(&self) -> bool {
        self.selected.is_empty() && self.focus.is_none() && self.mark.is_none()
    }
}

impl Reselect {
    /// The files of a saved session, to select again as their rows come;
    /// the focused one is also the anchor, as a click leaves it.
    pub(super) fn sought_again(paths: Vec<Vec<u16>>, focus: Option<Vec<u16>>) -> Reselect {
        let focus: Option<Box<[u16]>> = focus.map(Vec::into_boxed_slice);
        Reselect {
            paths: paths.into_iter().map(Vec::into_boxed_slice).collect(),
            mark: focus.clone(),
            focus,
        }
    }

    /// The files still sought (in no particular order), and the focused one
    /// if it is still sought.
    pub(super) fn sought(&self) -> (Vec<Vec<u16>>, Option<Vec<u16>>) {
        (
            self.paths.iter().map(|p| p.to_vec()).collect(),
            self.focus.as_deref().map(<[u16]>::to_vec),
        )
    }

    /// Takes in what an earlier refresh has not found yet: it was cleared
    /// from the list, so the list's selection no longer shows it.
    fn merge(&mut self, older: Reselect) {
        self.paths.extend(older.paths);
        self.focus = self.focus.take().or(older.focus);
        self.mark = self.mark.take().or(older.mark);
    }

    /// Every row is selected again (a refresh of a result all of which was
    /// selected, whatever its new size): the files need not be sought,
    /// only the focus and the anchor. Selecting each one again would
    /// change nothing and cost the list a message a file.
    pub(super) fn every_row_selected(&mut self) {
        self.paths.clear();
    }

    /// Everything has been found.
    pub(super) fn is_done(&self) -> bool {
        self.paths.is_empty() && self.focus.is_none() && self.mark.is_none()
    }

    /// The rows among `rows` (index and path) that hold the files sought;
    /// those files are no longer sought.
    fn find<'a>(&mut self, rows: impl IntoIterator<Item = (usize, &'a [u16])>) -> Found {
        let mut found = Found::default();
        for (index, path) in rows {
            if self.is_done() {
                break;
            }
            if self.paths.remove(path) {
                found.selected.push(index);
            }
            if self.focus.as_deref() == Some(path) {
                self.focus = None;
                found.focus = Some(index);
            }
            if self.mark.as_deref() == Some(path) {
                self.mark = None;
                found.mark = Some(index);
            }
        }
        found
    }
}

/// Whether `selected` rows of `total` are every row. No rows: nothing is
/// selected.
fn every_row(selected: u64, total: u64) -> bool {
    selected > 0 && selected >= total
}

/// Whether Select All is in force: it chose every row (`chosen`, the
/// window's flag) and every row is still selected, `selected` of `total`.
pub(super) fn all_chosen(chosen: bool, selected: u64, total: u64) -> bool {
    chosen && every_row(selected, total)
}

/// Whether the rows a block added (`old_total` rows before, `total` now)
/// are to be selected: Select All is in force (`chosen`, `selected` of
/// `old_total`).
fn new_rows_selected(chosen: bool, selected: u64, old_total: u64, total: u64) -> bool {
    total > old_total && all_chosen(chosen, selected, old_total)
}

/// How a refresh puts the selection back on its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Again {
    /// Select All's every row, as many as there are now (the files that
    /// came with the refresh too): one message to the list; only the focus
    /// and the anchor are sought by path, and Select All stays in force.
    Chosen,
    /// Every row of a result that kept its size, all of whose rows were
    /// selected though not by Select All: one message to the list too.
    Covered,
    /// The files noted, each selected again where its row is now.
    Files,
}

impl Again {
    /// The way back for a selection of `selected` rows of `total`
    /// (`chosen`: Select All's), for a refresh bringing `new_total` rows.
    /// A refresh that finds no rows selects nothing: the files are sought
    /// by path, so they are selected again if they come back.
    fn of(chosen: bool, selected: u64, total: u64, new_total: u64) -> Again {
        if !every_row(selected, total) || new_total == 0 {
            Again::Files
        } else if chosen {
            Again::Chosen
        } else if new_total == total {
            Again::Covered
        } else {
            Again::Files
        }
    }
}

/// The selection as a refresh found it.
pub(super) struct Kept {
    reselect: Reselect,
    again: Again,
}

impl Kept {
    /// The selection for another search's rows (Options > Results' "Clear
    /// selection on search" off): its files, never all the rows because
    /// all of the last search's were.
    pub(super) fn for_new_search(self) -> Kept {
        Kept {
            again: Again::Files,
            ..self
        }
    }
}

impl App {
    /// Runs `f`, whose changes to the list's selection are goz-gui's own,
    /// not the user's.
    pub(super) fn quietly(&self, f: impl FnOnce()) {
        let was = self.restoring.replace(true);
        f();
        self.restoring.set(was);
    }

    /// Runs `f`, which changes rows' states one at a time, as goz-gui's own
    /// change (`quietly`). In icon view the list measures an item's name
    /// (LVN_GETDISPINFO, then DrawText) on every change of its state, to
    /// know what to repaint: 0.1 to 0.3 ms an item, so selecting a few
    /// thousand files again after a refresh or a tab switch held the window
    /// for a third of a second. Meanwhile it is given no names
    /// (`disp_info`), which makes that a few microseconds an item; the
    /// cells it would then repaint are too small for the names drawn here,
    /// so the whole list repaints once after, and the status bar and the
    /// panes hear of the changes once.
    pub(super) fn in_bulk(&self, f: impl FnOnce()) {
        let was = self.bulk.replace(true);
        self.quietly(f);
        self.bulk.set(was);
        if was {
            return;
        }
        // SAFETY: repaints this thread's list.
        unsafe { InvalidateRect(self.list, core::ptr::null(), 0) };
        if self.bulk_changed.take() {
            self.invalidate_status();
            self.pane_event(PaneEvent::SelectionChanged);
        }
    }

    /// The selection changed: if the user changed it, files a refresh has
    /// not found yet are not wanted any more.
    pub(super) fn selection_changed(&self) {
        if !self.restoring.get() {
            self.reselect.borrow_mut().take();
            // Nor is the place and selection of a restored session.
            self.drop_view();
        }
    }

    /// Forgets the files a refresh was to select again, and Select All (a
    /// new search, or the rows gone).
    pub(super) fn forget_selection(&self) {
        self.reselect.borrow_mut().take();
        self.select_all.set(false);
    }

    /// Notes what is selected, by path, before a refresh's rows (`refresh`,
    /// `new_total` of them) or another search's replace the rows on screen.
    pub(super) fn note_selection(&self, refresh: bool, new_total: u64) -> Kept {
        let count = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
        let again = if refresh {
            let total = self.model.borrow().total;
            Again::of(self.select_all.get(), count, total, new_total)
        } else {
            Again::Files
        };
        // Every row selected again after a refresh: the files need not be
        // noted, only the focus and the anchor.
        let held = if again == Again::Files && count > 0 {
            self.selected_held()
        } else {
            Vec::new()
        };
        let index = |v: isize| usize::try_from(v).ok();
        let focused = index(send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ));
        let mark = index(send(self.list, LVM_GETSELECTIONMARK, 0, 0));
        let mut reselect = {
            let m = self.model.borrow();
            let path = |i: usize| m.peek(i).map(|row| row.path.clone());
            Reselect {
                paths: held.iter().filter_map(|&i| path(i)).collect(),
                focus: focused.and_then(path),
                mark: mark.and_then(path),
            }
        };
        if let Some(older) = self.reselect.borrow_mut().take() {
            reselect.merge(older);
        }
        Kept { reselect, again }
    }

    /// A refresh's rows are on screen: the selection by row number is
    /// cleared, and the files noted in `kept` are selected again as their
    /// rows arrive (`reselect_block`); or every row is, at once, for Select
    /// All (however many rows there are now) or for a selection of every
    /// row of a result that kept its size.
    pub(super) fn restore_selection(&self, kept: Kept) {
        let Kept {
            mut reselect,
            again,
        } = kept;
        let all = again != Again::Files;
        self.quietly(|| {
            self.clear_selection();
            if all {
                self.select_every_row();
            }
        });
        if all {
            reselect.every_row_selected();
        }
        // Select All stays in force, or ends: its files are sought by path
        // (a refresh that found no rows), or another search's rows came.
        self.select_all.set(again == Again::Chosen);
        if !reselect.is_done() {
            *self.reselect.borrow_mut() = Some(reselect);
        }
    }

    /// Every row selected: one message whatever their number (the list
    /// keeps its selection as ranges).
    pub(super) fn select_every_row(&self) {
        self.set_state(usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
    }

    /// A block of the rows on screen (not a refresh's or a search's first)
    /// said the result has `total` rows, not `old_total`: the list takes
    /// the new count, and Select All stays every row, so the rows that came
    /// are selected too. Fewer rows need nothing more: the list drops the
    /// rows past its end, and their selection with them. Any other
    /// selection stays on its rows: the new ones are not the user's.
    pub(super) fn recount(&self, old_total: u64, total: u64) {
        let selected = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
        let chosen = all_chosen(self.select_all.get(), selected, old_total);
        let add = new_rows_selected(self.select_all.get(), selected, old_total, total);
        self.set_count(total, true);
        if add {
            self.quietly(|| self.select_every_row());
        }
        if chosen {
            // The count's own changes to the list are not the user's.
            self.select_all.set(true);
        }
    }

    /// Block `block` arrived: selects again the files of the refresh that
    /// are in it.
    pub(super) fn reselect_block(&self, block: u32) {
        let found = {
            let mut slot = self.reselect.borrow_mut();
            let Some(reselect) = slot.as_mut() else {
                return;
            };
            let m = self.model.borrow();
            let found = reselect.find(m.block_rows(block).map(|(i, row)| (i, &row.path[..])));
            if reselect.is_done() {
                *slot = None;
            }
            found
        };
        if found.is_empty() {
            return;
        }
        if !found.selected.is_empty() {
            self.in_bulk(|| {
                for &i in &found.selected {
                    self.set_state(i, LVIS_SELECTED, LVIS_SELECTED);
                }
            });
        }
        self.quietly(|| {
            if let Some(i) = found.focus {
                self.set_state(i, LVIS_FOCUSED, LVIS_FOCUSED);
            }
            if let Some(i) = found.mark {
                send(self.list, LVM_SETSELECTIONMARK, 0, i as isize);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Box<[u16]> {
        s.encode_utf16().collect()
    }

    fn rows(paths: &[&str]) -> Vec<(usize, Box<[u16]>)> {
        paths.iter().enumerate().map(|(i, s)| (i, p(s))).collect()
    }

    fn find(r: &mut Reselect, at: &[(usize, Box<[u16]>)]) -> Found {
        r.find(at.iter().map(|(i, path)| (*i, &path[..])))
    }

    #[test]
    fn files_are_found_where_they_moved() {
        let mut r = Reselect {
            paths: [p(r"C:\b"), p(r"C:\c")].into_iter().collect(),
            focus: Some(p(r"C:\c")),
            mark: Some(p(r"C:\b")),
        };
        // A file came in above both.
        let found = find(&mut r, &rows(&[r"C:\a", r"C:\a0", r"C:\b", r"C:\c"]));
        assert_eq!(
            found,
            Found {
                selected: vec![2, 3],
                focus: Some(3),
                mark: Some(2),
            }
        );
        assert!(r.is_done());
    }

    #[test]
    fn a_file_gone_is_not_replaced_by_its_neighbour() {
        let mut r = Reselect {
            paths: [p(r"C:\b")].into_iter().collect(),
            focus: Some(p(r"C:\b")),
            mark: None,
        };
        // b was deleted: whatever is at its old row now stays unselected.
        let found = find(&mut r, &rows(&[r"C:\a", r"C:\c"]));
        assert_eq!(found, Found::default());
        assert!(!r.is_done(), "b may still be in a block not here yet");
    }

    #[test]
    fn files_in_later_blocks_are_found_when_they_come() {
        let mut r = Reselect {
            paths: [p(r"C:\a"), p(r"C:\z")].into_iter().collect(),
            focus: None,
            mark: None,
        };
        let first = find(&mut r, &rows(&[r"C:\a", r"C:\b"]));
        assert_eq!(first.selected, [0]);
        let later: Vec<_> = [(1024, p(r"C:\y")), (1025, p(r"C:\z"))].into();
        assert_eq!(find(&mut r, &later).selected, [1025]);
        assert!(r.is_done());
        // Nothing sought: nothing found.
        assert_eq!(find(&mut r, &later), Found::default());
    }

    #[test]
    fn a_saved_selection_is_sought_with_its_focus_as_the_anchor() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let mut r = Reselect::sought_again(vec![w(r"C:\b"), w(r"C:\d")], Some(w(r"C:\d")));
        let (mut paths, focus) = r.sought();
        paths.sort();
        assert_eq!(
            (paths, focus.clone()),
            (vec![w(r"C:\b"), w(r"C:\d")], Some(w(r"C:\d")))
        );
        let found = find(&mut r, &rows(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d"]));
        assert_eq!(
            found,
            Found {
                selected: vec![1, 3],
                focus: Some(3),
                mark: Some(3),
            }
        );
        assert!(r.is_done());
        assert_eq!(r.sought(), (Vec::new(), None));
        // Nothing saved: nothing sought.
        assert!(Reselect::sought_again(Vec::new(), None).is_done());
    }

    #[test]
    fn with_every_row_selected_again_only_the_place_is_sought() {
        let mut r = Reselect {
            paths: [p(r"C:\a"), p(r"C:\b"), p(r"C:\c")].into_iter().collect(),
            focus: Some(p(r"C:\b")),
            mark: Some(p(r"C:\a")),
        };
        r.every_row_selected();
        assert!(!r.is_done(), "the focus and the anchor are still sought");
        let found = find(&mut r, &rows(&[r"C:\a", r"C:\b", r"C:\c"]));
        assert_eq!(
            found,
            Found {
                selected: Vec::new(),
                focus: Some(1),
                mark: Some(0),
            }
        );
        assert!(r.is_done());
        // Nothing but files: nothing left to seek.
        let mut r = Reselect {
            paths: [p(r"C:\a")].into_iter().collect(),
            focus: None,
            mark: None,
        };
        r.every_row_selected();
        assert!(r.is_done());
    }

    #[test]
    fn select_all_is_every_row_again_whatever_the_new_size() {
        let chosen = true;
        // Select All of 5 M rows; the refresh's size does not enter into
        // it: every row is selected again, the new ones included.
        for new_total in [5_000_000, 5_000_040, 4_999_990, 1] {
            assert_eq!(
                Again::of(chosen, 5_000_000, 5_000_000, new_total),
                Again::Chosen
            );
        }
        // The list never counts more selected than it has rows; if the
        // model is a block behind, it is still every row.
        assert_eq!(
            Again::of(chosen, 5_000_001, 5_000_000, 5_000_002),
            Again::Chosen
        );
        // A refresh that finds no rows: the files are sought by path, to
        // be selected again if they come back (an editor's safe save).
        assert_eq!(Again::of(chosen, 1, 1, 0), Again::Files);
        assert_eq!(Again::of(chosen, 5_000_000, 5_000_000, 0), Again::Files);
        // Not every row any more (the flag is cleared by then; were it
        // not, the rows decide): the files, by path.
        assert_eq!(
            Again::of(chosen, 4_999_999, 5_000_000, 5_000_000),
            Again::Files
        );
        // Nothing selected, or no rows: nothing to select again.
        assert_eq!(Again::of(chosen, 0, 5_000_000, 5_000_000), Again::Files);
        assert_eq!(Again::of(chosen, 0, 0, 10), Again::Files);
        // Another search's rows are never all selected for it.
        let kept = Kept {
            reselect: Reselect::default(),
            again: Again::Chosen,
        };
        assert_eq!(kept.for_new_search().again, Again::Files);
    }

    #[test]
    fn a_selection_that_only_covers_every_row_is_its_files() {
        let chosen = false;
        // The only hit, clicked (or selected by Enter or by the list taking
        // the keyboard); a refresh of the same size: still every row.
        assert_eq!(Again::of(chosen, 1, 1, 1), Again::Covered);
        assert_eq!(Again::of(chosen, 3, 3, 3), Again::Covered);
        // Word's ~$ owner file came beside the document opened: the
        // document alone is selected again, by path.
        assert_eq!(Again::of(chosen, 1, 1, 2), Again::Files);
        assert_eq!(Again::of(chosen, 3, 3, 2), Again::Files);
        assert_eq!(Again::of(chosen, 1, 1, 0), Again::Files);
        // Anything less is the files, by path.
        assert_eq!(Again::of(chosen, 1, 5_000_000, 5_000_000), Again::Files);
        assert_eq!(Again::of(chosen, 0, 0, 0), Again::Files);
    }

    #[test]
    fn rows_a_block_adds_are_selected_only_under_select_all() {
        // Select All of 5 M, then a block says 3 more.
        assert!(new_rows_selected(true, 5_000_000, 5_000_000, 5_000_003));
        // Fewer rows: the list drops the ones past its end; nothing to add.
        assert!(!new_rows_selected(true, 5_000_000, 5_000_000, 4_999_990));
        assert!(!new_rows_selected(true, 5_000_000, 5_000_000, 5_000_000));
        // Every row selected, but not by Select All (the only row
        // clicked): the new rows are not the user's.
        assert!(!new_rows_selected(false, 1, 1, 2));
        assert!(!new_rows_selected(false, 5_000_000, 5_000_000, 5_000_003));
        // All but one, or none: the new rows stay unselected.
        assert!(!new_rows_selected(true, 4_999_999, 5_000_000, 5_000_003));
        assert!(!new_rows_selected(true, 0, 5_000_000, 5_000_003));
        // From no rows at all: nothing was selected.
        assert!(!new_rows_selected(true, 0, 0, 10));
        // Select All in force: the flag and every row still selected.
        assert!(all_chosen(true, 3, 3));
        assert!(!all_chosen(false, 3, 3));
        assert!(!all_chosen(true, 2, 3));
        assert!(!all_chosen(true, 0, 0));
    }

    #[test]
    fn a_second_refresh_keeps_what_the_first_has_not_found() {
        let mut older = Reselect {
            paths: [p(r"C:\far")].into_iter().collect(),
            focus: Some(p(r"C:\far")),
            mark: Some(p(r"C:\far")),
        };
        // Its block never came before the next refresh.
        assert_eq!(find(&mut older, &rows(&[r"C:\a"])), Found::default());
        let mut newer = Reselect {
            paths: [p(r"C:\a")].into_iter().collect(),
            focus: None,
            mark: Some(p(r"C:\a")),
        };
        newer.merge(older);
        let found = find(&mut newer, &rows(&[r"C:\a", r"C:\far"]));
        assert_eq!(
            found,
            Found {
                selected: vec![0, 1],
                focus: Some(1),
                mark: Some(0),
            }
        );
    }
}
