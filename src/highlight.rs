//! Which parts of a row the search matched, so the list can bold them.
//!
//! This reads the search text the way goz's query language does, closely
//! enough to show the user why a row matched: a term holding `\` or `/` (or
//! any term under Match Path) is looked for in the full path, any other in
//! the name, and a wildcard term lights up its literal pieces. It never
//! decides what matches; gozd does that.

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
        let mut h = Highlight {
            name: Vec::new(),
            path: Vec::new(),
            fold: !match_case,
        };
        for token in tokens(text) {
            h.add_token(&token, match_path);
        }
        h
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

    #[test]
    fn quoted_phrase_is_one_needle() {
        let (name, _) = run("\"annual report\"", r"C:\x\Annual Report.docx");
        assert_eq!(name, [(0, 13)]);
    }
}
