//! Type-ahead in the results list (`crate::typeahead` keeps the typed text
//! and matches it): a printable key typed in the list selects the next
//! held row whose name starts with the text typed.

use std::time::Instant;

use windows_sys::Win32::UI::Controls::{
    LVIS_FOCUSED, LVIS_SELECTED, LVM_ENSUREVISIBLE, LVM_GETNEXTITEM, LVM_SETSELECTIONMARK,
    LVNI_FOCUSED,
};

use super::{App, send};
use crate::{celltip, hover, typeahead};

impl App {
    /// `ch` typed in the list.
    pub(super) fn type_ahead(&self, ch: u16) {
        celltip::hide(self);
        let now = Instant::now();
        // A space starts nothing: it is the list's own key (it selects the
        // focused row) unless a name is being typed.
        if ch == u16::from(b' ') && !self.type_ahead.borrow().typing(now) {
            return;
        }
        let focus = usize::try_from(send(
            self.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_FOCUSED as isize,
        ))
        .ok();
        // Both borrows end before the list is told anything.
        let found = {
            let mut typed = self.type_ahead.borrow_mut();
            let q = typed.typed(ch, now);
            let m = self.model.borrow();
            let rows = m
                .loaded()
                .into_iter()
                .flatten()
                .filter_map(|i| m.peek(i).map(|row| (i, row.name())));
            typeahead::find(rows, q.prefix, typeahead::start(focus, q.after))
        };
        let Some(index) = found else {
            return;
        };
        // That row alone, as a click on it would.
        self.set_state(usize::MAX, 0, LVIS_SELECTED);
        self.set_state(
            index,
            LVIS_SELECTED | LVIS_FOCUSED,
            LVIS_SELECTED | LVIS_FOCUSED,
        );
        send(self.list, LVM_SETSELECTIONMARK, 0, index as isize);
        send(self.list, LVM_ENSUREVISIBLE, index, 0);
        hover::scrolled(self);
    }

    /// The list took the keyboard: typing starts a new name.
    pub(super) fn reset_type_ahead(&self) {
        self.type_ahead.borrow_mut().reset();
    }
}
