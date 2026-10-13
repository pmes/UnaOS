//! CHARTER: Kernel — driver (KEPLERUCODE rmbp-ledger B637, R119: the GK107's FECS/GPCCS context-switch firmware read from the ESP under a SHA-256 pin and started, behind `tests kgr`)
//!
//! KEPLERUCODE — KEPLERGR4 (B627) stopped at FECS reading HALTED with no context-switch firmware resident; R119 rules
//! route (c): NVIDIA's `fuc409c`/`fuc409d`/`fuc41ac`/`fuc41ad` staged from the BUNKER, digest-pinned in-tree like the
//! b43 set (FWPIN B455), never in the repository. Declared from `kepler_fifo.rs` as `kepler_fifo::ucode` (`#[path]`,
//! gpu/mod.rs untouched) under exactly `kgr`'s gate, and called only from `kgr::tests_kgr` (R80: nothing at boot).
//!
//! THE SET: arroyo's KEPLER-FW block (`UNAOS_KEPLER_FW_PATH`) puts the four files on the ESP at `FW/KEPLER/`. This
//! file reads them from the PROGRAM-SOURCE FAT (the boot card's own ESP — never a removable volume, R95), SHA-256s
//! each (`crypto_core::sha2`, the kernel's one SHA-256) and admits the set only when all four equal their rows in
//! `unaos/firmware/kepler.pins`. A missing row refuses `reason=unpinned` (the digest printed, for the seat to pin
//! against the KEPLER-FW build line); a different digest `reason=pin-mismatch`; a file missing `-> absent`.
//!
//! THE START — nouveau's EXTERNAL-firmware path, which is the path these four files take (`gr->firmware` is set by
//! `gf100_gr_load` only when all four load, gr/gf100.c:2596-2611; `gf100_gr_init_ctxctl` then takes
//! `gf100_gr_init_ctxctl_ext`, :1883-1893). Facts (R95 §2; no nouveau text copied):
//! * PMC 0x260 = 0 around the loads, 1 after (`nvkm_mc_unk260`, gf100.c:1759 / :1783; the write is 0x000260,
//!   subdev/mc/gf100.c:62-65, gk104's mc uses the same hook, mc/gk104.c:59).
//! * FECS falcon at 0x409000, GPCCS at 0x41a000 (gf100.c:2641-2649); both load DMEM then IMEM, data then code
//!   (`gf100_gr_init_fw`, gf100.c:1700-1705), FECS first (:1764, :1771).
//! * DMEM: falcon+0x1c0 = start | BIT(24) (auto-increment write), each LE word to falcon+0x1c4 (falcon/v1.c:68-79).
//!   IMEM: falcon+0x180 = start | BIT(24), a tag to falcon+0x188 before every 64-word (256-byte) block, starting
//!   at tag 0, each word to falcon+0x184, then zero-pad to a 64-word boundary (v1.c:29-65; gf100's falcon uses the
//!   v1 loaders, gf100.c:2163-2166). Tail bytes of a non-multiple-of-4 file are masked (v1.c:51-59, :82-89).
//! * Limits: falcon+0x108 bits 8:0 = IMEM size in 256-byte units, bits 17:9 = DMEM size (base.c:273-275) — a file
//!   larger than its memory is refused before any write.
//! * Start: 0x409800 = 0, 0x41a10c = 0, 0x40910c = 0, then GPCCS, then FECS (gf100.c:1786-1791); a falcon starts
//!   by writing 2 to +0x130 when CPUCTL (+0x100) bit 6 is set, else 2 to +0x100 (v1.c:93-101). Ready = 0x409800
//!   bit 0 within 2000 ms (gf100.c:1793-1797).
//! * Then the ctxsw method interface (0x409500 data, 0x409504 method, 0x409800 answer): the watchdog (method 0x21,
//!   data 0x7fffffff, gf100.c:973-980, called at :1799) and the main context image size (method 0x10, the answer
//!   non-zero in 0x409800 within 2000 ms, gf100.c:957-970, called at :1802). 0x409504 is touched ONLY after the
//!   firmware reported ready — without firmware its first touch wedges the unit (kepler_fifo.rs KFBIND notes).
//!
//! NOT HERE (the arc's STOP, keplerucode.md §3): the brief's "0x409100 = 2, wait 0x409800 bit 31" is nouveau's
//! INTERNAL-ucode path (`gf100_gr_init_ctxctl_int`, :1832-1880, which also loads the register lists with
//! `gf100_gr_init_csdata`) — nouveau's own falcon code, not these files; it is not used. The golden context
//! (`gf100_grctx_generate`, ctxgf100.c:1436-1558, with gk104's lists in ctxgk104.c) needs `gk104_gr_init` first and
//! is the next rung; until it exists `kgr` gates the SET_OBJECT with `reason=no-golden-context`.

use alloc::string::String;
use alloc::vec::Vec;

use super::super::kepler::{fecs_read, fecs_write, mmio_write};

/// Whether this tree generates a golden context image for the GR channel (`gf100_grctx_generate`, ctxgf100.c:1436-1558).
/// It does not yet (keplerucode.md §3): `kgr` gates the SET_OBJECT on this, so KEPLERGR4's push never runs against an
/// instance block whose GR context pointer (0x210) is 0. The generator's arc flips it.
pub const GOLDEN_CONTEXT: bool = false;

/// `(role, file)` — the pin roles and the names nouveau requests behind NvGrUseFW (gr/gf100.c:2603-2606).
pub const SET: [(&str, &str); 4] = [
    ("fecs_code", "fuc409c"),
    ("fecs_data", "fuc409d"),
    ("gpccs_code", "fuc41ac"),
    ("gpccs_data", "fuc41ad"),
];
/// Where arroyo's KEPLER-FW block puts them on the ESP.
const DIR0: &str = "FW";
const DIR1: &str = "KEPLER";
/// No falcon memory on this part is larger (base.c:273-275's fields top out at 0x1ff * 256 = 127.75 KiB); a bigger
/// file is refused before the heap is committed.
const MAX_FILE: usize = 128 * 1024;

const PINS: &str = include_str!("../../../../../firmware/kepler.pins");

const FECS: usize = 0x0040_9000; // gf100.c:2641-2642
const GPCCS: usize = 0x0041_a000; // gf100.c:2648-2649
const PMC_UNK260: usize = 0x0000_0260; // mc/gf100.c:62-65
const F_CPUCTL: usize = 0x100; // v1.c:95
const F_CPUCTL_ALIAS: usize = 0x130; // v1.c:97-98
const F_CPUCTL_ALIAS_EN: u32 = 1 << 6; // v1.c:97
const F_HWCFG: usize = 0x108; // base.c:273
const F_IMEMC: usize = 0x180; // v1.c:39
const F_IMEMD: usize = 0x184; // v1.c:44
const F_IMEMT: usize = 0x188; // v1.c:43
const F_DMEMC: usize = 0x1c0; // v1.c:76
const F_DMEMD: usize = 0x1c4; // v1.c:78
const F_AUTOINC: u32 = 1 << 24; // v1.c:38, :76
const F_UNK10C: usize = 0x10c; // gf100.c:1787-1788 (0x41a10c, 0x40910c)
const FECS_MAILBOX_ANSWER: usize = 0x0040_9800; // gf100.c:1786, :1794
const FECS_MTHD_DATA: usize = 0x0040_9500; // gf100.c:962
const FECS_MTHD_CMD: usize = 0x0040_9504; // gf100.c:963
const MTHD_WATCHDOG: u32 = 0x21; // gf100.c:979
const WATCHDOG_TIMEOUT: u32 = 0x7fff_ffff; // gf100.c:1799
const MTHD_IMAGE_SIZE: u32 = 0x10; // gf100.c:963
const READY_BUDGET_US: u64 = 2_000_000; // nvkm_msec(device, 2000, …) gf100.c:1793, :964

fn nib(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The pinned SHA-256 for `role`, or `None` (no row or a malformed one — both refuse as `unpinned`). The b43.pins
/// grammar (wifi/firmware.rs `pin_for`): `<role> <64 hex>`, `#` lines and blanks ignored.
fn pin_for(role: &str) -> Option<[u8; 32]> {
    for line in PINS.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(r), Some(h), None) = (it.next(), it.next(), it.next()) else { continue };
        if r != role || h.len() != 64 {
            continue;
        }
        let b = h.as_bytes();
        let mut out = [0u8; 32];
        let mut ok = true;
        for i in 0..32 {
            match (nib(b[2 * i]), nib(b[2 * i + 1])) {
                (Some(hi), Some(lo)) => out[i] = (hi << 4) | lo,
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(out);
        }
    }
    None
}

/// How many roles carry a row — printed so a `-> absent` with `pins=0/4` reads as "nothing pinned yet".
pub fn pinned_rows() -> usize {
    SET.iter().filter(|(r, _)| pin_for(r).is_some()).count()
}

fn hex32(d: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{:02x}", b));
    }
    s
}

/// The four images, admitted (every digest equals its pin).
pub struct Set {
    pub fecs_code: Vec<u8>,
    pub fecs_data: Vec<u8>,
    pub gpccs_code: Vec<u8>,
    pub gpccs_data: Vec<u8>,
}

fn name_is(e: &crate::fs::fat::DirEntry, want: &str) -> bool {
    e.name().trim_end_matches('.').eq_ignore_ascii_case(want)
}

/// Read `FW/KEPLER/<file>` x4 from the program-source FAT, digest each, compare against the pins. One line per file
/// (`[kgr] ucode <file> bytes= sha256= -> pinned|unpinned|pin-mismatch|absent|too-large|unreadable`), then `Ok` only
/// when all four are pinned. `Err` carries the set verdict word (`absent`, `unpinned`, `pin-mismatch`, `no-esp`, …).
pub fn stage() -> Result<Set, String> {
    let fs = match crate::fs::fat::mount_program_source() {
        Ok(f) => f,
        Err(_) => return Err(String::from("no-esp")),
    };
    let Ok(root) = fs.read_root() else { return Err(String::from("no-esp")) };
    let dir = |ents: &[crate::fs::fat::DirEntry], n: &str| ents.iter().find(|e| e.is_dir && name_is(e, n)).map(|e| e.first_cluster());
    let Some(c0) = dir(&root, DIR0) else {
        serial_println!("[kgr] ucode dir=/{}/{}/ -> absent (no /{}/ on the ESP; arroyo's KEPLER-FW block stages it from UNAOS_KEPLER_FW_PATH)", DIR0, DIR1, DIR0);
        return Err(String::from("absent"));
    };
    let Some(d0) = (if c0 == 0 { None } else { fs.read_dir(c0).ok() }) else { return Err(String::from("absent")) };
    let Some(c1) = dir(&d0, DIR1) else {
        serial_println!("[kgr] ucode dir=/{}/{}/ -> absent", DIR0, DIR1);
        return Err(String::from("absent"));
    };
    let Some(ents) = (if c1 == 0 { None } else { fs.read_dir(c1).ok() }) else { return Err(String::from("absent")) };

    let mut imgs: [Vec<u8>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    // Worst verdict wins: absent < unpinned < too-large/unreadable < pin-mismatch.
    let mut worst: (u8, &'static str) = (0, "pinned");
    for (i, (role, file)) in SET.iter().enumerate() {
        let Some(de) = ents.iter().find(|e| !e.is_dir && name_is(e, file)) else {
            serial_println!("[kgr] ucode {} role={} -> absent", file, role);
            if worst.0 < 1 {
                worst = (1, "absent");
            }
            continue;
        };
        if de.size as usize > MAX_FILE || de.size < 4 {
            serial_println!("[kgr] ucode {} role={} bytes={} -> refused reason=size (4..={})", file, role, de.size, MAX_FILE);
            if worst.0 < 3 {
                worst = (3, "size");
            }
            continue;
        }
        let mut buf = Vec::new();
        if fs.read_file(de, &mut buf, MAX_FILE).is_err() || buf.len() != de.size as usize {
            serial_println!("[kgr] ucode {} role={} bytes={} read={} -> refused reason=unreadable", file, role, de.size, buf.len());
            if worst.0 < 3 {
                worst = (3, "unreadable");
            }
            continue;
        }
        let sha = crypto_core::sha2::sha256(&buf);
        let v = match pin_for(role) {
            None => {
                if worst.0 < 2 {
                    worst = (2, "unpinned");
                }
                "refused reason=unpinned (pin it from the KEPLER-FW build line into unaos/firmware/kepler.pins)"
            }
            Some(p) if p != sha => {
                worst = (4, "pin-mismatch");
                "refused reason=pin-mismatch"
            }
            Some(_) => "pinned",
        };
        serial_println!("[kgr] ucode {} role={} bytes={} sha256={} -> {}", file, role, buf.len(), hex32(&sha), v);
        if v == "pinned" {
            imgs[i] = buf;
        }
    }
    if worst.0 != 0 {
        return Err(String::from(worst.1));
    }
    let [fecs_code, fecs_data, gpccs_code, gpccs_data] = imgs;
    Ok(Set { fecs_code, fecs_data, gpccs_code, gpccs_data })
}

#[inline]
fn word(b: &[u8], i: usize) -> u32 {
    // LE dwords (nouveau reads the blob as `u32 *` on a little-endian host, v1.c:44); a short tail is zero-filled,
    // which is v1.c:51-59's mask.
    let mut w = [0u8; 4];
    for k in 0..4 {
        if let Some(x) = b.get(i * 4 + k) {
            w[k] = *x;
        }
    }
    u32::from_le_bytes(w)
}

fn load_dmem(bar0: usize, base: usize, data: &[u8]) {
    fecs_write(bar0, base + F_DMEMC, F_AUTOINC); // start 0 | BIT(24), v1.c:76
    for i in 0..data.len().div_ceil(4) {
        fecs_write(bar0, base + F_DMEMD, word(data, i));
    }
}

fn load_imem(bar0: usize, base: usize, code: &[u8]) {
    fecs_write(bar0, base + F_IMEMC, F_AUTOINC); // start 0 | BIT(24), v1.c:38-39
    let n = code.len().div_ceil(4);
    let mut tag = 0u32;
    let mut i = 0usize;
    while i < n {
        if i & 0x3f == 0 {
            fecs_write(bar0, base + F_IMEMT, tag); // a tag every 256 bytes, v1.c:42-43
            tag += 1;
        }
        fecs_write(bar0, base + F_IMEMD, word(code, i));
        i += 1;
    }
    while i & 0x3f != 0 {
        fecs_write(bar0, base + F_IMEMD, 0); // pad to 0x40 words, v1.c:63-64
        i += 1;
    }
}

fn start_falcon(bar0: usize, base: usize) {
    let c = fecs_read(bar0, base + F_CPUCTL);
    if c & F_CPUCTL_ALIAS_EN != 0 {
        fecs_write(bar0, base + F_CPUCTL_ALIAS, 2);
    } else {
        fecs_write(bar0, base + F_CPUCTL, 2);
    }
}

/// (imem bytes, dmem bytes) from HWCFG, or `None` when the word is poison/absent/zero.
fn limits(bar0: usize, base: usize) -> Option<(usize, usize, u32)> {
    let h = fecs_read(bar0, base + F_HWCFG);
    if super::super::kepler::classify_fecs_word(h) != "VALUE" {
        return None;
    }
    Some((((h & 0x1ff) << 8) as usize, ((h & 0x3fe00) >> 1) as usize, h))
}

fn hz() -> u64 {
    let h = crate::arch::apic::tsc_hz();
    if h == 0 { 1_250_000_000 } else { h }
}

/// Poll `f` until it returns `Some` or `budget_us` passes. Returns the value and the microseconds it took.
fn poll<T>(budget_us: u64, mut f: impl FnMut() -> Option<T>) -> Option<(T, u64)> {
    let (t0, lim) = (crate::arch::now_cycles(), hz().saturating_mul(budget_us) / 1_000_000);
    loop {
        let dt = crate::arch::now_cycles().saturating_sub(t0);
        if let Some(v) = f() {
            return Some((v, dt.saturating_mul(1_000_000) / hz()));
        }
        if dt >= lim {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// The outcome `kgr` carries into KGR4: `Ok(ctx_size)` = the firmware reported ready and answered the image-size
/// method; `Err(word)` = what stopped it (`absent`, `unpinned`, `pin-mismatch`, `size`, `hwcfg`, `too-large`,
/// `ready-wall`, `size-wall`, …). Nothing is written to the GPU unless `stage` admitted all four.
pub fn load_and_start(bar0: usize) -> Result<u32, String> {
    let set = match stage() {
        Ok(s) => s,
        Err(why) => {
            serial_println!("[kgr] ucode set=FW/KEPLER pins={}/4 -> {} (nothing written; FECS stays as found)", pinned_rows(), why);
            return Err(why);
        }
    };
    let (Some(fl), Some(gl)) = (limits(bar0, FECS), limits(bar0, GPCCS)) else {
        serial_println!(
            "[kgr] ucode hwcfg fecs={:08X} gpccs={:08X} -> refused reason=hwcfg (unreadable; nothing written)",
            fecs_read(bar0, FECS + F_HWCFG), fecs_read(bar0, GPCCS + F_HWCFG)
        );
        return Err(String::from("hwcfg"));
    };
    let fits = set.fecs_code.len() <= fl.0 && set.fecs_data.len() <= fl.1 && set.gpccs_code.len() <= gl.0 && set.gpccs_data.len() <= gl.1;
    serial_println!(
        "[kgr] ucode limits fecs hwcfg={:08X} imem={} dmem={} gpccs hwcfg={:08X} imem={} dmem={} set {}/{}/{}/{} -> {}",
        fl.2, fl.0, fl.1, gl.2, gl.0, gl.1,
        set.fecs_code.len(), set.fecs_data.len(), set.gpccs_code.len(), set.gpccs_data.len(),
        if fits { "fits" } else { "too-large" }
    );
    if !fits {
        return Err(String::from("too-large"));
    }
    let pre = (fecs_read(bar0, FECS + F_CPUCTL), fecs_read(bar0, GPCCS + F_CPUCTL));
    unsafe { mmio_write(bar0, PMC_UNK260, 0) }; // gf100.c:1759
    load_dmem(bar0, FECS, &set.fecs_data);
    load_imem(bar0, FECS, &set.fecs_code);
    load_dmem(bar0, GPCCS, &set.gpccs_data);
    load_imem(bar0, GPCCS, &set.gpccs_code);
    unsafe { mmio_write(bar0, PMC_UNK260, 1) }; // gf100.c:1783
    fecs_write(bar0, FECS_MAILBOX_ANSWER, 0); // gf100.c:1786
    fecs_write(bar0, GPCCS + F_UNK10C, 0); // gf100.c:1787
    fecs_write(bar0, FECS + F_UNK10C, 0); // gf100.c:1788
    start_falcon(bar0, GPCCS); // gf100.c:1790
    start_falcon(bar0, FECS); // gf100.c:1791
    let ready = poll(READY_BUDGET_US, || {
        let m = fecs_read(bar0, FECS_MAILBOX_ANSWER);
        if m & 1 != 0 { Some(m) } else { None }
    });
    let post = (fecs_read(bar0, FECS + F_CPUCTL), fecs_read(bar0, GPCCS + F_CPUCTL));
    let Some((mb, us)) = ready else {
        serial_println!(
            "[kgr] ucode start fecs cpuctl={:08X}->{:08X} gpccs cpuctl={:08X}->{:08X} mailbox={:08X} budget_us={} -> ready-wall (0x409504 untouched)",
            pre.0, post.0, pre.1, post.1, fecs_read(bar0, FECS_MAILBOX_ANSWER), READY_BUDGET_US
        );
        return Err(String::from("ready-wall"));
    };
    // The firmware is up: the method interface is now the firmware's (gf100.c:1799, :1802).
    fecs_write(bar0, FECS_MAILBOX_ANSWER, 0);
    fecs_write(bar0, FECS_MTHD_DATA, WATCHDOG_TIMEOUT);
    fecs_write(bar0, FECS_MTHD_CMD, MTHD_WATCHDOG);
    fecs_write(bar0, FECS_MAILBOX_ANSWER, 0);
    fecs_write(bar0, FECS_MTHD_DATA, 0);
    fecs_write(bar0, FECS_MTHD_CMD, MTHD_IMAGE_SIZE);
    let size = poll(READY_BUDGET_US, || {
        let s = fecs_read(bar0, FECS_MAILBOX_ANSWER);
        if s != 0 { Some(s) } else { None }
    });
    match size {
        Some((sz, sus)) => {
            serial_println!(
                "[kgr] ucode start fecs cpuctl={:08X}->{:08X} gpccs cpuctl={:08X}->{:08X} mailbox={:08X} ready_us={} watchdog={:08X} ctx_size={:#x} size_us={} -> running",
                pre.0, post.0, pre.1, post.1, mb, us, WATCHDOG_TIMEOUT, sz, sus
            );
            Ok(sz)
        }
        None => {
            serial_println!(
                "[kgr] ucode start fecs cpuctl={:08X}->{:08X} gpccs cpuctl={:08X}->{:08X} mailbox={:08X} ready_us={} ctx_size=- -> size-wall",
                pre.0, post.0, pre.1, post.1, mb, us
            );
            Err(String::from("size-wall"))
        }
    }
}
