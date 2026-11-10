//! The preview pane from the window's side (`crate::preview` is the pane):
//! its thread's messages to the main window, the keys a preview handler
//! passes back, and Everything's layout menu that a right-click on the
//! preview opens.

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
};

use super::App;
use crate::dock::PaneId;
use crate::menu;
use crate::preview::{self, TAB_BACK_OUT, TAB_OUT, WM_REPLIES, WM_RETIRE};
use crate::wide::wz;

impl App {
    /// A command of the preview's range (`menu::F1_FIRST`..): true when it
    /// was one.
    pub(super) fn preview_command(&self, id: u16) -> bool {
        match id {
            // Tab or Shift+Tab with the keyboard in a preview handler's
            // window: on to the next or previous tab stop, as from the
            // pane.
            TAB_OUT | TAB_BACK_OUT => {
                if let Some(pane) = self.pane_hwnd(PaneId::Preview) {
                    self.tab_key(pane, id == TAB_BACK_OUT);
                }
                true
            }
            _ => false,
        }
    }

    /// A message of the preview's range (`slots::F1_MSG`..).
    pub(super) fn preview_message(&self, msg: u32, w: WPARAM, _l: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_RETIRE => preview::retired(w),
            WM_REPLIES => preview::replies_ready(self),
            _ => return None,
        }
        Some(0)
    }

    /// Everything's layout menu (its preview's right-click menu): Filter
    /// Bar, Status Bar, then the panes this build has, each checked while
    /// shown; at `x`, `y` on the screen.
    pub(crate) fn layout_menu(&self, x: i32, y: i32) {
        let mut items: Vec<(u16, String, bool)> = vec![
            (
                menu::FILTER_BAR,
                "Filt&er Bar".into(),
                self.filter_bar_visible(),
            ),
            (
                menu::STATUS_BAR,
                "Status &Bar".into(),
                self.status_bar_visible(),
            ),
        ];
        let mut panes: Vec<PaneId> = PaneId::KNOWN
            .into_iter()
            .filter(|&id| self.pane_registered(id))
            .collect();
        panes.sort_by_key(|id| id.menu_rank());
        let first_pane = items.len();
        for id in panes {
            if let Some((cmd, label)) = id.menu_item() {
                // The menu bar's label without its shortcut, as
                // Everything's menu has it.
                let label = label.split('\t').next().unwrap_or(label);
                items.push((cmd, label.into(), self.pane_visible(id)));
            }
        }
        // SAFETY: a popup menu made here and destroyed after it closes.
        let popup = unsafe { CreatePopupMenu() };
        if popup.is_null() {
            return;
        }
        for (i, (cmd, label, checked)) in items.iter().enumerate() {
            let label = wz(label);
            let check = if *checked { MF_CHECKED } else { MF_UNCHECKED };
            // SAFETY: appends to the menu just made; the label outlives
            // the call (the menu copies it).
            unsafe {
                if i == first_pane {
                    AppendMenuW(popup, MF_SEPARATOR, 0, core::ptr::null());
                }
                AppendMenuW(popup, MF_STRING | check, usize::from(*cmd), label.as_ptr());
            }
        }
        // Shown (or, in a test's dump, written out) while no state of the
        // window is borrowed.
        let cmd = crate::popup::track(popup, self.hwnd, POINT { x, y }, "layout menu", &|_| None);
        // SAFETY: the menu made above, closed now.
        unsafe { DestroyMenu(popup) };
        if cmd > 0 {
            self.command(cmd as u16);
        }
    }
}
