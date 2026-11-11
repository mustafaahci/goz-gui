//! The sidebars (Everything 1.5's Folders, Filters and Bookmarks panes): their
//! settings, and what of them is plain logic.
//!
//! - **Folders** scopes the search to the folder chosen in its tree: the
//!   folder goes to gozd as a quoted path term in front of what the user
//!   typed (`"C:\Users\me\" report`), which stays in the search box as it
//!   was, so editing the search keeps the folder. The term ends with a
//!   backslash, so only what is inside the folder matches, at any depth
//!   (Everything's Subfolders on). With a gozd that reads the new query
//!   language, Subfolders can be turned off: the folder then goes as
//!   `parent:"C:\Users\me"`, its own items only.
//! - **Bookmarks** shows the bookmarks as a tree under a box that filters
//!   them; which folders are open is kept (`bookmarks_sidebar_expanded`).
//!
//! The panes themselves are `window/folders_pane.rs`,
//! `window/bookmarks_pane.rs` and `window/filters_pane.rs`; the window's
//! side of the folder is `window/sidebars.rs`; the folder tree's file system
//! work is `crate::foldertree`.

use crate::bookmarks::tree::{Item, TOP, Tree};
use crate::bookmarks::{Bookmarks, Kind, one_line};
use crate::json::{self, Json};

/// The sidebars' settings, Everything's keys and defaults (the panes'
/// places and sizes are `crate::dock`'s).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `folders_sidebar_clear_selection_on_close` (0): closing the Folders
    /// pane forgets its folder.
    pub clear_on_close: bool,
    /// `folders_sidebar_match_only_when_shown` (1): the folder scopes the
    /// search only while the Folders pane is open; closed, it is kept for
    /// when it opens again.
    pub match_only_when_shown: bool,
    /// `folders_sidebar_labels` (1): drives show with their labels, as
    /// Explorer names them (`Windows (C:)`), rather than as `C:`.
    pub labels: bool,
    /// `folders_sidebar_subfolders` (1): the folder scopes the search with
    /// its subfolders; off, its own items only (`parent:`), which needs a
    /// gozd that reads the new query language.
    pub subfolders: bool,
    /// `bookmarks_sidebar_expanded` (`[]`): the bookmark folders open in
    /// the Bookmarks pane, each as its place (`Music/Rock`).
    pub bookmarks_expanded: Vec<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            clear_on_close: false,
            match_only_when_shown: true,
            labels: true,
            subfolders: true,
            bookmarks_expanded: Vec::new(),
        }
    }
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

fn bit(on: bool) -> String {
    u8::from(on).to_string()
}

impl Settings {
    /// The settings `get` gives (a key's value as the file holds it); what
    /// is missing or unreadable is the default.
    pub fn from_keys<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Settings {
        let d = Settings::default();
        Settings {
            clear_on_close: flag(get("folders_sidebar_clear_selection_on_close"))
                .unwrap_or(d.clear_on_close),
            match_only_when_shown: flag(get("folders_sidebar_match_only_when_shown"))
                .unwrap_or(d.match_only_when_shown),
            labels: flag(get("folders_sidebar_labels")).unwrap_or(d.labels),
            subfolders: flag(get("folders_sidebar_subfolders")).unwrap_or(d.subfolders),
            bookmarks_expanded: get("bookmarks_sidebar_expanded")
                .map(expanded_from_text)
                .unwrap_or_default(),
        }
    }

    /// The keys to write, in Everything's order.
    pub fn to_keys(&self) -> Vec<(String, String)> {
        vec![
            (
                "folders_sidebar_clear_selection_on_close".into(),
                bit(self.clear_on_close),
            ),
            ("folders_sidebar_labels".into(), bit(self.labels)),
            (
                "folders_sidebar_match_only_when_shown".into(),
                bit(self.match_only_when_shown),
            ),
            ("folders_sidebar_subfolders".into(), bit(self.subfolders)),
            (
                "bookmarks_sidebar_expanded".into(),
                expanded_to_text(&self.bookmarks_expanded),
            ),
        ]
    }
}

/// The most open bookmark folders kept: more than anyone opens by hand, and
/// a bound on the INI line.
const MAX_EXPANDED: usize = 256;

/// `bookmarks_sidebar_expanded`'s value: a JSON array of strings on one
/// line, as Everything writes its lists (`["Music","Music/Rock"]`).
pub fn expanded_to_text(folders: &[String]) -> String {
    let items = folders
        .iter()
        .take(MAX_EXPANDED)
        .map(|f| Json::str(f))
        .collect();
    json::to_text(&Json::Array(items)).trim_end().to_owned()
}

/// The folders in a `bookmarks_sidebar_expanded` value; nothing from a
/// value that is not an array, and no empty or repeated names.
pub fn expanded_from_text(text: &str) -> Vec<String> {
    let Ok(Json::Array(items)) = json::parse(text.trim().as_bytes()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for item in &items {
        if let Some(name) = item.as_text().filter(|n| !n.is_empty())
            && !out.iter().any(|o| o.eq_ignore_ascii_case(&name))
            && out.len() < MAX_EXPANDED
        {
            out.push(name);
        }
    }
    out
}

/// The folder as goz-gui keeps it: no trailing backslash, except for a
/// drive's root (`C:\`), and nothing but a path's characters at its ends.
pub fn normalize(folder: &str) -> String {
    let trimmed = folder.trim().trim_end_matches(['\\', '/']);
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else if trimmed.is_empty() && !folder.trim().is_empty() {
        // A lone backslash: the root of the current drive, which a scope
        // never names; kept as it came.
        folder.trim().to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The path term that keeps a search inside `folder`: quoted, with a
/// trailing backslash, so `C:\Users\me` matches `C:\Users\me\x` and not
/// `C:\Users\meg\x`.
pub fn scope_term(folder: &str) -> String {
    format!("\"{}\\\"", folder.trim_end_matches(['\\', '/']))
}

/// The term that keeps a search to `folder`'s own items, its subfolders'
/// left out (the new query language's `parent:`): quoted, without a
/// trailing backslash but for a drive's root.
pub fn parent_term(folder: &str) -> String {
    format!("parent:\"{}\"", normalize(folder))
}

/// The search gozd gets for `text` inside `folder`: the folder's term in
/// front of the text (the text alone without a folder).
#[cfg(test)]
pub fn scoped(folder: Option<&str>, text: &str) -> String {
    scoped_with(folder, text, true)
}

/// [`scoped`], the folder's subfolders left out unless `subfolders`.
pub fn scoped_with(folder: Option<&str>, text: &str, subfolders: bool) -> String {
    match folder {
        Some(f) if !f.is_empty() => {
            let term = if subfolders {
                scope_term(f)
            } else {
                parent_term(f)
            };
            if text.trim().is_empty() {
                term
            } else {
                format!("{term} {text}")
            }
        }
        _ => text.to_owned(),
    }
}

/// The folder holding `folder` (Up One Folder): `C:\a\b` gives `C:\a`,
/// `C:\a` gives `C:\`; `None` from a drive's root, which goes up to no
/// folder at all.
pub fn parent_folder(folder: &str) -> Option<String> {
    let f = normalize(folder);
    let body = f.trim_end_matches('\\');
    let cut = body.rfind('\\')?;
    Some(normalize(&body[..cut + 1]))
}

/// What the Bookmarks pane shows of one thing in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// A folder: its node in the bookmarks' [`Tree`] (of the same list),
    /// its place (`Music/Rock` as `["Music", "Rock"]`), its name, whether it
    /// shows open, and what it shows inside.
    Folder {
        node: usize,
        place: Vec<String>,
        name: String,
        open: bool,
        items: Vec<Shown>,
    },
    /// A bookmark: its row in the list, and its name on one line.
    Mark { row: usize, name: String },
}

/// A bookmark's name with its folders, `Music/Rock/Queen`: what the filter
/// box looks in, as Organize Bookmarks' Search does.
fn label(list: &Bookmarks, row: usize) -> String {
    let r = &list.rows()[row];
    let mut path = r.folder();
    path.push(one_line(r.name()));
    path.join("/")
}

/// What the Bookmarks pane shows for `list`: every folder and bookmark in
/// the menu's order (separators are the menu's alone), the folders in
/// `expanded` open. With a `filter`, only the bookmarks whose name (with its
/// folders) or search holds it, regardless of case, inside the folders that
/// hold them, which then all show open.
pub fn bookmark_view(list: &Bookmarks, filter: &str, expanded: &[String]) -> Vec<Shown> {
    let tree = Tree::build(list);
    let needle = filter.trim().to_lowercase();
    let wanted = |row: usize| {
        needle.is_empty()
            || label(list, row).to_lowercase().contains(&needle)
            || list.rows()[row].search().to_lowercase().contains(&needle)
    };
    let open = |node: usize| {
        let place = tree.nodes[node].path.join("/");
        expanded.iter().any(|e| e.eq_ignore_ascii_case(&place))
    };
    fn walk(
        list: &Bookmarks,
        tree: &Tree,
        node: usize,
        filtering: bool,
        wanted: &dyn Fn(usize) -> bool,
        open: &dyn Fn(usize) -> bool,
    ) -> Vec<Shown> {
        let mut out = Vec::new();
        for &item in &tree.nodes[node].items {
            match item {
                Item::Row(r) if list.rows()[r].kind() == Kind::Bookmark && wanted(r) => {
                    out.push(Shown::Mark {
                        row: r,
                        name: one_line(list.rows()[r].name()),
                    });
                }
                Item::Row(_) => {}
                Item::Folder(f) => {
                    let items = walk(list, tree, f, filtering, wanted, open);
                    if filtering && items.is_empty() {
                        continue;
                    }
                    out.push(Shown::Folder {
                        node: f,
                        place: tree.nodes[f].path.clone(),
                        name: tree.nodes[f].name().to_owned(),
                        open: filtering || open(f),
                        items,
                    });
                }
            }
        }
        out
    }
    walk(list, &tree, TOP, !needle.is_empty(), &wanted, &open)
}

/// The first bookmark `shown` lists, depth first: what Enter in the filter
/// box opens when nothing is chosen in the tree.
pub fn first_mark(shown: &[Shown]) -> Option<usize> {
    shown.iter().find_map(|s| match s {
        Shown::Mark { row, .. } => Some(*row),
        Shown::Folder { items, .. } => first_mark(items),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::tree::tests::list;
    use goz_core::query::parse_query;

    #[test]
    fn settings_read_back_as_written() {
        let d = Settings::default();
        assert!(!d.clear_on_close && d.match_only_when_shown && d.labels);
        assert!(d.bookmarks_expanded.is_empty());
        let keys = d.to_keys();
        let get = |k: &str| keys.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("bookmarks_sidebar_expanded"), Some("[]"));
        assert_eq!(get("folders_sidebar_match_only_when_shown"), Some("1"));
        assert_eq!(Settings::from_keys(get), d);

        let s = Settings {
            clear_on_close: true,
            match_only_when_shown: false,
            labels: false,
            subfolders: false,
            bookmarks_expanded: vec!["Music".into(), "Music/Rock \"live\"".into()],
        };
        let keys = s.to_keys();
        let get = |k: &str| keys.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(
            get("bookmarks_sidebar_expanded"),
            Some(r#"["Music","Music/Rock \"live\""]"#)
        );
        assert_eq!(Settings::from_keys(get), s);
    }

    #[test]
    fn unreadable_settings_are_the_defaults() {
        let bad = |k: &str| match k {
            "folders_sidebar_labels" => Some("maybe"),
            "bookmarks_sidebar_expanded" => Some("{not a list"),
            "folders_sidebar_match_only_when_shown" => Some("false"),
            _ => None,
        };
        let s = Settings::from_keys(bad);
        assert!(s.labels);
        assert!(!s.match_only_when_shown);
        assert!(s.bookmarks_expanded.is_empty());
        // Empty names and repeats (in any case) are dropped.
        assert_eq!(
            expanded_from_text(r#"["a","","A",3,"b"]"#),
            ["a".to_owned(), "b".to_owned()]
        );
        assert!(expanded_from_text("").is_empty());
    }

    #[test]
    fn a_folder_is_kept_without_its_trailing_backslash() {
        assert_eq!(normalize(r"C:\Users\me\"), r"C:\Users\me");
        assert_eq!(normalize(r"C:\Users\me"), r"C:\Users\me");
        assert_eq!(normalize("C:"), r"C:\");
        assert_eq!(normalize(r"C:\"), r"C:\");
        assert_eq!(normalize(r" D:\x\\ "), r"D:\x");
        assert_eq!(normalize(r"\\server\share\"), r"\\server\share");
    }

    #[test]
    fn the_scope_goes_in_front_of_the_text_as_a_path_term() {
        assert_eq!(scope_term(r"C:\Users\me"), r#""C:\Users\me\""#);
        assert_eq!(scope_term(r"C:\"), r#""C:\""#);
        assert_eq!(
            scoped(Some(r"C:\Program Files"), "report"),
            r#""C:\Program Files\" report"#
        );
        assert_eq!(scoped(Some(r"C:\x"), "  "), r#""C:\x\""#);
        assert_eq!(scoped(None, "report"), "report");
        assert_eq!(scoped(Some(""), "a b"), "a b");
    }

    #[test]
    fn subfolders_off_keeps_the_folders_own_items() {
        assert_eq!(
            scoped_with(Some(r"C:\Program Files"), "report", false),
            r#"parent:"C:\Program Files" report"#
        );
        assert_eq!(scoped_with(Some(r"C:\"), "", false), r#"parent:"C:\""#);
        assert_eq!(
            scoped_with(Some(r"D:\x\"), "a|b", false),
            r#"parent:"D:\x" a|b"#
        );
        assert_eq!(
            scoped_with(Some(r"C:\x"), "report", true),
            scoped(Some(r"C:\x"), "report")
        );
        assert!(Settings::default().subfolders);
        let off = Settings::from_keys(|k| (k == "folders_sidebar_subfolders").then_some("0"));
        assert!(!off.subfolders);
        assert!(
            off.to_keys()
                .contains(&("folders_sidebar_subfolders".into(), "0".into()))
        );
    }

    #[test]
    fn goz_reads_the_scope_as_a_path_term() {
        for (folder, text, term) in [
            (r"C:\Program Files", "report", r"c:\program files\"),
            (r"C:\", "", r"c:\"),
            (r"D:\a b\c", "ext:pdf size:>1mb", r"d:\a b\c\"),
            (r"E:\x", "*.txt", r"e:\x\"),
        ] {
            let q = scoped(Some(folder), text);
            let parsed = parse_query(&q).unwrap_or_else(|e| panic!("{q}: {e}"));
            assert_eq!(
                parsed.path_terms,
                [term.as_bytes().to_vec()],
                "{q} as a path term"
            );
        }
        // The user's own terms stay theirs.
        let parsed = parse_query(&scoped(Some(r"C:\x"), "report")).expect("parses");
        assert_eq!(parsed.name_terms, [b"report".to_vec()]);
        // With a filter and Match Path in front, as the window builds it.
        let q = crate::filter::query(
            crate::filter::Filter::Audio,
            true,
            &scoped(Some(r"C:\Music"), "live"),
        );
        let parsed = parse_query(&q).unwrap_or_else(|e| panic!("{q}: {e}"));
        // Match Path makes the user's term a path term too.
        assert_eq!(
            parsed.path_terms,
            [br"c:\music\".to_vec(), b"live".to_vec()]
        );
        assert!(parsed.filters.ext.is_some());
    }

    #[test]
    fn up_one_folder_stops_at_no_folder() {
        assert_eq!(parent_folder(r"C:\a\b").as_deref(), Some(r"C:\a"));
        assert_eq!(parent_folder(r"C:\a\b\").as_deref(), Some(r"C:\a"));
        assert_eq!(parent_folder(r"C:\a").as_deref(), Some(r"C:\"));
        assert_eq!(parent_folder(r"C:\"), None);
        assert_eq!(parent_folder("C:"), None);
        assert_eq!(parent_folder(""), None);
    }

    fn sample() -> Bookmarks {
        list(&[
            ("b", "", "one"),
            ("f", "", "Music"),
            ("b", "Music/Rock", "queen"),
            ("-", "", ""),
            ("b", "Music", "abba"),
            ("f", "", "Empty"),
            ("b", "", "two"),
        ])
    }

    fn names(shown: &[Shown]) -> String {
        shown
            .iter()
            .map(|s| match s {
                Shown::Mark { name, .. } => name.clone(),
                Shown::Folder {
                    name, open, items, ..
                } => format!("{name}{}[{}]", if *open { "+" } else { "" }, names(items)),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_pane_shows_the_folders_as_the_menu_does() {
        let l = sample();
        let v = bookmark_view(&l, "", &[]);
        assert_eq!(names(&v), "one Music[Rock[queen] abba] Empty[] two");
        // The open folders, by place, in any case.
        let v = bookmark_view(&l, "", &["music/rock".into(), "Music".into()]);
        assert_eq!(names(&v), "one Music+[Rock+[queen] abba] Empty[] two");
        assert_eq!(first_mark(&v), Some(0));
    }

    #[test]
    fn the_filter_keeps_matches_inside_their_folders() {
        let l = sample();
        // By name.
        let v = bookmark_view(&l, "QUEEN", &[]);
        assert_eq!(names(&v), "Music+[Rock+[queen]]");
        assert_eq!(first_mark(&v), Some(2));
        // By a folder's name: what is inside it.
        let v = bookmark_view(&l, "rock", &[]);
        assert_eq!(names(&v), "Music+[Rock+[queen]]");
        let v = bookmark_view(&l, "music", &[]);
        assert_eq!(names(&v), "Music+[Rock+[queen] abba]");
        // Nothing: no empty folders either.
        assert!(bookmark_view(&l, "zzz", &[]).is_empty());
        assert_eq!(first_mark(&[]), None);
    }

    #[test]
    fn the_filter_looks_in_the_search_too() {
        let mut l = Bookmarks::default();
        l.push(crate::bookmarks::Bookmark::new(
            &[],
            "Pictures",
            "ext:jpg;png",
            &crate::bookmarks::Settings::default(),
        ));
        l.push(crate::bookmarks::Bookmark::new(
            &[],
            "Docs",
            "ext:pdf",
            &crate::bookmarks::Settings::default(),
        ));
        let v = bookmark_view(&l, "PNG", &[]);
        assert_eq!(names(&v), "Pictures");
    }
}
