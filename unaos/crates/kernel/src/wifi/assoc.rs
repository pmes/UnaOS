// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the station's open-system join and the RSN 4-way handshake, S10, judged over byte slices; the frames
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
    /// STATIONSHORTCAP (B884): the beacon's capability Short Preamble bit (§9.4.1.4, `w::CAP_SHORT_PREAMBLE`).
    pub short: bool,
    /// STATIONERP (B884): the beacon's ERP element first octet (§9.4.2.12, `w::ERP_*`), `None` when absent.
    pub erp: Option<u8>,
}

/// A beacon / probe response naming exactly `ssid` (§9.4.2.2), else `None`.
pub fn bss_match(frame: &[u8], ssid: &[u8]) -> Option<Bss> {
    let b = parse_beacon(frame)?;
    if ssid.is_empty() || b.ssid != ssid {
        return None;
    }
    Some(Bss { bssid: b.bssid, protected: b.protected(), channel: b.channel, short: b.capabilities & w::CAP_SHORT_PREAMBLE != 0, erp: b.erp })
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
/// the rate is CCK 2/5.5/11 (CCK 1 is long only; OFDM has no such preamble). STATIONSHORTCAP (B884): our association request
/// now advertises Short Preamble when the beacon's bit allows it (`assoc_cap`); STATIONERP (B884): the BSS's ERP
/// Barker_Preamble_Mode (`barker`, `erp_barker`) forces the long preamble regardless of the capability bit.
pub fn station_short(cap: bool, barker: bool, rate: u8) -> bool {
    cap && !barker && matches!(rate, 4 | 11 | 22)
}

/// STATIONSHORTCAP (B884): S9's Capability Information word (§9.4.1.4) — ESS, with Short Preamble only when the BSS's beacon
/// set its own Short Preamble bit.
pub fn assoc_cap(b: &Bss) -> u16 {
    w::CAP_ESS | if b.short { w::CAP_SHORT_PREAMBLE } else { 0 }
}

/// STATIONERP (B884): the BSS asks for protection (ERP Use_Protection, §9.4.2.12): OFDM frames go behind a CTS-to-self.
pub fn erp_protect(erp: Option<u8>) -> bool {
    erp.map_or(false, |e| e & w::ERP_USE_PROTECTION != 0)
}

/// STATIONERP (B884): the BSS asks for the long preamble (ERP Barker_Preamble_Mode, §9.4.2.12).
pub fn erp_barker(erp: Option<u8>) -> bool {
    erp.map_or(false, |e| e & w::ERP_BARKER_PREAMBLE != 0)
}

/// STATIONERP (B884): the wire's `erp=` value — `<prot|open>,<barker|short>`, or `absent` (no ERP element heard).
pub fn erp_text(erp: Option<u8>) -> &'static str {
    match (erp.is_some(), erp_protect(erp), erp_barker(erp)) {
        (false, _, _) => "absent",
        (true, true, true) => "prot,barker",
        (true, true, false) => "prot,short",
        (true, false, true) => "open,barker",
        (true, false, false) => "open,short",
    }
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
    // 3. assoc-req (§9.3.3.5): cap ESS, listen 3, SSID, Supported Rates 8, Extended Supported Rates 4 — 52 octets; STATIONSHORTCAP
    //    (B884): the same request with cap ESS | Short Preamble differs only in octet 24 (21 00), in the same check.
    let mut want = hdr(0x00, [0, 0], BSSID, STA, BSSID, [0, 0]);
    want.extend_from_slice(&[0x01, 0x00, 0x03, 0x00, 0x00, 0x06]);
    want.extend_from_slice(SSID);
    want.extend_from_slice(&[0x01, 0x08, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x32, 0x04, 0x30, 0x48, 0x60, 0x6c]);
    let mut want_sp = want.clone();
    want_sp[24] = 0x21;
    checks.push(("assoc-req", 2, w::build_assoc_req(STA, BSSID, SSID, 0, w::CAP_ESS) == want && want.len() == 52
        && w::build_assoc_req(STA, BSSID, SSID, 0, w::CAP_ESS | w::CAP_SHORT_PREAMBLE) == want_sp));
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
    checks.push(("bss", 5, bss_match(&b, SSID) == Some(Bss { bssid: BSSID, protected: false, channel: Some(11), short: false, erp: None })));
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

#[cfg(feature = "wifi10")] fn rung_word(v: u8, ok: &'static str, no: &'static str) -> &'static str {
    match v { 1 => ok, 2 => no, _ => "ABSENT" }
}

/// `tests wifi` (verb.rs), after `wifi10()`: the KAT line, then the §11.3.1 state this boot's rungs reached.
#[cfg(feature = "wifi10")] pub fn witness() {
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

// ── MT7925S10 (zenbook-ledger Z55): the RSN 4-way handshake (IEEE Std 802.11-2020 §12.7), PURE — no device, no print ─────────────
// WPA2-PSK with CCMP: the PMK from the passphrase (`wpa_pmk`, Annex J.4), the PTK (`ptk`, the §12.7.1.2 PRF-384), the EAPOL-Key
// frames of §12.7.2 (descriptor type 2, key descriptor version 2 = HMAC-SHA1-128 MIC + AES key wrap) and the supplicant's side of
// messages 1-4 (`Handshake::on_m1`, `on_m3`). The arithmetic is `crypto_core` (WPA2CRYPTO, rmbp-ledger B882: nothing is re-built
// here); the radio holds the keys and does CCMP (no software CCMP). The device loop is `verb::s10_wpa2`, the key commands are
// `drivers/mt7925.rs`'s S10 block. Facts from the standard; no code copied.
use alloc::vec::Vec;
use crypto_core::ct::{ct_eq, Zeroize};
use wifi_core::ieee80211::RSN_PSK_CCMP;
use crypto_core::{hmac::Hmac, keywrap, pbkdf2::pbkdf2_hmac, prf80211, sha1::Sha1};

/// The EAPOL-Key frame through its Key Data Length: 802.1X header 4 + descriptor body 95 (§12.7.2 Figure 12-32); the MIC field.
const EK_HDR: usize = 99;
const EK_MIC: core::ops::Range<usize> = 81..97;
/// Key Information (§12.7.2 b): the bits judged — descriptor version (b0-2), key type (b3), install (b6), ack (b7), MIC (b8),
/// secure (b9), error (b10), request (b11), encrypted key data (b12). The key index (b4-5) and SMK (b13) are not judged.
const KI_JUDGED: u16 = 0x1fcf;
const KI_M1: u16 = 0x008a; // ver 2 | pairwise | ack
const KI_M2: u16 = 0x010a; // ver 2 | pairwise | MIC
const KI_M3: u16 = 0x13ca; // ver 2 | pairwise | install | ack | MIC | secure | encrypted
const KI_M4: u16 = 0x030a; // ver 2 | pairwise | MIC | secure
const KI_G1: u16 = 0x1382; // S10REKEY (Z63): ver 2 | group | ack | MIC | secure | encrypted (§12.7.7.2)
const KI_G2: u16 = 0x0302; // S10REKEY (Z63): ver 2 | group | MIC | secure (§12.7.7.3)
/// The most key data a message 3 may carry (the data is the RSN element and the KDEs, a few dozen octets).
const KD_MAX: usize = 1024;
/// The EtherType of 802.1X (EAPOL), and the Ethernet header it follows.
const ETY_EAPOL: [u8; 2] = [0x88, 0x8e];
const ETH: usize = 14;

/// Why a handshake message was refused: the wire's word is `name`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    Mic,
    Unwrap,
    RsnMismatch,
    Replay,
    Frame,
    /// S10REKEY (Z63): a group message 1 whose key data carries no GTK KDE.
    Kde,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::Mic => "mic",
            Why::Unwrap => "unwrap",
            Why::RsnMismatch => "rsn-mismatch",
            Why::Replay => "replay",
            Why::Frame => "frame",
            Why::Kde => "kde",
        }
    }
}

/// The keys message 3 delivers: the pairwise temporal key, the group key and its index. Zeroized on drop.
pub struct Keys {
    pub tk: [u8; 16],
    pub gtk: [u8; 16],
    pub gtk_id: u8,
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.tk.zeroize();
        self.gtk.zeroize();
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The PMK of a WPA2-PSK network (§12.7.1.2, Annex J.4): a passphrase of 8..=63 printable ASCII octets -> PBKDF2-HMAC-SHA1 over the
/// SSID, 4096 iterations, 32 octets; or exactly 64 hex digits = the PSK itself. Anything else (or an SSID outside 1..=32) is
/// `psk-len`. The caller zeroizes what it holds.
pub fn wpa_pmk(ssid: &[u8], pass: &[u8]) -> Result<[u8; 32], &'static str> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("psk-len");
    }
    let mut pmk = [0u8; 32];
    if pass.len() == 64 && pass.iter().all(|c| c.is_ascii_hexdigit()) {
        for i in 0..32 {
            pmk[i] = (hex_val(pass[2 * i]).unwrap_or(0) << 4) | hex_val(pass[2 * i + 1]).unwrap_or(0);
        }
        return Ok(pmk);
    }
    if pass.len() < 8 || pass.len() > 63 || !pass.iter().all(|c| (0x20..0x7f).contains(c)) {
        return Err("psk-len");
    }
    pbkdf2_hmac::<Sha1>(pass, ssid, 4096, &mut pmk).map_err(|_| "psk-len")?;
    Ok(pmk)
}

/// The PTK for CCMP (§12.7.1.3): PRF-384(PMK, "Pairwise key expansion", Min(AA,SPA) || Max(AA,SPA) || Min(ANonce,SNonce) ||
/// Max(ANonce,SNonce)) = KCK (16) || KEK (16) || TK (16). The two orders of the arguments give the same key.
pub fn ptk(pmk: &[u8; 32], aa: &Mac, spa: &Mac, an: &[u8; 32], sn: &[u8; 32]) -> [u8; 48] {
    let (a0, a1) = if aa <= spa { (aa, spa) } else { (spa, aa) };
    let (n0, n1) = if an <= sn { (an, sn) } else { (sn, an) };
    let parts: [&[u8]; 4] = [&a0[..], &a1[..], &n0[..], &n1[..]];
    let mut out = [0u8; 48];
    let _ = prf80211::prf(pmk, b"Pairwise key expansion", &parts, &mut out);
    out
}

/// One EAPOL-Key frame as the parser reads it: the 802.1X version, Key Information, replay counter, nonce, key data, and the frame
/// cut to its own length (the Ethernet minimum-size padding is not part of the MIC).
struct Ek<'a> {
    ver: u8,
    info: u16,
    replay: u64,
    nonce: [u8; 32],
    data: &'a [u8],
    frame: &'a [u8],
}

fn ek_parse(f: &[u8]) -> Option<Ek<'_>> {
    if f.len() < EK_HDR || f[0] == 0 || f[0] > 3 || f[1] != 3 {
        return None;
    }
    let body = u16::from_be_bytes([f[2], f[3]]) as usize;
    if body < EK_HDR - 4 || 4 + body > f.len() {
        return None;
    }
    let frame = &f[..4 + body];
    if frame[4] != 2 {
        return None;
    }
    let kd = u16::from_be_bytes([frame[97], frame[98]]) as usize;
    if EK_HDR + kd > frame.len() {
        return None;
    }
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&frame[17..49]);
    let mut r = [0u8; 8];
    r.copy_from_slice(&frame[9..17]);
    Some(Ek {
        ver: frame[0],
        info: u16::from_be_bytes([frame[5], frame[6]]),
        replay: u64::from_be_bytes(r),
        nonce,
        data: &frame[EK_HDR..EK_HDR + kd],
        frame,
    })
}

/// An EAPOL-Key frame (MIC zero): 802.1X version `ver`, type Key, descriptor 2, `info`, key length 0, `replay`, `nonce`, IV, RSC
/// and reserved zero, then `data`.
fn ek_build(ver: u8, info: u16, replay: u64, nonce: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut f = alloc::vec![0u8; EK_HDR + data.len()];
    f[0] = ver;
    f[1] = 3;
    f[2..4].copy_from_slice(&((EK_HDR - 4 + data.len()) as u16).to_be_bytes());
    f[4] = 2;
    f[5..7].copy_from_slice(&info.to_be_bytes());
    f[9..17].copy_from_slice(&replay.to_be_bytes());
    f[17..49].copy_from_slice(nonce);
    f[97..99].copy_from_slice(&(data.len() as u16).to_be_bytes());
    f[EK_HDR..].copy_from_slice(data);
    f
}

/// The EAPOL-Key MIC for descriptor version 2 (§12.7.2 g): HMAC-SHA1 under the KCK over the whole frame with the MIC field zero,
/// the first 16 octets.
pub fn mic(kck: &[u8], frame: &[u8]) -> [u8; 16] {
    let mut h = Hmac::<Sha1>::new(kck);
    if frame.len() >= EK_MIC.end {
        h.update(&frame[..EK_MIC.start]);
        h.update(&[0u8; 16]);
        h.update(&frame[EK_MIC.end..]);
    } else {
        h.update(frame);
    }
    let mut t = [0u8; 20];
    h.finalize_into(&mut t);
    let mut out = [0u8; 16];
    out.copy_from_slice(&t[..16]);
    t.zeroize();
    out
}

/// Key data unwrapped under the KEK (RFC 3394): `Unwrap` for a length that cannot be a wrapped key or a failed integrity check.
fn unwrap_kd(kek: &[u8], kd: &[u8]) -> Result<Vec<u8>, Why> {
    if kd.len() < 24 || kd.len() % 8 != 0 || kd.len() > KD_MAX {
        return Err(Why::Unwrap);
    }
    let mut plain = alloc::vec![0u8; kd.len() - 8];
    if keywrap::unwrap(kek, kd, &mut plain).is_err() {
        plain.zeroize();
        return Err(Why::Unwrap);
    }
    Ok(plain)
}

/// Walk message 3's unwrapped key data (§12.7.6.4, §12.7.2 h): the first RSN element must equal `rsn_ap` (the AP's beacon element)
/// byte for byte, and the GTK KDE (element dd, OUI 00-0F-AC, data type 1, key id in bits 0-1 of its first octet) carries a
/// 16-octet group key. Padding (a dd of length 0 then zeros) is walked as empty elements. S10REKEY (Z63): `rsn_ap` None = a group
/// message 1 (§12.7.7.2: the GTK KDE alone; an RSN element is not required and not judged; no GTK KDE = `Kde`).
fn scan_kd(p: &[u8], rsn_ap: Option<&[u8]>) -> Result<(u8, [u8; 16]), Why> {
    let (mut i, mut rsn, mut gtk): (usize, bool, Option<(u8, [u8; 16])>) = (0, false, None);
    while i + 2 <= p.len() {
        let (id, len) = (p[i], p[i + 1] as usize);
        if i + 2 + len > p.len() {
            return Err(Why::Frame);
        }
        let b = &p[i + 2..i + 2 + len];
        if id == 0x30 && !rsn {
            if rsn_ap.map_or(false, |ap| &p[i..i + 2 + len] != ap) {
                return Err(Why::RsnMismatch);
            }
            rsn = true;
        } else if id == 0xdd && len == 22 && b[..4] == [0x00, 0x0f, 0xac, 1] && gtk.is_none() {
            let mut g = [0u8; 16];
            g.copy_from_slice(&b[6..22]);
            gtk = Some((b[4] & 3, g));
        }
        i += 2 + len;
    }
    match (rsn_ap, rsn, gtk) {
        (None, _, Some(g)) | (Some(_), true, Some(g)) => Ok(g),
        (None, _, None) => Err(Why::Kde),
        (Some(_), false, _) => Err(Why::RsnMismatch),
        (Some(_), true, None) => Err(Why::Frame),
    }
}

/// The supplicant's side of the 4-way handshake for one join: the AP's address and RSN element, our address and SNonce, the PMK,
/// the PTK once message 1 named the ANonce, the highest replay counter accepted, the 802.1X version echoed, the step.
pub struct Handshake {
    aa: Mac,
    spa: Mac,
    snonce: [u8; 32],
    rsn_ap: Vec<u8>,
    pmk: [u8; 32],
    ptk: [u8; 48],
    anonce: [u8; 32],
    replay: u64,
    step: u8,
}

impl Drop for Handshake {
    fn drop(&mut self) {
        self.pmk.zeroize();
        self.ptk.zeroize();
    }
}

impl Handshake {
    /// `aa` the AP's address (the BSSID), `spa` ours, `snonce` fresh from the DRBG, `rsn_ap` the RSN element of the AP's beacon.
    pub fn new(aa: Mac, spa: Mac, snonce: [u8; 32], rsn_ap: &[u8], pmk: &[u8; 32]) -> Handshake {
        Handshake { aa, spa, snonce, rsn_ap: rsn_ap.to_vec(), pmk: *pmk, ptk: [0u8; 48], anonce: [0u8; 32], replay: 0, step: 0 }
    }

    /// Message 1 (§12.7.6.2: pairwise, ack, no MIC; any key data, a PMKID KDE, is not used) -> message 2 (§12.7.6.3: pairwise, MIC,
    /// our SNonce, key length 0, our RSN element as key data, the AP's replay counter and 802.1X version echoed). The PTK is derived
    /// here; a repeated message 1 (the AP resending) derives it again from its ANonce. A refused frame changes nothing.
    pub fn on_m1(&mut self, f: &[u8]) -> Result<Vec<u8>, Why> {
        let e = ek_parse(f).ok_or(Why::Frame)?;
        if e.info & KI_JUDGED != KI_M1 {
            return Err(Why::Frame);
        }
        self.anonce = e.nonce;
        self.replay = e.replay;
        self.ptk = ptk(&self.pmk, &self.aa, &self.spa, &self.anonce, &self.snonce);
        self.step = 1;
        let mut m2 = ek_build(e.ver, KI_M2, e.replay, &self.snonce, &RSN_PSK_CCMP);
        let m = mic(&self.ptk[..16], &m2);
        m2[EK_MIC].copy_from_slice(&m);
        Ok(m2)
    }

    /// Message 3 (§12.7.6.4: pairwise, install, ack, MIC, secure, encrypted; the ANonce of message 1; a replay counter above it; a MIC
    /// under the KCK; key data wrapped under the KEK holding the RSN element equal to the beacon's and the GTK KDE) -> message 4
    /// (§12.7.6.5: pairwise, MIC, secure, zero nonce, no data, the AP's replay counter) and the keys. Checked in that order, the MIC
    /// before anything it protects; a refused frame changes nothing.
    pub fn on_m3(&mut self, f: &[u8]) -> Result<(Vec<u8>, Keys), Why> {
        if self.step != 1 {
            return Err(Why::Frame);
        }
        let e = ek_parse(f).ok_or(Why::Frame)?;
        if e.info & KI_JUDGED != KI_M3 || e.nonce != self.anonce {
            return Err(Why::Frame);
        }
        let want = mic(&self.ptk[..16], e.frame);
        if !ct_eq(&e.frame[EK_MIC], &want) {
            return Err(Why::Mic);
        }
        if e.replay <= self.replay {
            return Err(Why::Replay);
        }
        let mut plain = unwrap_kd(&self.ptk[16..32], e.data)?;
        let r = scan_kd(&plain, Some(&self.rsn_ap[..]));
        plain.zeroize();
        let (gtk_id, mut gtk) = r?;
        let mut keys = Keys { tk: [0u8; 16], gtk, gtk_id };
        keys.tk.copy_from_slice(&self.ptk[32..48]);
        gtk.zeroize();
        let mut m4 = ek_build(e.ver, KI_M4, e.replay, &[0u8; 32], &[]);
        let m = mic(&self.ptk[..16], &m4);
        m4[EK_MIC].copy_from_slice(&m);
        self.replay = e.replay;
        self.step = 2;
        Ok((m4, keys))
    }
}

/// The 802.1X payload of an Ethernet frame the AP sent us (`bssid` -> `me`, EtherType 0x888e), else `None`.
pub fn eapol_payload<'a>(eth: &'a [u8], me: &Mac, bssid: &Mac) -> Option<&'a [u8]> {
    if eth.len() < ETH + 4 || eth[0..6] != me[..] || eth[6..12] != bssid[..] || eth[12..14] != ETY_EAPOL {
        return None;
    }
    Some(&eth[ETH..])
}

/// An Ethernet frame carrying the 802.1X payload `eapol` from `me` to `bssid` (the driver's data path takes Ethernet frames).
pub fn eapol_eth(me: &Mac, bssid: &Mac, eapol: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(ETH + eapol.len());
    f.extend_from_slice(bssid);
    f.extend_from_slice(me);
    f.extend_from_slice(&ETY_EAPOL);
    f.extend_from_slice(eapol);
    f
}

fn hx<const N: usize>(s: &str) -> [u8; N] {
    let b = s.as_bytes();
    let mut o = [0u8; N];
    for i in 0..N {
        let hi = b.get(2 * i).copied().and_then(hex_val).unwrap_or(0);
        let lo = b.get(2 * i + 1).copied().and_then(hex_val).unwrap_or(0);
        o[i] = (hi << 4) | lo;
    }
    o
}

/// A message 1 assembled octet by octet from Figure 12-32 (802.1X version 1, Key, body 95, descriptor 2, key info 0x008a, key
/// length 16, replay, ANonce, then IV, RSC, reserved, MIC and a key data length of 0), independent of `ek_build`.
fn kat_m1(an: &[u8; 32], replay: u64) -> Vec<u8> {
    let mut f = alloc::vec![1u8, 3, 0, 95, 2, 0x00, 0x8a, 0, 16];
    f.extend_from_slice(&replay.to_be_bytes());
    f.extend_from_slice(an);
    f.extend_from_slice(&[0u8; 16 + 8 + 8 + 16 + 2]);
    f
}

/// The authenticator's message 3 for the KAT: key data = `rsn` || the GTK KDE (dd 16, 00-0F-AC 1, id | tx, 0, GTK), framed by
/// `kat_wrapped` (0x13ca).
fn kat_m3(ptk: &[u8; 48], an: &[u8; 32], replay: u64, rsn: &[u8], gtk: &[u8; 16], gtk_id: u8) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(rsn);
    p.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 1, gtk_id | 4, 0]);
    p.extend_from_slice(gtk);
    kat_wrapped(ptk, 0x13ca, an, replay, &p)
}

/// An authenticator's EAPOL-Key frame for the KAT (kat_m3's frame half, hoisted by S10REKEY Z63 so message 3 and the group message 1
/// share it): `plain` with the dd-0 padding to a multiple of 8, wrapped under the KEK, framed with `info`, key length 16, `replay`,
/// `nonce`, the MIC under the KCK.
fn kat_wrapped(ptk: &[u8; 48], info: u16, nonce: &[u8; 32], replay: u64, plain: &[u8]) -> Vec<u8> {
    let mut p = plain.to_vec();
    if p.len() % 8 != 0 {
        p.push(0xdd);
        while p.len() % 8 != 0 {
            p.push(0);
        }
    }
    let mut w = alloc::vec![0u8; p.len() + 8];
    let _ = keywrap::wrap(&ptk[16..32], &p, &mut w);
    let mut f = alloc::vec![1u8, 3, 0, 0, 2, 0, 0, 0, 16];
    f[2..4].copy_from_slice(&((95 + w.len()) as u16).to_be_bytes());
    f[5..7].copy_from_slice(&info.to_be_bytes());
    f.extend_from_slice(&replay.to_be_bytes());
    f.extend_from_slice(nonce);
    f.extend_from_slice(&[0u8; 16 + 8 + 8 + 16]);
    f.extend_from_slice(&(w.len() as u16).to_be_bytes());
    f.extend_from_slice(&w);
    let m = mic(&ptk[..16], &f);
    f[EK_MIC].copy_from_slice(&m);
    f
}

/// `tests wpa2`'s pure legs `[pbkdf2, prf, mic, unwrap, frame, rekey]` (the key-command bodies, `keys`, are `mt7925::s10_kat`). Published
/// vectors: Annex J.4 (PSK), Annex J.3 (PRF-384), RFC 2202 case 2 (HMAC-SHA-1), RFC 3394 §4.1 (key wrap). No published PTK or
/// EAPOL-Key MIC vector was readable at the cut, so those two are pinned from an independent implementation (Python hashlib/hmac,
/// `scratch/.../tools/wpa2vec.py`): PMK = J.4 test 2's PSK, AA a0..a5, SPA b0..b5, SNonce c0..df, ANonce e0..ff -> PTK
/// e677aae8...1f6c2809 and, for the message 2 of a replay-1 message 1 carrying our RSN element, MIC 4b68b6ef2ab7b3e4be16d2a7dc2eb086.
pub fn wpa2_kat() -> [bool; 6] {
    let psk2: [u8; 32] = hx("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af");
    // 0. pbkdf2: Annex J.4 tests 1 and 2 through `wpa_pmk`; the 64-hex PSK form; the refusals.
    let pbkdf2 = wpa_pmk(b"IEEE", b"password") == Ok(hx("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"))
        && wpa_pmk(b"ThisIsASSID", b"ThisIsAPassword") == Ok(psk2)
        && wpa_pmk(b"IEEE", &[b'a'; 64]) == Ok([0xaa; 32])
        && wpa_pmk(b"IEEE", &[b'z'; 64]).is_err()
        && wpa_pmk(b"IEEE", b"short").is_err()
        && wpa_pmk(b"IEEE", &[b'a'; 7]).is_err()
        && wpa_pmk(b"IEEE", b"pass\x01word").is_err()
        && wpa_pmk(b"", b"password").is_err();
    // 1. prf: Annex J.3 test 3 (PRF-384) through prf80211::prf, the pinned PTK, the argument order of the Min/Max parts.
    let (aa, spa): (Mac, Mac) = (core::array::from_fn(|i| 0xa0 + i as u8), core::array::from_fn(|i| 0xb0 + i as u8));
    let (sn, an): ([u8; 32], [u8; 32]) = (core::array::from_fn(|i| 0xc0 + i as u8), core::array::from_fn(|i| 0xe0 + i as u8));
    let mut j3 = [0u8; 48];
    let k3 = [0xaau8; 80];
    let j3ok = prf80211::prf(&k3, b"prefix-3", &[b"Test Using Larger Than Block-Size Key - Hash Key First"], &mut j3).is_ok()
        && j3 == hx::<48>("0ab6c33ccf70d0d736f4b04c8a7373255511abc5073713163bd0b8c9eeb7e1956fa066820a73ddee3f6d3bd407e0682a");
    let want_ptk: [u8; 48] = hx("e677aae82af1fb9a25e93e22ad77022ed90489e672adcc6ae148d0afafb92b6936606222ae74d4d2d1db3ac91f6c2809");
    let k = ptk(&psk2, &aa, &spa, &an, &sn);
    let prf = j3ok && k == want_ptk && ptk(&psk2, &spa, &aa, &sn, &an) == want_ptk && ptk(&psk2, &aa, &spa, &sn, &sn) != want_ptk;
    // 2. mic: RFC 2202 case 2 truncated to 16; the message 2 of a replay-1 message 1 (121 octets) carries the pinned MIC.
    let mut h = Hmac::<Sha1>::new(b"Jefe");
    h.update(b"what do ya want for nothing?");
    let mut t = [0u8; 20];
    h.finalize_into(&mut t);
    let pin: [u8; 16] = hx("4b68b6ef2ab7b3e4be16d2a7dc2eb086");
    let mut hs = Handshake::new(aa, spa, sn, &RSN_PSK_CCMP, &psk2);
    let m1 = kat_m1(&an, 1);
    let m2 = hs.on_m1(&m1);
    let micok = t[..16] == hx::<16>("effcdf6ae5eb2fa2d27416d5f184df9c")
        && m2.as_ref().map_or(false, |m| m.len() == 121 && m[EK_MIC] == pin && mic(&k[..16], m) == pin);
    // 3. unwrap: RFC 3394 §4.1 through the key-data path (KEK 000102..0f, 128-bit key data), and a flipped octet refused.
    let kek: [u8; 16] = core::array::from_fn(|i| i as u8);
    let mut wrapped: [u8; 24] = hx("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
    let good = unwrap_kd(&kek, &wrapped).map_or(false, |p| p[..] == hx::<16>("00112233445566778899aabbccddeeff")[..]);
    wrapped[9] ^= 1;
    let unwrap = good && unwrap_kd(&kek, &wrapped) == Err(Why::Unwrap) && unwrap_kd(&kek, &wrapped[..16]) == Err(Why::Unwrap);
    // 4. frame: message 1 -> 2 field by field, the authenticator's message 3 -> keys and message 4, and the refusals.
    let gtk: [u8; 16] = core::array::from_fn(|i| 0x70 + i as u8);
    let mut frame = match &m2 {
        Ok(m) => {
            m[0] == 1 && m[1] == 3 && m[2..4] == [0, 117] && m[4] == 2 && m[5..7] == [0x01, 0x0a] && m[7..9] == [0, 0]
                && m[9..17] == 1u64.to_be_bytes() && m[17..49] == sn && m[49..81].iter().all(|&b| b == 0)
                && m[97..99] == [0, 22] && m[99..] == RSN_PSK_CCMP
        }
        Err(_) => false,
    };
    let m3 = kat_m3(&k, &an, 2, &RSN_PSK_CCMP, &gtk, 1);
    frame &= match hs.on_m3(&m3) {
        Ok((m4, keys)) => {
            m4.len() == 99 && m4[2..4] == [0, 95] && m4[5..7] == [0x03, 0x0a] && m4[9..17] == 2u64.to_be_bytes()
                && m4[17..49].iter().all(|&b| b == 0) && m4[97..99] == [0, 0] && m4[EK_MIC] == mic(&k[..16], &m4)
                && keys.tk[..] == k[32..48] && keys.gtk == gtk && keys.gtk_id == 1
        }
        Err(_) => false,
    };
    // A fresh handshake for each refusal: a flipped MIC, a different beacon RSN element, a replayed counter, a wrong ANonce, and
    // frames that are not message 1 / 3 at all.
    let fresh = |rsn: &[u8]| {
        let mut h = Handshake::new(aa, spa, sn, rsn, &psk2);
        let _ = h.on_m1(&m1);
        h
    };
    let mut bad = m3.clone();
    bad[EK_MIC.start] ^= 1;
    let mut other_rsn = RSN_PSK_CCMP;
    other_rsn[13] = 2;
    let replayed = kat_m3(&k, &an, 1, &RSN_PSK_CCMP, &gtk, 1);
    let mut wrong_an = an;
    wrong_an[0] ^= 1;
    let stale = kat_m3(&k, &wrong_an, 2, &RSN_PSK_CCMP, &gtk, 1);
    frame &= fresh(&RSN_PSK_CCMP).on_m3(&bad).err() == Some(Why::Mic)
        && fresh(&other_rsn).on_m3(&m3).err() == Some(Why::RsnMismatch)
        && fresh(&RSN_PSK_CCMP).on_m3(&replayed).err() == Some(Why::Replay)
        && fresh(&RSN_PSK_CCMP).on_m3(&stale).err() == Some(Why::Frame)
        && fresh(&RSN_PSK_CCMP).on_m3(&m1).err() == Some(Why::Frame)
        && fresh(&RSN_PSK_CCMP).on_m1(&m3).err() == Some(Why::Frame)
        && fresh(&RSN_PSK_CCMP).on_m1(&m1[..60]).err() == Some(Why::Frame)
        && Handshake::new(aa, spa, sn, &RSN_PSK_CCMP, &psk2).on_m3(&m3).err() == Some(Why::Frame);
    // The Ethernet wrapper: our frame to the AP reads back as not-for-us from our side; the AP's frame to us yields the payload.
    let tx = eapol_eth(&spa, &aa, &m1);
    frame &= eapol_payload(&tx, &spa, &aa).is_none() && tx[0..6] == aa && tx[6..12] == spa && tx[12..14] == [0x88, 0x8e] && tx[14..] == m1[..];
    let rx = eapol_eth(&aa, &spa, &m1);
    frame &= eapol_payload(&rx, &spa, &aa) == Some(&m1[..]) && eapol_payload(&rx, &spa, &spa).is_none();
    // 5. rekey (S10REKEY, zenbook-ledger Z63): the group-key handshake over the KCK/KEK this handshake kept (`rekey_rows`).
    let rekey = rekey_rows(&hs, &Handshake::new(aa, spa, sn, &RSN_PSK_CCMP, &psk2), &k, &m3);
    [pbkdf2, prf, micok, unwrap, frame, rekey]
}

// ── S10REKEY (MT7925S10REKEY, zenbook-ledger Z63): the group-key handshake (IEEE Std 802.11-2020 §12.7.7), PURE — no device, no print ──
// After the 4-way handshake the AP replaces the group key with a 2-message exchange: group message 1 (group, ack, MIC, secure,
// encrypted key data: the new GTK KDE wrapped under the KEK, a replay counter above every one accepted) and our group message 2
// (group, MIC, secure, the counter echoed, zero nonce, no key data). `Handshake::rekey` keeps the KCK and KEK of a finished handshake
// as a `Rekey` (zeroized on drop; the station hands it to the driver inside its judge closure, `mt7925::rekey_arm`, dropped by
// `rekey_clear` on link_down); `Rekey::on_group_m1` judges through the ONE `ek_parse`, `mic`, `unwrap_kd`, `scan_kd` and `ek_build`.
// The IGTK KDE (S10-MFP) is walked past, not parsed. Facts from the standard; no code copied.

/// The KCK and KEK of a finished 4-way handshake and the highest replay counter accepted from the AP. Zeroized on drop.
pub struct Rekey {
    kck: [u8; 16],
    kek: [u8; 16],
    replay: u64,
}

impl Drop for Rekey {
    fn drop(&mut self) {
        self.kck.zeroize();
        self.kek.zeroize();
    }
}

impl Handshake {
    /// The rekey state of a finished handshake (message 3 accepted, message 4 built): KCK = PTK[0..16], KEK = PTK[16..32], the
    /// replay counter of message 3. None before that.
    pub fn rekey(&self) -> Option<Rekey> {
        if self.step != 2 {
            return None;
        }
        let mut r = Rekey { kck: [0u8; 16], kek: [0u8; 16], replay: self.replay };
        r.kck.copy_from_slice(&self.ptk[..16]);
        r.kek.copy_from_slice(&self.ptk[16..32]);
        Some(r)
    }
}

impl Rekey {
    /// Group message 1 (§12.7.7.2) -> (group message 2 (§12.7.7.3), the GTK's key id, the GTK, the replay counter). Checked in
    /// that order, the MIC before anything it protects: Key Information, the MIC under the KCK, the replay counter above the last
    /// accepted, the key data unwrapped under the KEK, the GTK KDE. A refused frame changes nothing; the caller zeroizes the GTK.
    pub fn on_group_m1(&mut self, f: &[u8]) -> Result<(Vec<u8>, u8, [u8; 16], u64), Why> {
        let e = ek_parse(f).ok_or(Why::Frame)?;
        if e.info & KI_JUDGED != KI_G1 {
            return Err(Why::Frame);
        }
        let want = mic(&self.kck, e.frame);
        if !ct_eq(&e.frame[EK_MIC], &want) {
            return Err(Why::Mic);
        }
        if e.replay <= self.replay {
            return Err(Why::Replay);
        }
        let mut plain = unwrap_kd(&self.kek, e.data)?;
        let r = scan_kd(&plain, None);
        plain.zeroize();
        let (gtk_id, gtk) = r?;
        let mut m2 = ek_build(e.ver, KI_G2, e.replay, &[0u8; 32], &[]);
        let m = mic(&self.kck, &m2);
        m2[EK_MIC].copy_from_slice(&m);
        self.replay = e.replay;
        Ok((m2, gtk_id, gtk, e.replay))
    }
}

/// `wpa2_kat`'s `rekey` leg: `hs` finished (the pinned PTK `k`), `fresh` not; a canned group message 1 (replay 3, GTK 80..8f, key
/// id 2) -> the GTK, its id, the counter and group message 2 field by field; the same message again (replay), a flipped MIC, key data
/// without the GTK KDE, the pairwise message 3 `m3` refused; then replay 4 accepted (the refusals changed nothing).
fn rekey_rows(hs: &Handshake, fresh: &Handshake, k: &[u8; 48], m3: &[u8]) -> bool {
    let Some(mut rk) = hs.rekey() else { return false };
    let mut ok = fresh.rekey().is_none();
    let gtk: [u8; 16] = core::array::from_fn(|i| 0x80 + i as u8);
    let mut kde = alloc::vec![0xdd, 22, 0x00, 0x0f, 0xac, 1, 2, 0];
    kde.extend_from_slice(&gtk);
    let g1 = kat_wrapped(k, KI_G1, &[0u8; 32], 3, &kde);
    ok &= match rk.on_group_m1(&g1) {
        Ok((m2, id, g, replay)) => {
            m2.len() == 99 && m2[0] == 1 && m2[1] == 3 && m2[2..4] == [0, 95] && m2[4] == 2 && m2[5..7] == [0x03, 0x02]
                && m2[7..9] == [0, 0] && m2[9..17] == 3u64.to_be_bytes() && m2[17..49].iter().all(|&b| b == 0)
                && m2[97..99] == [0, 0] && m2[EK_MIC] == mic(&k[..16], &m2) && id == 2 && g == gtk && replay == 3
        }
        Err(_) => false,
    };
    ok &= rk.on_group_m1(&g1).err() == Some(Why::Replay);
    let mut bad = kat_wrapped(k, KI_G1, &[0u8; 32], 4, &kde);
    bad[EK_MIC.start] ^= 1;
    ok &= rk.on_group_m1(&bad).err() == Some(Why::Mic);
    let mut pad = [0u8; 16];
    pad[0] = 0xdd;
    ok &= rk.on_group_m1(&kat_wrapped(k, KI_G1, &[0u8; 32], 4, &pad)).err() == Some(Why::Kde);
    ok &= rk.on_group_m1(m3).err() == Some(Why::Frame);
    ok &= rk.on_group_m1(&kat_wrapped(k, KI_G1, &[0u8; 32], 4, &kde)).map_or(false, |r| r.3 == 4);
    ok
}

// ── S9REJOIN (MT7925S9REJOIN, zenbook-ledger Z59): STATIONREJOIN's backoff (rmbp-ledger B891), MOVED verbatim from wifi/data.rs ──
// PURE. Compiled under `wifi` (data.rs is wifi10-only), so both stations hold the ONE ladder: the Broadcom's `join::REJOIN`
// (through data.rs's `pub use`) and the MT7925's `drivers/mt7925.rs` `REJOIN` (its S9REJOIN block).

/// The first re-join waits this long after the loss; each failure doubles the wait.
pub const REJOIN_FIRST_MS: u64 = 1000;
/// The longest wait between two re-join attempts.
pub const REJOIN_CAP_MS: u64 = 30_000;

/// LOST_AT / LOST_WHY and the backoff: armed by `lose`, an attempt is `due` once the wait ran out; `failed` doubles the wait
/// (capped `REJOIN_CAP_MS`), `linked` disarms it, counts the re-join and resets the ladder for the next loss.
pub struct Rejoin {
    pub lost_at: u64,
    pub why: &'static str,
    /// Attempts since the last loss.
    pub n: u32,
    /// Re-joins that linked this boot.
    pub rejoins: u32,
    delay_ms: u64,
    due_at: u64,
    armed: bool,
}

impl Rejoin {
    pub const fn new() -> Rejoin {
        Rejoin { lost_at: 0, why: "", n: 0, rejoins: 0, delay_ms: REJOIN_FIRST_MS, due_at: 0, armed: false }
    }

    pub fn lose(&mut self, now_ms: u64, why: &'static str) {
        self.lost_at = now_ms;
        self.why = why;
        self.n = 0;
        self.delay_ms = REJOIN_FIRST_MS;
        self.due_at = now_ms + REJOIN_FIRST_MS;
        self.armed = true;
    }

    pub fn due(&self, now_ms: u64) -> bool {
        self.armed && now_ms >= self.due_at
    }

    /// One attempt starts: `(its number since the loss, ms since the loss)`.
    pub fn attempt(&mut self, now_ms: u64) -> (u32, u64) {
        self.n += 1;
        (self.n, now_ms.saturating_sub(self.lost_at))
    }

    pub fn failed(&mut self, now_ms: u64) {
        self.delay_ms = (self.delay_ms * 2).min(REJOIN_CAP_MS);
        self.due_at = now_ms + self.delay_ms;
    }

    pub fn linked(&mut self) {
        self.armed = false;
        self.rejoins += 1;
        self.delay_ms = REJOIN_FIRST_MS;
    }
}

// ── MT7925S10PMKKEEP (zenbook-ledger Z64): the kept PMK through the handshake ───────────────────────────────────────────────────
/// Wipe a PMK in place (the kept plan's, in the driver, which links no `crypto_core`).
pub fn zeroize_pmk(p: &mut [u8; 32]) {
    p.zeroize();
}

/// `tests wifijoin`'s `s10pmk=`: the KEPT PMK (`mt7925::pmk_rows`' read-back; Annex J.4 test 2's PSK) through `Handshake::new` and
/// the canned message 1 / message 3 of `wpa2_kat` (same addresses and nonces): the PTK is the pinned e677aae8..., the keys come
/// out as canned (TK = PTK[32..48], the GTK and its index), and a PMK with one flipped bit is refused at message 3 by the MIC.
pub fn pmk_kat(pmk: &[u8; 32]) -> bool {
    let (aa, spa): (Mac, Mac) = (core::array::from_fn(|i| 0xa0 + i as u8), core::array::from_fn(|i| 0xb0 + i as u8));
    let (sn, an): ([u8; 32], [u8; 32]) = (core::array::from_fn(|i| 0xc0 + i as u8), core::array::from_fn(|i| 0xe0 + i as u8));
    let want: [u8; 48] = hx("e677aae82af1fb9a25e93e22ad77022ed90489e672adcc6ae148d0afafb92b6936606222ae74d4d2d1db3ac91f6c2809");
    let k = ptk(pmk, &aa, &spa, &an, &sn);
    let gtk: [u8; 16] = core::array::from_fn(|i| 0x70 + i as u8);
    let (m1, m3) = (kat_m1(&an, 1), kat_m3(&k, &an, 2, &RSN_PSK_CCMP, &gtk, 1));
    let mut hs = Handshake::new(aa, spa, sn, &RSN_PSK_CCMP, pmk);
    let good = k == want
        && hs.on_m1(&m1).is_ok()
        && match hs.on_m3(&m3) {
            Ok((_, keys)) => keys.tk[..] == k[32..48] && keys.gtk == gtk && keys.gtk_id == 1,
            Err(_) => false,
        };
    let mut wrong = *pmk;
    wrong[0] ^= 1;
    let mut hw = Handshake::new(aa, spa, sn, &RSN_PSK_CCMP, &wrong);
    let _ = hw.on_m1(&m1);
    good && hw.on_m3(&m3).err() == Some(Why::Mic)
}
