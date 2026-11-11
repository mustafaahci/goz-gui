//! Reading a file for `content:` terms, as Everything 1.5 reads one that no
//! IFilter or mail reader takes: the bytes, read as text in each encoding
//! the term names.
//!
//! - `content:` takes the file's byte order mark: UTF-8 (EF BB BF),
//!   UTF-16 (FF FE) or UTF-16 big-endian (FE FF). A file without one is
//!   read as ANSI (the system's code page), UTF-8, UTF-16 and UTF-16BE, and
//!   matches when the text is in any of them.
//! - `ansicontent:`, `utf8content:`, `utf16content:` and `utf16becontent:`
//!   read it in that encoding alone (its own byte order mark skipped).
//!
//! The text matches the way Everything's own matching does by default:
//! case-insensitively (`case:` or Match Case match case), diacritics-
//! insensitively (`diacritics:` matches them: é is not e), anywhere
//! (`wholeword:`/`ww:`: only between characters that are not a letter, a
//! digit or a combining mark, as gozd's whole words). The file is read in
//! 1 MB pieces with the text that may start a match carried over, and the
//! read ends as soon as every term is decided.

use std::io::Read;

use memchr::memmem::Finder;
use windows_sys::Win32::Globalization::{
    CP_ACP, CPINFO, GetCPInfo, IsDBCSLeadByteEx, MultiByteToWideChar,
};

/// How much of a file is read at a time.
const PIECE: usize = 1 << 20;

/// The encodings a content term reads a file in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// `content:`: by the byte order mark, else all four.
    Auto,
    Ansi,
    Utf8,
    Utf16,
    Utf16Be,
}

/// One content term: the text it looks for and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    pub encoding: Encoding,
    /// As typed, quotes taken off.
    pub text: String,
    pub case: bool,
    pub whole_word: bool,
    pub diacritics: bool,
}

/// How a text is folded before it is compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fold {
    case: bool,
    diacritics: bool,
}

/// Appends `units` (UTF-16) folded as `f` says: lower case unless `case`,
/// without diacritics unless `diacritics` (their combining marks left out
/// too, so a decomposed é is e). Lone surrogates and characters outside the
/// BMP pass as they are.
fn fold_into(units: &[u16], f: Fold, out: &mut Vec<u16>) {
    for &u in units {
        if u < 0x80 {
            out.push(if !f.case && (u as u8).is_ascii_uppercase() {
                u + 0x20
            } else {
                u
            });
            continue;
        }
        if (0xD800..0xE000).contains(&u) {
            out.push(u);
            continue;
        }
        if !f.diacritics && is_combining(u) {
            continue;
        }
        let mut c = char::from_u32(u32::from(u)).unwrap_or('\u{fffd}');
        if !f.diacritics {
            c = crate::filetools::rename::base_letter(c);
        }
        if !f.case {
            let mut lower = c.to_lowercase();
            if let (Some(l), None) = (lower.next(), lower.next()) {
                c = l;
            }
        }
        let mut buf = [0u16; 2];
        out.extend_from_slice(c.encode_utf16(&mut buf));
    }
}

/// The combining diacritical marks (their main blocks).
fn is_combining(u: u16) -> bool {
    matches!(u, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Whether UTF-16 unit `u` is part of a word: a letter, a digit or a
/// combining mark (a surrogate counts as a letter: a character beyond the
/// BMP is most likely one).
fn is_word(u: u16) -> bool {
    if (0xD800..0xE000).contains(&u) {
        return true;
    }
    is_combining(u) || char::from_u32(u32::from(u)).is_some_and(char::is_alphanumeric)
}

// ---- Decoders: bytes to UTF-16, a piece at a time ----

/// The system's ANSI code page: whether it has two-byte characters.
fn ansi_is_dbcs() -> bool {
    // SAFETY: CPINFO is plain data, filled by the call.
    let mut info: CPINFO = unsafe { core::mem::zeroed() };
    // SAFETY: an out-struct of the right type.
    (unsafe { GetCPInfo(CP_ACP, &mut info) }) != 0 && info.MaxCharSize > 1
}

/// Appends `bytes` decoded from the ANSI code page.
fn ansi_into(bytes: &[u8], out: &mut Vec<u16>) {
    if bytes.is_empty() {
        return;
    }
    let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    let at = out.len();
    out.resize(at + bytes.len(), 0);
    // SAFETY: the source and its length; the room for as many UTF-16
    // units as there are bytes, which a code page never exceeds.
    let n =
        unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, out[at..].as_mut_ptr(), len) };
    out.truncate(at + usize::try_from(n).unwrap_or(0));
}

enum Decoder {
    Ansi { dbcs: bool, carry: Option<u8> },
    Utf8 { carry: Vec<u8> },
    Utf16 { be: bool, carry: Option<u8> },
}

impl Decoder {
    fn new(e: Encoding, dbcs: bool) -> Decoder {
        match e {
            Encoding::Ansi | Encoding::Auto => Decoder::Ansi { dbcs, carry: None },
            Encoding::Utf8 => Decoder::Utf8 { carry: Vec::new() },
            Encoding::Utf16 => Decoder::Utf16 {
                be: false,
                carry: None,
            },
            Encoding::Utf16Be => Decoder::Utf16 {
                be: true,
                carry: None,
            },
        }
    }

    /// Appends `bytes` decoded to `out`; what may be the start of a
    /// character cut at the piece's end waits for the next (or `eof`).
    fn feed(&mut self, bytes: &[u8], eof: bool, out: &mut Vec<u16>) {
        match self {
            Decoder::Ansi { dbcs, carry } => {
                let joined: Vec<u8>;
                let mut b = bytes;
                if let Some(c) = carry.take() {
                    joined = [&[c][..], bytes].concat();
                    b = &joined;
                }
                if *dbcs && !eof {
                    // A lead byte at the very end waits for its trail.
                    let mut i = 0;
                    // SAFETY (each call): a plain query of the code page.
                    while i < b.len() {
                        i += if unsafe { IsDBCSLeadByteEx(CP_ACP, b[i]) } != 0 {
                            2
                        } else {
                            1
                        };
                    }
                    if i > b.len() {
                        *carry = b.last().copied();
                        b = &b[..b.len() - 1];
                    }
                }
                ansi_into(b, out);
            }
            Decoder::Utf8 { carry } => {
                let mut joined = std::mem::take(carry);
                joined.extend_from_slice(bytes);
                let mut b: &[u8] = &joined;
                loop {
                    match std::str::from_utf8(b) {
                        Ok(s) => {
                            out.extend(s.encode_utf16());
                            break;
                        }
                        Err(e) => {
                            let good = e.valid_up_to();
                            // SAFETY: from_utf8 said these bytes are UTF-8.
                            let s = unsafe { std::str::from_utf8_unchecked(&b[..good]) };
                            out.extend(s.encode_utf16());
                            match e.error_len() {
                                Some(bad) => {
                                    out.push(0xFFFD);
                                    b = &b[good + bad..];
                                }
                                None if eof => {
                                    out.push(0xFFFD);
                                    break;
                                }
                                None => {
                                    *carry = b[good..].to_vec();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Decoder::Utf16 { be, carry } => {
                let mut b = bytes;
                if let Some(c) = carry.take() {
                    if let Some((&first, rest)) = b.split_first() {
                        // The carried byte is the unit's first.
                        out.push(if *be {
                            u16::from_be_bytes([c, first])
                        } else {
                            u16::from_le_bytes([c, first])
                        });
                        b = rest;
                    } else {
                        *carry = Some(c);
                        return;
                    }
                }
                let (pairs, odd) = b.as_chunks::<2>();
                let odd = odd.first().copied();
                out.extend(pairs.iter().map(|&p| {
                    if *be {
                        u16::from_be_bytes(p)
                    } else {
                        u16::from_le_bytes(p)
                    }
                }));
                if !eof {
                    *carry = odd;
                }
            }
        }
    }
}

// ---- Matching ----

/// One needle, folded, and what was found of it.
struct Needle {
    finder: Finder<'static>,
    len: usize,
    whole_word: bool,
}

/// One way of reading the file (an encoding and a fold) and the terms
/// that read it so.
struct Stream {
    decoder: Decoder,
    fold: Fold,
    /// The terms reading it, by index.
    terms: Vec<usize>,
    /// Decoded text of the piece, and folded text: the carried tail first.
    raw: Vec<u16>,
    text: Vec<u16>,
    /// Whether `text` starts at the file's first character.
    at_start: bool,
    /// The longest needle: its length plus one is carried over.
    keep: usize,
}

/// The bytes of `units`, as memmem searches them.
fn bytes_of(units: &[u16]) -> &[u8] {
    // SAFETY: u16 has no padding, and u8's alignment is 1.
    unsafe { core::slice::from_raw_parts(units.as_ptr().cast::<u8>(), units.len() * 2) }
}

/// The needle's bytes, folded as `f` says, in native order.
fn needle_units(text: &str, f: Fold) -> Vec<u16> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::new();
    fold_into(&units, f, &mut out);
    out
}

impl Stream {
    /// Looks for its terms not found yet in its text; `eof`: the text ends
    /// the file. Sets `found` bits; keeps the tail for the next piece.
    fn search(&mut self, needles: &[Needle], found: &mut u32, eof: bool) {
        let hay = bytes_of(&self.text);
        for &t in &self.terms {
            if *found & (1 << t) != 0 {
                continue;
            }
            let n = &needles[t];
            if n.len == 0 {
                *found |= 1 << t;
                continue;
            }
            let mut from = 0;
            while let Some(i) = n.finder.find(&hay[from..]) {
                let at = from + i;
                from = at + 1;
                if at % 2 != 0 {
                    continue;
                }
                let p = at / 2;
                if n.whole_word {
                    let end = p + n.len;
                    let before_ok = if p == 0 {
                        self.at_start
                    } else {
                        !is_word(self.text[p - 1])
                    };
                    if !before_ok {
                        continue;
                    }
                    let after_ok = match self.text.get(end) {
                        Some(&u) => !is_word(u),
                        // Undecided until the next piece (carried over).
                        None => eof,
                    };
                    if !after_ok {
                        continue;
                    }
                }
                *found |= 1 << t;
                break;
            }
        }
        // Carry the tail that may start a match (and the character before
        // it, for whole words).
        let keep = self.keep + 1;
        if self.text.len() > keep {
            let cut = self.text.len() - keep;
            self.text.drain(..cut);
            self.at_start = false;
        }
    }
}

/// Why a file was not read.
#[derive(Debug, PartialEq, Eq)]
pub enum Unread {
    /// It is offline or in the cloud: reading it would download it.
    Offline,
    /// It could not be opened or read.
    Failed,
    /// The search was stopped.
    Stopped,
}

/// The file attributes that mean its content is not here.
const NOT_HERE: u32 = 0x1000 /* OFFLINE */ | 0x40000 /* RECALL_ON_OPEN */ | 0x400000 /* RECALL_ON_DATA_ACCESS */;

/// A reader of files for a set of terms (at most 32).
pub struct Scanner {
    terms: Vec<Term>,
    dbcs: bool,
}

impl Scanner {
    pub fn new(terms: Vec<Term>) -> Scanner {
        Scanner {
            terms,
            dbcs: ansi_is_dbcs(),
        }
    }

    /// Which of the terms the file at `path` holds (bit `i`: term `i`).
    pub fn scan_file(
        &self,
        path: &std::path::Path,
        stop: &dyn Fn() -> bool,
    ) -> Result<u32, Unread> {
        let all = all_bits(self.terms.len());
        // Empty texts hold in every file: no read.
        let empty = self
            .terms
            .iter()
            .enumerate()
            .filter(|(_, t)| t.text.is_empty())
            .fold(0u32, |m, (i, _)| m | 1 << i);
        if empty == all {
            return Ok(all);
        }
        if let Ok(meta) = std::fs::metadata(path) {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & NOT_HERE != 0 {
                return Err(Unread::Offline);
            }
        }
        let file = std::fs::File::open(path).map_err(|_| Unread::Failed)?;
        self.scan(file, stop).map(|f| f | empty)
    }

    /// Which of the terms `source` holds.
    pub fn scan(&self, mut source: impl Read, stop: &dyn Fn() -> bool) -> Result<u32, Unread> {
        let all = all_bits(self.terms.len());
        let mut found = 0u32;
        let mut piece = vec![0u8; PIECE];
        let mut first = true;
        let mut streams: Vec<Stream> = Vec::new();
        let mut needles: Vec<Needle> = Vec::new();
        loop {
            if stop() {
                return Err(Unread::Stopped);
            }
            let n = read_full(&mut source, &mut piece).map_err(|_| Unread::Failed)?;
            let eof = n < piece.len();
            let mut bytes = &piece[..n];
            if first {
                first = false;
                let bom = bom_of(bytes);
                (streams, needles) = self.streams_for(bom);
                // A byte order mark is not text.
                bytes = &bytes[bom.map_or(0, |b| b.len())..];
            }
            for s in &mut streams {
                if s.terms.iter().all(|&t| found & (1 << t) != 0) {
                    continue;
                }
                s.raw.clear();
                s.decoder.feed(bytes, eof, &mut s.raw);
                fold_into(&s.raw, s.fold, &mut s.text);
                s.search(&needles, &mut found, eof);
            }
            if found == all || eof {
                return Ok(found);
            }
        }
    }

    /// The streams to read a file whose byte order mark is `bom`, and each
    /// term's needle.
    fn streams_for(&self, bom: Option<Bom>) -> (Vec<Stream>, Vec<Needle>) {
        let mut streams: Vec<Stream> = Vec::new();
        let mut needles = Vec::new();
        for (i, t) in self.terms.iter().enumerate() {
            let fold = Fold {
                case: t.case,
                diacritics: t.diacritics,
            };
            let units = needle_units(&t.text, fold);
            needles.push(Needle {
                finder: Finder::new(bytes_of(&units)).into_owned(),
                len: units.len(),
                whole_word: t.whole_word,
            });
            let encodings: &[Encoding] = match (t.encoding, bom) {
                (Encoding::Auto, Some(b)) => &[b.encoding()][..],
                (Encoding::Auto, None) => &[
                    Encoding::Ansi,
                    Encoding::Utf8,
                    Encoding::Utf16,
                    Encoding::Utf16Be,
                ],
                (e, _) => match e {
                    Encoding::Ansi => &[Encoding::Ansi],
                    Encoding::Utf8 => &[Encoding::Utf8],
                    Encoding::Utf16 => &[Encoding::Utf16],
                    _ => &[Encoding::Utf16Be],
                },
            };
            for &e in encodings {
                let at = streams
                    .iter()
                    .position(|s| s.fold == fold && decoder_kind(&s.decoder) == e);
                let s = match at {
                    Some(at) => &mut streams[at],
                    None => {
                        streams.push(Stream {
                            decoder: Decoder::new(e, self.dbcs),
                            fold,
                            terms: Vec::new(),
                            raw: Vec::new(),
                            text: Vec::new(),
                            at_start: true,
                            keep: 0,
                        });
                        streams.last_mut().expect("just pushed")
                    }
                };
                s.terms.push(i);
                s.keep = s.keep.max(units.len());
            }
        }
        (streams, needles)
    }
}

fn decoder_kind(d: &Decoder) -> Encoding {
    match d {
        Decoder::Ansi { .. } => Encoding::Ansi,
        Decoder::Utf8 { .. } => Encoding::Utf8,
        Decoder::Utf16 { be: false, .. } => Encoding::Utf16,
        Decoder::Utf16 { be: true, .. } => Encoding::Utf16Be,
    }
}

fn all_bits(n: usize) -> u32 {
    if n >= 32 { u32::MAX } else { (1u32 << n) - 1 }
}

/// Reads until `buf` is full or the source ends.
fn read_full(source: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bom {
    Utf8,
    Utf16,
    Utf16Be,
}

impl Bom {
    fn len(self) -> usize {
        match self {
            Bom::Utf8 => 3,
            Bom::Utf16 | Bom::Utf16Be => 2,
        }
    }

    fn encoding(self) -> Encoding {
        match self {
            Bom::Utf8 => Encoding::Utf8,
            Bom::Utf16 => Encoding::Utf16,
            Bom::Utf16Be => Encoding::Utf16Be,
        }
    }
}

fn bom_of(b: &[u8]) -> Option<Bom> {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(Bom::Utf8)
    } else if b.starts_with(&[0xFF, 0xFE]) {
        Some(Bom::Utf16)
    } else if b.starts_with(&[0xFE, 0xFF]) {
        Some(Bom::Utf16Be)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(e: Encoding, text: &str) -> Term {
        Term {
            encoding: e,
            text: text.into(),
            case: false,
            whole_word: false,
            diacritics: false,
        }
    }

    fn holds(terms: Vec<Term>, bytes: &[u8]) -> u32 {
        Scanner::new(terms).scan(bytes, &|| false).unwrap()
    }

    fn utf16(s: &str, be: bool) -> Vec<u8> {
        s.encode_utf16()
            .flat_map(|u| if be { u.to_be_bytes() } else { u.to_le_bytes() })
            .collect()
    }

    #[test]
    fn content_reads_every_encoding_without_a_bom() {
        let t = || vec![term(Encoding::Auto, "Chicken Noodle")];
        assert_eq!(holds(t(), b"soup: chicken noodle!"), 1, "ASCII, any case");
        assert_eq!(holds(t(), "çorba chicken noodle".as_bytes()), 1, "UTF-8");
        assert_eq!(holds(t(), &utf16("x chicken noodle y", false)), 1, "UTF-16");
        assert_eq!(
            holds(t(), &utf16("x chicken noodle y", true)),
            1,
            "UTF-16BE"
        );
        assert_eq!(holds(t(), b"chicken soup"), 0);
    }

    #[test]
    fn a_bom_chooses_the_encoding_for_content() {
        let t = || vec![term(Encoding::Auto, "abc")];
        let mut le = vec![0xFF, 0xFE];
        le.extend(utf16("xxabc", false));
        assert_eq!(holds(t(), &le), 1);
        // With a UTF-16 BOM, its bytes are not read as ANSI: "a\0b\0c" as
        // ANSI is no match, nor big-endian.
        let mut be_as_le = vec![0xFF, 0xFE];
        be_as_le.extend(utf16("xxabc", true));
        assert_eq!(holds(t(), &be_as_le), 0);
        let mut u8bom = vec![0xEF, 0xBB, 0xBF];
        u8bom.extend("abç".bytes());
        assert_eq!(holds(vec![term(Encoding::Auto, "ABÇ")], &u8bom), 1);
    }

    #[test]
    fn the_named_encodings_read_only_theirs() {
        let le = utf16("hello world", false);
        assert_eq!(holds(vec![term(Encoding::Utf16, "world")], &le), 1);
        assert_eq!(holds(vec![term(Encoding::Utf16Be, "world")], &le), 0);
        assert_eq!(holds(vec![term(Encoding::Utf8, "world")], &le), 0);
        assert_eq!(
            holds(vec![term(Encoding::Utf8, "world")], b"hello world"),
            1
        );
        assert_eq!(
            holds(vec![term(Encoding::Ansi, "world")], b"hello world"),
            1
        );
        assert_eq!(
            holds(
                vec![term(Encoding::Utf16Be, "world")],
                &utf16("a world", true)
            ),
            1
        );
    }

    #[test]
    fn modifiers_case_whole_word_and_diacritics() {
        let mut t = term(Encoding::Auto, "Cafe");
        assert_eq!(holds(vec![t.clone()], "un café".as_bytes()), 1, "é is e");
        // Decomposed: e and a combining acute accent.
        assert_eq!(holds(vec![t.clone()], "un cafe\u{301} noir".as_bytes()), 1);
        t.diacritics = true;
        assert_eq!(holds(vec![t.clone()], "un café".as_bytes()), 0);
        assert_eq!(holds(vec![t.clone()], "un cafe".as_bytes()), 1);
        t.case = true;
        assert_eq!(holds(vec![t.clone()], b"un cafe"), 0);
        assert_eq!(holds(vec![t.clone()], b"un Cafe"), 1);
        let mut w = term(Encoding::Auto, "cat");
        w.whole_word = true;
        assert_eq!(holds(vec![w.clone()], b"concatenate"), 0);
        assert_eq!(holds(vec![w.clone()], b"a cat."), 1);
        assert_eq!(
            holds(vec![w.clone()], b"cat"),
            1,
            "the file's start and end"
        );
        assert_eq!(
            holds(vec![w.clone()], b"cat_x"),
            1,
            "_ ends a word, as in gozd"
        );
        assert_eq!(holds(vec![w], b"cats"), 0);
    }

    #[test]
    fn several_terms_are_answered_each() {
        let terms = vec![
            term(Encoding::Auto, "alpha"),
            term(Encoding::Utf8, "beta"),
            term(Encoding::Auto, "gamma"),
            term(Encoding::Auto, ""),
        ];
        assert_eq!(holds(terms.clone(), b"gamma alpha"), 0b1101);
        assert_eq!(holds(terms, b"beta"), 0b1010);
    }

    /// A match across the 1 MB pieces, whole words checked across them too.
    #[test]
    fn matches_across_pieces_are_found() {
        let mut data = vec![b'x'; PIECE - 3];
        data.extend_from_slice(b" needle here");
        assert_eq!(holds(vec![term(Encoding::Auto, "needle")], &data), 1);
        let mut w = term(Encoding::Auto, "needle");
        w.whole_word = true;
        assert_eq!(holds(vec![w.clone()], &data), 1);
        // The word goes on past the piece's end: no whole-word match.
        let mut data = vec![b' '; PIECE - 6];
        data.extend_from_slice(b"needles");
        assert_eq!(holds(vec![w.clone()], &data), 0);
        // A UTF-8 character cut by the pieces' edge.
        let mut data = vec![b' '; PIECE - 1];
        data.extend("ñandu".bytes());
        assert_eq!(holds(vec![term(Encoding::Utf8, "ÑANDU")], &data), 1);
        let mut data = vec![0u8, 0];
        data.extend(vec![b' '; PIECE - 3]);
        data.extend(utf16("ab", false));
        assert_eq!(
            holds(vec![term(Encoding::Utf16, "ab")], &data),
            0,
            "odd place"
        );
    }

    /// Pieces cut inside a character wait for the rest of it.
    #[test]
    fn decoders_carry_a_cut_character() {
        let decode = |e: Encoding, parts: &[&[u8]]| {
            let mut d = Decoder::new(e, false);
            let mut out = Vec::new();
            for (i, p) in parts.iter().enumerate() {
                d.feed(p, i + 1 == parts.len(), &mut out);
            }
            String::from_utf16_lossy(&out)
        };
        let le = utf16("ün", false);
        assert_eq!(
            decode(Encoding::Utf16, &[&le[..1], &le[1..3], &le[3..]]),
            "ün"
        );
        let be = utf16("ün", true);
        assert_eq!(decode(Encoding::Utf16Be, &[&be[..3], &be[3..]]), "ün");
        let u8 = "ñu".as_bytes();
        assert_eq!(decode(Encoding::Utf8, &[&u8[..1], &u8[1..]]), "ñu");
        // Bad bytes read as U+FFFD; a cut character at the end too.
        assert_eq!(
            decode(Encoding::Utf8, &[b"a\xffb", b"\xc3"]),
            "a\u{fffd}b\u{fffd}"
        );
    }

    #[test]
    fn a_stopped_read_says_so() {
        let s = Scanner::new(vec![term(Encoding::Auto, "x")]);
        assert_eq!(s.scan(&b"abc"[..], &|| true), Err(Unread::Stopped));
        let gone = std::path::Path::new("C:\\goz-gui-w9-no-such-file\\x.txt");
        assert_eq!(s.scan_file(gone, &|| false), Err(Unread::Failed));
    }
}
