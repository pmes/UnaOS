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
// WC-C: midden also owns a COMPOSITOR WINDOW — a 24x16 surface via SYS_WIN_CREATE into which it renders
// its own bus stats as hex digits (see the window section below). The kernel draws the border and the
// title strip; midden draws the content. A refused create makes every repaint a no-op, so the witness
// bits above never depend on the compositor.
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
/// WC-C: the window verbs. midden is the first NON-fixture program to own a compositor window.
const SYS_WIN_CREATE: u64 = 29;
const SYS_WIN_PRESENT: u64 = 30;

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

/// Build a TWO-NAME request (`cp SRC DST`, `mv SRC DST`) into `req`: body is `[src_len u8][src][dst]`,
/// the space dropped. Returns the frame length, or 0 when the argument tail is not a valid pair
/// (missing separator, either name empty or over the 12-byte 8.3 bound).
///
/// `cp` and `mv` had byte-identical builders; the flat blob has a hard 4 KiB code-page budget and paid
/// for both copies, so they share one here. Verb-parameterised, not verb-aware — the kernel decides what
/// the verb MEANS; this only shapes the frame.
#[inline(never)]
fn build_pair(req: &mut [u8; 96], verb: u8, corr: u32, rest: &[u8]) -> usize {
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
        return 0;
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
    build_req(req, verb, corr, &body[..rest.len()])
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
        match build_pair(&mut req, VERB_CP, corr, rest) {
            0 => return (i64::MIN, 0),
            n => n,
        }
    } else if eq(verb_tok, b"rm") {
        // rm NAME -> bare 8.3 name (same shape as cat)
        if rest.is_empty() || rest.len() > 12 {
            return (i64::MIN, 0);
        }
        build_req(&mut req, VERB_RM, corr, rest)
    } else if eq(verb_tok, b"mv") {
        match build_pair(&mut req, VERB_MV, corr, rest) {
            0 => return (i64::MIN, 0),
            n => n,
        }
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

// --- WC-C: midden's bus-stats WINDOW ------------------------------------------------------------
//
// midden owns a real compositor window and renders its OWN content into it — the kernel draws the
// border and the title strip and NOTHING else. That is the whole point of the WC seam: app content is
// app-drawn, chrome is kernel-drawn, and neither can forge the other. So the digits below are blitted
// here, at EL0, from a bitmap font this program carries in its own .rodata.
//
// It shows two rows of four hex digits: the live witness bitmask (row 0) and the number of witness legs
// passed (row 1) — the same numbers the serial verdict prints, so panel and log cross-check by eye.
//
// Geometry: 24x16 ARGB8888 = 1536 B, which `SYS_WIN_CREATE` negotiates down to a SINGLE page of the
// 64 KiB window slot. Tiny on purpose — a status readout, not a canvas, drawn 1:1 — and the compositor's
// integer upscale (~30x on a 1920-wide panel) is what makes it legible, so this program never needs to
// learn the panel geometry. EL0 sees only its own surface; it always has.
//
// SIZE DISCIPLINE. This whole feature had ~230 bytes of the 4 KiB code page left to live in, so every
// choice here is the cheap one: the font is packed 3x5 bits into ONE u16 per glyph (32 B of .rodata, not
// 80), the two rows are one loop over eight nibbles of a packed u32 rather than two calls, the glyphs are
// blitted 1:1 (no upscale loops — the compositor upscales), and the blit writes without a per-pixel bound
// test because the call-site geometry is a compile-time constant that keeps every glyph box inside the
// surface.
const WIN_W: usize = 24;
const WIN_H: usize = 16;
const WIN_STRIDE: usize = WIN_W * 4;
/// Surface offset from this program's own window base: `USER_REGION_SIZE + FB_INFO_SIZE` (boot.rs) —
/// window region slot 0, the same VA the compat `SYS_FB_MAP` used to return. Part of the window ABI.
const WIN_SURF_OFF: u64 = 0x5000;
/// Window interior — a shade off the desktop so the kernel's frame reads. All four bytes are EQUAL on
/// purpose: it makes the background clear a single `write_bytes`, i.e. one call to the `memset` this blob
/// already links (the `rep` frame buffer needs it), instead of a 2048-iteration store loop LLVM unrolls
/// into a few hundred bytes we do not have.
const WIN_BG: u8 = 0x1A;
const WIN_FG: u32 = 0xFFC9_A6E8; // the UnaOS lilac the vug seams use

/// The 16 hex digits as 3x5 bitmaps, packed row-major into one u16 each: bit `row * 3 + col`, col 0
/// leftmost, row 0 top. Bit 15 is unused.
static HEX_GLYPHS: [u16; 16] = [
    0b0_111_101_101_101_111, // 0
    0b0_111_010_010_110_010, // 1
    0b0_111_001_111_100_111, // 2
    0b0_111_100_111_100_111, // 3
    0b0_100_100_111_101_101, // 4
    0b0_111_100_111_001_111, // 5
    0b0_111_101_111_001_111, // 6
    0b0_100_100_100_100_111, // 7
    0b0_111_101_111_101_111, // 8
    0b0_111_100_111_101_111, // 9
    0b0_101_101_111_101_111, // A
    0b0_011_101_011_101_011, // b
    0b0_111_001_001_001_111, // C
    0b0_011_101_101_101_011, // d
    0b0_111_001_111_001_111, // E
    0b0_001_001_111_001_111, // F
];

/// This program's own window base VA. `adr` is PC-relative and byte-granular, so it is correct at
/// whatever address the kernel copied the flat blob to — the blob begins exactly at `_start`, which the
/// linker script forces to offset 0 of the EL0 window.
#[inline(always)]
fn window_base() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("adr {0}, _start", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

/// Blit one 3x5 glyph 1:1 at `(x0, y0)`. `#[inline(never)]` is load-bearing, not a style choice: inlined,
/// LLVM unrolls the eight-digit caller around this 15-bit nest and `win_paint` triples — which this blob's
/// 4 KiB code page cannot pay for. Emitted once, called eight times.
///
/// Nested row/col rather than one loop over the 15 bits: `b % 3` / `b / 3` would each compile to a
/// multiply-shift sequence. 1:1 pixels for the same reason — the compositor's integer upscale is what
/// makes the glyphs big on the panel, not this loop.
#[inline(never)]
unsafe fn draw_glyph(surf: *mut u8, x0: usize, y0: usize, bits: u16) {
    let mut ry = 0usize;
    while ry < 5 {
        let mut rx = 0usize;
        while rx < 3 {
            if bits >> (ry * 3 + rx) & 1 != 0 {
                let off = (y0 + ry) * WIN_STRIDE + (x0 + rx) * 4;
                unsafe { (surf.add(off) as *mut u32).write_volatile(WIN_FG) };
            }
            rx += 1;
        }
        ry += 1;
    }
}

/// Repaint the surface and present it. `stats` is the two rows packed as `(top << 16) | bottom`; a
/// negative `id` (create refused) makes this a no-op, so midden's bus witnesses never depend on the
/// compositor being able to give it a window.
#[inline(never)]
fn win_paint(id: i64, surf: *mut u8, stats: u32) {
    if id < 0 {
        return;
    }
    // Non-volatile on purpose (see WIN_BG): `svc` clobbers memory, so these stores cannot be sunk past
    // the present below, which is the only ordering this needs.
    unsafe { core::ptr::write_bytes(surf, WIN_BG, WIN_W * WIN_H * 4) };
    // Eight nibbles, most-significant first: n 0..4 -> row 0 (top), n 4..8 -> row 1 (bottom).
    let mut n = 0usize;
    while n < 8 {
        let nib = ((stats >> (28 - 4 * n)) & 0xF) as usize;
        // `get` rather than indexing: a bounds-checked index compiles a `core::panicking` call into a
        // blob that must contain no panic path at all. `nib` is masked, so the None arm is unreachable.
        let Some(&bits) = HEX_GLYPHS.get(nib) else {
            return;
        };
        let x0 = 1 + (n & 3) * 6;
        let y0 = 1 + (n >> 2) * 7;
        unsafe { draw_glyph(surf, x0, y0, bits) };
        n += 1;
    }
    let _ = svc(SYS_WIN_PRESENT, id as u64, 0, 0);
}

#[unsafe(no_mangle)]
extern "C" fn midden_main() -> ! {
    let mut w: u64 = 0;
    let mut rep = [0u8; FRAME_MAX];
    // WC-C: open midden's window FIRST, so the panel shows the stats readout for the whole run rather
    // than only at the end. Every `paint` below is a no-op if the create failed — the bus witnesses this
    // program exists to produce never depend on the compositor.
    let win = svc(SYS_WIN_CREATE, WIN_W as u64, WIN_H as u64, 0);
    let wsurf = (window_base() + WIN_SURF_OFF) as *mut u8;

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

    win_paint(win, wsurf, (w as u32) << 16 | w.count_ones());

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

    // WC-C: final repaint — the panel readout ends on the SAME bitmask the serial verdict reports, so
    // the two witnesses are cross-checkable by eye (row 0 = the bitmask in hex, row 1 = legs passed).
    win_paint(win, wsurf, (w as u32) << 16 | w.count_ones());

    let _ = svc(SYS_REPORT, w, 0, 0);
    let _ = svc(SYS_EXIT, MIDDEN_EXIT_STATUS, 0, 0);
    loop {}
}

/// No panic path exists by construction (raw-pointer helpers, no fmt); this never runs.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
