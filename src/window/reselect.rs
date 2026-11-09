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
//! noted (nothing knows their paths), but no action takes them either. A
//! selection of every row stays one while the result keeps its size.
//!
//! As soon as the user changes the selection, whatever has not come back
//! yet is forgotten, so it never adds itself to a selection made since.
//!
//! A restored session (`session.rs`) selects its files again the same way.

use std::collections::HashSet;

use windows_sys::Win32::UI::Controls::{
    LVIS_FOCUSED, LVIS_SELECTED, LVM_GETNEXTITEM, LVM_GETSELECTEDCOUNT, LVM_GETSELECTIONMARK,
    LVM_SETSELECTIONMARK, LVNI_FOCUSED,
};

use super::{App, send};

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

/// The selection as a refresh found it.
pub(super) struct Kept {
    reselect: Reselect,
    /// Every row was selected, of this many.
    all_of: Option<u64>,
}

impl App {
    /// Runs `f`, whose changes to the list's selection are goz-gui's own,
    /// not the user's.
    pub(super) fn quietly(&self, f: impl FnOnce()) {
        let was = self.restoring.replace(true);
        f();
        self.restoring.set(was);
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

    /// Forgets the files a refresh was to select again (a new search).
    pub(super) fn forget_selection(&self) {
        self.reselect.borrow_mut().take();
    }

    /// Notes what is selected, by path, before a refresh's rows replace
    /// the rows on screen.
    pub(super) fn note_selection(&self) -> Kept {
        let count = send(self.list, LVM_GETSELECTEDCOUNT, 0, 0).max(0) as u64;
        let held = if count > 0 {
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
        let (mut reselect, total) = {
            let m = self.model.borrow();
            let path = |i: usize| m.peek(i).map(|row| row.path.clone());
            let reselect = Reselect {
                paths: held.iter().filter_map(|&i| path(i)).collect(),
                focus: focused.and_then(path),
                mark: mark.and_then(path),
            };
            (reselect, m.total)
        };
        if let Some(older) = self.reselect.borrow_mut().take() {
            reselect.merge(older);
        }
        Kept {
            reselect,
            all_of: (count > 0 && count >= total).then_some(total),
        }
    }

    /// A refresh's rows are on screen, `total` of them: the selection by
    /// row number is cleared, and the files noted in `kept` are selected
    /// again as their rows arrive (`reselect_block`).
    pub(super) fn restore_selection(&self, kept: Kept, total: u64) {
        self.quietly(|| {
            self.clear_selection();
            if kept.all_of == Some(total) {
                self.set_state(usize::MAX, LVIS_SELECTED, LVIS_SELECTED);
            }
        });
        if !kept.reselect.is_done() {
            *self.reselect.borrow_mut() = Some(kept.reselect);
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
        self.quietly(|| {
            for &i in &found.selected {
                self.set_state(i, LVIS_SELECTED, LVIS_SELECTED);
            }
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
