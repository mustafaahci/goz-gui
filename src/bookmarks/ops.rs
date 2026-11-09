//! Changes to the bookmarks, made by identity rather than by row number, so
//! that a change made in this window can be made again on the file as
//! another goz-gui (or Everything) has left it since. That is the merge
//! rule (`file.rs`): before each write the file is read again, and the
//! changes this window made since its last write are made again on it.
//!
//! - A row is known by its kind, its folder and its name (the second of
//!   two bookmarks alike is the second again); a folder by its place.
//! - An edit of a row the file no longer has (another process deleted it)
//!   adds it again, at the end.
//! - A deletion, move or rename of something the file no longer has does
//!   nothing.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::tree::{Item, TOP, Tree};
use super::{Bookmark, Bookmarks, FolderPath, Kind, same_path, within};

/// Which row a change is about: its kind, folder and name, and which of
/// the rows alike it is (the first is 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Id {
    pub kind: Kind,
    pub folder: FolderPath,
    pub name: String,
    pub nth: usize,
}

impl Id {
    pub fn of(list: &Bookmarks, row: usize) -> Id {
        let r = &list.rows()[row];
        let mut id = Id {
            kind: r.kind(),
            folder: r.folder(),
            name: r.name().to_owned(),
            nth: 0,
        };
        id.nth = list.rows()[..row]
            .iter()
            .filter(|other| id.matches(other))
            .count();
        id
    }

    fn matches(&self, row: &Bookmark) -> bool {
        row.kind() == self.kind && row.name() == self.name && same_path(&row.folder(), &self.folder)
    }

    /// The row in `list`.
    pub fn find(&self, list: &Bookmarks) -> Option<usize> {
        list.rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| self.matches(r))
            .nth(self.nth)
            .map(|(i, _)| i)
    }
}

/// What a change acts on: a row, or a folder with everything in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Row(Id),
    Folder(FolderPath),
}

impl Target {
    pub fn of(list: &Bookmarks, tree: &Tree, item: Item) -> Target {
        match item {
            Item::Row(r) => Target::Row(Id::of(list, r)),
            Item::Folder(f) => Target::Folder(tree.nodes[f].path.clone()),
        }
    }

    /// Where it is in `tree` now.
    pub fn item(&self, list: &Bookmarks, tree: &Tree) -> Option<Item> {
        match self {
            Target::Row(id) => id.find(list).map(Item::Row),
            Target::Folder(path) => tree.find(path).filter(|&f| f != TOP).map(Item::Folder),
        }
    }
}

/// A change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// A new row, at the end (the last of its folder).
    Add(Bookmark),
    /// The row `id` is now `row` (added at the end when it is gone).
    Edit { id: Id, row: Bookmark },
    /// These rows, and these folders with everything in them, deleted.
    Delete(Vec<Target>),
    /// Swapped with the one before it in its folder (`up`) or after it.
    Move { target: Target, up: bool },
    /// Moved into the folder `to`, as the last things it holds.
    MoveTo {
        targets: Vec<Target>,
        to: FolderPath,
    },
    /// A folder renamed.
    Rename { folder: FolderPath, name: String },
    /// Another bookmarks file merged in (see [`Bookmarks::import`]).
    Import(Bookmarks),
}

/// The rows of the folder at `path`: its own row(s), and every row inside
/// it, however deep.
fn rows_of_folder(list: &Bookmarks, path: &[String]) -> Vec<usize> {
    list.rows()
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            (r.kind() == Kind::Folder && same_path(&r.place(), path)) || within(&r.folder(), path)
        })
        .map(|(i, _)| i)
        .collect()
}

/// `path` with its first `from.len()` names replaced by `to`.
fn rebase(path: &[String], from: &[String], to: &[String]) -> FolderPath {
    to.iter().chain(&path[from.len()..]).cloned().collect()
}

/// Every folder above the rows (`a/b` gives `a` and `a/b`), as map keys.
fn folder_keys(path: &[String]) -> impl Iterator<Item = String> + '_ {
    (1..=path.len()).map(move |n| {
        path[..n]
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>()
            .join("/")
    })
}

impl Bookmarks {
    /// Makes the change `op`; false when it found nothing to change.
    pub fn apply(&mut self, op: &Op) -> bool {
        match op {
            Op::Add(row) => {
                self.push(row.clone());
                true
            }
            Op::Edit { id, row } => {
                match id.find(self) {
                    Some(i) if *self.rows()[i] == *row => return false,
                    Some(i) => self.replace(i, row.clone()),
                    None => self.push(row.clone()),
                }
                true
            }
            Op::Delete(targets) => self.delete(targets),
            Op::Move { target, up } => self.move_one(target, *up),
            Op::MoveTo { targets, to } => {
                let mut changed = false;
                for target in targets {
                    changed |= self.move_to(target, to);
                }
                changed
            }
            Op::Rename { folder, name } => self.rename(folder, name),
            Op::Import(other) => self.import(other),
        }
    }

    fn delete(&mut self, targets: &[Target]) -> bool {
        let mut gone = BTreeSet::new();
        for target in targets {
            match target {
                Target::Row(id) => gone.extend(id.find(self)),
                Target::Folder(path) if !path.is_empty() => {
                    gone.extend(rows_of_folder(self, path));
                }
                Target::Folder(_) => {}
            }
        }
        for &i in gone.iter().rev() {
            self.remove(i);
        }
        !gone.is_empty()
    }

    /// Swaps `target` with its neighbour in its folder: the later of the
    /// two has its first row put just before the earlier one's, which puts
    /// it first and changes no other folder's order.
    fn move_one(&mut self, target: &Target, up: bool) -> bool {
        let tree = Tree::build(self);
        let Some(item) = target.item(self, &tree) else {
            return false;
        };
        let Some((parent, at)) = tree.parent_of(item) else {
            return false;
        };
        let items = &tree.nodes[parent].items;
        let (earlier, later) = if up {
            match at.checked_sub(1) {
                Some(before) => (items[before], item),
                None => return false,
            }
        } else {
            match items.get(at + 1) {
                Some(&after) => (item, after),
                None => return false,
            }
        };
        let (Some(to), Some(from)) = (tree.first_row(earlier), tree.first_row(later)) else {
            return false;
        };
        let row = self.remove(from);
        self.insert(to, row);
        true
    }

    fn move_to(&mut self, target: &Target, to: &[String]) -> bool {
        match target {
            Target::Row(id) => {
                let Some(i) = id.find(self) else {
                    return false;
                };
                if same_path(&self.rows()[i].folder(), to) {
                    return false;
                }
                let mut row = self.remove(i);
                Arc::make_mut(&mut row).set_folder(to);
                self.push(row);
                true
            }
            Target::Folder(path) => {
                let Some((name, parent)) = path.split_last() else {
                    return false;
                };
                // Not into itself, and not where it is.
                if within(to, path) || same_path(parent, to) {
                    return false;
                }
                let rows = rows_of_folder(self, path);
                if rows.is_empty() {
                    return false;
                }
                let mut new = to.to_vec();
                new.push(name.clone());
                let mut moved = Vec::new();
                for &i in rows.iter().rev() {
                    moved.push(self.remove(i));
                }
                for mut row in moved.into_iter().rev() {
                    let folder = row.folder();
                    if row.kind() == Kind::Folder && same_path(&row.place(), path) {
                        Arc::make_mut(&mut row).set_folder(to);
                    } else {
                        Arc::make_mut(&mut row).set_folder(&rebase(&folder, path, &new));
                    }
                    self.push(row);
                }
                true
            }
        }
    }

    fn rename(&mut self, folder: &[String], name: &str) -> bool {
        let Some((_, parent)) = folder.split_last() else {
            return false;
        };
        let mut new = parent.to_vec();
        new.push(name.to_owned());
        let mut changed = false;
        for i in 0..self.len() {
            let row = self.row_mut(i);
            if row.kind() == Kind::Folder && same_path(&row.place(), folder) {
                row.set_name(name);
                changed = true;
            } else if within(&row.folder(), folder) {
                let moved = rebase(&row.folder(), folder, &new);
                row.set_folder(&moved);
                changed = true;
            }
        }
        changed
    }

    /// Merges `other` (an Everything or goz-gui bookmarks file) in:
    ///
    /// - a folder this list has already is not added again;
    /// - a bookmark this list has (the same folder and name, the n-th of
    ///   its name there) is replaced by the imported one if they differ,
    ///   and left alone if they are the same; any other is added;
    /// - a separator (or a row of a Type goz-gui does not know) is added
    ///   only to a folder the import makes, or to a list that was empty,
    ///   so importing the same file twice adds nothing.
    ///
    /// Added rows go at the end, in the imported file's order.
    pub fn import(&mut self, other: &Bookmarks) -> bool {
        let before = Tree::build(self);
        let was_empty = !self
            .rows()
            .iter()
            .any(|r| matches!(r.kind(), Kind::Bookmark | Kind::Folder | Kind::Separator));
        let had: HashSet<String> = before
            .folders_below(TOP)
            .iter()
            .flat_map(|&(f, _)| folder_keys(&before.nodes[f].path))
            .collect();
        let mut folders = had.clone();
        // This list's bookmarks by folder and name.
        let mut mine: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (i, r) in self.rows().iter().enumerate() {
            if r.kind() == Kind::Bookmark {
                let key = (
                    folder_keys(&r.folder()).last().unwrap_or_default(),
                    r.name().to_owned(),
                );
                mine.entry(key).or_default().push(i);
            }
        }
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        let mut changed = false;
        for row in other.rows() {
            let folder = row.folder();
            let folder_key = folder_keys(&folder).last().unwrap_or_default();
            let fresh = if folder.is_empty() {
                was_empty
            } else {
                !had.contains(&folder_key)
            };
            match row.kind() {
                Kind::Folder => {
                    let place = row.place();
                    if folders.contains(&folder_keys(&place).last().unwrap_or_default()) {
                        continue;
                    }
                    folders.extend(folder_keys(&place));
                }
                Kind::Bookmark => {
                    let key = (folder_key, row.name().to_owned());
                    let n = seen.entry(key.clone()).or_insert(0);
                    let at = mine.get(&key).and_then(|rows| rows.get(*n)).copied();
                    *n += 1;
                    if let Some(i) = at {
                        if &self.rows()[i] != row {
                            self.replace(i, row.clone());
                            changed = true;
                        }
                        continue;
                    }
                    folders.extend(folder_keys(&folder));
                }
                Kind::Separator | Kind::Other => {
                    if !fresh {
                        continue;
                    }
                }
            }
            self.push(row.clone());
            changed = true;
        }
        changed
    }

    /// The rows of `items` (a folder with everything in it), with the rows
    /// of the folders they are in, in file order: a file that, imported,
    /// puts them back where they were.
    pub fn export(&self, tree: &Tree, items: &[Item]) -> Bookmarks {
        let mut keep = BTreeSet::new();
        let above = |path: &[String], keep: &mut BTreeSet<usize>| {
            for n in 1..=path.len() {
                if let Some(f) = tree.find(&path[..n]) {
                    keep.extend(tree.nodes[f].row);
                }
            }
        };
        for &item in items {
            match item {
                Item::Row(r) => {
                    keep.insert(r);
                    above(&self.rows()[r].folder(), &mut keep);
                }
                Item::Folder(f) => {
                    let path = &tree.nodes[f].path;
                    keep.extend(rows_of_folder(self, path));
                    above(&path[..path.len().saturating_sub(1)], &mut keep);
                }
            }
        }
        self.with_rows(keep.into_iter().map(|i| Arc::clone(&self.rows()[i])))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tree::tests::list;
    use super::super::{Settings, split};
    use super::*;

    /// The list as `kind folder/name` lines: `b Music/beatles`.
    fn rows(l: &Bookmarks) -> Vec<String> {
        l.rows()
            .iter()
            .map(|r| {
                let kind = match r.kind() {
                    Kind::Bookmark => "b",
                    Kind::Folder => "f",
                    Kind::Separator => "-",
                    Kind::Other => "?",
                };
                let mut path = r.folder();
                path.push(r.name().to_owned());
                format!("{kind} {}", path.join("/"))
            })
            .collect()
    }

    fn sample() -> Bookmarks {
        list(&[
            ("b", "", "one"),
            ("f", "", "Music"),
            ("b", "Music", "beatles"),
            ("f", "Music", "Rock"),
            ("b", "Music/Rock", "queen"),
            ("-", "", ""),
            ("b", "", "two"),
            ("b", "Music", "abba"),
        ])
    }

    fn row_id(l: &Bookmarks, name: &str) -> Id {
        let i = l.rows().iter().position(|r| r.name() == name).expect(name);
        Id::of(l, i)
    }

    #[test]
    fn ids_find_their_rows_again() {
        let l = list(&[
            ("b", "", "x"),
            ("b", "A", "x"),
            ("b", "", "x"),
            ("-", "", ""),
            ("-", "", ""),
        ]);
        let second = Id::of(&l, 2);
        assert_eq!((second.nth, second.find(&l)), (1, Some(2)));
        assert_eq!(Id::of(&l, 1).find(&l), Some(1));
        assert_eq!(Id::of(&l, 4).nth, 1);
        // Another list: the same identity, wherever it now is.
        let mut other = l.clone();
        other.remove(0);
        assert_eq!(second.find(&other), None, "only one x is left at the top");
        let a = Id::of(&l, 1);
        assert_eq!(a.find(&other), Some(0));
        // Folders are told apart regardless of case.
        let lower = list(&[("b", "a", "x")]);
        assert_eq!(a.find(&lower), Some(0));
    }

    #[test]
    fn adds_edits_and_deletes() {
        let mut l = sample();
        assert!(l.apply(&Op::Add(Bookmark::new(
            &split("Music"),
            "new",
            "n",
            &Settings::default()
        ))));
        assert_eq!(rows(&l).last().map(String::as_str), Some("b Music/new"));
        // An edit replaces the row in place.
        let id = row_id(&l, "two");
        let mut row = (*l.rows()[6]).clone();
        row.set_name("deux");
        assert!(l.apply(&Op::Edit {
            id: id.clone(),
            row: row.clone()
        }));
        assert_eq!(rows(&l)[6], "b deux");
        // The same edit again changes nothing.
        assert!(!l.apply(&Op::Edit {
            id: row_id(&l, "deux"),
            row: row.clone()
        }));
        // Deleted elsewhere, edited here: added again, at the end.
        let mut theirs = sample();
        theirs.remove(6);
        assert!(theirs.apply(&Op::Edit { id, row }));
        assert_eq!(rows(&theirs).last().map(String::as_str), Some("b deux"));
        // A folder goes with everything in it; a row by its identity.
        let mut l = sample();
        assert!(l.apply(&Op::Delete(vec![
            Target::Folder(split("music")),
            Target::Row(row_id(&sample(), "two")),
        ])));
        assert_eq!(rows(&l), ["b one", "- "]);
        // Nothing of it left: nothing done.
        assert!(!l.apply(&Op::Delete(vec![Target::Folder(split("Music"))])));
    }

    fn order(l: &Bookmarks, folder: &str) -> Vec<String> {
        let t = Tree::build(l);
        let node = t.find(&split(folder)).expect("folder");
        t.nodes[node]
            .items
            .iter()
            .map(|&i| match i {
                Item::Row(r) if l.rows()[r].kind() == Kind::Separator => "-".to_owned(),
                Item::Row(r) => l.rows()[r].name().to_owned(),
                Item::Folder(f) => format!("[{}]", t.nodes[f].name()),
            })
            .collect()
    }

    #[test]
    fn moves_up_and_down_within_a_folder() {
        let mut l = sample();
        assert_eq!(order(&l, ""), ["one", "[Music]", "-", "two"]);
        assert!(l.apply(&Op::Move {
            target: Target::Row(row_id(&l, "two")),
            up: true
        }));
        assert_eq!(order(&l, ""), ["one", "[Music]", "two", "-"]);
        // A folder moves with its rows; no other folder's order changes.
        assert!(l.apply(&Op::Move {
            target: Target::Folder(split("Music")),
            up: true
        }));
        assert_eq!(order(&l, ""), ["[Music]", "one", "two", "-"]);
        assert_eq!(order(&l, "Music"), ["beatles", "[Rock]", "abba"]);
        assert!(l.apply(&Op::Move {
            target: Target::Folder(split("Music")),
            up: false
        }));
        assert_eq!(order(&l, ""), ["one", "[Music]", "two", "-"]);
        assert!(l.apply(&Op::Move {
            target: Target::Row(row_id(&l, "abba")),
            up: true
        }));
        assert_eq!(order(&l, "Music"), ["beatles", "abba", "[Rock]"]);
        // At either end: nothing.
        assert!(!l.apply(&Op::Move {
            target: Target::Row(row_id(&l, "one")),
            up: true
        }));
        assert!(!l.apply(&Op::Move {
            target: Target::Folder(split("Music/Rock")),
            up: false
        }));
        // A folder with no row of its own moves by its first row.
        let mut l = list(&[
            ("b", "", "a"),
            ("b", "X", "x1"),
            ("b", "", "b"),
            ("b", "X", "x2"),
        ]);
        assert!(l.apply(&Op::Move {
            target: Target::Row(row_id(&l, "b")),
            up: true
        }));
        assert_eq!(order(&l, ""), ["a", "b", "[X]"]);
        assert_eq!(order(&l, "X"), ["x1", "x2"]);
    }

    #[test]
    fn moves_into_folders_and_renames() {
        let mut l = sample();
        assert!(l.apply(&Op::MoveTo {
            targets: vec![Target::Row(row_id(&l, "one"))],
            to: split("Music/Rock"),
        }));
        assert_eq!(order(&l, "Music/Rock"), ["queen", "one"]);
        // A folder takes everything in it along; not into itself.
        assert!(!l.apply(&Op::MoveTo {
            targets: vec![Target::Folder(split("Music"))],
            to: split("Music/Rock"),
        }));
        assert!(l.apply(&Op::MoveTo {
            targets: vec![Target::Folder(split("Music/Rock"))],
            to: Vec::new(),
        }));
        assert_eq!(order(&l, ""), ["[Music]", "-", "two", "[Rock]"]);
        assert_eq!(order(&l, "Rock"), ["queen", "one"]);
        assert_eq!(order(&l, "Music"), ["beatles", "abba"]);
        // Already there: nothing.
        assert!(!l.apply(&Op::MoveTo {
            targets: vec![Target::Folder(split("rock"))],
            to: Vec::new(),
        }));
        // A rename renames the folder's row and every row's folder below.
        let mut l = sample();
        assert!(l.apply(&Op::Rename {
            folder: split("MUSIC"),
            name: "Songs".into()
        }));
        assert_eq!(
            rows(&l),
            [
                "b one",
                "f Songs",
                "b Songs/beatles",
                "f Songs/Rock",
                "b Songs/Rock/queen",
                "- ",
                "b two",
                "b Songs/abba"
            ]
        );
        assert!(!l.apply(&Op::Rename {
            folder: split("Music"),
            name: "x".into()
        }));
    }

    #[test]
    fn imports_merge_without_doubling() {
        let mut l = sample();
        // The same file again: nothing new.
        assert!(!l.apply(&Op::Import(sample())));
        assert_eq!(l, sample());
        // A file with a changed bookmark, a new one, a separator in a known
        // folder and a new folder with a separator.
        let mut theirs = list(&[
            ("f", "", "Music"),
            ("b", "Music", "beatles"),
            ("-", "Music", ""),
            ("b", "Music", "new"),
            ("f", "", "Films"),
            ("-", "Films", ""),
            ("b", "Films", "alien"),
        ]);
        let mut changed = (*theirs.rows()[1]).clone();
        changed.set_search(Some("the beatles"));
        theirs.replace(1, changed);
        assert!(l.apply(&Op::Import(theirs.clone())));
        assert_eq!(
            rows(&l)[8..],
            ["b Music/new", "f Films", "- Films/", "b Films/alien"]
        );
        assert_eq!(l.rows()[2].search(), "the beatles");
        // Again: nothing.
        let before = l.clone();
        assert!(!l.apply(&Op::Import(theirs.clone())));
        assert_eq!(l, before);
        // Into an empty list, everything comes, separators too.
        let mut empty = Bookmarks::default();
        assert!(empty.apply(&Op::Import(sample())));
        assert_eq!(empty, sample());
    }

    #[test]
    fn exports_take_the_folders_they_are_in() {
        let l = sample();
        let t = Tree::build(&l);
        let rock = t.find(&split("Music/Rock")).expect("rock");
        let out = l.export(&t, &[Item::Folder(rock)]);
        assert_eq!(
            rows(&out),
            ["f Music", "f Music/Rock", "b Music/Rock/queen"]
        );
        let abba = l
            .rows()
            .iter()
            .position(|r| r.name() == "abba")
            .expect("abba");
        let out = l.export(&t, &[Item::Row(abba), Item::Row(0)]);
        assert_eq!(rows(&out), ["b one", "f Music", "b Music/abba"]);
        // Imported into an empty list, it is that again.
        let mut back = Bookmarks::default();
        back.apply(&Op::Import(out.clone()));
        assert_eq!(back, out);
    }
}
