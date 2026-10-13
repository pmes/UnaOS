//! AETHERWPT3 (LEDGER SR138): the Encoding Standard's legacy decoders (WHATWG Encoding §9 single-byte, §10 gb18030 /
//! GBK, §11 Big5, §12 EUC-JP / ISO-2022-JP / Shift_JIS, §13 EUC-KR) over the generated §5 indexes ([`index`],
//! from the WHATWG index JSON by `gen_index.py`), the §4.2 label table, and §6 "decode" for a whole byte buffer
//! (BOM sniffing, UTF-8 / UTF-16 / windows-1252 / x-user-defined / replacement beside the legacy codecs).
//!
//! Two consumers: `TextDecoder` (api/platform.rs's prelude streams through `__codec_decode`, carrying a
//! decoder's state between calls as four integers), and the document decoder (dom/charset.rs, HTML §13.2.3).
//!
//! Each decoder is the specification's handler, byte by byte: "prepend to the queue" is a push-back stack read
//! before the rest of the input; an error is one U+FFFD (the replacement error mode) and is counted, so a
//! `fatal` caller can refuse the output.

#[rustfmt::skip]
pub mod index;

/// The encoding a label names (Encoding §4.2 "get an encoding": ASCII whitespace stripped, ASCII case-insensitive),
/// as its canonical name (`Shift_JIS`, `windows-1252`, …).
pub fn get_encoding(label: &str) -> Option<&'static str> {
    let l = label.trim_matches(|c| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')).to_ascii_lowercase();
    index::LABELS.iter().find(|(_, labels)| labels.contains(&l.as_str())).map(|(name, _)| *name)
}

/// A legacy decoder's state between calls (Encoding §4.1 "decoder"): the lead byte(s) and flags of the
/// algorithm, serialized as four integers for the page's TextDecoder to hold.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct DecoderState(pub [u32; 4]);

/// What a run produced: UTF-16 code units (errors as U+FFFD) and how many errors there were.
pub struct Decoded {
    pub units: Vec<u16>,
    pub errors: usize,
}

const ESC: u8 = 0x1B;

/// ISO-2022-JP decoder states (Encoding §12.2.1).
const J_ASCII: u32 = 0;
const J_ROMAN: u32 = 1;
const J_KATAKANA: u32 = 2;
const J_LEAD: u32 = 3;
const J_TRAIL: u32 = 4;
const J_ESCAPE_START: u32 = 5;
const J_ESCAPE: u32 = 6;

fn single_byte_row(name: &str) -> Option<usize> {
    let n = if name == "iso-8859-8-i" { "iso-8859-8" } else { name };
    index::SINGLE_BYTE_NAMES.iter().position(|x| *x == n)
}

/// True for the encodings [`decode_chunk`] carries (every legacy encoding; UTF-8, UTF-16, windows-1252 and
/// x-user-defined stay with the page's own decoders).
pub fn is_legacy(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    (single_byte_row(&n).is_some() && n != "windows-1252")
        || matches!(n.as_str(), "gbk" | "gb18030" | "big5" | "euc-jp" | "iso-2022-jp" | "shift_jis" | "euc-kr")
}

fn at16(t: &[u16], i: usize) -> Option<u32> {
    t.get(i).copied().filter(|&c| c != 0).map(u32::from)
}

/// Encoding §5 "index gb18030 ranges code point".
fn gb18030_ranges(pointer: u32) -> Option<u32> {
    if (pointer > 39419 && pointer < 189000) || pointer > 1237575 {
        return None;
    }
    if pointer == 7457 {
        return Some(0xE7C7);
    }
    let (off, cp) = index::GB18030_RANGES.iter().rev().find(|(p, _)| *p <= pointer).copied()?;
    Some(cp + pointer - off)
}

enum Step {
    Continue,
    Emit(u32),
    Emit2(u32, u32),
    Error,
    Finished,
}

/// Runs the decoder named `name` (a canonical or lowercase name for which [`is_legacy`] holds) over `bytes`,
/// starting from `state`; `flush` = the end of the stream follows the bytes (TextDecoder without
/// `{stream: true}`). Returns the output and the state to resume from.
pub fn decode_chunk(name: &str, state: DecoderState, bytes: &[u8], flush: bool) -> (Decoded, DecoderState) {
    let n = name.to_ascii_lowercase();
    let mut st = state.0;
    let mut out = Decoded { units: Vec::with_capacity(bytes.len()), errors: 0 };
    let mut pushback: Vec<u8> = Vec::new();
    let mut i = 0usize;
    let row = single_byte_row(&n);
    let push = |out: &mut Decoded, cp: u32| {
        if cp > 0xFFFF {
            let c = cp - 0x10000;
            out.units.push(0xD800 + (c >> 10) as u16);
            out.units.push(0xDC00 + (c & 0x3FF) as u16);
        } else {
            out.units.push(cp as u16);
        }
    };
    loop {
        // the next item: a pushed-back byte, an input byte, end-of-queue (when flushing), or stop.
        let byte: Option<u8> = if let Some(b) = pushback.pop() {
            Some(b)
        } else if i < bytes.len() {
            i += 1;
            Some(bytes[i - 1])
        } else if flush {
            None
        } else {
            break;
        };
        let mut prepend: Vec<u8> = Vec::new();
        let step = match (n.as_str(), row) {
            (_, Some(r)) if n != "windows-1252" => match byte {
                None => Step::Finished,
                Some(b) if b < 0x80 => Step::Emit(b as u32),
                Some(b) => match index::SINGLE_BYTE[r][(b - 0x80) as usize] {
                    0 => Step::Error,
                    cp => Step::Emit(cp as u32),
                },
            },
            ("gbk" | "gb18030", _) => {
                // st = [first, second, third, _]
                let (first, second, third) = (st[0], st[1], st[2]);
                match byte {
                    None if first == 0 && second == 0 && third == 0 => Step::Finished,
                    None => {
                        st[0] = 0;
                        st[1] = 0;
                        st[2] = 0;
                        Step::Error
                    }
                    Some(b) if third != 0 => {
                        if !(0x30..=0x39).contains(&b) {
                            prepend = vec![second as u8, third as u8, b];
                            st[0] = 0;
                            st[1] = 0;
                            st[2] = 0;
                            Step::Error
                        } else {
                            let pointer = (first - 0x81) * (10 * 126 * 10) + (second - 0x30) * (10 * 126) + (third - 0x81) * 10 + b as u32 - 0x30;
                            st[0] = 0;
                            st[1] = 0;
                            st[2] = 0;
                            match gb18030_ranges(pointer) {
                                Some(cp) => Step::Emit(cp),
                                None => Step::Error,
                            }
                        }
                    }
                    Some(b) if second != 0 => {
                        if (0x81..=0xFE).contains(&b) {
                            st[2] = b as u32;
                            Step::Continue
                        } else {
                            prepend = vec![second as u8, b];
                            st[0] = 0;
                            st[1] = 0;
                            Step::Error
                        }
                    }
                    Some(b) if first != 0 => {
                        if (0x30..=0x39).contains(&b) {
                            st[1] = b as u32;
                            Step::Continue
                        } else {
                            let lead = first;
                            st[0] = 0;
                            let offset = if b < 0x7F { 0x40 } else { 0x41 };
                            let pointer = if (0x40..=0x7E).contains(&b) || (0x80..=0xFE).contains(&b) {
                                Some((lead - 0x81) * 190 + (b as u32 - offset))
                            } else {
                                None
                            };
                            match pointer.and_then(|p| at16(&index::GB18030, p as usize)) {
                                Some(cp) => Step::Emit(cp),
                                None => {
                                    if b < 0x80 {
                                        prepend = vec![b];
                                    }
                                    Step::Error
                                }
                            }
                        }
                    }
                    Some(b) if b < 0x80 => Step::Emit(b as u32),
                    Some(0x80) => Step::Emit(0x20AC),
                    Some(b) if (0x81..=0xFE).contains(&b) => {
                        st[0] = b as u32;
                        Step::Continue
                    }
                    Some(_) => Step::Error,
                }
            }
            ("big5", _) => {
                let lead = st[0];
                match byte {
                    None if lead != 0 => {
                        st[0] = 0;
                        Step::Error
                    }
                    None => Step::Finished,
                    Some(b) if lead != 0 => {
                        st[0] = 0;
                        let offset = if b < 0x7F { 0x40 } else { 0x62 };
                        let pointer = if (0x40..=0x7E).contains(&b) || (0xA1..=0xFE).contains(&b) {
                            Some((lead - 0x81) * 157 + (b as u32 - offset))
                        } else {
                            None
                        };
                        match pointer {
                            Some(1133) => Step::Emit2(0xCA, 0x304),
                            Some(1135) => Step::Emit2(0xCA, 0x30C),
                            Some(1164) => Step::Emit2(0xEA, 0x304),
                            Some(1166) => Step::Emit2(0xEA, 0x30C),
                            p => match p.and_then(|p| index::BIG5.get(p as usize).copied().filter(|&c| c != 0)) {
                                Some(cp) => Step::Emit(cp),
                                None => {
                                    if b < 0x80 {
                                        prepend = vec![b];
                                    }
                                    Step::Error
                                }
                            },
                        }
                    }
                    Some(b) if b < 0x80 => Step::Emit(b as u32),
                    Some(b) if (0x81..=0xFE).contains(&b) => {
                        st[0] = b as u32;
                        Step::Continue
                    }
                    Some(_) => Step::Error,
                }
            }
            ("euc-jp", _) => {
                // st = [lead, jis0212 flag, _, _]
                let lead = st[0];
                match byte {
                    None if lead != 0 => {
                        st[0] = 0;
                        Step::Error
                    }
                    None => Step::Finished,
                    Some(b) if lead == 0x8E && (0xA1..=0xDF).contains(&b) => {
                        st[0] = 0;
                        Step::Emit(0xFF61 - 0xA1 + b as u32)
                    }
                    Some(b) if lead == 0x8F && (0xA1..=0xFE).contains(&b) => {
                        st[1] = 1;
                        st[0] = b as u32;
                        Step::Continue
                    }
                    Some(b) if lead != 0 => {
                        st[0] = 0;
                        let mut cp = None;
                        if (0xA1..=0xFE).contains(&lead) && (0xA1..=0xFE).contains(&b) {
                            let pointer = ((lead - 0xA1) * 94 + b as u32 - 0xA1) as usize;
                            cp = if st[1] == 1 { at16(&index::JIS0212, pointer) } else { at16(&index::JIS0208, pointer) };
                        }
                        st[1] = 0;
                        match cp {
                            Some(cp) => Step::Emit(cp),
                            None => {
                                if b < 0x80 {
                                    prepend = vec![b];
                                }
                                Step::Error
                            }
                        }
                    }
                    Some(b) if b < 0x80 => Step::Emit(b as u32),
                    Some(b) if b == 0x8E || b == 0x8F || (0xA1..=0xFE).contains(&b) => {
                        st[0] = b as u32;
                        Step::Continue
                    }
                    Some(_) => Step::Error,
                }
            }
            ("iso-2022-jp", _) => {
                // st = [decoder state, output state, lead, output flag]
                let ds = st[0];
                match ds {
                    J_ASCII | J_ROMAN | J_KATAKANA | J_LEAD => match byte {
                        Some(ESC) => {
                            st[0] = J_ESCAPE_START;
                            Step::Continue
                        }
                        None => Step::Finished,
                        Some(b) => {
                            st[3] = 0;
                            match ds {
                                J_ASCII if b <= 0x7F && b != 0x0E && b != 0x0F => Step::Emit(b as u32),
                                J_ROMAN if b == 0x5C => Step::Emit(0xA5),
                                J_ROMAN if b == 0x7E => Step::Emit(0x203E),
                                J_ROMAN if b <= 0x7F && b != 0x0E && b != 0x0F => Step::Emit(b as u32),
                                J_KATAKANA if (0x21..=0x5F).contains(&b) => Step::Emit(0xFF61 - 0x21 + b as u32),
                                J_LEAD if (0x21..=0x7E).contains(&b) => {
                                    st[2] = b as u32;
                                    st[0] = J_TRAIL;
                                    Step::Continue
                                }
                                _ => Step::Error,
                            }
                        }
                    },
                    J_TRAIL => match byte {
                        Some(ESC) => {
                            st[0] = J_ESCAPE_START;
                            Step::Error
                        }
                        Some(b) if (0x21..=0x7E).contains(&b) => {
                            st[0] = J_LEAD;
                            let pointer = ((st[2] - 0x21) * 94 + b as u32 - 0x21) as usize;
                            match at16(&index::JIS0208, pointer) {
                                Some(cp) => Step::Emit(cp),
                                None => Step::Error,
                            }
                        }
                        None => {
                            st[0] = J_LEAD;
                            Step::Error
                        }
                        Some(_) => {
                            st[0] = J_LEAD;
                            Step::Error
                        }
                    },
                    J_ESCAPE_START => match byte {
                        Some(b) if b == 0x24 || b == 0x28 => {
                            st[2] = b as u32;
                            st[0] = J_ESCAPE;
                            Step::Continue
                        }
                        other => {
                            if let Some(b) = other {
                                prepend = vec![b];
                            }
                            st[3] = 0;
                            st[0] = st[1];
                            Step::Error
                        }
                    },
                    _ => {
                        // J_ESCAPE
                        let lead = st[2];
                        st[2] = 0;
                        let next = match (lead, byte) {
                            (0x28, Some(0x42)) => Some(J_ASCII),
                            (0x28, Some(0x4A)) => Some(J_ROMAN),
                            (0x28, Some(0x49)) => Some(J_KATAKANA),
                            (0x24, Some(0x40)) | (0x24, Some(0x42)) => Some(J_LEAD),
                            _ => None,
                        };
                        match next {
                            Some(s) => {
                                st[0] = s;
                                st[1] = s;
                                let output = st[3];
                                st[3] = 1;
                                if output == 0 { Step::Continue } else { Step::Error }
                            }
                            None => {
                                prepend = match byte {
                                    Some(b) => vec![lead as u8, b],
                                    None => vec![lead as u8],
                                };
                                st[3] = 0;
                                st[0] = st[1];
                                Step::Error
                            }
                        }
                    }
                }
            }
            ("shift_jis", _) => {
                let lead = st[0];
                match byte {
                    None if lead != 0 => {
                        st[0] = 0;
                        Step::Error
                    }
                    None => Step::Finished,
                    Some(b) if lead != 0 => {
                        st[0] = 0;
                        let offset = if b < 0x7F { 0x40 } else { 0x41 };
                        let lead_offset = if lead < 0xA0 { 0x81 } else { 0xC1 };
                        let pointer = if (0x40..=0x7E).contains(&b) || (0x80..=0xFC).contains(&b) {
                            Some((lead - lead_offset) * 188 + b as u32 - offset)
                        } else {
                            None
                        };
                        match pointer {
                            Some(p) if (8836..=10715).contains(&p) => Step::Emit(0xE000 - 8836 + p),
                            p => match p.and_then(|p| at16(&index::JIS0208, p as usize)) {
                                Some(cp) => Step::Emit(cp),
                                None => {
                                    if b < 0x80 {
                                        prepend = vec![b];
                                    }
                                    Step::Error
                                }
                            },
                        }
                    }
                    Some(b) if b <= 0x80 => Step::Emit(b as u32),
                    Some(b) if (0xA1..=0xDF).contains(&b) => Step::Emit(0xFF61 - 0xA1 + b as u32),
                    Some(b) if (0x81..=0x9F).contains(&b) || (0xE0..=0xFC).contains(&b) => {
                        st[0] = b as u32;
                        Step::Continue
                    }
                    Some(_) => Step::Error,
                }
            }
            ("euc-kr", _) => {
                let lead = st[0];
                match byte {
                    None if lead != 0 => {
                        st[0] = 0;
                        Step::Error
                    }
                    None => Step::Finished,
                    Some(b) if lead != 0 => {
                        st[0] = 0;
                        let pointer = if (0x41..=0xFE).contains(&b) { Some((lead - 0x81) * 190 + (b as u32 - 0x41)) } else { None };
                        match pointer.and_then(|p| at16(&index::EUC_KR, p as usize)) {
                            Some(cp) => Step::Emit(cp),
                            None => {
                                if b < 0x80 {
                                    prepend = vec![b];
                                }
                                Step::Error
                            }
                        }
                    }
                    Some(b) if b < 0x80 => Step::Emit(b as u32),
                    Some(b) if (0x81..=0xFE).contains(&b) => {
                        st[0] = b as u32;
                        Step::Continue
                    }
                    Some(_) => Step::Error,
                }
            }
            _ => match byte {
                None => Step::Finished,
                Some(_) => Step::Error,
            },
        };
        for b in prepend.into_iter().rev() {
            pushback.push(b);
        }
        match step {
            Step::Continue => {}
            Step::Emit(cp) => push(&mut out, cp),
            Step::Emit2(a, b) => {
                push(&mut out, a);
                push(&mut out, b);
            }
            Step::Error => {
                out.errors += 1;
                out.units.push(0xFFFD);
            }
            Step::Finished => {
                if pushback.is_empty() {
                    break;
                }
            }
        }
    }
    (out, DecoderState(st))
}

fn utf16_decode(bytes: &[u8], be: bool) -> Vec<u16> {
    let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
    let mut pending_lead: Option<u16> = None;
    for pair in bytes.chunks(2) {
        if pair.len() < 2 {
            break;
        }
        let cu = if be { u16::from_be_bytes([pair[0], pair[1]]) } else { u16::from_le_bytes([pair[0], pair[1]]) };
        if let Some(lead) = pending_lead.take() {
            if (0xDC00..=0xDFFF).contains(&cu) {
                out.push(lead);
                out.push(cu);
                continue;
            }
            out.push(0xFFFD);
        }
        if (0xD800..=0xDBFF).contains(&cu) {
            pending_lead = Some(cu);
        } else if (0xDC00..=0xDFFF).contains(&cu) {
            out.push(0xFFFD);
        } else {
            out.push(cu);
        }
    }
    if pending_lead.is_some() || bytes.len() % 2 == 1 {
        out.push(0xFFFD);
    }
    out
}

/// Encoding §6 "decode" of a whole buffer with `encoding` (a canonical name) as the fallback: a UTF-8 / UTF-16
/// BOM overrides it (and is dropped); errors are U+FFFD. Answers the text and the encoding actually used.
pub fn decode(bytes: &[u8], encoding: &'static str) -> (String, &'static str) {
    let (enc, body): (&'static str, &[u8]) = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        ("UTF-8", &bytes[3..])
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        ("UTF-16BE", &bytes[2..])
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        ("UTF-16LE", &bytes[2..])
    } else {
        (encoding, bytes)
    };
    let text = match enc {
        "UTF-8" => String::from_utf8_lossy(body).into_owned(),
        "UTF-16BE" => String::from_utf16_lossy(&utf16_decode(body, true)),
        "UTF-16LE" => String::from_utf16_lossy(&utf16_decode(body, false)),
        "replacement" => {
            if body.is_empty() {
                String::new()
            } else {
                "\u{FFFD}".to_string()
            }
        }
        "x-user-defined" => body.iter().map(|&b| if b < 0x80 { b as char } else { char::from_u32(0xF780 + b as u32 - 0x80).unwrap_or('\u{FFFD}') }).collect(),
        "windows-1252" => {
            let r = single_byte_row("windows-1252").unwrap_or(0);
            body.iter().map(|&b| if b < 0x80 { b as char } else { char::from_u32(index::SINGLE_BYTE[r][(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}') }).collect()
        }
        other => {
            let (d, _) = decode_chunk(other, DecoderState::default(), body, true);
            String::from_utf16_lossy(&d.units)
        }
    };
    (text, enc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(name: &str, b: &[u8]) -> (String, usize) {
        let (d, _) = decode_chunk(name, DecoderState::default(), b, true);
        (String::from_utf16_lossy(&d.units), d.errors)
    }

    /// One known answer per decoder (the code points are the WHATWG indexes'), an error each, and streaming across
    /// a lead byte.
    #[test]
    fn legacy_decoders_known_answers() {
        assert_eq!(get_encoding(" SJIS\n"), Some("Shift_JIS"));
        assert_eq!(get_encoding("latin1"), Some("windows-1252"));
        assert_eq!(dec("shift_jis", &[0x82, 0xA0, 0x41, 0x82]), ("あA\u{FFFD}".to_string(), 1));
        assert_eq!(dec("euc-jp", &[0xA4, 0xA2, 0x8E, 0xB1]), ("あｱ".to_string(), 0));
        assert_eq!(dec("iso-2022-jp", b"\x1b$B$\"\x1b(BA"), ("あA".to_string(), 0));
        assert_eq!(dec("euc-kr", &[0xB0, 0xA1]), ("가".to_string(), 0));
        assert_eq!(dec("big5", &[0xA4, 0x40, 0x88, 0x62]), ("一\u{CA}\u{304}".to_string(), 0));
        assert_eq!(dec("gbk", &[0xC4, 0xE3, 0x80]), ("你€".to_string(), 0));
        assert_eq!(dec("gb18030", &[0x81, 0x30, 0x81, 0x30]), ("\u{80}".to_string(), 0));
        assert_eq!(dec("koi8-r", &[0xC1]), ("а".to_string(), 0));
        assert_eq!(dec("iso-8859-6", &[0xA1]), ("\u{FFFD}".to_string(), 1));
        let (a, s) = decode_chunk("shift_jis", DecoderState::default(), &[0x82], false);
        assert!(a.units.is_empty());
        let (b, _) = decode_chunk("shift_jis", s, &[0xA0], true);
        assert_eq!(String::from_utf16_lossy(&b.units), "あ");
        assert_eq!(decode(&[0xFF, 0xFE, 0x41, 0x00], "Shift_JIS"), ("A".to_string(), "UTF-16LE"));
        assert_eq!(decode(&[0x80], "windows-1252").0, "€");
    }
}
