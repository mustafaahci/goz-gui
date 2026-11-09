//! The window's settings and the settings file: read once before the
//! window is made, written by the store thread about a second after the
//! last change, and at exit (or the end of the Windows session), when the
//! window waits up to [`FLUSH`] for the write.
//!
//! Only the keys whose value changed here since they were last written are
//! written, over whatever the file holds then, so several goz-gui windows
//! keep each other's settings; a key Home holds fixed is never written
//! (see `crate::settings`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SetTimer};

use super::{App, TIMER_SETTINGS, TIMER_STORE_RETRY, WM_STORE};
use crate::bookmarks::one_line;
use crate::placement;
use crate::settings::{Settings, WindowPos};
use crate::store::file::{self, Loaded};
use crate::store::ini::Ini;
use crate::store::location::{self, SETTINGS_FILE};
use crate::store::{IniWrites, writer::Writer};
use crate::wide::window_text;
use crate::{places, syntax};

/// How long after the last change the settings are written.
const SAVE_DELAY_MS: u32 = 1000;
/// How long exit waits for the writes still queued. Longer than a write's
/// wait for another goz-gui (`store::file::WRITE_WAIT`), so a write behind
/// a busy file gives up and the others still go out.
pub(super) const FLUSH: Duration = Duration::from_secs(2);
/// After a write failed, the first try again; each failure after doubles
/// it, up to [`RETRY_MAX_MS`].
const RETRY_FIRST_MS: u32 = 5000;
const RETRY_MAX_MS: u32 = 60_000;

/// WM_STORE's wParam: the write failed and waits to be tried again.
pub(super) const STORE_FAILED: usize = 0;
/// WM_STORE's wParam: written, with something to tell the user.
pub(super) const STORE_NOTE: usize = 1;

/// Tells the window (from the store thread) what a write came to: `text`,
/// and whether the write `failed` (it is tried again). The text is freed
/// again if the window is gone.
pub(super) fn report(hwnd: usize, text: String, failed: bool) {
    let raw = Box::into_raw(Box::new(text));
    let kind = if failed { STORE_FAILED } else { STORE_NOTE };
    // SAFETY: posts to the window; if that fails the box is taken back.
    unsafe {
        if PostMessageW(hwnd as HWND, WM_STORE, kind, raw as isize) == 0 {
            drop(Box::from_raw(raw));
        }
    }
}

/// [`report`] for a write's result: nothing when it went through quietly.
pub(super) fn report_result(hwnd: usize, result: Result<Option<String>, String>) {
    match result {
        Ok(None) => {}
        Ok(Some(note)) => report(hwnd, note, false),
        Err(why) => report(hwnd, why, true),
    }
}

pub(super) struct Prefs {
    /// The settings as read: the ones the window does not change, and the
    /// fixed Home values.
    pub base: Settings,
    /// Each key's value as last read or written: a change is a difference
    /// from it.
    written: BTreeMap<String, String>,
    writes: Option<Arc<IniWrites>>,
    /// What to tell the user once the window is up: the file could not be
    /// read.
    pub note: Option<String>,
    /// A try again is set for after a failed write; the wait it was set
    /// with; and when the last one ran (`retry_delay`).
    retry_armed: bool,
    retry_ms: u32,
    last_retry: Option<Instant>,
}

impl Prefs {
    /// Reads the settings file (before the window exists: it is small).
    pub fn load() -> Prefs {
        let path = location::data_file(SETTINGS_FILE);
        let mut note = None;
        let ini = match &path {
            None => Ini::default(),
            Some(path) => {
                // Another goz-gui recovering the same file waits for this.
                let _lock = file::Lock::acquire(path, Duration::from_secs(2));
                match file::load(path, Ini::parse) {
                    Loaded::Missing => Ini::default(),
                    Loaded::Read(ini, _) => ini,
                    // The backup is the file again (`file::load` put it
                    // back), so the next start reads it too.
                    Loaded::Recovered {
                        backup,
                        moved_to,
                        why,
                        ..
                    } => {
                        let name = moved_to
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        note = Some(if backup.is_some() {
                            format!(
                                "The settings file could not be read ({why}); it was kept as \
                                 {name} and its backup was used"
                            )
                        } else {
                            format!(
                                "The settings file could not be read ({why}); it was kept as \
                                 {name} and the defaults were used"
                            )
                        });
                        backup.unwrap_or_default()
                    }
                    Loaded::Unreadable(why) => {
                        note = Some(format!("The settings file could not be read: {why}"));
                        Ini::default()
                    }
                }
            }
        };
        let base = Settings::from_ini(&ini);
        let written = base.to_pairs().into_iter().collect();
        Prefs {
            base,
            written,
            writes: path.map(|p| Arc::new(IniWrites::new(p, ini))),
            note,
            retry_armed: false,
            retry_ms: 0,
            last_retry: None,
        }
    }
}

impl App {
    /// A setting changed: the file is written once they stop changing.
    pub(super) fn settings_changed(&self) {
        // SAFETY: a timer of this window; setting it again restarts it.
        unsafe { SetTimer(self.hwnd, TIMER_SETTINGS, SAVE_DELAY_MS, None) };
    }

    /// The window's normal rectangle (screen pixels, with the DPI there),
    /// and whether it is maximized (or minimized from maximized).
    pub(super) fn placement_now(&self) -> (Option<WindowPos>, bool) {
        let (pos, maximized) = match placement::normal_place(self.hwnd) {
            Some((pos, maximized)) => (Some(pos), maximized),
            None => (None, false),
        };
        (pos, maximized || self.maximize_deferred.get())
    }

    /// The settings as the window has them now.
    fn current_settings(&self) -> Settings {
        let (window, maximized) = self.placement_now();
        let text = window_text(self.edit);
        let o = self.opts.get();
        let mut s = self.prefs.borrow().base.clone();
        if window.is_some() {
            s.window = window;
        }
        s.maximized = maximized;
        s.match_case = o.match_case;
        s.match_path = o.match_path;
        s.filter = o.filter;
        s.sort = o.sort;
        // One line, as the box shows it (an INI value is one line).
        s.search = one_line(&text);
        s.theme = self.theme_mode();
        s.on_top = self.on_top.get();
        s.status_bar = self.status_bar_visible();
        s.columns = self.layout.get();
        s.view = self.view();
        // The dialogs' places, the Search Syntax window's as it is now if
        // it is open.
        syntax::keep_place();
        s.places = places::all();
        s
    }

    /// Queues a write of the settings changed since the last one.
    pub(super) fn save_settings(&self) {
        self.sync_columns();
        let current = self.current_settings();
        let pairs = current.to_pairs();
        let (changes, all, writes) = {
            let mut prefs = self.prefs.borrow_mut();
            let changes: Vec<(String, String)> = pairs
                .iter()
                .filter(|(k, v)| !current.is_fixed(k) && prefs.written.get(k) != Some(v))
                .cloned()
                .collect();
            for (k, v) in &changes {
                prefs.written.insert(k.clone(), v.clone());
            }
            // Keys missing from the file get the value here; a fixed one
            // the value read.
            let all: Vec<(String, String)> = pairs
                .into_iter()
                .map(|(k, v)| match prefs.written.get(&k) {
                    Some(read) if current.is_fixed(&k) => (k, read.clone()),
                    _ => (k, v),
                })
                .collect();
            (changes, all, prefs.writes.clone())
        };
        let Some(writes) = writes else { return };
        // A write that failed left its changes staged: they go out now even
        // when nothing else changed (the retry, or exit).
        if changes.is_empty() && !writes.pending() {
            return;
        }
        writes.stage(changes, all, current.ini_backup);
        let hwnd = self.hwnd as usize;
        let job = Arc::clone(&writes);
        self.writer.submit(
            writes.path(),
            Box::new(move || report_result(hwnd, job.write_now())),
        );
    }

    /// Writes what changed and waits for the store thread, at most
    /// [`FLUSH`]: the process is about to end.
    pub(super) fn save_and_flush(&self) {
        self.save_everything(true);
        self.writer.flush(FLUSH);
    }

    /// Hands every file with something to write to the store thread: what
    /// changed, and what a failed write left staged. `exit`: the session is
    /// written whatever it holds (the last window closed is the one kept).
    pub(super) fn save_everything(&self, exit: bool) {
        self.save_settings();
        self.save_history();
        self.save_bookmarks();
        self.save_runs();
        self.save_session(exit);
    }

    /// The store thread wrote a file with something to say, or could not
    /// write it (`failed`): the status bar says so, and a failed write is
    /// tried again a little later (5 s, then twice as long each time the
    /// try fails again, up to a minute), so a file held for a moment by
    /// another program (an antivirus, a backup or sync tool) is written
    /// once it is free, not only when something else changes. Several
    /// files failing together share one try.
    pub(super) fn store_report(&self, text: String, failed: bool) {
        self.show_note(text);
        if !failed {
            return;
        }
        let delay = {
            let mut prefs = self.prefs.borrow_mut();
            if prefs.retry_armed {
                // A try is on its way already: it takes this file too.
                return;
            }
            prefs.retry_ms = retry_delay(prefs.retry_ms, prefs.last_retry.map(|t| t.elapsed()));
            prefs.retry_armed = true;
            prefs.retry_ms
        };
        // SAFETY: a timer of this window.
        unsafe { SetTimer(self.hwnd, TIMER_STORE_RETRY, delay, None) };
    }

    /// The retry timer: everything still staged is written again.
    pub(super) fn retry_writes(&self) {
        {
            let mut prefs = self.prefs.borrow_mut();
            prefs.retry_armed = false;
            prefs.last_retry = Some(Instant::now());
        }
        self.save_everything(false);
    }
}

/// The wait before the next try after a failed write: twice the last one
/// (up to a minute) when the failure came from the last try (`since` it
/// ran: within the time a write may wait for the file, and a little more),
/// else the first.
fn retry_delay(last_ms: u32, since_try: Option<Duration>) -> u32 {
    let from_the_try =
        since_try.is_some_and(|t| t < Duration::from_millis(u64::from(RETRY_FIRST_MS)));
    if last_ms > 0 && from_the_try {
        last_ms.saturating_mul(2).min(RETRY_MAX_MS)
    } else {
        RETRY_FIRST_MS
    }
}

/// The store thread, made once per process.
pub(super) fn writer() -> Writer {
    Writer::spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_write_is_tried_again_sooner_then_later() {
        let ms = Duration::from_millis;
        // The first failure (or one long after the last try): 5 s.
        assert_eq!(retry_delay(0, None), 5000);
        assert_eq!(retry_delay(20_000, Some(ms(600_000))), 5000);
        // The try failed again (its writes report within a second or two):
        // twice as long, up to a minute.
        assert_eq!(retry_delay(5000, Some(ms(40))), 10_000);
        assert_eq!(retry_delay(10_000, Some(ms(1600))), 20_000);
        assert_eq!(retry_delay(40_000, Some(ms(5))), 60_000);
        assert_eq!(retry_delay(60_000, Some(ms(5))), 60_000);
    }
}
