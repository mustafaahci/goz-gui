//! `Session.json`: the window as it was, for History > Restore Previous
//! Session.
//!
//! The file has the shape of Everything 1.5's own `Session.json` (an array
//! of windows, each with its tabs) and uses its key names, so a session can
//! be carried between the two: goz-gui reads the first open window of
//! Everything's file and its current tab (`tab_cursel`). goz-gui has one
//! window and one "tab" and writes only what it keeps:
//!
//! - the window: `window_x`, `window_y`, `window_wide`, `window_high`
//!   (screen pixels, as the INI's) with goz-gui's `window_dpi`,
//!   `maximized`, `ontop`, `theme`, `status_bar_visible`;
//! - the tab: `search`, `match_case`, `match_path`, `filter`, `sort`,
//!   `sort_ascending`, `listview_item_focus` and `listview_selection` (full
//!   paths, as Everything's), and goz-gui's own `listview_top_index` (the
//!   row at the top of the list), `nav_history` (Back and Forward's entries,
//!   oldest first, each with the search keys above) and `nav_history_index`
//!   (the current one).
//!
//! Unknown keys are ignored. The file is replaced as a whole: the session
//! written last wins. `SessionFile` writes it on the store thread, like
//! every other file goz-gui keeps (`crate::store`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use goz_core::types::{SortDir, SortSpec};

use crate::bookmarks::{sort_key, sort_name};
use crate::filter::Filter;
use crate::history::NavHistory;
use crate::json::{self, Json};
use crate::settings::{self, OnTop, WindowPos};
use crate::store::file::{self, Loaded};
use crate::theme::ThemeMode;

/// The file's name in the data folder, as Everything's.
pub const FILE_NAME: &str = "Session.json";
/// The most selected files kept (by path): enough for any selection a
/// person makes by hand, and a bound on the file's size.
pub const MAX_SELECTION: usize = 1000;
/// How long a read or write waits for another goz-gui on the same file.
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// A search with its options, as the window shows it and as Back and
/// Forward bring it back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    pub text: String,
    pub match_case: bool,
    pub match_path: bool,
    pub filter: Filter,
    pub sort: SortSpec,
    /// The Folders sidebar's folder, which keeps the search inside it
    /// (Everything's `folders_sidebar_selection`, a list of one here).
    pub folder: Option<String>,
}

/// The results as they were: the search, where the list was scrolled to,
/// what was selected, and the navigation history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub search: Search,
    /// The row at the top of the list.
    pub top: u64,
    /// The focused file's full path.
    pub focus: Option<Vec<u16>>,
    /// The selected files' full paths.
    pub selection: Vec<Vec<u16>>,
    /// Back and Forward's entries, oldest first.
    pub nav: Vec<Search>,
    /// The current entry of `nav`.
    pub nav_index: usize,
}

/// A window as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The normal (not minimized or maximized) rectangle.
    pub window: Option<WindowPos>,
    pub maximized: bool,
    pub on_top: OnTop,
    pub theme: ThemeMode,
    pub status_bar: bool,
    pub view: View,
}

impl Default for Session {
    fn default() -> Session {
        Session {
            window: None,
            maximized: false,
            on_top: OnTop::Never,
            theme: ThemeMode::UserDefault,
            status_bar: true,
            view: View::default(),
        }
    }
}

fn int(o: &Json, key: &str) -> Option<i64> {
    o.get(key).and_then(Json::as_int)
}

fn flag(o: &Json, key: &str) -> Option<bool> {
    o.get(key).and_then(Json::as_flag)
}

/// A search's keys. `tab`: a tab's, which always has the folder's key, as
/// Everything's; a navigation entry has it only with a folder, so one
/// without stays on one line.
fn search_members(s: &Search, tab: bool) -> Vec<(String, Json)> {
    let mut members = vec![
        ("search".into(), Json::str(&s.text)),
        ("match_case".into(), Json::bit(s.match_case)),
        ("match_path".into(), Json::bit(s.match_path)),
        (
            "filter".into(),
            Json::str(match s.filter {
                Filter::Everything => "",
                f => f.title(),
            }),
        ),
        ("sort".into(), Json::str(sort_name(s.sort.key))),
        (
            "sort_ascending".into(),
            Json::bit(s.sort.dir == SortDir::Asc),
        ),
    ];
    if tab || s.folder.is_some() {
        members.push((
            "folders_sidebar_selection".into(),
            Json::Array(s.folder.iter().map(|f| Json::str(f)).collect()),
        ));
    }
    members
}

/// The search keys of a tab or a navigation entry; what is missing or
/// unreadable (a filter or a sort goz-gui does not have) is the default.
fn read_search(o: &Json) -> Search {
    let sort = o
        .get("sort")
        .and_then(Json::as_text)
        .and_then(|name| sort_key(&name))
        .map_or_else(SortSpec::default, |key| SortSpec {
            key,
            dir: match flag(o, "sort_ascending") {
                Some(true) => SortDir::Asc,
                Some(false) => SortDir::Desc,
                None => SortSpec::default_for(key).dir,
            },
        });
    Search {
        text: o.get("search").and_then(Json::as_text).unwrap_or_default(),
        match_case: flag(o, "match_case").unwrap_or(false),
        match_path: flag(o, "match_path").unwrap_or(false),
        filter: o
            .get("filter")
            .and_then(Json::as_text)
            .and_then(|t| Filter::from_title(&t))
            .unwrap_or_default(),
        sort,
        // The first folder of Everything's list (goz-gui scopes to one).
        folder: o
            .get("folders_sidebar_selection")
            .and_then(Json::as_array)
            .and_then(|a| {
                a.iter()
                    .find_map(|v| v.as_text().filter(|t| !t.trim().is_empty()))
            })
            .map(|f| crate::sidebars::normalize(&f)),
    }
}

/// The window rectangle, if the four keys hold a sane one.
fn read_window(o: &Json) -> Option<WindowPos> {
    let (x, y, w, h) = (
        int(o, "window_x")?,
        int(o, "window_y")?,
        int(o, "window_wide")?,
        int(o, "window_high")?,
    );
    let sane = (1..=100_000).contains(&w)
        && (1..=100_000).contains(&h)
        && x.abs() <= 100_000
        && y.abs() <= 100_000;
    sane.then(|| WindowPos {
        x: x as i32,
        y: y as i32,
        width: w as i32,
        height: h as i32,
        dpi: int(o, "window_dpi")
            .filter(|d| (48..=1920).contains(d))
            .map(|d| d as u32),
    })
}

fn is_open(o: &Json) -> bool {
    flag(o, "is_closed") != Some(true)
}

fn read_view(tab: &Json) -> View {
    let mut nav: Vec<Search> = tab
        .get("nav_history")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|e| matches!(e, Json::Object(_)))
        .map(read_search)
        .collect();
    let mut nav_index = int(tab, "nav_history_index")
        .and_then(|i| usize::try_from(i).ok())
        .unwrap_or(usize::MAX);
    // The newest entries, as the window keeps them.
    let max = NavHistory::<Search>::MAX;
    if nav.len() > max {
        let cut = nav.len() - max;
        nav.drain(..cut);
        nav_index = nav_index.saturating_sub(cut);
    }
    let nav_index = nav_index.min(nav.len().saturating_sub(1));
    let path = |v: &Json| v.as_wide().filter(|p| !p.is_empty()).map(<[u16]>::to_vec);
    View {
        search: read_search(tab),
        top: int(tab, "listview_top_index")
            .and_then(|t| u64::try_from(t).ok())
            .unwrap_or(0),
        focus: tab.get("listview_item_focus").and_then(path),
        selection: tab
            .get("listview_selection")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(path)
            .take(MAX_SELECTION)
            .collect(),
        nav,
        nav_index,
    }
}

impl Session {
    /// The session in a parsed `Session.json`: its first open window (the
    /// first window if all are closed), and that window's current tab.
    /// `None` when it holds no window; `Err` when it is not a session.
    pub fn from_json(doc: &Json) -> Result<Option<Session>, String> {
        let windows: &[Json] = match doc {
            Json::Array(items) => items,
            Json::Object(_) => core::slice::from_ref(doc),
            _ => return Err("it is not a session (no window in it)".into()),
        };
        let Some(win) = windows
            .iter()
            .find(|w| is_open(w))
            .or_else(|| windows.first())
        else {
            return Ok(None);
        };
        if !matches!(win, Json::Object(_)) {
            return Err("it is not a session (a window is not an object)".into());
        }
        let tabs = win.get("tabs").and_then(Json::as_array).unwrap_or_default();
        let current = int(win, "tab_cursel")
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| tabs.get(i))
            .filter(|t| is_open(t));
        let tab = current
            .or_else(|| tabs.iter().find(|t| is_open(t)))
            .or_else(|| tabs.first());
        let d = Session::default();
        Ok(Some(Session {
            window: read_window(win),
            maximized: flag(win, "maximized").unwrap_or(d.maximized),
            on_top: int(win, "ontop")
                .and_then(OnTop::from_number)
                .unwrap_or(d.on_top),
            theme: int(win, "theme")
                .and_then(settings::theme_from)
                .unwrap_or(d.theme),
            status_bar: flag(win, "status_bar_visible").unwrap_or(d.status_bar),
            view: tab.map(read_view).unwrap_or_default(),
        }))
    }

    /// The session as `Session.json` holds it: one window with one tab.
    pub fn to_json(&self) -> Json {
        let mut win: Vec<(String, Json)> = vec![("is_closed".into(), Json::int(0))];
        if let Some(w) = self.window {
            win.push(("window_x".into(), Json::int(w.x.into())));
            win.push(("window_y".into(), Json::int(w.y.into())));
            win.push(("window_wide".into(), Json::int(w.width.into())));
            win.push(("window_high".into(), Json::int(w.height.into())));
            if let Some(dpi) = w.dpi {
                win.push(("window_dpi".into(), Json::int(dpi.into())));
            }
        }
        win.push(("maximized".into(), Json::bit(self.maximized)));
        win.push(("ontop".into(), Json::int(self.on_top.number().into())));
        win.push((
            "theme".into(),
            Json::int(settings::theme_number(self.theme).into()),
        ));
        win.push(("status_bar_visible".into(), Json::bit(self.status_bar)));
        win.push(("tab_cursel".into(), Json::int(0)));
        let v = &self.view;
        let mut tab: Vec<(String, Json)> = vec![("is_closed".into(), Json::int(0))];
        tab.extend(search_members(&v.search, true));
        tab.push((
            "listview_top_index".into(),
            Json::int(i64::try_from(v.top).unwrap_or(i64::MAX)),
        ));
        tab.push((
            "listview_item_focus".into(),
            Json::Str(v.focus.clone().unwrap_or_default()),
        ));
        tab.push((
            "listview_selection".into(),
            Json::Array(
                v.selection
                    .iter()
                    .take(MAX_SELECTION)
                    .map(|p| Json::Str(p.clone()))
                    .collect(),
            ),
        ));
        tab.push((
            "nav_history_index".into(),
            Json::int(i64::try_from(v.nav_index).unwrap_or(0)),
        ));
        tab.push((
            "nav_history".into(),
            Json::Array(
                v.nav
                    .iter()
                    .map(|s| Json::Object(search_members(s, false)))
                    .collect(),
            ),
        ));
        win.push(("tabs".into(), Json::Array(vec![Json::Object(tab)])));
        Json::Array(vec![Json::Object(win)])
    }

    pub fn to_text(&self) -> String {
        json::to_text(&self.to_json())
    }
}

/// Reads a `Session.json`. `Err` when it is not JSON or not a session (a
/// power cut, another file saved over it): `crate::store::file::load` then
/// moves it aside and tries the backup.
pub fn parse(bytes: &[u8]) -> Result<Option<Session>, String> {
    Session::from_json(&json::parse(bytes)?)
}

/// The session saved last (by any goz-gui), read at startup, and what to
/// tell the user when the file could not be read.
pub fn load(path: &Path) -> (Option<Session>, Option<String>) {
    // Another goz-gui writing or recovering the file goes first.
    let _lock = file::Lock::acquire(path, LOCK_WAIT);
    match file::load(path, parse) {
        Loaded::Missing => (None, None),
        Loaded::Read(session, _) => (session, None),
        Loaded::Recovered {
            backup,
            moved_to,
            why,
            ..
        } => {
            let name = moved_to
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let session = backup.flatten();
            let note = if session.is_some() {
                format!(
                    "The session file could not be read ({why}); it was kept as {name} and its \
                     backup was used"
                )
            } else {
                format!(
                    "The session file could not be read ({why}); it was kept as {name}, and there \
                     is no previous session"
                )
            };
            (session, Some(note))
        }
        Loaded::Unreadable(why) => (
            None,
            Some(format!("The session file could not be read: {why}")),
        ),
    }
}

/// The session file, shared by the window (which stages the text to write)
/// and the store thread (which writes it).
pub struct SessionFile {
    path: PathBuf,
    backup: bool,
    /// The text to write next.
    staged: Mutex<Option<String>>,
}

impl SessionFile {
    pub fn new(path: PathBuf, backup: bool) -> SessionFile {
        SessionFile {
            path,
            backup,
            staged: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Option<String>> {
        self.staged.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `text` is what the next write puts in the file.
    pub fn stage(&self, text: String) {
        *self.lock() = Some(text);
    }

    /// A text staged and not written yet (a write failed): the next save
    /// writes it even when the window did not change.
    pub fn pending(&self) -> bool {
        self.lock().is_some()
    }

    /// Writes the staged session over the file (the last one written wins),
    /// unless the file holds the same text. A file there that is not a
    /// session is moved aside first, never written over unseen; one that
    /// cannot be read at all is left alone, and the text stays staged.
    /// Run on the store thread. `Ok(Some)`: written, with something to
    /// tell the user (the backup could not be updated).
    pub fn write_now(&self) -> Result<Option<String>, String> {
        let Some(text) = self.lock().take() else {
            return Ok(None);
        };
        let result = self.write_text(&text);
        if result.is_err() {
            // Staged again meanwhile: the newer text stays.
            let mut staged = self.lock();
            if staged.is_none() {
                *staged = Some(text);
            }
        }
        result
    }

    fn write_text(&self, text: &str) -> Result<Option<String>, String> {
        let path = &self.path;
        // Not written over another goz-gui's write: tried again later.
        let _lock = file::Lock::for_write(path, file::WRITE_WAIT)?;
        // `previous`: the file as read and parsed whole, the backup kept.
        let (same, previous) = match file::load(path, |b| parse(b).map(|_| b == text.as_bytes())) {
            Loaded::Read(same, bytes) => (same, Some(bytes)),
            Loaded::Missing | Loaded::Recovered { .. } => (false, None),
            Loaded::Unreadable(why) => {
                return Err(format!(
                    "{} could not be read, so goz-gui does not write over it: {why}",
                    path.display()
                ));
            }
        };
        if same {
            return Ok(None);
        }
        let previous = previous.as_deref().filter(|_| self.backup);
        file::write(path, text.as_bytes(), previous)
            .map_err(|e| format!("The session could not be saved to {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use goz_core::types::SortKey;

    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn search(text: &str) -> Search {
        Search {
            text: text.into(),
            ..Search::default()
        }
    }

    fn sample() -> Session {
        Session {
            window: Some(WindowPos {
                x: -1900,
                y: 40,
                width: 1200,
                height: 700,
                dpi: Some(144),
            }),
            maximized: true,
            on_top: OnTop::WhileSearching,
            theme: ThemeMode::Dark,
            status_bar: false,
            view: View {
                search: Search {
                    text: r#"c:\windows\system32\*.dll "a b""#.into(),
                    match_case: true,
                    match_path: true,
                    filter: Filter::Executable,
                    sort: SortSpec {
                        key: SortKey::Size,
                        dir: SortDir::Asc,
                    },
                    folder: Some(r"C:\Windows".into()),
                },
                top: 2048,
                focus: Some(w(r"C:\Windows\System32\kernel32.dll")),
                selection: vec![
                    w(r"C:\Windows\System32\kernel32.dll"),
                    vec![0x43, 0x3a, 0x5c, 0xd800, 0x78],
                ],
                nav: vec![
                    search(""),
                    Search {
                        sort: SortSpec::default_for(SortKey::DateModified),
                        ..search("*.exe")
                    },
                    search("é😀"),
                    Search {
                        folder: Some(r"D:\".into()),
                        ..search("in a folder")
                    },
                ],
                nav_index: 1,
            },
        }
    }

    #[test]
    fn a_session_reads_back_as_written() {
        let s = sample();
        let text = s.to_text();
        assert_eq!(parse(text.as_bytes()), Ok(Some(s.clone())));
        // Everything's layout and keys.
        for line in [
            "[\r\n\t{\r\n\t\t\"is_closed\":0,\r\n\t\t\"window_x\":-1900,\r\n",
            "\t\t\"window_dpi\":144,\r\n\t\t\"maximized\":1,\r\n\t\t\"ontop\":2,\r\n\t\t\"theme\":2,\r\n",
            "\t\t\"status_bar_visible\":0,\r\n\t\t\"tab_cursel\":0,\r\n\t\t\"tabs\":\r\n\t\t[\r\n\t\t\t{\r\n",
            "\t\t\t\t\"filter\":\"Executable\",\r\n\t\t\t\t\"sort\":\"Size\",\r\n\t\t\t\t\"sort_ascending\":1,\r\n",
            "\t\t\t\t\"listview_top_index\":2048,\r\n",
            "\t\t\t\t\"listview_item_focus\":\"C:\\\\Windows\\\\System32\\\\kernel32.dll\",\r\n",
            "\"C:\\\\\\ud800x\"]",
            "\t\t\t\t\t{\"search\":\"*.exe\",\"match_case\":0,\"match_path\":0,\"filter\":\"\",\"sort\":\"Date Modified\",\"sort_ascending\":0},\r\n",
            // The Folders sidebar's folder, Everything's key: always in the
            // tab, in a navigation entry only when it has one.
            "\t\t\t\t\"folders_sidebar_selection\":[\"C:\\\\Windows\"],\r\n",
            "\t\t\t\t\t\t\"folders_sidebar_selection\":[\"D:\\\\\"]\r\n",
            "\t\t\t}\r\n\t\t]\r\n\t}\r\n]\r\n",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        // Without a window rectangle or a focus.
        let bare = Session {
            window: None,
            view: View {
                focus: None,
                ..View::default()
            },
            ..Session::default()
        };
        let text = bare.to_text();
        assert!(!text.contains("window_x"));
        assert!(text.contains("\"listview_item_focus\":\"\","));
        assert_eq!(parse(text.as_bytes()), Ok(Some(bare)));
    }

    /// Everything 1.5.0.1424b's own `Session.json`, cut down: two tabs, the
    /// second current.
    const EVERYTHING: &str = "[\r\n\t{\r\n\t\t\"is_closed\":0,\r\n\t\t\"window_x\":200,\r\n\
        \t\t\"window_y\":150,\r\n\t\t\"window_wide\":1400,\r\n\t\t\"window_high\":850,\r\n\
        \t\t\"maximized\":0,\r\n\t\t\"ontop\":0,\r\n\t\t\"order\":0,\r\n\t\t\"filters_visible\":1,\r\n\
        \t\t\"bookmarks_sidebar_expanded\":[],\r\n\t\t\"theme\":0,\r\n\t\t\"status_bar_visible\":1,\r\n\
        \t\t\"zoom\":100,\r\n\t\t\"tab_cursel\":1,\r\n\t\t\"tabs\":\r\n\t\t[\r\n\t\t\t{\r\n\
        \t\t\t\t\"is_closed\":0,\r\n\t\t\t\t\"search\":\"*.png|*.jpg\",\r\n\t\t\t\t\"match_case\":0,\r\n\
        \t\t\t\t\"match_path\":0,\r\n\t\t\t\t\"filter\":\"\",\r\n\t\t\t\t\"sort\":\"Date Modified\",\r\n\
        \t\t\t\t\"sort_ascending\":0,\r\n\t\t\t\t\"listview_item_focus\":\"C:\\\\a.png\",\r\n\
        \t\t\t\t\"listview_selection\":[\"C:\\\\a.png\"],\r\n\
        \t\t\t\t\"columns\":[{\"name\":\"Name\",\"width\":256},{\"name\":\"Path\",\"width\":256}]\r\n\
        \t\t\t},\r\n\t\t\t{\r\n\t\t\t\t\"is_closed\":0,\r\n\
        \t\t\t\t\"search\":\"c:\\\\windows\\\\system32\\\\*.exe\",\r\n\t\t\t\t\"match_case\":0,\r\n\
        \t\t\t\t\"match_path\":0,\r\n\t\t\t\t\"filter\":\"\",\r\n\t\t\t\t\"sort\":\"Name\",\r\n\
        \t\t\t\t\"sort_ascending\":1,\r\n\t\t\t\t\"secondary_sort\":\"\",\r\n\
        \t\t\t\t\"listview_item_focus\":\"C:\\\\Windows\\\\System32\\\\agentactivationruntimestarter.exe\",\r\n\
        \t\t\t\t\"listview_selection\":[\"C:\\\\Windows\\\\System32\\\\agentactivationruntimestarter.exe\"],\r\n\
        \t\t\t\t\"filelist_slot0\":[]\r\n\t\t\t}\r\n\t\t]\r\n\t}\r\n]\r\n";

    #[test]
    fn everythings_session_reads_as_its_current_tab() {
        let s = parse(EVERYTHING.as_bytes())
            .expect("valid")
            .expect("a window");
        assert_eq!(
            s.window,
            Some(WindowPos {
                x: 200,
                y: 150,
                width: 1400,
                height: 850,
                dpi: None
            })
        );
        assert!(!s.maximized && s.status_bar);
        assert_eq!((s.on_top, s.theme), (OnTop::Never, ThemeMode::UserDefault));
        let v = &s.view;
        assert_eq!(v.search.text, r"c:\windows\system32\*.exe");
        assert_eq!(v.search.sort, SortSpec::default());
        let agent = w(r"C:\Windows\System32\agentactivationruntimestarter.exe");
        assert_eq!(v.focus.as_ref(), Some(&agent));
        assert_eq!(v.selection, [agent]);
        assert_eq!((v.top, v.nav.len(), v.nav_index), (0, 0, 0));
        // The first tab when tab_cursel is out of range.
        let first = EVERYTHING.replace("\"tab_cursel\":1", "\"tab_cursel\":9");
        let s = parse(first.as_bytes()).expect("valid").expect("a window");
        assert_eq!(s.view.search.text, "*.png|*.jpg");
        assert_eq!(
            s.view.search.sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc
            }
        );
    }

    #[test]
    fn closed_windows_and_tabs_are_passed_over() {
        let doc = "[{\"is_closed\":1,\"tabs\":[{\"search\":\"closed window\"}]},\
                   {\"is_closed\":0,\"tab_cursel\":0,\"tabs\":[{\"is_closed\":1,\"search\":\"closed tab\"},\
                   {\"search\":\"open\"}]}]";
        let s = parse(doc.as_bytes()).expect("valid").expect("a window");
        assert_eq!(s.view.search.text, "open");
        // Every window closed: the first one.
        let doc = "[{\"is_closed\":1,\"tabs\":[{\"search\":\"a\"}]},{\"is_closed\":1}]";
        let s = parse(doc.as_bytes()).expect("valid").expect("a window");
        assert_eq!(s.view.search.text, "a");
        // No window: no session; no tab: the default view.
        assert_eq!(parse(b"[]"), Ok(None));
        let s = parse(b"[{\"theme\":2}]").expect("valid").expect("a window");
        assert_eq!(
            (s.theme, s.view.clone()),
            (ThemeMode::Dark, View::default())
        );
    }

    #[test]
    fn what_cannot_be_read_is_the_default_and_what_is_not_json_is_refused() {
        let s = parse(
            b"[{\"window_x\":1,\"window_y\":2,\"window_wide\":0,\"window_high\":5,\"ontop\":7,\
              \"theme\":\"dark\",\"maximized\":\"yes\",\"tabs\":[{\"sort\":\"Extension\",\
              \"sort_ascending\":0,\"filter\":\"Pictures\",\"listview_top_index\":-5,\
              \"listview_item_focus\":7,\"listview_selection\":[\"\",3,\"C:\\\\x\"],\
              \"nav_history\":[1,{\"search\":\"n\"}],\"nav_history_index\":99}]}]",
        )
        .expect("valid")
        .expect("a window");
        assert_eq!(s.window, None);
        assert_eq!(
            (s.on_top, s.theme, s.maximized),
            (OnTop::Never, ThemeMode::UserDefault, false)
        );
        let v = &s.view;
        assert_eq!(
            (v.search.sort, v.search.filter),
            (SortSpec::default(), Filter::Everything)
        );
        assert_eq!((v.top, v.focus.clone()), (0, None));
        assert_eq!(v.selection, [w(r"C:\x")]);
        assert_eq!((v.nav.len(), v.nav_index), (1, 0));
        // A sort without a direction takes the key's own.
        let s = parse(b"[{\"tabs\":[{\"sort\":\"size\"}]}]")
            .expect("valid")
            .expect("window");
        assert_eq!(s.view.search.sort, SortSpec::default_for(SortKey::Size));
        for bad in [
            &b"\0\0\0"[..],
            b"{\"tabs\":",
            b"42",
            b"\"text\"",
            b"[1]",
            b"[\"a\"]",
        ] {
            assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn long_histories_keep_their_newest_entries() {
        let max = NavHistory::<Search>::MAX;
        let nav: Vec<Search> = (0..max + 10).map(|i| search(&i.to_string())).collect();
        let s = Session {
            view: View {
                nav,
                nav_index: max + 5,
                selection: (0..MAX_SELECTION + 7)
                    .map(|i| w(&format!("C:\\{i}")))
                    .collect(),
                ..View::default()
            },
            ..Session::default()
        };
        let back = parse(s.to_text().as_bytes())
            .expect("valid")
            .expect("window");
        assert_eq!(back.view.nav.len(), max);
        assert_eq!(back.view.nav[0].text, "10");
        assert_eq!(back.view.nav_index, max - 5);
        assert_eq!(
            back.view.nav[back.view.nav_index].text,
            (max + 5).to_string()
        );
        assert_eq!(back.view.selection.len(), MAX_SELECTION);
    }

    #[test]
    fn the_file_is_written_whole_and_only_when_it_changes() {
        let dir = file::tests::scratch("session-file");
        let path = dir.join(FILE_NAME);
        let f = SessionFile::new(path.clone(), true);
        assert_eq!(f.write_now(), Ok(None), "nothing staged");
        assert!(!path.exists());
        let one = sample().to_text();
        f.stage(one.clone());
        f.write_now().expect("written");
        assert_eq!(fs::read_to_string(&path).expect("read"), one);
        assert!(!file::backup_path(&path).exists());
        // The same text again: the file is not touched.
        let before = file::stamp(&path);
        f.stage(one.clone());
        f.write_now().expect("same");
        assert_eq!(file::stamp(&path), before);
        assert!(!file::backup_path(&path).exists());
        // Another goz-gui's session there: the last written wins, and the
        // one before is the backup.
        let two = Session::default().to_text();
        f.stage(two.clone());
        f.write_now().expect("written");
        assert_eq!(fs::read_to_string(&path).expect("read"), two);
        assert_eq!(
            fs::read_to_string(file::backup_path(&path)).expect("backup"),
            one
        );
        assert_eq!(load(&path), (Some(Session::default()), None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_kept_aside_and_never_written_over_unseen() {
        let dir = file::tests::scratch("session-broken");
        let path = dir.join(FILE_NAME);
        // Read at startup: the backup instead, with a note.
        fs::write(&path, [0u8; 16]).expect("zeroed");
        fs::write(file::backup_path(&path), sample().to_text()).expect("backup");
        let (session, note) = load(&path);
        assert_eq!(session, Some(sample()));
        let note = note.expect("a note");
        assert!(
            note.contains("Session.corrupt-1.json") && note.contains("backup"),
            "{note}"
        );
        assert_eq!(
            fs::read(file::corrupt_path(&path, 1)).expect("kept"),
            [0u8; 16]
        );
        // Broken again before a write: moved aside, then written.
        fs::write(&path, b"{not json").expect("broken");
        let f = SessionFile::new(path.clone(), false);
        f.stage(Session::default().to_text());
        f.write_now().expect("written");
        assert_eq!(
            fs::read(file::corrupt_path(&path, 2)).expect("kept"),
            b"{not json"
        );
        assert_eq!(load(&path), (Some(Session::default()), None));
        // No backup: no session, and a note saying so.
        fs::write(&path, b"[").expect("broken");
        fs::remove_file(file::backup_path(&path)).expect("no backup");
        let (session, note) = load(&path);
        assert_eq!(session, None);
        assert!(note.expect("a note").contains("no previous session"));
        // A folder in its place: left alone, and the text waits.
        fs::create_dir(&path).expect("folder");
        f.stage("[]\r\n".into());
        assert!(f.write_now().is_err());
        assert!(path.is_dir());
        assert!(f.pending(), "written by the next save");
        fs::remove_dir(&path).expect("gone");
        f.write_now().expect("written later");
        assert!(!f.pending());
        assert_eq!(fs::read_to_string(&path).expect("read"), "[]\r\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
