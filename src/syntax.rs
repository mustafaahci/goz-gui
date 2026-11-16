//! Help > Search Syntax: what the search box accepts, in a resizable window
//! of its own whose text can be selected and copied.
//!
//! The list of search terms is goz's own help text (goz-core renders it from
//! the same tables its tests run through the parser), so it describes
//! exactly what the parser goz-gui is built with accepts: Everything 1.5's
//! syntax, which gozd 0.7 reads, and which goz-gui reads itself for an open
//! file list. goz-gui adds what its menus do to a search; a gozd that does
//! not read the new syntax yet (`client::boolean_syntax`) gets lines
//! saying what its searches of the index refuse.

use std::cell::{Cell, RefCell};

use goz_core::escompat::{HelpScreen, help};
use windows_sys::Win32::Foundation::{HWND, LRESULT};
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DC_BRUSH, DEFAULT_CHARSET, FF_MODERN, FIXED_PITCH,
    FW_NORMAL, HDC, LOGFONTW, OUT_DEFAULT_PRECIS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::paint::fill;
use crate::places::{self, Which};
use crate::theme::{self, Palette};
use crate::win::class::{self, Create, create_plain, id_menu};
use crate::win::gdi::{self, Paint};
use crate::win::{self, Msg, edit, subclass, sys};
use crate::window::app_icon;

const CLASS: &str = "goz-gui-syntax";
const ID_TEXT: usize = 1;

/// The text box's subclass (Esc, Ctrl+A).
static TEXT_KEYS: subclass::Key = subclass::Key::new("search syntax text box keys");

thread_local! {
    /// The window while it is open; one at a time.
    static OPEN: Cell<HWND> = const { Cell::new(core::ptr::null_mut()) };
    static FONT: RefCell<Option<gdi::Owned>> = const { RefCell::new(None) };
    static DARK: Cell<bool> = const { Cell::new(false) };
}

/// goz-gui's own rows: what to type or choose, and what it does, a line of
/// text each.
///
/// W9: goz's own table above is the new query language's (goz-gui is built
/// with goz-core's parser, which reads it all); a gozd that does not read
/// it yet (v0.6.0) gets these lines too, as its searches of the index
/// refuse what it lacks.
const OLDER_GOZD: &[(&str, &[&str])] = &[
    (
        "|  !  < >",
        &[
            "This gozd refuses OR, NOT, groups, modifiers",
            "and the newer functions in a search of its",
            "index; quote | ! < > to search for them as",
            "text, as in \"a|b\". An open file list reads",
            "them all; a newer gozd (goz 0.7) does too",
        ],
    ),
    (
        "content:",
        &[
            "With this gozd, only content terms joined",
            "to the search with spaces (AND)",
        ],
    ),
];
const IN_GOZ_GUI: &[(&str, &[&str])] = &[
    (
        "File > Open File List...",
        &[
            "Searches the list instead of the index, with",
            "the syntax above but child:, descendant:,",
            "sibling: and their kin, the counts of related",
            "entries and total-child-size: (a list has no",
            "folder tree); dc: and attrib: read the list's",
            "Date Created and Attributes",
        ],
    ),
    (
        "content:abc",
        &[
            "Read by goz-gui under your account, not by",
            "gozd; up to 3 content terms in any place of",
            "the search (in an OR, after !), Esc stops it",
        ],
    ),
    ("Search > Match Case", &["Matches case, as case: does"]),
    (
        "Search > Match Path",
        &["Matches full paths, as matchpath: does"],
    ),
    (
        "Search > Audio, ...",
        &[
            "Only files with the filter's extensions, as",
            "ext: does; Folder keeps only folders. The",
            "filter and the search text must both match",
        ],
    ),
    (
        "Ctrl+1, 2, 3, 6",
        &[
            "Sort by name, path, size or date modified;",
            "the same key again reverses the order",
        ],
    ),
    (
        "filelist:a.txt;b.txt",
        &[
            "Items of those whole names, or full paths",
            "with a \\ (* and ? too). In an open file",
            "list; from gozd's index with a gozd that",
            "reads OR",
        ],
    ),
    (
        "filelist0:",
        &[
            "The names in file list slot 0 (to 9): File |",
            "Set File List Slot N (a key in Options >",
            "Keyboard) puts the selected results there",
        ],
    ),
    (
        "Ctrl+click filelist0:",
        &[
            "Edits the slot; also F2 with the caret in",
            "it, or the right-click menu's Edit File",
            "List Slot...",
        ],
    ),
    (
        "parent-filelist0:",
        &[
            "Items in a folder of slot 0 (by full path",
            "or name); ancestor-filelist0: anywhere",
            "under one",
        ],
    ),
    (
        "child-filelist0:",
        &[
            "Folders holding an item of slot 0;",
            "descendant-filelist0: folders above one;",
            "sibling-filelist0: items beside one",
        ],
    ),
    (
        "size-filelist0:",
        &[
            "Files of a slot item's size; name- and",
            "stem-filelist0: of a slot item's name or",
            "stem",
        ],
    ),
    (
        "exists-in-filelist0:$stem:.srt",
        &[
            "Items whose name, made so, is in slot 0",
            "($name:, $stem:, $extension:, $path:,",
            "$full-path:, $parent-name:); in an open",
            "file list",
        ],
    ),
    ("save-filelist0:", &["Puts the search's results in slot 0"]),
    (
        "a save-filelist1: new-search: b",
        &[
            "Runs a, puts its results in slot 1, then",
            "runs b as a new search (b may read",
            "filelist1:); not updated live (F5 is)",
        ],
    ),
    (
        "include-file-list:\"C:\\l.efu\"",
        &[
            "Items a list on the disk names (an EFU or",
            "CSV's Filename, or a name a line);",
            "Ctrl+click it to pick the file",
        ],
    ),
    (
        "open-filelist:\"C:\\l.efu\"",
        &[
            "Searches that file list instead, for this",
            "search; Ctrl+click it to pick the file",
        ],
    ),
    (
        "filelist-filename:l.efu",
        &[
            "Rows of an open file list of that name",
            "(nothing of gozd's index is in one)",
        ],
    ),
];

/// The window's text, lines ending in `\n`, as gozd's syntax is now.
pub fn text() -> String {
    text_for(crate::client::boolean_syntax())
}

/// The window's text; `boolean`: gozd reads OR, NOT and groups.
pub fn text_for(boolean: bool) -> String {
    let full = help(HelpScreen::Full);
    let terms = section(&full, "Search text").unwrap_or(&full);
    let column = text_column(terms);
    let mut out = String::from(
        "Search syntax\n\
         \n\
         goz-gui searches as you type. Separate words with spaces to find\n\
         names that contain all of them, in any order.\n\
         \n",
    );
    out.push_str(terms);
    let mut tables = Vec::new();
    if !boolean {
        tables.push(("This gozd", OLDER_GOZD));
    }
    tables.push(("In goz-gui", IN_GOZ_GUI));
    for (title, rows) in tables {
        out.push('\n');
        out.push_str(title);
        out.push('\n');
        for (usage, lines) in rows {
            // A usage wider than its column has a line of its own.
            let own_line = usage.chars().count() + 1 > column.saturating_sub(2);
            if own_line {
                out.push_str(&format!("  {usage}\n"));
            }
            for (i, line) in lines.iter().enumerate() {
                let usage = if i == 0 && !own_line { *usage } else { "" };
                out.push_str(&format!("  {usage:<width$}{line}\n", width = column - 2));
            }
        }
    }
    out
}

/// The lines of `help` from the line `title` to the next blank line.
fn section<'a>(help: &'a str, title: &str) -> Option<&'a str> {
    let start = help.find(&format!("\n{title}\n"))? + 1;
    let rest = &help[start..];
    let end = rest.find("\n\n").map_or(rest.len(), |i| i + 1);
    Some(&rest[..end])
}

/// Where the text of goz's table starts, so goz-gui's rows line up with it.
fn text_column(table: &str) -> usize {
    let column = table.lines().nth(1).and_then(|row| {
        let usage = row.len() - row.trim_start().len();
        let gap = usage + row[usage..].find("  ")?;
        Some(gap + row[gap..].len() - row[gap..].trim_start().len())
    });
    column.unwrap_or(26).max(24)
}

/// Shows the window, or brings it forward when it is open.
pub fn show(owner: HWND, dark: bool) {
    let open = OPEN.get();
    if !open.is_null() {
        win::show(open, SW_RESTORE);
        win::set_foreground(open);
        return;
    }
    DARK.set(dark);
    // The class stays registered once the window closes, and its icons
    // with it; loading new ones for a registration that would fail leaks
    // them, two icons per opening.
    if !win::class::is_registered(CLASS) {
        let icons = [
            app_icon(sys::metric(SM_CXICON)),
            app_icon(sys::metric(SM_CXSMICON)),
        ];
        let handle = |i: &Option<win::icon::Icon>| {
            i.as_ref()
                .map_or(core::ptr::null_mut(), win::icon::Icon::handle)
        };
        let c = class::Class {
            cursor: sys::system_cursor(IDC_ARROW),
            icon: handle(&icons[0]),
            icon_small: handle(&icons[1]),
            ..class::Class::new(CLASS)
        };
        if !c.register() {
            // Given to no class: destroyed as they drop.
            return;
        }
        // Kept by the class for the process.
        for icon in icons.into_iter().flatten() {
            icon.leak();
        }
    }
    // Centred on the main window, at its scale; then where it was last
    // (`places`).
    let dpi = crate::custom::window_dpi(owner) as i32;
    let (w, h) = (720 * dpi / 96, 600 * dpi / 96);
    let rc = win::window_rect(owner);
    let x = rc.left + (rc.right - rc.left - w) / 2;
    let y = rc.top + (rc.bottom - rc.top - h) / 2;
    let hwnd = class::create(
        &Create {
            ex_style: 0,
            class: CLASS,
            title: "Search Syntax - goz",
            style: WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            rect: (x, y, w, h),
            parent: owner,
            menu: core::ptr::null_mut(),
        },
        wndproc,
    );
    if hwnd.is_null() {
        return;
    }
    let style = WS_CHILD
        | WS_VISIBLE
        | WS_VSCROLL
        | WS_HSCROLL
        | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_NOHIDESEL) as u32;
    let text_box = create_plain(&Create {
        ex_style: 0,
        class: "EDIT",
        title: "",
        style,
        rect: (0, 0, 0, 0),
        parent: hwnd,
        menu: id_menu(ID_TEXT),
    });
    subclass::set(text_box, &TEXT_KEYS, text_proc);
    OPEN.set(hwnd);
    apply_fonts(hwnd);
    win::set_text(text_box, &text().replace('\n', "\r\n"));
    edit::set_sel(text_box, 0, 0);
    apply_theme(dark);
    places::restore(
        hwnd,
        Which::SearchSyntax,
        core::ptr::null_mut(),
        (w / 3, h / 4),
    );
    layout(hwnd);
    // Shown already when it opens maximized.
    if !win::is_visible(hwnd) {
        win::show(hwnd, SW_SHOWNORMAL);
    }
    win::set_focus(text_box);
}

/// Shows the text as gozd's syntax is now, if the window is open.
pub fn refresh() {
    let hwnd = OPEN.get();
    if hwnd.is_null() {
        return;
    }
    win::set_text(text_box(hwnd), &text().replace('\n', "\r\n"));
}

/// Keeps where the window is, if it is open (the settings are about to be
/// written).
pub fn keep_place() {
    let hwnd = OPEN.get();
    if !hwnd.is_null() {
        places::keep(hwnd, Which::SearchSyntax, core::ptr::null_mut());
    }
}

/// Follows the main window's theme while the window is open.
pub fn apply_theme(dark: bool) {
    let hwnd = OPEN.get();
    if hwnd.is_null() {
        return;
    }
    DARK.set(dark);
    let edit = text_box(hwnd);
    theme::set_titlebar(hwnd, dark);
    theme::allow(edit, dark);
    theme::set_class(
        edit,
        Some(if dark {
            "DarkMode_Explorer"
        } else {
            "Explorer"
        }),
    );
    win::invalidate(hwnd, None, true);
    win::invalidate(edit, None, true);
}

fn text_box(hwnd: HWND) -> HWND {
    win::dlg::item(hwnd, ID_TEXT as u16)
}

fn scale(hwnd: HWND, v: i32) -> i32 {
    v * crate::custom::window_dpi(hwnd) as i32 / 96
}

/// A monospaced font at the window's DPI, so goz's two columns line up.
fn apply_fonts(hwnd: HWND) {
    let text = text_box(hwnd);
    let mut lf = LOGFONTW {
        lfHeight: -scale(hwnd, 13),
        lfWeight: FW_NORMAL as i32,
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_DEFAULT_PRECIS,
        lfClipPrecision: CLIP_DEFAULT_PRECIS,
        lfQuality: CLEARTYPE_QUALITY,
        lfPitchAndFamily: FIXED_PITCH | FF_MODERN,
        ..Default::default()
    };
    for (to, from) in lf.lfFaceName.iter_mut().zip("Consolas".encode_utf16()) {
        *to = from;
    }
    // A font owned here; the old one is deleted (as it drops) once the
    // text box has the new one.
    let font = gdi::create_font(&lf);
    win::set_font(
        text,
        font.as_ref().map_or(core::ptr::null_mut(), gdi::Owned::get),
        true,
    );
    let margin = scale(hwnd, 6) as u16;
    edit::set_margins(text, EC_LEFTMARGIN | EC_RIGHTMARGIN, margin, margin);
    let old = FONT.replace(font);
    drop(old);
}

fn layout(hwnd: HWND) {
    let rc = win::client_rect(hwnd);
    let inset = scale(hwnd, 6);
    win::move_window(
        text_box(hwnd),
        inset,
        inset,
        (rc.right - 2 * inset).max(0),
        (rc.bottom - 2 * inset).max(0),
        true,
    );
}

fn wndproc(m: &Msg) -> LRESULT {
    let hwnd = m.hwnd;
    match m.id {
        WM_SIZE => {
            layout(hwnd);
            0
        }
        WM_PAINT => {
            let p = Paint::begin(hwnd);
            fill(p.hdc(), &p.area(), Palette::new(DARK.get()).bg);
            0
        }
        WM_ERASEBKGND => 1,
        // A read-only text box asks for the static colours.
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let pal = Palette::new(DARK.get());
            let hdc = m.w as HDC;
            gdi::set_text_color(hdc, pal.fg);
            gdi::set_bk_color(hdc, pal.bg);
            gdi::set_dc_brush_color(hdc, pal.bg);
            gdi::stock_object(DC_BRUSH) as LRESULT
        }
        WM_SETFOCUS => {
            win::set_focus(text_box(hwnd));
            0
        }
        WM_DPICHANGED => {
            if let Some(rc) = m.dpi_rect() {
                win::set_pos(
                    hwnd,
                    core::ptr::null_mut(),
                    (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            apply_fonts(hwnd);
            layout(hwnd);
            0
        }
        WM_DESTROY => {
            places::keep(hwnd, Which::SearchSyntax, core::ptr::null_mut());
            OPEN.set(core::ptr::null_mut());
            // Deleted as it drops, the text box gone with the window.
            let font = FONT.take();
            drop(font);
            0
        }
        _ => m.def(),
    }
}

/// The text box's subclass: Esc closes the window, and Ctrl+A selects all
/// the text, which a multi-line text box does not do by itself.
fn text_proc(m: &Msg) -> LRESULT {
    match m.id {
        WM_KEYDOWN if m.w == usize::from(VK_ESCAPE) => {
            win::post_close(win::parent(m.hwnd));
            return 0;
        }
        WM_KEYDOWN if m.w == usize::from(b'A') && sys::key_down(VK_CONTROL) => {
            edit::select_all(m.hwnd);
            return 0;
        }
        // Esc and Ctrl+A as characters, handled above.
        WM_CHAR if m.w == 0x1b || m.w == 0x01 => return 0,
        _ => {}
    }
    m.def()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terms_are_gozs() {
        let full = help(HelpScreen::Full);
        let terms = section(&full, "Search text").expect("goz's help has the section");
        assert!(terms.starts_with("Search text\n"));
        assert!(terms.ends_with('\n') && !terms.ends_with("\n\n"));
        for term in [
            "ext:cs;txt",
            "size:>1mb",
            "dm:today",
            "folder:",
            "matchpath:",
        ] {
            assert!(terms.contains(term), "{term}");
        }
        for boolean in [false, true] {
            let text = text_for(boolean);
            assert!(text.contains(terms));
            // goz's help wraps at 79 columns; goz-gui's own lines do too.
            assert!(
                text.lines().all(|l| l.chars().count() <= 79),
                "{boolean}: {text}"
            );
        }
    }

    #[test]
    fn the_new_syntax_is_listed_and_an_older_gozd_says_what_it_lacks() {
        // goz's own table is the new query language's, whatever the gozd.
        for boolean in [false, true] {
            let text = text_for(boolean);
            for usage in ["report|pdf", "!draft", "<a b>|c", "ww:TEXT", "len:>40"] {
                assert!(text.contains(usage), "{boolean}: {usage}");
            }
            assert!(text.contains("File > Open File List..."));
        }
        let old = text_for(false);
        assert!(old.contains("This gozd refuses OR, NOT, groups"));
        let new = text_for(true);
        assert!(!new.contains("This gozd"));
    }

    #[test]
    fn the_quoted_example_is_searchable() {
        // goz-core's parser, goz-gui's for open file lists, reads both; in
        // quotes the | is text.
        assert!(goz_core::query::parse_query("\"a|b\"").is_ok());
        assert!(goz_core::query::parse_query("a|b").is_ok());
    }
}
