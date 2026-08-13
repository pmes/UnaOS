// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BANDY-1 (ROADMAP §3b arc 1): the on-UnaOS SMessage bus — the v1 CODEC.
//
// "Port the bus, not the binary convention." This module is the WIRE layer of the syscall-backed
// bus transport (SYS_MSEND/SYS_MRECV live in syscall.rs): ONE coherent UnaOS-NATIVE format,
// end-to-end — a fixed 52-byte header + typed request payloads for the first three verbs
// (ls / cat / cp) + typed replies.
//
// FORMAT AUTHORITY (Peter's ruling, R17 2026-07-16, superseding the earlier host-compat
// condition): "we're writing this OS and filesystem so we can make whatever change we need to do
// it right." Host byte-compat is NOT a requirement; the format below is chosen on merit and is
// the SPEC OF RECORD from this commit — the KATs in `bus_codec_selftest` freeze it (self-authored
// goldens, the compat anchor going forward). The host bandy bus (an in-process broadcast of
// `SMessage` values — no header, no framing, no principal) migrates to THIS format in a later
// HOST arc, out of this lane. The host `TerminalOutput`/`TerminalError` reply types survive as
// CONCEPTS: a success reply's body is output text to present; an error reply is its errno.
//
// THE MERIT CASE for the reply shape: the header already carries a machine-readable `status`
// (the errno the equivalence witness compares byte-for-byte against the direct syscalls) and
// echoes `verb` + `corr`, so a reply needs NO self-describing envelope — a success body is the
// verb-typed payload bytes (ls: listing text; cat: file content; cp: empty), and an ERROR reply
// carries NO body at all (status is the whole truth; a client renders its own message). One
// binary layout both directions, no string-escaping layer, no second parser — the smallest
// honest surface for a kernel-side codec that must fail closed.
//
// DECODE CEILINGS (verdict A, the BEFS-HARDEN lesson verbatim): a decode budget is the max FORCED
// allocation. BUS_BODY_MAX = 4 KiB, so the largest allocation any hostile frame can force is
// BUS_FRAME_MAX = 4148 bytes — three orders of magnitude under the 48 MiB kernel heap, and the
// per-ASID mailboxes (depth 16, syscall.rs) bound the aggregate. A frame exceeding any ceiling is
// rejected fail-closed, never partially decoded.
//
// PRINCIPAL FIELD (verdict C): bytes 16..48 carry the 32-byte PrincipalRecord wire image
// (kind, len, value[30]). An EL0 REQUEST must present it ALL-ZERO — the kernel stamps the
// sender's record INSIDE the kernel after validation; a caller-supplied (nonzero) principal is
// rejected `BadPrincipal` (-EINVAL at the syscall), never overwritten. REPLIES carry the RESERVED
// KERNEL principal kind (PRIN_KERNEL_REPLY, syscall.rs), fail-closed everywhere a grantee can
// appear. This module treats the field as opaque bytes; policy lives at the transport.
//
// No heap use anywhere in this module beyond caller-provided buffers (one heap staging in the
// selftest's at-ceiling KAT); no_std; aarch64-only (declared under arch/aarch64; zero x86 surface).

/// Frame magic: b"UBS1" — UnaOS Bus, wire v1.
pub use una_abi::BUS_MAGIC;
/// Wire version. A bump is a protocol break (rule on it; never silent).
pub use una_abi::BUS_VERSION;

/// Header length. Layout (little-endian):
///   0..4  magic  4..5 version  5..6 kind  6..7 verb  7..8 reserved(=0)
///   8..12 corr(u32)  12..16 status(i32)  16..48 principal(32B)  48..52 body_len(u32)
/// REQUEST: status MUST be 0, principal MUST be all-zero (the kernel stamps it), verb ∈ {LS,CAT,CP},
/// body = the verb's typed payload. REPLY: verb ECHOES the request verb, corr echoes, status = 0
/// (body = the verb's typed output) or a negative errno (body MUST be empty).
pub use una_abi::BUS_HDR_LEN;
/// HARD DECODE CEILING for the body — the max forced allocation per frame (see module doc).
pub use una_abi::BUS_BODY_MAX;
/// The whole-frame ceiling: header + max body.
pub use una_abi::BUS_FRAME_MAX;

/// Frame kinds.
pub use una_abi::BUS_KIND_REQUEST;
pub use una_abi::BUS_KIND_REPLY;

/// Verbs. A reply echoes its request's verb.
/// BANDY-1 read-side subset (ROADMAP §3b's first three):
pub use una_abi::BUS_VERB_LS;
pub use una_abi::BUS_VERB_CAT;
pub use una_abi::BUS_VERB_CP;
/// BANDY-2 write-side subset (ROADMAP §3b arc 2) — the DESTRUCTIVE verbs, fulfilled through the
/// EXISTING FAT + ACL machinery under the invoker's stamped principal (syscall.rs), errno-mirrored:
///   * WRITE — create-or-truncate a root file with a typed content payload (sys_open(O_CREAT)+write twin);
///   * RM    — unlink a root file by name (sys_unlink twin — owner-only delete, durable ACL clear first);
///   * MV     — rename a root file in place (the fat.rs rename_entry twin — owner-only, ACL name re-bind).
/// ADDITIVE to the frozen v1 layout: new verb tags + typed payloads, no header/ceiling change.
pub use una_abi::BUS_VERB_WRITE;
pub use una_abi::BUS_VERB_RM;
pub use una_abi::BUS_VERB_MV;

/// The full set of valid verbs (both request and reply kinds). Kept as one predicate so the
/// frozen `frame_parse` verb gate and the typed-body dispatch stay in lockstep.
#[inline]
pub fn verb_valid(verb: u8) -> bool {
    matches!(
        verb,
        BUS_VERB_LS | BUS_VERB_CAT | BUS_VERB_CP | BUS_VERB_WRITE | BUS_VERB_RM | BUS_VERB_MV
    )
}

/// 8.3 name bound — mirrors syscall.rs MAX_NAME (the sys_open bound the equivalence witness
/// holds cat/cp to).
pub use una_abi::BUS_NAME_MAX;

/// A decoded frame header. `principal` is the raw 32-byte PrincipalRecord wire image, opaque here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BusHdr {
    pub kind: u8,
    pub verb: u8,
    pub corr: u32,
    pub status: i32,
    pub principal: [u8; 32],
    pub body_len: u32,
}

/// Why a frame was refused. Every arm is FAIL-CLOSED: nothing about a refused frame is acted on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BusDecodeErr {
    /// Shorter than one header, or the declared body does not exactly fit the presented bytes.
    Truncated,
    /// Bad magic / version / reserved byte / kind / verb / reply-shape violation.
    Malformed,
    /// body_len exceeds BUS_BODY_MAX (the hard ceiling).
    TooBig,
    /// A REQUEST carried a nonzero status or a nonzero principal field (verdict C: reject,
    /// don't overwrite).
    BadPrincipal,
    /// The verb payload failed its typed validation (name bounds, cp length arithmetic).
    BadBody,
}

#[inline]
fn rd_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Serialize a header into `out[..BUS_HDR_LEN]`. Kernel-side use (reply building) and EL0
/// clients hand-build the same layout (midden's request builder mirrors this byte-for-byte).
pub fn hdr_write(h: &BusHdr, out: &mut [u8]) {
    debug_assert!(out.len() >= BUS_HDR_LEN);
    out[0..4].copy_from_slice(&BUS_MAGIC);
    out[4] = BUS_VERSION;
    out[5] = h.kind;
    out[6] = h.verb;
    out[7] = 0;
    out[8..12].copy_from_slice(&h.corr.to_le_bytes());
    out[12..16].copy_from_slice(&h.status.to_le_bytes());
    out[16..48].copy_from_slice(&h.principal);
    out[48..52].copy_from_slice(&h.body_len.to_le_bytes());
}

/// Parse + validate a frame's header against the whole presented byte range. Enforces, in order:
/// length ≥ header; magic/version/reserved; kind ∈ {REQUEST, REPLY}; verb ∈ {LS, CAT, CP, WRITE,
/// RM, MV} (both kinds — a reply echoes its request's verb); body_len ≤ BUS_BODY_MAX (the ceiling — checked
/// BEFORE the fit check so an absurd length is TooBig, not Truncated); the declared body exactly
/// fits the presented bytes (no trailing slack — a frame is exact, not a prefix); a REPLY with a
/// nonzero status carries NO body (an error is its errno, never errno-plus-bytes). Returns the
/// header; the body is `frame[BUS_HDR_LEN..]`.
pub fn frame_parse(frame: &[u8]) -> Result<BusHdr, BusDecodeErr> {
    if frame.len() < BUS_HDR_LEN {
        return Err(BusDecodeErr::Truncated);
    }
    if frame[0..4] != BUS_MAGIC || frame[4] != BUS_VERSION || frame[7] != 0 {
        return Err(BusDecodeErr::Malformed);
    }
    let kind = frame[5];
    let verb = frame[6];
    if !matches!(kind, BUS_KIND_REQUEST | BUS_KIND_REPLY) || !verb_valid(verb) {
        return Err(BusDecodeErr::Malformed);
    }
    let body_len = rd_u32(&frame[48..52]);
    if body_len as usize > BUS_BODY_MAX {
        return Err(BusDecodeErr::TooBig); // the hard ceiling — fail closed, nothing decoded
    }
    if frame.len() != BUS_HDR_LEN + body_len as usize {
        return Err(BusDecodeErr::Truncated);
    }
    let status = i32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]);
    if kind == BUS_KIND_REPLY && status != 0 && body_len != 0 {
        return Err(BusDecodeErr::Malformed); // an error reply is its errno — never errno + bytes
    }
    let mut principal = [0u8; 32];
    principal.copy_from_slice(&frame[16..48]);
    Ok(BusHdr { kind, verb, corr: rd_u32(&frame[8..12]), status, principal, body_len })
}

/// Request-side validation (verdict C): an EL0 REQUEST must carry status == 0 and an ALL-ZERO
/// principal field — the kernel is the only stamper. Reject, don't overwrite.
pub fn request_validate(h: &BusHdr) -> Result<(), BusDecodeErr> {
    if h.kind != BUS_KIND_REQUEST {
        return Err(BusDecodeErr::Malformed);
    }
    if h.status != 0 || h.principal != [0u8; 32] {
        return Err(BusDecodeErr::BadPrincipal);
    }
    Ok(())
}

/// CAT request body: the bare 8.3 name bytes. 1..=BUS_NAME_MAX, printable ASCII with no space
/// (the sys_open twin turns non-UTF-8 into -ENOENT; here a byte that can never appear in a FAT
/// 8.3 name is BadBody — the transport refuses what the namespace could never match).
pub fn cat_body_parse(body: &[u8]) -> Result<&[u8], BusDecodeErr> {
    if body.is_empty() || body.len() > BUS_NAME_MAX {
        return Err(BusDecodeErr::BadBody);
    }
    if !body.iter().all(|&b| (0x21..0x7f).contains(&b)) {
        return Err(BusDecodeErr::BadBody);
    }
    Ok(body)
}

/// CP request body: `[src_len u8][src bytes][dst bytes]` — both names under the CAT rules; the
/// length arithmetic must consume the body EXACTLY (no slack, no overlap).
pub fn cp_body_parse(body: &[u8]) -> Result<(&[u8], &[u8]), BusDecodeErr> {
    if body.len() < 2 {
        return Err(BusDecodeErr::BadBody);
    }
    let src_len = body[0] as usize;
    if src_len == 0 || src_len > BUS_NAME_MAX || 1 + src_len >= body.len() {
        return Err(BusDecodeErr::BadBody);
    }
    let src = cat_body_parse(&body[1..1 + src_len])?;
    let dst = cat_body_parse(&body[1 + src_len..])?;
    Ok((src, dst))
}

/// BANDY-2 WRITE request body: `[name_len u8][name bytes][content bytes]`. The name is under the
/// CAT rules (1..=BUS_NAME_MAX printable-ASCII 8.3 bytes); the content is the remainder — ANY bytes
/// (a file is arbitrary content), possibly EMPTY (a 0-byte create). The frame ceiling
/// (BUS_BODY_MAX) already bounds the whole body, so the content is bounded by construction
/// (≤ BUS_BODY_MAX − 1 − name_len); the transport applies the semantic BUS_CP_MAX content ceiling.
pub fn write_body_parse(body: &[u8]) -> Result<(&[u8], &[u8]), BusDecodeErr> {
    if body.is_empty() {
        return Err(BusDecodeErr::BadBody);
    }
    let name_len = body[0] as usize;
    // The name must fit AFTER the length byte; content is whatever follows (may be empty).
    if name_len == 0 || name_len > BUS_NAME_MAX || 1 + name_len > body.len() {
        return Err(BusDecodeErr::BadBody);
    }
    let name = cat_body_parse(&body[1..1 + name_len])?;
    let content = &body[1 + name_len..];
    Ok((name, content))
}

/// BANDY-2 RM request body: the bare 8.3 name (identical shape to CAT — reuses `cat_body_parse`).
/// BANDY-2 MV request body: `[src_len u8][src][dst]` (identical shape to CP — reuses `cp_body_parse`).
///
/// Build a request frame into `buf`; returns the frame length. The EL0 client-side layout in
/// kernel-testable form (midden mirrors this); also the M1/M5 fixtures' request builder.
pub fn build_request(verb: u8, corr: u32, body: &[u8], buf: &mut [u8]) -> usize {
    let h = BusHdr {
        kind: BUS_KIND_REQUEST,
        verb,
        corr,
        status: 0,
        principal: [0u8; 32],
        body_len: body.len() as u32,
    };
    hdr_write(&h, buf);
    buf[BUS_HDR_LEN..BUS_HDR_LEN + body.len()].copy_from_slice(body);
    BUS_HDR_LEN + body.len()
}

/// Build a reply frame into `buf`: verb + corr echoed from the request, `status` = 0 (body =
/// the verb-typed output bytes) or a negative errno (body forced EMPTY — the error-reply shape
/// is structural here, not caller discipline). `principal` is the kernel-reply record's wire
/// image (the transport supplies it — this module stays policy-free). Returns the frame length.
pub fn build_reply(verb: u8, corr: u32, status: i32, principal: [u8; 32], body: &[u8], buf: &mut [u8]) -> usize {
    let body = if status == 0 { body } else { &[] };
    let h = BusHdr { kind: BUS_KIND_REPLY, verb, corr, status, principal, body_len: body.len() as u32 };
    hdr_write(&h, buf);
    buf[BUS_HDR_LEN..BUS_HDR_LEN + body.len()].copy_from_slice(body);
    BUS_HDR_LEN + body.len()
}

// ---------------------------------------------------------------------------------------------
// BANDY-CODEC selftest — the M1 KATs, run every boot (the K4-ready idiom: read-only, in-RAM,
// no disk, no card; an uncounted `:: BANDY-CODEC: … ::` witness line).
// ---------------------------------------------------------------------------------------------

/// FROZEN NATIVE GOLDENS (self-authored at the format's first commit — the compat anchor; a
/// mismatch = the wire moved = a protocol break to rule on, never a test to "fix up").
/// A `cat GROW.BIN` request, corr = 7: header + 8-byte name body.
const GOLDEN_REQ_CAT: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x01, // kind REQUEST
    0x02, // verb CAT
    0x00, // reserved
    0x07, 0x00, 0x00, 0x00, // corr = 7
    0x00, 0x00, 0x00, 0x00, // status = 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal[0..16] = zero (kernel stamps)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal[16..32] = zero
    0x08, 0x00, 0x00, 0x00, // body_len = 8
    b'G', b'R', b'O', b'W', b'.', b'B', b'I', b'N', // body = "GROW.BIN"
];

/// A successful `ls` REPLY, corr = 7, stamped with the reserved kernel-reply principal
/// (kind 4, len 0 — syscall.rs PRIN_KERNEL_REPLY): header + 9-byte listing body.
const GOLDEN_REP_LS: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x02, // kind REPLY
    0x01, // verb LS (echoed)
    0x00, // reserved
    0x07, 0x00, 0x00, 0x00, // corr = 7 (echoed)
    0x00, 0x00, 0x00, 0x00, // status = 0
    0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal: kind=4 (KERNEL_REPLY), len=0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //            value tail = zero
    0x09, 0x00, 0x00, 0x00, // body_len = 9
    b'A', b'.', b'T', b'X', b'T', b' ', b'4', b'2', b'\n', // body = "A.TXT 42\n"
];

/// An error `cat` REPLY, corr = 9, status = -13 (EACCES): errno in the header, NO body.
const GOLDEN_REP_ERR: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x02, // kind REPLY
    0x02, // verb CAT (echoed)
    0x00, // reserved
    0x09, 0x00, 0x00, 0x00, // corr = 9 (echoed)
    0xf3, 0xff, 0xff, 0xff, // status = -13 (EACCES) little-endian
    0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal: kind=4 (KERNEL_REPLY), len=0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x00, 0x00, 0x00, 0x00, // body_len = 0 — an error reply is its errno, never errno + bytes
];

/// The kernel-reply principal wire image the goldens pin (kind 4, len 0, zero value).
fn reply_principal_bytes() -> [u8; 32] {
    let mut p = [0u8; 32];
    p[0] = 4;
    p
}

/// The M1 witness: frozen native goldens (request + both reply shapes) + round-trips +
/// fail-closed decoding + the ceilings. Emits ONE uncounted `:: BANDY-CODEC: … ::` line.
pub fn bus_codec_selftest() {
    let mut w = 0u32;

    // bit0: the frozen request golden — build_request reproduces it byte-for-byte and it
    // round-trips field-exact through frame_parse + request_validate + the typed body parser.
    let mut req = [0u8; 128];
    let n = build_request(BUS_VERB_CAT, 7, b"GROW.BIN", &mut req);
    if n == GOLDEN_REQ_CAT.len() && &req[..n] == GOLDEN_REQ_CAT {
        if let Ok(h) = frame_parse(&req[..n]) {
            if h.kind == BUS_KIND_REQUEST
                && h.verb == BUS_VERB_CAT
                && h.corr == 7
                && h.status == 0
                && h.body_len == 8
                && request_validate(&h).is_ok()
                && cat_body_parse(&req[BUS_HDR_LEN..n]) == Ok(b"GROW.BIN".as_slice())
            {
                w |= 1 << 0;
            }
        }
    }

    // bit1: the frozen SUCCESS-reply golden — build_reply reproduces it and it round-trips.
    let mut rep = [0u8; 128];
    let rn = build_reply(BUS_VERB_LS, 7, 0, reply_principal_bytes(), b"A.TXT 42\n", &mut rep);
    if rn == GOLDEN_REP_LS.len() && &rep[..rn] == GOLDEN_REP_LS {
        if let Ok(h) = frame_parse(&rep[..rn]) {
            if h.kind == BUS_KIND_REPLY
                && h.verb == BUS_VERB_LS
                && h.corr == 7
                && h.status == 0
                && &rep[BUS_HDR_LEN..rn] == b"A.TXT 42\n"
            {
                w |= 1 << 1;
            }
        }
    }

    // bit2: the frozen ERROR-reply golden — errno in the header, body forced empty (build_reply
    // drops a body handed alongside a nonzero status — structural), and the parser refuses a
    // hand-forged errno+body frame.
    let mut erep = [0u8; 128];
    let en = build_reply(BUS_VERB_CAT, 9, -13, reply_principal_bytes(), b"MUST-DROP", &mut erep);
    let golden_ok = en == GOLDEN_REP_ERR.len() && &erep[..en] == GOLDEN_REP_ERR;
    let mut forged = [0u8; 128];
    let fl = build_reply(BUS_VERB_CAT, 9, 0, reply_principal_bytes(), b"X", &mut forged);
    forged[12..16].copy_from_slice(&(-13i32).to_le_bytes()); // errno + body, hand-forged
    if golden_ok && frame_parse(&forged[..fl]) == Err(BusDecodeErr::Malformed) {
        w |= 1 << 2;
    }

    // bit3: ls + cp request round-trips (typed payloads)
    {
        let mut f = [0u8; 128];
        let n_ls = build_request(BUS_VERB_LS, 1, b"", &mut f);
        let ls_ok = matches!(frame_parse(&f[..n_ls]), Ok(h) if h.verb == BUS_VERB_LS && h.body_len == 0);
        let mut body = [0u8; 32];
        body[0] = 8;
        body[1..9].copy_from_slice(b"GROW.BIN");
        body[9..17].copy_from_slice(b"COPY.BIN");
        let n_cp = build_request(BUS_VERB_CP, 2, &body[..17], &mut f);
        let cp_ok = match frame_parse(&f[..n_cp]) {
            Ok(h) if h.verb == BUS_VERB_CP => {
                cp_body_parse(&f[BUS_HDR_LEN..n_cp])
                    == Ok((b"GROW.BIN".as_slice(), b"COPY.BIN".as_slice()))
            }
            _ => false,
        };
        if ls_ok && cp_ok {
            w |= 1 << 3;
        }
    }

    // bit4: fail-closed decoding — every malformed class refused with the RIGHT refusal
    {
        let mut f = [0u8; 128];
        let n = build_request(BUS_VERB_CAT, 7, b"GROW.BIN", &mut f);
        let mut m = f;
        m[0] = b'X'; // magic
        let bad_magic = frame_parse(&m[..n]) == Err(BusDecodeErr::Malformed);
        let mut m = f;
        m[4] = 2; // version
        let bad_ver = frame_parse(&m[..n]) == Err(BusDecodeErr::Malformed);
        let mut m = f;
        m[7] = 1; // reserved
        let bad_rsvd = frame_parse(&m[..n]) == Err(BusDecodeErr::Malformed);
        let mut m = f;
        m[5] = 9; // kind
        let bad_kind = frame_parse(&m[..n]) == Err(BusDecodeErr::Malformed);
        let mut m = f;
        m[6] = 9; // verb
        let bad_verb = frame_parse(&m[..n]) == Err(BusDecodeErr::Malformed);
        let mut m = f;
        m[12] = 1; // nonzero status in a request
        let bad_status = matches!(
            frame_parse(&m[..n]).map(|h| request_validate(&h)),
            Ok(Err(BusDecodeErr::BadPrincipal))
        );
        let mut m = f;
        m[16] = 2; // caller-supplied principal kind byte — MUST be refused, not overwritten
        let bad_prin = matches!(
            frame_parse(&m[..n]).map(|h| request_validate(&h)),
            Ok(Err(BusDecodeErr::BadPrincipal))
        );
        let ok = bad_magic
            && bad_ver
            && bad_rsvd
            && bad_kind
            && bad_verb
            && bad_status
            && bad_prin
            && frame_parse(&f[..BUS_HDR_LEN - 1]) == Err(BusDecodeErr::Truncated)
            && frame_parse(&f[..n - 1]) == Err(BusDecodeErr::Truncated) // body under-fits
            && cat_body_parse(b"") == Err(BusDecodeErr::BadBody)
            && cat_body_parse(b"WAYTOOLONGNAME") == Err(BusDecodeErr::BadBody)
            && cat_body_parse(b"BAD NAME") == Err(BusDecodeErr::BadBody) // 0x20 refused
            && cp_body_parse(&[0x08]) == Err(BusDecodeErr::BadBody)
            && cp_body_parse(&[0x00, b'A', b'B']) == Err(BusDecodeErr::BadBody)
            && cp_body_parse(&[0x02, b'A', b'B']) == Err(BusDecodeErr::BadBody); // no dst bytes
        if ok {
            w |= 1 << 4;
        }
    }

    // bit5: the HARD CEILING — body_len over BUS_BODY_MAX is TooBig from the HEADER ALONE (the
    // transport never stages an over-ceiling body to find out); a frame AT the ceiling parses.
    {
        let mut hdr = [0u8; BUS_HDR_LEN];
        let h = BusHdr {
            kind: BUS_KIND_REQUEST,
            verb: BUS_VERB_CAT,
            corr: 0,
            status: 0,
            principal: [0u8; 32],
            body_len: (BUS_BODY_MAX + 1) as u32,
        };
        hdr_write(&h, &mut hdr);
        let too_big = frame_parse(&hdr) == Err(BusDecodeErr::TooBig);
        // At-ceiling frame: heap-stage it (a 4 KiB+ array has no business on a kernel stack).
        let mut max_frame = alloc::vec![0u8; BUS_FRAME_MAX];
        let h2 = BusHdr { body_len: BUS_BODY_MAX as u32, verb: BUS_VERB_LS, ..h };
        hdr_write(&h2, &mut max_frame);
        let at_ceiling = frame_parse(&max_frame).is_ok();
        if too_big && at_ceiling {
            w |= 1 << 5;
        }
    }

    const ALL: u32 = (1 << 6) - 1;
    if w == ALL {
        serial_println!(
            ":: BANDY-CODEC: native v1 wire frozen (request+reply goldens, typed ls/cat/cp payloads, error-reply = errno-no-body), decode fail-closed (body ceiling {} B) PASS [w={:#04x}] ::",
            BUS_BODY_MAX,
            w
        );
    } else {
        serial_println!(":: BANDY-CODEC: FAIL — w={:#x}/{:#x} ::", w, ALL);
    }
}

// ---------------------------------------------------------------------------------------------
// BANDY-CODEC2 selftest — the M1 write-side KATs (BANDY-2). A SIBLING of BANDY-CODEC (the
// BANDY-1 goldens/witness are left byte-identical): frozen native goldens for the three new
// request verbs (write/rm/mv) + round-trips + fail-closed decoding of the new typed bodies.
// Same idiom: read-only, in-RAM, no disk; one uncounted `:: BANDY-CODEC2: … ::` line.
// ---------------------------------------------------------------------------------------------

/// FROZEN NATIVE GOLDEN — a `write NOTE.TXT "hi\n"` REQUEST, corr = 11: header + body
/// `[name_len=8]["NOTE.TXT"]["hi\n"]` (12 body bytes). Self-authored at BANDY-2's first commit.
const GOLDEN_REQ_WRITE: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x01, // kind REQUEST
    0x04, // verb WRITE
    0x00, // reserved
    0x0b, 0x00, 0x00, 0x00, // corr = 11
    0x00, 0x00, 0x00, 0x00, // status = 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal zero (kernel stamps)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x0c, 0x00, 0x00, 0x00, // body_len = 12
    0x08, // name_len = 8
    b'N', b'O', b'T', b'E', b'.', b'T', b'X', b'T', // name
    b'h', b'i', b'\n', // content
];

/// FROZEN NATIVE GOLDEN — an `rm NOTE.TXT` REQUEST, corr = 12: header + bare 8.3 name (8 bytes).
const GOLDEN_REQ_RM: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x01, // kind REQUEST
    0x05, // verb RM
    0x00, // reserved
    0x0c, 0x00, 0x00, 0x00, // corr = 12
    0x00, 0x00, 0x00, 0x00, // status = 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x08, 0x00, 0x00, 0x00, // body_len = 8
    b'N', b'O', b'T', b'E', b'.', b'T', b'X', b'T', // name
];

/// FROZEN NATIVE GOLDEN — a `mv NOTE.TXT DONE.TXT` REQUEST, corr = 13: header + body
/// `[src_len=8]["NOTE.TXT"]["DONE.TXT"]` (17 body bytes).
const GOLDEN_REQ_MV: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x01, // kind REQUEST
    0x06, // verb MV
    0x00, // reserved
    0x0d, 0x00, 0x00, 0x00, // corr = 13
    0x00, 0x00, 0x00, 0x00, // status = 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0x11, 0x00, 0x00, 0x00, // body_len = 17
    0x08, // src_len = 8
    b'N', b'O', b'T', b'E', b'.', b'T', b'X', b'T', // src
    b'D', b'O', b'N', b'E', b'.', b'T', b'X', b'T', // dst
];

/// The M1 write-side witness. Emits ONE uncounted `:: BANDY-CODEC2: … ::` line.
pub fn bus_codec2_selftest() {
    let mut w = 0u32;

    // bit0: the frozen WRITE golden — build_request reproduces it byte-for-byte and it round-trips
    // field-exact through frame_parse + the typed write body parser (name + content split).
    {
        let mut req = [0u8; 128];
        let mut body = [0u8; 32];
        body[0] = 8;
        body[1..9].copy_from_slice(b"NOTE.TXT");
        body[9..12].copy_from_slice(b"hi\n");
        let n = build_request(BUS_VERB_WRITE, 11, &body[..12], &mut req);
        if n == GOLDEN_REQ_WRITE.len() && &req[..n] == GOLDEN_REQ_WRITE {
            if let Ok(h) = frame_parse(&req[..n]) {
                if h.kind == BUS_KIND_REQUEST
                    && h.verb == BUS_VERB_WRITE
                    && h.corr == 11
                    && h.body_len == 12
                    && request_validate(&h).is_ok()
                    && write_body_parse(&req[BUS_HDR_LEN..n])
                        == Ok((b"NOTE.TXT".as_slice(), b"hi\n".as_slice()))
                {
                    w |= 1 << 0;
                }
            }
        }
    }

    // bit1: the frozen RM golden — reproduces byte-for-byte; body reuses the CAT name parser.
    {
        let mut req = [0u8; 128];
        let n = build_request(BUS_VERB_RM, 12, b"NOTE.TXT", &mut req);
        if n == GOLDEN_REQ_RM.len() && &req[..n] == GOLDEN_REQ_RM {
            if let Ok(h) = frame_parse(&req[..n]) {
                if h.verb == BUS_VERB_RM
                    && h.corr == 12
                    && cat_body_parse(&req[BUS_HDR_LEN..n]) == Ok(b"NOTE.TXT".as_slice())
                {
                    w |= 1 << 1;
                }
            }
        }
    }

    // bit2: the frozen MV golden — reproduces byte-for-byte; body reuses the CP (src,dst) parser.
    {
        let mut req = [0u8; 128];
        let mut body = [0u8; 32];
        body[0] = 8;
        body[1..9].copy_from_slice(b"NOTE.TXT");
        body[9..17].copy_from_slice(b"DONE.TXT");
        let n = build_request(BUS_VERB_MV, 13, &body[..17], &mut req);
        if n == GOLDEN_REQ_MV.len() && &req[..n] == GOLDEN_REQ_MV {
            if let Ok(h) = frame_parse(&req[..n]) {
                if h.verb == BUS_VERB_MV
                    && h.corr == 13
                    && cp_body_parse(&req[BUS_HDR_LEN..n])
                        == Ok((b"NOTE.TXT".as_slice(), b"DONE.TXT".as_slice()))
                {
                    w |= 1 << 2;
                }
            }
        }
    }

    // bit3: an EMPTY-content write round-trips (a 0-byte create): body = [name_len][name], no tail.
    {
        let mut req = [0u8; 128];
        let mut body = [0u8; 16];
        body[0] = 7;
        body[1..8].copy_from_slice(b"EMP.TXT");
        let n = build_request(BUS_VERB_WRITE, 14, &body[..8], &mut req);
        let ok = matches!(frame_parse(&req[..n]), Ok(h) if h.verb == BUS_VERB_WRITE)
            && write_body_parse(&req[BUS_HDR_LEN..n]) == Ok((b"EMP.TXT".as_slice(), b"".as_slice()));
        if ok {
            w |= 1 << 3;
        }
    }

    // bit4: fail-closed decoding of the new typed bodies — every malformed class refused BadBody.
    {
        let write_ok = write_body_parse(b"") == Err(BusDecodeErr::BadBody)   // empty body
            && write_body_parse(&[0x00, b'A']) == Err(BusDecodeErr::BadBody) // name_len 0
            && write_body_parse(&[0x0d, b'A']) == Err(BusDecodeErr::BadBody) // name_len > NAME_MAX
            && write_body_parse(&[0x08, b'A', b'B']) == Err(BusDecodeErr::BadBody) // name overruns body
            && write_body_parse(&[0x02, b'A', b' ', b'x']) == Err(BusDecodeErr::BadBody); // space in name
        // rm reuses cat_body_parse, mv reuses cp_body_parse (their fail-closed classes are proven in
        // BANDY-CODEC bit4); re-assert the shared parsers reject the obvious bus-rm/mv abuse here.
        let rm_ok = cat_body_parse(b"") == Err(BusDecodeErr::BadBody)
            && cat_body_parse(b"TOOLONGABCDEF") == Err(BusDecodeErr::BadBody);
        let mv_ok = cp_body_parse(&[0x00, b'A', b'B']) == Err(BusDecodeErr::BadBody)
            && cp_body_parse(&[0x02, b'A', b'B']) == Err(BusDecodeErr::BadBody); // no dst
        // a request frame carrying an UNKNOWN verb (7) is Malformed at the header (additive gate).
        let mut f = [0u8; 128];
        let n = build_request(BUS_VERB_RM, 12, b"NOTE.TXT", &mut f);
        f[6] = 7; // verb 7 is not valid
        let bad_verb = frame_parse(&f[..n]) == Err(BusDecodeErr::Malformed);
        if write_ok && rm_ok && mv_ok && bad_verb {
            w |= 1 << 4;
        }
    }

    // bit5: a WRITE with a max-length content round-trips at the frame ceiling (heap-staged — a
    // 4 KiB+ array has no business on a kernel stack). name_len=8 -> content = ceiling − 9 bytes.
    {
        let mut frame = alloc::vec![0u8; BUS_FRAME_MAX];
        let content_len = BUS_BODY_MAX - 1 - 8;
        let mut body = alloc::vec![0u8; BUS_BODY_MAX];
        body[0] = 8;
        body[1..9].copy_from_slice(b"BIGGG.BI"); // a valid 8-byte printable 8.3 name
        for (i, b) in body[9..9 + content_len].iter_mut().enumerate() {
            *b = 0x30 + (i % 10) as u8;
        }
        let n = build_request(BUS_VERB_WRITE, 15, &body[..BUS_BODY_MAX], &mut frame);
        let ok = n == BUS_FRAME_MAX
            && matches!(frame_parse(&frame[..n]), Ok(h) if h.verb == BUS_VERB_WRITE && h.body_len as usize == BUS_BODY_MAX)
            && matches!(write_body_parse(&frame[BUS_HDR_LEN..n]), Ok((name, content))
                if name == b"BIGGG.BI" && content.len() == content_len);
        if ok {
            w |= 1 << 5;
        }
    }

    const ALL: u32 = (1 << 6) - 1;
    if w == ALL {
        serial_println!(
            ":: BANDY-CODEC2: write-side wire frozen (write/rm/mv request goldens, typed write [name_len][name][content] payload, empty + at-ceiling content, decode fail-closed) PASS [w={:#04x}] ::",
            w
        );
    } else {
        serial_println!(":: BANDY-CODEC2: FAIL — w={:#x}/{:#x} ::", w, ALL);
    }
}
