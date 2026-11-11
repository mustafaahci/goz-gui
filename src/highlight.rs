//! Which parts of a row the search matched, so the list can bold them.
//!
//! This reads the search text the way goz's query language does, closely
//! enough to show the user why a row matched: a term holding `\` or `/` (or
//! any term under Match Path) is looked for in the full path, any other in
//! the name, and a wildcard term lights up its literal pieces. It never
//! decides what matches; gozd does that.
//!
//! A gozd that reads Everything 1.5's syntax (`crate::client::boolean_syntax`)
//! gets it read that way too ([`Highlight::with_syntax`]): every alternative
//! of `a|b` lights up, a term or `<group>` under `!` (or `not:`) does not,
//! `and:`/`or:` are operators, a modifier (`ww:`, `case:`, `path:`, ...) is
//! taken off its term, and functions that test no text (`len:`, `parent:`,
//! `size:`, ...) light nothing.

/// The needles of one search, folded unless the search matches case.
#[derive(Default)]
pub struct Highlight {
    name: Vec<Vec<u16>>,
    path: Vec<Vec<u16>>,
    fold: bool,
}

/// A half-open range of UTF-16 code units to draw bold.
pub type Span = (usize, usize);

impl Highlight {
    pub fn new(text: &str, match_case: bool, match_path: bool) -> Self {
        Self::with_syntax(text, match_case, match_path, false)
    }

    /// [`Highlight::new`], reading `text` in the new query language (OR,
    /// NOT, groups, modifiers) when `boolean` says gozd does.
    pub fn with_syntax(text: &str, match_case: bool, match_path: bool, boolean: bool) -> Self {
        let mut h = Highlight {
            name: Vec::new(),
            path: Vec::new(),
            fold: !match_case,
        };
        if boolean {
            h.add_query(text, match_path);
        } else {
            for token in tokens(text) {
                h.add_token(&token, match_path);
            }
        }
        h
    }

    /// The new syntax's search `text`: its terms that are not negated.
    fn add_query(&mut self, text: &str, in_path: bool) {
        for term in positive_terms(text) {
            self.add_new_term(&term, in_path);
        }
    }

    /// One term of the new syntax, as written (quotes and all): its
    /// modifiers taken off, then its text, or a function's.
    fn add_new_term(&mut self, raw: &str, mut in_path: bool) {
        let mut rest = raw;
        while let Some((func, value)) = split_func(rest) {
            match func.as_str() {
                "path" => in_path = true,
                "nopath" => in_path = false,
                f if MODIFIERS.contains(&f) => {}
                "ext" => {
                    let value = value.replace('"', "");
                    let value = value.trim_start_matches('<').trim_end_matches('>');
                    for ext in value.split([';', '|']).map(str::trim) {
                        let ext = ext.trim_start_matches('.');
                        if ext.is_empty() {
                            continue;
                        }
                        let needle = format!(".{ext}");
                        for piece in needle.split(['*', '?']).filter(|p| p.len() > 1) {
                            self.push(piece, false);
                        }
                    }
                    return;
                }
                f if TEXT_FUNCTIONS.contains(&f) => {}
                // Every other function tests something else than text:
                // size:, dm:, len:, parent: (a folder, not a match), ...
                _ => return,
            }
            rest = value;
        }
        let rest = rest.trim();
        if rest.len() >= 2 && rest.starts_with('<') && rest.ends_with('>') {
            // A modifier's group: `ww:<a|b>`.
            self.add_query(&rest[1..rest.len() - 1], in_path);
            return;
        }
        let text = rest.replace('"', "");
        if !text.is_empty() {
            self.add_term(&text, in_path);
        }
    }

    fn add_token(&mut self, token: &str, match_path: bool) {
        if token.starts_with('!') {
            return;
        }
        if let Some(colon) = token.find(':') {
            let func = &token[..colon];
            let value = &token[colon + 1..];
            if func.len() > 1 && func.bytes().all(|b| b.is_ascii_alphabetic()) {
                match func.to_ascii_lowercase().as_str() {
                    "path" => self.add_term(value, true),
                    "file" | "folder" | "dir" => self.add_term(value, match_path),
                    "ext" => {
                        for ext in value.split(';').filter(|e| !e.is_empty()) {
                            let needle = format!(".{}", ext.trim_start_matches('.'));
                            self.push(&needle, false);
                        }
                    }
                    _ => {}
                }
                return;
            }
        }
        self.add_term(token, match_path);
    }

    fn add_term(&mut self, term: &str, in_path: bool) {
        let in_path = in_path || term.contains(['\\', '/']);
        let term = term.replace('/', "\\");
        for piece in term.split(['*', '?']).filter(|p| !p.is_empty()) {
            self.push(piece, in_path);
        }
    }

    fn push(&mut self, needle: &str, in_path: bool) {
        let mut units: Vec<u16> = needle.encode_utf16().collect();
        if self.fold {
            units.iter_mut().for_each(|c| *c = fold(*c));
        }
        if in_path {
            self.path.push(units);
        } else {
            self.name.push(units);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.path.is_empty()
    }

    /// The bold spans of one row: `name` indexes the name, `parent` the
    /// folder shown in the Path column (the full path up to the separator
    /// before the name).
    pub fn spans(
        &self,
        path: &[u16],
        name_start: usize,
        scratch: &mut Vec<u16>,
        name: &mut Vec<Span>,
        parent: &mut Vec<Span>,
    ) {
        name.clear();
        parent.clear();
        if self.is_empty() {
            return;
        }
        scratch.clear();
        if self.fold {
            scratch.extend(path.iter().map(|&c| fold(c)));
        } else {
            scratch.extend_from_slice(path);
        }
        let parent_len = name_start.saturating_sub(1);
        for needle in &self.path {
            for (s, e) in find_all(scratch, needle) {
                if s < parent_len {
                    parent.push((s, e.min(parent_len)));
                }
                if e > name_start {
                    name.push((s.max(name_start) - name_start, e - name_start));
                }
            }
        }
        let name_text = &scratch[name_start..];
        for needle in &self.name {
            name.extend(find_all(name_text, needle));
        }
        merge(name);
        merge(parent);
    }
}

/// The search text split as goz splits it: on spaces, with quotes grouping
/// and then dropped.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Modifiers of the new syntax: they say how their term matches, and
/// are taken off it (`path:` and `nopath:` are read on their own).
const MODIFIERS: &[&str] = &[
    "case",
    "nocase",
    "ww",
    "noww",
    "wholeword",
    "wholewords",
    "nowholeword",
    "nowholewords",
    "prefix",
    "noprefix",
    "suffix",
    "nosuffix",
    "startwith",
    "nostartwith",
    "endwith",
    "noendwith",
    "^",
    "$",
    "whole",
    "nowhole",
    "exact",
    "noexact",
    "nopunc",
    "ignore-punctuation",
    "nows",
    "ignore-whitespace",
    "starall",
    "nowildcards",
    "file",
    "files",
    "folder",
    "folders",
    "dir",
    "no-file",
    "no-folder",
    "metric",
];

/// Functions whose value is text in the name: the name's parts and
/// Everything's macros (`audio:beatles`).
const TEXT_FUNCTIONS: &[&str] = &[
    "name", "stem", "audio", "zip", "doc", "exe", "image", "pic", "video",
];

/// A term's leading `func:` (lower-cased, a global `?` dropped) and what
/// follows it, unless the colon is quoted or the name is no function's
/// (a drive's one letter, `C:`).
fn split_func(raw: &str) -> Option<(String, &str)> {
    let colon = raw.find(':')?;
    let name = raw[..colon].trim_start_matches('?');
    let named = name.len() > 1 && name.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
        || name == "^"
        || name == "$";
    named.then(|| (name.to_ascii_lowercase(), &raw[colon + 1..]))
}

/// Whether `raw` ends at a function's value start: right after `func:`,
/// or a comparison begun there (`size:>`, `dm:<=`), where `<` and `>`
/// compare rather than group.
fn at_value_start(raw: &str) -> bool {
    let bare = raw.trim_end_matches(['<', '>', '=', '!']);
    bare.ends_with(':') && raw.len() - bare.len() <= 2
}

/// How long the value group starting at `chars[0]` (`<`) runs, its `>`
/// included, when it closes before white space (`ext:<pdf|txt>`); `None`
/// for a comparison (`dm:<30days`).
fn value_group_len(chars: &[char]) -> Option<usize> {
    let (mut depth, mut quoted) = (0usize, false);
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            c if c.is_whitespace() => return None,
            _ => {}
        }
    }
    None
}

/// The terms of a search in the new syntax that are not negated, each as
/// written (quotes kept). `|` and white space separate terms, `<` `>`
/// group them, `!` and `not:` negate the next term or group (a group's
/// negation reaching every term in it, two cancelling), `and:` and `or:`
/// are operators; in quotes all of these are text.
fn positive_terms(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    // Each open group's negation.
    let mut groups: Vec<bool> = Vec::new();
    let mut not = false;
    let mut i = 0;
    let ends_term =
        |c: Option<&char>| c.is_none_or(|c| c.is_whitespace() || matches!(c, '|' | '>'));
    while i < chars.len() {
        let negated = groups.last().copied().unwrap_or(false);
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '|' => {
                // `!|`: an empty operand negated.
                not = false;
                i += 1;
            }
            '!' => {
                not = !not;
                i += 1;
                // `! foo`: a negated empty term, then foo.
                if ends_term(chars.get(i)) {
                    not = false;
                }
            }
            '<' => {
                groups.push(negated ^ not);
                not = false;
                i += 1;
            }
            '>' if !groups.is_empty() => {
                groups.pop();
                not = false;
                i += 1;
            }
            _ => {
                let mut raw = String::new();
                let mut quoted = false;
                while let Some(&c) = chars.get(i) {
                    if quoted {
                        quoted = c != '"';
                    } else if c == '"' {
                        quoted = true;
                    } else if c.is_whitespace()
                        || c == '|'
                        || (c == '>' && !groups.is_empty() && !at_value_start(&raw))
                    {
                        break;
                    } else if c == '<'
                        && at_value_start(&raw)
                        && let Some(len) = value_group_len(&chars[i..])
                    {
                        raw.extend(&chars[i..i + len]);
                        i += len;
                        continue;
                    }
                    raw.push(c);
                    i += 1;
                }
                match raw.to_ascii_lowercase().as_str() {
                    "and:" | "or:" => {}
                    "not:" => not = !not,
                    _ => {
                        if !(negated ^ not) {
                            out.push(raw);
                        }
                        not = false;
                    }
                }
            }
        }
    }
    out
}

/// Lower-cases one UTF-16 code unit when that keeps it one code unit, so
/// folded text indexes exactly like the original.
pub fn fold(c: u16) -> u16 {
    if c < 0x80 {
        return u16::from((c as u8).to_ascii_lowercase());
    }
    let Some(ch) = char::from_u32(u32::from(c)) else {
        return c;
    };
    let mut lower = ch.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) if (l as u32) < 0x1_0000 => l as u32 as u16,
        _ => c,
    }
}

fn find_all<'a>(hay: &'a [u16], needle: &'a [u16]) -> impl Iterator<Item = Span> + 'a {
    let mut at = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() || needle.len() > hay.len() {
            return None;
        }
        while at + needle.len() <= hay.len() {
            if hay[at..at + needle.len()] == *needle {
                let span = (at, at + needle.len());
                at = span.1;
                return Some(span);
            }
            at += 1;
        }
        None
    })
}

/// Sorts spans and joins the ones that touch or overlap.
fn merge(spans: &mut Vec<Span>) {
    if spans.len() < 2 {
        return;
    }
    spans.sort_unstable();
    let mut out = 0;
    for i in 1..spans.len() {
        if spans[i].0 <= spans[out].1 {
            spans[out].1 = spans[out].1.max(spans[i].1);
        } else {
            out += 1;
            spans[out] = spans[i];
        }
    }
    spans.truncate(out + 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn run(query: &str, path: &str) -> (Vec<Span>, Vec<Span>) {
        let h = Highlight::new(query, false, false);
        let path = w(path);
        let name_start = path.iter().rposition(|&c| c == u16::from(b'\\')).unwrap() + 1;
        let (mut name, mut parent, mut scratch) = (Vec::new(), Vec::new(), Vec::new());
        h.spans(&path, name_start, &mut scratch, &mut name, &mut parent);
        (name, parent)
    }

    #[test]
    fn name_terms_light_the_name_only() {
        let (name, parent) = run("note", r"C:\notes\Notepad.exe");
        assert_eq!(name, [(0, 4)]);
        assert!(parent.is_empty());
    }

    #[test]
    fn path_wildcard_lights_folder_and_extension() {
        let (name, parent) = run(
            r"c:\windows\system32\*.exe",
            r"C:\Windows\System32\notepad.exe",
        );
        assert_eq!(parent, [(0, 19)]);
        assert_eq!(name, [(7, 11)]);
    }

    #[test]
    fn functions_are_not_text() {
        let (name, parent) = run("size:>1mb ext:pdf report", r"C:\docs\report.pdf");
        // "report" and ".pdf" touch, so they join into one bold run.
        assert_eq!(name, [(0, 10)]);
        assert!(parent.is_empty());
    }

    fn run_new(query: &str, path: &str) -> (Vec<Span>, Vec<Span>) {
        let h = Highlight::with_syntax(query, false, false, true);
        let path = w(path);
        let name_start = path.iter().rposition(|&c| c == u16::from(b'\\')).unwrap() + 1;
        let (mut name, mut parent, mut scratch) = (Vec::new(), Vec::new(), Vec::new());
        h.spans(&path, name_start, &mut scratch, &mut name, &mut parent);
        (name, parent)
    }

    #[test]
    fn every_alternative_lights_up() {
        let file = r"C:\x\report invoice.pdf";
        assert_eq!(run_new("report|invoice", file).0, [(0, 6), (7, 14)]);
        assert_eq!(run_new("report | invoice", file).0, [(0, 6), (7, 14)]);
        assert_eq!(
            run_new("<report pdf>|invoice", file).0,
            [(0, 6), (7, 14), (15, 18)]
        );
        assert_eq!(run_new("x or: invoice", file).0, [(7, 14)]);
        // The old syntax reads `|` as text, as a gozd before it would.
        assert!(run("report|invoice", file).0.is_empty());
    }

    #[test]
    fn negated_terms_and_groups_stay_plain() {
        let file = r"C:\x\report draft v2.txt";
        assert_eq!(run_new("report !draft", file).0, [(0, 6)]);
        assert_eq!(run_new("!<report|draft> v2", file).0, [(13, 15)]);
        assert_eq!(run_new("not: report draft", file).0, [(7, 12)]);
        assert_eq!(run_new("!!report", file).0, [(0, 6)]);
        // `!` alone negates nothing that follows.
        assert_eq!(run_new("! report", file).0, [(0, 6)]);
        // Under a negated group, a negated term matched.
        assert_eq!(run_new("!<report !draft>", file).0, [(7, 12)]);
        // A `!` inside a word is text.
        assert_eq!(run_new("wow!.txt", r"C:\x\wow!.txt").0, [(0, 8)]);
    }

    #[test]
    fn modifiers_come_off_their_terms() {
        let file = r"C:\src\Report Invoice.txt";
        assert_eq!(run_new("ww:report prefix:inv", file).0, [(0, 6), (7, 10)]);
        for m in [
            "case:",
            "nocase:",
            "suffix:",
            "startwith:",
            "endwith:",
            "whole:",
            "exact:",
            "nopath:",
            "file:",
            "ww:case:",
        ] {
            assert_eq!(run_new(&format!("{m}report"), file).0, [(0, 6)], "{m}");
        }
        assert_eq!(run_new("ww:<report|invoice>", file).0, [(0, 6), (7, 14)]);
        // path: looks in the full path.
        let (name, parent) = run_new("path:src", file);
        assert!(name.is_empty());
        assert_eq!(parent, [(3, 6)]);
        // Quoted, an operator is text.
        assert_eq!(run_new("\"t|x\"", r"C:\a\t|x").0, [(0, 3)]);
    }

    #[test]
    fn functions_that_test_no_text_light_nothing() {
        let file = r"C:\src\notes.txt";
        let (name, parent) = run_new(r"len:>4 parent:C:\src infolder:C:\src notes", file);
        assert_eq!(name, [(0, 5)]);
        assert!(parent.is_empty());
        assert_eq!(run_new("<size:>1kb notes>|dm:<30days", file).0, [(0, 5)]);
        assert_eq!(run_new("size:<1kb|2kb> notes", file).0, [(0, 5)]);
        assert_eq!(run_new("ext:<pdf|txt>", file).0, [(5, 9)]);
        assert_eq!(run_new("ext:tx?;pdf", file).0, [(5, 8)]);
        assert_eq!(run_new("doc:notes", file).0, [(0, 5)]);
    }

    #[test]
    fn quoted_phrase_is_one_needle() {
        let (name, _) = run("\"annual report\"", r"C:\x\Annual Report.docx");
        assert_eq!(name, [(0, 13)]);
    }
}
