//! The result list's columns as the user arranges them: shown or hidden
//! (the header's right-click menu, a middle click to remove one, Reset
//! Columns), dragged into another order, or resized. `crate::columns` holds
//! the layout and its settings keys; this puts it on the list and reads
//! the user's changes back.
//!
//! Name is always the list's column 0 (the list view's item column), so
//! it is never deleted; the others are deleted and inserted as they are
//! hidden and shown. Where each one is on screen is the header's order
//! array. `list_cols` maps a list column's index to its id for everything
//! that gets an index from the list (custom draw, hit tests, clicks).

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::{
    LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVM_DELETECOLUMN,
    LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_INSERTCOLUMNW, LVM_SETCOLUMNORDERARRAY,
    LVM_SETCOLUMNWIDTH,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyMenu, PostMessageW};

use super::{App, WM_COLUMNS, send};
use crate::columns::{self, COL_NAME, COL_SIZE, COLS, Layout, TITLES};
use crate::menu;
use crate::wide::wz;

/// The list's columns by index: each one's id, `len` of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListCols {
    ids: [usize; COLS],
    len: usize,
}

impl ListCols {
    pub fn ids(&self) -> &[usize] {
        &self.ids[..self.len]
    }

    fn push(&mut self, id: usize) {
        if self.len < COLS {
            self.ids[self.len] = id;
            self.len += 1;
        }
    }

    fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    fn index_of(&self, id: usize) -> Option<usize> {
        self.ids().iter().position(|&c| c == id)
    }
}

impl App {
    /// The id (`COL_*`) of the list's column `index`.
    pub fn column_id(&self, index: usize) -> Option<usize> {
        self.list_cols.get().ids().get(index).copied()
    }

    /// A column's width at the window's DPI.
    fn column_px(&self, id: usize) -> i32 {
        columns::scale(self.layout.get().width(id), self.dpi.get())
    }

    /// Puts `layout` on the list: Name stays column 0; the others are
    /// inserted in their order, then the header's order array puts every
    /// column where the layout says.
    pub(super) fn apply_columns(&self, layout: Layout) {
        self.layout.set(layout);
        let mut cols = self.list_cols.get();
        // The others go, from the last; the mirror follows each step, so
        // anything the list asks meanwhile gets the right ids.
        for index in (1..cols.len).rev() {
            send(self.list, LVM_DELETECOLUMN, index, 0);
            cols.truncate(index);
            self.list_cols.set(cols);
        }
        let mut wanted = vec![COL_NAME];
        wanted.extend(layout.shown().iter().copied().filter(|&id| id != COL_NAME));
        for &id in &wanted[cols.len..] {
            let mut text = wz(TITLES[id]);
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if id == COL_SIZE {
                    LVCFMT_RIGHT
                } else {
                    LVCFMT_LEFT
                },
                cx: self.column_px(id),
                pszText: text.as_mut_ptr(),
                // SAFETY: the rest of LVCOLUMNW is plain data.
                ..unsafe { core::mem::zeroed() }
            };
            send(
                self.list,
                LVM_INSERTCOLUMNW,
                cols.len,
                (&raw const col) as isize,
            );
            cols.push(id);
            self.list_cols.set(cols);
        }
        send(
            self.list,
            LVM_SETCOLUMNWIDTH,
            0,
            self.column_px(COL_NAME) as isize,
        );
        let order: Vec<i32> = layout
            .shown()
            .iter()
            .filter_map(|&id| cols.index_of(id))
            .map(|i| i as i32)
            .collect();
        send(
            self.list,
            LVM_SETCOLUMNORDERARRAY,
            order.len(),
            order.as_ptr() as isize,
        );
        // SAFETY: repaints this thread's list and header.
        unsafe {
            InvalidateRect(self.header, core::ptr::null(), 0);
            InvalidateRect(self.list, core::ptr::null(), 0);
        }
    }

    /// The header's drag or resize changed the columns: notes it once the
    /// header is done (its order array changes after HDN_ENDDRAG returns).
    pub(super) fn columns_touched(&self) {
        if !self.columns_posted.replace(true) {
            // SAFETY: posts to this thread's window.
            unsafe { PostMessageW(self.hwnd, WM_COLUMNS, 0, 0) };
        }
    }

    /// Reads the order and widths the user gave the columns into the
    /// layout; a change is saved.
    pub(super) fn sync_columns(&self) {
        self.columns_posted.set(false);
        let cols = self.list_cols.get();
        let mut order = [0i32; COLS];
        send(
            self.list,
            LVM_GETCOLUMNORDERARRAY,
            cols.len,
            order.as_mut_ptr() as isize,
        );
        let old = self.layout.get();
        let mut layout = old;
        let shown: Vec<usize> = order[..cols.len]
            .iter()
            .filter_map(|&i| cols.ids().get(i.max(0) as usize).copied())
            .collect();
        layout.set_order(&shown);
        for (index, &id) in cols.ids().iter().enumerate() {
            let px = send(self.list, LVM_GETCOLUMNWIDTH, index, 0) as i32;
            // A width goz-gui set reads back as it was; only the user's
            // own changes it, so it never drifts through rounding.
            if px != self.column_px(id) {
                layout.set_width(id, columns::unscale(px, self.dpi.get()));
            }
        }
        if layout != old {
            self.layout.set(layout);
            self.settings_changed();
        }
    }

    /// The DPI changed: each column at its width for the new one.
    pub(super) fn rescale_columns(&self) {
        let cols = self.list_cols.get();
        for (index, &id) in cols.ids().iter().enumerate() {
            send(
                self.list,
                LVM_SETCOLUMNWIDTH,
                index,
                self.column_px(id) as isize,
            );
        }
    }

    fn change_columns(&self, change: impl FnOnce(&mut Layout) -> bool) {
        self.sync_columns();
        let mut layout = self.layout.get();
        if change(&mut layout) {
            self.apply_columns(layout);
            self.settings_changed();
        }
    }

    /// A middle click on the header's item `index`: that column goes,
    /// unless it is Name.
    pub fn remove_column(&self, index: usize) {
        if let Some(id) = self.column_id(index) {
            self.change_columns(|l| l.hide(id));
        }
    }

    /// The header's right-click menu at `pt` (screen): Name (always on),
    /// the other columns to show or hide, and Reset Columns.
    pub fn header_menu(&self, pt: POINT) {
        let layout = self.layout.get();
        let shown: [bool; COLS] = core::array::from_fn(|id| layout.is_shown(id));
        let popup = menu::header_menu(&shown);
        let cmd = crate::popup::track(popup, self.hwnd, pt, "header menu", &|_| None);
        // SAFETY: the menu this function made.
        unsafe { DestroyMenu(popup) };
        let Ok(cmd) = u16::try_from(cmd) else {
            return;
        };
        if cmd == menu::RESET_COLUMNS {
            self.change_columns(|l| {
                let reset = Layout::default();
                core::mem::replace(l, reset) != reset
            });
        } else if let Some(id) = menu::column_of(cmd) {
            self.change_columns(|l| {
                if l.is_shown(id) {
                    l.hide(id)
                } else {
                    l.show(id)
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::{COL_DATE, COL_PATH};

    #[test]
    fn list_columns_map_indices_to_ids() {
        let mut cols = ListCols::default();
        for id in [COL_NAME, COL_DATE, COL_PATH] {
            cols.push(id);
        }
        assert_eq!(cols.ids(), [COL_NAME, COL_DATE, COL_PATH]);
        assert_eq!(cols.index_of(COL_PATH), Some(2));
        assert_eq!(cols.index_of(COL_SIZE), None);
        cols.truncate(1);
        assert_eq!(cols.ids(), [COL_NAME]);
        for id in 0..10 {
            cols.push(id);
        }
        assert_eq!(cols.ids().len(), COLS, "never more than there are");
    }
}
