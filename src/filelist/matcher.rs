//! goz-gui's own matcher for an open file list: the search box's text is
//! parsed by goz-core's own parser (so it reads exactly as gozd reads it,
//! and is refused with gozd's words), then each row of the list is tested
//! the way gozd's engine tests an index entry: the kind (`file:`,
//! `folder:`), `size:`, `dm:`, every name term (a substring of the name),
//! every name wildcard (`*`, `?`, the whole name), `ext:`, then every path
//! term (a term with `\`, or every term under Match Path / `path:`: a
//! substring of the full path) and path wildcard. Without Match Case
//! (`case:`) both sides are folded as goz folds them.

use goz_core::fold::fold_into;
use goz_core::query::{Kind, ParsedQuery};
use memchr::memmem::Finder;

/// A parsed search, ready to test rows with.
pub struct Matcher {
    parsed: ParsedQuery,
    names: Vec<Finder<'static>>,
    paths: Vec<Finder<'static>>,
    /// Reusable buffers: a row's folded name and path, and an extension.
    name_buf: Vec<u8>,
    path_buf: Vec<u8>,
    ext_buf: Vec<u8>,
}

/// One row as the matcher sees it.
pub struct Candidate<'a> {
    /// The full path, UTF-8.
    pub path: &'a [u8],
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
}

/// The name in a full path: what follows its last `\`, or the whole path
/// for a drive's root (`C:\`).
pub fn name_of(path: &[u8]) -> &[u8] {
    match memchr::memrchr(b'\\', path) {
        Some(i) if i + 1 < path.len() => &path[i + 1..],
        _ => path,
    }
}

impl Matcher {
    pub fn new(parsed: ParsedQuery) -> Matcher {
        let finder = |t: &Vec<u8>| Finder::new(t.as_slice()).into_owned();
        Matcher {
            names: parsed.name_terms.iter().map(finder).collect(),
            paths: parsed.path_terms.iter().map(finder).collect(),
            parsed,
            name_buf: Vec::new(),
            path_buf: Vec::new(),
            ext_buf: Vec::new(),
        }
    }

    /// Whether `row` matches the search.
    pub fn matches(&mut self, row: &Candidate<'_>) -> bool {
        let f = &self.parsed.filters;
        match f.kind {
            Some(Kind::File) if row.is_dir => return false,
            Some(Kind::Folder) if !row.is_dir => return false,
            _ => {}
        }
        // An unknown size is no match for a size filter, as in gozd.
        if let Some(range) = f.size
            && !row.size.is_some_and(|s| range.contains(s))
        {
            return false;
        }
        if let Some(date) = f.date
            && !date.matches(row.mtime)
        {
            return false;
        }
        let case = self.parsed.match_case;
        let name = name_of(row.path);
        if !self.names.is_empty() || !self.parsed.wildcards.is_empty() {
            let hay = if case {
                name
            } else {
                self.name_buf.clear();
                fold_into(name, &mut self.name_buf);
                &self.name_buf
            };
            if !self.names.iter().all(|t| t.find(hay).is_some())
                || !self.parsed.wildcards.iter().all(|w| w.matches(hay))
            {
                return false;
            }
        }
        // The extension, folded whatever the case (es and gozd alike).
        if let Some(exts) = &f.ext {
            let Some(dot) = memchr::memrchr(b'.', name) else {
                return false;
            };
            self.ext_buf.clear();
            fold_into(&name[dot + 1..], &mut self.ext_buf);
            if !exts.contains(&self.ext_buf) {
                return false;
            }
        }
        if !self.paths.is_empty() || !self.parsed.path_wildcards.is_empty() {
            let hay = if case {
                row.path
            } else {
                self.path_buf.clear();
                fold_into(row.path, &mut self.path_buf);
                &self.path_buf
            };
            if !self.paths.iter().all(|t| t.find(hay).is_some())
                || !self.parsed.path_wildcards.iter().all(|w| w.matches(hay))
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::{LocalClock, parse_query_at};

    struct Clock;
    impl LocalClock for Clock {
        fn now(&self) -> i64 {
            // 2024-06-01 12:00 UTC.
            133_617_168_000_000_000
        }
        fn to_local(&self, utc: i64) -> Option<i64> {
            Some(utc)
        }
        fn to_utc(&self, local: i64) -> Option<i64> {
            Some(local)
        }
        fn week_start(&self) -> u8 {
            0
        }
    }

    fn hits(
        query: &str,
        case: bool,
        rows: &[(&str, bool, Option<u64>, Option<i64>)],
    ) -> Vec<String> {
        let mut m = Matcher::new(parse_query_at(query, case, &Clock).expect("parses"));
        rows.iter()
            .filter(|(p, d, s, t)| {
                m.matches(&Candidate {
                    path: p.as_bytes(),
                    is_dir: *d,
                    size: *s,
                    mtime: *t,
                })
            })
            .map(|r| r.0.to_owned())
            .collect()
    }

    const ROWS: [(&str, bool, Option<u64>, Option<i64>); 6] = [
        (
            r"C:\Music\Queen - Bohemian Rhapsody.mp3",
            false,
            Some(8_000_000),
            Some(133_600_000_000_000_000),
        ),
        (
            r"C:\Music\notes.txt",
            false,
            Some(120),
            Some(133_000_000_000_000_000),
        ),
        (r"C:\Music", true, None, None),
        (
            r"D:\Projects\Report final.PDF",
            false,
            Some(2_000_000),
            None,
        ),
        (r"D:\Projects\café.txt", false, Some(5), None),
        (r"C:\", true, None, None),
    ];

    #[test]
    fn terms_wildcards_and_paths_as_gozd_reads_them() {
        assert_eq!(hits("queen rhapsody", false, &ROWS), [ROWS[0].0]);
        assert_eq!(hits("\"final.pdf\"", false, &ROWS), [ROWS[3].0]);
        assert!(hits("Final", true, &ROWS).is_empty(), "case:");
        assert_eq!(hits("*.txt", false, &ROWS), [ROWS[1].0, ROWS[4].0]);
        assert_eq!(hits("not?s.*", false, &ROWS), [ROWS[1].0]);
        assert_eq!(hits(r"music\ .txt", false, &ROWS), [ROWS[1].0]);
        assert_eq!(hits(r"D:\", false, &ROWS).len(), 2);
        assert_eq!(
            hits("CAFÉ", false, &ROWS),
            [ROWS[4].0],
            "folded as goz folds"
        );
        assert_eq!(hits("C:", false, &ROWS), [ROWS[5].0], "a root is all name");
    }

    #[test]
    fn functions() {
        assert_eq!(hits("ext:pdf;mp3", false, &ROWS), [ROWS[0].0, ROWS[3].0]);
        assert_eq!(hits("size:>1mb", false, &ROWS), [ROWS[0].0, ROWS[3].0]);
        assert_eq!(hits("folder:", false, &ROWS), [ROWS[2].0, ROWS[5].0]);
        assert!(hits("file: music", false, &ROWS).is_empty());
        assert_eq!(hits("folder: music", false, &ROWS), [ROWS[2].0]);
        assert_eq!(hits("dm:2024", false, &ROWS), [ROWS[0].0]);
        assert_eq!(hits("path:music notes", false, &ROWS), [ROWS[1].0]);
    }

    #[test]
    fn names_in_paths() {
        assert_eq!(name_of(br"C:\a\b.txt"), b"b.txt");
        assert_eq!(name_of(br"C:\"), br"C:\");
        assert_eq!(name_of(b"plain"), b"plain");
    }
}
