//! Search > Everything, Audio, … Video: result filters, each a run of search
//! terms put in front of what the user typed. The extension lists are
//! Everything 1.5's own, so a filter finds the same files in both.

use crate::menu;

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
        }
    }

    /// The filter's name in Everything's lists, such as a bookmark's Filter
    /// column.
    pub fn title(self) -> &'static str {
        self.name().unwrap_or("Everything")
    }

    /// The filter named `title`, in any case.
    pub fn from_title(title: &str) -> Option<Filter> {
        let title = title.trim();
        Filter::ALL
            .into_iter()
            .find(|filter| filter.title().eq_ignore_ascii_case(title))
    }

    /// The Search menu's label, with its access key.
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
        }
    }

    pub fn menu_id(self) -> u16 {
        menu::FILTER_FIRST + self as u16
    }

    pub fn from_menu_id(id: u16) -> Option<Filter> {
        Filter::ALL
            .into_iter()
            .find(|filter| filter.menu_id() == id)
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
            Filter::Everything | Filter::Folder => None,
        }
    }

    /// The terms the filter adds to a search.
    fn terms(self) -> Option<String> {
        match self {
            Filter::Everything => None,
            Filter::Folder => Some("folder:".into()),
            _ => self.extensions().map(|exts| format!("ext:{exts}")),
        }
    }

    /// A term that makes gozd honour both the filter and the user's own
    /// terms where it would otherwise keep only the user's: gozd ANDs terms,
    /// but of several `ext:` it keeps the last, and the same for `file:` and
    /// `folder:`. Put last, this one wins.
    fn reconcile(self, text: &str) -> Option<String> {
        let funcs = functions(text);
        match self {
            Filter::Everything => None,
            Filter::Folder => funcs
                .iter()
                .any(|(name, _)| name == "file")
                .then(|| NOTHING.to_owned()),
            _ => {
                let ours = self.extensions()?;
                // gozd ignores an `ext:` with no extension in it.
                let theirs = funcs.iter().rev().find_map(|(name, value)| {
                    let exts: Vec<&str> = value
                        .split(';')
                        .map(|e| e.trim_start_matches('.'))
                        .filter(|e| !e.is_empty())
                        .collect();
                    (name == "ext" && !exts.is_empty()).then_some(exts)
                })?;
                let both: Vec<&str> = theirs
                    .into_iter()
                    .filter(|e| ours.split(';').any(|o| o.eq_ignore_ascii_case(e)))
                    .collect();
                Some(if both.is_empty() {
                    NOTHING.to_owned()
                } else {
                    format!("ext:{}", both.join(";"))
                })
            }
        }
    }
}

/// The search gozd gets for `text`: the filter's terms, Match Path's
/// directive, the text, and whatever keeps the filter in force beside the
/// user's own `ext:`/`file:`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use goz_core::query::{Kind, parse_query};

    fn exts(q: &str) -> Option<Vec<String>> {
        let parsed = parse_query(q).unwrap_or_else(|e| panic!("{q}: {e}"));
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
    fn menu_ids_round_trip() {
        for filter in Filter::ALL {
            assert_eq!(Filter::from_menu_id(filter.menu_id()), Some(filter));
        }
        assert_eq!(Filter::from_menu_id(menu::FILTER_FIRST + 8), None);
        assert_eq!(Filter::Everything.name(), None);
        assert_eq!(Filter::Folder.name(), Some("Folder"));
    }
}
