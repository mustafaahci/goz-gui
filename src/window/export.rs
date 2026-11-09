//! File > Export... (Ctrl+S): every result of the search on screen to a
//! file, on a thread of its own (`crate::export`). The status bar follows
//! it; Esc or File > Export... again stops it.

use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, MB_ICONWARNING, MB_OK};

use super::{App, TIMER_NOTE, WM_EXPORT};
use crate::client::Search;
use crate::export::{self, Ended, Format, Job};
use crate::store::location;
use crate::{filedlg, format, msgbox};

impl App {
    /// File > Export...: asks where (a test's `GOZ_GUI_EXPORT_TO` names the
    /// file instead), then writes every result of the search on screen,
    /// not only the rows held. While an export runs, it stops it.
    pub(super) fn export(&self) {
        if self.cancel_export() {
            return;
        }
        let (path, format) = match location::export_to() {
            Some(path) => {
                let format = Format::of_path(&path).unwrap_or(Format::Efu);
                (path, format)
            }
            None => {
                let kind = self.export_type.get();
                let Some((path, kind)) =
                    filedlg::save_kind(self.hwnd, "Export Result List", "", export::KINDS, kind)
                else {
                    return;
                };
                // The extension typed wins over the type chosen.
                let format =
                    Format::of_path(&path).unwrap_or(Format::ALL[kind.min(Format::ALL.len() - 1)]);
                (path, format)
            }
        };
        if location::is_data_file(&path) {
            msgbox::show(
                self.hwnd,
                &format!(
                    "{} is one of goz-gui's own files; an export would replace it. \
                     Choose another file.",
                    path.display()
                ),
                "Export",
                MB_OK | MB_ICONWARNING,
            );
            return;
        }
        if let Some(i) = Format::ALL.iter().position(|&f| f == format) {
            self.export_type.set(i);
            self.settings_changed();
        }
        self.sync_columns();
        let columns = export::csv_columns(self.layout.get().shown());
        let search = {
            let m = self.model.borrow();
            Search {
                query: m.search.query.clone(),
                sort: m.search.sort,
                match_case: m.search.match_case,
            }
        };
        let bom = self.prefs.borrow().base.utf8_bom;
        let running = export::start(
            self.hwnd,
            WM_EXPORT,
            Job {
                search,
                format,
                columns,
                path,
                bom,
            },
        );
        *self.export.borrow_mut() = Some(running);
        self.export_progress();
    }

    /// Stops the export under way, if one is: true if there was one.
    pub(super) fn cancel_export(&self) -> bool {
        match self.export.borrow().as_ref() {
            Some(running) => {
                running.stop();
                true
            }
            None => false,
        }
    }

    /// The status bar says how far the export got, until it ends.
    pub(super) fn export_progress(&self) {
        let Some((done, total)) = self.export.borrow().as_ref().map(|r| r.progress()) else {
            return;
        };
        // SAFETY: a timer of this window: no timer takes the note away.
        unsafe { KillTimer(self.hwnd, TIMER_NOTE) };
        *self.note.borrow_mut() = Some(if total == 0 {
            "Exporting...".to_owned()
        } else {
            format!(
                "Exporting {} of {}...",
                format::grouped(done),
                format::grouped(total)
            )
        });
        self.invalidate_status();
    }

    /// The export's thread ended: the status bar says how.
    pub(super) fn export_ended(&self, ended: Ended) {
        let running = self.export.borrow_mut().take();
        if let Some(running) = running {
            // Its thread posted this as its last act.
            running.finish();
        }
        let note = match ended {
            Ended::Written(n, path) => format!(
                "Exported {} {} to {}",
                format::grouped(n),
                if n == 1 { "item" } else { "items" },
                path.display()
            ),
            Ended::Stopped => "Export cancelled".to_owned(),
            Ended::Failed(why) => {
                // SAFETY: plays a system sound.
                unsafe { MessageBeep(MB_ICONWARNING) };
                format!("Export failed: {why}")
            }
        };
        self.show_note(note);
    }
}
