//! goz-gui's command line: the subset of Everything's switches that makes
//! sense for goz, read the same way whether goz-gui starts with it or a
//! running goz-gui is handed it by a second start (see `crate::instance`).
//!
//! Switches start with `-` or `/` and are matched without case:
//! `-s`/`-search <text>`, `-p`/`-path <folder>`, `-filter <name>`,
//! `-sort <name>` with `-sort-ascending`/`-sort-descending`,
//! `-case`/`-nocase`, `-matchpath`/`-nomatchpath`, `-startup` (start in the
//! background, no window), `-toggle-window`, `-exit` (close the running
//! goz-gui), `-instance <name>`, `-preview`/`-no-preview` (show or hide the
//! preview pane). Anything else is search text, as if after `-s`.

use goz_core::types::{SortDir, SortKey};

use crate::bookmarks::sort_key;
use crate::filter::Filter;

/// What a command line asks for. Fields left `None` keep the window's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub search: Option<String>,
    /// Search this folder (and its subfolders): its path, quoted, before
    /// the search text.
    pub path: Option<String>,
    pub filter: Option<Filter>,
    pub sort: Option<SortKey>,
    pub dir: Option<SortDir>,
    pub match_case: Option<bool>,
    pub match_path: Option<bool>,
    /// Start without showing the window (starting with Windows).
    pub startup: bool,
    pub toggle_window: bool,
    pub exit: bool,
    /// A named instance: its own single-instance key.
    pub instance: Option<String>,
    /// `-preview` / `-no-preview`: show or hide the preview pane.
    pub preview: Option<bool>,
}

impl Launch {
    /// Whether it asks for anything about the search itself.
    pub fn changes_search(&self) -> bool {
        self.search.is_some()
            || self.path.is_some()
            || self.filter.is_some()
            || self.sort.is_some()
            || self.dir.is_some()
            || self.match_case.is_some()
            || self.match_path.is_some()
    }

    /// The search text to put in the box: the folder as a quoted path term
    /// (with its trailing backslash, so only what is inside it matches),
    /// then the text.
    pub fn search_text(&self) -> Option<String> {
        let folder = self.path.as_deref().map(|p| {
            let p = p.trim_end_matches(['\\', '/']);
            format!("\"{p}\\\"")
        });
        match (folder, self.search.as_deref()) {
            (None, None) => None,
            (Some(f), None) | (Some(f), Some("")) => Some(f),
            (None, Some(s)) => Some(s.to_owned()),
            (Some(f), Some(s)) => Some(format!("{f} {s}")),
        }
    }
}

/// The switch's name without its `-` or `/`, lower-cased; `None` for text.
fn switch(arg: &str) -> Option<String> {
    let rest = arg.strip_prefix('-').or_else(|| arg.strip_prefix('/'))?;
    // A lone "-" or "/" is text; so is a path such as /usr or -1.
    if rest.is_empty() || !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(rest.to_ascii_lowercase())
}

/// Reads `args` (without the program's own name). An unknown switch, or one
/// missing its value, is an error naming it.
pub fn parse(args: &[String]) -> Result<Launch, String> {
    let mut l = Launch::default();
    let mut text: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        let Some(name) = switch(arg) else {
            text.push(arg);
            continue;
        };
        let mut value = |what: &str| -> Result<String, String> {
            let v = args
                .get(i)
                .cloned()
                .ok_or_else(|| format!("{arg} needs {what}"))?;
            i += 1;
            Ok(v)
        };
        match name.as_str() {
            "s" | "search" => l.search = Some(value("a search")?),
            "p" | "path" => l.path = Some(value("a folder")?),
            "filter" => {
                let v = value("a filter name")?;
                l.filter = Some(
                    Filter::from_title(&v)
                        .or_else(|| {
                            v.eq_ignore_ascii_case("everything")
                                .then_some(Filter::Everything)
                        })
                        .ok_or_else(|| format!("unknown filter: {v}"))?,
                );
            }
            "sort" => {
                let v = value("a sort name")?;
                l.sort = Some(sort_by_name(&v).ok_or_else(|| format!("unknown sort: {v}"))?);
            }
            "sort-ascending" => l.dir = Some(SortDir::Asc),
            "sort-descending" => l.dir = Some(SortDir::Desc),
            "case" => l.match_case = Some(true),
            "nocase" => l.match_case = Some(false),
            "matchpath" => l.match_path = Some(true),
            "nomatchpath" => l.match_path = Some(false),
            "startup" => l.startup = true,
            "toggle-window" => l.toggle_window = true,
            "exit" | "quit" => l.exit = true,
            "instance" => l.instance = Some(value("an instance name")?),
            "preview" => l.preview = Some(true),
            "no-preview" => l.preview = Some(false),
            _ => return Err(format!("unknown switch: {arg}")),
        }
    }
    if l.search.is_none() && !text.is_empty() {
        l.search = Some(text.join(" "));
    }
    Ok(l)
}

/// A sort by Everything's name for it (`Name`, `Path`, `Size`,
/// `Date Modified`, also written `date-modified` or `dm`), any case.
fn sort_by_name(name: &str) -> Option<SortKey> {
    let n = name.trim().to_ascii_lowercase().replace(['-', '_'], " ");
    match n.as_str() {
        "dm" | "date modified" | "datemodified" => Some(SortKey::DateModified),
        "name" => Some(SortKey::Name),
        "path" => Some(SortKey::Path),
        "size" => Some(SortKey::Size),
        _ => sort_key(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Launch, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn search_switches_and_bare_text() {
        assert_eq!(
            p(&["-s", "report.pdf"]).unwrap().search.as_deref(),
            Some("report.pdf")
        );
        assert_eq!(p(&["/SEARCH", "x"]).unwrap().search.as_deref(), Some("x"));
        assert_eq!(
            p(&["annual", "report"]).unwrap().search.as_deref(),
            Some("annual report")
        );
        // -s wins over bare text.
        assert_eq!(p(&["a", "-s", "b"]).unwrap().search.as_deref(), Some("b"));
        assert_eq!(p(&[]).unwrap(), Launch::default());
    }

    #[test]
    fn options_and_sorts() {
        let l = p(&[
            "-filter",
            "Audio",
            "-sort",
            "Size",
            "-sort-ascending",
            "-case",
            "-nomatchpath",
        ])
        .unwrap();
        assert_eq!(l.filter, Some(Filter::Audio));
        assert_eq!(l.sort, Some(SortKey::Size));
        assert_eq!(l.dir, Some(SortDir::Asc));
        assert_eq!(l.match_case, Some(true));
        assert_eq!(l.match_path, Some(false));
        assert_eq!(
            p(&["-sort", "date-modified"]).unwrap().sort,
            Some(SortKey::DateModified)
        );
        assert_eq!(
            p(&["-filter", "everything"]).unwrap().filter,
            Some(Filter::Everything)
        );
        assert!(l.changes_search());
    }

    #[test]
    fn process_switches() {
        let l = p(&["-startup", "-instance", "work"]).unwrap();
        assert!(l.startup && !l.exit && !l.changes_search());
        assert_eq!(l.instance.as_deref(), Some("work"));
        assert!(p(&["-exit"]).unwrap().exit);
        assert!(p(&["-toggle-window"]).unwrap().toggle_window);
        // Everything's pane switches.
        assert_eq!(p(&["-preview"]).unwrap().preview, Some(true));
        assert_eq!(p(&["/No-Preview"]).unwrap().preview, Some(false));
        assert!(!p(&["-preview"]).unwrap().changes_search());
        assert_eq!(p(&[]).unwrap().preview, None);
    }

    #[test]
    fn errors_name_the_switch() {
        assert_eq!(p(&["-s"]).unwrap_err(), "-s needs a search");
        assert_eq!(p(&["-bogus"]).unwrap_err(), "unknown switch: -bogus");
        assert!(p(&["-sort", "bogus"]).is_err());
        assert!(p(&["-filter", "bogus"]).is_err());
    }

    #[test]
    fn paths_and_negative_numbers_are_text() {
        assert_eq!(p(&["/", "-1"]).unwrap().search.as_deref(), Some("/ -1"));
    }

    #[test]
    fn folder_becomes_a_quoted_path_term() {
        let l = p(&["-path", r"C:\Users\me\"]).unwrap();
        assert_eq!(l.search_text().as_deref(), Some(r#""C:\Users\me\""#));
        let l = p(&["-p", r"D:\Docs", "-s", "invoice"]).unwrap();
        assert_eq!(l.search_text().as_deref(), Some(r#""D:\Docs\" invoice"#));
        assert_eq!(p(&["x"]).unwrap().search_text().as_deref(), Some("x"));
        assert_eq!(Launch::default().search_text(), None);
    }
}
