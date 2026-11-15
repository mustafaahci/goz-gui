//! Wave 9: the header menu's Organize Columns..., Everything 1.5's dialog:
//! the shown columns in their order, with Add... (the columns not shown,
//! as a menu at the button: goz-gui has these ten, not Everything's
//! hundreds of properties), Move Up, Move Down, Remove (never Name) and
//! Reset (Everything's default columns). OK puts them on the list; widths
//! stay as they were. Everything's Edit... (a column's property and
//! alignment) has nothing to edit in goz-gui's fixed columns and is left
//! out.

use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, IDCANCEL, IDOK, LBN_DBLCLK, LBN_SELCHANGE, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY,
    WM_COMMAND, WM_INITDIALOG, WS_BORDER, WS_TABSTOP, WS_VSCROLL,
};

use crate::columns::{COL_NAME, COLS, Layout};
use crate::dialog::{Class, LIST_BOX, Look, Template, apply_theme, center_on_owner, common_msg};
use crate::win::ext::b4 as ext;
use crate::win::{self, Msg, dlg, listbox};

const LIST: u16 = 1000;
const ADD: u16 = 1002;
const UP: u16 = 1004;
const DOWN: u16 = 1005;
const REMOVE: u16 = 1003;
const RESET: u16 = 1010;
const OK: u16 = IDOK as u16;
const CANCEL: u16 = IDCANCEL as u16;

/// The columns as the dialog has them: the shown ones in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub shown: Vec<usize>,
}

impl Order {
    pub fn of(layout: &Layout) -> Order {
        Order {
            shown: layout.shown().to_vec(),
        }
    }

    /// The columns not shown, in the default order.
    pub fn hidden(&self) -> Vec<usize> {
        (0..COLS).filter(|id| !self.shown.contains(id)).collect()
    }

    /// Adds column `id` after `at` (or last); its place.
    pub fn add(&mut self, id: usize, at: Option<usize>) -> Option<usize> {
        if id >= COLS || self.shown.contains(&id) {
            return None;
        }
        let place = at.map_or(self.shown.len(), |a| (a + 1).min(self.shown.len()));
        self.shown.insert(place, id);
        Some(place)
    }

    /// Whether the column at `at` may be removed: any but Name.
    pub fn removable(&self, at: usize) -> bool {
        self.shown.get(at).is_some_and(|&id| id != COL_NAME)
    }

    pub fn remove(&mut self, at: usize) -> bool {
        if !self.removable(at) {
            return false;
        }
        self.shown.remove(at);
        true
    }

    /// Moves the column at `at` up (`-1`) or down (`1`); its new place.
    pub fn shift(&mut self, at: usize, by: isize) -> Option<usize> {
        let to = at.checked_add_signed(by)?;
        if at >= self.shown.len() || to >= self.shown.len() {
            return None;
        }
        self.shown.swap(at, to);
        Some(to)
    }

    /// The layout with these columns in this order, `base`'s widths.
    pub fn layout(&self, base: Layout) -> Layout {
        let shown: Vec<(usize, i32)> = self.shown.iter().map(|&id| (id, base.width(id))).collect();
        Layout::from_shown(base, &shown)
    }
}

struct Organize {
    order: RefCell<Order>,
    look: Cell<Look>,
    done: Cell<bool>,
}

/// Organize Columns over `layout`: the new layout at OK, `None` at Cancel.
pub fn show(owner: HWND, layout: Layout, look: Look) -> Option<Layout> {
    let s = Organize {
        order: RefCell::new(Order::of(&layout)),
        look: Cell::new(look),
        done: Cell::new(false),
    };
    // Everything's dialog: a 387 × 480 pixel client area at 100 %.
    let mut t = Template::new("Organize Columns", 0, 258, 295);
    t.control(
        Class::Named("LISTBOX"),
        "",
        LIST,
        WS_TABSTOP | WS_VSCROLL | WS_BORDER | LIST_BOX | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32,
        0,
        (8, 7, 185, 259),
    )
    .control(
        Class::Button,
        "&Add...",
        ADD,
        WS_TABSTOP,
        0,
        (197, 7, 53, 14),
    )
    .control(
        Class::Button,
        "Move &Up",
        UP,
        WS_TABSTOP,
        0,
        (197, 50, 53, 14),
    )
    .control(
        Class::Button,
        "&Move Down",
        DOWN,
        WS_TABSTOP,
        0,
        (197, 68, 53, 14),
    )
    .control(
        Class::Button,
        "Remo&ve",
        REMOVE,
        WS_TABSTOP,
        0,
        (197, 86, 53, 14),
    )
    .control(
        Class::Button,
        "&Reset",
        RESET,
        WS_TABSTOP,
        0,
        (197, 108, 53, 14),
    )
    .control(
        Class::Button,
        "OK",
        OK,
        WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        0,
        (139, 274, 53, 14),
    )
    .control(
        Class::Button,
        "Cancel",
        CANCEL,
        WS_TABSTOP,
        0,
        (197, 274, 53, 14),
    );
    t.show(owner, &|m| proc(&s, m));
    s.done.get().then(|| s.order.into_inner().layout(layout))
}

fn enable(dlg: HWND, id: u16, on: bool) {
    win::enable(dlg::item(dlg, id), on);
}

impl Organize {
    fn selected(&self, dlg: HWND) -> Option<usize> {
        usize::try_from(listbox::cur_sel(dlg::item(dlg, LIST))).ok()
    }

    /// The list as the order is, `select` selected.
    fn fill(&self, dlg: HWND, select: Option<usize>) {
        let list = dlg::item(dlg, LIST);
        listbox::reset(list);
        for &id in &self.order.borrow().shown {
            listbox::add(list, crate::columns::title(id));
        }
        if let Some(at) = select {
            listbox::set_cur_sel(list, at as i32);
        }
        self.update(dlg);
    }

    fn update(&self, dlg: HWND) {
        let at = self.selected(dlg);
        let o = self.order.borrow();
        let len = o.shown.len();
        enable(dlg, UP, at.is_some_and(|a| a > 0));
        enable(dlg, DOWN, at.is_some_and(|a| a + 1 < len));
        enable(dlg, REMOVE, at.is_some_and(|a| o.removable(a)));
        enable(dlg, ADD, !o.hidden().is_empty());
    }

    /// Add...: the columns not shown, as a menu under the button.
    fn add(&self, dlg: HWND) {
        let hidden = self.order.borrow().hidden();
        if hidden.is_empty() {
            return;
        }
        let menu = crate::menu::columns_menu(&hidden);
        let rc = win::window_rect(dlg::item(dlg, ADD));
        let pt = POINT {
            x: rc.left,
            y: rc.bottom,
        };
        let cmd = crate::popup::track(menu, dlg, pt, "organize columns add", &|_| None);
        // The menu made above.
        ext::destroy_menu(menu);
        let Some(id) = u16::try_from(cmd).ok().and_then(crate::menu::column_of) else {
            return;
        };
        let at = self.selected(dlg);
        let placed = self.order.borrow_mut().add(id, at);
        self.fill(dlg, placed);
    }
}

fn proc(s: &Organize, m: &Msg) -> isize {
    let dlg = m.hwnd;
    if let Some(answer) = common_msg(m, &s.look) {
        return answer;
    }
    match m.id {
        WM_INITDIALOG => {
            s.fill(dlg, Some(0));
            apply_theme(dlg, s.look.get().dark);
            center_on_owner(dlg);
            win::set_focus(dlg::item(dlg, LIST));
            0
        }
        WM_COMMAND => {
            let (id, code) = (win::loword(m.w), u32::from(win::hiword(m.w)));
            match id {
                LIST if code == LBN_SELCHANGE || code == LBN_DBLCLK => s.update(dlg),
                ADD => s.add(dlg),
                UP | DOWN => {
                    if let Some(at) = s.selected(dlg) {
                        let by = if id == UP { -1 } else { 1 };
                        let to = s.order.borrow_mut().shift(at, by);
                        s.fill(dlg, to.or(Some(at)));
                    }
                }
                REMOVE => {
                    if let Some(at) = s.selected(dlg)
                        && s.order.borrow_mut().remove(at)
                    {
                        let len = s.order.borrow().shown.len();
                        s.fill(dlg, Some(at.min(len.saturating_sub(1))));
                    }
                }
                RESET => {
                    *s.order.borrow_mut() = Order::of(&Layout::default());
                    s.fill(dlg, Some(0));
                }
                OK => {
                    s.done.set(true);
                    dlg::end(dlg, 1);
                }
                CANCEL => dlg::end(dlg, 0),
                _ => return 0,
            }
            1
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::{COL_ATTRIB, COL_DATE, COL_PATH, COL_RECENT, COL_SIZE};

    #[test]
    fn the_shown_columns_are_organized() {
        let mut base = Layout::default();
        base.set_width(COL_RECENT, 200);
        let mut o = Order::of(&base);
        assert_eq!(o.shown, [COL_NAME, COL_PATH, COL_SIZE, COL_DATE]);
        assert!(o.hidden().contains(&COL_RECENT));
        // Added after the one selected, or last.
        assert_eq!(o.add(COL_RECENT, Some(0)), Some(1));
        assert_eq!(o.add(COL_ATTRIB, None), Some(5));
        assert_eq!(o.add(COL_ATTRIB, None), None, "once");
        // Name stays.
        assert!(!o.remove(0) && o.remove(2));
        assert_eq!(o.shift(0, 1), Some(1));
        assert_eq!(o.shift(0, -1), None);
        assert_eq!(
            o.shown,
            [COL_RECENT, COL_NAME, COL_SIZE, COL_DATE, COL_ATTRIB]
        );
        let l = o.layout(base);
        assert_eq!(l.shown(), o.shown.as_slice());
        assert_eq!(l.width(COL_RECENT), 200, "widths kept");
        assert!(!l.is_shown(COL_PATH));
    }
}
