// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the station's open-system join, judged over byte slices; the frames
//! themselves are the shared-core codec `wifi_core::ieee80211`, never re-built here).
//!
//! WIFIAUTH (rmbp-ledger B788) — rungs S8 (Open System authentication, IEEE Std 802.11-2020 §9.3.3.11,
//! §12.3.3.2) and S9 (association, §9.3.3.5/§9.3.3.6). This file holds the PURE half: which BSS a beacon
//! names (`bss_match`), whether a received frame is the AP's answer to us (`auth_reply`, `assoc_reply`,
//! `refusal`: addr1 == our MAC, addr2 == addr3 == the BSSID, §9.2.4.3, then the `wifi_core` parser),
//! the KAT fixtures (`kat`) and the `tests wifi` state line (`witness`). The device half — the frames out
//! through WIFI10's ring, the answers read from S6's receiver — is `bringup/join.rs`.
//!
//! The KATs are hand-assembled from the clause layouts (no captured auth/assoc frame exists in
//! `docs/dev/evidence`: `[wifi10] rx-raw` prints the RX header only), so `n=12/12` certifies the codec
//! against the standard's field order, not a replay of a capture.

use wifi_core::ieee80211::{
    self as w, parse_assoc_resp, parse_auth, parse_beacon, parse_reason, AssocResult, AuthResult, Mac,
};

/// A BSS whose beacon or probe response carried the SSID the operator named.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bss {
    pub bssid: Mac,
    /// The capability Privacy bit or an RSN element (`Beacon::protected`): WPA is unbuilt, so S8 refuses.
    pub protected: bool,
    pub channel: Option<u8>,
}

/// A beacon / probe response naming exactly `ssid` (§9.4.2.2), else `None`.
pub fn bss_match(frame: &[u8], ssid: &[u8]) -> Option<Bss> {
    let b = parse_beacon(frame)?;
    if ssid.is_empty() || b.ssid != ssid {
        return None;
    }
    Some(Bss { bssid: b.bssid, protected: b.protected(), channel: b.channel })
}

/// §9.2.4.3 for a frame from the AP to the station: addr1 = us, addr2 = addr3 = the BSSID.
fn from_bss_to_us(frame: &[u8], me: Mac, bssid: Mac) -> bool {
    frame.len() >= 24 && frame[4..10] == me && frame[10..16] == bssid && frame[16..22] == bssid
}

/// The AP's authentication frame to us, its fixed fields (§9.4.1.1, §9.4.1.2, §9.4.1.9).
pub fn auth_reply(frame: &[u8], me: Mac, bssid: Mac) -> Option<AuthResult> {
    if !from_bss_to_us(frame, me, bssid) { return None; }
    parse_auth(frame)
}

/// The AP's association response to us: status (§9.4.1.9) and AID (§9.4.1.8).
pub fn assoc_reply(frame: &[u8], me: Mac, bssid: Mac) -> Option<AssocResult> {
    if !from_bss_to_us(frame, me, bssid) { return None; }
    parse_assoc_resp(frame)
}

/// STATIONRATE (B855): `assoc_reply`'s judge and answer, with the rate set the response listed (`w::assoc_resp_rates`,
/// the codec's total parser: `None` = unread — a response with no rate element, or a malformed IE list).
pub fn assoc_reply_rates(frame: &[u8], me: Mac, bssid: Mac) -> Option<(AssocResult, Option<w::RateSet>)> {
    assoc_reply(frame, me, bssid).map(|a| (a, w::assoc_resp_rates(frame)))
}

/// STATIONSHORTPRE (B862): the association response's capability Short Preamble bit (§9.4.1.4 bit 5, 0x0020, the first
/// fixed field at octet 24): the AP allows short preamble in its BSS. `false` for a frame too short to carry it.
pub fn short_preamble(frame: &[u8]) -> bool {
    frame.len() >= 26 && u16::from_le_bytes([frame[24], frame[25]]) & 0x0020 != 0
}

/// STATIONAIR (B862): S9's pick — `assoc_reply_rates`' judge and answer, with the capability's short-preamble bit.
pub fn assoc_reply_air(frame: &[u8], me: Mac, bssid: Mac) -> Option<(AssocResult, Option<w::RateSet>, bool)> {
    assoc_reply_rates(frame, me, bssid).map(|(a, r)| (a, r, short_preamble(frame)))
}

/// STATIONSHORTPRE (B862): the station's primary rate uses the short preamble — the AP's capability bit allows it (`cap`) and
/// the rate is CCK 2/5.5/11 (CCK 1 is long only; OFDM has no such preamble). Until STATIONSHORTCAP (our association request
/// advertises capability 0x0001 only, `wifi_core::ieee80211::build_assoc_req`) the AP's bit alone decides.
pub fn station_short(cap: bool, rate: u8) -> bool {
    cap && matches!(rate, 4 | 11 | 22)
}

/// STATIONDURATION (B862): the rate the AP answers a frame at `rate` with — the control response rate (IEEE Std 802.11-2020
/// §10.6.6.5.2): the highest BASIC rate not faster than `rate` in its modulation class (CCK / OFDM); none (or the set unread)
/// -> the highest mandatory rate of that class not faster than it (CCK: every CCK rate, so `rate` itself; OFDM: 24, 12, 6).
/// A rate outside the legacy set answers CCK 1.
pub fn ack_rate(rs: Option<w::RateSet>, rate: u8) -> u8 {
    let Some(k) = w::LEGACY_RATES.iter().position(|r| *r == rate) else { return w::RATE_1M };
    let (lo, hi) = if k < 4 { (0usize, 4usize) } else { (4usize, 12usize) };
    let basic = rs.map_or(0u16, |r| r.basic);
    if let Some(i) = (lo..hi).rev().find(|&i| basic & (1 << i) != 0 && w::LEGACY_RATES[i] <= rate) {
        return w::LEGACY_RATES[i];
    }
    if k < 4 { rate } else { [w::RATE_24M, 24, 12].iter().copied().find(|r| *r <= rate).unwrap_or(12) }
}

/// STATIONRATE (B855): the kept station's data rate (500 kbit/s units) — OFDM 24 when the AP listed it AND the one Null
/// probe at OFDM 24 was acked at that rate; else the highest CCK rate the AP listed (11, 5.5, 2); else, or with the
/// set unread, CCK 1.
pub fn station_rate(rs: Option<w::RateSet>, ofdm_acked: bool) -> u8 {
    let Some(rs) = rs else { return w::RATE_1M };
    if ofdm_acked && rs.has(w::RATE_24M) {
        return w::RATE_24M;
    }
    [w::RATE_11M, 11, 4].iter().copied().find(|r| rs.has(*r)).unwrap_or(w::RATE_1M)
}

/// The OFDM probe runs: the AP listed OFDM 24.
pub fn ofdm_listed(rs: Option<w::RateSet>) -> bool {
    rs.map_or(false, |r| r.has(w::RATE_24M))
}

/// The wire's name for a rate `station_rate` chooses.
pub fn rate_name(rate: u8) -> &'static str {
    match rate { 4 => "cck2", 11 => "cck5.5", 22 => "cck11", 12 => "ofdm6", 18 => "ofdm9", 24 => "ofdm12", 36 => "ofdm18", 48 => "ofdm24", 72 => "ofdm36", 96 => "ofdm48", 108 => "ofdm54", _ => "cck1" } // STATIONRATECTL (B881): every legacy rate the controller can step to
}

/// The AP's rate set as `1*,2*,5.5*,11*,6,9,…` (Mbit/s, `*` = basic), or `unread`.
pub fn rates_text(rs: Option<w::RateSet>) -> alloc::string::String {
    let Some(rs) = rs else { return alloc::string::String::from("unread") };
    let mut t = alloc::string::String::new();
    for (i, r) in w::LEGACY_RATES.iter().enumerate() {
        if rs.listed & (1 << i) == 0 { continue; }
        if !t.is_empty() { t.push(','); }
        if *r == 11 { t.push_str("5.5") } else { t.push_str(&alloc::format!("{}", r / 2)) }
        if rs.basic & (1 << i) != 0 { t.push('*'); }
    }
    t
}

/// A Deauthentication / Disassociation from the AP to us: its name and reason code (§9.4.1.7).
pub fn refusal(frame: &[u8], me: Mac, bssid: Mac) -> Option<(&'static str, u16)> {
    if !from_bss_to_us(frame, me, bssid) { return None; }
    parse_reason(frame).map(|(sub, r)| (if sub == w::SUBTYPE_DEAUTH { "deauth" } else { "disassoc" }, r))
}

// ── KAT fixtures: STA 02:55:4e:41:4f:53, BSSID 00:11:22:33:44:55, SSID "UnaNet" ─────────────────────
const STA: Mac = [0x02, 0x55, 0x4e, 0x41, 0x4f, 0x53];
const BSSID: Mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
const SSID: &[u8] = b"UnaNet";

/// The 24-octet management header (§9.3.3.1): FC (§9.2.4.1), duration (§9.2.4.2), addr1..3 (§9.2.4.3),
/// sequence control (§9.2.4.4).
fn hdr(fc0: u8, dur: [u8; 2], a1: Mac, a2: Mac, a3: Mac, seq: [u8; 2]) -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec![fc0, 0x00, dur[0], dur[1]];
    f.extend_from_slice(&a1);
    f.extend_from_slice(&a2);
    f.extend_from_slice(&a3);
    f.extend_from_slice(&seq);
    f
}

/// `tests wifi` — the codec and the judges against hand-assembled frames. `(pass, total, first failing name)`.
pub fn kat() -> (u32, u32, Option<&'static str>, [bool; 6]) {
    let mut checks: alloc::vec::Vec<(&'static str, usize, bool)> = alloc::vec::Vec::new();
    // 1. auth-req (§9.3.3.11): FC b0 00, addr1/3 BSSID, addr2 STA, body alg 0, txn 1, status 0 — 30 octets.
    let mut want = hdr(0xb0, [0, 0], BSSID, STA, BSSID, [0, 0]);
    want.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x00, 0x00]);
    checks.push(("auth-req", 0, w::build_auth_open(STA, BSSID, 0) == want && want.len() == 30));
    // 2. auth-resp: from the BSS to us, txn 2, status 0 / 13; misaddressed and truncated refused.
    let mut ok = hdr(0xb0, [0x3a, 0x01], STA, BSSID, BSSID, [0x10, 0x00]);
    ok.extend_from_slice(&[0x00, 0x00, 0x02, 0x00, 0x00, 0x00]);
    checks.push(("auth-resp", 1, auth_reply(&ok, STA, BSSID) == Some(AuthResult { alg: 0, seq: 2, status: 0 })));
    let mut s13 = ok.clone();
    s13[28] = 0x0d;
    checks.push(("auth-resp", 1, auth_reply(&s13, STA, BSSID).map(|a| a.status) == Some(13)));
    let mut other = ok.clone();
    other[9] = 0x54;
    checks.push(("auth-resp", 1, auth_reply(&other, STA, BSSID).is_none()));
    checks.push(("auth-resp", 1, auth_reply(&ok[..29], STA, BSSID).is_none()));
    // 3. assoc-req (§9.3.3.5): cap ESS, listen 3, SSID, Supported Rates 8, Extended Supported Rates 4 — 52 octets.
    let mut want = hdr(0x00, [0, 0], BSSID, STA, BSSID, [0, 0]);
    want.extend_from_slice(&[0x01, 0x00, 0x03, 0x00, 0x00, 0x06]);
    want.extend_from_slice(SSID);
    want.extend_from_slice(&[0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x32, 0x04, 0x30, 0x48, 0x60, 0x6c]);
    checks.push(("assoc-req", 2, w::build_assoc_req(STA, BSSID, SSID, 0) == want && want.len() == 52));
    // 4. assoc-resp (§9.3.3.6): cap, status 0, AID 1 with the two top bits set (§9.4.1.8); status 17.
    let mut ar = hdr(0x10, [0x3a, 0x01], STA, BSSID, BSSID, [0x20, 0x00]);
    ar.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x01, 0xc0, 0x01, 0x04, 0x82, 0x84, 0x8b, 0x96]);
    checks.push(("assoc-resp", 3, assoc_reply(&ar, STA, BSSID) == Some(AssocResult { status: 0, aid: 1 })));
    let mut a17 = ar.clone();
    a17[26] = 0x11;
    checks.push(("assoc-resp", 3, assoc_reply(&a17, STA, BSSID).map(|a| a.status) == Some(17)));
    // 5. refusal (§9.3.3.12): deauth, reason 7 (§9.4.1.7).
    let mut d = hdr(0xc0, [0x3a, 0x01], STA, BSSID, BSSID, [0x30, 0x00]);
    d.extend_from_slice(&[0x07, 0x00]);
    checks.push(("refusal", 4, refusal(&d, STA, BSSID) == Some(("deauth", 7))));
    // 6. bss: a beacon (§9.3.3.2) naming UnaNet on channel 11, open; the Privacy bit; a different SSID.
    let mut b = hdr(0x80, [0, 0], [0xff; 6], BSSID, BSSID, [0, 0]);
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&[0x64, 0x00, 0x01, 0x04, 0x00, 0x06]);
    b.extend_from_slice(SSID);
    b.extend_from_slice(&[0x03, 0x01, 0x0b]);
    checks.push(("bss", 5, bss_match(&b, SSID) == Some(Bss { bssid: BSSID, protected: false, channel: Some(11) })));
    let mut bp = b.clone();
    bp[34] = 0x11;
    checks.push(("bss", 5, bss_match(&bp, SSID).map(|x| x.protected) == Some(true)));
    checks.push(("bss", 5, bss_match(&b, b"UnaNe").is_none()));
    let mut groups = [true; 6];
    let mut pass = 0u32;
    let mut first = None;
    for (name, g, ok) in checks.iter() {
        if *ok { pass += 1 } else {
            groups[*g] = false;
            if first.is_none() { first = Some(*name) }
        }
    }
    (pass, checks.len() as u32, first, groups)
}

fn rung_word(v: u8, ok: &'static str, no: &'static str) -> &'static str {
    match v { 1 => ok, 2 => no, _ => "ABSENT" }
}

/// `tests wifi` (verb.rs), after `wifi10()`: the KAT line, then the §11.3.1 state this boot's rungs reached.
pub fn witness() {
    let (p, t, first, g) = kat();
    let m = |b: bool| if b { "MATCH" } else { "DIFF" };
    serial_println!(
        ":: wifiauth: kat auth-req={} auth-resp={} assoc-req={} assoc-resp={} refusal={} bss={} n={}/{} -> {}{} ::",
        m(g[0]), m(g[1]), m(g[2]), m(g[3]), m(g[4]), m(g[5]), p, t,
        if first.is_none() { "PASS" } else { "FAIL" },
        first.map(|f| alloc::format!(" first={}", f)).unwrap_or_default()
    );
    let (s8, s9, aid) = super::status::join();
    let state = if s9 == 1 { 3 } else if s8 == 1 { 2 } else { 1 };
    serial_println!(
        ":: wifiauth: state={} s8={} s9={} aid={} ::",
        state, rung_word(s8, "AUTHENTICATED", "NOT-AUTHENTICATED"), rung_word(s9, "ASSOCIATED", "NOT-ASSOCIATED"),
        if s9 == 1 { alloc::format!("{}", aid) } else { alloc::string::String::from("-") }
    );
}

/// STATIONRATECTL (B881): the TX header's fallback rate for a frame at `rate` (500 kbit/s units) — the highest BASIC CCK rate
/// strictly below it (the AP answers a basic rate at that rate, so the fallback's NAV is exact), CCK 1 when none is (or the set
/// is unread, or `rate` is CCK 1). Stated divergence from b43 `b43_calc_fallback_rate` (OFDM -> OFDM): the header's fallback
/// words stay in CCK form (`tx::fb_of`).
pub fn fallback_rate(rs: Option<w::RateSet>, rate: u8) -> u8 {
    let basic = rs.map_or(0u16, |r| r.basic);
    (0..4usize).rev().find(|&i| basic & (1 << i) != 0 && w::LEGACY_RATES[i] < rate).map_or(w::RATE_1M, |i| w::LEGACY_RATES[i])
}
