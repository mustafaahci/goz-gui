//! A test hook, `GOZ_GUI_TRACE=<file>`: a timeline of the window's thread,
//! written to `<file>` as goz-gui ends. Every message the thread takes from
//! its queue (a modal dialog's loop's too), every window it makes, the
//! points goz-gui marks (a dialog's run, `WM_INITDIALOG`, `WM_SHOWWINDOW`)
//! and, with `GOZ_GUI_TRACE_SENDS` too, the sends that make, show and paint
//! a window (`WM_CREATE`, `WM_SHOWWINDOW`, `WM_PAINT`, ...) with their
//! returns (which slow every send down). Each point has the system's precise
//! time (`GetSystemTimePreciseAsFileTime`, which a probe in another process
//! reads alike) and the thread's CPU cycles, so a probe that posts a command
//! and waits for its dialog can see what the thread did meanwhile, for how
//! long, and whether it worked or waited. Without the variable nothing is
//! hooked or kept.

use std::cell::RefCell;
use std::fmt::Write as _;

use windows_sys::Win32::Foundation::{HWND, LRESULT};
use windows_sys::Win32::UI::WindowsAndMessaging::HCBT_CREATEWND;

use crate::win;
use crate::win::hook::{self, Event, Hook, Kind};

/// One point of the timeline.
struct Entry {
    /// The system's precise time, in 100 ns since 1601 (a FILETIME).
    at: u64,
    /// The thread's CPU cycles so far.
    cycles: u64,
    /// `g` taken from the queue, `c` a window made, `s` sent, `r` the send
    /// returned, `m` a mark.
    kind: char,
    msg: u32,
    hwnd: usize,
    w: usize,
    /// The window's class (a mark's name).
    class: String,
}

/// The sends kept: those that make, show and paint windows, and commands.
const SENDS: [u32; 10] = [
    0x0001, // WM_CREATE
    0x0110, // WM_INITDIALOG
    0x0018, // WM_SHOWWINDOW
    0x000f, // WM_PAINT
    0x0014, // WM_ERASEBKGND
    0x0085, // WM_NCPAINT
    0x0111, // WM_COMMAND
    0x0121, // WM_ENTERIDLE
    0x0086, // WM_NCACTIVATE
    0x0047, // WM_WINDOWPOSCHANGED
];

thread_local! {
    /// The timeline while tracing (on the thread that started it).
    static LOG: RefCell<Option<Vec<Entry>>> = const { RefCell::new(None) };
    /// The hooks, removed by [`finish`].
    static HOOKS: RefCell<Vec<Hook>> = const { RefCell::new(Vec::new()) };
}

fn tracing() -> bool {
    LOG.with_borrow(Option::is_some)
}

/// Adds a point stamped now. Whatever it holds was read before.
fn push(kind: char, hwnd: usize, msg: u32, w: usize, class: String) {
    let at = win::time::precise_time();
    let cycles = win::process::thread_cycles();
    let entry = Entry {
        at,
        cycles,
        kind,
        msg,
        hwnd,
        w,
        class,
    };
    LOG.with_borrow_mut(|log| {
        if let Some(log) = log {
            log.push(entry);
        }
    });
}

/// A window's point, if tracing; its class is read first.
fn push_window(kind: char, hwnd: HWND, msg: u32, w: usize) {
    if !tracing() {
        return;
    }
    // The name as a 64-unit buffer takes it: 63 units at most.
    let class = if hwnd.is_null() {
        String::new()
    } else {
        let name: Vec<u16> = crate::win::class_name(hwnd)
            .encode_utf16()
            .take(63)
            .collect();
        String::from_utf16_lossy(&name)
    };
    push(kind, hwnd as usize, msg, w, class);
}

/// A named point of the timeline (`msg` 0, the name as the class), when
/// tracing.
pub fn mark(name: &str) {
    if tracing() {
        push('m', 0, 0, 0, name.to_owned());
    }
}

/// Starts tracing this thread when `GOZ_GUI_TRACE` names a file.
pub fn start() {
    if std::env::var_os("GOZ_GUI_TRACE").is_none() {
        return;
    }
    LOG.set(Some(Vec::with_capacity(1 << 16)));
    let sends = std::env::var_os("GOZ_GUI_TRACE_SENDS").is_some();
    // Hooks of this thread only, removed by `finish`.
    let mut hooks = Vec::new();
    hooks.extend(hook::set(Kind::GetMessage, got));
    hooks.extend(hook::set(Kind::Cbt, cbt));
    if sends {
        hooks.extend(hook::set(Kind::CallWndProc, sent));
        hooks.extend(hook::set(Kind::CallWndProcRet, returned));
    }
    HOOKS.set(hooks);
    mark("trace start");
}

/// Ends tracing and writes the timeline to `GOZ_GUI_TRACE`'s file, one
/// point a line: time (FILETIME), kind, message, its name, class, window,
/// wParam, the thread's cycles.
pub fn finish() {
    // The hooks `start` set on this thread, taken out as they drop.
    drop(HOOKS.take());
    let Some(log) = LOG.take() else {
        return;
    };
    let Some(path) = std::env::var_os("GOZ_GUI_TRACE") else {
        return;
    };
    let _ = std::fs::write(path, text(&log));
}

/// The timeline as `finish` writes it.
fn text(log: &[Entry]) -> String {
    let mut text = String::with_capacity(log.len() * 64);
    for e in log {
        let _ = writeln!(
            text,
            "{}\t{}\t{:#06x}\t{}\t{}\t{:x}\t{}\t{}",
            e.at,
            e.kind,
            e.msg,
            name(e.msg),
            e.class,
            e.hwnd,
            e.w,
            e.cycles
        );
    }
    text
}

/// A message's name, for the common ones.
fn name(msg: u32) -> &'static str {
    match msg {
        0x0001 => "WM_CREATE",
        0x000f => "WM_PAINT",
        0x0010 => "WM_CLOSE",
        0x0014 => "WM_ERASEBKGND",
        0x0018 => "WM_SHOWWINDOW",
        0x0047 => "WM_WINDOWPOSCHANGED",
        0x0085 => "WM_NCPAINT",
        0x0086 => "WM_NCACTIVATE",
        0x0100 => "WM_KEYDOWN",
        0x0110 => "WM_INITDIALOG",
        0x0111 => "WM_COMMAND",
        0x0113 => "WM_TIMER",
        0x0118 => "WM_SYSTIMER",
        0x0121 => "WM_ENTERIDLE",
        0x0200 => "WM_MOUSEMOVE",
        0x8000..=0xbfff => "WM_APP+",
        0xc000..=0xffff => "registered",
        _ => "",
    }
}

// Each hook only looks, and passes every event on unchanged (`None`).

fn got(event: &Event) -> Option<LRESULT> {
    if let Event::Got { msg, removed: true } = event {
        push_window('g', msg.hwnd, msg.message, msg.wParam);
    }
    None
}

fn cbt(event: &Event) -> Option<LRESULT> {
    match *event {
        Event::Create { hwnd, .. } => push_window('c', hwnd, 0, 0),
        // A window made with no creation data to show.
        Event::Cbt { code, w, .. } if code == HCBT_CREATEWND as i32 => {
            push_window('c', w as HWND, 0, 0);
        }
        _ => {}
    }
    None
}

fn sent(event: &Event) -> Option<LRESULT> {
    if let Event::Sent {
        hwnd, message, w, ..
    } = *event
        && SENDS.contains(&message)
    {
        push_window('s', hwnd, message, w);
    }
    None
}

fn returned(event: &Event) -> Option<LRESULT> {
    if let Event::Returned {
        hwnd, message, w, ..
    } = *event
        && SENDS.contains(&message)
    {
        push_window('r', hwnd, message, w);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_kept_unless_tracing() {
        // A thread of its own: nothing started, so a mark keeps nothing.
        std::thread::spawn(|| {
            mark("dialog run");
            assert!(!tracing());
            assert!(LOG.take().is_none());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn points_are_kept_in_order_and_written_one_a_line() {
        std::thread::spawn(|| {
            LOG.set(Some(Vec::new()));
            mark("dialog run");
            push('g', 0x1a2b, 0x0111, 1600, "goz-gui".into());
            mark("dialog shown");
            let log = LOG.take().unwrap();
            assert_eq!(log.len(), 3);
            assert!(log.windows(2).all(|p| p[0].at <= p[1].at));
            assert!(log.windows(2).all(|p| p[0].cycles <= p[1].cycles));
            let text = text(&log);
            let lines: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
            assert_eq!(lines.len(), 3);
            assert_eq!(lines[0][1..7], ["m", "0x0000", "", "dialog run", "0", "0"]);
            assert_eq!(
                lines[1][1..7],
                ["g", "0x0111", "WM_COMMAND", "goz-gui", "1a2b", "1600"]
            );
            assert_eq!(lines[2][4], "dialog shown");
            // Time and cycles are numbers, as a probe reads them.
            assert!(lines.iter().all(|l| l.len() == 8
                && l[0].parse::<u64>().is_ok()
                && l[7].parse::<u64>().is_ok()));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn messages_are_named() {
        assert_eq!(name(0x0110), "WM_INITDIALOG");
        assert_eq!(name(0x8002), "WM_APP+");
        assert_eq!(name(0xc0ef), "registered");
        assert_eq!(name(0x0005), "");
    }
}
