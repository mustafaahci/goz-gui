//! Undo and Redo of what goz-gui did to the files (Edit > Undo, Ctrl+Z;
//! Redo, Ctrl+Y; Undo History..., Ctrl+Shift+Z).
//!
//! goz-gui keeps its own record of the operations it performed: a rename
//! (in place, or Advanced Rename's many as one), a move, a copy, a delete
//! to the Recycle Bin, a new folder, a change of file times and a new
//! shortcut. At most `max_undo` (Everything's key, 256) are kept, the
//! oldest going first; a new one forgets what was undone (Redo). Each
//! record knows where each of its items went, as the shell reported it
//! (`super::Sink`), so undoing it is exact: a rename or a move goes back to
//! the old name and folder, a copy goes to the Recycle Bin, a delete comes
//! back out of it, a new folder (still empty) and a shortcut go to the
//! Recycle Bin, times go back. An item moved or replaced since is skipped,
//! never written over (`super::Task::checked`).
//!
//! The record is in memory only, for the life of the window, as Explorer's.

use super::{Set, Step, StepResult, Times, name_of};
use crate::fileops::parent;

/// What a record did: Everything's four, and goz-gui's three more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Rename,
    Move,
    Copy,
    Delete,
    NewFolder,
    SetFileTime,
    CreateShortcut,
}

impl Action {
    /// In Everything's order: the Undo History's Action list.
    pub const ALL: [Action; 7] = [
        Action::Delete,
        Action::Rename,
        Action::Move,
        Action::Copy,
        Action::NewFolder,
        Action::SetFileTime,
        Action::CreateShortcut,
    ];

    /// Its name, as the Undo History's Action column and the Edit menu say
    /// it.
    pub fn label(self) -> &'static str {
        match self {
            Action::Rename => "Rename",
            Action::Move => "Move",
            Action::Copy => "Copy",
            Action::Delete => "Delete",
            Action::NewFolder => "New Folder",
            Action::SetFileTime => "Set File Time",
            Action::CreateShortcut => "Create Shortcut",
        }
    }
}

/// One item of a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The item before: the old path; for a delete, where it was; for a
    /// shortcut, its target; for a new folder, nothing.
    pub from: Vec<u16>,
    /// After: the new path, the copy, the new folder or shortcut; for a
    /// delete, the item in the Recycle Bin (its `$R` file).
    pub to: Vec<u16>,
    /// A change of times: before and after.
    pub before: Option<Times>,
    pub after: Option<Times>,
}

/// An operation goz-gui performed, as Undo can take it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Its number, for the Undo History.
    pub id: u64,
    /// When it was done (FILETIME, UTC).
    pub date: u64,
    pub action: Action,
    pub items: Vec<Item>,
}

impl Record {
    /// What Undo does: a step per item, the last item first.
    pub fn undo_steps(&self) -> Vec<Step> {
        self.items
            .iter()
            .rev()
            .map(|it| undo_step(self.action, it))
            .collect()
    }

    /// What Redo does: a step per item, in order.
    pub fn redo_steps(&self) -> Vec<Step> {
        self.items
            .iter()
            .map(|it| redo_step(self.action, it))
            .collect()
    }

    /// The items `results` (of [`undo_steps`](Self::undo_steps), the last
    /// item first) undid, and those they did not, as two records (either
    /// may be empty) with this one's number, date and action.
    pub fn split_undone(self, results: &[StepResult]) -> (Record, Record) {
        let n = self.items.len();
        let (mut done, mut left) = (self.empty_like(), self.empty_like());
        for (i, item) in self.items.into_iter().enumerate() {
            match results.get(n - 1 - i) {
                Some(StepResult::Done { .. }) => done.items.push(item),
                _ => left.items.push(item),
            }
        }
        (done, left)
    }

    /// The items `results` (of [`redo_steps`](Self::redo_steps)) redid,
    /// as they are now (where the item went again, its times), and those
    /// they did not.
    pub fn split_redone(self, results: &[StepResult]) -> (Record, Record) {
        let (mut done, mut left) = (self.empty_like(), self.empty_like());
        for (i, mut item) in self.items.into_iter().enumerate() {
            match results.get(i) {
                Some(StepResult::Done { to, before, after }) => {
                    if !to.is_empty() {
                        item.to = to.clone();
                    }
                    if before.is_some() {
                        item.before = *before;
                        item.after = *after;
                    }
                    done.items.push(item);
                }
                _ => left.items.push(item),
            }
        }
        (done, left)
    }

    /// A record of this one's number, date and action, with no items.
    fn empty_like(&self) -> Record {
        Record {
            id: self.id,
            date: self.date,
            action: self.action,
            items: Vec::new(),
        }
    }
}

fn undo_step(action: Action, it: &Item) -> Step {
    let name = |p: &[u16]| name_of(p).to_vec();
    let folder = |p: &[u16]| parent(p).unwrap_or(&[]).to_vec();
    match action {
        Action::Rename => Step::Rename {
            path: it.to.clone(),
            name: name(&it.from),
        },
        Action::Move => Step::Move {
            path: it.to.clone(),
            folder: folder(&it.from),
            name: Some(name(&it.from)),
        },
        Action::Copy | Action::CreateShortcut => Step::Recycle {
            path: it.to.clone(),
            empty_only: false,
        },
        Action::NewFolder => Step::Recycle {
            path: it.to.clone(),
            empty_only: true,
        },
        Action::Delete => Step::Restore {
            recycled: it.to.clone(),
            path: it.from.clone(),
        },
        Action::SetFileTime => Step::Times {
            path: it.to.clone(),
            set: times_set(it.before.unwrap_or_default()),
        },
    }
}

fn redo_step(action: Action, it: &Item) -> Step {
    let name = |p: &[u16]| name_of(p).to_vec();
    let folder = |p: &[u16]| parent(p).unwrap_or(&[]).to_vec();
    match action {
        Action::Rename => Step::Rename {
            path: it.from.clone(),
            name: name(&it.to),
        },
        Action::Move => Step::Move {
            path: it.from.clone(),
            folder: folder(&it.to),
            name: Some(name(&it.to)),
        },
        Action::Copy => Step::Copy {
            path: it.from.clone(),
            folder: folder(&it.to),
            name: Some(name(&it.to)),
        },
        Action::Delete => Step::Recycle {
            path: it.from.clone(),
            empty_only: false,
        },
        Action::NewFolder => Step::NewFolder {
            folder: folder(&it.to),
            name: name(&it.to),
            exact: true,
        },
        Action::SetFileTime => Step::Times {
            path: it.to.clone(),
            set: times_set(it.after.unwrap_or_default()),
        },
        Action::CreateShortcut => Step::Shortcut {
            target: it.from.clone(),
            link: Some(it.to.clone()),
            desktop: None,
        },
    }
}

/// Steps that put back `t`: created, modified, accessed.
fn times_set(t: Times) -> [Set; 3] {
    [Set::At(t.created), Set::At(t.modified), Set::At(t.accessed)]
}

/// The items of an action from what its steps did: each step that was
/// done (`results`, with the paths `sources` they acted on).
pub fn items_of(action: Action, sources: &[Vec<u16>], results: &[StepResult]) -> Vec<Item> {
    sources
        .iter()
        .zip(results)
        .filter_map(|(from, r)| match r {
            StepResult::Done { to, before, after } if !to.is_empty() => Some(Item {
                from: if action == Action::NewFolder {
                    Vec::new()
                } else {
                    from.clone()
                },
                to: to.clone(),
                before: *before,
                after: *after,
            }),
            _ => None,
        })
        .collect()
}

/// goz-gui's undo record: what can be undone (the last done last), and
/// what was undone and can be redone (the last undone last).
#[derive(Debug, Default)]
pub struct Stack {
    done: Vec<Record>,
    undone: Vec<Record>,
    /// How many are kept (`max_undo`); 0 keeps none.
    max: usize,
    next_id: u64,
}

impl Stack {
    pub fn new(max: usize) -> Stack {
        Stack {
            max,
            next_id: 1,
            ..Stack::default()
        }
    }

    #[cfg(test)]
    fn set_max(&mut self, max: usize) {
        self.max = max;
        self.trim();
    }

    fn trim(&mut self) {
        let over = self.done.len().saturating_sub(self.max);
        self.done.drain(..over);
        let over = self.undone.len().saturating_sub(self.max);
        self.undone.drain(..over);
    }

    /// Records a new operation (nothing for no items); what was undone can
    /// no longer be redone. Its number.
    pub fn push(&mut self, action: Action, date: u64, items: Vec<Item>) -> Option<u64> {
        if items.is_empty() || self.max == 0 {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.done.push(Record {
            id,
            date,
            action,
            items,
        });
        self.undone.clear();
        self.trim();
        Some(id)
    }

    /// What Undo would undo, and Redo redo.
    pub fn next_undo(&self) -> Option<&Record> {
        self.done.last()
    }

    pub fn next_redo(&self) -> Option<&Record> {
        self.undone.last()
    }

    /// Every record that can be undone, the oldest first.
    pub fn done(&self) -> &[Record] {
        &self.done
    }

    /// Takes the last record out to undo it.
    pub fn take_undo(&mut self) -> Option<Record> {
        self.done.pop()
    }

    /// Takes the last undone record out to redo it.
    pub fn take_redo(&mut self) -> Option<Record> {
        self.undone.pop()
    }

    /// Takes out, to undo them, the items `picks` names (a record's number
    /// and an item's place in it), as one record per record they are in,
    /// the newest first.
    pub fn take_items(&mut self, picks: &[(u64, usize)]) -> Vec<Record> {
        let mut out = Vec::new();
        for rec in self.done.iter_mut().rev() {
            let mut chosen: Vec<usize> = picks
                .iter()
                .filter(|(id, _)| *id == rec.id)
                .map(|&(_, i)| i)
                .filter(|&i| i < rec.items.len())
                .collect();
            if chosen.is_empty() {
                continue;
            }
            chosen.sort_unstable();
            chosen.dedup();
            let mut taken = rec.empty_like();
            for &i in chosen.iter().rev() {
                taken.items.push(rec.items.remove(i));
            }
            taken.items.reverse();
            out.push(taken);
        }
        self.done.retain(|r| !r.items.is_empty());
        out
    }

    /// An undo finished: what it undid can be redone, what it did not is
    /// put back where it was.
    pub fn undone(&mut self, done: Record, left: Record) {
        if !done.items.is_empty() {
            self.undone.push(done);
        }
        Self::put_back(&mut self.done, left);
        self.trim();
    }

    /// A redo finished: what it redid can be undone again (what is still
    /// undone stays redoable), what it did not is put back.
    pub fn redone(&mut self, done: Record, left: Record) {
        if !done.items.is_empty() {
            Self::put_back(&mut self.done, done);
        }
        Self::put_back(&mut self.undone, left);
        self.trim();
    }

    /// Puts `rec`'s items back into `list`: into the record of its number
    /// if it is there, else as a record in its place by number.
    fn put_back(list: &mut Vec<Record>, rec: Record) {
        if rec.items.is_empty() {
            return;
        }
        if let Some(have) = list.iter_mut().find(|r| r.id == rec.id) {
            have.items.extend(rec.items);
            return;
        }
        let at = list.partition_point(|r| r.id < rec.id);
        list.insert(at, rec);
    }

    /// How many records can be undone and redone.
    #[cfg(test)]
    fn len(&self) -> (usize, usize) {
        (self.done.len(), self.undone.len())
    }
}

/// The Edit menu's Undo and Redo items, as Everything labels them: "&Undo
/// Rename", "&Redo Move", or plain "&Undo" / "&Redo" with nothing to do.
pub fn menu_label(redo: bool, action: Option<Action>) -> String {
    let verb = if redo { "&Redo" } else { "&Undo" };
    let key = if redo { "Ctrl+Y" } else { "Ctrl+Z" };
    match action {
        Some(a) => format!("{verb} {}\t{key}", a.label()),
        None => format!("{verb}\t{key}"),
    }
}

/// A short account of a record for the status bar: "Undo Rename of a.txt
/// to b.txt", "Undo Move of 3 items".
pub fn describe(verb: &str, rec: &Record) -> String {
    let name = |p: &[u16]| String::from_utf16_lossy(name_of(p));
    match (rec.items.as_slice(), rec.action) {
        ([one], Action::Rename) => {
            format!("{verb} Rename of {} to {}", name(&one.from), name(&one.to))
        }
        ([one], Action::NewFolder) => format!("{verb} New Folder {}", name(&one.to)),
        ([one], Action::CreateShortcut) => {
            format!("{verb} Create Shortcut {}", name(&one.to))
        }
        ([one], Action::Delete | Action::SetFileTime) => {
            format!("{verb} {} of {}", rec.action.label(), name(&one.from))
        }
        ([one], _) => format!(
            "{verb} {} of {} to {}",
            rec.action.label(),
            name(&one.from),
            String::from_utf16_lossy(parent(&one.to).unwrap_or(&[]))
        ),
        (items, _) => format!(
            "{verb} {} of {} items",
            rec.action.label(),
            crate::format::grouped(items.len() as u64)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn item(from: &str, to: &str) -> Item {
        Item {
            from: w(from),
            to: w(to),
            before: None,
            after: None,
        }
    }

    fn done(to: &str) -> StepResult {
        StepResult::Done {
            to: w(to),
            before: None,
            after: None,
        }
    }

    #[test]
    fn each_action_is_undone_and_redone_exactly() {
        let rec = |action, items| Record {
            id: 1,
            date: 0,
            action,
            items,
        };
        let r = rec(Action::Rename, vec![item(r"C:\a\x.txt", r"C:\a\y.txt")]);
        assert_eq!(
            r.undo_steps(),
            [Step::Rename {
                path: w(r"C:\a\y.txt"),
                name: w("x.txt")
            }]
        );
        assert_eq!(
            r.redo_steps(),
            [Step::Rename {
                path: w(r"C:\a\x.txt"),
                name: w("y.txt")
            }]
        );
        // A move back to its folder under its old name (a "Keep both" had
        // given it another).
        let r = rec(Action::Move, vec![item(r"C:\a\x.txt", r"D:\b\x (2).txt")]);
        assert_eq!(
            r.undo_steps(),
            [Step::Move {
                path: w(r"D:\b\x (2).txt"),
                folder: w(r"C:\a"),
                name: Some(w("x.txt"))
            }]
        );
        assert_eq!(
            r.redo_steps(),
            [Step::Move {
                path: w(r"C:\a\x.txt"),
                folder: w(r"D:\b"),
                name: Some(w("x (2).txt"))
            }]
        );
        // A copy goes to the Recycle Bin; again, under the same name.
        let r = rec(
            Action::Copy,
            vec![item(r"C:\a\x.txt", r"C:\a\x - Copy.txt")],
        );
        assert_eq!(
            r.undo_steps(),
            [Step::Recycle {
                path: w(r"C:\a\x - Copy.txt"),
                empty_only: false
            }]
        );
        assert_eq!(
            r.redo_steps(),
            [Step::Copy {
                path: w(r"C:\a\x.txt"),
                folder: w(r"C:\a"),
                name: Some(w("x - Copy.txt"))
            }]
        );
        // A delete comes back out of the Recycle Bin.
        let r = rec(
            Action::Delete,
            vec![item(r"C:\a\x.txt", r"C:\$Recycle.Bin\S\$R1.txt")],
        );
        assert_eq!(
            r.undo_steps(),
            [Step::Restore {
                recycled: w(r"C:\$Recycle.Bin\S\$R1.txt"),
                path: w(r"C:\a\x.txt")
            }]
        );
        // A new folder only while it is empty, and made again exactly.
        let r = rec(Action::NewFolder, vec![item("", r"C:\a\New folder (2)")]);
        assert_eq!(
            r.undo_steps(),
            [Step::Recycle {
                path: w(r"C:\a\New folder (2)"),
                empty_only: true
            }]
        );
        assert_eq!(
            r.redo_steps(),
            [Step::NewFolder {
                folder: w(r"C:\a"),
                name: w("New folder (2)"),
                exact: true
            }]
        );
        // Times back and forth.
        let t = |c, a, m| Times {
            created: c,
            accessed: a,
            modified: m,
        };
        let r = rec(
            Action::SetFileTime,
            vec![Item {
                before: Some(t(1, 2, 3)),
                after: Some(t(4, 5, 6)),
                ..item(r"C:\f", r"C:\f")
            }],
        );
        assert_eq!(
            r.undo_steps(),
            [Step::Times {
                path: w(r"C:\f"),
                set: [Set::At(1), Set::At(3), Set::At(2)]
            }]
        );
        assert_eq!(
            r.redo_steps(),
            [Step::Times {
                path: w(r"C:\f"),
                set: [Set::At(4), Set::At(6), Set::At(5)]
            }]
        );
        let r = rec(
            Action::CreateShortcut,
            vec![item(r"C:\f", r"C:\f - Shortcut.lnk")],
        );
        assert_eq!(
            r.redo_steps(),
            [Step::Shortcut {
                target: w(r"C:\f"),
                link: Some(w(r"C:\f - Shortcut.lnk")),
                desktop: None
            }]
        );
    }

    #[test]
    fn undo_and_redo_go_back_and_forth() {
        let mut s = Stack::new(256);
        assert!(s.next_undo().is_none());
        let id = s
            .push(Action::Rename, 10, vec![item(r"C:\a", r"C:\b")])
            .unwrap();
        s.push(Action::Copy, 20, vec![item(r"C:\c", r"D:\c")]);
        assert_eq!(s.next_undo().map(|r| r.action), Some(Action::Copy));
        // Undo the copy; then the rename.
        let copy = s.take_undo().unwrap();
        let (did, left) = copy.clone().split_undone(&[done("")]);
        s.undone(did, left);
        assert_eq!(s.next_redo().map(|r| r.action), Some(Action::Copy));
        assert_eq!(s.next_undo().map(|r| r.id), Some(id));
        // Redo the copy: it can be undone again, the rename stays.
        let again = s.take_redo().unwrap();
        let (did, left) = again.split_redone(&[done(r"D:\c")]);
        s.redone(did, left);
        assert_eq!(s.len(), (2, 0));
        assert_eq!(s.next_undo().map(|r| r.action), Some(Action::Copy));
        // A new action forgets what was undone.
        let rename = s.take_undo().unwrap();
        let (did, left) = rename.split_undone(&[done("")]);
        s.undone(did, left);
        assert_eq!(s.len(), (1, 1));
        s.push(Action::Move, 30, vec![item(r"C:\x", r"D:\x")]);
        assert_eq!(s.len(), (2, 0));
        assert!(s.next_redo().is_none());
        // Nothing to record: no record.
        assert_eq!(s.push(Action::Move, 40, Vec::new()), None);
    }

    #[test]
    fn items_not_undone_stay_where_they_were() {
        let mut s = Stack::new(256);
        s.push(
            Action::Rename,
            1,
            vec![
                item(r"C:\a", r"C:\A1"),
                item(r"C:\b", r"C:\B1"),
                item(r"C:\c", r"C:\C1"),
            ],
        );
        let rec = s.take_undo().unwrap();
        // The steps go last item first: c done, b skipped, a done.
        let results = [done(""), StepResult::Skipped("x".into()), done("")];
        let (did, left) = rec.split_undone(&results);
        assert_eq!(
            did.items,
            [item(r"C:\a", r"C:\A1"), item(r"C:\c", r"C:\C1")]
        );
        assert_eq!(left.items, [item(r"C:\b", r"C:\B1")]);
        s.undone(did, left);
        assert_eq!(s.done()[0].items, [item(r"C:\b", r"C:\B1")]);
        assert_eq!(s.next_redo().unwrap().items.len(), 2);
        // A redo takes the new place of each item it redid.
        let redo = s.take_redo().unwrap();
        let (did, left) = redo.split_redone(&[done(r"C:\A2"), StepResult::NotRun]);
        assert_eq!(did.items, [item(r"C:\a", r"C:\A2")]);
        s.redone(did, left);
        // Back with the record it came from.
        assert_eq!(s.done().len(), 1);
        assert_eq!(s.done()[0].items.len(), 2);
        assert_eq!(s.next_redo().unwrap().items, [item(r"C:\c", r"C:\C1")]);
    }

    #[test]
    fn the_history_undoes_picked_items() {
        let mut s = Stack::new(256);
        let a = s
            .push(Action::Rename, 1, vec![item("a", "a1"), item("b", "b1")])
            .unwrap();
        let b = s.push(Action::Move, 2, vec![item("c", "c1")]).unwrap();
        let taken = s.take_items(&[(a, 1), (b, 0), (a, 1), (9, 0), (a, 7)]);
        assert_eq!(taken.len(), 2);
        assert_eq!((taken[0].id, taken[0].items.len()), (b, 1));
        assert_eq!(taken[1].items, [item("b", "b1")]);
        // The move had nothing left: gone; the rename keeps its first.
        assert_eq!(s.done().len(), 1);
        assert_eq!(s.done()[0].items, [item("a", "a1")]);
        // Put back where it was, in its record.
        s.undone(taken[1].empty_like(), taken[1].clone());
        assert_eq!(s.done()[0].items.len(), 2);
    }

    #[test]
    fn at_most_max_undo_are_kept() {
        let mut s = Stack::new(3);
        for i in 0..5 {
            s.push(Action::Copy, i, vec![item("a", "b")]);
        }
        assert_eq!(s.done().len(), 3);
        assert_eq!(s.done()[0].date, 2, "the oldest went first");
        s.set_max(1);
        assert_eq!(s.done().len(), 1);
        let mut none = Stack::new(0);
        assert_eq!(none.push(Action::Copy, 0, vec![item("a", "b")]), None);
        assert!(none.next_undo().is_none());
    }

    #[test]
    fn labels_are_everythings() {
        assert_eq!(menu_label(false, None), "&Undo\tCtrl+Z");
        assert_eq!(
            menu_label(false, Some(Action::Rename)),
            "&Undo Rename\tCtrl+Z"
        );
        assert_eq!(menu_label(true, Some(Action::Move)), "&Redo Move\tCtrl+Y");
        assert_eq!(menu_label(true, None), "&Redo\tCtrl+Y");
        let rec = |action, items| Record {
            id: 1,
            date: 0,
            action,
            items,
        };
        assert_eq!(
            describe(
                "Undid",
                &rec(Action::Rename, vec![item(r"C:\a\x.txt", r"C:\a\y.txt")])
            ),
            "Undid Rename of x.txt to y.txt"
        );
        assert_eq!(
            describe(
                "Redid",
                &rec(Action::Move, vec![item(r"C:\a\x.txt", r"D:\b\x.txt")])
            ),
            r"Redid Move of x.txt to D:\b"
        );
        assert_eq!(
            describe("Undid", &rec(Action::Copy, vec![item("a", "b"); 1200])),
            "Undid Copy of 1,200 items"
        );
        let sources = vec![w("a"), w("b"), w("c")];
        let results = [done("A"), StepResult::NotRun, done("")];
        assert_eq!(items_of(Action::Copy, &sources, &results), [item("a", "A")]);
    }
}
