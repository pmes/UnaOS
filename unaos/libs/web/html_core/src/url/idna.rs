//! UTS #46 (Unicode IDNA Compatibility Processing, revision for Unicode 16.0) as the URL Standard calls it from
//! §3.3 "domain to ASCII": Unicode ToASCII with CheckHyphens = false, CheckBidi = true, CheckJoiners = true,
//! UseSTD3ASCIIRules = beStrict, Transitional_Processing = false, VerifyDnsLength = beStrict,
//! IgnoreInvalidPunycode = false.
//!
//! The processing steps (UTS #46 §4): Map (the IDNA Mapping Table), Normalize (NFC, UAX #15), Break (on
//! U+002E), Convert/Validate (Punycode labels decoded, every label held to the §4.1 validity criteria:
//! NFC, no leading "xn--" after decoding, no U+002E, no leading combining mark, status valid or deviation,
//! the RFC 5892 Appendix A ContextJ rules, and — for a Bidi domain name — the six RFC 5893 §2 conditions),
//! then ToASCII (§4.2) Punycode-encodes (RFC 3492) every non-ASCII label.
//!
//! The data is generated (`tools/gen_idna.py` → `idna_tables.rs`); this file is the algorithm only.

use alloc::string::String;
use alloc::vec::Vec;

use super::host::forbidden_domain;
use super::idna_tables as t;
use super::UrlError;

// ---------------------------------------------------------------- table lookups

/// The UTS #46 status of `c` and its mapping (the mapping is empty unless the status is MAPPED).
fn map_entry(c: char) -> (u8, &'static str) {
    let cp = c as u32;
    let i = t::MAP.partition_point(|r| r.0 <= cp).saturating_sub(1);
    let (_, st, off, len) = t::MAP[i];
    (st, t::MAP_POOL.get(off as usize..off as usize + len as usize).unwrap_or(""))
}

fn range3(tab: &[(u32, u32, u8)], c: u32) -> u8 {
    let i = tab.partition_point(|r| r.1 < c);
    match tab.get(i) {
        Some(&(lo, _, v)) if lo <= c => v,
        _ => 0,
    }
}

/// Canonical_Combining_Class.
pub fn ccc(c: u32) -> u8 {
    range3(t::CCC, c)
}

/// General_Category is Mn, Mc or Me.
pub fn is_mark(c: char) -> bool {
    let c = c as u32;
    let i = t::MARKS.partition_point(|r| r.1 < c);
    matches!(t::MARKS.get(i), Some(&(lo, _)) if lo <= c)
}

// Bidi_Class indices, as BIDI_NAMES orders them.
const L: u8 = 0;
const R: u8 = 1;
const AL: u8 = 2;
const EN: u8 = 3;
const ES: u8 = 4;
const ET: u8 = 5;
const AN: u8 = 6;
const CS: u8 = 7;
const NSM: u8 = 8;
const BN: u8 = 9;
const ON: u8 = 13;

fn bidi(c: char) -> u8 {
    range3(t::BIDI, c as u32)
}

// Joining_Type values as JOINING encodes them.
const JT_L: u8 = 1;
const JT_D: u8 = 2;
const JT_R: u8 = 3;
const JT_T: u8 = 4;

fn joining(c: char) -> u8 {
    range3(t::JOINING, c as u32)
}

// ---------------------------------------------------------------- UAX #15 NFC

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// Full canonical decomposition of one code point (Hangul algorithmically, the rest from DECOMP recursively).
fn decompose(c: u32, out: &mut Vec<u32>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&c) {
        let s = c - S_BASE;
        out.push(L_BASE + s / N_COUNT);
        out.push(V_BASE + (s % N_COUNT) / T_COUNT);
        if s % T_COUNT != 0 {
            out.push(T_BASE + s % T_COUNT);
        }
        return;
    }
    match t::DECOMP.binary_search_by_key(&c, |r| r.0) {
        Ok(i) => {
            let (_, a, b) = t::DECOMP[i];
            decompose(a, out);
            if b != 0 {
                decompose(b, out);
            }
        }
        Err(_) => out.push(c),
    }
}

/// The primary composite of the pair, if any (Hangul LV / LVT algorithmically, the rest from COMPOSE).
fn compose_pair(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a) && (a - S_BASE) % T_COUNT == 0 && (T_BASE + 1..T_BASE + T_COUNT).contains(&b) {
        return Some(a + (b - T_BASE));
    }
    t::COMPOSE.binary_search_by(|r| (r.0, r.1).cmp(&(a, b))).ok().map(|i| t::COMPOSE[i].2)
}

/// Normalization Form C: canonical decomposition, canonical ordering, canonical composition.
pub fn nfc(input: &[char]) -> Vec<char> {
    let mut d: Vec<u32> = Vec::with_capacity(input.len());
    for &c in input {
        decompose(c as u32, &mut d);
    }
    // Canonical ordering: every maximal run of non-starters, stably sorted by combining class.
    let mut i = 0;
    while i < d.len() {
        if ccc(d[i]) == 0 {
            i += 1;
            continue;
        }
        let s = i;
        while i < d.len() && ccc(d[i]) != 0 {
            i += 1;
        }
        d[s..i].sort_by_key(|&c| ccc(c));
    }
    // Canonical composition: a character composes with the last starter unless blocked (a character between
    // them with combining class 0 or >= its own).
    let mut out: Vec<u32> = Vec::with_capacity(d.len());
    let mut starter: Option<usize> = None;
    for c in d {
        let cc = ccc(c);
        if let Some(si) = starter {
            let last = out[out.len() - 1];
            let adjacent = out.len() - 1 == si;
            let lc = ccc(last);
            let blocked = !adjacent && (lc == 0 || lc >= cc);
            if !blocked {
                if let Some(p) = compose_pair(out[si], c) {
                    out[si] = p;
                    continue;
                }
            }
        }
        if cc == 0 {
            starter = Some(out.len());
        }
        out.push(c);
    }
    out.into_iter().filter_map(char::from_u32).collect()
}

/// Whether `s` is in NFC.
pub fn is_nfc(s: &[char]) -> bool {
    nfc(s) == s
}

// ---------------------------------------------------------------- RFC 3492 Punycode

const BASE: u32 = 36;
const TMIN: u32 = 1;
const TMAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;

fn adapt(mut delta: u32, numpoints: u32, first: bool) -> u32 {
    delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / numpoints;
    let mut k = 0;
    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }
    k + (((BASE - TMIN + 1) * delta) / (delta + SKEW))
}

fn digit(d: u32) -> char {
    (if d < 26 { b'a' + d as u8 } else { b'0' + (d - 26) as u8 }) as char
}

/// RFC 3492 §6.3 encoding (lowercase digits; the basic code points are copied first, then a `-` if any).
pub fn punycode_encode(input: &str) -> Option<String> {
    let cps: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let mut out: String = input.chars().filter(|c| c.is_ascii()).collect();
    let b = out.len() as u32;
    let mut h = b;
    if b > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    while (h as usize) < cps.len() {
        let m = *cps.iter().filter(|&&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &c in &cps {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, h + 1, h == b);
                delta = 0;
                h += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

/// RFC 3492 §6.2 decoding; `None` on any failure (bad digit, overflow, a non-basic code point in the basic
/// part, a decoded basic code point, or a value that is not a Unicode scalar value).
pub fn punycode_decode(input: &str) -> Option<String> {
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    if !basic.is_ascii() {
        return None;
    }
    let mut out: Vec<u32> = basic.chars().map(|c| c as u32).collect();
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let bytes = rest.as_bytes();
    let mut p = 0;
    while p < bytes.len() {
        let oldi = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let c = *bytes.get(p)?;
            p += 1;
            let d = match c {
                b'a'..=b'z' => c - b'a',
                b'A'..=b'Z' => c - b'A',
                b'0'..=b'9' => c - b'0' + 26,
                _ => return None,
            } as u32;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - oldi, len, oldi == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        if n < 0x80 || n > 0x10FFFF {
            return None;
        }
        out.insert(i as usize, n);
        i += 1;
    }
    out.into_iter().map(char::from_u32).collect()
}

// ---------------------------------------------------------------- UTS #46 §4 processing

/// RFC 5892 Appendix A.1 (ZERO WIDTH NON-JOINER) and A.2 (ZERO WIDTH JOINER).
fn context_j_ok(label: &[char]) -> bool {
    for (i, &c) in label.iter().enumerate() {
        if c != '\u{200C}' && c != '\u{200D}' {
            continue;
        }
        if i > 0 && ccc(label[i - 1] as u32) == 9 {
            continue; // Before(cp) has Canonical_Combining_Class Virama
        }
        if c == '\u{200D}' {
            return false;
        }
        // (Joining_Type:{L,D})(Joining_Type:T)* ZWNJ (Joining_Type:T)*(Joining_Type:{R,D})
        let mut j = i;
        let mut left = false;
        while j > 0 {
            j -= 1;
            match joining(label[j]) {
                JT_T => continue,
                JT_L | JT_D => left = true,
                _ => {}
            }
            break;
        }
        let mut right = false;
        for &x in &label[i + 1..] {
            match joining(x) {
                JT_T => continue,
                JT_R | JT_D => right = true,
                _ => {}
            }
            break;
        }
        if !(left && right) {
            return false;
        }
    }
    true
}

/// RFC 5893 §2, the six conditions, for one non-empty label of a Bidi domain name.
fn bidi_ok(label: &[char]) -> bool {
    let Some(&first) = label.first() else { return true };
    let classes: Vec<u8> = label.iter().map(|&c| bidi(c)).collect();
    let last_non_nsm = classes.iter().rev().copied().find(|&b| b != NSM);
    match bidi(first) {
        R | AL => {
            // 2: only R, AL, AN, EN, ES, CS, ET, ON, BN, NSM.
            if !classes.iter().all(|b| matches!(*b, R | AL | AN | EN | ES | CS | ET | ON | BN | NSM)) {
                return false;
            }
            // 3: ends with R, AL, EN or AN, then any NSM.
            if !matches!(last_non_nsm, Some(R | AL | EN | AN)) {
                return false;
            }
            // 4: not both EN and AN.
            !(classes.contains(&EN) && classes.contains(&AN))
        }
        L => {
            // 5: only L, EN, ES, CS, ET, ON, BN, NSM.
            if !classes.iter().all(|b| matches!(*b, L | EN | ES | CS | ET | ON | BN | NSM)) {
                return false;
            }
            // 6: ends with L or EN, then any NSM.
            matches!(last_non_nsm, Some(L | EN))
        }
        // 1: the first character must be L, R or AL.
        _ => false,
    }
}

/// UTS #46 §4.1 validity criteria for one label (CheckHyphens = false, CheckJoiners = true, nontransitional);
/// the Bidi criterion is applied across the domain by the caller.
fn label_valid(label: &[char], std3: bool, was_punycode: bool) -> bool {
    if was_punycode {
        // 1. NFC (a label that was not Punycode came out of the Normalize step already in NFC).
        if !is_nfc(label) {
            return false;
        }
        // 3. Not CheckHyphens: must not begin with "xn--".
        if label.len() >= 4 && label[..4].iter().copied().eq("xn--".chars()) {
            return false;
        }
        // 4. No U+002E.
        if label.contains(&'.') {
            return false;
        }
    }
    // 5. Must not begin with a combining mark.
    if label.first().is_some_and(|&c| is_mark(c)) {
        return false;
    }
    // 6. Status valid or deviation; with UseSTD3ASCIIRules an ASCII code point must be [a-z0-9-].
    for &c in label {
        let (st, _) = map_entry(c);
        if st != t::VALID && st != t::DEVIATION {
            return false;
        }
        if std3 && c.is_ascii() && !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return false;
        }
    }
    // 7. CheckJoiners.
    context_j_ok(label)
}

/// UTS #46 §4.2 ToASCII with the URL Standard's flags; `be_strict` sets UseSTD3ASCIIRules and VerifyDnsLength.
pub fn uts46_to_ascii(domain: &str, be_strict: bool) -> Result<String, UrlError> {
    // 1. Map.
    let mut mapped: Vec<char> = Vec::with_capacity(domain.len());
    for c in domain.chars() {
        let (st, m) = map_entry(c);
        match st {
            t::IGNORED => {}
            t::MAPPED => mapped.extend(m.chars()),
            _ => mapped.push(c), // valid, deviation (nontransitional), disallowed (caught in Validate)
        }
    }
    // 2. Normalize.
    let normalized = nfc(&mapped);
    // 3. Break; 4. Convert/Validate.
    let mut labels: Vec<Vec<char>> = Vec::new();
    for raw in normalized.split(|&c| c == '.') {
        let is_puny = raw.len() >= 4 && raw[..4].iter().copied().eq("xn--".chars());
        if is_puny {
            if !raw.iter().all(|c| c.is_ascii()) {
                return Err(UrlError::InvalidHost);
            }
            let ascii: String = raw[4..].iter().collect();
            let decoded = punycode_decode(&ascii).ok_or(UrlError::InvalidHost)?;
            if decoded.is_empty() || decoded.is_ascii() {
                return Err(UrlError::InvalidHost);
            }
            let chars: Vec<char> = decoded.chars().collect();
            if !label_valid(&chars, be_strict, true) {
                return Err(UrlError::InvalidHost);
            }
            labels.push(chars);
        } else {
            if !label_valid(raw, be_strict, false) {
                return Err(UrlError::InvalidHost);
            }
            labels.push(raw.to_vec());
        }
    }
    // CheckBidi: a Bidi domain name holds an R, AL or AN character in some label.
    let bidi_domain = labels.iter().flatten().any(|&c| matches!(bidi(c), R | AL | AN));
    if bidi_domain && !labels.iter().filter(|l| !l.is_empty()).all(|l| bidi_ok(l)) {
        return Err(UrlError::InvalidHost);
    }
    // ToASCII: Punycode-encode the non-ASCII labels.
    let mut out = String::with_capacity(domain.len());
    for (i, l) in labels.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        if l.iter().all(|c| c.is_ascii()) {
            out.extend(l.iter());
        } else {
            let s: String = l.iter().collect();
            out.push_str("xn--");
            out.push_str(&punycode_encode(&s).ok_or(UrlError::InvalidHost)?);
        }
    }
    if be_strict {
        // VerifyDnsLength: total 1..=253 (without a trailing root dot), every label 1..=63.
        let body = out.strip_suffix('.').unwrap_or(&out);
        if body.is_empty() || body.len() > 253 || body.split('.').any(|l| l.is_empty() || l.len() > 63) {
            return Err(UrlError::InvalidHost);
        }
    }
    Ok(out)
}

/// URL Standard §3.3 "domain to ASCII".
pub fn domain_to_ascii(domain: &str, be_strict: bool) -> Result<String, UrlError> {
    // The standard's permitted shortcut: an ASCII domain with no "xn--" label (ASCII case-insensitively) maps
    // to its ASCII lowercase under UTS #46 with these flags.
    let fast = !be_strict
        && domain.is_ascii()
        && !domain.split('.').any(|l| l.len() >= 4 && l[..4].eq_ignore_ascii_case("xn--"));
    let result = if fast { domain.to_ascii_lowercase() } else { uts46_to_ascii(domain, be_strict)? };
    if !be_strict && (result.is_empty() || result.chars().any(forbidden_domain)) {
        return Err(UrlError::InvalidHost);
    }
    Ok(result)
}
