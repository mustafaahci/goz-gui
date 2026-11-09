//! The bookmarks as folders: which rows each folder holds, in file order.
//!
//! A folder is where its row puts it, or, when no row makes it (a file of
//! Everything's edited by hand, a row of a folder deleted elsewhere), where
//! the first row inside it is. Folder names are told apart regardless of
//! case; two rows making the same folder are one folder.

use std::collections::HashMap;

use super::{Bookmarks, FolderPath, Kind};

/// One thing a folder holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A bookmark or a separator: its row.
    Row(usize),
    /// A folder: its node.
    Folder(usize),
}

/// A folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Its place; empty for the top.
    pub path: FolderPath,
    /// Its row, if one makes it.
    pub row: Option<usize>,
    /// What it holds, in order.
    pub items: Vec<Item>,
}

impl Node {
    /// Its name: the last of its path's ("" for the top).
    pub fn name(&self) -> &str {
        self.path.last().map_or("", String::as_str)
    }
}

/// The folders of a list. Node 0 is the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    by_key: HashMap<String, usize>,
}

/// A place as a map key: its names in lower case.
fn key(path: &[String]) -> String {
    path.iter()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

/// The top folder's node.
pub const TOP: usize = 0;

impl Tree {
    pub fn build(list: &Bookmarks) -> Tree {
        let mut tree = Tree {
            nodes: vec![Node {
                path: Vec::new(),
                row: None,
                items: Vec::new(),
            }],
            by_key: HashMap::from([(String::new(), TOP)]),
        };
        for (i, row) in list.rows().iter().enumerate() {
            match row.kind() {
                Kind::Bookmark | Kind::Separator => {
                    let at = tree.ensure(&row.folder());
                    tree.nodes[at].items.push(Item::Row(i));
                }
                Kind::Folder => {
                    let place = row.place();
                    let at = tree.ensure(&place);
                    if at != TOP && tree.nodes[at].row.is_none() {
                        // Its row's spelling, rather than that of a row
                        // inside it that came first.
                        tree.nodes[at].row = Some(i);
                        tree.nodes[at].path = place;
                    }
                }
                Kind::Other => {}
            }
        }
        tree
    }

    /// The node of `path`, made (with the folders above it) where it is
    /// first named.
    fn ensure(&mut self, path: &[String]) -> usize {
        if let Some(&at) = self.by_key.get(&key(path)) {
            return at;
        }
        let parent = self.ensure(&path[..path.len() - 1]);
        let at = self.nodes.len();
        self.nodes.push(Node {
            path: path.to_vec(),
            row: None,
            items: Vec::new(),
        });
        self.nodes[parent].items.push(Item::Folder(at));
        self.by_key.insert(key(path), at);
        at
    }

    /// The node of the folder at `path`.
    pub fn find(&self, path: &[String]) -> Option<usize> {
        self.by_key.get(&key(path)).copied()
    }

    /// The node holding `item`, and its index there.
    pub fn parent_of(&self, item: Item) -> Option<(usize, usize)> {
        self.nodes
            .iter()
            .enumerate()
            .find_map(|(n, node)| node.items.iter().position(|&i| i == item).map(|at| (n, at)))
    }

    /// Every folder below `node`, depth first in order, with its depth
    /// (1 for `node`'s own).
    pub fn folders_below(&self, node: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        self.walk_folders(node, 1, &mut out);
        out
    }

    fn walk_folders(&self, node: usize, depth: usize, out: &mut Vec<(usize, usize)>) {
        for &item in &self.nodes[node].items {
            if let Item::Folder(f) = item {
                out.push((f, depth));
                self.walk_folders(f, depth + 1, out);
            }
        }
    }

    /// The bookmarks (not separators) under `node`, depth first in the
    /// order the menu shows them.
    pub fn bookmarks_below(&self, list: &Bookmarks, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.walk_bookmarks(list, node, &mut out);
        out
    }

    fn walk_bookmarks(&self, list: &Bookmarks, node: usize, out: &mut Vec<usize>) {
        for &item in &self.nodes[node].items {
            match item {
                Item::Row(r) if list.rows()[r].kind() == Kind::Bookmark => out.push(r),
                Item::Row(_) => {}
                Item::Folder(f) => self.walk_bookmarks(list, f, out),
            }
        }
    }

    /// How many bookmarks and folders are under `node`.
    pub fn count_below(&self, list: &Bookmarks, node: usize) -> (usize, usize) {
        (
            self.bookmarks_below(list, node).len(),
            self.folders_below(node).len(),
        )
    }

    /// The first row of `item` in the file: where it is placed among the
    /// things its folder holds.
    pub fn first_row(&self, item: Item) -> Option<usize> {
        match item {
            Item::Row(r) => Some(r),
            Item::Folder(f) => {
                let node = &self.nodes[f];
                let inside = node.items.iter().filter_map(|&i| self.first_row(i)).min();
                match (node.row, inside) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::{Bookmark, Settings, split};
    use super::*;

    pub(crate) fn list(rows: &[(&str, &str, &str)]) -> Bookmarks {
        let mut list = Bookmarks::default();
        for &(kind, folder, name) in rows {
            let folder = split(folder);
            list.push(match kind {
                "f" => Bookmark::new_folder(&folder, name),
                "-" => Bookmark::new_separator(&folder),
                _ => Bookmark::new(&folder, name, name, &Settings::default()),
            });
        }
        list
    }

    fn shape(tree: &Tree, list: &Bookmarks, node: usize) -> String {
        tree.nodes[node]
            .items
            .iter()
            .map(|&item| match item {
                Item::Row(r) if list.rows()[r].kind() == Kind::Separator => "-".to_owned(),
                Item::Row(r) => list.rows()[r].name().to_owned(),
                Item::Folder(f) => format!("{}[{}]", tree.nodes[f].name(), shape(tree, list, f)),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn folders_hold_their_rows_in_file_order() {
        let l = list(&[
            ("b", "", "one"),
            ("f", "", "Music"),
            ("b", "Music", "beatles"),
            ("-", "", ""),
            ("b", "music/rock", "queen"),
            ("f", "Music", "Rock"),
            ("b", "", "two"),
            ("b", "Music", "abba"),
            ("f", "", "Empty"),
            ("f", "MUSIC", "rock"),
        ]);
        let t = Tree::build(&l);
        assert_eq!(
            shape(&t, &l, TOP),
            "one Music[beatles Rock[queen] abba] - two Empty[]"
        );
        // A folder named before its row is where it was first named, with
        // its row found; the second row making it changes nothing.
        let rock = t.find(&split("Music/Rock")).expect("rock");
        assert_eq!(t.nodes[rock].row, Some(5));
        assert_eq!(t.nodes[rock].path, ["Music", "Rock"]);
        assert_eq!(t.first_row(Item::Folder(rock)), Some(4));
        let music = t.find(&split("music")).expect("music");
        assert_eq!(t.first_row(Item::Folder(music)), Some(1));
        assert_eq!(t.parent_of(Item::Folder(rock)), Some((music, 1)));
        assert_eq!(t.parent_of(Item::Row(6)), Some((TOP, 3)));
        assert_eq!(
            t.folders_below(TOP)
                .iter()
                .map(|&(f, d)| (t.nodes[f].name().to_owned(), d))
                .collect::<Vec<_>>(),
            [
                ("Music".to_owned(), 1),
                ("Rock".to_owned(), 2),
                ("Empty".to_owned(), 1)
            ]
        );
        let names: Vec<&str> = t
            .bookmarks_below(&l, TOP)
            .iter()
            .map(|&r| l.rows()[r].name())
            .collect();
        assert_eq!(names, ["one", "beatles", "queen", "abba", "two"]);
        assert_eq!(t.count_below(&l, music), (3, 1));
        assert_eq!(t.find(&split("Nowhere")), None);
    }
}
