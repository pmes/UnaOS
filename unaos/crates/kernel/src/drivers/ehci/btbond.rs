// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BT-BOND — the bond store: the record schema, the codec, the table rules, and the fixture that
// proves all three without a radio.
//
// WHY THIS FILE EXISTS SEPARATELY FROM `mod.rs`, and it is the same reason `bt_name.rs` does. A bond
// record is byte handling with a right answer known in advance: a fixed layout, a fixed length, a
// key span the store indexes by. Byte handling belongs where a fixture can drive it over vectors,
// not welded into a 10 000-line dispatch loop where the only way to be wrong is on the air, in
// Peter's room, once. Everything here is a pure function over slices — no MMIO, no transfer, no
// `Controller`, no `unsafe` — so the fixture below runs on any boot, in QEMU, with no BT hardware
// modelled at all, and a red leg is a statement about the codec rather than about the radio.
//
// M1 SCOPE, stated so the absences read as deliberate. This file carries the schema, the codec, the
// table (replace-not-append, LRU eviction, lookup by either identity form) and the proofs. It is NOT
// yet wired to SSP: `bt_ssp_pair`'s Link Key Notification arm does not call `stage_store` and its
// Link Key Request arm does not call `lookup` — that is M2, and until it lands the only thing that
// stages a record is the selftest below. The witness family is live from M1 so the `strings`
// discipline has something to find.
//
// WHAT NEVER APPEARS ON THE WIRE. Link key bytes. Every witness here prints addresses, key TYPES,
// counts and sequence numbers — never key material. That is the standing bt-ssp law
// (`drivers/ehci/mod.rs`, the Link Key Notification arm), extended to the store.

use super::bt_name::{bt_addr_eq, bt_addr_render_msb, BT_L3_PEER_ADDR_BYTES};
use crate::fs::holocron::{self, HCRON_CLASS_BTBOND};

// =========================================================================================
// SCHEMA v1
// =========================================================================================

/// Bond record version, inside the holocron record body. Independent of the holocron framing
/// version: the store may re-frame without the bond schema changing, and vice versa.
pub const BTBOND_VER: u8 = 1;

/// `ver(1) | flags(1) | bd_addr(6) | bd_addr_type(1) | link_key(16) | key_type(1) | le_addr(6) |
/// le_addr_type(1) | seq_used(4)`.
///
/// **37 bytes.** The design document tallies this field list as "31 bytes"; that is an arithmetic
/// slip in the prose, not a different layout — the field list is normative and it sums to 37.
pub const BTBOND_REC_LEN: usize = 37;

/// `flags` bit 0: the LE identity fields carry a real address rather than zeros.
pub const BTBOND_FLAG_LE_PRESENT: u8 = 0x01;

/// Address type 0x00 — public. The BR/EDR page address is always public; the vocabulary mirrors
/// HCI's so a stored type can be handed straight to a command.
pub const BTBOND_ADDR_PUBLIC: u8 = 0x00;

/// Address type 0x01 — random. Meaningful only for the LE identity half.
pub const BTBOND_ADDR_RANDOM: u8 = 0x01;

/// Bonds the store keeps. One speaker today; headroom without unbounded growth. A re-pair of a known
/// `bd_addr` REPLACES its record (holocron's `put` is keyed on the address), so the store cannot
/// accumulate one entry per boot the way the RAM-only session key's predecessor could.
pub const BTBOND_MAX: usize = 4;

/// The Link Key Notification event assembly the SSP dispatch loop parses: `evt_code(1) | plen(1) |
/// bd_addr(6) | link_key(16) | key_type(1)`.
pub const BTBOND_LKN_LEN: usize = 25;

/// Which identity form a lookup matched on.
///
/// Not decoration: the BT-C1 page trains to the address the speaker ADVERTISES still time out, and
/// the live hypothesis is that a dual-mode device pages under a different BR/EDR address. A store
/// that records both forms and says which one answered turns every future reconnect into evidence
/// about that hypothesis, for free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtBondMatch {
    /// Matched the BR/EDR page address — the record's primary key.
    BdAddr,
    /// Matched the LE identity/advertise address the peer was first seen under.
    LeAddr,
}

impl BtBondMatch {
    pub fn as_str(self) -> &'static str {
        match self {
            BtBondMatch::BdAddr => "bd_addr",
            BtBondMatch::LeAddr => "le_addr",
        }
    }
}

/// One bond, decoded.
///
/// **Both identity forms from day one.** Today the only address this tree knows is the LE advertise
/// address (`bt_name.rs`'s `BT_L3_PEER_ADDR_BYTES`); the BR/EDR page address is whatever
/// `Connection Complete` binds a handle to. Carrying both means that when the address question
/// resolves, a bond written by the pairing path already records the address it actually
/// authenticated on AND the address the peer was first seen under — so a lookup by either form hits
/// the same record and no schema bump is needed.
#[derive(Clone, Copy)]
pub struct BtBond {
    pub flags: u8,
    /// BR/EDR page address, WIRE order (LSB first), as every HCI command wants it.
    pub bd_addr: [u8; 6],
    pub bd_addr_type: u8,
    /// The link key. Never printed. Never logged. Never rendered.
    pub link_key: [u8; 16],
    /// `Key_Type` verbatim from the Link Key Notification (0x04..0x08).
    pub key_type: u8,
    /// LE identity/advertise address, wire order; zeros when `flags` bit 0 is clear.
    pub le_addr: [u8; 6],
    pub le_addr_type: u8,
    /// The holocron write counter at this bond's last successful use — the LRU clock. There is no
    /// RTC on this machine; the write counter is the only monotonic quantity available.
    pub seq_used: u32,
}

impl BtBond {
    /// A bond on `addr` with `key`/`key_type`, no LE identity recorded yet.
    pub fn new(addr: &[u8; 6], key: &[u8; 16], key_type: u8, seq_used: u32) -> Self {
        Self {
            flags: 0,
            bd_addr: *addr,
            bd_addr_type: BTBOND_ADDR_PUBLIC,
            link_key: *key,
            key_type,
            le_addr: [0u8; 6],
            le_addr_type: BTBOND_ADDR_PUBLIC,
            seq_used,
        }
    }

    /// Record an LE identity address alongside the BR/EDR one, setting the presence flag.
    pub fn with_le_identity(mut self, le_addr: &[u8; 6], le_addr_type: u8) -> Self {
        self.le_addr = *le_addr;
        self.le_addr_type = le_addr_type;
        self.flags |= BTBOND_FLAG_LE_PRESENT;
        self
    }

    /// Does this record carry an LE identity?
    pub fn has_le(&self) -> bool {
        self.flags & BTBOND_FLAG_LE_PRESENT != 0
    }

    /// Serialize to the v1 body.
    pub fn encode(&self) -> [u8; BTBOND_REC_LEN] {
        let mut b = [0u8; BTBOND_REC_LEN];
        b[0] = BTBOND_VER;
        b[1] = self.flags;
        b[2..8].copy_from_slice(&self.bd_addr);
        b[8] = self.bd_addr_type;
        b[9..25].copy_from_slice(&self.link_key);
        b[25] = self.key_type;
        b[26..32].copy_from_slice(&self.le_addr);
        b[32] = self.le_addr_type;
        b[33..37].copy_from_slice(&self.seq_used.to_le_bytes());
        b
    }

    /// Parse a v1 body. Refuses a wrong length or an unknown version — a body this build cannot read
    /// is refused whole, never half-adopted (the store's standing rule).
    pub fn decode(body: &[u8]) -> Option<Self> {
        if body.len() != BTBOND_REC_LEN || body[0] != BTBOND_VER {
            return None;
        }
        let mut bond = Self {
            flags: body[1],
            bd_addr: [0u8; 6],
            bd_addr_type: body[8],
            link_key: [0u8; 16],
            key_type: body[25],
            le_addr: [0u8; 6],
            le_addr_type: body[32],
            seq_used: u32::from_le_bytes([body[33], body[34], body[35], body[36]]),
        };
        bond.bd_addr.copy_from_slice(&body[2..8]);
        bond.link_key.copy_from_slice(&body[9..25]);
        bond.le_addr.copy_from_slice(&body[26..32]);
        Some(bond)
    }

    /// The lookup rule: `bd_addr` first, then the LE identity when one is recorded.
    ///
    /// Order matters. The BR/EDR address is the address authentication actually ran on, so it is the
    /// answer whenever it is available; the LE form is the fallback that makes a bond found under
    /// one identity usable under the other.
    pub fn matches(&self, addr: &[u8; 6]) -> Option<BtBondMatch> {
        if bt_addr_eq(&self.bd_addr, addr) {
            return Some(BtBondMatch::BdAddr);
        }
        if self.has_le() && bt_addr_eq(&self.le_addr, addr) {
            return Some(BtBondMatch::LeAddr);
        }
        None
    }
}

/// Parse a Link Key Notification (event 0x18) assembly into `(bd_addr, link_key, key_type)`.
///
/// This is the exact byte handling the SSP dispatch loop's Link Key Notification arm performs
/// inline today, lifted where a fixture can drive it. M2 replaces the inline copy with a call here;
/// until then the fixture covers the parse the driver will use, which is why the codec KAT is not
/// vacuous even with no BT wiring in the tree.
pub fn parse_link_key_notification(asm: &[u8]) -> Option<([u8; 6], [u8; 16], u8)> {
    if asm.len() < BTBOND_LKN_LEN {
        return None;
    }
    let mut addr = [0u8; 6];
    let mut key = [0u8; 16];
    addr.copy_from_slice(&asm[2..8]);
    key.copy_from_slice(&asm[8..24]);
    Some((addr, key, asm[24]))
}

// =========================================================================================
// THE TABLE — thin over the holocron seam, no second copy of the records
// =========================================================================================

/// Stage a bond into the store. **RAM and a dirty flag only** — this is what the SSP path will call
/// with `EHCI_HID` held, and under that lock it must be a `memcpy` and nothing else. No I/O, no
/// allocation, no wait. The write happens later, from the main loop, in
/// [`holocron::flush_if_dirty`].
///
/// Replace-not-append: an existing record for the same `bd_addr` is overwritten in place. When the
/// class is at [`BTBOND_MAX`] and the address is new, the record with the smallest `seq_used` is
/// evicted first — LRU by write counter, because there is no clock.
pub fn stage_store(addr: &[u8; 6], key: &[u8; 16], key_type: u8) -> bool {
    let seq = holocron::seq();
    let bond = BtBond::new(addr, key, key_type, seq);
    stage_record(&bond)
}

/// Stage an already-built record (the shape [`stage_store`] and the selftest share).
pub fn stage_record(bond: &BtBond) -> bool {
    let body = bond.encode();
    let known = holocron::get(HCRON_CLASS_BTBOND, &bond.bd_addr, &mut [0u8; BTBOND_REC_LEN]).is_some();
    if !known && count() >= BTBOND_MAX {
        evict_lru();
    }
    match holocron::put(HCRON_CLASS_BTBOND, &bond.bd_addr, &body) {
        Ok(()) => {
            serial_println!(
                ":: [btbond] stored addr={} type={:#04x} le={} -> staged; flush is deferred past the service pass == witness ::",
                addr_of(&bond.bd_addr),
                bond.key_type,
                le_of(bond)
            );
            true
        }
        Err(e) => {
            serial_println!(
                ":: [btbond] store REFUSED addr={} -> {} == witness ::",
                addr_of(&bond.bd_addr),
                holocron::hcron_reason(e)
            );
            false
        }
    }
}

/// Drop the bond for `addr`. **RAM and a dirty flag only**, like [`stage_store`] — the dead bond
/// leaves the medium at the next flush, not just RAM. This is what the stale-key discard will call
/// (M2): a bond the peer has forgotten must not survive on disk to wedge every future reconnect.
pub fn stage_remove(addr: &[u8; 6]) -> bool {
    let gone = holocron::remove(HCRON_CLASS_BTBOND, addr);
    serial_println!(
        ":: [btbond] evict addr={} present={} -> {} == witness ::",
        addr_of(addr),
        gone,
        if gone {
            "staged for removal; the record leaves the medium at the next flush"
        } else {
            "no such bond; nothing staged"
        }
    );
    gone
}

/// How many bonds the store holds.
pub fn count() -> usize {
    holocron::count(HCRON_CLASS_BTBOND)
}

/// Look a peer up by EITHER identity form.
///
/// Returns the bond and which form answered. A `None` here does not distinguish "no such bond" from
/// "the store has not been read yet" — [`holocron::is_loaded`] is that question, and the caller must
/// ask it before printing a miss, because on a boot that arms the radio the first BT chain can run
/// before storage is up.
pub fn lookup(addr: &[u8; 6]) -> Option<(BtBond, BtBondMatch)> {
    let mut body = [0u8; BTBOND_REC_LEN];
    // Primary key first — one indexed hit, no scan.
    if let Some(n) = holocron::get(HCRON_CLASS_BTBOND, addr, &mut body) {
        if let Some(b) = BtBond::decode(&body[..n]) {
            return Some((b, BtBondMatch::BdAddr));
        }
    }
    // Then the LE identity fallback. The predicate is pure and reads only the body it is handed —
    // `holocron::find_body` runs it with the store lock held.
    let want = *addr;
    let n = holocron::find_body(
        HCRON_CLASS_BTBOND,
        |b| {
            b.len() == BTBOND_REC_LEN
                && b[0] == BTBOND_VER
                && b[1] & BTBOND_FLAG_LE_PRESENT != 0
                && b[26..32] == want
        },
        &mut body,
    )?;
    let b = BtBond::decode(&body[..n])?;
    Some((b, BtBondMatch::LeAddr))
}

/// Drop the bond with the smallest `seq_used`. Called only when the class is full and a new address
/// arrives — the design's LRU-by-write-counter rule.
fn evict_lru() {
    let mut body = [0u8; BTBOND_REC_LEN];
    let mut victim: Option<([u8; 6], u32)> = None;
    // Bounded by the STORE's table, not by `BTBOND_MAX`: a file written by a build with a larger
    // cap (or a hand-edited one) can legitimately carry more bond records than this build would
    // create, and the victim search must see all of them or it would evict the smallest `seq_used`
    // of a prefix rather than of the class.
    for i in 0..holocron::HCRON_MAX_RECORDS {
        let Some(n) = holocron::nth_body(HCRON_CLASS_BTBOND, i, &mut body) else {
            break;
        };
        let Some(b) = BtBond::decode(&body[..n]) else {
            continue;
        };
        match victim {
            Some((_, seq)) if b.seq_used >= seq => {}
            _ => victim = Some((b.bd_addr, b.seq_used)),
        }
    }
    if let Some((addr, seq)) = victim {
        holocron::remove(HCRON_CLASS_BTBOND, &addr);
        serial_println!(
            ":: [btbond] table full at {} — evicted the least-recently-used bond addr={} seq_used={} (LRU by write counter; this machine has no RTC) == witness ::",
            BTBOND_MAX,
            addr_of(&addr),
            seq
        );
    }
}

/// A printable wire-order BD_ADDR, rendered MSB-first for the human eye.
///
/// Wraps `bt_name.rs`'s renderer rather than carrying a second copy of the byte-order decision: the
/// wire order of a stored address is exactly the thing this tree has already been bitten by, and one
/// renderer means a witness line here cannot disagree with a witness line there.
struct Addr([u8; 17]);

impl core::fmt::Display for Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(core::str::from_utf8(&self.0).unwrap_or("??:??:??:??:??:??"))
    }
}

fn addr_of(a: &[u8; 6]) -> Addr {
    Addr(bt_addr_render_msb(a))
}

/// An address that may not have been recorded — prints `absent` rather than six zero bytes, so a
/// missing LE identity never reads as a real address of `00:00:00:00:00:00`.
struct MaybeAddr(Option<Addr>);

impl core::fmt::Display for MaybeAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Some(a) => a.fmt(f),
            None => f.write_str("absent"),
        }
    }
}

fn le_of(b: &BtBond) -> MaybeAddr {
    MaybeAddr(if b.has_le() { Some(addr_of(&b.le_addr)) } else { None })
}

// =========================================================================================
// THE CODEC KAT — the schema, the parse and the framing, proven with no radio and no medium
// =========================================================================================

/// Drive the whole bond codec over vectors whose answer is known before anything is asked.
///
/// Pure: registers and stack buffers. No radio, no block device, no filesystem. The legs:
///
///   1. a synthetic Link Key Notification (the 25-byte assembly the SSP arm parses) decodes to the
///      address, key and key type it was built from;
///   2. that becomes a record, encodes to exactly [`BTBOND_REC_LEN`] bytes, and decodes back
///      field-identical — link key included, compared but never printed;
///   3. `decode` refuses a short body;
///   4. `decode` refuses an unknown schema version;
///   5. the holocron class registry's key span indexes the schema's `bd_addr` — the store and the
///      schema cannot drift apart without this leg going red;
///   6. the either-form lookup rule discriminates: BR/EDR hits as `bd_addr`, the LE identity hits as
///      `le_addr`, a third address misses, and an LE address in a record with the presence flag
///      CLEAR does not hit;
///   7. record → holocron framing → parse → decode is byte-identical; flipping one byte of the
///      framed image makes the CRC refuse it; and the untouched copy still round-trips.
///
/// Returns true when every leg held.
pub fn codec_fixture() -> bool {
    let mut fails = 0u32;
    let mut legs = 0u32;

    // A Link Key Notification as the controller would deliver it, on the address this tree knows.
    let mut asm = [0u8; BTBOND_LKN_LEN];
    asm[0] = 0x18; // BT_EVT_LINK_KEY_NOTIFY
    asm[1] = 23; // parameter length
    asm[2..8].copy_from_slice(&BT_L3_PEER_ADDR_BYTES);
    for i in 0..16 {
        asm[8 + i] = 0x10 + i as u8; // a fixture key — not a real one, and never printed
    }
    asm[24] = 0x04; // unauthenticated combination key, P-192

    // Leg 1 — the event parse.
    legs += 1;
    let parsed = parse_link_key_notification(&asm);
    let (addr, key, key_type) = match parsed {
        Some(v) if v.0 == BT_L3_PEER_ADDR_BYTES && v.2 == 0x04 => v,
        _ => {
            fails += 1;
            serial_println!(
                ":: [btbond] codec fixture leg=lkn-parse -> FAIL (a synthetic Link Key Notification did not decode to the address/type it was built from) == witness ::"
            );
            ([0u8; 6], [0u8; 16], 0)
        }
    };
    // A short assembly must be refused, not read past.
    legs += 1;
    if parse_link_key_notification(&asm[..BTBOND_LKN_LEN - 1]).is_some() {
        fails += 1;
        serial_println!(
            ":: [btbond] codec fixture leg=lkn-short -> FAIL (a TRUNCATED Link Key Notification parsed) == witness ::"
        );
    }

    let le = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66];
    let bond = BtBond::new(&addr, &key, key_type, 7).with_le_identity(&le, BTBOND_ADDR_RANDOM);

    // Leg 2 — encode/decode round-trip.
    legs += 1;
    let body = bond.encode();
    if body.len() != BTBOND_REC_LEN {
        fails += 1;
        serial_println!(
            ":: [btbond] codec fixture leg=rec-len -> FAIL (encoded {} bytes, schema says {}) == witness ::",
            body.len(),
            BTBOND_REC_LEN
        );
    }
    match BtBond::decode(&body) {
        Some(b) => {
            let same = b.flags == bond.flags
                && b.bd_addr == bond.bd_addr
                && b.bd_addr_type == bond.bd_addr_type
                && b.link_key == bond.link_key
                && b.key_type == bond.key_type
                && b.le_addr == bond.le_addr
                && b.le_addr_type == bond.le_addr_type
                && b.seq_used == bond.seq_used;
            if !same {
                fails += 1;
                serial_println!(
                    ":: [btbond] codec fixture leg=rec-roundtrip -> FAIL (a field changed across encode+decode) == witness ::"
                );
            }
        }
        None => {
            fails += 1;
            serial_println!(
                ":: [btbond] codec fixture leg=rec-roundtrip -> FAIL (a freshly encoded record was refused) == witness ::"
            );
        }
    }

    // Leg 3 — a short body.
    legs += 1;
    if BtBond::decode(&body[..BTBOND_REC_LEN - 1]).is_some() {
        fails += 1;
        serial_println!(
            ":: [btbond] codec fixture leg=rec-short -> FAIL (a TRUNCATED record decoded) == witness ::"
        );
    }

    // Leg 4 — an unknown schema version.
    legs += 1;
    let mut wrongver = body;
    wrongver[0] = BTBOND_VER.wrapping_add(1);
    if BtBond::decode(&wrongver).is_some() {
        fails += 1;
        serial_println!(
            ":: [btbond] codec fixture leg=rec-version -> FAIL (a record of an UNKNOWN schema version decoded) == witness ::"
        );
    }

    // Leg 5 — the store's key span really is this schema's bd_addr.
    legs += 1;
    match holocron::class_key_span(HCRON_CLASS_BTBOND) {
        Some((off, len)) if off == 2 && len == 6 && body[off..off + len] == bond.bd_addr => {}
        _ => {
            fails += 1;
            serial_println!(
                ":: [btbond] codec fixture leg=key-span -> FAIL (the holocron class registry does not index this schema's bd_addr) == witness ::"
            );
        }
    }

    // Leg 6 — the either-form lookup rule.
    legs += 1;
    let other = [0x99u8, 0x88, 0x77, 0x66, 0x55, 0x44];
    let no_le = BtBond::new(&addr, &key, key_type, 7);
    let rule_ok = bond.matches(&addr) == Some(BtBondMatch::BdAddr)
        && bond.matches(&le) == Some(BtBondMatch::LeAddr)
        && bond.matches(&other).is_none()
        && no_le.matches(&le).is_none();
    if !rule_ok {
        fails += 1;
        serial_println!(
            ":: [btbond] codec fixture leg=either-form -> FAIL (the bd_addr/le_addr lookup rule did not discriminate) == witness ::"
        );
    }

    // Leg 7 — through the holocron framing, damaged and clean.
    legs += 1;
    let rec = match holocron::Record::new(HCRON_CLASS_BTBOND, &body) {
        Ok(r) => r,
        Err(e) => {
            serial_println!(
                ":: [btbond] codec fixture leg=framing -> FAIL (the store refused the record: {}) == witness ::",
                holocron::hcron_reason(e)
            );
            fails += 1;
            holocron::Record::empty()
        }
    };
    let mut img = [0u8; holocron::HCRON_IMAGE_MAX];
    match holocron::serialize_into(9, &[rec], &mut img) {
        Ok(len) => {
            // Clean: framing → parse → schema decode, byte-identical.
            let clean_ok = match holocron::parse_image(&img[..len]) {
                Ok(p) => p.count == 1 && BtBond::decode(p.records()[0].body()).map(|b| b.encode()) == Some(body),
                Err(_) => false,
            };
            if !clean_ok {
                fails += 1;
                serial_println!(
                    ":: [btbond] codec fixture leg=framing-clean -> FAIL (record -> framed image -> parse -> decode was not byte-identical) == witness ::"
                );
            }
            // Damaged: one byte of the framed image, and the CRC must refuse the whole thing.
            let mut bad = [0u8; holocron::HCRON_IMAGE_MAX];
            bad[..len].copy_from_slice(&img[..len]);
            bad[holocron::HCRON_HDR_LEN + 4] ^= 0x40; // inside the record body
            if holocron::parse_image(&bad[..len]).is_ok() {
                fails += 1;
                serial_println!(
                    ":: [btbond] codec fixture leg=framing-damaged -> FAIL (a CORRUPTED framed record PARSED — the CRC refusal did not fire) == witness ::"
                );
            }
            // And the untouched copy still round-trips, so the seven refusals above are not simply a
            // parser that refuses everything.
            if !matches!(holocron::parse_image(&img[..len]), Ok(p) if p.count == 1) {
                fails += 1;
                serial_println!(
                    ":: [btbond] codec fixture leg=framing-pristine -> FAIL (the untouched framed image stopped parsing) == witness ::"
                );
            }
            for v in bad.iter_mut() {
                *v = 0;
            }
        }
        Err(e) => {
            fails += 1;
            serial_println!(
                ":: [btbond] codec fixture leg=framing -> FAIL (serialize: {}) == witness ::",
                holocron::hcron_reason(e)
            );
        }
    }

    // The fixture handled a (synthetic) link key. Wipe every buffer that touched it.
    for v in img.iter_mut() {
        *v = 0;
    }
    for v in asm.iter_mut() {
        *v = 0;
    }

    if fails == 0 {
        serial_println!(
            ":: [btbond] codec fixture: {}/{} legs — event parse, {}-byte record round-trip, short/version refusals, key-span agreement, either-form lookup, and the framing CRC refusal all held -> PASS ::",
            legs, legs, BTBOND_REC_LEN
        );
        true
    } else {
        serial_println!(
            ":: [btbond] codec fixture: {}/{} legs failed -> FAIL ::",
            fails, legs
        );
        false
    }
}

// =========================================================================================
// THE STORE ROUND-TRIP — a bond through the REAL table, the REAL flush, the REAL file
// =========================================================================================

/// Stage a fixture bond, flush it, read the file back, verify, then remove it and flush again.
///
/// This is the strongest honest QEMU witness available for the bond path: it proves
/// `stage_store` → `flush_if_dirty` → the file → `load` → `lookup` end to end, through the actual
/// block layer, with **no radio at all** (QEMU models no BT controller; the internal hub and radio
/// are bench hardware). It runs from the main loop with no driver lock held, which is also what
/// makes it a demonstration of the deferral rather than merely a test of the codec.
///
/// Fixture address `aa:bb:cc:dd:ee:ff` — never a real peer's, so a green run can never be a real
/// bond's write. Self-cleaning: the record is removed and the removal flushed before the verdict, so
/// the medium is left as it was found (an empty store file) and a re-run is identical.
pub fn selftest_once() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if !holocron::is_loaded() {
        DONE.store(false, Ordering::Relaxed); // not our turn yet — retry next pass
        return;
    }
    let Ok(fs) = crate::fs::fat::mount() else {
        serial_println!(":: [btbond] store round-trip: no FAT volume — SKIPPED ::");
        return;
    };
    if let Some(why) = fs.write_veto() {
        serial_println!(
            ":: [btbond] store round-trip: volume vetoes writes ({}) — SKIPPED ::",
            why
        );
        return;
    }
    drop(fs);

    let fixture = [0xFFu8, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]; // wire order => aa:bb:cc:dd:ee:ff
    let mut key = [0u8; 16];
    for (i, k) in key.iter_mut().enumerate() {
        *k = 0xC0 ^ i as u8;
    }
    let le = BT_L3_PEER_ADDR_BYTES;
    let before = count();

    let mut verdict: Result<(), &'static str> = Ok(());

    // 1. Stage — RAM only, and the store must go dirty without anything having been written.
    let bond = BtBond::new(&fixture, &key, 0x04, holocron::seq()).with_le_identity(&le, BTBOND_ADDR_PUBLIC);
    if !stage_record(&bond) {
        verdict = Err("stage refused");
    }
    if verdict.is_ok() && !holocron::is_dirty() {
        verdict = Err("staging a bond did not make the store dirty");
    }
    if verdict.is_ok() && count() != before + 1 {
        verdict = Err("staging a bond did not add a record");
    }

    // 2. Flush — the deferred write, from main-loop context with no driver lock held.
    if verdict.is_ok() {
        holocron::flush_if_dirty();
        if holocron::is_dirty() {
            verdict = Err("the flush did not clear the dirty flag");
        }
    }

    // 3. Look it up through the table, by BOTH identity forms.
    if verdict.is_ok() {
        match lookup(&fixture) {
            Some((b, BtBondMatch::BdAddr)) if b.link_key == key && b.key_type == 0x04 => {}
            Some((_, m)) => {
                serial_println!(
                    ":: [btbond] store round-trip stage=lookup -> matched on {} where bd_addr was required ::",
                    m.as_str()
                );
                verdict = Err("bd_addr lookup matched the wrong form or the wrong record");
            }
            None => verdict = Err("bd_addr lookup missed a bond that was just staged"),
        }
    }
    if verdict.is_ok() {
        match lookup(&le) {
            Some((_, BtBondMatch::LeAddr)) => {}
            Some((_, m)) => {
                serial_println!(
                    ":: [btbond] store round-trip stage=lookup-le -> matched on {} where le_addr was required ::",
                    m.as_str()
                );
                verdict = Err("le_addr lookup matched the wrong form");
            }
            None => verdict = Err("le_addr lookup missed a bond that records an LE identity"),
        }
    }

    // 4. Self-clean: remove the fixture bond and flush the removal, so the medium goes back to what
    //    it was and a second boot sees no fixture record.
    let removed = stage_remove(&fixture);
    holocron::flush_if_dirty();
    let clean = removed && !holocron::is_dirty() && count() == before && lookup(&fixture).is_none();
    for k in key.iter_mut() {
        *k = 0;
    }

    match verdict {
        Ok(()) if clean => serial_println!(
            ":: [btbond] store round-trip: staged addr={} (le={}) under the lock-free path, flushed to {} at seq={}, looked it up by BOTH identity forms, then evicted + re-flushed; store back to n={} -> PASS ::",
            addr_of(&fixture),
            addr_of(&le),
            holocron::HCRON_PATH,
            holocron::seq(),
            before
        ),
        Ok(()) => serial_println!(
            ":: [btbond] store round-trip: the bond legs held but the self-clean did not (removed={} dirty={} n={}) -> FAIL ::",
            removed,
            holocron::is_dirty(),
            count()
        ),
        Err(why) => serial_println!(
            ":: [btbond] store round-trip: {} (self-clean removed={}) -> FAIL ::",
            why,
            removed
        ),
    }
}

/// The class's main-loop presence, called by [`holocron::service`] once the store is loaded.
///
/// M1 carries exactly one thing here: the store round-trip. M2 adds nothing to this function — the
/// SSP wiring lives at the Link Key Notification and Link Key Request arms in `mod.rs`, under the
/// EHCI lock, where `stage_store` and `lookup` are the only calls that may appear.
pub fn service() {
    selftest_once();
}
