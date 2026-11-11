//! Wave 9, stream B2: what goz-gui asks of a gozd that speaks the new
//! protocol, gated on what its `Hello.features` lists, so that a gozd that
//! lists nothing (v0.6.0 and older) is asked exactly what it was before.
//!
//! - **Columns** (`"columns"`): a search asks gozd for Date Created, Date
//!   Accessed, Attributes and Date Recently Changed when they are shown
//!   (`want_ctime` and so on); without it the disk is read for the rows
//!   drawn (`crate::props`), as before.
//! - **Sorts** (`"sort:extension"`, `"sort:date_created"`, ...): a sort by
//!   one of the new keys goes to gozd only when it lists the key; otherwise
//!   gozd is asked by name, as an older gozd is always asked.
//! - **Push** (`"watch"`): one connection of its own, on which gozd says
//!   which volumes changed ([`Watcher`]); the window then refreshes instead
//!   of polling every 2 s, and polls again on any failure of it.
//! - **Index settings** (`"config"`): [`get_config`] and [`set_config`],
//!   for Options > Indexes.

use std::sync::Mutex;

use goz_core::proto::{
    ConfigChange, IndexConfig, PROTO_VERSION, QueryRequest, Request, Response, feature,
};
use goz_core::types::{SortKey, SortSpec};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::{Failure, Io, Pipe, Shared, Stop};
use crate::props::Want;

/// What the gozd last greeted listed in `Hello.features`; empty until a
/// connection greeted it, and from a gozd predating the field.
static FEATURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What the gozd last greeted lists in `Hello.features`.
pub fn features() -> Vec<String> {
    FEATURES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the gozd last greeted lists `name`.
pub fn has_feature(name: &str) -> bool {
    FEATURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|f| f == name)
}

/// Records a greeting's features; `true` when they changed.
pub(super) fn set_features(new: Vec<String>) -> bool {
    let mut f = FEATURES.lock().unwrap_or_else(|e| e.into_inner());
    if *f == new {
        return false;
    }
    *f = new;
    true
}

/// The feature a sort by `key` needs, `None` for the four every gozd reads.
pub fn sort_feature(key: SortKey) -> Option<&'static str> {
    Some(match key {
        SortKey::Name | SortKey::Path | SortKey::Size | SortKey::DateModified => return None,
        SortKey::Extension => feature::SORT_EXTENSION,
        SortKey::DateCreated => feature::SORT_DATE_CREATED,
        SortKey::DateAccessed => feature::SORT_DATE_ACCESSED,
        SortKey::Attributes => feature::SORT_ATTRIBUTES,
        SortKey::DateRecentlyChanged => feature::SORT_DATE_RECENTLY_CHANGED,
    })
}

fn lists(features: &[String], name: &str) -> bool {
    features.iter().any(|f| f == name)
}

/// The sort gozd is asked for: `sort`, or by name when `features` does not
/// list its key (a sort kept from a newer gozd, sent to an older one, which
/// would refuse the whole request).
pub fn wire_sort(sort: SortSpec, features: &[String]) -> SortSpec {
    match sort_feature(sort.key) {
        Some(name) if !lists(features, name) => SortSpec::default(),
        _ => sort,
    }
}

/// The columns gozd is asked for: those shown (`want`), when `features`
/// lists "columns" and the rows' details are asked at all; none otherwise
/// (the disk is read for them instead, `crate::props`).
pub fn wire_want(want: Want, details: bool, features: &[String]) -> Want {
    if details && lists(features, feature::COLUMNS) {
        want
    } else {
        Want::default()
    }
}

/// What a search's request carries beyond the query: its sort, which rows,
/// and what of them.
pub(super) struct Ask {
    pub sort: SortSpec,
    pub offset: u32,
    pub limit: Option<u32>,
    /// Sizes and dates modified.
    pub details: bool,
    pub match_case: bool,
    pub total_size: bool,
    /// The wave 9 columns shown.
    pub want: Want,
}

/// The query request for `query` as `ask` says, gated on `features`: a gozd
/// that lists none of them gets exactly v0.6.0's request.
pub(super) fn query_request(query: String, ask: &Ask, features: &[String]) -> Request {
    let want = wire_want(ask.want, ask.details, features);
    Request::Query(QueryRequest {
        query,
        scope: None,
        sort: wire_sort(ask.sort, features),
        offset: ask.offset,
        limit: ask.limit,
        want_size: ask.details,
        want_mtime: ask.details,
        match_case: ask.match_case,
        want_total_size: ask.total_size,
        want_ctime: want.ctime,
        want_atime: want.atime,
        want_attrib: want.attrib,
        want_rtime: want.rtime,
    })
}

/// A connection of its own, greeted; the features its gozd lists.
fn greeted(shared: &Shared) -> Result<(Pipe, Vec<String>), Failure> {
    let mut pipe = Pipe::open()?;
    pipe.send(&Request::Hello {
        proto_min: PROTO_VERSION,
        proto_max: PROTO_VERSION,
        client: concat!("goz-gui ", env!("CARGO_PKG_VERSION")).into(),
    })
    .map_err(Io::into_failure)?;
    match pipe.response(shared, None).map_err(Io::into_failure)? {
        Response::Hello { features, .. } => Ok((pipe, features)),
        Response::Error { message, .. } => Err(Failure::Other(format!(
            "gozd refused this client: {message}"
        ))),
        _ => Err(Failure::Other("unexpected reply from gozd".into())),
    }
}

/// Sends `request` on a connection of its own and reads gozd's settings
/// back.
fn config_request(request: &Request) -> Result<IndexConfig, String> {
    let stop = Stop::new();
    let (mut pipe, features) = greeted(&stop.0).map_err(|f| failure_text(&f))?;
    if !lists(&features, feature::CONFIG) {
        return Err("this gozd does not list the \"config\" feature".into());
    }
    pipe.send(request).map_err(|e| io_text(&e))?;
    match pipe.response(&stop.0, None).map_err(|e| io_text(&e))? {
        Response::Config(config) => Ok(config),
        Response::Error { message, .. } => Err(message),
        _ => Err("unexpected reply from gozd".into()),
    }
}

/// gozd's index settings (`GetConfig`).
pub fn get_config() -> Result<IndexConfig, String> {
    config_request(&Request::GetConfig)
}

/// Changes gozd's index settings (`SetConfig`): the settings as they now
/// are, or gozd's refusal ("needs administrator" for a caller that is not
/// an elevated administrator).
pub fn set_config(change: ConfigChange) -> Result<IndexConfig, String> {
    config_request(&Request::SetConfig(change))
}

fn failure_text(f: &Failure) -> String {
    match f {
        Failure::NotRunning => "gozd is not running".into(),
        Failure::Untrusted => "the pipe's owner is not the goz service".into(),
        Failure::Refused(m) | Failure::Other(m) => m.clone(),
    }
}

fn io_text(e: &Io) -> String {
    match e {
        Io::Cancelled => "cancelled".into(),
        Io::Lost(m) => m.clone(),
    }
}

// ---- Push: one Watch connection ----

/// What a [`Watcher`] tells its window (`wParam`; no payload).
pub const WATCH_ON: usize = 1;
/// gozd says a watched volume changed: the results may be stale.
pub const WATCH_CHANGED: usize = 2;
/// The watch ended (gozd went away, refused it, or lists no "watch"):
/// polling takes over.
pub const WATCH_OFF: usize = 3;

/// One `Request::Watch` connection on a thread of its own, telling a window
/// what it hears. Stopped (the pipe closed, which ends the watch in gozd)
/// when dropped.
pub struct Watcher {
    stop: Stop,
}

impl Watcher {
    /// Watches every volume, frames no closer than `min_interval_ms`; news
    /// goes to `hwnd` as `msg` with [`WATCH_ON`], [`WATCH_CHANGED`] or
    /// [`WATCH_OFF`] in `wParam`. `None` when no thread could be started.
    pub fn spawn(hwnd: HWND, msg: u32, min_interval_ms: u32) -> Option<Watcher> {
        let stop = Stop::new();
        let thread = stop.clone();
        let hwnd = hwnd as usize;
        let tell = move |what: usize| {
            // SAFETY: a plain post (no payload) to the window that started
            // the watch; a window gone since just drops it.
            unsafe { PostMessageW(hwnd as HWND, msg, what, 0) };
        };
        std::thread::Builder::new()
            .name("goz-watch".into())
            .spawn(move || {
                watch(&thread, min_interval_ms, &mut |news| {
                    if !thread.stopped() {
                        tell(news);
                    }
                });
                // However it ended (gozd refused it, went away, or lists
                // no "watch"), polling takes over.
                if !thread.stopped() {
                    tell(WATCH_OFF);
                }
            })
            .ok()?;
        Some(Watcher { stop })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// How a watch ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Ended {
    Stopped,
    /// The gozd lists no "watch".
    Unsupported,
    Failed(String),
}

/// Runs one watch until it is stopped or fails, telling `tell` what it
/// hears.
fn watch(stop: &Stop, min_interval_ms: u32, tell: &mut dyn FnMut(usize)) -> Ended {
    let shared = &*stop.0;
    let (mut pipe, features) = match greeted(shared) {
        Ok(g) => g,
        Err(_) if stop.stopped() => return Ended::Stopped,
        Err(f) => return Ended::Failed(failure_text(&f)),
    };
    if !lists(&features, feature::WATCH) {
        return Ended::Unsupported;
    }
    let request = Request::Watch {
        volumes: None,
        min_interval_ms,
    };
    if let Err(e) = pipe.send(&request) {
        return ended(stop, &e);
    }
    // Quiet for as long as nothing changes: no silence limit.
    pipe.silence_ms = INFINITE;
    follow(stop, &mut || pipe.response(shared, None), tell)
}

fn ended(stop: &Stop, e: &Io) -> Ended {
    if stop.stopped() {
        Ended::Stopped
    } else {
        Ended::Failed(io_text(e))
    }
}

/// Reads a watch's frames from `next` until it ends: `Watching` first,
/// then `Changed` frames, each told to `tell`.
pub(super) fn follow(
    stop: &Stop,
    next: &mut dyn FnMut() -> Result<Response, Io>,
    tell: &mut dyn FnMut(usize),
) -> Ended {
    let mut on = false;
    loop {
        match next() {
            Ok(Response::Watching) if !on => {
                on = true;
                tell(WATCH_ON);
            }
            Ok(Response::Changed { .. }) if on => tell(WATCH_CHANGED),
            Ok(Response::Error { message, .. }) => return Ended::Failed(message),
            Ok(_) => return Ended::Failed("unexpected reply from gozd to Watch".into()),
            Err(e) => return ended(stop, &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::proto::{ProtoError, VolumeGen};
    use goz_core::types::SortDir;

    fn f(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    const ALL_WANT: Want = Want {
        ctime: true,
        atime: true,
        attrib: true,
        rtime: true,
    };

    fn ask(sort: SortSpec, want: Want) -> Ask {
        Ask {
            sort,
            offset: 0,
            limit: Some(10),
            details: true,
            match_case: false,
            total_size: true,
            want,
        }
    }

    #[test]
    fn an_old_gozd_is_asked_v060s_request() {
        // v0.6.0 lists nothing: no column flags, no new sort key.
        for key in [
            SortKey::Extension,
            SortKey::DateCreated,
            SortKey::DateAccessed,
            SortKey::Attributes,
            SortKey::DateRecentlyChanged,
        ] {
            let sort = SortSpec {
                key,
                dir: SortDir::Desc,
            };
            let Request::Query(q) = query_request("x".into(), &ask(sort, ALL_WANT), &[]) else {
                panic!("a query");
            };
            assert_eq!(q.sort, SortSpec::default(), "{key:?}");
            assert!(!q.want_ctime && !q.want_atime && !q.want_attrib && !q.want_rtime);
            // Serialized, it is v0.6.0's request: no new field on the wire.
            let mut frame = Vec::new();
            goz_core::proto::encode_request(&Request::Query(q), &mut frame);
            let json = String::from_utf8_lossy(&frame);
            assert!(json.contains("\"query\":\"x\""), "{json}");
            assert!(
                !json.contains("want_ctime") && !json.contains("Extension"),
                "{json}"
            );
        }
        // The four keys every gozd reads go as they are.
        let size = SortSpec::default_for(SortKey::Size);
        assert_eq!(wire_sort(size, &[]), size);
    }

    #[test]
    fn each_sort_key_needs_its_own_feature() {
        let pairs = [
            (SortKey::Extension, "sort:extension"),
            (SortKey::DateCreated, "sort:date_created"),
            (SortKey::DateAccessed, "sort:date_accessed"),
            (SortKey::Attributes, "sort:attributes"),
            (SortKey::DateRecentlyChanged, "sort:date_recently_changed"),
        ];
        for (key, name) in pairs {
            assert_eq!(sort_feature(key), Some(name));
            let sort = SortSpec::default_for(key);
            assert_eq!(wire_sort(sort, &f(&[name])), sort, "{name}");
            // Another key's feature is not this one's.
            let others: Vec<&str> = pairs.iter().map(|p| p.1).filter(|&n| n != name).collect();
            assert_eq!(wire_sort(sort, &f(&others)), SortSpec::default(), "{name}");
        }
    }

    #[test]
    fn columns_are_asked_only_of_a_gozd_with_them() {
        let some = Want {
            ctime: true,
            attrib: true,
            ..Want::default()
        };
        let features = f(&["columns"]);
        assert_eq!(wire_want(some, true, &features), some);
        assert_eq!(wire_want(some, true, &f(&["watch"])), Want::default());
        // A request for paths alone asks no column.
        assert_eq!(wire_want(some, false, &features), Want::default());
        let Request::Query(q) =
            query_request("x".into(), &ask(SortSpec::default(), some), &features)
        else {
            panic!("a query");
        };
        assert!(q.want_ctime && q.want_attrib && !q.want_atime && !q.want_rtime);
    }

    #[test]
    fn features_are_news_only_when_they_change() {
        let before = features();
        set_features(f(&["watch", "columns"]));
        assert!(has_feature("watch") && has_feature("columns") && !has_feature("config"));
        assert!(!set_features(f(&["watch", "columns"])));
        assert!(set_features(f(&[])));
        assert!(!has_feature("watch"));
        set_features(before);
    }

    /// Plays `frames` to [`follow`]; what it told, and how it ended.
    fn play(frames: Vec<Result<Response, Io>>, stopped: bool) -> (Vec<usize>, Ended) {
        let stop = Stop::new();
        if stopped {
            stop.stop();
        }
        let mut frames = frames.into_iter();
        let mut told = Vec::new();
        let end = follow(
            &stop,
            &mut || frames.next().unwrap_or(Err(Io::Lost("closed".into()))),
            &mut |n| told.push(n),
        );
        (told, end)
    }

    fn changed() -> Response {
        Response::Changed {
            volumes: vec![VolumeGen {
                mount: "C:\\".into(),
                generation: 7,
            }],
        }
    }

    #[test]
    fn a_watch_tells_its_frames_and_falls_back_on_any_failure() {
        // On, two changes, then the pipe breaks: off (the window polls).
        let (told, end) = play(
            vec![Ok(Response::Watching), Ok(changed()), Ok(changed())],
            false,
        );
        assert_eq!(told, [WATCH_ON, WATCH_CHANGED, WATCH_CHANGED]);
        assert!(matches!(end, Ended::Failed(_)));
        // gozd refuses the watch (its caps): failed, never on.
        let refused = Response::Error {
            code: ProtoError::Unsupported,
            message: "too many watchers".into(),
        };
        let (told, end) = play(vec![Ok(refused)], false);
        assert!(told.is_empty());
        assert_eq!(end, Ended::Failed("too many watchers".into()));
        // A frame out of turn: a Changed before Watching, a page of results.
        let (told, end) = play(vec![Ok(changed())], false);
        assert!(told.is_empty() && matches!(end, Ended::Failed(_)));
        let (told, end) = play(vec![Ok(Response::Watching), Ok(Response::Watching)], false);
        assert_eq!(told, [WATCH_ON]);
        assert!(matches!(end, Ended::Failed(_)));
        // Stopped by the window: ended quietly, whatever the pipe said.
        let (_, end) = play(vec![Ok(Response::Watching)], true);
        assert_eq!(end, Ended::Stopped);
    }
}
