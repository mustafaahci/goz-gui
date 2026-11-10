//! Tools > Options from the window's side: the pages the dialog shows
//! (each a module here, `crate::options` the dialog), the page it opens on,
//! and what follows an OK or Apply.

use super::App;
use crate::dialog;
use crate::options::{self, Page};
use crate::settings::Background;

mod general;
mod history;
mod home;
mod keyboard;
mod results;
mod search;
mod ui;
mod view;

/// The pages, in any order: the dialog puts them in Everything's
/// (`Page::order`, `Page::parent`).
fn pages() -> Vec<Box<dyn Page>> {
    #[allow(unused_mut, reason = "the features push their pages below")]
    let mut pages: Vec<Box<dyn Page>> = vec![
        Box::new(general::General),
        Box::new(ui::Ui),
        Box::new(search::Search),
        Box::new(view::View),
        Box::new(keyboard::Keyboard),
        Box::new(history::History),
    ];
    // Wave 1: each feature adds its pages on its own lines below, so their
    // changes never meet (FRAMEWORK.md).

    // F1 (preview pane).

    // F2 (sidebars).

    // F3 (tabs and windows).

    // F4 (search tools).

    // F5 (file tools).

    // F6 (integration and the Options pages).
    pages.push(Box::new(home::Home));
    pages.push(Box::new(results::Results));

    pages
}

impl App {
    pub(super) fn options_dialog(&self) {
        let first = self.prefs.borrow().base.options_page.clone();
        // Dialogs opened from a background command do not take the
        // foreground (see `dialog::Quiet`).
        let _quiet = dialog::Quiet::begin();
        let last = options::show(self, pages(), &first);
        if last != first {
            self.prefs.borrow_mut().base.options_page = last;
            self.settings_changed();
        }
    }

    /// OK or Apply applied pages: the settings file follows, and the
    /// status bar says what could not be done.
    pub fn options_applied(&self, problems: Vec<String>) {
        self.settings_changed();
        if !problems.is_empty() {
            self.show_note(problems.join("; "));
        }
    }

    /// Changes the tray and hotkey settings as `change` says (the UI and
    /// Keyboard pages each have some of them) and puts them in place.
    fn change_background(&self, change: impl FnOnce(&mut Background), problems: &mut Vec<String>) {
        let mut b = self.background();
        change(&mut b);
        if b == self.background() {
            return;
        }
        let taken = self.apply_background(b);
        self.prefs.borrow_mut().base.background = b;
        if !taken.is_empty() {
            problems.push(format!(
                "Another program already uses the {} hotkey",
                taken.join(" and ")
            ));
        }
    }
}
