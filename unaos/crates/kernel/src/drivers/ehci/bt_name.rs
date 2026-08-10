// BT-L2 — the advertised Local Name: the AD-structure walk, the match rules, and the fixture
// that proves the walk on payloads whose answer is known in advance. Since GR23 it also carries
// BT-L3's ADDRESS match (`bt_addr_matches` and below) for exactly the same reasons: it is pure
// byte handling with a right answer known in advance, so it belongs where a fixture and the
// host harness can drive the same source the radio path runs.
//
// WHY THIS IS ITS OWN FILE. Boot AR heard Peter's MEGABOOM — `addr=88:c6:26:cc:2d:3c`, byte for
// byte the address the host's own stack reports for it — and rendered its name as `"."`, one
// character. The name filter then correctly refused a device whose name did not contain
// "MEGABOOM", and the capture recorded a clean, plausible-looking no-match. The first suspicion
// was this walk: an AD `Length` COUNTS THE TYPE BYTE, and the classic off-by-one on that produces
// exactly a one-character result.
//
// It is not the walk (see `bt_decode_local_name`, and `BT_NAME_CASES` which proves it). But
// "I read the code and it looks right" is not evidence, and the walk had no way to be WRONG in a
// way a capture could see. So the walk moved here, where a fixture can drive it over payloads
// whose decode is known in advance — and where the same source can be exercised without a radio,
// because none of it touches hardware.
//
// Nothing in this file performs I/O or reads a register. It is pure byte handling over a slice,
// which is what lets `bt_name_fixture` run it anywhere and what lets the host-side harness in
// `tools/btname_harness.rs` `include!`s this exact file rather than a copy of it. THE SHARED-SOURCE
// LAW IS THE POINT: a harness that tests a transcription proves nothing about the kernel.

/// AD structure types carrying a device name (Bluetooth Core Supplement / Assigned Numbers,
/// Generic Access Profile): 0x08 Shortened Local Name, 0x09 Complete Local Name.
pub const BT_AD_NAME_SHORT: u8 = 0x08;
pub const BT_AD_NAME_COMPLETE: u8 = 0x09;

/// Cap on the local-name bytes kept per device. AD names run to 29 bytes; 24 keeps the witness
/// line one serial line without eliding the distinguishing part of a real name. A name cut at the
/// cap is printed with a trailing `~`.
pub const BT_L2_NAME_MAX: usize = 24;

/// Cap on the RAW AD payload bytes kept per device for the witness. A legacy advertising PDU
/// carries at most 31 octets of AD data, so 31 is the whole of it and never a prefix in practice —
/// the `cut` flag exists only so a controller that reports more cannot silently lose the tail.
pub const BT_L2_RAW_MAX: usize = 31;

/// Below this many decoded characters, a name is not evidence of anything and the RAW payload is
/// witnessed alongside it. Boot AR's `"."` is the reason the threshold exists: one character is
/// indistinguishable from a misparse by eye, and the capture offered nothing to check it against.
pub const BT_NAME_SUSPECT_LEN: usize = 3;

/// The decoded Local Name, and the two facts about it that decide what a match MEANS.
#[derive(Clone, Copy)]
pub struct BtName {
    pub name: [u8; BT_L2_NAME_MAX],
    pub nlen: usize,
    /// Set when the name was cut at `BT_L2_NAME_MAX`.
    pub ncut: bool,
    /// Set when the name came from a COMPLETE Local Name (AD type 0x09) rather than a Shortened
    /// one (0x08). A shortened name is a PREFIX of the complete one, so this decides both whether
    /// a later report may replace the stored name and whether a name MISS is final or merely
    /// unproven.
    pub ncomplete: bool,
}

impl BtName {
    pub const fn empty() -> Self {
        BtName { name: [0; BT_L2_NAME_MAX], nlen: 0, ncut: false, ncomplete: false }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.name[..self.nlen]
    }
}

/// Walk the AD structures of one advertising payload and return the Local Name it carries.
///
/// AD structures are a sequence of (Length(1), AD_Type(1), AD_Data(Length-1)) — **the Length octet
/// counts the type octet but not itself**, which is why the data slice is `[off+2 .. off+1+l]` and
/// is `l - 1` bytes long. That single expression is the off-by-one the boot-AR investigation
/// suspected; `BT_NAME_CASES` pins it, and breaking it fails `megaboom-complete` loudly.
///
/// A Complete Local Name (0x09) ends the walk — the device has said there is no longer name to
/// wait for. A Shortened one (0x08) is kept but the walk continues in case the complete name
/// follows in the same payload.
pub fn bt_decode_local_name(data: &[u8]) -> BtName {
    let dlen = data.len();
    let mut out = BtName::empty();
    let mut off = 0usize;
    while off + 2 <= dlen {
        let l = data[off] as usize;
        if l == 0 || off + 1 + l > dlen {
            break; // 0 = end of significant part; over-long = malformed tail, stop
        }
        let ty = data[off + 1];
        if ty == BT_AD_NAME_COMPLETE || ty == BT_AD_NAME_SHORT {
            let src = &data[off + 2..off + 1 + l];
            // An EMPTY name field (Length==1: type byte only, no data) is legal on the air and
            // carries no name. Without this guard a Complete Local Name of zero bytes ERASED a
            // Shortened name captured earlier in the same walk — a device that advertises "Pete"
            // then an empty complete name would print name=(none). An empty field is skipped; a
            // COMPLETE one still ends the walk.
            if !src.is_empty() {
                let take = src.len().min(BT_L2_NAME_MAX);
                out.name[..take].copy_from_slice(&src[..take]);
                out.nlen = take;
                out.ncut = src.len() > BT_L2_NAME_MAX;
                out.ncomplete = ty == BT_AD_NAME_COMPLETE;
            }
            if ty == BT_AD_NAME_COMPLETE {
                break;
            }
        }
        off += 1 + l;
    }
    out
}

/// ASCII-only folding on purpose: advertised Local Names are UTF-8 on the air, and a correct
/// Unicode case fold is a table this driver has no business carrying. Every byte outside ASCII is
/// compared verbatim, so a non-ASCII name still matches itself exactly — what it will not do is
/// match a differently-cased form of itself, which is a miss and never a false hit.
///
/// An EMPTY needle matches everything, so the caller must treat `Some("")` as "no filter" rather
/// than let it silently admit the whole room; it is refused explicitly at the call site.
pub fn bt_name_contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return needle.is_empty();
    }
    let fold = |b: u8| if b.is_ascii_uppercase() { b + 32 } else { b };
    for start in 0..=(hay.len() - needle.len()) {
        let mut all = true;
        for k in 0..needle.len() {
            if fold(hay[start + k]) != fold(needle[k]) {
                all = false;
                break;
            }
        }
        if all {
            return true;
        }
    }
    false
}

/// BT-L3 — could `needle` still be found in a name of which `hay` is only a PREFIX?
///
/// A Shortened Local Name (AD type 0x08) is a prefix of the complete one, so a miss against it is
/// not a miss against the device — the rest of the name was simply never heard. The match can only
/// have been lost by STRADDLING the cut, which means some non-empty suffix of `hay` equals the
/// corresponding prefix of `needle`. (A whole containment is a real hit and is tested first by the
/// caller, so this answers the strictly weaker question.)
///
/// The empty suffix is deliberately excluded: every string trivially ends with the empty prefix,
/// and admitting it would make EVERY shortened name a maybe, which is the same as no signal at all.
pub fn bt_name_maybe_ci(hay: &[u8], needle: &[u8]) -> bool {
    let fold = |b: u8| if b.is_ascii_uppercase() { b + 32 } else { b };
    let maxk = hay.len().min(needle.len());
    for k in (1..=maxk).rev() {
        let tail = &hay[hay.len() - k..];
        let mut all = true;
        for j in 0..k {
            if fold(tail[j]) != fold(needle[j]) {
                all = false;
                break;
            }
        }
        if all {
            return true;
        }
    }
    false
}

// -------------------------------------------------------------------------------------------
// THE FIXTURE — payloads whose decode is known before the walk runs.
// -------------------------------------------------------------------------------------------

/// One fixture leg: a synthetic advertising payload and the decode it MUST produce.
pub struct BtNameCase {
    /// Short name for the witness line.
    pub what: &'static str,
    /// The AD payload exactly as it would arrive in an `LE Advertising Report`'s `Data` field.
    pub data: &'static [u8],
    pub want_name: &'static [u8],
    pub want_complete: bool,
    pub want_cut: bool,
    /// Why this leg exists — printed on FAILURE so a red fixture explains itself in the capture.
    pub why: &'static str,
}

/// A 25-character name: one past `BT_L2_NAME_MAX`, so the cut path is exercised by a payload that
/// is legal on the air rather than by a special case.
const LONG_NAME_AD: &[u8] = b"\x1a\x09ABCDEFGHIJKLMNOPQRSTUVWXY";

pub const BT_NAME_CASES: &[BtNameCase] = &[
    BtNameCase {
        what: "megaboom-complete",
        // Flags(0x01) | Complete 16-bit UUID list carrying UE's 0x61FE | Complete Local Name.
        // This is the shape a real connectable advertiser uses, so the walk is proven stepping
        // OVER two structures before it reaches the name, not just on a name in isolation.
        data: b"\x02\x01\x06\x03\x03\xfe\x61\x09\x09MEGABOOM",
        want_name: b"MEGABOOM",
        want_complete: true,
        want_cut: false,
        why: "AD Length counts the TYPE byte: a Complete Local Name of 8 chars declares Length 9. \
               An off-by-one here yields a 1-char name — exactly Boot AR's \".\" signature",
    },
    BtNameCase {
        what: "megaboom-observed-one-char",
        // THE BOOT AR SIGNATURE, pinned. A name AD of Length 2 carries exactly one data byte.
        // If the walk is correct then `"."` is what a one-byte name field genuinely decodes to,
        // and the misparse theory has to explain the payload, not the parser.
        data: b"\x02\x01\x06\x02\x09\x2e",
        want_name: b".",
        want_complete: true,
        want_cut: false,
        why: "a one-byte Complete Local Name decodes to one byte; this leg is what makes \
               \"the parser invented it\" and \"the air carried it\" different predictions",
    },
    BtNameCase {
        what: "shortened-only",
        data: b"\x05\x08MEGA",
        want_name: b"MEGA",
        want_complete: false,
        want_cut: false,
        why: "a Shortened Local Name is kept, and must NOT be reported as complete — a miss \
               against it is unproven, not final",
    },
    BtNameCase {
        what: "shortened-then-complete",
        data: b"\x05\x08MEGA\x09\x09MEGABOOM",
        want_name: b"MEGABOOM",
        want_complete: true,
        want_cut: false,
        why: "the complete name must override a shortened one heard earlier in the SAME payload",
    },
    BtNameCase {
        what: "empty-complete-does-not-erase-shortened",
        data: b"\x05\x08MEGA\x01\x09",
        want_name: b"MEGA",
        want_complete: false,
        want_cut: false,
        why: "a zero-length Complete Local Name carries no name and must not erase what was heard",
    },
    BtNameCase {
        what: "no-name-at-all",
        data: b"\x02\x01\x06\x03\x03\xfe\x61",
        want_name: b"",
        want_complete: false,
        want_cut: false,
        why: "a payload with no name structure must decode to NO name — not to a stray byte",
    },
    BtNameCase {
        what: "length-runs-past-end",
        // Declares 9 but only 4 name bytes are present: a malformed tail, and the walk must stop
        // rather than read past the payload.
        data: b"\x09\x09MEGA",
        want_name: b"",
        want_complete: false,
        want_cut: false,
        why: "an over-long Length is a malformed tail; the walk must stop, not read past the data",
    },
    BtNameCase {
        what: "over-long-name-is-cut",
        data: LONG_NAME_AD,
        want_name: b"ABCDEFGHIJKLMNOPQRSTUVWX",
        want_complete: true,
        want_cut: true,
        why: "a name past the 24-byte cap must be marked cut, so a name MISMATCH can be read as \
               possibly false rather than as fact",
    },
];

/// Run one fixture leg. Returns `true` when the decode matched the expectation exactly.
pub fn bt_name_case_passes(c: &BtNameCase) -> bool {
    let got = bt_decode_local_name(c.data);
    got.as_bytes() == c.want_name && got.ncomplete == c.want_complete && got.ncut == c.want_cut
}

// ===========================================================================================
// BT-L3 — THE ADDRESS MATCH. Same file, same reasons: pure byte handling, no I/O, shared with
// the fixture and with `tools/btname_harness.rs` rather than transcribed into either.
// ===========================================================================================

/// `Peer_Address_Type` values that may go into `HCI_LE_Create_Connection`. 0x02/0x03 are the
/// resolved-identity forms and are refused earlier, by the caller.
pub const BT_ADDR_TYPE_PUBLIC: u8 = 0x00;
pub const BT_ADDR_TYPE_RANDOM: u8 = 0x01;

/// **THE BYTE ORDER, AND THE ONLY PLACE IT IS WRITTEN DOWN.**
///
/// A BD_ADDR travels LITTLE-ENDIAN on the wire: the `LE Advertising Report`'s six `Address`
/// octets arrive LSB first, and `bt_le_drain` stores them exactly as they arrived
/// (`addr.copy_from_slice(&pkt[6..12])` — no reversal). The human notation is the reverse, so
/// every witness renders `addr[5]` first. A constant written in the HUMAN order and compared
/// against a stored address therefore NEVER MATCHES, and nothing in a capture would say why:
/// the target would simply never be selected, on every boot, silently.
///
/// So the constant is written in WIRE order — and `bt_addr_render_msb` below plus the
/// `byte-order-relationship` fixture leg pin the relationship between it and the printed form,
/// so that a future reader cannot get it wrong and a future editor cannot get it wrong quietly.
///
/// `88:c6:26:cc:2d:3c` is Peter's Ultimate Ears MEGABOOM: the address his own host stack reports
/// for the paired device, and byte for byte the address boot AR decoded off the air.
pub const BT_L3_PEER_ADDR_BYTES: [u8; 6] = [0x3C, 0x2D, 0xCC, 0x26, 0xC6, 0x88];

/// The SAME address in the human, MSB-first notation, as ASCII — the exact text a witness line
/// prints. The fixture asserts `bt_addr_render_msb(BT_L3_PEER_ADDR_BYTES) == this`, which is what
/// makes the wire order above checkable rather than merely asserted in a comment.
pub const BT_L3_PEER_ADDR_TEXT: &[u8; 17] = b"88:c6:26:cc:2d:3c";

/// Render a stored (wire-order) BD_ADDR into the human MSB-first `aa:bb:cc:dd:ee:ff` ASCII form.
/// Lowercase hex, to match every existing `bt-l2`/`bt-l3` witness line.
pub fn bt_addr_render_msb(addr: &[u8; 6]) -> [u8; 17] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [b':'; 17];
    for k in 0..6 {
        let b = addr[5 - k];
        out[k * 3] = HEX[(b >> 4) as usize];
        out[k * 3 + 1] = HEX[(b & 0x0F) as usize];
    }
    out
}

/// Byte-for-byte address equality. Trivial on purpose: the value of putting it here is that the
/// fixture, the harness and the radio path all call the SAME comparison, so a leg that passes is
/// a statement about the selection the radio will actually make.
pub fn bt_addr_eq(a: &[u8; 6], b: &[u8; 6]) -> bool {
    a == b
}

/// The full address rule: the six bytes AND the address type.
///
/// The type is part of the identity, not decoration. A public and a random address are drawn from
/// different spaces and a random one can collide with a public one numerically; connecting to a
/// random-address device because its bytes happen to equal our public target would be exactly the
/// "reached into the room" failure the whole filter exists to prevent. The host reports Peter's
/// speaker as `(public)`, so `BT_ADDR_TYPE_PUBLIC` is what the caller asks for.
pub fn bt_addr_matches(addr: &[u8; 6], atype: u8, want: &[u8; 6], want_type: u8) -> bool {
    bt_addr_eq(addr, want) && atype == want_type
}

/// One address fixture leg: a candidate as the drain would have stored it, the target, and the
/// verdict the rule MUST reach.
pub struct BtAddrCase {
    pub what: &'static str,
    /// The candidate address in WIRE order — i.e. exactly as `bt_le_drain` stores it.
    pub addr: [u8; 6],
    pub atype: u8,
    pub want: [u8; 6],
    pub want_type: u8,
    pub expect: bool,
    /// Why this leg exists — printed on FAILURE so a red fixture explains itself in the capture.
    pub why: &'static str,
}

pub const BT_ADDR_CASES: &[BtAddrCase] = &[
    BtAddrCase {
        what: "megaboom-exact",
        addr: BT_L3_PEER_ADDR_BYTES,
        atype: BT_ADDR_TYPE_PUBLIC,
        want: BT_L3_PEER_ADDR_BYTES,
        want_type: BT_ADDR_TYPE_PUBLIC,
        expect: true,
        why: "the target address, public, must match itself — if this leg is red the filter \
               cannot select anything and a no-match proves nothing about the room",
    },
    BtAddrCase {
        what: "wrong-last-byte",
        // Wire order, so index 0 is the byte printed LAST: 88:c6:26:cc:2d:3d.
        addr: [0x3D, 0x2D, 0xCC, 0x26, 0xC6, 0x88],
        atype: BT_ADDR_TYPE_PUBLIC,
        want: BT_L3_PEER_ADDR_BYTES,
        want_type: BT_ADDR_TYPE_PUBLIC,
        expect: false,
        why: "one byte off is a DIFFERENT device; an address filter that admits a neighbour is \
               not an address filter",
    },
    BtAddrCase {
        what: "byte-order-reversed-must-not-match",
        // The target written in the HUMAN order by mistake — the exact error this file exists to
        // make impossible. It must NOT match a correctly stored wire-order address.
        addr: [0x88, 0xC6, 0x26, 0xCC, 0x2D, 0x3C],
        atype: BT_ADDR_TYPE_PUBLIC,
        want: BT_L3_PEER_ADDR_BYTES,
        want_type: BT_ADDR_TYPE_PUBLIC,
        expect: false,
        why: "if this leg PASSES as a match the constant was written MSB-first and the filter is \
               silently dead on every boot",
    },
    BtAddrCase {
        what: "right-bytes-wrong-address-type",
        addr: BT_L3_PEER_ADDR_BYTES,
        atype: BT_ADDR_TYPE_RANDOM,
        want: BT_L3_PEER_ADDR_BYTES,
        want_type: BT_ADDR_TYPE_PUBLIC,
        expect: false,
        why: "a random address may collide numerically with a public one; the type is part of \
               the identity and a create posted with the wrong one names a different device",
    },
    BtAddrCase {
        what: "all-zero-candidate",
        addr: [0; 6],
        atype: BT_ADDR_TYPE_PUBLIC,
        want: BT_L3_PEER_ADDR_BYTES,
        want_type: BT_ADDR_TYPE_PUBLIC,
        expect: false,
        why: "a default-initialised BtDev must not match the target — an empty table entry is \
               not a sighting",
    },
];

/// Run one address fixture leg. Returns `true` when the rule reached the expected verdict.
pub fn bt_addr_case_passes(c: &BtAddrCase) -> bool {
    bt_addr_matches(&c.addr, c.atype, &c.want, c.want_type) == c.expect
}

/// THE BYTE-ORDER RELATIONSHIP, as a checkable proposition: the constant, rendered the way every
/// witness line renders a stored address, is the address Peter's host reports.
///
/// This is the leg that fails if `BT_L3_PEER_ADDR_BYTES` is ever rewritten MSB-first. It is
/// deliberately separate from the match legs: those would still all pass with a consistently
/// reversed constant (they compare it against itself) — only this one compares it against the
/// OUTSIDE WORLD.
pub fn bt_addr_order_holds() -> bool {
    &bt_addr_render_msb(&BT_L3_PEER_ADDR_BYTES) == BT_L3_PEER_ADDR_TEXT
}
