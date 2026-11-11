//! The search worker: one background thread that owns the connection to
//! gozd's pipe, runs each request off the UI thread and posts the answer back
//! to the window as a message.
//!
//! The pipe is opened overlapped so every wait can also wake on the cancel
//! event: a new search makes the request in flight stale, and the worker then
//! drops that connection at once (gozd stops a search whose client went away)
//! instead of reading the rest of an answer nobody will look at.

use std::cell::Cell;
use std::io;
use std::os::windows::io::BorrowedHandle;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use goz_core::proto::{
    DaemonStatus, FrameDecoder, MAX_SERVER_FRAME, PIPE_NAME, PROTO_VERSION, QueryRequest, Request,
    Response, ResultItem, decode_response_frame, encode_request,
};
use goz_core::types::SortSpec;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND,
    ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::wide::wz;

/// Rows per request. A screen shows a few dozen, so one block covers a lot of
/// scrolling while keeping each answer small enough to arrive in one frame.
pub const BLOCK_ROWS: u32 = 1024;

/// How long gozd may stay silent mid-answer before the worker gives up.
const SILENCE_MS: u32 = 30_000;

/// How long a request that went stale may still run to its end. Most
/// searches finish well inside this, and letting them keeps the connection
/// (and gozd) free of abandoned work; dropping the connection instead leaves
/// gozd to finish each one anyway, so a burst of keystrokes would queue up.
const STALE_GRACE: Duration = Duration::from_millis(200);

/// The search each new connection sends after its greeting to learn
/// whether gozd reads the new query language (Everything 1.5's OR, NOT
/// and groups): a gozd that does answers it (no rows asked for), an older
/// one refuses the `|`.
pub const PROBE_QUERY: &str = "goz_gui_probe_a|goz_gui_probe_b";

/// Whether the gozd last spoken to reads the new query language; set by
/// the window from [`Reply::Syntax`], read wherever the search syntax
/// offered depends on it. Off until a probe says otherwise.
static BOOLEAN_SYNTAX: AtomicBool = AtomicBool::new(false);

/// What the gozd last greeted said it is (its name and version) and the
/// protocol version it speaks: Help > Troubleshooting Information's.
static SERVER: Mutex<Option<(String, u16)>> = Mutex::new(None);

/// gozd's name and version and its protocol, once a connection greeted it.
pub fn server() -> Option<(String, u16)> {
    SERVER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether gozd reads OR, NOT and groups (see [`PROBE_QUERY`]).
pub fn boolean_syntax() -> bool {
    BOOLEAN_SYNTAX.load(Ordering::Relaxed)
}

/// Records what the probe learned; `true` when that changed it.
pub fn set_boolean_syntax(on: bool) -> bool {
    BOOLEAN_SYNTAX.swap(on, Ordering::Relaxed) != on
}

/// What gozd's answer to [`PROBE_QUERY`] says of its syntax: results, the
/// new one; a refusal, the old; anything else, nothing.
fn probe_verdict(response: &Response) -> Option<bool> {
    match response {
        Response::Results(_) => Some(true),
        Response::Error { .. } => Some(false),
        _ => None,
    }
}

/// What the user searched for.
pub struct Search {
    pub query: String,
    pub sort: SortSpec,
    pub match_case: bool,
    /// The file list searched instead of gozd's index (File > Open File
    /// List..., wave 4): its rows answer every block and stream of the
    /// search, from the thread that asks.
    pub filelist: Option<Arc<crate::filelist::FileList>>,
}

/// One result row, kept as UTF-16 because that is what every draw wants.
pub struct Row {
    pub path: Box<[u16]>,
    /// Where the name starts in `path`.
    pub name_start: u32,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub is_dir: bool,
    /// Index into the system image list, or one of the `icons::` states.
    pub icon: Cell<i32>,
}

impl Row {
    pub fn name(&self) -> &[u16] {
        &self.path[self.name_start as usize..]
    }

    /// The folder holding the row, without its trailing separator.
    pub fn parent(&self) -> &[u16] {
        &self.path[..(self.name_start as usize).saturating_sub(1)]
    }

    fn from_item(item: ResultItem) -> Row {
        let path: Box<[u16]> = match item.path_u16 {
            Some(units) => units.into(),
            None => item.path.encode_utf16().collect(),
        };
        Row::new(path, item.size, item.mtime_ft, item.is_dir)
    }

    /// A row for the full path `path` (a file list's, wave 4).
    pub fn new(path: Box<[u16]>, size: Option<u64>, mtime: Option<i64>, is_dir: bool) -> Row {
        let after_sep = path
            .iter()
            .rposition(|&c| c == u16::from(b'\\'))
            .map_or(0, |i| i + 1);
        // A volume root ("C:\") is all name.
        let name_start = if after_sep >= path.len() {
            0
        } else {
            after_sep
        };
        Row {
            path,
            name_start: name_start as u32,
            size,
            mtime,
            is_dir,
            icon: Cell::new(crate::icons::UNSET),
        }
    }
}

/// Why a request got no answer.
#[derive(Clone, Debug)]
pub enum Failure {
    /// The pipe does not exist: gozd is not running.
    NotRunning,
    /// Something owns the pipe name that is not the elevated daemon.
    Untrusted,
    /// gozd refused the query, with its reason.
    Refused(String),
    Other(String),
}

pub enum Job {
    Block {
        epoch: u64,
        block: u32,
        search: Arc<Search>,
    },
    Status,
}

pub enum Reply {
    Block {
        epoch: u64,
        block: u32,
        rows: Vec<Row>,
        total: u64,
        incomplete: bool,
        metadata_pending: bool,
        /// The summed size of every matching file; only block 0 asks.
        total_size: Option<u64>,
        elapsed: Duration,
    },
    Failed {
        epoch: u64,
        block: u32,
        failure: Failure,
    },
    Status(Result<DaemonStatus, Failure>),
    /// What a new connection's probe learned of gozd's syntax, posted when
    /// it differs from what was learned before (the first time, and when
    /// gozd was restarted as another build): whether it reads OR, NOT and
    /// groups.
    Syntax(bool),
}

struct Event(HANDLE);

// SAFETY: an event handle may be signalled and waited on from any thread.
unsafe impl Send for Event {}
// SAFETY: as above; the handle is never closed while shared.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Event {
        // SAFETY: an unnamed, manual-reset, initially unsignalled event.
        let h = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
        assert!(!h.is_null(), "CreateEventW failed");
        Event(h)
    }
    fn set(&self) {
        // SAFETY: the handle is a live event.
        unsafe { SetEvent(self.0) };
    }
    fn reset(&self) {
        // SAFETY: the handle is a live event.
        unsafe { ResetEvent(self.0) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

struct Queue {
    /// The newest search generation; block jobs of older ones are stale.
    latest: u64,
    jobs: Vec<Job>,
    quit: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    work: Event,
    cancel: Event,
    hwnd: usize,
    msg: u32,
    /// What the probe last learned: 0 nothing yet, 1 the old syntax, 2
    /// the new.
    syntax: AtomicU8,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_stale(&self, epoch: Option<u64>) -> bool {
        let q = self.lock();
        q.quit || epoch.is_some_and(|g| g < q.latest)
    }

    fn post(&self, reply: Reply) {
        let p = Box::into_raw(Box::new(reply));
        // SAFETY: the window owns the box once the message is queued; if
        // posting fails (the window is gone) it is reclaimed here.
        unsafe {
            if PostMessageW(self.hwnd as HWND, self.msg, 0, p as isize) == 0 {
                drop(Box::from_raw(p));
            }
        }
    }
}

pub struct Worker {
    shared: Arc<Shared>,
}

impl Worker {
    /// Starts the worker; its replies arrive at `hwnd` as message `msg`, with
    /// a `Box<Reply>` in `lParam`.
    pub fn spawn(hwnd: HWND, msg: u32) -> Worker {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                latest: 0,
                jobs: Vec::new(),
                quit: false,
            }),
            work: Event::new(),
            cancel: Event::new(),
            hwnd: hwnd as usize,
            msg,
            syntax: AtomicU8::new(0),
        });
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("goz-search".into())
            .spawn(move || run(&thread_shared))
            .expect("spawn search thread");
        Worker { shared }
    }

    /// Makes `epoch` the newest search: queued blocks of older searches are
    /// dropped, and one in flight is abandoned.
    pub fn begin(&self, epoch: u64) {
        let mut q = self.shared.lock();
        q.latest = epoch;
        q.jobs
            .retain(|j| !matches!(j, Job::Block { epoch: g, .. } if *g < epoch));
        self.shared.cancel.set();
    }

    pub fn push(&self, job: Job) {
        self.shared.lock().jobs.push(job);
        self.shared.work.set();
    }

    /// Drops the queued blocks of search `epoch` that `wanted` says are no
    /// longer wanted, and returns them. Scrolling asks for every block it
    /// paints on the way; without this, each of them would still be run
    /// (a full sorted query in gozd each) long after the list moved on.
    pub fn trim(&self, epoch: u64, wanted: impl Fn(u32) -> bool) -> Vec<u32> {
        trim_jobs(&mut self.shared.lock().jobs, epoch, wanted)
    }
}

/// [`Worker::trim`] on a queue.
fn trim_jobs(jobs: &mut Vec<Job>, epoch: u64, wanted: impl Fn(u32) -> bool) -> Vec<u32> {
    let mut dropped = Vec::new();
    jobs.retain(|job| match *job {
        Job::Block {
            epoch: e, block, ..
        } if e == epoch && !wanted(block) => {
            dropped.push(block);
            false
        }
        _ => true,
    });
    dropped
}

impl Worker {
    pub fn quit(&self) {
        self.shared.lock().quit = true;
        self.shared.work.set();
        self.shared.cancel.set();
    }
}

/// Stops a [`stream`] running on another thread: its read is abandoned
/// (after the grace a stale search gets) and the stream ends.
#[derive(Clone)]
pub struct Stop(Arc<Shared>);

impl Stop {
    pub fn new() -> Stop {
        // Nothing is ever posted through this one: only its quit flag and
        // its cancel event are used.
        Stop(Arc::new(Shared {
            queue: Mutex::new(Queue {
                latest: 0,
                jobs: Vec::new(),
                quit: false,
            }),
            work: Event::new(),
            cancel: Event::new(),
            hwnd: 0,
            msg: 0,
            syntax: AtomicU8::new(0),
        }))
    }

    pub fn stop(&self) {
        self.0.lock().quit = true;
        self.0.cancel.set();
    }

    pub fn stopped(&self) -> bool {
        self.0.lock().quit
    }
}

/// How a [`stream`] ended.
pub enum Streamed {
    /// Every row went to the callback; how many.
    Done(u64),
    Stopped,
    Failed(Failure),
}

/// Runs `search` whole (no limit) on a connection of its own, handing each
/// page gozd sends (up to `PAGE_ROWS` rows) and the total to `page` as it
/// arrives; `page` returns an error to end it. Meant for a thread of its
/// own (File > Export): the worker's searches go on meanwhile, and no page
/// is kept once `page` has had it.
pub fn stream(
    stop: &Stop,
    search: &Search,
    mut page: impl FnMut(Vec<Row>, u64) -> Result<(), String>,
) -> Streamed {
    let all = RowRange {
        offset: 0,
        limit: None,
        details: true,
    };
    stream_range(stop, search, all, |rows, total| {
        page(rows, total).map(|()| true)
    })
}

/// The rows of a search [`stream_range`] asks for.
pub struct RowRange {
    /// The first row, and how many from it (`None`: to the end).
    pub offset: u32,
    pub limit: Option<u32>,
    /// Sizes and dates too, not only the paths.
    pub details: bool,
}

/// [`stream`] for the rows `range` names. `page` returns whether it wants
/// more: `Ok(false)` ends the stream there (`Done` with the rows had so
/// far), and the connection, which is this call's own, goes with it, so
/// gozd stops sending.
pub fn stream_range(
    stop: &Stop,
    search: &Search,
    range: RowRange,
    mut page: impl FnMut(Vec<Row>, u64) -> Result<bool, String>,
) -> Streamed {
    if let Some(list) = &search.filelist {
        return list_stream(stop, search, range, list, page);
    }
    let shared = &*stop.0;
    let mut conn = None;
    let pipe = match connect(&mut conn, shared) {
        Ok(p) => p,
        Err(_) if stop.stopped() => return Streamed::Stopped,
        Err(f) => return Streamed::Failed(f),
    };
    let request = Request::Query(QueryRequest {
        query: search.query.clone(),
        scope: None,
        sort: search.sort,
        offset: range.offset,
        limit: range.limit,
        want_size: range.details,
        want_mtime: range.details,
        match_case: search.match_case,
        want_total_size: false,
    });
    if let Err(e) = pipe.send(&request) {
        return match e {
            Io::Cancelled => Streamed::Stopped,
            Io::Lost(m) => Streamed::Failed(Failure::Other(m)),
        };
    }
    let mut rows = 0u64;
    loop {
        if stop.stopped() {
            return Streamed::Stopped;
        }
        match pipe.response(shared, None) {
            Ok(Response::Results(p)) => {
                let more = p.more;
                rows += p.items.len() as u64;
                let items = p.items.into_iter().map(Row::from_item).collect();
                match page(items, p.total) {
                    Err(why) => return Streamed::Failed(Failure::Other(why)),
                    Ok(false) => return Streamed::Done(rows),
                    Ok(true) => {}
                }
                if !more {
                    return Streamed::Done(rows);
                }
            }
            Ok(Response::Error { message, .. }) => {
                return Streamed::Failed(Failure::Refused(message));
            }
            Ok(_) => {
                return Streamed::Failed(Failure::Other("unexpected reply from gozd".into()));
            }
            Err(Io::Cancelled) => return Streamed::Stopped,
            Err(Io::Lost(_)) if stop.stopped() => return Streamed::Stopped,
            Err(Io::Lost(m)) => return Streamed::Failed(Failure::Other(m)),
        }
    }
}

fn run(shared: &Shared) {
    let mut conn: Option<Pipe> = None;
    loop {
        let job = loop {
            {
                let mut q = shared.lock();
                if q.quit {
                    return;
                }
                let latest = q.latest;
                q.jobs
                    .retain(|j| !matches!(j, Job::Block { epoch, .. } if *epoch < latest));
                // Status first; otherwise the newest block, which is the one
                // the user scrolled to last.
                let pick = q
                    .jobs
                    .iter()
                    .position(|j| matches!(j, Job::Status))
                    .or(q.jobs.len().checked_sub(1));
                if let Some(i) = pick {
                    // The cancel event speaks of jobs older than this one.
                    shared.cancel.reset();
                    shared.work.reset();
                    if q.jobs.len() > 1 {
                        shared.work.set();
                    }
                    break q.jobs.remove(i);
                }
                shared.work.reset();
            }
            // SAFETY: a live event handle.
            unsafe { WaitForSingleObject(shared.work.0, INFINITE) };
        };
        match job {
            Job::Status => {
                let status = status(&mut conn, shared);
                shared.post(Reply::Status(status));
            }
            Job::Block {
                epoch,
                block,
                search,
            } => {
                if let Some(reply) = fetch_block(&mut conn, shared, epoch, block, &search) {
                    shared.post(reply);
                }
            }
        }
    }
}

/// Runs one block of a search. `None` means the search went stale and its
/// answer is not wanted.
fn fetch_block(
    conn: &mut Option<Pipe>,
    shared: &Shared,
    epoch: u64,
    block: u32,
    search: &Search,
) -> Option<Reply> {
    if let Some(list) = &search.filelist {
        return list_block(shared, epoch, block, search, list);
    }
    let started = Instant::now();
    let request = Request::Query(QueryRequest {
        query: search.query.clone(),
        scope: None,
        sort: search.sort,
        offset: block * BLOCK_ROWS,
        limit: Some(BLOCK_ROWS),
        want_size: true,
        want_mtime: true,
        match_case: search.match_case,
        // Summing every match costs gozd a pass over them, so only the
        // first block of a search asks; the status bar shows it.
        want_total_size: block == 0,
    });
    let failed = |failure| {
        Some(Reply::Failed {
            epoch,
            block,
            failure,
        })
    };
    // A kept connection may have died with a restarted daemon: one retry on
    // a fresh one tells that apart from a daemon that is really gone.
    for attempt in 0..2 {
        let reused = conn.is_some();
        let pipe = match connect(conn, shared) {
            Ok(p) => p,
            Err(f) => return failed(f),
        };
        pipe.give_up_at.set(None);
        let mut rows = Vec::new();
        let (mut total, mut incomplete, mut pending) = (0, false, false);
        let mut total_size = None;
        let outcome = pipe.send(&request).and_then(|()| {
            loop {
                match pipe.response(shared, Some(epoch))? {
                    Response::Results(page) => {
                        total = page.total;
                        incomplete |= page.volumes_incomplete;
                        pending |= page.metadata_pending;
                        total_size = page.total_size.or(total_size);
                        rows.extend(page.items.into_iter().map(Row::from_item));
                        if !page.more {
                            break Ok(None);
                        }
                    }
                    Response::Error { message, .. } => break Ok(Some(Failure::Refused(message))),
                    _ => break Err(Io::Lost("unexpected reply from gozd".into())),
                }
            }
        });
        match outcome {
            // Finished inside its grace period, but nobody wants it now.
            Ok(None) if shared.is_stale(Some(epoch)) => return None,
            Ok(None) => {
                return Some(Reply::Block {
                    epoch,
                    block,
                    rows,
                    total,
                    incomplete,
                    metadata_pending: pending,
                    total_size,
                    elapsed: started.elapsed(),
                });
            }
            Ok(Some(f)) => return failed(f),
            Err(Io::Cancelled) => {
                *conn = None;
                return None;
            }
            Err(Io::Lost(m)) => {
                *conn = None;
                if !(reused && attempt == 0) {
                    return failed(Failure::Other(m));
                }
            }
        }
    }
    failed(Failure::Other("gozd closed the connection".into()))
}

/// A block of a search over an open file list (wave 4): its rows, from
/// the list's answer, made here on the worker's thread (and kept, so the
/// search's other blocks cost a copy each). `None` when the search went
/// stale before it was made.
fn list_block(
    shared: &Shared,
    epoch: u64,
    block: u32,
    search: &Search,
    list: &crate::filelist::FileList,
) -> Option<Reply> {
    let started = Instant::now();
    let stale = || shared.is_stale(Some(epoch));
    match list.answer(&search.query, search.sort, search.match_case, &stale) {
        Ok(None) => None,
        Err(why) => Some(Reply::Failed {
            epoch,
            block,
            failure: Failure::Refused(why),
        }),
        Ok(Some(m)) => {
            let from = block as usize * BLOCK_ROWS as usize;
            let rows = list.rows_of(&m, from..from + BLOCK_ROWS as usize);
            Some(Reply::Block {
                epoch,
                block,
                rows,
                total: m.order.len() as u64,
                // Still being read: the window searches again in a moment,
                // as while gozd indexes.
                incomplete: !m.complete,
                metadata_pending: false,
                total_size: (block == 0).then_some(m.total_size),
                elapsed: started.elapsed(),
            })
        }
    }
}

/// [`stream_range`] over an open file list: its answer, a page at a time.
fn list_stream(
    stop: &Stop,
    search: &Search,
    range: RowRange,
    list: &crate::filelist::FileList,
    mut page: impl FnMut(Vec<Row>, u64) -> Result<bool, String>,
) -> Streamed {
    const PAGE: usize = 4096;
    let stale = || stop.stopped();
    let m = match list.answer(&search.query, search.sort, search.match_case, &stale) {
        Ok(Some(m)) => m,
        Ok(None) => return Streamed::Stopped,
        Err(why) => return Streamed::Failed(Failure::Refused(why)),
    };
    let total = m.order.len();
    let end = range
        .limit
        .map_or(total, |l| {
            (range.offset as usize).saturating_add(l as usize)
        })
        .min(total);
    let mut at = (range.offset as usize).min(end);
    if at == end {
        // Nothing to hand over, but the total, as gozd's empty page says.
        return match page(Vec::new(), total as u64) {
            Err(why) => Streamed::Failed(Failure::Other(why)),
            Ok(_) => Streamed::Done(0),
        };
    }
    let mut done = 0u64;
    while at < end {
        if stop.stopped() {
            return Streamed::Stopped;
        }
        let next = (at + PAGE).min(end);
        let rows = list.rows_of(&m, at..next);
        done += rows.len() as u64;
        match page(rows, total as u64) {
            Err(why) => return Streamed::Failed(Failure::Other(why)),
            Ok(false) => return Streamed::Done(done),
            Ok(true) => {}
        }
        at = next;
    }
    Streamed::Done(done)
}

fn status(conn: &mut Option<Pipe>, shared: &Shared) -> Result<DaemonStatus, Failure> {
    for attempt in 0..2 {
        let reused = conn.is_some();
        let pipe = connect(conn, shared)?;
        let outcome = pipe
            .send(&Request::Status)
            .and_then(|()| pipe.response(shared, None));
        match outcome {
            Ok(Response::Status(s)) => return Ok(s),
            Ok(Response::Error { message, .. }) => return Err(Failure::Refused(message)),
            Ok(_) => {
                *conn = None;
                return Err(Failure::Other("unexpected reply from gozd".into()));
            }
            Err(Io::Cancelled) => return Err(Failure::Other("cancelled".into())),
            Err(Io::Lost(m)) => {
                *conn = None;
                if !(reused && attempt == 0) {
                    return Err(Failure::Other(m));
                }
            }
        }
    }
    Err(Failure::Other("gozd closed the connection".into()))
}

/// The kept connection, opened and greeted first if there is none.
fn connect<'a>(conn: &'a mut Option<Pipe>, shared: &Shared) -> Result<&'a mut Pipe, Failure> {
    if conn.is_none() {
        let mut pipe = Pipe::open()?;
        pipe.send(&Request::Hello {
            proto_min: PROTO_VERSION,
            proto_max: PROTO_VERSION,
            client: concat!("goz-gui ", env!("CARGO_PKG_VERSION")).into(),
        })
        .map_err(Io::into_failure)?;
        match pipe.response(shared, None).map_err(Io::into_failure)? {
            Response::Hello { proto, server, .. } => {
                *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some((server, proto));
            }
            Response::Error { message, .. } => {
                return Err(Failure::Other(format!(
                    "gozd refused this client: {message}"
                )));
            }
            _ => return Err(Failure::Other("unexpected reply from gozd".into())),
        }
        // Only the worker's connections ask (a stream's has nobody to tell).
        if shared.msg != 0 {
            probe(&mut pipe, shared).map_err(Io::into_failure)?;
        }
        *conn = Some(pipe);
    }
    Ok(conn.as_mut().expect("connected above"))
}

/// Sends [`PROBE_QUERY`] on a new connection and posts what it learned
/// when that is news. A refusal leaves the connection as good as before.
fn probe(pipe: &mut Pipe, shared: &Shared) -> Result<(), Io> {
    pipe.send(&Request::Query(QueryRequest {
        query: PROBE_QUERY.into(),
        scope: None,
        sort: SortSpec::default(),
        offset: 0,
        limit: Some(0),
        want_size: false,
        want_mtime: false,
        match_case: false,
        want_total_size: false,
    }))?;
    let verdict = loop {
        match pipe.response(shared, None)? {
            Response::Results(page) if page.more => {}
            other => break probe_verdict(&other),
        }
    };
    if let Some(on) = verdict {
        let code = if on { 2 } else { 1 };
        if shared.syntax.swap(code, Ordering::Relaxed) != code {
            shared.post(Reply::Syntax(on));
        }
    }
    Ok(())
}

enum Io {
    /// The request went stale while waiting.
    Cancelled,
    /// The connection broke or timed out; it cannot be used again.
    Lost(String),
}

impl Io {
    fn into_failure(self) -> Failure {
        match self {
            Io::Cancelled => Failure::Other("cancelled".into()),
            Io::Lost(m) => Failure::Other(m),
        }
    }
}

struct Pipe {
    handle: HANDLE,
    /// Signals each overlapped read or write.
    event: Event,
    decoder: FrameDecoder,
    buf: Vec<u8>,
    /// Set once the request in flight went stale: when to stop waiting.
    give_up_at: Cell<Option<Instant>>,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the handle is owned; no I/O is outstanding once a call
        // below returns, as each waits for its own completion.
        unsafe { CloseHandle(self.handle) };
    }
}

impl Pipe {
    fn open() -> Result<Pipe, Failure> {
        let name = wz(PIPE_NAME);
        let mut tries = 0;
        let handle = loop {
            // SAFETY: `name` is NUL-terminated. SECURITY_SQOS_PRESENT with an
            // anonymous level keeps a squatting server from impersonating us.
            let h = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    core::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT,
                    core::ptr::null_mut(),
                )
            };
            if h != INVALID_HANDLE_VALUE {
                break h;
            }
            // SAFETY: reads this thread's last error.
            match unsafe { GetLastError() } {
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => return Err(Failure::NotRunning),
                // gozd keeps one listening instance; another client may hold
                // it for a moment.
                ERROR_PIPE_BUSY if tries < 8 => {
                    tries += 1;
                    // SAFETY: `name` is NUL-terminated.
                    unsafe { WaitNamedPipeW(name.as_ptr(), 250) };
                }
                e => {
                    return Err(Failure::Other(format!(
                        "cannot open {PIPE_NAME}: {}",
                        io::Error::from_raw_os_error(e as i32)
                    )));
                }
            }
        };
        // Only the elevated daemon may answer: a non-elevated process can
        // create the pipe name first and serve made-up results.
        // SAFETY: `handle` is open for the duration of the call.
        let trusted =
            goz_winfs::pipe_server_is_trusted(unsafe { BorrowedHandle::borrow_raw(handle) });
        if !matches!(trusted, Ok(true)) {
            // SAFETY: closing the handle just opened.
            unsafe { CloseHandle(handle) };
            return Err(Failure::Untrusted);
        }
        Ok(Pipe {
            handle,
            event: Event::new(),
            decoder: FrameDecoder::new(MAX_SERVER_FRAME),
            buf: vec![0; 256 * 1024],
            give_up_at: Cell::new(None),
        })
    }

    fn send(&mut self, request: &Request) -> Result<(), Io> {
        let mut out = Vec::new();
        encode_request(request, &mut out);
        let mut sent = 0;
        while sent < out.len() {
            // SAFETY: `ov` and `out` outlive the write, whose completion is
            // awaited before either is touched again.
            unsafe {
                let mut ov: OVERLAPPED = core::mem::zeroed();
                ov.hEvent = self.event.0;
                let ok = WriteFile(
                    self.handle,
                    out[sent..].as_ptr(),
                    (out.len() - sent) as u32,
                    core::ptr::null_mut(),
                    &mut ov,
                );
                if ok == 0 && GetLastError() != ERROR_IO_PENDING {
                    return Err(Io::Lost(last_error("cannot write to gozd")));
                }
                let mut n = 0;
                if GetOverlappedResult(self.handle, &ov, &mut n, 1) == 0 || n == 0 {
                    return Err(Io::Lost(last_error("cannot write to gozd")));
                }
                sent += n as usize;
            }
        }
        Ok(())
    }

    /// The next reply frame, decoded. `epoch` is the search the read serves;
    /// `None` for a request no new search makes stale.
    fn response(&mut self, shared: &Shared, epoch: Option<u64>) -> Result<Response, Io> {
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => {
                    return decode_response_frame(&frame).map_err(|e| Io::Lost(e.to_string()));
                }
                Ok(None) => {}
                Err(e) => return Err(Io::Lost(e.to_string())),
            }
            let n = self.read_some(shared, epoch)?;
            self.decoder.feed(&self.buf[..n]);
        }
    }

    fn read_some(&mut self, shared: &Shared, epoch: Option<u64>) -> Result<usize, Io> {
        // SAFETY: `ov` and `self.buf` stay put until the read completes or is
        // cancelled, and every path below waits for that before returning.
        unsafe {
            let mut ov: OVERLAPPED = core::mem::zeroed();
            ov.hEvent = self.event.0;
            let ok = ReadFile(
                self.handle,
                self.buf.as_mut_ptr(),
                self.buf.len() as u32,
                core::ptr::null_mut(),
                &mut ov,
            );
            if ok == 0 {
                match GetLastError() {
                    ERROR_IO_PENDING => self.wait(&mut ov, shared, epoch)?,
                    ERROR_MORE_DATA => {}
                    _ => return Err(Io::Lost(last_error("gozd closed the connection"))),
                }
            }
            let mut n = 0;
            if GetOverlappedResult(self.handle, &ov, &mut n, 0) == 0
                && GetLastError() != ERROR_MORE_DATA
            {
                return Err(Io::Lost(last_error("gozd closed the connection")));
            }
            if n == 0 {
                return Err(Io::Lost("gozd closed the connection".into()));
            }
            Ok(n as usize)
        }
    }

    /// Waits for the pending I/O on `ov`. Once its search goes stale it gets
    /// [`STALE_GRACE`] more to finish before it is abandoned; gozd going
    /// silent abandons it too.
    unsafe fn wait(
        &self,
        ov: &mut OVERLAPPED,
        shared: &Shared,
        epoch: Option<u64>,
    ) -> Result<(), Io> {
        let handles = [self.event.0, shared.cancel.0];
        loop {
            if let Some(deadline) = self.give_up_at.get() {
                let left = deadline.saturating_duration_since(Instant::now());
                // SAFETY: a live event handle.
                let w = unsafe { WaitForSingleObject(self.event.0, left.as_millis() as u32) };
                if w == WAIT_OBJECT_0 {
                    return Ok(());
                }
                // SAFETY: `ov` is the pending request on this handle.
                unsafe { self.abort(ov) };
                return Err(Io::Cancelled);
            }
            // SAFETY: both handles are live events.
            let w = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, SILENCE_MS) };
            if w == WAIT_OBJECT_0 {
                return Ok(());
            }
            if w == WAIT_OBJECT_0 + 1 {
                if !shared.is_stale(epoch) {
                    // Raised for an older request; take it down and look again
                    // in case a newer search raised it meanwhile.
                    shared.cancel.reset();
                    if !shared.is_stale(epoch) {
                        continue;
                    }
                }
                self.give_up_at.set(Some(Instant::now() + STALE_GRACE));
                continue;
            }
            // SAFETY: as above.
            unsafe { self.abort(ov) };
            return Err(Io::Lost("gozd did not reply in time".into()));
        }
    }

    unsafe fn abort(&self, ov: &mut OVERLAPPED) {
        let mut n = 0;
        // SAFETY: cancels and then waits for the one request using `ov`, so
        // the kernel is done with it and its buffer on return.
        unsafe {
            CancelIoEx(self.handle, ov);
            GetOverlappedResult(self.handle, ov, &mut n, 1);
        }
    }
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", io::Error::last_os_error())
}

// Used by the window to describe a missing daemon.
pub fn not_running_text() -> String {
    use goz_winfs::ServiceState;
    match goz_winfs::service_state(goz_core::proto::SERVICE_NAME) {
        Ok(ServiceState::NotInstalled) => {
            "The goz service is not installed. Run gozd.exe install from an elevated terminal."
                .into()
        }
        Ok(ServiceState::Disabled) => "The goz service is disabled.".into(),
        Ok(ServiceState::Stopped) => "The goz service is stopped. Tools > Start goz Service".into(),
        Ok(ServiceState::Starting) => "The goz service is starting…".into(),
        Ok(ServiceState::Stopping) => "The goz service is stopping.".into(),
        Ok(ServiceState::Paused) => "The goz service is paused.".into(),
        Ok(ServiceState::Running) => "Waiting for gozd…".into(),
        Err(_) => "gozd is not running.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(epoch: u64, block: u32) -> Job {
        Job::Block {
            epoch,
            block,
            search: Arc::new(Search {
                query: String::new(),
                sort: SortSpec::default(),
                match_case: false,
                filelist: None,
            }),
        }
    }

    fn queued(jobs: &[Job]) -> Vec<(u64, u32)> {
        jobs.iter()
            .filter_map(|j| match *j {
                Job::Block { epoch, block, .. } => Some((epoch, block)),
                Job::Status => None,
            })
            .collect()
    }

    #[test]
    fn trimming_keeps_what_is_wanted_and_other_searches() {
        let mut jobs = vec![
            block(5, 0),
            block(5, 40),
            Job::Status,
            block(5, 900),
            block(6, 40),
            block(5, 41),
        ];
        let dropped = trim_jobs(&mut jobs, 5, |b| b == 0 || (40..=41).contains(&b));
        assert_eq!(dropped, [900]);
        assert_eq!(queued(&jobs), [(5, 0), (5, 40), (6, 40), (5, 41)]);
        assert!(jobs.iter().any(|j| matches!(j, Job::Status)));
        // Another search's blocks are its own business.
        assert_eq!(trim_jobs(&mut jobs, 7, |_| false), Vec::<u32>::new());
        assert_eq!(jobs.len(), 5);
    }

    #[test]
    fn the_probe_tells_the_syntax_apart() {
        use goz_core::proto::{ProtoError, QueryResults};
        let answered = Response::Results(QueryResults {
            total: 0,
            items: vec![],
            more: false,
            volumes_incomplete: false,
            metadata_pending: false,
            generation: 0,
            total_size: None,
        });
        assert_eq!(probe_verdict(&answered), Some(true));
        let refused = Response::Error {
            code: ProtoError::BadQuery,
            message: "'|' is not supported".into(),
        };
        assert_eq!(probe_verdict(&refused), Some(false));
        let hello = Response::Hello {
            proto: PROTO_VERSION,
            server: "gozd".into(),
            ready: true,
        };
        assert_eq!(probe_verdict(&hello), None);
        // The probe is what an older gozd refuses: goz-core v0.6.0's parser,
        // the one an old gozd runs, rejects it.
        assert!(goz_core::query::parse_query(PROBE_QUERY).is_err());
        // Learning the same thing twice is no news.
        let before = boolean_syntax();
        assert!(set_boolean_syntax(!before));
        assert!(!set_boolean_syntax(!before));
        set_boolean_syntax(before);
    }
}
