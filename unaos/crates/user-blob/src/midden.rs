#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BANDY-1 M4: MIDDEN — program #3, the first real (non-fixture) on-UnaOS program (Peter's E
// verdict: named midden from day one; carried onto the boot media as `MIDDEN.BIN`). The on-metal
// seed of the host midden handler ("The Terminal"): it PARSES `ls` / `cat` / `cp` command TEXT
// into TYPED bus messages (the native v1 wire, arch/aarch64/bus.rs), sends them over the
// syscall transport (SYS_MSEND), receives the typed replies (SYS_MRECV), and PRINTS them —
// ROADMAP §3b principle 1: smart commands, no byte-stream reparsing between stages. Host-midden
// code sharing is deferred (Peter's E verdict); the host handler migrates to this wire in a
// later HOST arc.
//
// It also carries the EQUIVALENCE witness's EL0 legs (verdict D): the SAME file denied via the
// direct syscall surface (SYS_OPEN of the launcher-planted foreign-owned BUSPRIV.BIN) must be
// denied via the bus with the BYTE-SAME errno — and allowed <-> allowed on the public
// MIDTXT.TXT. Both comparisons happen HERE, at EL0, through the real production paths.
//
// Witness bits (SYS_REPORT -> the launcher's BANDY_MIDDEN_WITNESS):
//   bit0  ls round-trip: status 0, verb+corr echoed, non-empty listing body
//   bit1  cat MIDTXT.TXT round-trip: status 0 and the body is EXACTLY the launcher's fixture text
//   bit2  cp MIDTXT.TXT MIDCPY.TXT round-trip: status 0 (content verified kernel-side by the launcher)
//   bit3  bus cat BUSPRIV.BIN DENIED: reply status == -13 (EACCES)
//   bit4  direct SYS_OPEN(BUSPRIV.BIN, RO) DENIED: returns -13 (EACCES)
//   bit5  EQUIVALENCE: the two denial errnos are BYTE-SAME (bit3's status == bit4's return),
//         and the allowed legs agree (direct RO open of MIDTXT.TXT >= 0 while bus cat gave 0)
//   BANDY-2 write-side legs:
//   bit6  WRITE round-trip: write WRT.TXT -> cat WRT.TXT reads the content back byte-exact
//   bit7  RM round-trip: rm WRT.TXT -> cat WRT.TXT is now -ENOENT
//   bit8  MV round-trip: write MVA.TXT -> mv MVA.TXT MVB.TXT -> cat MVB byte-exact, cat MVA -ENOENT
//   bit9  EXTENDED EQUIVALENCE: rm/mv/write of the foreign BUSPRIV.BIN all DENIED -EACCES via the
//         bus, byte-same as a direct SYS_OPEN(RW) of it (-EACCES) — owner authority holds both ways
// then SYS_EXIT(0xB5) — the midden sentinel, routed to EL0_MIDDEN_DONE.
//
// Build discipline (the K2OWN build-lane idiom): an extra `[[bin]]` of crates/user-blob, flat
// blob <= one 4 KiB code page, fully position-independent, .text+.rodata only (NO .data/.bss —
// no mutable statics), and NO panic paths: every buffer access below goes through raw-pointer
// helpers with statically-sound bounds, because a single reachable core::panicking call drags
// the fmt machinery into a blob with a 4 KiB budget.

use core::arch::naked_asm;

/// Flat-blob entry: the kernel enters at offset 0 with SP_EL0 at the window top. Hop to the
/// Rust body (PC-relative `b` — no relocation, no GOT).
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    naked_asm!("b {main}", main = sym midden_main);
}

// --- syscall ABI (Linux-aarch64: x8 = nr, args x0..x2, ret x0) ---------------------------------

#[inline(always)]
fn svc(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            inout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            clobber_abi("C"),
        );
    }
    ret
}

const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
const SYS_REPORT: u64 = 3;
const SYS_OPEN: u64 = 11;
const SYS_CLOSE: u64 = 17;
const SYS_MSEND: u64 = 19;
const SYS_MRECV: u64 = 20;

const MIDDEN_EXIT_STATUS: u64 = 0xB5;

#[inline(always)]
fn con_write(buf: &[u8]) {
    let _ = svc(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64);
}

// --- the native v1 wire (mirrors arch/aarch64/bus.rs byte-for-byte) ----------------------------

const HDR: usize = 52;
const FRAME_MAX: usize = HDR + 4096;
const VERB_LS: u8 = 1;
const VERB_CAT: u8 = 2;
const VERB_CP: u8 = 3;
// BANDY-2 write-side verbs (mirror arch/aarch64/bus.rs).
const VERB_WRITE: u8 = 4;
const VERB_RM: u8 = 5;
const VERB_MV: u8 = 6;

#[inline(always)]
unsafe fn put(p: *mut u8, i: usize, b: u8) {
    unsafe { *p.add(i) = b };
}
#[inline(always)]
unsafe fn get(p: *const u8, i: usize) -> u8 {
    unsafe { *p.add(i) }
}

/// Build a request frame (header zeroed: status 0, principal ALL-ZERO — the kernel stamps).
/// `body` bounds are statically sound at every call site (names <= 12 bytes; cp <= 26).
fn build_req(buf: &mut [u8; 96], verb: u8, corr: u32, body: &[u8]) -> usize {
    let p = buf.as_mut_ptr();
    unsafe {
        let mut i = 0;
        while i < HDR {
            put(p, i, 0);
            i += 1;
        }
        put(p, 0, b'U');
        put(p, 1, b'B');
        put(p, 2, b'S');
        put(p, 3, b'1');
        put(p, 4, 1); // version
        put(p, 5, 1); // kind REQUEST
        put(p, 6, verb);
        put(p, 8, corr as u8);
        put(p, 9, (corr >> 8) as u8);
        put(p, 10, (corr >> 16) as u8);
        put(p, 11, (corr >> 24) as u8);
        let bl = body.len();
        put(p, 48, bl as u8);
        put(p, 49, (bl >> 8) as u8);
        let mut j = 0;
        while j < bl && HDR + j < 96 {
            put(p, HDR + j, get(body.as_ptr(), j));
            j += 1;
        }
        HDR + bl
    }
}

/// Parse a reply in `rep[..n]`: verify magic/version/kind, verb+corr echo; return the status
/// (i64-widened) and the body length, or i64::MIN on a malformed frame.
fn check_reply(rep: &[u8; FRAME_MAX], n: usize, verb: u8, corr: u32) -> (i64, usize) {
    let p = rep.as_ptr();
    if n < HDR {
        return (i64::MIN, 0);
    }
    unsafe {
        if get(p, 0) != b'U'
            || get(p, 1) != b'B'
            || get(p, 2) != b'S'
            || get(p, 3) != b'1'
            || get(p, 4) != 1
            || get(p, 5) != 2 // kind REPLY
            || get(p, 6) != verb
            || get(p, 8) != corr as u8
            || get(p, 9) != (corr >> 8) as u8
        {
            return (i64::MIN, 0);
        }
        let status = i32::from_le_bytes([get(p, 12), get(p, 13), get(p, 14), get(p, 15)]) as i64;
        let bl = (get(p, 48) as usize) | ((get(p, 49) as usize) << 8);
        if HDR + bl != n {
            return (i64::MIN, 0);
        }
        (status, bl)
    }
}

/// PARSE one command line (the E verdict's text->typed seam): split on ' ', map the verb token
/// to a typed request, and send it. Returns (status, body_len) from the reply, or i64::MIN on
/// a transport/parse failure. `rep` receives the reply frame.
fn run_command(line: &[u8], corr: u32, rep: &mut [u8; FRAME_MAX]) -> (i64, usize) {
    // tokenize: verb = up to the first space; rest = the argument tail
    let mut sp = line.len();
    let mut i = 0;
    while i < line.len() {
        if unsafe { get(line.as_ptr(), i) } == b' ' {
            sp = i;
            break;
        }
        i += 1;
    }
    let verb_tok = &line[..sp];
    let rest: &[u8] = if sp < line.len() { &line[sp + 1..] } else { &[] };

    let mut req = [0u8; 96];
    let n = if eq(verb_tok, b"ls") {
        build_req(&mut req, VERB_LS, corr, &[])
    } else if eq(verb_tok, b"cat") {
        if rest.is_empty() || rest.len() > 12 {
            return (i64::MIN, 0);
        }
        build_req(&mut req, VERB_CAT, corr, rest)
    } else if eq(verb_tok, b"cp") {
        // cp SRC DST -> body [src_len u8][src][dst]
        let mut sp2 = rest.len();
        let mut j = 0;
        while j < rest.len() {
            if unsafe { get(rest.as_ptr(), j) } == b' ' {
                sp2 = j;
                break;
            }
            j += 1;
        }
        if sp2 == 0 || sp2 > 12 || sp2 + 1 >= rest.len() || rest.len() - sp2 - 1 > 12 {
            return (i64::MIN, 0);
        }
        let mut body = [0u8; 26];
        let bp = body.as_mut_ptr();
        unsafe {
            put(bp, 0, sp2 as u8);
            let mut k = 0;
            while k < rest.len() && k < 25 {
                if k != sp2 {
                    // src bytes land at 1..=sp2, dst bytes at sp2+1.. (the space is dropped)
                    let dst_i = if k < sp2 { 1 + k } else { k };
                    put(bp, dst_i, get(rest.as_ptr(), k));
                }
                k += 1;
            }
        }
        build_req(&mut req, VERB_CP, corr, &body[..rest.len()])
    } else if eq(verb_tok, b"rm") {
        // rm NAME -> bare 8.3 name (same shape as cat)
        if rest.is_empty() || rest.len() > 12 {
            return (i64::MIN, 0);
        }
        build_req(&mut req, VERB_RM, corr, rest)
    } else if eq(verb_tok, b"mv") {
        // mv SRC DST -> body [src_len u8][src][dst] (same shape as cp)
        let mut sp2 = rest.len();
        let mut j = 0;
        while j < rest.len() {
            if unsafe { get(rest.as_ptr(), j) } == b' ' {
                sp2 = j;
                break;
            }
            j += 1;
        }
        if sp2 == 0 || sp2 > 12 || sp2 + 1 >= rest.len() || rest.len() - sp2 - 1 > 12 {
            return (i64::MIN, 0);
        }
        let mut body = [0u8; 26];
        let bp = body.as_mut_ptr();
        unsafe {
            put(bp, 0, sp2 as u8);
            let mut k = 0;
            while k < rest.len() && k < 25 {
                if k != sp2 {
                    let dst_i = if k < sp2 { 1 + k } else { k };
                    put(bp, dst_i, get(rest.as_ptr(), k));
                }
                k += 1;
            }
        }
        build_req(&mut req, VERB_MV, corr, &body[..rest.len()])
    } else if eq(verb_tok, b"write") {
        // write NAME CONTENT -> body [name_len u8][name][content]. Split the argument tail on the
        // FIRST space: name = up to it, content = the remainder (may itself contain spaces).
        let mut sp2 = rest.len();
        let mut j = 0;
        while j < rest.len() {
            if unsafe { get(rest.as_ptr(), j) } == b' ' {
                sp2 = j;
                break;
            }
            j += 1;
        }
        // name 1..=12; content bounded by the 96-byte request buffer (HDR 52 + 1 + name + content).
        if sp2 == 0 || sp2 > 12 || sp2 >= rest.len() {
            return (i64::MIN, 0);
        }
        let content_len = rest.len() - sp2 - 1;
        if 1 + sp2 + content_len > 44 {
            return (i64::MIN, 0);
        }
        let mut body = [0u8; 44];
        let bp = body.as_mut_ptr();
        unsafe {
            put(bp, 0, sp2 as u8);
            let mut k = 0;
            // name bytes -> body[1..=sp2]
            while k < sp2 {
                put(bp, 1 + k, get(rest.as_ptr(), k));
                k += 1;
            }
            // content bytes (after the space) -> body[1+sp2..]
            let mut c = 0;
            while c < content_len {
                put(bp, 1 + sp2 + c, get(rest.as_ptr(), sp2 + 1 + c));
                c += 1;
            }
        }
        build_req(&mut req, VERB_WRITE, corr, &body[..1 + sp2 + content_len])
    } else {
        return (i64::MIN, 0);
    };

    let verb_byte = req[6];
    if svc(SYS_MSEND, req.as_ptr() as u64, n as u64, 0) != 0 {
        return (i64::MIN, 0);
    }
    let rn = svc(SYS_MRECV, rep.as_mut_ptr() as u64, FRAME_MAX as u64, 0);
    if rn < HDR as i64 {
        return (i64::MIN, 0);
    }
    check_reply(rep, rn as usize, verb_byte, corr)
}

#[inline(always)]
fn eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if unsafe { get(a.as_ptr(), i) != get(b.as_ptr(), i) } {
            return false;
        }
        i += 1;
    }
    true
}

/// The fixture text the launcher stages in MIDTXT.TXT — bit1 byte-verifies the cat body.
const MIDTXT_CONTENT: &[u8] = b"midden bus fixture\n";
const EACCES: i64 = -13;
const ENOENT: i64 = -2;
/// BANDY-2: the content midden WRITES (no spaces — the write parser splits name/content on the
/// first space); the cat readback byte-verifies it. Also the mv round-trip's payload.
const WR_CONTENT: &[u8] = b"middenwrote";
/// The truncate-overwrite content (bit6) — a SHORTER string, so the readback proving it equals this
/// (not the old longer content) confirms a genuine truncate, not a stale-tail overwrite-in-place.
const WR2_CONTENT: &[u8] = b"again";

#[unsafe(no_mangle)]
extern "C" fn midden_main() -> ! {
    let mut w: u64 = 0;
    let mut rep = [0u8; FRAME_MAX];

    // (0) "ls" — parse, send, print the listing
    let (st, bl) = run_command(b"ls", 1, &mut rep);
    if st == 0 && bl > 0 {
        w |= 1 << 0;
        con_write(b"midden> ls\n");
        con_write(&rep[HDR..HDR + bl]);
    }

    // (1) "cat MIDTXT.TXT" — the allowed cat; body must be the fixture text, byte-exact
    let (st_cat, bl) = run_command(b"cat MIDTXT.TXT", 2, &mut rep);
    if st_cat == 0 && bl == MIDTXT_CONTENT.len() && eq(&rep[HDR..HDR + bl], MIDTXT_CONTENT) {
        w |= 1 << 1;
        con_write(b"midden> cat MIDTXT.TXT\n");
        con_write(&rep[HDR..HDR + bl]);
    }

    // (2) "cp MIDTXT.TXT MIDCPY.TXT" — success reply is status 0, empty body
    let (st, bl) = run_command(b"cp MIDTXT.TXT MIDCPY.TXT", 3, &mut rep);
    if st == 0 && bl == 0 {
        w |= 1 << 2;
        con_write(b"midden> cp MIDTXT.TXT MIDCPY.TXT ok\n");
    }

    // (3) "cat BUSPRIV.BIN" — the DENIED bus leg (foreign-owned file)
    let (st_bus_denied, _) = run_command(b"cat BUSPRIV.BIN", 4, &mut rep);
    if st_bus_denied == EACCES {
        w |= 1 << 3;
    }

    // (4) direct SYS_OPEN(BUSPRIV.BIN, RO) — the DENIED direct leg
    let name = b"BUSPRIV.BIN";
    let st_direct_denied = svc(SYS_OPEN, name.as_ptr() as u64, name.len() as u64, 0);
    if st_direct_denied == EACCES {
        w |= 1 << 4;
    }

    // (5) EQUIVALENCE (verdict D): denied == denied with the BYTE-SAME errno, allowed == allowed
    let txt = b"MIDTXT.TXT";
    let h = svc(SYS_OPEN, txt.as_ptr() as u64, txt.len() as u64, 0);
    let allowed_agree = h >= 0 && st_cat == 0;
    if h >= 0 {
        let _ = svc(SYS_CLOSE, h as u64, 0, 0);
    }
    if st_bus_denied == st_direct_denied && st_bus_denied == EACCES && allowed_agree {
        w |= 1 << 5;
    }

    // ===== BANDY-2 write-side legs =====================================================

    // (6) WRITE round-trip: CREATE WRT.TXT, cat it back byte-exact, then TRUNCATE-overwrite it with
    // new content and cat that back byte-exact (exercises both create and truncate-by-owner).
    let (st_w, bl_w) = run_command(b"write WRT.TXT middenwrote", 6, &mut rep);
    let (st_rb, bl_rb) = run_command(b"cat WRT.TXT", 7, &mut rep);
    let create_ok =
        st_w == 0 && bl_w == 0 && st_rb == 0 && bl_rb == WR_CONTENT.len() && eq(&rep[HDR..HDR + bl_rb], WR_CONTENT);
    let (st_w2, bl_w2) = run_command(b"write WRT.TXT again", 18, &mut rep);
    let (st_rb2, bl_rb2) = run_command(b"cat WRT.TXT", 19, &mut rep);
    let trunc_ok =
        st_w2 == 0 && bl_w2 == 0 && st_rb2 == 0 && bl_rb2 == WR2_CONTENT.len() && eq(&rep[HDR..HDR + bl_rb2], WR2_CONTENT);
    if create_ok && trunc_ok {
        w |= 1 << 6;
        con_write(b"midden> write WRT.TXT (create+truncate) / cat byte-exact ok\n");
    }

    // (7) RM round-trip: delete WRT.TXT, then cat -> -ENOENT (the name is gone).
    let (st_rm, _) = run_command(b"rm WRT.TXT", 8, &mut rep);
    let (st_gone, _) = run_command(b"cat WRT.TXT", 9, &mut rep);
    if st_rm == 0 && st_gone == ENOENT {
        w |= 1 << 7;
        con_write(b"midden> rm WRT.TXT / cat -> gone ok\n");
    }

    // (8) MV round-trip: write MVA.TXT, rename to MVB.TXT, cat MVB byte-exact, cat MVA -> gone.
    let (st_wa, _) = run_command(b"write MVA.TXT middenwrote", 10, &mut rep);
    let (st_mv, _) = run_command(b"mv MVA.TXT MVB.TXT", 11, &mut rep);
    let (st_catb, bl_b) = run_command(b"cat MVB.TXT", 12, &mut rep);
    let mvb_ok = st_catb == 0 && bl_b == WR_CONTENT.len() && eq(&rep[HDR..HDR + bl_b], WR_CONTENT);
    let (st_cata, _) = run_command(b"cat MVA.TXT", 13, &mut rep);
    if st_wa == 0 && st_mv == 0 && mvb_ok && st_cata == ENOENT {
        w |= 1 << 8;
        con_write(b"midden> mv MVA.TXT MVB.TXT / cat MVB ok / MVA gone\n");
        let _ = run_command(b"rm MVB.TXT", 14, &mut rep); // self-clean the mv target
    }

    // (9) EXTENDED EQUIVALENCE (verdict D, write side): every DESTRUCTIVE verb on the foreign-owned
    // BUSPRIV.BIN is DENIED -EACCES via the bus, BYTE-SAME as the direct SYS_OPEN(RW) denial — the
    // owner-authority gate holds identically through the bus and the syscall surface.
    let (st_rm_denied, _) = run_command(b"rm BUSPRIV.BIN", 15, &mut rep);
    let (st_mv_denied, _) = run_command(b"mv BUSPRIV.BIN STOLEN.BIN", 16, &mut rep);
    let (st_wr_denied, _) = run_command(b"write BUSPRIV.BIN x", 17, &mut rep);
    // direct: opening the foreign file for WRITE (delete/truncate would need such a handle) -> EACCES.
    // mode bit0 = RW (CAP_READ|CAP_WRITE); no O_CREAT (the file exists).
    let bp = b"BUSPRIV.BIN";
    const O_RW: u64 = 1;
    let st_direct_rw = svc(SYS_OPEN, bp.as_ptr() as u64, bp.len() as u64, O_RW);
    if st_rm_denied == EACCES
        && st_mv_denied == EACCES
        && st_wr_denied == EACCES
        && st_direct_rw == EACCES
    {
        w |= 1 << 9;
    }

    let _ = svc(SYS_REPORT, w, 0, 0);
    let _ = svc(SYS_EXIT, MIDDEN_EXIT_STATUS, 0, 0);
    loop {}
}

/// No panic path exists by construction (raw-pointer helpers, no fmt); this never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
