//! The `goz:` URL: a link that opens goz-gui on a search, as Everything's
//! `es:` URL does. Windows runs `goz-gui.exe -url "<the link>"` for it once
//! Options > General registers the protocol (`crate::integration`).
//!
//! `goz:<search>[?<option>[&<option>...]]`. The search and the options'
//! values are percent-decoded as UTF-8 (`%20` is a space, `%3F` a question
//! mark, which would otherwise start the options; `+` stays a plus).
//! Options, any case: `case`, `nocase`, `path`, `nopath` (Match Case and
//! Match Path; `case=0` and `path=1` work too), `filter=<name>` (Audio,
//! Compressed, Document, Executable, Folder, Image, Video, Everything),
//! `sort=<name>` (Name, Path, Size, Date Modified), `ascending` and
//! `descending`. As with `-url` in Everything (since 1408a), any scheme is
//! taken off (`es:` too, but not a drive letter), and so are a `//` after
//! it with the `/` a browser may add at the end. An option goz-gui does
//! not know is refused, naming it, rather than searching for something
//! else than the link meant; what follows a `#` is not used.

use goz_core::types::{SortDir, SortKey};

use crate::filter::Filter;

/// What a `goz:` URL asks for; `None` leaves the window's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UrlSearch {
    pub search: String,
    pub match_case: Option<bool>,
    pub match_path: Option<bool>,
    pub filter: Option<Filter>,
    pub sort: Option<SortKey>,
    pub dir: Option<SortDir>,
}

/// `text` with its `%XX` escapes decoded (as UTF-8); a `%` not followed by
/// two hex digits stays as it is.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `url` without its scheme (`goz:`, `es:`, any `name:` of two letters or
/// more, so `C:\` stays), and without a `//` after it.
fn strip_scheme(url: &str) -> (&str, bool) {
    let scheme_len = url
        .find(':')
        .filter(|&n| {
            n >= 2
                && url[..n].starts_with(|c: char| c.is_ascii_alphabetic())
                && url[..n]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        })
        .map_or(0, |n| n + 1);
    let rest = &url[scheme_len..];
    match rest.strip_prefix("//") {
        Some(r) if scheme_len > 0 => (r, true),
        _ => (rest, false),
    }
}

/// A flag option's value: none (on), `1`/`0`, `true`/`false`, `yes`/`no`.
fn flag(name: &str, value: Option<&str>) -> Result<bool, String> {
    match value.map(str::to_ascii_lowercase).as_deref() {
        None | Some("" | "1" | "true" | "yes" | "on") => Ok(true),
        Some("0" | "false" | "no" | "off") => Ok(false),
        Some(v) => Err(format!("goz: URL option {name}={v} is neither 1 nor 0")),
    }
}

/// Reads a `goz:` URL. An option goz-gui does not know, or a bad value, is
/// an error naming it.
pub fn parse(url: &str) -> Result<UrlSearch, String> {
    let url = url.trim();
    let url = url.split_once('#').map_or(url, |(before, _)| before);
    let (rest, slashes) = strip_scheme(url);
    let (search, options) = match rest.split_once('?') {
        Some((s, o)) => (s, Some(o)),
        None => (rest, None),
    };
    // A browser that was given `goz://text` may hand over `goz://text/`.
    let search = if slashes {
        search.strip_suffix('/').unwrap_or(search)
    } else {
        search
    };
    let mut out = UrlSearch {
        search: percent_decode(search),
        ..UrlSearch::default()
    };
    for option in options.unwrap_or("").split('&') {
        if option.is_empty() {
            continue;
        }
        let (name, value) = match option.split_once('=') {
            Some((n, v)) => (n, Some(percent_decode(v))),
            None => (option, None),
        };
        let name = percent_decode(name).to_ascii_lowercase();
        let value = value.as_deref();
        match name.as_str() {
            "case" | "matchcase" | "match-case" | "match_case" => {
                out.match_case = Some(flag(&name, value)?);
            }
            "nocase" => out.match_case = Some(!flag(&name, value)?),
            "path" | "matchpath" | "match-path" | "match_path" => {
                out.match_path = Some(flag(&name, value)?);
            }
            "nopath" | "nomatchpath" => out.match_path = Some(!flag(&name, value)?),
            "filter" => {
                let v = value.unwrap_or_default();
                out.filter = Some(
                    Filter::from_title(v)
                        .or_else(|| {
                            (v.is_empty() || v.eq_ignore_ascii_case("everything"))
                                .then_some(Filter::Everything)
                        })
                        .ok_or_else(|| format!("goz: URL filter {v} is not one goz-gui has"))?,
                );
            }
            "sort" => {
                let v = value.unwrap_or_default();
                out.sort = Some(
                    crate::cmdline::sort_by_name(v)
                        .ok_or_else(|| format!("goz: URL sort {v} is not one goz can sort by"))?,
                );
            }
            "ascending" | "sort-ascending" | "sort_ascending" => {
                out.dir = Some(if flag(&name, value)? {
                    SortDir::Asc
                } else {
                    SortDir::Desc
                });
            }
            "descending" | "sort-descending" | "sort_descending" => {
                out.dir = Some(if flag(&name, value)? {
                    SortDir::Desc
                } else {
                    SortDir::Asc
                });
            }
            _ => return Err(format!("goz: URL option {name} is not one goz-gui knows")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_is_decoded() {
        assert_eq!(parse("goz:report").unwrap().search, "report");
        assert_eq!(
            parse("goz:annual%20report%3F.pdf").unwrap().search,
            "annual report?.pdf"
        );
        // UTF-8, as browsers escape it; a lone % stays.
        assert_eq!(parse("goz:%C3%BCber").unwrap().search, "über");
        assert_eq!(parse("goz:100%").unwrap().search, "100%");
        assert_eq!(parse("goz:a%2").unwrap().search, "a%2");
        assert_eq!(parse("goz:a+b").unwrap().search, "a+b");
        assert_eq!(parse("goz:").unwrap(), UrlSearch::default());
    }

    #[test]
    fn schemes_and_slashes_come_off_but_not_drives() {
        assert_eq!(parse("GOZ:x").unwrap().search, "x");
        assert_eq!(parse("es:x").unwrap().search, "x");
        assert_eq!(parse("goz://x/").unwrap().search, "x");
        assert_eq!(parse("goz://x").unwrap().search, "x");
        assert_eq!(parse(r"goz:C:\Users\%20x").unwrap().search, r"C:\Users\ x");
        // No scheme at all: a drive's path is the search.
        assert_eq!(parse(r"C:\Windows").unwrap().search, r"C:\Windows");
        assert_eq!(parse("plain text").unwrap().search, "plain text");
        // A fragment is not part of the search.
        assert_eq!(parse("goz:x#report.pdf").unwrap().search, "x");
    }

    #[test]
    fn options_set_the_search_options() {
        let u = parse("goz:*.mp3?case&path&filter=Audio&sort=Size&ascending").unwrap();
        assert_eq!(u.search, "*.mp3");
        assert_eq!(u.match_case, Some(true));
        assert_eq!(u.match_path, Some(true));
        assert_eq!(u.filter, Some(Filter::Audio));
        assert_eq!(u.sort, Some(SortKey::Size));
        assert_eq!(u.dir, Some(SortDir::Asc));
        let u = parse("goz:x?CASE=0&nopath&sort=date%20modified&descending&filter=everything&")
            .unwrap();
        assert_eq!(u.match_case, Some(false));
        assert_eq!(u.match_path, Some(false));
        assert_eq!(u.sort, Some(SortKey::DateModified));
        assert_eq!(u.dir, Some(SortDir::Desc));
        assert_eq!(u.filter, Some(Filter::Everything));
        assert_eq!(parse("goz:x?ascending=0").unwrap().dir, Some(SortDir::Desc));
        // No options: nothing changes but the search.
        let u = parse("goz:x?").unwrap();
        assert_eq!((u.match_case, u.filter, u.sort), (None, None, None));
    }

    #[test]
    fn what_goz_gui_cannot_do_is_refused_by_name() {
        assert_eq!(
            parse("goz:x?regex").unwrap_err(),
            "goz: URL option regex is not one goz-gui knows"
        );
        assert!(parse("goz:x?sort=type").unwrap_err().contains("type"));
        assert!(
            parse("goz:x?filter=pictures")
                .unwrap_err()
                .contains("pictures")
        );
        assert!(
            parse("goz:x?case=maybe")
                .unwrap_err()
                .contains("case=maybe")
        );
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("%41%62c"), "Abc");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode(""), "");
    }
}
