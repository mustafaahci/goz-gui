//! Edit > Find (the find bar): what a find looks for in the results'
//! names, and the order it goes through the rows in.
//!
//! A find matches a result's name, not its path: the text anywhere in the
//! name, or, holding `*` or `?`, the whole name as a wildcard pattern (as
//! goz's own wildcards do). Case does not matter unless Match Case is on.
//!
//! The rows a find looks at are planned as parts ([`plan`]): from the
//! focused row to the end (or the top), then, wrapping round, the rest.
//! The rows the window holds are looked at first, on the window's thread
//! ([`walk`]); the parts left over are fetched from gozd on a thread of
//! their own (`window::findbar`), the parts going backwards in chunks that
//! grow from the end of the part ([`back_chunks`]).

use crate::highlight::{Span, fold};

/// Everything's messages, shown in the status bar as a find wraps round or
/// finds nothing.
pub const WRAPPED_TO_TOP: &str = "Reached end of results, continued from the top.";
pub const WRAPPED_TO_BOTTOM: &str = "Reached top of results, continued from the bottom.";

/// Everything's "not found" message for `text`.
pub fn not_found(text: &str) -> String {
    format!("The following specified text was not found: {text}")
}

/// One unit of a wildcard pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glob {
    Lit(u16),
    /// `?`: one character (a surrogate pair is one).
    One,
    /// `*`: any run of characters, none included.
    Any,
}

/// What a find looks for, ready to test names against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needle {
    /// The text, folded unless the find matches case.
    text: Vec<u16>,
    case: bool,
    /// The whole-name pattern, when the text holds `*` or `?`.
    glob: Option<Vec<Glob>>,
}

fn fold_unit(c: u16, case: bool) -> u16 {
    if case { c } else { fold(c) }
}

fn is_high(c: u16) -> bool {
    (0xd800..0xdc00).contains(&c)
}

fn is_low(c: u16) -> bool {
    (0xdc00..0xe000).contains(&c)
}

impl Needle {
    /// The needle for `text`; `None` for an empty text, which finds
    /// nothing.
    pub fn new(text: &str, match_case: bool) -> Option<Needle> {
        if text.is_empty() {
            return None;
        }
        let units: Vec<u16> = text
            .encode_utf16()
            .map(|c| fold_unit(c, match_case))
            .collect();
        let glob = text.contains(['*', '?']).then(|| {
            let mut g: Vec<Glob> = Vec::with_capacity(units.len());
            for &c in &units {
                match c {
                    0x2a if g.last() == Some(&Glob::Any) => {}
                    0x2a => g.push(Glob::Any),
                    0x3f => g.push(Glob::One),
                    c => g.push(Glob::Lit(c)),
                }
            }
            g
        });
        Some(Needle {
            text: units,
            case: match_case,
            glob,
        })
    }

    /// Whether `name` is a match.
    pub fn matches(&self, name: &[u16]) -> bool {
        match &self.glob {
            Some(glob) => glob_match(glob, name, self.case),
            None => self.find_from(name, 0).is_some(),
        }
    }

    /// Where the text is in `name` at or after `from`.
    fn find_from(&self, name: &[u16], from: usize) -> Option<usize> {
        let n = self.text.len();
        if n == 0 || name.len() < n {
            return None;
        }
        (from..=name.len() - n).find(|&at| {
            name[at..at + n]
                .iter()
                .zip(&self.text)
                .all(|(&c, &t)| fold_unit(c, self.case) == t)
        })
    }

    /// The parts of `name` to light up (Highlight All): every place the
    /// text is in it; for a pattern that matches the whole name, its
    /// literal pieces where they matched, left to right. Nothing for a
    /// name that is no match.
    pub fn spans(&self, name: &[u16], out: &mut Vec<Span>) {
        out.clear();
        match &self.glob {
            None => {
                let mut at = 0;
                while let Some(found) = self.find_from(name, at) {
                    out.push((found, found + self.text.len()));
                    at = found + self.text.len();
                }
            }
            Some(glob) => {
                if !glob_match(glob, name, self.case) {
                    return;
                }
                // The pieces between the wildcards, each looked for after
                // the one before, as the pattern takes them.
                let mut at = 0;
                for piece in glob.split(|g| !matches!(g, Glob::Lit(_))) {
                    if piece.is_empty() {
                        continue;
                    }
                    let lits: Vec<u16> = piece
                        .iter()
                        .map(|g| match g {
                            Glob::Lit(c) => *c,
                            _ => 0,
                        })
                        .collect();
                    let n = lits.len();
                    let found = (at..=name.len().saturating_sub(n)).find(|&i| {
                        i + n <= name.len()
                            && name[i..i + n]
                                .iter()
                                .zip(&lits)
                                .all(|(&c, &t)| fold_unit(c, self.case) == t)
                    });
                    match found {
                        Some(i) => {
                            out.push((i, i + n));
                            at = i + n;
                        }
                        None => break,
                    }
                }
            }
        }
    }
}

/// Whether the pattern matches all of `name`: `*` any run, `?` one
/// character, the rest itself (folded unless `case`). Backtracks on the
/// last `*` only, which is enough for a whole-string match.
fn glob_match(glob: &[Glob], name: &[u16], case: bool) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    // The width of the character at `t`: a surrogate pair is one.
    let width = |t: usize| {
        if is_high(name[t]) && t + 1 < name.len() && is_low(name[t + 1]) {
            2
        } else {
            1
        }
    };
    while t < name.len() {
        match glob.get(p) {
            Some(Glob::Lit(c)) if *c == fold_unit(name[t], case) => {
                p += 1;
                t += 1;
            }
            Some(Glob::One) => {
                p += 1;
                t += width(t);
            }
            Some(Glob::Any) => {
                star = Some((p + 1, t));
                p += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    let next = st + width(st);
                    star = Some((sp, next));
                    p = sp;
                    t = next;
                }
                None => return false,
            },
        }
    }
    glob[p..].iter().all(|g| *g == Glob::Any)
}

/// A stretch of rows a find goes through in one direction: `start..end`,
/// from `start` up (`forward`) or from `end - 1` down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub start: u64,
    pub end: u64,
    pub forward: bool,
    /// The find went round the end (or the top) to get here.
    pub wrapped: bool,
    /// The row the find started from (its anchor) is this part's own: its
    /// first (a find as the text is typed) or its last (a lone match found
    /// again round the end).
    pub edge: bool,
}

/// Which side of the anchor a part's rows are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// After it: Next's first part, Previous's part round the end.
    After,
    /// Before it: Next's part round the end, Previous's first part.
    Before,
}

impl Part {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Which side of the anchor its rows are on.
    pub fn side(&self) -> Side {
        if self.forward != self.wrapped {
            Side::After
        } else {
            Side::Before
        }
    }

    /// Where the anchor was as the part was planned: the row it starts
    /// counting from (after the anchor), or stops before (before it).
    pub fn planned(&self) -> u64 {
        match self.side() {
            Side::After => self.start,
            Side::Before => self.end,
        }
    }

    /// The `i`-th row it goes through.
    fn at(&self, i: u64) -> u64 {
        if self.forward {
            self.start + i
        } else {
            self.end - 1 - i
        }
    }

    /// The rows after the first `done` of it.
    fn after(&self, done: u64) -> Part {
        if self.forward {
            Part {
                start: self.start + done,
                ..*self
            }
        } else {
            Part {
                end: self.end - done,
                ..*self
            }
        }
    }
}

/// The rows a find goes through among `total`, in order, from `focus`:
/// Next (`forward`) from the row after it, Previous from the row before it,
/// each round to the far end and on from the other end back to the focused
/// row itself (so a lone match is found again, as wrapping onto itself).
/// `inclusive` starts at the focused row instead (a find as the text is
/// typed keeps the row that still matches). Without a focused row, Next
/// goes from the top and Previous from the bottom, with no wrapping.
pub fn plan(total: u64, focus: Option<u64>, forward: bool, inclusive: bool) -> Vec<Part> {
    let part = |start: u64, end: u64, wrapped: bool, edge: bool| Part {
        start,
        end,
        forward,
        wrapped,
        edge,
    };
    let parts = match focus.filter(|&f| f < total) {
        None => vec![part(0, total, false, false)],
        Some(f) if forward => {
            let from = if inclusive { f } else { f + 1 };
            vec![
                part(from, total, false, inclusive),
                part(0, from, true, !inclusive),
            ]
        }
        Some(f) => {
            let to = if inclusive { f + 1 } else { f };
            vec![
                part(0, to, false, inclusive),
                part(to, total, true, !inclusive),
            ]
        }
    };
    parts.into_iter().filter(|p| p.len() > 0).collect()
}

/// What going through the rows held found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Walked {
    Found {
        index: u64,
        wrapped: bool,
    },
    /// A row not held came first: the rows from it on, still to look at.
    Rest(Vec<Part>),
    /// Every row was looked at: no match.
    Exhausted,
}

/// Goes through `parts` in order while the rows are held: `probe(i)` is
/// `None` for a row not held, else whether it matches.
pub fn walk(parts: &[Part], mut probe: impl FnMut(u64) -> Option<bool>) -> Walked {
    for (k, part) in parts.iter().enumerate() {
        for i in 0..part.len() {
            let index = part.at(i);
            match probe(index) {
                Some(true) => {
                    return Walked::Found {
                        index,
                        wrapped: part.wrapped,
                    };
                }
                Some(false) => {}
                None => {
                    let mut rest = vec![part.after(i)];
                    rest.extend_from_slice(&parts[k + 1..]);
                    return Walked::Rest(rest);
                }
            }
        }
    }
    Walked::Exhausted
}

/// How far rows may have moved, either way, between the find's plan and
/// its stream from gozd (files that came or went before the anchor
/// meanwhile, on a busy disk); the stream reads this much past the plan on
/// the anchor's side, to find where the anchor is now.
pub const SLACK: u64 = 4096;

/// A row a find takes: its index and path.
pub type Pick = (u64, Box<[u16]>);

/// Which match of one stream of rows a find takes, given where the anchor
/// (the file the find started from) turns out to be in it: rows may have
/// moved since the part was planned, and the anchor tells which side of it
/// a row is really on. Without the anchor in the stream (none, gone, or
/// moved further than [`SLACK`]), the plan's row numbers decide.
pub struct Gate {
    side: Side,
    edge: bool,
    planned: u64,
    /// The first match is wanted (a forward stream), else the last.
    first: bool,
    anchor: Option<Box<[u16]>>,
    seen: bool,
    /// The pick by the plan's numbers, and by the anchor.
    by_plan: Option<Pick>,
    by_anchor: Option<Pick>,
    /// Decided: stop reading.
    done: Option<Option<Pick>>,
}

impl Gate {
    /// The gate of `part`'s stream; `anchor` the path of the row the find
    /// started from, if it had one. `first`: the first match wanted.
    pub fn new(part: &Part, anchor: Option<&[u16]>, first: bool) -> Gate {
        Gate {
            side: part.side(),
            edge: part.edge,
            planned: part.planned(),
            first,
            anchor: anchor.map(Box::from),
            seen: false,
            by_plan: None,
            by_anchor: None,
            done: None,
        }
    }

    /// A gate that takes every match of its stream: a chunk far from the
    /// anchor.
    pub fn open(first: bool) -> Gate {
        Gate {
            side: Side::After,
            edge: false,
            planned: 0,
            first,
            anchor: None,
            seen: false,
            by_plan: None,
            by_anchor: None,
            done: None,
        }
    }

    fn decide(&mut self, pick: Option<Pick>) -> bool {
        self.done = Some(pick);
        true
    }

    /// Row `index` of the stream (`path`, and whether it `matched`): true
    /// when the gate has decided and the stream can stop.
    pub fn row(&mut self, index: u64, path: &[u16], matched: bool) -> bool {
        if self.done.is_some() {
            return true;
        }
        let is_anchor = !self.seen && self.anchor.as_deref() == Some(path);
        let pick = || (index, Box::<[u16]>::from(path));
        match (self.side, self.first) {
            (Side::After, true) => {
                if is_anchor {
                    // What came before it was before it after all.
                    self.seen = true;
                    self.by_plan = None;
                    if self.edge && matched {
                        return self.decide(Some(pick()));
                    }
                    return false;
                }
                if matched {
                    if self.seen {
                        return self.decide(Some(pick()));
                    }
                    if index >= self.planned && self.by_plan.is_none() {
                        self.by_plan = Some(pick());
                    }
                }
                // No anchor to wait for, or it is gone: the plan decides.
                let waited = self.anchor.is_none() || index >= self.planned.saturating_add(SLACK);
                if !self.seen && waited && self.by_plan.is_some() {
                    let p = self.by_plan.take();
                    return self.decide(p);
                }
                false
            }
            (Side::After, false) => {
                if is_anchor {
                    self.seen = true;
                    if self.edge && matched {
                        self.by_anchor = Some(pick());
                    }
                    return false;
                }
                if matched {
                    if self.seen {
                        self.by_anchor = Some(pick());
                    }
                    if index >= self.planned {
                        self.by_plan = Some(pick());
                    }
                }
                false
            }
            (Side::Before, true) => {
                if is_anchor {
                    // Nothing after it counts; one waiting before it does.
                    self.seen = true;
                    let p = self
                        .by_plan
                        .take()
                        .or_else(|| (self.edge && matched).then(pick));
                    return self.decide(p);
                }
                if matched {
                    if index < self.planned {
                        return self.decide(Some(pick()));
                    }
                    // Past the plan: it counts only if the anchor comes.
                    if self.by_plan.is_none() {
                        self.by_plan = Some(pick());
                    }
                }
                false
            }
            (Side::Before, false) => {
                if is_anchor {
                    self.seen = true;
                    if self.edge && matched {
                        self.by_anchor = Some(pick());
                    }
                    let p = self.by_anchor.take();
                    return self.decide(p);
                }
                if matched {
                    // The last before the anchor, as far as it is seen.
                    self.by_anchor = Some(pick());
                    if index < self.planned {
                        self.by_plan = Some(pick());
                    }
                }
                false
            }
        }
    }

    /// What the gate took, once the stream ended (or it decided).
    pub fn pick(self) -> Option<Pick> {
        if let Some(done) = self.done {
            return done;
        }
        match (self.side, self.first, self.seen) {
            // Forward after the anchor: by the plan once no anchor came.
            (Side::After, true, false) => self.by_plan,
            (Side::After, true, true) => None,
            (Side::After, false, true) => self.by_anchor,
            (Side::After, false, false) => self.by_plan,
            // Before an anchor that never came: by the plan's numbers.
            (Side::Before, true, _) => None,
            (Side::Before, false, true) => self.by_anchor,
            (Side::Before, false, false) => self.by_plan,
        }
    }
}

/// The first chunk a backward part is fetched in, and the most one grows
/// to: the rows nearest the starting point come first, and a long way
/// back costs a few large requests rather than many small ones.
const BACK_FIRST: u64 = 4096;
const BACK_MOST: u64 = 262_144;

/// The chunks a backward part `start..end` is fetched in, nearest first:
/// each a range read in order, whose last match is the one wanted.
pub fn back_chunks(start: u64, end: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let (mut hi, mut size) = (end, BACK_FIRST);
    while hi > start {
        let lo = hi.saturating_sub(size).max(start);
        out.push((lo, hi));
        hi = lo;
        size = (size * 2).min(BACK_MOST);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn m(needle: &str, case: bool, name: &str) -> bool {
        Needle::new(needle, case).is_some_and(|n| n.matches(&w(name)))
    }

    #[test]
    fn text_is_found_anywhere_in_the_name() {
        assert!(m("port", false, "Report.pdf"));
        assert!(m("REPORT", false, "report.pdf"));
        assert!(!m("REPORT", true, "report.pdf"));
        assert!(m("Report", true, "Report.pdf"));
        assert!(!m("x", false, "Report.pdf"));
        assert!(m("ÄRGER", false, "das ärger.txt"));
        assert!(Needle::new("", false).is_none());
        // Longer than the name.
        assert!(!m("report.pdf.bak", false, "report.pdf"));
    }

    #[test]
    fn wildcards_match_the_whole_name() {
        assert!(m("*.pdf", false, "Report.PDF"));
        assert!(!m("*.pdf", false, "report.pdf.bak"));
        assert!(m("re*rt*", false, "report.txt"));
        assert!(m("r?port.*", false, "Report.txt"));
        assert!(!m("r?port", false, "rport"));
        assert!(m("*", false, "anything"));
        assert!(m("a**b", false, "a-b"));
        // A surrogate pair is one character for `?`.
        assert!(m("?.txt", false, "😀.txt"));
        assert!(m("a*", true, "abc") && !m("A*", true, "abc"));
    }

    #[test]
    fn spans_light_up_each_match() {
        let mut out = Vec::new();
        let n = Needle::new("ab", false).expect("needle");
        n.spans(&w("xABab-ab"), &mut out);
        assert_eq!(out, [(1, 3), (3, 5), (6, 8)]);
        n.spans(&w("nothing"), &mut out);
        assert!(out.is_empty());
        // A pattern: its literal pieces, in order, only for a match.
        let g = Needle::new("re*.pdf", false).expect("needle");
        g.spans(&w("Report.pdf"), &mut out);
        assert_eq!(out, [(0, 2), (6, 10)]);
        g.spans(&w("Report.txt"), &mut out);
        assert!(out.is_empty());
    }

    /// A part as a find from a focused row plans it: the anchor is the
    /// last row of the part round the end.
    fn p(start: u64, end: u64, forward: bool, wrapped: bool) -> Part {
        Part {
            start,
            end,
            forward,
            wrapped,
            edge: wrapped,
        }
    }

    fn pe(start: u64, end: u64, forward: bool, wrapped: bool, edge: bool) -> Part {
        Part {
            edge,
            ..p(start, end, forward, wrapped)
        }
    }

    /// Runs a gate over `rows` (index, name, matched) and says what it took
    /// and how many rows it read.
    fn gate(mut g: Gate, rows: &[(u64, &str, bool)]) -> (Option<u64>, usize) {
        let mut read = 0;
        for &(i, name, m) in rows {
            read += 1;
            if g.row(i, &w(name), m) {
                break;
            }
        }
        (g.pick().map(|(i, _)| i), read)
    }

    #[test]
    fn the_anchor_says_which_side_a_row_is_on() {
        let a = w("anchor");
        // Next from row 10 (planned 11..): files came before the anchor,
        // which is now row 13; 12 matches but is before it.
        let part = p(11, 100, true, false);
        let rows = [
            (11, "x", false),
            (12, "m", true),
            (13, "anchor", true),
            (14, "y", false),
            (15, "m2", true),
        ];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows), (Some(15), 5));
        // Files went: the anchor is row 8, read in the slack before the
        // plan; 9 matches and is after it.
        let rows = [(8, "anchor", false), (9, "m", true)];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows).0, Some(9));
        // No anchor at all: the first match from the plan's row on.
        let rows = [(9, "m", true), (11, "x", false), (12, "m", true)];
        assert_eq!(gate(Gate::new(&part, None, true), &rows), (Some(12), 3));
        // The anchor gone: the plan's pick once the slack is read.
        let far = 11 + SLACK;
        let rows = [(12, "m", true), (far, "x", false)];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows), (Some(12), 2));
        // As the text is typed, the anchor itself counts.
        let part = pe(10, 100, true, false, true);
        let rows = [(10, "anchor", true)];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows).0, Some(10));
    }

    #[test]
    fn round_the_end_stops_at_the_anchor() {
        let a = w("anchor");
        // Next's part round the end, planned 0..11 (the anchor row 10 its
        // last): files came, the anchor is now row 12 and 11 is before it.
        let part = p(0, 11, true, true);
        let rows = [(9, "x", false), (11, "m", true), (12, "anchor", false)];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows).0, Some(11));
        // A lone match: the anchor itself, found again.
        let rows = [(9, "x", false), (10, "anchor", true), (11, "m", true)];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows), (Some(10), 2));
        // Without the anchor, what is past the plan does not count.
        let rows = [(9, "x", false), (11, "m", true)];
        assert_eq!(gate(Gate::new(&part, Some(&a), true), &rows).0, None);
        // Previous's first part (before row 10), its first chunk read past
        // the plan: the last match before the anchor, now row 11.
        let part = p(0, 10, false, false);
        let rows = [
            (5, "m", true),
            (10, "m2", true),
            (11, "anchor", false),
            (12, "m3", true),
        ];
        assert_eq!(gate(Gate::new(&part, Some(&a), false), &rows).0, Some(10));
        // Without the anchor: the last before the plan's row.
        let rows = [(5, "m", true), (10, "m2", true), (12, "m3", true)];
        assert_eq!(gate(Gate::new(&part, Some(&a), false), &rows).0, Some(5));
        // Previous round the end (from the bottom down to the anchor at 10,
        // its own row counting): the last match after it.
        let part = p(10, 100, false, true);
        let rows = [(8, "m", true), (9, "anchor", true), (50, "x", false)];
        assert_eq!(gate(Gate::new(&part, Some(&a), false), &rows).0, Some(9));
        let rows = [(8, "m", true), (9, "anchor", false), (50, "m", true)];
        assert_eq!(gate(Gate::new(&part, Some(&a), false), &rows).0, Some(50));
        // An open gate takes the first, or the last, match.
        let rows = [(1, "m", true), (2, "m", true)];
        assert_eq!(gate(Gate::open(true), &rows).0, Some(1));
        assert_eq!(gate(Gate::open(false), &rows).0, Some(2));
    }

    #[test]
    fn a_find_goes_round_from_the_focused_row() {
        // Next from row 3 of 10: 4..10, then round to 0..4 (row 3 last).
        assert_eq!(
            plan(10, Some(3), true, false),
            [p(4, 10, true, false), p(0, 4, true, true)]
        );
        // As the text is typed: from row 3 itself.
        assert_eq!(
            plan(10, Some(3), true, true),
            [pe(3, 10, true, false, true), pe(0, 3, true, true, false)]
        );
        // Previous from row 3: 2 down to 0, then 9 down to 3.
        assert_eq!(
            plan(10, Some(3), false, false),
            [p(0, 3, false, false), p(3, 10, false, true)]
        );
        // Nothing focused: top to bottom, or bottom to top, once.
        assert_eq!(plan(10, None, true, false), [p(0, 10, true, false)]);
        assert_eq!(plan(10, None, false, false), [p(0, 10, false, false)]);
        // From the last row: straight round; nothing for no rows.
        assert_eq!(plan(10, Some(9), true, false), [p(0, 10, true, true)]);
        assert!(plan(0, None, true, false).is_empty());
        // A focus past the end (rows went) counts as none.
        assert_eq!(plan(5, Some(7), true, false), [p(0, 5, true, false)]);
    }

    #[test]
    fn walking_the_held_rows() {
        let parts = plan(10, Some(3), true, false);
        // Rows 0..8 held, 6 matches.
        let held = |i: u64| (i < 8).then_some(i == 6);
        assert_eq!(
            walk(&parts, held),
            Walked::Found {
                index: 6,
                wrapped: false
            }
        );
        // No match before row 8, which is not held: the rest from there.
        let held = |i: u64| (i < 8).then_some(false);
        assert_eq!(
            walk(&parts, held),
            Walked::Rest(vec![p(8, 10, true, false), p(0, 4, true, true)])
        );
        // Everything held, the only match before the focus: found round.
        let held = |i: u64| Some(i == 1);
        assert_eq!(
            walk(&parts, held),
            Walked::Found {
                index: 1,
                wrapped: true
            }
        );
        assert_eq!(walk(&parts, |_| Some(false)), Walked::Exhausted);
        // Backwards: from row 2 down, the rest of the part from the gap.
        let parts = plan(10, Some(3), false, false);
        let held = |i: u64| (i >= 2).then_some(false);
        assert_eq!(
            walk(&parts, held),
            Walked::Rest(vec![p(0, 2, false, false), p(3, 10, false, true)])
        );
    }

    #[test]
    fn backward_chunks_grow_from_the_end() {
        assert_eq!(back_chunks(0, 100), [(0, 100)]);
        assert_eq!(
            back_chunks(1000, 20_000),
            [(15_904, 20_000), (7712, 15_904), (1000, 7712)]
        );
        assert!(back_chunks(5, 5).is_empty());
        // Every row once, contiguous, and no chunk past the largest.
        let chunks = back_chunks(0, 3_000_000);
        assert_eq!(chunks.first().map(|c| c.1), Some(3_000_000));
        assert_eq!(chunks.last().map(|c| c.0), Some(0));
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].0, pair[1].1);
        }
        assert!(chunks.iter().all(|(lo, hi)| hi - lo <= BACK_MOST));
    }

    #[test]
    fn messages_are_everythings() {
        assert_eq!(
            not_found("abc"),
            "The following specified text was not found: abc"
        );
    }
}
