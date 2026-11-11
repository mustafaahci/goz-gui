//! The result list's columns as the user arranges them: shown or hidden
//! (the header's right-click menu, a middle click to remove one, Reset
//! Columns), dragged into another order, resized, or sized to fit their
//! text. `crate::columns` holds the layout and its settings keys; this puts
//! it on the list and reads the user's changes back.
//!
//! Name is always the list's column 0 (the list view's item column), so
//! it is never deleted; the others are deleted and inserted as they are
//! hidden and shown. Where each one is on screen is the header's order
//! array. `list_cols` maps a list column's index to its id for everything
//! that gets an index from the list (custom draw, hit tests, clicks).
//!
//! Wave 5: while a file list with dates created is open, the list has one
//! more column after the layout's, Date Created (`COL_CREATED`). It is not
//! in the layout (the settings never keep it); its width lasts while the
//! window does.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetDC, InvalidateRect, ReleaseDC, SelectObject};
use windows_sys::Win32::UI::Controls::{
    LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT, LVCFMT_RIGHT, LVCOLUMNW, LVM_DELETECOLUMN,
    LVM_GETCOLUMNORDERARRAY, LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE, LVM_GETITEMCOUNT,
    LVM_GETTOPINDEX, LVM_INSERTCOLUMNW, LVM_SETCOLUMNORDERARRAY, LVM_SETCOLUMNWIDTH,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyMenu, PostMessageW};

use super::{App, WM_COLUMNS, send};
use crate::columns::{self, COL_CREATED, COL_NAME, COL_SIZE, LIST_COLS, Layout};
use crate::wide::wz;
use crate::{header, menu, paint};

/// The list's columns by index: each one's id, `len` of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListCols {
    ids: [usize; LIST_COLS],
    len: usize,
}

impl ListCols {
    pub fn ids(&self) -> &[usize] {
        &self.ids[..self.len]
    }

    fn push(&mut self, id: usize) {
        if self.len < LIST_COLS {
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

    /// Whether the list has the Date Created column.
    pub fn has_created(&self) -> bool {
        self.index_of(COL_CREATED).is_some()
    }
}

impl App {
    /// The id (`COL_*`) of the list's column `index`.
    pub fn column_id(&self, index: usize) -> Option<usize> {
        self.list_cols.get().ids().get(index).copied()
    }

    /// A column's width at the window's DPI.
    fn column_px(&self, id: usize) -> i32 {
        let width = if id == COL_CREATED {
            self.created_width()
        } else {
            self.layout.get().width(id)
        };
        columns::scale(width, self.dpi.get())
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
        // A file list's dates created: after the layout's columns.
        if self.created_column_wanted() {
            wanted.push(COL_CREATED);
        }
        for &id in &wanted[cols.len..] {
            let mut text = wz(columns::title(id));
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
            .chain(Some(&COL_CREATED))
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
        let mut order = [0i32; LIST_COLS];
        send(
            self.list,
            LVM_GETCOLUMNORDERARRAY,
            cols.len,
            order.as_mut_ptr() as isize,
        );
        let old = self.layout.get();
        let mut layout = old;
        // Date Created is no column of the layout's: its place is not kept.
        let shown: Vec<usize> = order[..cols.len]
            .iter()
            .filter_map(|&i| cols.ids().get(i.max(0) as usize).copied())
            .filter(|&id| id != COL_CREATED)
            .collect();
        layout.set_order(&shown);
        for (index, &id) in cols.ids().iter().enumerate() {
            let px = send(self.list, LVM_GETCOLUMNWIDTH, index, 0) as i32;
            // A width goz-gui set reads back as it was; only the user's
            // own changes it, so it never drifts through rounding. A list
            // that cannot say (0: not in report view, or between rebuilding
            // its columns) changes nothing.
            if px > 0 && px != self.column_px(id) {
                let width = columns::unscale(px, self.dpi.get());
                if id == COL_CREATED {
                    self.set_created_width(width);
                } else {
                    layout.set_width(id, width);
                }
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

    /// The width (at the window's DPI) the list's column `index` needs to
    /// show its title and the text of every held row in view, as they are
    /// drawn: Name with its icon, matches in bold.
    fn fit_px(&self, index: usize) -> Option<i32> {
        let id = self.column_id(index)?;
        let title: Vec<u16> = columns::title(id).encode_utf16().collect();
        let font = self.fonts.borrow().normal;
        // SAFETY: a DC of this thread's list, measured in and given back;
        // the old font is put back first.
        let title_w = unsafe {
            let hdc = GetDC(self.list);
            let old = SelectObject(hdc, font);
            let w = paint::width(hdc, &title);
            SelectObject(hdc, old);
            ReleaseDC(self.list, hdc);
            w
        };
        let title_need = title_w + 2 * self.scale(header::TEXT_PAD);
        let top = send(self.list, LVM_GETTOPINDEX, 0, 0).max(0) as usize;
        let page = send(self.list, LVM_GETCOUNTPERPAGE, 0, 0).max(0) as usize;
        let count = send(self.list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        // The rows in view, a partly shown one at the bottom included.
        let rows = top..(top + page + 1).min(count);
        let m = self.model.borrow();
        let needs = rows
            .filter_map(|i| m.peek(i))
            .map(|row| paint::cell_text(self, &m, row, id, &RECT::default(), 0).need);
        Some(columns::fit_width(title_need, needs))
    }

    /// Sizes the list's columns `indices` to fit their text, and keeps the
    /// widths as the user's own.
    fn size_to_fit(&self, indices: &[usize]) {
        // All measured before any changes: a new width can bring a
        // horizontal scroll bar, which changes the rows in view.
        let widths: Vec<(usize, i32)> = indices
            .iter()
            .filter_map(|&i| Some((i, self.fit_px(i)?)))
            .collect();
        for (index, px) in widths {
            send(self.list, LVM_SETCOLUMNWIDTH, index, px as isize);
        }
        self.sync_columns();
    }

    /// Size All Columns to Fit (the header menu, Ctrl+Num +).
    pub(super) fn size_all_columns_to_fit(&self) {
        let all: Vec<usize> = (0..self.list_cols.get().ids().len()).collect();
        self.size_to_fit(&all);
    }

    /// The header's right-click menu at `pt` (screen), over the list's
    /// column `on` (none from the keyboard, or right of the last column):
    /// sizing that column or all of them to fit, removing it, Name (always
    /// on), the other columns to show or hide, and Reset Columns.
    pub fn header_menu(&self, pt: POINT, on: Option<usize>) {
        let layout = self.layout.get();
        let shown: [bool; columns::COLS] = core::array::from_fn(|id| layout.is_shown(id));
        let on_id = on.and_then(|i| self.column_id(i));
        let on = on.filter(|_| on_id.is_some());
        let popup = menu::header_menu(&shown, on_id);
        let cmd = crate::popup::track(popup, self.hwnd, pt, "header menu", &|_| None);
        // SAFETY: the menu this function made.
        unsafe { DestroyMenu(popup) };
        let Ok(cmd) = u16::try_from(cmd) else {
            return;
        };
        if cmd == menu::SIZE_COLUMN_TO_FIT {
            if let Some(index) = on {
                self.size_to_fit(&[index]);
            }
        } else if cmd == menu::SIZE_ALL_COLUMNS_TO_FIT {
            self.size_all_columns_to_fit();
        } else if cmd == menu::REMOVE_COLUMN {
            if let Some(index) = on {
                self.remove_column(index);
            }
        } else if cmd == menu::RESET_COLUMNS {
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
        assert!(!cols.has_created());
        for id in 1..10 {
            cols.push(id);
        }
        assert_eq!(cols.ids().len(), LIST_COLS, "never more than there are");
        assert!(cols.has_created());
        assert_eq!(columns::title(COL_CREATED), "Date Created");
        assert_eq!(columns::title(COL_DATE), "Date Modified");
    }
}
