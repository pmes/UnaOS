//! CHARTER: Kernel — fs-core
//!
//! ATTRSURF (B299): the attribute verbs on the v1 bus — ADDITIVE to the frozen frame exactly as
//! BANDY-2 was: five new verb tags (`BUS_VERB_ATTR_SET/GET/LIST/QUERY/STAT` = 11..=15, una-abi) and
//! typed bodies, no header or ceiling change. The bodies are una-abi's ATTRSURF layout, the SAME
//! bytes `SYS_ATTR_*` / `SYS_QUERY` / `SYS_STAT` take: SET/GET carry a request
//! (`[path_len u16][key_len u16][path][key][value wire]`), LIST/STAT carry the bare absolute path,
//! QUERY the bare expression. Fulfilment is `crate::fs::attrsys::bus_fulfil` on both arches.
//!
//! Lives in its own file because `bus.rs` is LINE-NEUTRAL (panic `Location`s); declared there on an
//! existing line. KATs: [`selftest`], one uncounted `:: BANDY-ATTR: … ::` line every boot beside
//! BANDY-CODEC2.
use super::{build_request, frame_parse, request_validate, BusDecodeErr, BUS_HDR_LEN, BUS_KIND_REQUEST};
pub use una_abi::{BUS_VERB_ATTR_GET, BUS_VERB_ATTR_LIST, BUS_VERB_ATTR_QUERY, BUS_VERB_ATTR_SET, BUS_VERB_ATTR_STAT};

/// Is `verb` one of the five attribute verbs?
#[inline]
pub fn is_attr_verb(verb: u8) -> bool {
    (BUS_VERB_ATTR_SET..=BUS_VERB_ATTR_STAT).contains(&verb)
}

/// Typed body validation, fail-closed. SET: a request whose remainder is exactly ONE valid value
/// wire. GET: a request with a non-empty key and NO remainder. LIST/STAT: a non-empty path of at
/// most `ATTR_PATH_MAX` bytes. QUERY: a non-empty UTF-8 expression of at most `ATTR_VALUE_MAX`.
pub fn attr_body_parse(verb: u8, body: &[u8]) -> Result<(), BusDecodeErr> {
    let ok = match verb {
        BUS_VERB_ATTR_SET => matches!(una_abi::attr_req_parse(body), Ok((_, k, rest))
            if !k.is_empty() && matches!(una_abi::attr_wire_parse(rest), Ok((_, _, used)) if used == rest.len())),
        BUS_VERB_ATTR_GET => matches!(una_abi::attr_req_parse(body), Ok((_, k, rest)) if !k.is_empty() && rest.is_empty()),
        BUS_VERB_ATTR_LIST | BUS_VERB_ATTR_STAT => !body.is_empty() && body.len() <= una_abi::ATTR_PATH_MAX,
        BUS_VERB_ATTR_QUERY => !body.is_empty() && body.len() <= una_abi::ATTR_VALUE_MAX && core::str::from_utf8(body).is_ok(),
        _ => false,
    };
    if ok { Ok(()) } else { Err(BusDecodeErr::BadBody) }
}

/// Frozen `SET` request, corr = 21.
const GOLDEN_SET: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 0x0b, 0x00, 0x15, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x1c, 0x00, 0x00, 0x00, 0x07, 0x00, 0x01, 0x00, 0x2f, 0x68, 0x6f, 0x6d, 0x65, 0x2f, 0x41, 0x6e,
    0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Frozen `GET` request, corr = 22.
const GOLDEN_GET: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 0x0c, 0x00, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x0c, 0x00, 0x00, 0x00, 0x07, 0x00, 0x01, 0x00, 0x2f, 0x68, 0x6f, 0x6d, 0x65, 0x2f, 0x41, 0x6e,
];

/// Frozen `LIST` request, corr = 23.
const GOLDEN_LIST: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 0x0d, 0x00, 0x17, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x00, 0x00, 0x00, 0x2f, 0x68, 0x6f, 0x6d, 0x65, 0x2f, 0x41,
];

/// Frozen `QUERY` request, corr = 24.
const GOLDEN_QUERY: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 0x0e, 0x00, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x00, 0x00, 0x00, 0x6e, 0x20, 0x3d, 0x3d, 0x20, 0x34, 0x32,
];

/// Frozen `STAT` request, corr = 25.
const GOLDEN_STAT: &[u8] = &[
    0x55, 0x42, 0x53, 0x31, 0x01, 0x01, 0x0f, 0x00, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x07, 0x00, 0x00, 0x00, 0x2f, 0x68, 0x6f, 0x6d, 0x65, 0x2f, 0x41,
];

/// The KATs: each frozen golden is reproduced byte-for-byte by `build_request`, parses through the
/// frozen `frame_parse` + `request_validate`, and its body passes [`attr_body_parse`]; then the
/// fail-closed classes (trailing slack after a SET value, a GET with a value, a bad tag, an empty
/// path) are refused.
pub fn selftest() {
    let mut w = 0u32;
    let goldens: [(&[u8], u8, u32); 5] = [
        (GOLDEN_SET, BUS_VERB_ATTR_SET, 21),
        (GOLDEN_GET, BUS_VERB_ATTR_GET, 22),
        (GOLDEN_LIST, BUS_VERB_ATTR_LIST, 23),
        (GOLDEN_QUERY, BUS_VERB_ATTR_QUERY, 24),
        (GOLDEN_STAT, BUS_VERB_ATTR_STAT, 25),
    ];
    for (i, (g, verb, corr)) in goldens.iter().enumerate() {
        let body = &g[BUS_HDR_LEN..];
        let mut buf = [0u8; 128];
        let n = build_request(*verb, *corr, body, &mut buf);
        let ok = n == g.len()
            && &buf[..n] == *g
            && matches!(frame_parse(&buf[..n]), Ok(h) if h.kind == BUS_KIND_REQUEST && h.verb == *verb && h.corr == *corr && request_validate(&h).is_ok())
            && attr_body_parse(*verb, body).is_ok();
        if ok {
            w |= 1 << i;
        }
    }
    {
        let set = &GOLDEN_SET[BUS_HDR_LEN..];
        let mut slack = [0u8; 64];
        slack[..set.len()].copy_from_slice(set);
        let slack_bad = attr_body_parse(BUS_VERB_ATTR_SET, &slack[..set.len() + 1]) == Err(BusDecodeErr::BadBody);
        let get_with_value = attr_body_parse(BUS_VERB_ATTR_GET, set) == Err(BusDecodeErr::BadBody);
        let mut badtag = [0u8; 64];
        badtag[..set.len()].copy_from_slice(set);
        badtag[12] = 9; // the value wire's tag byte (4 + 7 path + 1 key = 12)
        let tag_bad = attr_body_parse(BUS_VERB_ATTR_SET, &badtag[..set.len()]) == Err(BusDecodeErr::BadBody);
        let empty_bad = attr_body_parse(BUS_VERB_ATTR_STAT, b"") == Err(BusDecodeErr::BadBody)
            && attr_body_parse(BUS_VERB_ATTR_QUERY, b"") == Err(BusDecodeErr::BadBody);
        if slack_bad && get_with_value && tag_bad && empty_bad {
            w |= 1 << 5;
        }
    }
    const ALL: u32 = (1 << 6) - 1;
    if w == ALL {
        serial_println!(":: BANDY-ATTR: attribute verbs on the wire (set/get/list/query/stat request goldens 11..=15, typed bodies, fail-closed) -> PASS [w={:#04x}] ::", w);
    } else {
        serial_println!(":: BANDY-ATTR: w={:#x}/{:#x} -> FAIL ::", w, ALL);
    }
}
