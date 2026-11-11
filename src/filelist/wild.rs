//! Wildcards and name filters as Everything's file-list features read them
//! (wave 6): the `filelist:` search function's items and the
//! `-create-file-list-include-only-*` / `-exclude-*` filters.
//!
//! A pattern is matched whole, `*` standing for any run of characters
//! (backslashes too) and `?` for one character; both sides are folded as
//! goz folds names unless the case matters. A pattern holding a `\` is
//! matched against the full path, any other against the name, as goz
//! reads a search term.

use goz_core::fold::fold_into;

/// The bytes a UTF-8 sequence starting with `b` takes (1 for a stray
/// continuation byte, so a walk always moves on).
fn utf8_len(b: u8) -> usize {
    match b {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

/// Whether `pattern` (`*` and `?` wildcards) matches the whole of `text`,
/// byte for byte but for `?`, which takes one character (a whole UTF-8
/// sequence). Backtracks to the last `*` only: O(len² ) at worst.
pub fn glob(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    // The last `*` seen, and where in the text its run ends now.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(b'?') => {
                p += 1;
                t += utf8_len(text[t]);
            }
            Some(&c) if c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                // The `*` takes one more character, and the rest is
                // tried again after it.
                Some((sp, st)) => {
                    let next = (st + utf8_len(text[st])).min(text.len());
                    star = Some((sp, next));
                    p = sp + 1;
                    t = next;
                }
                None => return false,
            },
        }
    }
    pattern[p.min(pattern.len())..].iter().all(|&c| c == b'*')
}

/// Whether `text` holds a wildcard.
pub fn has_wildcard(text: &str) -> bool {
    text.contains(['*', '?'])
}

/// `text` folded as goz folds names.
pub fn folded(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// One filter of a `;` list.
#[derive(Debug)]
enum Item {
    /// A wildcard pattern (folded), on the full path when it holds a `\`.
    Glob { pattern: Vec<u8>, path: bool },
    /// `regex:...`: a regular expression found anywhere in the name, or in
    /// the full path when it holds an escaped backslash (`\\`), as gozd's
    /// `regex:` reads one; any case.
    Regex { regex: regex::Regex, path: bool },
}

/// A `;` list of wildcard filters (Everything's `-create-file-list-*`
/// filters): an item matches when any of them does.
#[derive(Debug, Default)]
pub struct NameFilter {
    items: Vec<Item>,
}

impl NameFilter {
    /// Reads `text`: filters separated by `;`, each a wildcard pattern or
    /// `regex:` and a regular expression. An error names a regular
    /// expression that does not read.
    pub fn parse(text: &str) -> Result<NameFilter, String> {
        let mut items = Vec::new();
        for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let regex = part
                .get(..6)
                .filter(|p| p.eq_ignore_ascii_case("regex:"))
                .map(|_| &part[6..]);
            items.push(match regex {
                Some(re) => Item::Regex {
                    regex: regex::RegexBuilder::new(re)
                        .case_insensitive(true)
                        .build()
                        .map_err(|e| format!("regex:{re} does not read: {e}"))?,
                    path: re.contains(r"\\"),
                },
                None => {
                    let unslashed = part.replace('/', "\\");
                    Item::Glob {
                        path: unslashed.contains('\\'),
                        pattern: folded(unslashed.as_bytes()),
                    }
                }
            });
        }
        Ok(NameFilter { items })
    }

    /// Whether the item at `path` (a full path, its name last) matches one
    /// of the filters.
    pub fn matches(&self, path: &str) -> bool {
        let name = path.rsplit('\\').next().unwrap_or(path);
        let mut folded_path: Option<Vec<u8>> = None;
        let mut folded_name: Option<Vec<u8>> = None;
        self.items.iter().any(|item| match item {
            Item::Glob {
                pattern,
                path: true,
            } => glob(
                pattern,
                folded_path.get_or_insert_with(|| folded(path.as_bytes())),
            ),
            Item::Glob { pattern, .. } => glob(
                pattern,
                folded_name.get_or_insert_with(|| folded(name.as_bytes())),
            ),
            Item::Regex { regex, path: true } => regex.is_match(path),
            Item::Regex { regex, .. } => regex.is_match(name),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_whole_texts() {
        let g = |p: &str, t: &str| glob(p.as_bytes(), t.as_bytes());
        assert!(g("*.mp3", "song.mp3"));
        assert!(!g("*.mp3", "song.mp3.txt"));
        assert!(g("a?c", "abc") && g("a?c", "aéc"), "? takes a character");
        assert!(!g("a?c", "ac"));
        assert!(g("*", "") && g("**", "x") && g("", ""));
        assert!(!g("", "x"));
        assert!(g("*a*b*", "xxaxxbxx"));
        assert!(g(r"c:\*\x.txt", r"c:\a\b\x.txt"), "* takes backslashes");
        assert!(g("*é", "café") && !g("*e", "café"));
        assert!(has_wildcard("a*") && has_wildcard("?") && !has_wildcard("a.txt"));
    }

    #[test]
    fn name_filters_are_everythings() {
        let f = NameFilter::parse("*.MP3; *.flac ;;").unwrap();
        assert!(f.matches(r"D:\Music\A Song.mp3"));
        assert!(f.matches(r"D:\Music\b.FLAC"));
        assert!(!f.matches(r"D:\Music\c.wav"));
        assert!(!f.matches(r"D:\x.mp3\c.wav"), "the name only");
        // A pattern with a backslash is matched against the full path.
        let f = NameFilter::parse(r"*\node_modules").unwrap();
        assert!(f.matches(r"C:\src\app\node_modules"));
        assert!(!f.matches(r"C:\src\node_modules_old"));
        let f = NameFilter::parse(r"regex:^\d+\.txt$").unwrap();
        assert!(f.matches(r"C:\a\123.TXT") && !f.matches(r"C:\a\x123.txt"));
        let f = NameFilter::parse(r"regex:\\cache\\").unwrap();
        assert!(f.matches(r"C:\x\Cache\y.bin"), "a path regex");
        assert!(NameFilter::parse("regex:(").is_err());
        assert!(!NameFilter::parse(" ; ").unwrap().matches(r"C:x"));
    }
}
