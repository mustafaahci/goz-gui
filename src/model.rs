//! The rows on screen: a bounded cache of result blocks for the search being
//! shown, and the search waiting to replace it.
//!
//! The list view is virtual (owner data), so it asks for rows by index as it
//! draws them. Rows live in blocks of `BLOCK_ROWS`; only the blocks scrolled
//! past recently stay, so memory holds a few thousand rows whatever the match
//! count.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use crate::client::{BLOCK_ROWS, Failure, Row, Search};
use crate::highlight::Highlight;
use crate::icons::UNSET;

/// Blocks kept at most: about 4k rows, a little over 1 MB of long paths
/// (measured: some 270 bytes a row). The screen needs one or two; the rest
/// spare a fetch when scrolling back. Held rows cost memory for as long as
/// the result is shown, and the idle budget is about 8 MB for the whole
/// process, shell and icon state included.
const MAX_BLOCKS: usize = 4;

struct Block {
    rows: Vec<Row>,
    used: Cell<u64>,
}

/// A search sent to gozd whose first block has not come back yet. The old
/// rows stay on screen until it does, so typing never flashes an empty list.
pub struct Pending {
    pub epoch: u64,
    pub search: Arc<Search>,
    pub highlight: Highlight,
    /// A refresh of the same search: keep the scroll position, and the
    /// selected files (by path, as rows move: `window::reselect`).
    pub keep_place: bool,
}

pub struct Model {
    /// The generation of the rows on screen.
    pub shown: u64,
    pub search: Arc<Search>,
    pub highlight: Highlight,
    pub pending: Option<Pending>,
    pub total: u64,
    pub incomplete: bool,
    pub metadata_pending: bool,
    /// The summed size of every matching file, once gozd said.
    pub total_size: Option<u64>,
    pub elapsed: Option<Duration>,
    pub failure: Option<(Failure, String)>,
    /// Why gozd refused a block after the first (its time budget ran out,
    /// say). Those rows stay empty, and are not asked for again, until the
    /// next search or refresh.
    pub partial_failure: Option<String>,
    blocks: HashMap<u32, Block>,
    requested: HashSet<u32>,
    /// Blocks of a selection an action waits for (`window::results`): never
    /// dropped to make room while pinned, even past `MAX_BLOCKS`.
    pinned: HashSet<u32>,
    /// The blocks of this search dropped to make room, with a fingerprint
    /// of the files they held, row by row: gozd answers a block fetched
    /// again with whatever is at those rows now, which a file added or
    /// removed above them moves.
    dropped: HashMap<u32, u64>,
    tick: Cell<u64>,
}

/// The files of a block's rows, in order, as one number: two blocks with
/// the same fingerprint hold the same files in the same rows.
fn fingerprint(rows: &[Row]) -> u64 {
    let mut h = DefaultHasher::new();
    rows.len().hash(&mut h);
    for row in rows {
        row.path.hash(&mut h);
    }
    h.finish()
}

impl Model {
    pub fn new(search: Arc<Search>) -> Model {
        Model {
            shown: 0,
            search,
            highlight: Highlight::default(),
            pending: None,
            total: 0,
            incomplete: false,
            metadata_pending: false,
            total_size: None,
            elapsed: None,
            failure: None,
            partial_failure: None,
            blocks: HashMap::new(),
            requested: HashSet::new(),
            pinned: HashSet::new(),
            dropped: HashMap::new(),
            tick: Cell::new(0),
        }
    }

    /// Shows `pending`'s search from now on, with no rows yet.
    pub fn adopt(&mut self, pending: Pending) {
        self.shown = pending.epoch;
        self.search = pending.search;
        self.highlight = pending.highlight;
        self.blocks.clear();
        self.requested.clear();
        self.pinned.clear();
        self.dropped.clear();
        self.total = 0;
        self.incomplete = false;
        self.metadata_pending = false;
        self.total_size = None;
        self.failure = None;
        self.partial_failure = None;
    }

    /// Row `index`, if held; its block counts as just used.
    pub fn row(&self, index: usize) -> Option<&Row> {
        let block = self.blocks.get(&((index / BLOCK_ROWS as usize) as u32))?;
        let tick = self.tick.get() + 1;
        self.tick.set(tick);
        block.used.set(tick);
        block.rows.get(index % BLOCK_ROWS as usize)
    }

    /// Row `index`, if held, without counting as a use: for bookkeeping
    /// that is not the user looking at the row.
    pub fn peek(&self, index: usize) -> Option<&Row> {
        let block = self.blocks.get(&((index / BLOCK_ROWS as usize) as u32))?;
        block.rows.get(index % BLOCK_ROWS as usize)
    }

    /// The rows of `block` with their indices, if it is held; not a use.
    pub fn block_rows(&self, block: u32) -> impl Iterator<Item = (usize, &Row)> {
        let first = block as usize * BLOCK_ROWS as usize;
        self.blocks
            .get(&block)
            .into_iter()
            .flat_map(|b| b.rows.iter())
            .enumerate()
            .map(move |(i, row)| (first + i, row))
    }

    /// The blocks covering rows `from..=to` that are neither held nor asked
    /// for yet; they are marked as asked for.
    pub fn missing(&mut self, from: usize, to: usize) -> Vec<u32> {
        let last = (self.total.saturating_sub(1) as usize).min(to);
        if self.total == 0 || from > last {
            return Vec::new();
        }
        let first_block = (from / BLOCK_ROWS as usize) as u32;
        let last_block = (last / BLOCK_ROWS as usize) as u32;
        self.request(first_block..=last_block)
    }

    /// Of `blocks`, those neither held nor asked for yet; they are marked
    /// as asked for.
    pub fn request(&mut self, blocks: impl IntoIterator<Item = u32>) -> Vec<u32> {
        blocks
            .into_iter()
            .filter(|b| !self.blocks.contains_key(b) && self.requested.insert(*b))
            .collect()
    }

    /// Whether `block` is held.
    pub fn holds(&self, block: u32) -> bool {
        self.blocks.contains_key(&block)
    }

    /// Keeps exactly `blocks` from being dropped to make room (none: the
    /// cache shrinks back to its size as the next blocks come).
    pub fn set_pins(&mut self, blocks: impl IntoIterator<Item = u32>) {
        self.pinned = blocks.into_iter().collect();
    }

    /// An action is to wait for `blocks` (each block with a selected row):
    /// unless all are held (`None`, nothing changes), they are kept from
    /// now on, in place of the blocks kept for an action before; the ones
    /// not held are given, and of those the ones to ask gozd for. Nothing
    /// is dropped here, so no block counted as held goes before it is
    /// kept; the caller lets the cache shrink back after
    /// ([`make_room`](Self::make_room)).
    pub fn pin_for(&mut self, blocks: &[u32]) -> Option<(BTreeSet<u32>, Vec<u32>)> {
        let waiting: BTreeSet<u32> = blocks.iter().copied().filter(|&b| !self.holds(b)).collect();
        if waiting.is_empty() {
            return None;
        }
        self.set_pins(blocks.iter().copied());
        let missing = self.request(waiting.iter().copied());
        Some((waiting, missing))
    }

    /// Whether `block` is kept from being dropped.
    pub fn pinned(&self, block: u32) -> bool {
        self.pinned.contains(&block)
    }

    /// The row ranges held, as `start..end`, in order.
    pub fn loaded(&self) -> Vec<core::ops::Range<usize>> {
        let mut out: Vec<_> = self
            .blocks
            .iter()
            .map(|(&b, block)| {
                let start = b as usize * BLOCK_ROWS as usize;
                start..start + block.rows.len()
            })
            .collect();
        out.sort_by_key(|r| r.start);
        out
    }

    /// The summed size of the files in the result when every row of it is
    /// held, which is what gozd's `total_size` says, for a gozd that
    /// predates it. Folders and sizes not read yet add nothing, as there.
    pub fn held_total_size(&self) -> Option<u64> {
        let held: usize = self.blocks.values().map(|b| b.rows.len()).sum();
        if self.total == 0 || held as u64 != self.total {
            return None;
        }
        let sizes = self.blocks.values().flat_map(|b| &b.rows);
        Some(
            sizes
                .filter(|r| !r.is_dir)
                .filter_map(|r| r.size)
                .fold(0, u64::saturating_add),
        )
    }

    /// The index of the held row of the file at `path`.
    pub fn find(&self, path: &[u16]) -> Option<usize> {
        self.blocks.iter().find_map(|(&b, block)| {
            let i = block.rows.iter().position(|r| *r.path == *path)?;
            Some(b as usize * BLOCK_ROWS as usize + i)
        })
    }

    /// The file or folder at `old` was renamed to `new` (a full path): its
    /// held row shows the new path and name, and looks its icon up again
    /// (the extension may have changed); the held rows of what a renamed
    /// folder holds show their new paths too (their names are the same).
    /// The indices of the rows changed, in no particular order.
    pub fn rename(&mut self, old: &[u16], new: &[u16]) -> Vec<usize> {
        const BACKSLASH: u16 = b'\\' as u16;
        let mut changed = Vec::new();
        for (&b, block) in &mut self.blocks {
            for (i, row) in block.rows.iter_mut().enumerate() {
                let path = &row.path;
                if **path == *old {
                    row.path = new.into();
                    row.name_start = new
                        .iter()
                        .rposition(|&c| c == BACKSLASH)
                        .map_or(0, |i| i + 1) as u32;
                    row.icon.set(UNSET);
                } else if path.len() > old.len()
                    && path.starts_with(old)
                    && path[old.len()] == BACKSLASH
                {
                    let mut moved = new.to_vec();
                    moved.extend_from_slice(&path[old.len()..]);
                    // Its name starts after the folder's backslash, so past
                    // `old`: it moves with the folder's new length.
                    row.name_start = (row.name_start as usize - old.len() + new.len()) as u32;
                    row.path = moved.into();
                } else {
                    continue;
                }
                changed.push(b as usize * BLOCK_ROWS as usize + i);
            }
        }
        changed
    }

    /// A block asked for will not come: it may be asked for again.
    pub fn forget_request(&mut self, block: u32) {
        self.requested.remove(&block);
    }

    /// Holds `rows` as `block`, making room by dropping the block used
    /// longest ago. The blocks of `on_screen` are never the ones dropped: a
    /// block that arrives late (scrolled past) would otherwise push out the
    /// rows in view, which nothing repaints and so nothing marks as used.
    /// Pinned blocks are never dropped, and do not count: while there are
    /// some, the cache holds them and `MAX_BLOCKS` others.
    ///
    /// True when `block` was held before, dropped, and now holds other
    /// files in its rows: the result changed meanwhile, and the list's
    /// selection on those rows (row numbers) is not on the files chosen.
    pub fn insert(&mut self, block: u32, mut rows: Vec<Row>, on_screen: Range<u32>) -> bool {
        self.requested.remove(&block);
        let moved = self
            .dropped
            .remove(&block)
            .is_some_and(|f| f != fingerprint(&rows));
        rows.shrink_to_fit();
        let tick = self.tick.get() + 1;
        self.tick.set(tick);
        self.blocks.insert(
            block,
            Block {
                rows,
                used: Cell::new(tick),
            },
        );
        self.make_room(on_screen);
        moved
    }

    /// Drops the blocks used longest ago, as [`insert`](Self::insert) says,
    /// until the cache is back to its size.
    pub fn make_room(&mut self, on_screen: Range<u32>) {
        // Pinned blocks do not count: the others keep their room.
        let pinned = self
            .blocks
            .keys()
            .filter(|k| self.pinned.contains(k))
            .count();
        while self.blocks.len() > MAX_BLOCKS + pinned {
            let oldest = |spare_screen: bool| {
                self.blocks
                    .iter()
                    .filter(|(k, _)| !self.pinned.contains(k))
                    .filter(|(k, _)| !spare_screen || !on_screen.contains(k))
                    .min_by_key(|(_, b)| b.used.get())
                    .map(|(k, _)| *k)
            };
            let Some(victim) = oldest(true).or_else(|| oldest(false)) else {
                break;
            };
            if let Some(gone) = self.blocks.remove(&victim) {
                self.dropped.insert(victim, fingerprint(&gone.rows));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model::new(Arc::new(Search {
            query: String::new(),
            sort: goz_core::types::SortSpec::default(),
            match_case: false,
        }))
    }

    fn row(size: Option<u64>, is_dir: bool) -> Row {
        Row {
            path: r"C:\a".encode_utf16().collect(),
            name_start: 3,
            size,
            mtime: None,
            is_dir,
            icon: Cell::new(0),
        }
    }

    #[test]
    fn loaded_ranges_are_in_order() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 3;
        m.insert(2, vec![row(Some(1), false), row(Some(2), false)], 0..0);
        m.insert(0, vec![row(Some(1), false)], 0..0);
        let first = BLOCK_ROWS as usize * 2;
        assert_eq!(m.loaded(), [0..1, first..first + 2]);
    }

    #[test]
    fn held_total_size_needs_every_row() {
        let mut m = model();
        m.total = 4;
        m.insert(0, vec![row(Some(100), false), row(None, true)], 0..0);
        assert_eq!(m.held_total_size(), None, "half the rows");
        m.insert(
            0,
            vec![
                row(Some(100), false),
                row(None, true),
                row(None, false),
                row(Some(23), false),
            ],
            0..0,
        );
        assert_eq!(m.held_total_size(), Some(123));
        m.total = 0;
        m.insert(0, Vec::new(), 0..0);
        assert_eq!(m.held_total_size(), None, "no results");
    }

    #[test]
    fn blocks_on_screen_are_never_dropped() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 100;
        // Block 0 is on screen and came first, so it is the oldest; the
        // blocks arriving after it must push out one another instead.
        m.insert(0, vec![row(None, false)], 0..1);
        for b in 1..=(MAX_BLOCKS as u32 * 3) {
            m.insert(b, vec![row(None, false)], 0..1);
        }
        assert_eq!(m.blocks.len(), MAX_BLOCKS);
        assert!(m.peek(0).is_some(), "the block on screen stays");
        // Once off screen, the oldest goes as before.
        m.insert(1000, vec![row(None, false)], 999..1001);
        assert!(m.peek(0).is_none());
    }

    #[test]
    fn pinned_blocks_stay_past_the_cache_size() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 100;
        let pins: Vec<u32> = (10..10 + MAX_BLOCKS as u32 * 3).collect();
        m.set_pins(pins.iter().copied());
        assert!(m.pinned(10) && !m.pinned(0));
        // Asked for once: held or asked blocks are not asked again.
        assert_eq!(m.request(pins.iter().copied()), pins);
        assert!(m.request(pins.iter().copied()).is_empty());
        for &b in &pins {
            m.insert(b, vec![row(None, false)], 0..1);
        }
        // Blocks that are not pinned come and go round them.
        for b in 50..60 {
            m.insert(b, vec![row(None, false)], 0..1);
        }
        assert!(pins.iter().all(|&b| m.holds(b)), "every pinned block held");
        assert_eq!(m.blocks.len(), pins.len() + MAX_BLOCKS);
        assert!((56..60).all(|b| m.holds(b)), "the newest others");
        // Unpinned, the cache shrinks back with the next block.
        m.set_pins([]);
        m.insert(70, vec![row(None, false)], 0..1);
        assert_eq!(m.blocks.len(), MAX_BLOCKS);
        assert!(m.holds(70));
        // A new search forgets the pins.
        m.set_pins([3]);
        m.adopt(Pending {
            epoch: 2,
            search: Arc::clone(&m.search),
            highlight: Highlight::default(),
            keep_place: false,
        });
        assert!(!m.pinned(3));
    }

    #[test]
    fn a_replacing_action_keeps_every_block_it_counts_as_held() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 100;
        // An action waits: its blocks 10-21 are here and kept, with the
        // cache's own four, so more than MAX_BLOCKS are held.
        let first: Vec<u32> = (10..22).collect();
        assert!(m.pin_for(&first).is_some());
        for &b in &first {
            m.insert(b, vec![row(None, false)], 0..1);
        }
        for b in 50..54 {
            m.insert(b, vec![row(None, false)], 0..1);
        }
        assert_eq!(m.blocks.len(), first.len() + MAX_BLOCKS);
        // All held: nothing to wait for, and the pins stay as they were.
        assert_eq!(m.pin_for(&[10, 50]), None);
        assert!(m.pinned(21) && !m.pinned(50));
        // Another action replaces it before block 30 has come: every block
        // with a selected row is held (and kept) or waited for, also after
        // the cache shrinks back.
        let second: Vec<u32> = (12..22).chain([30]).collect();
        let (waiting, missing) = m.pin_for(&second).unwrap();
        assert_eq!(waiting.into_iter().collect::<Vec<_>>(), [30]);
        assert_eq!(missing, [30]);
        m.make_room(0..1);
        assert!((12..22).all(|b| m.holds(b)), "none counted as held dropped");
        assert!(!m.pinned(10) && !m.holds(10), "the first's own block goes");
        assert_eq!(m.blocks.len(), 10 + MAX_BLOCKS);
    }

    #[test]
    fn a_block_fetched_again_says_whether_its_files_moved() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 100;
        let rows = |names: &[&str]| -> Vec<Row> {
            names
                .iter()
                .map(|n| {
                    let mut r = row(None, false);
                    r.path = n.encode_utf16().collect::<Vec<u16>>().into();
                    r
                })
                .collect()
        };
        let drop_block_0 = |m: &mut Model| {
            for b in 1..=MAX_BLOCKS as u32 {
                m.insert(b, rows(&["x"]), 99..100);
            }
            assert!(!m.holds(0));
        };
        assert!(!m.insert(0, rows(&[r"C:", r"C:"]), 0..1), "never held");
        drop_block_0(&mut m);
        // The same files in the same rows: nothing moved.
        assert!(!m.insert(0, rows(&[r"C:", r"C:"]), 99..100));
        drop_block_0(&mut m);
        // A file came in above: the rows hold other files now.
        assert!(m.insert(0, rows(&[r"C: ", r"C:"]), 99..100));
        drop_block_0(&mut m);
        // Compared with what it held last.
        assert!(!m.insert(0, rows(&[r"C: ", r"C:"]), 99..100));
        drop_block_0(&mut m);
        // A new search (or a refresh) starts afresh.
        m.adopt(Pending {
            epoch: 2,
            search: Arc::clone(&m.search),
            highlight: Highlight::default(),
            keep_place: true,
        });
        m.total = u64::from(BLOCK_ROWS) * 100;
        assert!(!m.insert(0, rows(&[r"C:\z"]), 0..1));
    }

    #[test]
    fn a_refused_block_is_not_asked_for_again_until_the_next_search() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 10;
        assert_eq!(m.missing(5000, 5000), [4]);
        // gozd refused block 4: the window leaves it asked for and says why.
        m.partial_failure = Some("stopped after 10 s".into());
        assert_eq!(m.missing(5000, 5000), Vec::<u32>::new(), "every repaint");
        m.adopt(Pending {
            epoch: 2,
            search: Arc::clone(&m.search),
            highlight: Highlight::default(),
            keep_place: true,
        });
        assert_eq!(m.partial_failure, None);
        m.total = u64::from(BLOCK_ROWS) * 10;
        assert_eq!(m.missing(5000, 5000), [4], "a refresh asks anew");
    }

    #[test]
    fn a_renamed_file_keeps_its_row() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 2;
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let mut b = row(None, false);
        b.path = w(r"C:\dir\b.txt").into();
        b.name_start = 7;
        m.insert(1, vec![row(None, false), b], 0..0);
        let at = BLOCK_ROWS as usize + 1;
        assert_eq!(m.find(&w(r"C:\dir\b.txt")), Some(at));
        assert_eq!(m.rename(&w(r"C:\dir\b.txt"), &w(r"C:\dir\c.md")), [at]);
        let row = m.peek(at).unwrap();
        assert_eq!(String::from_utf16_lossy(row.name()), "c.md");
        assert_eq!(String::from_utf16_lossy(row.parent()), r"C:\dir");
        assert_eq!(row.icon.get(), UNSET, "looked up again");
        assert_eq!(m.find(&w(r"C:\dir\b.txt")), None);
        assert!(m.rename(&w(r"C:\x"), &w(r"C:\y")).is_empty(), "not held");
    }

    #[test]
    fn a_renamed_folder_takes_what_it_holds_along() {
        let mut m = model();
        m.total = 5;
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let at = |path: &str| {
            let mut r = row(None, false);
            r.name_start = path.rfind('\\').map_or(0, |i| i + 1) as u32;
            r.path = w(path).into();
            r
        };
        m.insert(
            0,
            vec![
                at(r"C:\proj"),
                at(r"C:\proj\a.txt"),
                at(r"C:\proj\sub\b.txt"),
                at(r"C:\project\c.txt"),
                at(r"C:\x\proj"),
            ],
            0..1,
        );
        let mut changed = m.rename(&w(r"C:\proj"), &w(r"C:\proj-old"));
        changed.sort_unstable();
        assert_eq!(changed, [0, 1, 2], "not C:\\project, not C:\\x\\proj");
        let show = |i: usize| {
            let r = m.peek(i).unwrap();
            (
                String::from_utf16_lossy(r.parent()),
                String::from_utf16_lossy(r.name()),
            )
        };
        assert_eq!(show(0), (r"C:".into(), "proj-old".into()));
        assert_eq!(show(1), (r"C:\proj-old".into(), "a.txt".into()));
        assert_eq!(show(2), (r"C:\proj-old\sub".into(), "b.txt".into()));
        assert_eq!(show(3), (r"C:\project".into(), "c.txt".into()));
        assert_eq!(show(4), (r"C:\x".into(), "proj".into()));
    }

    #[test]
    fn peeking_is_not_a_use() {
        let mut m = model();
        m.total = u64::from(BLOCK_ROWS) * 100;
        for b in 0..MAX_BLOCKS as u32 {
            m.insert(b, vec![row(None, false)], 0..0);
        }
        // Peeking at block 0 leaves it the oldest; reading block 1's row
        // makes block 1 the newest.
        let first = BLOCK_ROWS as usize;
        assert!(m.peek(0).is_some());
        assert!(m.row(first).is_some());
        assert_eq!(m.block_rows(1).map(|(i, _)| i).collect::<Vec<_>>(), [first]);
        assert_eq!(m.block_rows(70).count(), 0, "not held");
        m.insert(50, vec![row(None, false)], 0..0);
        assert!(m.peek(0).is_none());
        assert!(m.peek(first).is_some());
    }
}
