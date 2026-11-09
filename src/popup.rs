//! Showing a context menu (the row menu, the list's background menu, the
//! header's), and the test hook that writes it out instead.
//!
//! With `GOZ_GUI_MENU_DUMP=1` the menu, built exactly as for the screen
//! (the shell's items included), is walked as a user opening every submenu
//! would make it: its owner gets WM_INITMENUPOPUP for it and each submenu
//! (which is when the shell fills Open with and Send to), and WM_MEASUREITEM
//! and WM_DRAWITEM (into a bitmap in memory, nothing on the screen) for each
//! item or icon the shell draws itself. Each item goes to `menu-dump.log`
//! in the data folder: its label, id, state, the shell's verb, nested by
//! submenu; a drawn one with its size and how many pixels the drawing set. `GOZ_GUI_MENU_PICK=<label or verb>` then returns that item's
//! id, as if it had been clicked.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DC_BRUSH, DeleteDC, DeleteObject, FillRect, GetDC,
    GetPixel, GetStockObject, ReleaseDC, SelectObject, SetDCBrushColor,
};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODA_DRAWENTIRE, ODT_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMenuDefaultItem, GetMenuItemCount, GetMenuItemInfoW, HBMMENU_CALLBACK, HMENU, MENUITEMINFOW,
    MFS_CHECKED, MFS_DISABLED, MFT_OWNERDRAW, MFT_RADIOCHECK, MFT_SEPARATOR, MIIM_BITMAP,
    MIIM_DATA, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SendMessageW,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_DRAWITEM, WM_INITMENUPOPUP,
    WM_MEASUREITEM,
};

use crate::store::location;

/// Shows `menu` at `pt` (screen) for `owner` until an item is chosen or
/// the menu closes, and returns the item's id, 0 for none. In a test's
/// dump (`GOZ_GUI_MENU_DUMP`) the menu is written out as `what` instead,
/// and the id is the item `GOZ_GUI_MENU_PICK` names. `verb` gives the
/// shell's verb of an item, by id.
pub fn track(
    menu: HMENU,
    owner: HWND,
    pt: POINT,
    what: &str,
    verb: &dyn Fn(u32) -> Option<String>,
) -> u32 {
    if !location::menu_dump() {
        // SAFETY: a menu of this thread shown for a window of it; the menu
        // loop sends the owner its messages, while the caller holds no
        // borrow.
        let id = unsafe {
            TrackPopupMenuEx(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                owner,
                core::ptr::null(),
            )
        };
        return id.max(0) as u32;
    }
    let mut entries = Vec::new();
    walk(menu, owner, 0, 0, verb, &mut entries);
    let pick = location::menu_pick();
    let chosen = pick.and_then(|p| choose(&entries, p));
    let mut text = format!("== {what} at {},{} ==\r\n", pt.x, pt.y);
    for e in &entries {
        text.push_str(&e.line());
        text.push_str("\r\n");
    }
    text.push_str(&match (pick, chosen) {
        (None, _) => "== no pick ==\r\n".to_owned(),
        (Some(p), Some(id)) => format!("== picked '{p}': id={id} ==\r\n"),
        (Some(p), None) => format!("== pick '{p}' matches no item ==\r\n"),
    });
    location::append_data_file(location::MENU_DUMP_LOG, &text);
    chosen.unwrap_or(0)
}

/// An item (or its icon) its owner draws: the size it measured, and how
/// many pixels its drawing set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Drawn {
    width: u32,
    height: u32,
    ink: u32,
}

/// One item of a menu, as the dump writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Entry {
    depth: usize,
    separator: bool,
    /// The text as the menu has it (`&` access keys, a tab before the
    /// shortcut); empty for an item the shell draws itself.
    label: String,
    /// 0 for a submenu or a separator.
    id: u32,
    submenu: bool,
    default: bool,
    checked: bool,
    radio: bool,
    greyed: bool,
    /// Drawn by its owner (the shell's handler): what it measured and drew.
    owner_drawn: Option<Drawn>,
    /// Its icon is drawn by its owner: what was measured and drawn for it.
    callback_bitmap: Option<Drawn>,
    verb: Option<String>,
}

impl Entry {
    fn line(&self) -> String {
        let indent = "  ".repeat(self.depth);
        if self.separator {
            return format!("{indent}----");
        }
        let mut line = format!("{indent}{}", self.label.replace('\t', "\\t"));
        if self.submenu {
            line.push_str("\tsubmenu");
        } else {
            line.push_str(&format!("\tid={}", self.id));
        }
        let mut state = Vec::new();
        for (on, name) in [
            (self.default, "default"),
            (self.checked, "checked"),
            (self.radio, "radio"),
            (self.greyed, "greyed"),
        ] {
            if on {
                state.push(name.to_owned());
            }
        }
        for (drawn, what) in [
            (self.owner_drawn, "owner-drawn"),
            (self.callback_bitmap, "icon"),
        ] {
            if let Some(d) = drawn {
                state.push(format!("{what} {}x{} ink {}", d.width, d.height, d.ink));
            }
        }
        if !state.is_empty() {
            line.push_str(&format!("\t({})", state.join(",")));
        }
        if let Some(verb) = &self.verb {
            line.push_str(&format!("\tverb={verb}"));
        }
        line
    }
}

/// A label as it reads: no access-key `&` (`&&` is one `&`), no shortcut
/// after a tab, any case.
fn plain(label: &str) -> String {
    let text = label.split('\t').next().unwrap_or("");
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                out.push('&');
            }
            continue;
        }
        out.push(c);
    }
    out.trim().to_lowercase()
}

/// The id of the first item (depth first, as dumped) whose label or verb
/// is `pick`; an item, not a submenu or separator.
fn choose(entries: &[Entry], pick: &str) -> Option<u32> {
    let want = plain(pick);
    let item = |e: &&Entry| !e.separator && !e.submenu && e.id != 0;
    entries
        .iter()
        .filter(item)
        .find(|e| plain(&e.label) == want)
        .or_else(|| {
            entries.iter().filter(item).find(|e| {
                e.verb
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case(pick.trim()))
            })
        })
        .map(|e| e.id)
}

/// Opens `menu` (item `pos` of its parent) as the menu loop would, then
/// notes its items, opening each submenu in turn.
fn walk(
    menu: HMENU,
    owner: HWND,
    depth: usize,
    pos: usize,
    verb: &dyn Fn(u32) -> Option<String>,
    out: &mut Vec<Entry>,
) {
    // SAFETY: a message to a window of this thread about a menu of it, as
    // the menu loop sends it; the caller holds no borrow.
    unsafe { SendMessageW(owner, WM_INITMENUPOPUP, menu as usize, pos as isize) };
    // SAFETY: reads a menu of this thread.
    let (count, default) = unsafe { (GetMenuItemCount(menu), GetMenuDefaultItem(menu, 0, 0)) };
    for i in 0..count.max(0) as u32 {
        let mut text = [0u16; 512];
        // SAFETY: the rest of MENUITEMINFOW is plain data.
        let mut mii: MENUITEMINFOW = unsafe { core::mem::zeroed() };
        mii.cbSize = size_of::<MENUITEMINFOW>() as u32;
        mii.fMask = MIIM_FTYPE
            | MIIM_STATE
            | MIIM_ID
            | MIIM_SUBMENU
            | MIIM_STRING
            | MIIM_DATA
            | MIIM_BITMAP;
        mii.dwTypeData = text.as_mut_ptr();
        mii.cch = text.len() as u32 - 1;
        // SAFETY: the struct and the text buffer it points at.
        if unsafe { GetMenuItemInfoW(menu, i, 1, &mut mii) } == 0 {
            continue;
        }
        let owner_drawn = mii.fType & MFT_OWNERDRAW != 0;
        let label = if owner_drawn {
            String::new()
        } else {
            let len = text.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&text[..len])
        };
        let submenu = !mii.hSubMenu.is_null();
        let measure = || drawn(owner, menu, mii.wID, mii.dwItemData);
        let entry = Entry {
            depth,
            separator: mii.fType & MFT_SEPARATOR != 0,
            label,
            id: if submenu { 0 } else { mii.wID },
            submenu,
            default: default != u32::MAX && default == mii.wID && !submenu,
            checked: mii.fState & MFS_CHECKED != 0,
            radio: mii.fType & MFT_RADIOCHECK != 0,
            greyed: mii.fState & MFS_DISABLED != 0,
            owner_drawn: owner_drawn.then(measure),
            callback_bitmap: core::ptr::eq(mii.hbmpItem, HBMMENU_CALLBACK).then(measure),
            verb: (!submenu && mii.wID != 0).then(|| verb(mii.wID)).flatten(),
        };
        out.push(entry);
        if submenu {
            walk(mii.hSubMenu, owner, depth + 1, i as usize, verb, out);
        }
    }
}

/// An item its owner draws, as the menu loop would ask for it: measured
/// (WM_MEASUREITEM), then drawn (WM_DRAWITEM) into a bitmap in memory
/// filled with a key colour, whose pixels that changed are counted.
fn drawn(owner: HWND, menu: HMENU, id: u32, data: usize) -> Drawn {
    let (width, height) = measure(owner, id, data);
    let ink = draw(owner, menu, id, data, width, height);
    Drawn { width, height, ink }
}

/// Pixels of a `width` × `height` bitmap that the owner's WM_DRAWITEM for
/// item `id` of `menu` set; 0 for a size that is none or absurd.
fn draw(owner: HWND, menu: HMENU, id: u32, data: usize, width: u32, height: u32) -> u32 {
    /// Magenta, which no menu draws.
    const KEY: u32 = 0x00ff_00ff;
    if width == 0 || height == 0 || width > 4000 || height > 400 {
        return 0;
    }
    let (w, h) = (width as i32, height as i32);
    // SAFETY: a memory DC and bitmap made, used and deleted here; the
    // message goes to a window of this thread with a struct that outlives
    // it.
    unsafe {
        let screen = GetDC(core::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        ReleaseDC(core::ptr::null_mut(), screen);
        if dc.is_null() || bitmap.is_null() {
            DeleteObject(bitmap);
            DeleteDC(dc);
            return 0;
        }
        let old = SelectObject(dc, bitmap);
        let rc = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        SetDCBrushColor(dc, KEY);
        FillRect(dc, &rc, GetStockObject(DC_BRUSH));
        let dis = DRAWITEMSTRUCT {
            CtlType: ODT_MENU,
            CtlID: 0,
            itemID: id,
            itemAction: ODA_DRAWENTIRE,
            itemState: 0,
            hwndItem: menu as HWND,
            hDC: dc,
            rcItem: rc,
            itemData: data,
        };
        SendMessageW(owner, WM_DRAWITEM, 0, (&raw const dis) as isize);
        let mut ink = 0;
        for y in 0..h {
            for x in 0..w {
                if GetPixel(dc, x, y) != KEY {
                    ink += 1;
                }
            }
        }
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        ink
    }
}

/// What the owner measures an item it draws at (WM_MEASUREITEM, as the
/// menu loop asks before showing it).
fn measure(owner: HWND, id: u32, data: usize) -> (u32, u32) {
    let mut mis = MEASUREITEMSTRUCT {
        CtlType: ODT_MENU,
        CtlID: 0,
        itemID: id,
        itemWidth: 0,
        itemHeight: 0,
        itemData: data,
    };
    // SAFETY: a message to a window of this thread with a struct that
    // outlives it.
    unsafe { SendMessageW(owner, WM_MEASUREITEM, 0, (&raw mut mis) as isize) };
    (mis.itemWidth, mis.itemHeight)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, id: u32, verb: Option<&str>) -> Entry {
        Entry {
            label: label.into(),
            id,
            verb: verb.map(Into::into),
            ..Entry::default()
        }
    }

    #[test]
    fn labels_read_without_access_keys_or_shortcuts() {
        assert_eq!(plain("&Open"), "open");
        assert_eq!(plain("Copy &Full Path"), "copy full path");
        assert_eq!(plain("&Refresh\tF5"), "refresh");
        assert_eq!(plain("R&&D"), "r&d");
        assert_eq!(plain(" Set Run &Count... "), "set run count...");
    }

    #[test]
    fn a_pick_names_a_label_or_a_verb() {
        let mut sub = item("Se&nd to", 0, None);
        sub.submenu = true;
        let entries = [
            item("&Open", 1000, None),
            sub,
            item("Desktop (create shortcut)", 0x7010, Some("desktop")),
            Entry {
                separator: true,
                ..Entry::default()
            },
            item("&Copy", 0x7020, Some("copy")),
            item("", 0x7030, Some("openas")),
        ];
        assert_eq!(choose(&entries, "Open"), Some(1000));
        assert_eq!(choose(&entries, "copy"), Some(0x7020), "label first");
        assert_eq!(choose(&entries, "OPENAS"), Some(0x7030), "then the verb");
        assert_eq!(choose(&entries, "send to"), None, "a submenu is no item");
        assert_eq!(choose(&entries, "nothing"), None);
    }

    #[test]
    fn dump_lines() {
        let mut e = item("&Open\tEnter", 1000, None);
        e.default = true;
        assert_eq!(e.line(), "&Open\\tEnter\tid=1000\t(default)");
        let mut s = item("Send to", 0, None);
        s.submenu = true;
        s.depth = 1;
        assert_eq!(s.line(), "  Send to\tsubmenu");
        let mut o = item("", 0x7001, Some("openas"));
        o.owner_drawn = Some(Drawn {
            width: 120,
            height: 22,
            ink: 900,
        });
        o.callback_bitmap = Some(Drawn {
            width: 16,
            height: 16,
            ink: 0,
        });
        o.depth = 2;
        assert_eq!(
            o.line(),
            "    \tid=28673\t(owner-drawn 120x22 ink 900,icon 16x16 ink 0)\tverb=openas"
        );
        let sep = Entry {
            separator: true,
            depth: 1,
            ..Entry::default()
        };
        assert_eq!(sep.line(), "  ----");
    }
}
