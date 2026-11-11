//! Search > Everything, Audio, … Video: result filters, each a run of search
//! terms put in front of what the user typed. The extension lists are
//! Everything 1.5's own, so a filter finds the same files in both. After
//! them come the user's own filters (Search > Organize Filters...,
//! `crate::filters`), in the menus, the filter bar and Ctrl+] / Ctrl+[.

use crate::{filters, menu};

/// A result filter from the Search menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    /// No filter.
    #[default]
    Everything,
    Audio,
    Compressed,
    Document,
    Executable,
    Folder,
    Image,
    Video,
    /// One of the user's own filters, by its id in `crate::filters`, which
    /// names the same filter for as long as goz-gui runs.
    Custom(u16),
}

const AUDIO: &str = "aac;ac3;adt;adts;aif;aifc;aiff;amr;ape;au;cda;dts;ec3;fla;flac;lpcm;m1a;m2a;\
                     m3u;m3u8;m4a;m4b;m4p;mid;midi;mka;mp2;mp3;mpa;mpc;oga;ogg;opus;ra;rmi;snd;\
                     wav;wax;weba;wma";
const COMPRESSED: &str = "7z;ace;arj;bz2;cab;gz;gzip;jar;r00;r01;r02;r03;r04;r05;r06;r07;r08;\
                          r09;r10;r11;r12;r13;r14;r15;r16;r17;r18;r19;r20;r21;r22;r23;r24;r25;\
                          r26;r27;r28;r29;rar;tar;tgz;z;zip";
const DOCUMENT: &str = "asm;c;cc;chm;cpp;cs;css;csv;cxx;doc;docm;docx;dot;dotm;dotx;efu;epub;h;\
                        hpp;htm;html;hxx;ini;java;js;json;lua;md;mht;mhtml;mobi;odp;ods;odt;ofd;\
                        pdf;php;pl;potm;potx;ppam;pps;ppsm;ppsx;ppt;pptm;pptx;ps1xml;pssc;pub;py;\
                        rtf;sldm;sldx;sql;tsv;txt;vb;vsd;wpd;wps;wri;xlam;xls;xlsb;xlsm;xlsx;\
                        xltm;xltx;xml;xsl";
const EXECUTABLE: &str = "bat;cmd;exe;msi;msp;msu;ps1;scr;vbs";
const IMAGE: &str = "ani;apng;avif;avifs;bmp;bpg;cur;dds;gif;heic;heics;heif;heifs;hif;ico;jfi;\
                     jfif;jif;jpe;jpeg;jpg;jxl;jxr;pcx;png;psb;psd;svg;tga;tif;tiff;wdp;webp;wmf";
const VIDEO: &str = "3g2;3gp;3gp2;3gpp;amv;asf;asx;avi;bdmv;bik;d2v;divx;drc;dsa;dsm;dss;dsv;evo;\
                     f4v;flc;fli;flic;flv;hdmov;ifo;ivf;m1v;m2p;m2t;m2ts;m2v;m4v;mkv;mod;mov;mp2v;\
                     mp4;mp4v;mpe;mpeg;mpg;mpls;mpv2;mpv4;mts;ogm;ogv;ogx;pss;pva;qt;ram;ratdvd;\
                     rm;rmm;rmvb;roq;rpm;smil;smk;swf;tod;tp;tpr;ts;tts;uvu;vob;vp6;webm;wm;wmp;\
                     wmv;wmx;wvx";

/// An extension no name can have, since no name holds a backslash: the
/// search a filter and the user's own terms make when they contradict.
const NOTHING: &str = r"ext:\";

impl Filter {
    /// Every filter, in menu order.
    pub const ALL: [Filter; 8] = [
        Filter::Everything,
        Filter::Audio,
        Filter::Compressed,
        Filter::Document,
        Filter::Executable,
        Filter::Folder,
        Filter::Image,
        Filter::Video,
    ];

    /// Every filter the menus offer, in their order: the built-in ones,
    /// then the user's (`crate::filters::listed`).
    pub fn all() -> Vec<Filter> {
        let mut out = Filter::ALL.to_vec();
        out.extend(filters::listed().into_iter().map(Filter::Custom));
        out
    }

    /// A built-in filter's place in [`Filter::ALL`] (0 for a custom one).
    pub fn index(self) -> usize {
        match self {
            Filter::Everything | Filter::Custom(_) => 0,
            Filter::Audio => 1,
            Filter::Compressed => 2,
            Filter::Document => 3,
            Filter::Executable => 4,
            Filter::Folder => 5,
            Filter::Image => 6,
            Filter::Video => 7,
        }
    }

    /// What the status bar calls an active filter; `None` for no filter.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Filter::Everything => None,
            Filter::Audio => Some("Audio"),
            Filter::Compressed => Some("Compressed"),
            Filter::Document => Some("Document"),
            Filter::Executable => Some("Executable"),
            Filter::Folder => Some("Folder"),
            Filter::Image => Some("Image"),
            Filter::Video => Some("Video"),
            Filter::Custom(id) => Some(filters::name(id)),
        }
    }

    /// The filter's name in Everything's lists, such as a bookmark's Filter
    /// column.
    pub fn title(self) -> &'static str {
        self.name().unwrap_or("Everything")
    }

    /// The filter named `title`, in any case: a built-in one, else one of
    /// the user's that the menus offer.
    pub fn from_title(title: &str) -> Option<Filter> {
        let title = title.trim();
        Filter::ALL
            .into_iter()
            .find(|filter| filter.title().eq_ignore_ascii_case(title))
            .or_else(|| filters::find(title).map(Filter::Custom))
    }

    /// The Search menu's label, with its access key (a custom filter's
    /// name as it is, `&` shown as itself).
    pub fn label(self) -> &'static str {
        match self {
            Filter::Everything => "&Everything",
            Filter::Audio => "&Audio",
            Filter::Compressed => "&Compressed",
            Filter::Document => "&Document",
            Filter::Executable => "&Executable",
            Filter::Folder => "&Folder",
            Filter::Image => "&Image",
            Filter::Video => "&Video",
            Filter::Custom(id) => filters::label(id),
        }
    }

    /// The menu command that chooses it: a built-in's by its place, a
    /// custom one's by its place among those listed (0, no command, for
    /// one no longer listed).
    pub fn menu_id(self) -> u16 {
        match self {
            Filter::Custom(id) => filters::listed()
                .iter()
                .position(|&i| i == id)
                .filter(|&at| at < menu::CUSTOM_FILTERS)
                .map_or(0, |at| menu::CUSTOM_FILTER_FIRST + at as u16),
            f => menu::FILTER_FIRST + f.index() as u16,
        }
    }

    pub fn from_menu_id(id: u16) -> Option<Filter> {
        if let Some(at) = id.checked_sub(menu::CUSTOM_FILTER_FIRST)
            && usize::from(at) < menu::CUSTOM_FILTERS
        {
            return filters::listed()
                .get(usize::from(at))
                .map(|&i| Filter::Custom(i));
        }
        Filter::ALL
            .into_iter()
            .find(|filter| filter.menu_id() == id)
    }

    /// Search > Next Filter (`forward`) or Previous Filter: the one after or
    /// before in the menu, round from the last to the first (from a filter
    /// no longer listed, as from Everything).
    pub fn cycle(self, forward: bool) -> Filter {
        let all = Filter::all();
        let n = all.len();
        let at = all.iter().position(|&f| f == self).unwrap_or(0);
        all[if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        }]
    }

    /// A built-in filter's search, as Everything's Organize Filters shows
    /// it and `Filters.csv` keeps it; a custom filter's terms.
    pub fn search(self) -> String {
        self.terms().unwrap_or_default()
    }

    /// The extensions the filter keeps, `;`-separated.
    fn extensions(self) -> Option<&'static str> {
        match self {
            Filter::Audio => Some(AUDIO),
            Filter::Compressed => Some(COMPRESSED),
            Filter::Document => Some(DOCUMENT),
            Filter::Executable => Some(EXECUTABLE),
            Filter::Image => Some(IMAGE),
            Filter::Video => Some(VIDEO),
            Filter::Everything | Filter::Folder | Filter::Custom(_) => None,
        }
    }

    /// The terms the filter adds to a search.
    fn terms(self) -> Option<String> {
        match self {
            Filter::Everything => None,
            Filter::Folder => Some("folder:".into()),
            Filter::Custom(id) => filters::terms(id),
            _ => self.extensions().map(|exts| format!("ext:{exts}")),
        }
    }

    /// Terms that make gozd honour both the filter and the user's own
    /// terms where it would otherwise keep only the user's: gozd ANDs terms,
    /// but of several `ext:` it keeps the last, of `file:` and `folder:` the
    /// last, and of several `size:` the last. Put last, these win: the
    /// extensions both name, the sizes both allow, or nothing at all when
    /// the two contradict.
    fn reconcile(self, text: &str) -> Option<String> {
        reconcile(&self.terms()?, text)
    }
}

/// [`Filter::reconcile`] for a filter whose terms are `terms`.
fn reconcile(terms: &str, text: &str) -> Option<String> {
    let ours = parsed(terms)?;
    // A text gozd refuses is refused whatever comes after it.
    let theirs = parsed(text)?;
    if let (Some(a), Some(b)) = (ours.kind, theirs.kind)
        && a != b
    {
        return Some(NOTHING.to_owned());
    }
    let mut out = Vec::new();
    if let (Some(ours), Some(theirs)) = (&ours.ext, &theirs.ext) {
        let both: Vec<&str> = theirs
            .iter()
            .filter(|e| ours.contains(e))
            .map(String::as_str)
            .collect();
        if both.is_empty() {
            return Some(NOTHING.to_owned());
        }
        out.push(format!("ext:{}", both.join(";")));
    }
    if let (Some(a), Some(b)) = (ours.size, theirs.size) {
        let (min, max) = (a.0.max(b.0), a.1.min(b.1));
        if min > max {
            return Some(NOTHING.to_owned());
        }
        out.push(match (min, max) {
            (min, u64::MAX) => format!("size:>={min}"),
            (0, max) => format!("size:<={max}"),
            (min, max) => format!("size:{min}..{max}"),
        });
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// Files or folders only (`file:`, `folder:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Folder,
}

/// The parts of a search a filter's terms and the user's both may give,
/// of which gozd keeps only the last: its last `ext:` holding an
/// extension (lower-cased, without its dot), its kind, its last `size:` as
/// an inclusive byte range.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parts {
    ext: Option<Vec<String>>,
    kind: Option<Kind>,
    size: Option<(u64, u64)>,
}

/// Whether gozd refuses `text` for an operator it does not take: an
/// unquoted `|` or `!`.
fn has_operator(text: &str) -> bool {
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '|' | '!' if !quoted => return true,
            _ => {}
        }
    }
    false
}

/// [`Parts`] of `text`, read as goz's parser reads it; `None` for a text it
/// refuses (an operator, an unknown function, a `size:` it cannot read).
fn parsed(text: &str) -> Option<Parts> {
    if has_operator(text) {
        return None;
    }
    let mut p = Parts::default();
    for (name, value) in functions(text) {
        match name.as_str() {
            "ext" => {
                let exts: Vec<String> = value
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(|e| e.trim_start_matches('.').to_lowercase())
                    .collect();
                if !exts.is_empty() {
                    p.ext = Some(exts);
                }
            }
            "size" => p.size = Some(size_range(&value)?),
            "file" => p.kind = Some(Kind::File),
            "folder" | "dir" => p.kind = Some(Kind::Folder),
            "dm" | "datemodified" | "path" | "case" | "matchpath" => {}
            _ => return None,
        }
    }
    Some(p)
}

/// A `size:` value as goz reads it, as an inclusive byte range: a named
/// size (`tiny`, ...), a range `a..b`, a comparison, or a size (1024-based
/// `kb`, `mb`, `gb`, `tb`, or `b`).
fn size_range(value: &str) -> Option<(u64, u64)> {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    let v = value.trim();
    let named = match v.to_ascii_lowercase().as_str() {
        "empty" => Some((0, 0)),
        "tiny" => Some((0, 10 * KB)),
        "small" => Some((10 * KB, 100 * KB)),
        "medium" => Some((100 * KB, MB)),
        "large" => Some((MB, 16 * MB)),
        "huge" => Some((16 * MB, 128 * MB)),
        "gigantic" => Some((128 * MB, u64::MAX)),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some((lo, hi)) = v.split_once("..") {
        let lo = if lo.trim().is_empty() { 0 } else { bytes(lo)? };
        let hi = if hi.trim().is_empty() {
            u64::MAX
        } else {
            bytes(hi)?
        };
        return Some((lo, hi));
    }
    for op in [">=", "<=", ">", "<"] {
        if let Some(rest) = v.strip_prefix(op) {
            let n = bytes(rest)?;
            return Some(match op {
                ">=" => (n, u64::MAX),
                ">" => (n.saturating_add(1), u64::MAX),
                "<=" => (0, n),
                _ => (0, n.saturating_sub(1)),
            });
        }
    }
    let n = bytes(v.strip_prefix('=').unwrap_or(v))?;
    Some((n, n))
}

/// `10mb`, `1024`, `2gb` as bytes (1024-based).
fn bytes(s: &str) -> Option<u64> {
    let lower = s.trim().to_ascii_lowercase();
    let units = [
        ("tb", 1u64 << 40),
        ("gb", 1 << 30),
        ("mb", 1 << 20),
        ("kb", 1 << 10),
        ("b", 1),
    ];
    let (num, mult) = units
        .iter()
        .find_map(|&(u, m)| lower.strip_suffix(u).map(|n| (n.to_owned(), m)))
        .unwrap_or((lower.clone(), 1));
    num.trim().parse::<u64>().ok()?.checked_mul(mult)
}

/// The `name:value` functions in `text`, names lower-cased, split the way
/// goz's tokenizer splits them: on spaces outside quotes, quotes dropped, and
/// a token is a function when an unquoted colon follows a name of letters.
fn functions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut in_quotes, mut quoted, mut colon) = (false, false, None);
    let mut flush = |cur: &mut String, colon: &mut Option<usize>| {
        if let Some(at) = colon.take() {
            let name: &str = &cur[..at];
            if name.len() > 1 && name.bytes().all(|b| b.is_ascii_alphabetic()) {
                out.push((name.to_ascii_lowercase(), cur[at + 1..].to_owned()));
            }
        }
        cur.clear();
    };
    for ch in text.chars() {
        if in_quotes {
            if ch == '"' {
                in_quotes = false;
            } else {
                cur.push(ch);
            }
            continue;
        }
        match ch {
            '"' => {
                in_quotes = true;
                quoted = true;
            }
            c if c.is_whitespace() => {
                flush(&mut cur, &mut colon);
                quoted = false;
            }
            c => {
                if c == ':' && !quoted && colon.is_none() {
                    colon = Some(cur.len());
                }
                cur.push(c);
            }
        }
    }
    flush(&mut cur, &mut colon);
    out
}

/// The search gozd gets for `text`: the filter's terms, Match Path's
/// directive, the text, and whatever keeps the filter in force beside the
/// user's own `ext:`, `file:`/`folder:` and `size:`.
pub fn query(filter: Filter, match_path: bool, text: &str) -> String {
    let mut q = String::new();
    if let Some(terms) = filter.terms() {
        q.push_str(&terms);
        q.push(' ');
    }
    if match_path {
        q.push_str("matchpath: ");
    }
    q.push_str(text);
    if let Some(term) = filter.reconcile(text) {
        q.push(' ');
        q.push_str(&term);
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::{Kind, parse_query};

    /// `q` as gozd v0.6.0 reads its functions: a later `ext:` or `size:`
    /// replaces an earlier one, so the reconciled term goz-gui appends is
    /// the one that counts. (The new parser, goz-core's now, ANDs them all,
    /// which the appended intersection leaves the same.) Each word that
    /// starts a function is kept only as the last of its name.
    fn v060(q: &str) -> String {
        let words: Vec<&str> = q.split_whitespace().collect();
        let func = |w: &str| {
            let w = w.to_ascii_lowercase();
            ["ext:", "size:"]
                .into_iter()
                .find(|f| w.starts_with(f))
                .map(str::to_owned)
        };
        let kept: Vec<&str> = words
            .iter()
            .enumerate()
            .filter(|&(i, w)| match func(w) {
                Some(f) => !words[i + 1..]
                    .iter()
                    .any(|later| func(later).as_ref() == Some(&f)),
                None => true,
            })
            .map(|(_, w)| *w)
            .collect();
        kept.join(" ")
    }

    fn exts(q: &str) -> Option<Vec<String>> {
        let q = v060(q);
        let parsed = parse_query(&q).unwrap_or_else(|e| panic!("{q}: {e}"));
        parsed.filters.ext.map(|v| {
            v.into_iter()
                .map(|e| String::from_utf8(e).expect("ascii"))
                .collect()
        })
    }

    #[test]
    fn every_filter_parses_and_keeps_its_extensions() {
        for filter in Filter::ALL {
            for text in ["", "report", r"C:\Users\ foo*.x", "size:>1mb"] {
                let q = query(filter, false, text);
                let parsed = parse_query(&q).unwrap_or_else(|e| panic!("{q}: {e}"));
                match filter.extensions() {
                    Some(list) => {
                        let want: Vec<&str> = list.split(';').collect();
                        assert_eq!(exts(&q).expect("ext filter"), want, "{filter:?}");
                    }
                    None => assert_eq!(parsed.filters.ext, None, "{filter:?}"),
                }
                let kind = (filter == Filter::Folder).then_some(Kind::Folder);
                assert_eq!(parsed.filters.kind, kind, "{filter:?}");
            }
        }
    }

    #[test]
    fn the_lists_are_everythings() {
        let count = |f: Filter| f.extensions().map_or(0, |e| e.split(';').count());
        assert_eq!(count(Filter::Audio), 40);
        assert_eq!(count(Filter::Compressed), 43);
        assert_eq!(count(Filter::Document), 71);
        assert_eq!(count(Filter::Executable), 9);
        assert_eq!(count(Filter::Image), 34);
        assert_eq!(count(Filter::Video), 75);
        for filter in Filter::ALL {
            if let Some(list) = filter.extensions() {
                assert!(!list.contains(' ') && !list.contains(";;"), "{filter:?}");
            }
        }
    }

    #[test]
    fn user_terms_are_anded_with_the_filter() {
        let q = query(Filter::Audio, false, "live");
        let parsed = parse_query(&q).expect("parses");
        assert_eq!(parsed.name_terms, [b"live".to_vec()]);
        assert!(parsed.filters.ext.is_some_and(|e| e.len() == 40));
        // The terms gozd allows leave plenty for the user's.
        let many = vec!["x"; 150].join(" ");
        assert!(parse_query(&query(Filter::Video, false, &many)).is_ok());
    }

    #[test]
    fn the_users_ext_meets_the_filters() {
        // Both lists hold mp3: only mp3 is left.
        assert_eq!(
            exts(&query(Filter::Audio, false, "ext:mp3;pdf live")),
            Some(vec!["mp3".into()])
        );
        assert_eq!(
            exts(&query(Filter::Audio, false, "ext:.MP3")),
            Some(vec!["mp3".into()])
        );
        // Nothing in common: nothing matches.
        assert_eq!(
            exts(&query(Filter::Audio, false, "ext:pdf")),
            Some(vec!["\\".into()])
        );
        // An empty ext: is ignored by gozd, so the filter stands alone.
        assert_eq!(
            query(Filter::Audio, false, "ext: x"),
            format!("ext:{AUDIO} ext: x")
        );
        // A quoted "ext:pdf" is text, not a function.
        assert_eq!(
            query(Filter::Audio, false, "\"ext:pdf\""),
            format!("ext:{AUDIO} \"ext:pdf\"")
        );
    }

    #[test]
    fn folder_filter_and_file_contradict() {
        let q = query(Filter::Folder, false, "file:x");
        assert_eq!(exts(&q), Some(vec!["\\".into()]));
        let q = query(Filter::Folder, false, "dir:x");
        assert_eq!(q, "folder: dir:x");
        // Under an extension filter, folder: narrows it as in Everything.
        let parsed = parse_query(&query(Filter::Image, false, "folder:")).expect("parses");
        assert_eq!(parsed.filters.kind, Some(Kind::Folder));
    }

    #[test]
    fn match_path_and_no_filter() {
        assert_eq!(query(Filter::Everything, false, "a b"), "a b");
        assert_eq!(query(Filter::Everything, true, "a"), "matchpath: a");
        assert_eq!(query(Filter::Folder, true, "a"), "folder: matchpath: a");
    }

    #[test]
    fn a_custom_filters_terms_meet_the_users() {
        let q = |terms: &str, text: &str| {
            let mut q = format!("{terms} {text}");
            if let Some(t) = reconcile(terms, text) {
                q.push(' ');
                q.push_str(&t);
            }
            q
        };
        let parse = |s: &str| parse_query(&v060(s)).unwrap_or_else(|e| panic!("{s}: {e}"));
        // Sizes: what both allow.
        let p = parse(&q("size:>1mb", "size:<10mb"));
        let size = p.filters.size.expect("size");
        assert_eq!(
            (size.min, size.max),
            (1024 * 1024 + 1, 10 * 1024 * 1024 - 1)
        );
        let p = parse(&q("size:>1mb", "size:>5mb"));
        assert_eq!(p.filters.size.map(|s| s.min), Some(5 * 1024 * 1024 + 1));
        assert_eq!(p.filters.size.map(|s| s.max), Some(u64::MAX));
        // None at all: nothing matches.
        assert_eq!(exts(&q("size:>1mb", "size:<1kb")), Some(vec!["\\".into()]));
        // Kinds that contradict, and extensions.
        assert_eq!(exts(&q("file: report", "folder:")), Some(vec!["\\".into()]));
        assert_eq!(reconcile("folder:", "dir:x"), None);
        assert_eq!(
            exts(&q("ext:mp3;flac size:>1mb", "ext:FLAC;wav")),
            Some(vec!["flac".into()])
        );
        // Nothing of the user's to meet: the filter's terms alone.
        assert_eq!(reconcile("ext:mp3 size:>1mb", "live"), None);
        // A user's text gozd refuses: left for gozd to say why.
        assert_eq!(reconcile("ext:mp3", "a|b ext:pdf"), None);
    }

    #[test]
    fn parts_are_read_as_goz_reads_them() {
        use goz_core::query::{LocalClock, parse_query_at};
        struct Clock;
        impl LocalClock for Clock {
            fn now(&self) -> i64 {
                133_801_632_000_000_000
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
            fn date_order(&self) -> goz_core::query::DateOrder {
                goz_core::query::DateOrder::MonthDayYear
            }
        }
        for text in [
            "",
            "report",
            "ext:MP3;.flac x ext:pdf",
            "ext: x",
            "ext:;",
            "\"ext:pdf\" a",
            "file: size:>1mb",
            "folder:x dir:y",
            "size:1kb..2MB",
            "size:..500",
            "size:=12",
            "size:<10kb size:>=3gb",
            "size:>1mb dm:lastweek case: path:x matchpath:",
            "a|b",
            "!x",
            "\"a|b\" ext:txt",
            "parent:x",
            "size:abc",
            r"C:\Windows 12:30 *.rs",
        ] {
            let ours = parsed(text);
            // What goz-gui does not read (an operator, a function gozd
            // v0.6.0 lacks, a value it refuses) is left to gozd as it is;
            // the new parser may read more of it.
            if ours.is_none() {
                continue;
            }
            let goz = parse_query_at(&v060(text), false, &Clock)
                .ok()
                .map(|q| Parts {
                    ext: q.filters.ext.map(|v| {
                        v.into_iter()
                            .map(|e| String::from_utf8(e).expect("ascii"))
                            .collect()
                    }),
                    kind: q.filters.kind.map(|k| match k {
                        goz_core::query::Kind::File => super::Kind::File,
                        goz_core::query::Kind::Folder => super::Kind::Folder,
                    }),
                    size: q.filters.size.map(|s| (s.min, s.max)),
                });
            assert_eq!(ours, goz, "{text}");
        }
    }

    /// Where the readings of the size words part: v0.6.0's took their lower
    /// bound in (tiny took empty files), the new parser's (Everything's)
    /// leaves it out. The range goz-gui appends for v0.6.0 is ANDed with
    /// the words themselves by the new parser, so its answer is the new
    /// parser's.
    #[test]
    fn size_words_are_read_as_each_gozd_reads_them() {
        let new = |q: &str| {
            parse_query(q)
                .expect("parses")
                .filters
                .size
                .map(|s| (s.min, s.max))
        };
        assert_eq!(parsed("size:tiny").and_then(|p| p.size), Some((0, 10240)));
        assert_eq!(new("size:tiny"), Some((1, 10240)));
        assert_eq!(
            parsed("size:huge").and_then(|p| p.size),
            Some((16 << 20, 128 << 20))
        );
        assert_eq!(new("size:huge"), Some(((16 << 20) + 1, 128 << 20)));
    }

    #[test]
    fn functions_split_as_goz_does() {
        assert_eq!(
            functions(r#"ext:"mp3;wav" a "b:c" C:\x size:>1 12:30"#),
            [
                ("ext".to_owned(), "mp3;wav".to_owned()),
                ("size".to_owned(), ">1".to_owned()),
            ]
        );
        assert_eq!(functions("EXT:Pdf"), [("ext".to_owned(), "Pdf".to_owned())]);
    }

    #[test]
    fn filters_cycle_round() {
        assert_eq!(Filter::Everything.cycle(true), Filter::Audio);
        assert_eq!(Filter::Video.cycle(true), Filter::Everything);
        assert_eq!(Filter::Everything.cycle(false), Filter::Video);
        assert_eq!(Filter::Audio.cycle(false), Filter::Everything);
        for f in Filter::ALL {
            assert_eq!(f.cycle(true).cycle(false), f);
        }
        let mut f = Filter::Image;
        for _ in 0..Filter::ALL.len() {
            f = f.cycle(true);
        }
        assert_eq!(f, Filter::Image);
    }

    #[test]
    fn menu_ids_round_trip() {
        for filter in Filter::ALL {
            assert_eq!(Filter::from_menu_id(filter.menu_id()), Some(filter));
        }
        assert_eq!(Filter::from_menu_id(menu::FILTER_FIRST + 8), None);
        assert_eq!(Filter::Everything.name(), None);
        assert_eq!(Filter::Folder.name(), Some("Folder"));
    }
}
