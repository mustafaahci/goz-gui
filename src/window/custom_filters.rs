//! The user's own filters on the window (`crate::filters`): Search > Add to
//! Filters... and Organize Filters..., the Search menu's items for them,
//! the filter in force following a change, and `Filters.csv` written on
//! the store thread.

use super::App;
use super::prefs::report_result;
use crate::filter::Filter;
use crate::filters::{self, FilterDef};
use crate::store::file;
use crate::wide::window_text;
use crate::{dialog, filter_dialogs, menu};

impl App {
    /// Puts the user's filters in the Search menu after the built-in ones.
    pub(super) fn filters_menu(&self) {
        let items: Vec<(u16, &str)> = Filter::all()
            .into_iter()
            .filter(|f| matches!(f, Filter::Custom(_)))
            .map(|f| (f.menu_id(), f.label()))
            // Past the most the menu lists, a filter has no id.
            .filter(|&(id, _)| id != 0)
            .collect();
        menu::set_custom_filters(self.menus.search, &items);
    }

    /// Reads `Filters.csv` again if another program changed it.
    pub(super) fn refresh_filters(&self) {
        if filters::refresh() {
            self.filters_changed();
        }
    }

    /// The filters changed: the menu follows; the filter in force, if it
    /// is the user's, searches again as it is now, or gives way to
    /// Everything once it is gone (or no longer offered).
    fn filters_changed(&self) {
        self.filters_menu();
        let mut o = self.opts.get();
        if let Filter::Custom(id) = o.filter {
            if !filters::is_listed(id) {
                o.filter = Filter::Everything;
                self.opts.set(o);
            }
            self.start_search(false);
        }
        self.invalidate_filter_bar();
        self.invalidate_status();
    }

    /// Search > Organize Filters... (Ctrl+Shift+K).
    pub(super) fn organize_filters(&self) {
        self.refresh_filters();
        let list = filters::list();
        let result = {
            let _quiet = dialog::Quiet::begin();
            filter_dialogs::organize(self.hwnd, list.clone(), self.dialog_look())
        };
        if let Some(new) = result
            && new != list
        {
            filters::set(new);
            self.save_filters();
            self.filters_changed();
        }
    }

    /// Search > Add to Filters...: a new filter of the search in the box,
    /// with Match Case and Match Path as they are, named in the editor.
    pub(super) fn add_to_filters(&self) {
        self.refresh_filters();
        let o = self.opts.get();
        let search = window_text(self.edit);
        let def = FilterDef::new("", &search, o.match_case, o.match_path);
        let mut list = filters::list();
        let made = {
            let _quiet = dialog::Quiet::begin();
            filter_dialogs::edit(
                self.hwnd,
                "New Filter",
                &def,
                &list.custom,
                None,
                self.dialog_look(),
            )
        };
        if let Some(def) = made {
            list.custom.push(def);
            filters::set(list);
            self.save_filters();
            self.filters_changed();
        }
    }

    /// Hands `Filters.csv` to the store thread when a change waits (a
    /// write that failed is tried again with the others).
    pub(super) fn save_filters(&self) {
        let backup = self.prefs.borrow().base.csv_backup;
        let (path, text, previous) = match filters::write_job(backup) {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(why) => {
                self.show_note(why);
                return;
            }
        };
        let hwnd = self.hwnd as usize;
        let at = path.clone();
        self.writer.submit(
            &at,
            Box::new(move || {
                let result = file::Lock::for_write(&path, file::WRITE_WAIT).and_then(|_lock| {
                    file::write(&path, text.as_bytes(), previous.as_deref())
                        .map_err(|e| format!("The filters could not be saved: {e}"))
                });
                if result.is_ok() {
                    filters::written(text);
                }
                report_result(hwnd, result);
            }),
        );
    }
}
