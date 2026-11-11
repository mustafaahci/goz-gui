//! Options > Indexes, laid out as Everything 1.5's page (its labels, ids,
//! places and access keys), over gozd's index settings (`crate::indexes`).
//!
//! The switches are gozd's, read and written through an `IndexSettings`
//! on a thread of the page's own: the page shows "Asking gozd..." until the
//! answer comes, then gozd's settings, or (a gozd that cannot report them)
//! every switch greyed with the reason. Apply sends only what changed, then
//! reads the settings back, so the page never shows a change gozd did not
//! make; the status bar says how it went.
//!
//! goz always indexes file sizes and dates modified, so those two stay on
//! and greyed. The database location, the multi-user database file name,
//! the journal and Force Rebuild are Everything's own, with nothing in
//! gozd to set: they are shown greyed, as Everything lays the page out.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::SystemServices::SS_LEFT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_AUTOCHECKBOX, ES_AUTOHSCROLL, ES_NUMBER, PostMessageW, WM_APP, WS_EX_CLIENTEDGE, WS_TABSTOP,
};

use crate::dialog::Class;
use crate::indexes::{IndexConfig, IndexSettings, Key};
use crate::options::{Page, RESTORE, Rows, checked, enable, order, set_check, set_text};
use crate::window::App;

// Everything's control ids.
const DB_LABEL: u16 = 1402;
const DB_LOCATION: u16 = 1187;
const DB_BROWSE: u16 = 1188;
const MULTI_USER: u16 = 1189;
const RECENT: u16 = 1193;
const JOURNAL: u16 = 1539;
const JOURNAL_LABEL: u16 = 1540;
const JOURNAL_SIZE: u16 = 1541;
const JOURNAL_UNIT: u16 = 1542;
const SIZE: u16 = 1194;
const FAST_SIZE: u16 = 1195;
const FOLDER_SIZE: u16 = 1204;
const CREATED: u16 = 1196;
const FAST_CREATED: u16 = 1197;
const MODIFIED: u16 = 1198;
const FAST_MODIFIED: u16 = 1199;
const ACCESSED: u16 = 1200;
const FAST_ACCESSED: u16 = 1201;
const ATTRIBUTES: u16 = 1202;
const FAST_ATTRIBUTES: u16 = 1203;
const FAST_EXTENSION: u16 = 1206;
const FORCE_REBUILD: u16 = 1191;
/// goz-gui's own: what the page says of gozd's settings.
const NOTE: u16 = 1600;

/// The page's own message: the settings' source answered.
const WM_LOADED: u32 = WM_APP + 423;

/// Each setting of gozd's and its check box.
const SWITCHES: [(Key, u16); 11] = [
    (Key::IndexRecentChanges, RECENT),
    (Key::FastSizeSort, FAST_SIZE),
    (Key::IndexFolderSize, FOLDER_SIZE),
    (Key::IndexDateCreated, CREATED),
    (Key::FastDateCreatedSort, FAST_CREATED),
    (Key::FastDateModifiedSort, FAST_MODIFIED),
    (Key::IndexDateAccessed, ACCESSED),
    (Key::FastDateAccessedSort, FAST_ACCESSED),
    (Key::IndexAttributes, ATTRIBUTES),
    (Key::FastAttributesSort, FAST_ATTRIBUTES),
    (Key::FastExtensionSort, FAST_EXTENSION),
];

/// Everything's own, with nothing in gozd to set: always greyed (their
/// labels stay as Everything has them, not greyed).
const EVERYTHINGS_OWN: [u16; 7] = [
    DB_LOCATION,
    DB_BROWSE,
    MULTI_USER,
    JOURNAL,
    JOURNAL_SIZE,
    SIZE,
    MODIFIED,
];

const ALWAYS: &str = "goz always indexes file sizes and dates modified. The database \
                      location, the journal and Force Rebuild are Everything's own: gozd keeps \
                      its index in its service's folder, keeps no journal and rebuilds by itself.";

/// What the page knows of gozd's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Known {
    Asking,
    Settings(IndexConfig),
    Unavailable(String),
}

pub(super) struct Indexes {
    source: Arc<dyn IndexSettings>,
    /// The source's last answer, from its thread.
    answer: Arc<Mutex<Option<Result<IndexConfig, String>>>>,
    /// The settings as gozd has them (what Apply changes from).
    loaded: Cell<Option<IndexConfig>>,
}

impl Indexes {
    pub(super) fn new() -> Indexes {
        Indexes {
            source: crate::indexes::source(),
            answer: Arc::new(Mutex::new(None)),
            loaded: Cell::new(None),
        }
    }

    /// Asks the source on a thread of its own; `page` hears when it said.
    fn ask(&self, page: HWND) {
        let source = Arc::clone(&self.source);
        let answer = Arc::clone(&self.answer);
        let page = page as usize;
        let spawned = std::thread::Builder::new()
            .name("goz-index-settings".into())
            .spawn(move || {
                let got = source.get();
                *answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(got);
                // SAFETY: a plain post (no payload); a page gone since
                // drops it.
                unsafe { PostMessageW(page as HWND, WM_LOADED, 0, 0) };
            });
        if spawned.is_err() {
            show(
                page as HWND,
                &Known::Unavailable("The settings could not be read.".into()),
            );
        }
    }
}

/// The note under the switches.
fn note_of(known: &Known) -> String {
    match known {
        Known::Asking => format!("Asking gozd for its index settings…\n\n{ALWAYS}"),
        Known::Settings(_) => format!(
            "These are gozd's settings, for every user of this computer; changing them \
             takes goz-gui run as administrator.\n\n{ALWAYS}"
        ),
        Known::Unavailable(why) => {
            format!("{why} Until then they cannot be shown or changed here.\n\n{ALWAYS}")
        }
    }
}

/// Shows `known` in the page's controls.
fn show(page: HWND, known: &Known) {
    // goz's own: on, greyed.
    set_check(page, SIZE, true);
    set_check(page, MODIFIED, true);
    for id in EVERYTHINGS_OWN {
        enable(page, id, false);
    }
    let settings = match known {
        Known::Settings(c) => Some(*c),
        _ => None,
    };
    for (key, id) in SWITCHES {
        set_check(page, id, settings.is_some_and(|c| c.get(key)));
    }
    enable(page, FORCE_REBUILD, false);
    set_text(page, NOTE, &note_of(known));
}

/// The switches as the page shows them, over `base`.
fn shown(page: HWND, base: IndexConfig) -> IndexConfig {
    let mut c = base;
    for (key, id) in SWITCHES {
        c.set(key, checked(page, id));
    }
    c
}

impl Page for Indexes {
    fn name(&self) -> &'static str {
        "Indexes"
    }

    fn order(&self) -> u32 {
        order::INDEXES
    }

    /// Everything's page at 100 %: its pixels (left 775, top 377 in a page
    /// at 763, 365) in dialog units of 1.5 × 1.625 pixels, from the rows'
    /// origin (8, 7).
    fn build(&self, rows: &mut Rows<'_>) {
        let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32;
        let left = 82;
        let right_x = 90;
        let right_w = 167;
        rows.control(
            Class::Static,
            "Database &location:",
            DB_LABEL,
            SS_LEFT,
            0,
            (0, 0, 257, 9),
        )
        .control(
            Class::Edit,
            "",
            DB_LOCATION,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            (0, 12, 203, 13),
        )
        .control(
            Class::Button,
            "&Browse...",
            DB_BROWSE,
            WS_TABSTOP,
            0,
            (207, 11, 50, 14),
        )
        .control(
            Class::Button,
            "&Multi-user database filename",
            MULTI_USER,
            check,
            0,
            (0, 29, 257, 10),
        )
        .control(
            Class::Button,
            "Index recent chan&ges",
            RECENT,
            check,
            0,
            (0, 42, 257, 10),
        )
        .control(
            Class::Button,
            "Enable journal",
            JOURNAL,
            check,
            0,
            (0, 57, left, 10),
        )
        .control(
            Class::Static,
            "Maximum size:",
            JOURNAL_LABEL,
            SS_LEFT,
            0,
            (right_x, 58, 50, 9),
        )
        .control(
            Class::Edit,
            "1024",
            JOURNAL_SIZE,
            WS_TABSTOP | (ES_AUTOHSCROLL | ES_NUMBER) as u32,
            WS_EX_CLIENTEDGE,
            (140, 55, 50, 13),
        )
        .control(
            Class::Static,
            "KB",
            JOURNAL_UNIT,
            SS_LEFT,
            0,
            (194, 58, 12, 9),
        )
        .control(
            Class::Button,
            "Index file &size",
            SIZE,
            check,
            0,
            (0, 72, left, 10),
        )
        .control(
            Class::Button,
            "Fast s&ize sort",
            FAST_SIZE,
            check,
            0,
            (right_x, 72, right_w, 10),
        )
        .control(
            Class::Button,
            "Index folder si&ze",
            FOLDER_SIZE,
            check,
            0,
            (0, 85, 257, 10),
        )
        .control(
            Class::Button,
            "Index &date created",
            CREATED,
            check,
            0,
            (0, 98, left, 10),
        )
        .control(
            Class::Button,
            "Fast date created sort",
            FAST_CREATED,
            check,
            0,
            (right_x, 98, right_w, 10),
        )
        .control(
            Class::Button,
            "Index da&te modified",
            MODIFIED,
            check,
            0,
            (0, 111, left, 10),
        )
        .control(
            Class::Button,
            "Fast date modified s&ort",
            FAST_MODIFIED,
            check,
            0,
            (right_x, 111, right_w, 10),
        )
        .control(
            Class::Button,
            "Index dat&e accessed",
            ACCESSED,
            check,
            0,
            (0, 124, left, 10),
        )
        .control(
            Class::Button,
            "Fast date accessed sort",
            FAST_ACCESSED,
            check,
            0,
            (right_x, 124, right_w, 10),
        )
        .control(
            Class::Button,
            "I&ndex attributes",
            ATTRIBUTES,
            check,
            0,
            (0, 137, left, 10),
        )
        .control(
            Class::Button,
            "Fast attrib&utes sort",
            FAST_ATTRIBUTES,
            check,
            0,
            (right_x, 137, right_w, 10),
        )
        .control(
            Class::Button,
            "Fast e&xtension sort",
            FAST_EXTENSION,
            check,
            0,
            (right_x, 150, right_w, 10),
        )
        .control(
            Class::Button,
            "&Force Rebuild",
            FORCE_REBUILD,
            WS_TABSTOP,
            0,
            (0, 162, 59, 14),
        )
        .control(Class::Static, "", NOTE, SS_LEFT, 0, (0, 182, 257, 50));
    }

    fn fill(&self, _app: &App, page: HWND) {
        // The last answer while the dialog is open; asked again each time
        // the page is made (gozd may have changed meanwhile).
        self.loaded.set(None);
        show(page, &Known::Asking);
        self.ask(page);
    }

    /// Greyed: everything until gozd's settings are known; a fast sort
    /// while its index is off, as Everything.
    fn update_enabled(&self, page: HWND) {
        let known = self.loaded.get().is_some();
        for (key, id) in SWITCHES {
            let needs = key.needs().is_none_or(|n| {
                let (_, nid) = SWITCHES[SWITCHES.iter().position(|&(k, _)| k == n).unwrap_or(0)];
                checked(page, nid)
            });
            enable(page, id, known && needs);
        }
        enable(page, RESTORE, known);
    }

    fn apply(&self, app: &App, page: HWND, problems: &mut Vec<String>) {
        let Some(old) = self.loaded.get() else {
            // Nothing could change: every switch was greyed.
            return;
        };
        let new = shown(page, old);
        let change = old.change_to(&new);
        if change.is_empty() {
            return;
        }
        let source = Arc::clone(&self.source);
        let answer = Arc::clone(&self.answer);
        let (hwnd, page_h) = (app.hwnd as usize, page as usize);
        let spawned = std::thread::Builder::new()
            .name("goz-index-settings".into())
            .spawn(move || {
                let said = match source.set(&change) {
                    Ok(done) => done,
                    Err(why) => format!("gozd's index settings were not changed: {why}"),
                };
                // Read back, so the page shows what gozd has.
                let got = source.get();
                *answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(got);
                // SAFETY: plain posts; the window owns the box once the
                // message is queued, and it is taken back if not.
                unsafe {
                    PostMessageW(page_h as HWND, WM_LOADED, 0, 0);
                    let text = Box::into_raw(Box::new(said));
                    if PostMessageW(hwnd as HWND, crate::window::WM_INDEX_SET, 0, text as isize)
                        == 0
                    {
                        drop(Box::from_raw(text));
                    }
                }
            });
        if spawned.is_err() {
            problems.push("gozd's index settings could not be changed".into());
        }
    }

    /// Everything's defaults, once gozd's settings are known.
    fn defaults(&self, page: HWND) {
        if self.loaded.get().is_none() {
            return;
        }
        let d = IndexConfig::default();
        for (key, id) in SWITCHES {
            set_check(page, id, d.get(key));
        }
    }

    fn message(&self, _app: &App, page: HWND, msg: u32, _w: WPARAM, _l: LPARAM) -> Option<isize> {
        if msg != WM_LOADED {
            return None;
        }
        let got = self
            .answer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        let known = match got {
            Ok(c) => {
                self.loaded.set(Some(c));
                Known::Settings(c)
            }
            Err(why) => {
                self.loaded.set(None);
                Known::Unavailable(why)
            }
        };
        show(page, &known);
        self.update_enabled(page);
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_says_what_the_page_knows() {
        assert!(note_of(&Known::Asking).starts_with("Asking gozd"));
        let why = note_of(&Known::Unavailable("gozd 0.6.0 cannot.".into()));
        assert!(why.starts_with("gozd 0.6.0 cannot. Until then they cannot be shown"));
        assert!(note_of(&Known::Settings(IndexConfig::default())).contains("administrator"));
    }

    #[test]
    fn every_setting_has_its_switch_once() {
        for k in Key::ALL {
            assert_eq!(
                SWITCHES.iter().filter(|&&(key, _)| key == k).count(),
                1,
                "{k:?}"
            );
        }
        let mut ids: Vec<u16> = SWITCHES
            .iter()
            .map(|&(_, id)| id)
            .chain(EVERYTHINGS_OWN)
            .collect();
        ids.extend([
            FORCE_REBUILD,
            NOTE,
            RESTORE,
            DB_LABEL,
            JOURNAL_LABEL,
            JOURNAL_UNIT,
        ]);
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "ids of their own");
    }
}
