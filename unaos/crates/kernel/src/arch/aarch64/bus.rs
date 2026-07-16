// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BANDY-1 (ROADMAP §3b arc 1): the on-UnaOS SMessage bus — the v1 SUBSET CODEC.
//
// "Port the bus, not the binary convention." This module is the WIRE layer of the syscall-backed
// bus transport (SYS_MSEND/SYS_MRECV live in syscall.rs): a fixed 52-byte header + typed verb
// payloads for the first three verbs (ls / cat / cp), and the reply bodies.
//
// COMPAT SURFACE (verdict A as ruled at the arc's STOP, Maestro 2026-07-16 — state it honestly):
//   * REPLY BODIES are HOST-GOLDEN: the exact serde_json bytes the host bandy bus's `SMessage`
//     reply subset serializes to (`{"TerminalOutput":"…"}` / `{"TerminalError":"…"}`). The
//     goldens in `bus_codec_selftest` are CAPTURED from the real host serializer by
//     `tools/bandy-golden` (never hand-authored); the emitters here are proven byte-compatible
//     against those captures every boot.
//   * The HEADER and the REQUEST payloads are UnaOS-NATIVE: host bandy is an in-process
//     broadcast of `SMessage` values (no header, no framing, no principal) and has no ls/cat/cp
//     request variant — there is nothing to be byte-compatible WITH. Their KATs below freeze
//     self-authored goldens from this first commit; they are the compat anchor going forward.
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
// No heap use anywhere in this module beyond caller-provided buffers; no_std; aarch64-only
// (declared under arch/aarch64; zero x86 surface).

/// Frame magic: b"UBS1" — UnaOS Bus, wire v1.
pub const BUS_MAGIC: [u8; 4] = *b"UBS1";
/// Wire version. A bump is a protocol break (rule on it; never silent).
pub const BUS_VERSION: u8 = 1;

/// Header length. Layout (little-endian):
///   0..4  magic  4..5 version  5..6 kind  6..7 verb  7..8 reserved(=0)
///   8..12 corr(u32)  12..16 status(i32)  16..48 principal(32B)  48..52 body_len(u32)
pub const BUS_HDR_LEN: usize = 52;
/// HARD DECODE CEILING for the body — the max forced allocation per frame (see module doc).
pub const BUS_BODY_MAX: usize = 4096;
/// The whole-frame ceiling: header + max body.
pub const BUS_FRAME_MAX: usize = BUS_HDR_LEN + BUS_BODY_MAX;

/// Frame kinds.
pub const BUS_KIND_REQUEST: u8 = 1;
pub const BUS_KIND_REPLY: u8 = 2;

/// Request verbs (the v1 subset — ROADMAP §3b's first three).
pub const BUS_VERB_LS: u8 = 1;
pub const BUS_VERB_CAT: u8 = 2;
pub const BUS_VERB_CP: u8 = 3;

/// 8.3 name bound — mirrors syscall.rs MAX_NAME (the sys_open bound the equivalence witness
/// holds cat/cp to).
pub const BUS_NAME_MAX: usize = 12;

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
    /// Shorter than one header, or the declared body overruns the presented bytes.
    Truncated,
    /// Bad magic / version / reserved byte / kind / verb-for-kind.
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

/// Serialize a header into `out[..BUS_HDR_LEN]`. Kernel-side use only (the kernel builds replies
/// and re-stamps validated requests); EL0 clients hand-build the same layout.
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
/// length ≥ header; magic/version/reserved; kind ∈ {REQUEST, REPLY}; verb valid for the kind;
/// body_len ≤ BUS_BODY_MAX (the ceiling — checked BEFORE the overrun check so an absurd length is
/// TooBig, not Truncated); declared body exactly fits the presented bytes (no trailing slack — a
/// frame is exact, not a prefix). Returns the header; the body is `frame[BUS_HDR_LEN..]`.
pub fn frame_parse(frame: &[u8]) -> Result<BusHdr, BusDecodeErr> {
    if frame.len() < BUS_HDR_LEN {
        return Err(BusDecodeErr::Truncated);
    }
    if frame[0..4] != BUS_MAGIC || frame[4] != BUS_VERSION || frame[7] != 0 {
        return Err(BusDecodeErr::Malformed);
    }
    let kind = frame[5];
    let verb = frame[6];
    let verb_ok = match kind {
        BUS_KIND_REQUEST => matches!(verb, BUS_VERB_LS | BUS_VERB_CAT | BUS_VERB_CP),
        BUS_KIND_REPLY => verb == 0,
        _ => return Err(BusDecodeErr::Malformed),
    };
    if !verb_ok {
        return Err(BusDecodeErr::Malformed);
    }
    let body_len = rd_u32(&frame[48..52]);
    if body_len as usize > BUS_BODY_MAX {
        return Err(BusDecodeErr::TooBig); // the hard ceiling — fail closed, nothing decoded
    }
    if frame.len() != BUS_HDR_LEN + body_len as usize {
        return Err(BusDecodeErr::Truncated);
    }
    let mut principal = [0u8; 32];
    principal.copy_from_slice(&frame[16..48]);
    Ok(BusHdr {
        kind,
        verb,
        corr: rd_u32(&frame[8..12]),
        status: i32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]),
        principal,
        body_len,
    })
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

/// CAT body: the bare 8.3 name bytes. 1..=BUS_NAME_MAX, ASCII printable (the sys_open twin turns
/// non-UTF-8 into -ENOENT; here a non-ASCII byte can never be a FAT 8.3 name, so it is BadBody —
/// the transport refuses what the namespace could never match).
pub fn cat_body_parse(body: &[u8]) -> Result<&[u8], BusDecodeErr> {
    if body.is_empty() || body.len() > BUS_NAME_MAX {
        return Err(BusDecodeErr::BadBody);
    }
    if !body.iter().all(|&b| (0x21..0x7f).contains(&b)) {
        return Err(BusDecodeErr::BadBody);
    }
    Ok(body)
}

/// CP body: `[src_len u8][src bytes][dst bytes]` — both names under the CAT rules; the length
/// arithmetic must consume the body EXACTLY (no slack, no overlap).
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

// ---------------------------------------------------------------------------------------------
// Reply-body emitters — byte-compatible with the HOST serializer (serde_json) for the reply
// subset. Proven against tools/bandy-golden captures in bus_codec_selftest, every boot.
// ---------------------------------------------------------------------------------------------

/// serde_json's string-escape rules, byte-exact (see serde_json ser.rs: `"` and `\` escaped;
/// 0x08/0x09/0x0A/0x0C/0x0D as \b \t \n \f \r; every other byte < 0x20 as \u00xx with LOWERCASE
/// hex; everything ≥ 0x20 — including DEL 0x7f — passes through raw). Input here is ASCII by
/// construction (fulfillment sanitizes file content), so the ≥ 0x80 UTF-8 question never arises.
/// Appends to `out` up to `max`; returns false (caller fails closed) if the budget would overflow.
fn json_escape_into(s: &[u8], out: &mut [u8], pos: &mut usize, max: usize) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in s {
        let esc: &[u8] = match b {
            b'"' => b"\\\"",
            b'\\' => b"\\\\",
            0x08 => b"\\b",
            0x09 => b"\\t",
            0x0a => b"\\n",
            0x0c => b"\\f",
            0x0d => b"\\r",
            c if c < 0x20 => {
                if *pos + 6 > max {
                    return false;
                }
                out[*pos..*pos + 4].copy_from_slice(b"\\u00");
                out[*pos + 4] = HEX[(c >> 4) as usize];
                out[*pos + 5] = HEX[(c & 0xf) as usize];
                *pos += 6;
                continue;
            }
            _ => {
                if *pos + 1 > max {
                    return false;
                }
                out[*pos] = b;
                *pos += 1;
                continue;
            }
        };
        if *pos + esc.len() > max {
            return false;
        }
        out[*pos..*pos + esc.len()].copy_from_slice(esc);
        *pos += esc.len();
    }
    true
}

fn json_wrap_into(variant: &[u8], text: &[u8], out: &mut [u8]) -> Option<usize> {
    let max = out.len();
    // {"<variant>":"<escaped>"}
    let head_len = 1 + 1 + variant.len() + 1 + 2; // {"VARIANT":"
    if head_len > max {
        return None;
    }
    out[0] = b'{';
    out[1] = b'"';
    out[2..2 + variant.len()].copy_from_slice(variant);
    let mut pos = 2 + variant.len();
    out[pos..pos + 3].copy_from_slice(b"\":\"");
    pos += 3;
    if !json_escape_into(text, out, &mut pos, max) {
        return None;
    }
    if pos + 2 > max {
        return None;
    }
    out[pos..pos + 2].copy_from_slice(b"\"}");
    Some(pos + 2)
}

/// `{"TerminalOutput":"<text>"}` — the host reply shape for verb output. `None` = the escaped
/// body would exceed `out` (the caller sized `out` at BUS_BODY_MAX: fail closed, send an error
/// reply instead — never a truncated JSON body).
pub fn reply_output_into(text: &[u8], out: &mut [u8]) -> Option<usize> {
    json_wrap_into(b"TerminalOutput", text, out)
}

/// `{"TerminalError":"<text>"}` — the host reply shape for a refused/failed verb.
pub fn reply_error_into(text: &[u8], out: &mut [u8]) -> Option<usize> {
    json_wrap_into(b"TerminalError", text, out)
}

// ---------------------------------------------------------------------------------------------
// BANDY-CODEC selftest — the M1 KATs, run every boot (the K4-ready idiom: read-only, in-RAM,
// no disk, no card; an uncounted `:: BANDY-CODEC: … ::` witness line).
// ---------------------------------------------------------------------------------------------

/// HOST-GOLDEN reply bodies — captured from the REAL host serializer by `tools/bandy-golden`
/// (run of 2026-07-16, committed as tools/bandy-golden/golden-frames.txt). NEVER hand-authored;
/// regenerate with `cargo run -p bandy-golden` and re-paste on a ruled protocol change only.
const GOLDEN_LS: (&[u8], &[u8]) = (
    b"HELLO.BIN 1024\nK2OWN.BIN 512\n",
    br#"{"TerminalOutput":"HELLO.BIN 1024\nK2OWN.BIN 512\n"}"#,
);
const GOLDEN_ESCAPES: (&[u8], &[u8]) = (
    b"line1\nline2\ttab\r\"quoted\" back\\slash\x08\x0c",
    br#"{"TerminalOutput":"line1\nline2\ttab\r\"quoted\" back\\slash\b\f"}"#,
);
const GOLDEN_CTL: (&[u8], &[u8]) =
    (b"ctl:\x01\x1fend", br#"{"TerminalOutput":"ctl:\u0001\u001fend"}"#);
const GOLDEN_DEL: (&[u8], &[u8]) = (b"del:\x7fend", b"{\"TerminalOutput\":\"del:\x7fend\"}");
const GOLDEN_EMPTY: (&[u8], &[u8]) = (b"", br#"{"TerminalOutput":""}"#);
const GOLDEN_ERR1: (&[u8], &[u8]) = (b"cat: errno -13", br#"{"TerminalError":"cat: errno -13"}"#);
const GOLDEN_ERR2: (&[u8], &[u8]) = (b"cp: errno -2", br#"{"TerminalError":"cp: errno -2"}"#);

/// SELF-AUTHORED request goldens (frozen at the first commit — the compat anchor for the
/// UnaOS-native header + typed payloads; there is no host frame to capture, see the module doc).
/// A `cat GROW.BIN` request, corr = 7: header + 8-byte name body.
const GOLDEN_REQ_CAT: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, // "UBS1"
    0x01, // version 1
    0x01, // kind REQUEST
    0x02, // verb CAT
    0x00, // reserved
    0x07, 0x00, 0x00, 0x00, // corr = 7
    0x00, 0x00, 0x00, 0x00, // status = 0
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal[0..16] = zero
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // principal[16..32] = zero
    0x08, 0x00, 0x00, 0x00, // body_len = 8
    b'G', b'R', b'O', b'W', b'.', b'B', b'I', b'N', // body = "GROW.BIN"
];

fn golden_reply_ok(sample: (&[u8], &[u8])) -> bool {
    let mut buf = [0u8; BUS_BODY_MAX];
    reply_output_into(sample.0, &mut buf) == Some(sample.1.len())
        && &buf[..sample.1.len()] == sample.1
}

fn golden_error_ok(sample: (&[u8], &[u8])) -> bool {
    let mut buf = [0u8; BUS_BODY_MAX];
    reply_error_into(sample.0, &mut buf) == Some(sample.1.len())
        && &buf[..sample.1.len()] == sample.1
}

/// Build a minimal valid request frame into `buf`; returns the frame slice length.
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

/// The M1 witness: goldens (host-captured replies + frozen native requests) + fail-closed
/// decoding + the ceilings. Emits ONE uncounted `:: BANDY-CODEC: … PASS/FAIL … ::` line.
pub fn bus_codec_selftest() {
    let mut w = 0u32;

    // bit0..bit2: host-golden reply bodies, byte-compat with serde_json (capture provenance above)
    if golden_reply_ok(GOLDEN_LS) {
        w |= 1 << 0;
    }
    if golden_reply_ok(GOLDEN_ESCAPES) && golden_reply_ok(GOLDEN_CTL) {
        w |= 1 << 1;
    }
    if golden_reply_ok(GOLDEN_DEL)
        && golden_reply_ok(GOLDEN_EMPTY)
        && golden_error_ok(GOLDEN_ERR1)
        && golden_error_ok(GOLDEN_ERR2)
    {
        w |= 1 << 2;
    }

    // bit3: the frozen native request golden — build_request must reproduce it byte-for-byte,
    // and frame_parse must round-trip it field-exact.
    let mut req = [0u8; BUS_FRAME_MAX];
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
                w |= 1 << 3;
            }
        }
    }

    // bit4: ls + cp round-trips (typed payloads)
    {
        let mut f = [0u8; BUS_FRAME_MAX];
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
            w |= 1 << 4;
        }
    }

    // bit5: fail-closed decoding — every malformed class refused with the RIGHT refusal
    {
        let mut f = [0u8; BUS_FRAME_MAX];
        let n = build_request(BUS_VERB_CAT, 7, b"GROW.BIN", &mut f);
        let mut bad_magic = [0u8; BUS_FRAME_MAX];
        bad_magic[..n].copy_from_slice(&f[..n]);
        bad_magic[0] = b'X';
        let mut bad_ver = [0u8; BUS_FRAME_MAX];
        bad_ver[..n].copy_from_slice(&f[..n]);
        bad_ver[4] = 2;
        let mut bad_rsvd = [0u8; BUS_FRAME_MAX];
        bad_rsvd[..n].copy_from_slice(&f[..n]);
        bad_rsvd[7] = 1;
        let mut bad_kind = [0u8; BUS_FRAME_MAX];
        bad_kind[..n].copy_from_slice(&f[..n]);
        bad_kind[5] = 9;
        let mut bad_verb = [0u8; BUS_FRAME_MAX];
        bad_verb[..n].copy_from_slice(&f[..n]);
        bad_verb[6] = 9;
        let mut bad_status = [0u8; BUS_FRAME_MAX];
        bad_status[..n].copy_from_slice(&f[..n]);
        bad_status[12] = 1; // nonzero status in a request
        let mut bad_prin = [0u8; BUS_FRAME_MAX];
        bad_prin[..n].copy_from_slice(&f[..n]);
        bad_prin[16] = 2; // caller-supplied principal kind byte — MUST be refused, not overwritten
        let ok = frame_parse(&bad_magic[..n]) == Err(BusDecodeErr::Malformed)
            && frame_parse(&bad_ver[..n]) == Err(BusDecodeErr::Malformed)
            && frame_parse(&bad_rsvd[..n]) == Err(BusDecodeErr::Malformed)
            && frame_parse(&bad_kind[..n]) == Err(BusDecodeErr::Malformed)
            && frame_parse(&bad_verb[..n]) == Err(BusDecodeErr::Malformed)
            && matches!(
                frame_parse(&bad_status[..n]).map(|h| request_validate(&h)),
                Ok(Err(BusDecodeErr::BadPrincipal))
            )
            && matches!(
                frame_parse(&bad_prin[..n]).map(|h| request_validate(&h)),
                Ok(Err(BusDecodeErr::BadPrincipal))
            )
            && frame_parse(&f[..BUS_HDR_LEN - 1]) == Err(BusDecodeErr::Truncated)
            && frame_parse(&f[..n - 1]) == Err(BusDecodeErr::Truncated) // body shorter than declared
            && cat_body_parse(b"") == Err(BusDecodeErr::BadBody)
            && cat_body_parse(b"WAYTOOLONGNAME") == Err(BusDecodeErr::BadBody)
            && cat_body_parse(b"BAD NAME") == Err(BusDecodeErr::BadBody) // 0x20 refused
            && cp_body_parse(&[0x08]) == Err(BusDecodeErr::BadBody)
            && cp_body_parse(&[0x00, b'A', b'B']) == Err(BusDecodeErr::BadBody)
            && cp_body_parse(&[0x02, b'A', b'B']) == Err(BusDecodeErr::BadBody); // no dst bytes
        if ok {
            w |= 1 << 5;
        }
    }

    // bit6: the HARD CEILING — body_len over BUS_BODY_MAX is TooBig (fail-closed, nothing
    // decoded); a frame AT the ceiling parses. The ceiling is the max forced allocation.
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
        // Present a header claiming an over-ceiling body: refused as TooBig from the HEADER ALONE
        // (the transport never stages an over-ceiling body to find out).
        let too_big = frame_parse(&hdr) == Err(BusDecodeErr::TooBig);
        let mut max_frame = [0u8; BUS_FRAME_MAX];
        let h2 = BusHdr { body_len: BUS_BODY_MAX as u32, verb: BUS_VERB_LS, ..h };
        hdr_write(&h2, &mut max_frame);
        // (an LS with a max body is semantically odd but wire-legal; typed validation is later)
        let at_ceiling = frame_parse(&max_frame).is_ok();
        if too_big && at_ceiling {
            w |= 1 << 6;
        }
    }

    // bit7: reply emitters fail CLOSED on budget overflow (never a truncated JSON body)
    {
        let mut tiny = [0u8; 8];
        if reply_output_into(b"0123456789", &mut tiny).is_none()
            && reply_error_into(b"0123456789", &mut tiny).is_none()
        {
            w |= 1 << 7;
        }
    }

    const ALL: u32 = (1 << 8) - 1;
    if w == ALL {
        serial_println!(
            ":: BANDY-CODEC: v1 subset codec — reply bodies HOST-golden (serde_json byte-compat, tools/bandy-golden capture), native request header+payloads frozen, decode fail-closed (body ceiling {} B) PASS [w={:#04x}] ::",
            BUS_BODY_MAX,
            w
        );
    } else {
        serial_println!(":: BANDY-CODEC: FAIL — w={:#x}/{:#x} ::", w, ALL);
    }
}
