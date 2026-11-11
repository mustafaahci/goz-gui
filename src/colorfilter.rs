//! Color filters: rows whose file name matches a filter draw their text in
//! the filter's colour (Everything 1.5's `color_filters`).
//!
//! The setting is Everything's JSON array of
//! `{"filter":…,"include_files":1,"include_folders":1,"foreground_color":"#RRGGBB","foreground_dark_color":"#RRGGBB"}`,
//! the first filter that matches a row giving its colour (the dark one in
//! the dark theme, the light one when it has no dark one). A filter is one
//! or more patterns separated by `;`:
//! - `*.mp3`, `report?.txt`, `*.tar.*`: the name, with `*` and `?`;
//! - with a `\`: the full path, `*` staying within one folder and `**`
//!   crossing folders (`C:\Windows\*` the folder's own items,
//!   `C:\Windows\**` everything under it);
//! - `regex:` then a regular expression, searched in the name;
//!
//! all ignoring case.

use regex::{Regex, RegexBuilder};

use crate::colors::{format_color, parse_color};
use crate::json::{self, Json};

/// One rule as the setting holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorFilter {
    pub filter: String,
    pub files: bool,
    pub folders: bool,
    pub color: Option<u32>,
    pub dark_color: Option<u32>,
}

impl Default for ColorFilter {
    fn default() -> ColorFilter {
        ColorFilter {
            filter: String::new(),
            files: true,
            folders: true,
            color: None,
            dark_color: None,
        }
    }
}

fn flag(v: Option<&Json>, default: bool) -> bool {
    match v {
        Some(Json::Bool(b)) => *b,
        Some(Json::Num(n)) => *n != 0.0,
        _ => default,
    }
}

fn color(v: Option<&Json>) -> Option<u32> {
    match v? {
        Json::Str(_) => parse_color(&v?.as_text()?),
        Json::Num(n) if *n >= 0.0 && *n <= f64::from(0x00ff_ffffu32) => Some(*n as u32),
        _ => None,
    }
}

/// The rules in a `color_filters` value; `Err` says why it is not one.
pub fn parse(value: &str) -> Result<Vec<ColorFilter>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let json = json::parse(value.as_bytes())?;
    let items = json
        .as_array()
        .ok_or_else(|| "color_filters is not a list".to_owned())?;
    let mut out = Vec::new();
    for item in items {
        if !matches!(item, Json::Object(_)) {
            return Err("an item of color_filters is not an object".into());
        }
        out.push(ColorFilter {
            filter: item
                .get("filter")
                .and_then(Json::as_text)
                .unwrap_or_default(),
            files: flag(item.get("include_files"), true),
            folders: flag(item.get("include_folders"), true),
            color: color(item.get("foreground_color")),
            dark_color: color(item.get("foreground_dark_color")),
        });
    }
    Ok(out)
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The rules as one line of JSON, as the INI holds them.
pub fn format(rules: &[ColorFilter]) -> String {
    if rules.is_empty() {
        return String::new();
    }
    let items: Vec<String> = rules
        .iter()
        .map(|r| {
            let c = |c: Option<u32>| quote(&c.map(format_color).unwrap_or_default());
            format!(
                "{{\"filter\":{},\"include_files\":{},\"include_folders\":{},\"foreground_color\":{},\"foreground_dark_color\":{}}}",
                quote(&r.filter),
                u8::from(r.files),
                u8::from(r.folders),
                c(r.color),
                c(r.dark_color)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// A pattern of a rule, ready to match.
#[derive(Debug)]
enum Pattern {
    /// Folded to lower case.
    Name(Vec<u16>),
    Path(Vec<u16>),
    Regex(Regex),
}

#[derive(Debug)]
struct Rule {
    patterns: Vec<Pattern>,
    files: bool,
    folders: bool,
    color: Option<u32>,
    dark_color: Option<u32>,
}

/// The rules made ready to match rows as they are drawn.
#[derive(Debug, Default)]
pub struct Matcher {
    rules: Vec<Rule>,
}

fn fold(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) {
        c + 32
    } else if c >= 0x80 {
        char::from_u32(u32::from(c))
            .and_then(|ch| {
                let mut lower = ch.to_lowercase();
                let one = lower.next()?;
                lower.next().is_none().then_some(one)
            })
            .and_then(|l| u16::try_from(u32::from(l)).ok())
            .unwrap_or(c)
    } else {
        c
    }
}

fn folded(s: &str) -> Vec<u16> {
    s.encode_utf16().map(fold).collect()
}

const STAR: u16 = b'*' as u16;
const QUESTION: u16 = b'?' as u16;
const SLASH: u16 = b'\\' as u16;

/// Whether `text` matches the folded `pat`: `?` one character, `*` any
/// run, which stays within a folder when `in_path` (`**` crossing them).
fn wild(pat: &[u16], text: &[u16], in_path: bool) -> bool {
    if in_path {
        wild_path(pat, text)
    } else {
        wild_name(pat, text)
    }
}

/// [`wild`] for a name, where every `*` crosses anything: iterative, with
/// one backtrack point (the last star), as is usual for glob matching.
fn wild_name(pat: &[u16], text: &[u16]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pat.len() && pat[p] == STAR {
            p += 1;
            star = Some((p, t));
            continue;
        }
        if p < pat.len() && (pat[p] == fold(text[t]) || pat[p] == QUESTION) {
            p += 1;
            t += 1;
            continue;
        }
        match star {
            Some((sp, st)) => {
                star = Some((sp, st + 1));
                p = sp;
                t = st + 1;
            }
            None => return false,
        }
    }
    pat[p..].iter().all(|&c| c == STAR)
}

/// [`wild`] for a full path, where `*` and `?` stay within a folder and
/// `**` crosses them. One backtrack point cannot serve both (a `*` after a
/// `**` that meets a `\` must hand the run back to the `**`), so every place
/// in the pattern the text so far can reach is followed at once: each
/// character of the text is read once, with no backtracking.
fn wild_path(pat: &[u16], text: &[u16]) -> bool {
    #[derive(Clone, Copy)]
    enum Tok {
        Char(u16),
        One,
        Star,
        Deep,
    }
    let mut toks = Vec::with_capacity(pat.len());
    let mut i = 0;
    while i < pat.len() {
        toks.push(match pat[i] {
            STAR if pat.get(i + 1) == Some(&STAR) => {
                i += 1;
                Tok::Deep
            }
            STAR => Tok::Star,
            QUESTION => Tok::One,
            c => Tok::Char(c),
        });
        i += 1;
    }
    // `at[k]`: the text so far can end before token `k`; a star there may
    // also match nothing, which reaches the token after it.
    let close = |at: &mut [bool]| {
        for (k, tok) in toks.iter().enumerate() {
            if at[k] && matches!(tok, Tok::Star | Tok::Deep) {
                at[k + 1] = true;
            }
        }
    };
    let mut at = vec![false; toks.len() + 1];
    let mut next = vec![false; toks.len() + 1];
    at[0] = true;
    close(&mut at);
    for &raw in text {
        let c = fold(raw);
        next.fill(false);
        let mut any = false;
        for (k, tok) in toks.iter().enumerate() {
            if !at[k] {
                continue;
            }
            let (stay, step) = match *tok {
                Tok::Char(p) => (false, p == c),
                Tok::One => (false, c != SLASH),
                Tok::Star => (c != SLASH, false),
                Tok::Deep => (true, false),
            };
            next[k] |= stay;
            next[k + 1] |= step;
            any |= stay || step;
        }
        if !any {
            return false;
        }
        std::mem::swap(&mut at, &mut next);
        close(&mut at);
    }
    at[toks.len()]
}

impl Matcher {
    pub fn new(rules: &[ColorFilter]) -> Matcher {
        let rules = rules
            .iter()
            .filter(|r| r.color.is_some() || r.dark_color.is_some())
            .map(|r| Rule {
                patterns: r
                    .filter
                    .split(';')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .filter_map(|p| {
                        if let Some(re) = p.strip_prefix("regex:") {
                            RegexBuilder::new(re)
                                .case_insensitive(true)
                                .size_limit(1 << 20)
                                .build()
                                .ok()
                                .map(Pattern::Regex)
                        } else if p.contains('\\') {
                            Some(Pattern::Path(folded(p)))
                        } else {
                            Some(Pattern::Name(folded(p)))
                        }
                    })
                    .collect(),
                files: r.files,
                folders: r.folders,
                color: r.color,
                dark_color: r.dark_color,
            })
            .collect();
        Matcher { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The text colour of a row: its full `path` (the name its end, from
    /// `name_start`), whether it is a folder, in the dark theme or not.
    pub fn color(&self, path: &[u16], name_start: usize, is_dir: bool, dark: bool) -> Option<u32> {
        let name = &path[name_start.min(path.len())..];
        for r in &self.rules {
            if (is_dir && !r.folders) || (!is_dir && !r.files) {
                continue;
            }
            let colour = if dark {
                r.dark_color.or(r.color)
            } else {
                r.color.or(r.dark_color)
            };
            let Some(colour) = colour else { continue };
            let hit = r.patterns.iter().any(|p| match p {
                Pattern::Name(p) => wild(p, name, false),
                Pattern::Path(p) => wild(p, path, true),
                Pattern::Regex(re) => re.is_match(&String::from_utf16_lossy(name)),
            });
            if hit {
                return Some(colour);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::rgb;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn rule(filter: &str, color: u32) -> ColorFilter {
        ColorFilter {
            filter: filter.into(),
            color: Some(color),
            ..ColorFilter::default()
        }
    }

    fn hit(m: &Matcher, path: &str, dir: bool) -> Option<u32> {
        let p = w(path);
        let start = p.iter().rposition(|&c| c == SLASH).map_or(0, |i| i + 1);
        m.color(&p, start, dir, false)
    }

    #[test]
    fn everythings_json_reads_and_writes_back() {
        let v = r##"[{"filter":"*.exe;*.dll","include_files":1,"include_folders":0,"foreground_color":"#ff0000","foreground_dark_color":"#ff8080"},{"filter":"C:\\Windows\\**","include_files":true,"include_folders":true,"foreground_color":"","foreground_dark_color":""}]"##;
        let rules = parse(v).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].filter, "*.exe;*.dll");
        assert!(rules[0].files && !rules[0].folders);
        assert_eq!(rules[0].color, Some(rgb(0xff, 0, 0)));
        assert_eq!(rules[0].dark_color, Some(rgb(0xff, 0x80, 0x80)));
        assert_eq!(rules[1].filter, r"C:\Windows\**");
        assert_eq!(rules[1].color, None);
        let again = format(&rules);
        assert!(!again.contains('\n'));
        assert_eq!(parse(&again).unwrap(), rules);
        assert_eq!(parse("").unwrap(), Vec::new());
        assert_eq!(format(&[]), "");
        assert!(parse("{}").is_err());
        assert!(parse("[1]").is_err());
        assert!(parse("[{").is_err());
    }

    #[test]
    fn names_paths_and_regexes_match_as_everything() {
        let m = Matcher::new(&[
            rule("*.exe;*.DLL", 1),
            rule(r"C:\Windows\*", 2),
            rule(r"D:\Music\**", 3),
            rule("regex:^read", 4),
            rule("notes.txt", 5),
        ]);
        assert_eq!(hit(&m, r"C:\x\setup.EXE", false), Some(1));
        assert_eq!(hit(&m, r"C:\x\a.dll", false), Some(1));
        assert_eq!(hit(&m, r"C:\x\a.dllx", false), None);
        // `*` keeps to the folder's own items; `**` goes deeper.
        assert_eq!(hit(&m, r"C:\Windows\win.ini", false), Some(2));
        assert_eq!(hit(&m, r"C:\Windows\System32\x.ini", false), None);
        assert_eq!(hit(&m, r"d:\music\a\b\c.flac", false), Some(3));
        assert_eq!(hit(&m, r"C:\x\README.md", false), Some(4));
        assert_eq!(hit(&m, r"C:\x\Notes.TXT", false), Some(5));
        assert_eq!(hit(&m, r"C:\x\my notes.txt", false), None);
        // Folders and files as the rule says.
        let only_files = Matcher::new(&[ColorFilter {
            folders: false,
            ..rule("*", 9)
        }]);
        assert_eq!(hit(&only_files, r"C:\a", true), None);
        assert_eq!(hit(&only_files, r"C:\a", false), Some(9));
    }

    #[test]
    fn the_first_rule_wins_and_the_dark_colour_is_the_dark_themes() {
        let m = Matcher::new(&[
            ColorFilter {
                dark_color: Some(7),
                ..rule("*.txt", 6)
            },
            rule("*", 8),
            // No colour: never matches.
            ColorFilter {
                filter: "*.md".into(),
                ..ColorFilter::default()
            },
        ]);
        let p = w(r"C:\a.txt");
        assert_eq!(m.color(&p, 3, false, false), Some(6));
        assert_eq!(m.color(&p, 3, false, true), Some(7));
        assert_eq!(hit(&m, r"C:\a.md", false), Some(8));
        assert!(Matcher::new(&[ColorFilter::default()]).is_empty());
    }

    #[test]
    fn a_bad_regex_is_left_out_and_wildcards_backtrack() {
        let m = Matcher::new(&[rule("regex:(", 1), rule("a*b*c", 2)]);
        assert_eq!(hit(&m, r"C:\(", false), None);
        assert_eq!(hit(&m, r"C:\axxbyyc", false), Some(2));
        assert_eq!(hit(&m, r"C:\axxbyy", false), None);
        assert!(wild(&folded("*"), &w(""), false));
        assert!(wild(&folded("?"), &w("é"), false));
        assert!(wild(&folded("é*"), &w("É.txt"), false));
    }

    /// A `*` after a `**` keeps to one folder, and the `**` takes as many
    /// folders as it needs (one backtrack point for both lost them).
    #[test]
    fn a_star_after_two_stars_lets_them_take_more_folders() {
        let m = Matcher::new(&[rule(r"C:\Windows\**\*.dll", 1)]);
        assert_eq!(hit(&m, r"C:\Windows\System32\x.dll", false), Some(1));
        assert_eq!(
            hit(&m, r"C:\Windows\System32\drivers\x.dll", false),
            Some(1)
        );
        assert_eq!(hit(&m, r"C:\Windows\a\b\c\d\X.DLL", false), Some(1));
        assert_eq!(hit(&m, r"C:\Windows\x.dll", false), None);
        assert_eq!(hit(&m, r"C:\Windows\a\x.dllx", false), None);
        assert_eq!(hit(&m, r"C:\Other\a\x.dll", false), None);
        let p = |pat: &str, text: &str| wild(&folded(pat), &w(text), true);
        assert!(p(r"C:\**\a*\*.txt", r"C:\x\y\ab\z.txt"));
        assert!(!p(r"C:\**\a*\*.txt", r"C:\x\ab\y\z.txt"));
        assert!(p(r"**\?\*", r"C:\x\y\z"));
        assert!(!p(r"C:\*\*", r"C:\x\y\z"));
        assert!(p(r"C:\**", r"C:\"));
        assert!(p(r"C:\*.*", r"C:\a.b"));
        assert!(!p(r"C:\?", r"C:\\"));
    }
}
