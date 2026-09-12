//! GA10B-PROBE5 — the FIRMWARE LOADER for rung 5a of the GA10B ladder (`ga10bprobe5`, DEFAULT OFF;
//! armed by any non-empty `UNAOS_GA10B_PROBE5`). Design: docs/dev/OS/08_VIDEO/GA10B-RUNG5-BRIEF.md
//! §5.1 (P6/P7/P8), §5.2.1 (how the bytes get from the card into the window), §5.6, §5.7. Ledger A63.
//!
//! WHAT IT DOES. Reads NVIDIA's `acr-gsp` triple — three ORDINARY FILES on the boot volume, under
//! `GA10B/` at the FAT root beside `GA10B/LICENCE.txt` (R52: staged UNMODIFIED on the media, loaded as
//! DATA, never linked, never embedded, never committed) — through the VFS (`MountTable::stat` /
//! `MountTable::read` over `fs::fat`, which parses VFAT long names, PI-FS-3, so the vendor's names stay as
//! shipped), verifies each file's SIZE and SHA-256 against the constants below (A61: digests are FACTS,
//! not the blob), and places them in the rung-4 DMA window at 256-byte-aligned offsets — text at +0,
//! data at the next 256-byte boundary, manifest at the next — so a BCR address encoded `pa >> 8`
//! (rung-5 brief §1.2) is lossless for every section. The SHA-256 is computed over the bytes IN THE
//! WINDOW after a `dsb sy` (P6: "digest what landed, not what was read").
//!
//! WHAT IT NEVER DOES. Not one MMIO access: this module has no GPU register, no BAR0 pointer and no
//! write primitive. Its only side effects are VFS reads, a memcpy into a window the CALLER hands it
//! and serial lines. Every refusal is therefore a ZERO-MMIO refusal by construction (brief §5.6):
//! `-> REFUSED reason=<absent|size|digest|window> name=<file>`. It does not decrypt, inspect, patch or
//! rename a vendor byte (§5.5): read, place, digest — nothing else.
//!
//! WHERE IT RUNS. On the Orin, from `ga10b_ignite::ga10bprobe5_run` (rung 5a, `ga10bprobe5a`), which
//! hands it the rung-4 Normal-NC window and `/boot/GA10B`. On QEMU virt (`UNAOS_GA10B_PROBE5=1
//! ./arroyo test-arm`, `witness` armed), from `fixture_service` on the storage-ready loop of
//! `kernel_main`, against a FAT image `arroyo` builds carrying three SYNTHETIC files of the SAME names
//! and sizes (our own bytes, their own digests, in the `witness`-gated table below — never a vendor
//! byte) in `GA10B/`, and a twin `GA10BBAD/` with one byte of the data section flipped: the first pass
//! must print `-> LOADED`, the second `-> REFUSED reason=digest` (the go-red proof, from a file, every
//! run).
//!
//! WITNESS FAMILY `[ga10bfw]` — 9 bytes bracketed, over the 8-byte LLVM immediate-encode floor, so
//! `LC_ALL=C grep -a -o -F '[ga10bfw]' kernel.elf` certifies the armed artifact (LAWS §5).

use alloc::vec::Vec;

/// One expected section: the file's shipped name, its byte size and its SHA-256.
pub struct Expect {
    pub name: &'static str,
    pub size: u32,
    pub sha: [u8; 32],
}

/// The three roles, in placement order (text at +0, data next, manifest next), and the BCR register
/// each is bound for (rung-5 brief §4.1.2 — an INFERENCE from the names, marked as one there).
pub const ROLE: [&str; 3] = ["fmccode", "fmcdata", "pkcparam"];

const fn nib(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

/// 64 hex characters -> 32 bytes, at compile time (a typo in a digest is a REFUSED boot, never a panic).
const fn hx(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nib(b[2 * i]) << 4) | nib(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// The VENDOR triple — names, sizes and SHA-256 digests measured on the bench from
/// `nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb` (ledger A61, 2026-09-12; MANIFEST beside the
/// staged files). These are facts about the files, not the files: a boot that reads bytes with any other
/// digest REFUSES.
pub const ACR_GSP: [Expect; 3] = [
    Expect {
        name: "acr-gsp.text.encrypt.bin.prod",
        size: 28_672,
        sha: hx("ceeae9ec72ef80f24c70473f9fff02588d1ab56a30d18b688469c331370ce12a"),
    },
    Expect {
        name: "acr-gsp.data.encrypt.bin.prod",
        size: 9_472,
        sha: hx("e5cd6c6af29929e2944c3a63e4074b6c94d5c2345103566253450d6fa0cb2492"),
    },
    Expect {
        name: "acr-gsp.manifest.encrypt.bin.out.bin.prod",
        size: 2_048,
        sha: hx("03f3ecc6c1fa2be3a1f5c1ca4d13442a17c9f69958eb3fba492990626dee2320"),
    },
];

/// The SYNTHETIC triple the QEMU fixture loads — same names, same sizes, OUR bytes: each file is the
/// line `UNAOS GA10B5 SYNTHETIC <role> section (not a vendor byte)` repeated to the vendor file's size
/// (`arroyo test_aarch64` generates them with `yes | head -c`). Their digests were taken once with
/// `sha256sum` (2026-09-12) and are what the fixture expects; the `GA10BBAD/` twin flips byte 100 of the
/// data section, so its digest is NOT this one and the second pass must REFUSE.
#[cfg(feature = "witness")]
pub const SYNTH: [Expect; 3] = [
    Expect {
        name: "acr-gsp.text.encrypt.bin.prod",
        size: 28_672,
        sha: hx("63d690b55399bcdc9a81fbd7d36b9a9473b27f92b9426fec5ef2b0d2d409dd6a"),
    },
    Expect {
        name: "acr-gsp.data.encrypt.bin.prod",
        size: 9_472,
        sha: hx("3c71684b4c105fb9cb1eace202ae188bc74b6e880e80ace55c13f6ea3efbf50e"),
    },
    Expect {
        name: "acr-gsp.manifest.encrypt.bin.out.bin.prod",
        size: 2_048,
        sha: hx("7d0107d83740ac717a28b310db3b0084edc557d68ecfe549377a2e8507b6a393"),
    },
];

/// Where one section landed: its offset inside the window, its physical address, its byte size.
#[derive(Clone, Copy)]
pub struct Placed {
    pub off: u64,
    pub pa: u64,
    pub size: u32,
}

/// The loader's verdict. `Loaded` carries the three placements in ROLE order and the window it used;
/// `Refused` carries the reason token (`absent` | `size` | `digest` | `window`) and the file it names.
pub enum Outcome {
    Loaded { placed: [Placed; 3], total: u64, window: u64, end: u64 },
    Refused { reason: &'static str, name: &'static str },
}

#[inline]
fn align256(n: u64) -> u64 {
    (n + 255) & !255
}

/// The bytes the three sections need in a window, each rounded up to the next 256-byte boundary (P8).
pub fn window_need(expect: &[Expect; 3]) -> u64 {
    align256(expect[0].size as u64) + align256(expect[1].size as u64) + align256(expect[2].size as u64)
}

/// A digest as 64 lowercase hex characters, for the wire.
pub struct Hex64([u8; 64]);
impl Hex64 {
    pub fn of(d: &[u8; 32]) -> Self {
        const T: &[u8; 16] = b"0123456789abcdef";
        let mut o = [0u8; 64];
        for (i, b) in d.iter().enumerate() {
            o[2 * i] = T[(b >> 4) as usize];
            o[2 * i + 1] = T[(b & 0xf) as usize];
        }
        Hex64(o)
    }
    pub fn as_str(&self) -> &str {
        // Only bytes from `T` above are ever written, so this is ASCII by construction.
        unsafe { core::str::from_utf8_unchecked(&self.0) }
    }
}

fn err_name(e: &crate::fs::vfs::VfsError) -> &'static str {
    use crate::fs::vfs::VfsError as E;
    match e {
        E::NoSuchVolume => "no-such-volume",
        E::NoSuchPath => "no-such-path",
        E::NotADirectory => "not-a-directory",
        E::IsADirectory => "is-a-directory",
        E::Denied => "denied",
        E::Unsupported => "unsupported",
        _ => "other",
    }
}

/// The loader. `dir` is the directory the three files live in (`/boot/GA10B` on the Orin); `expect` the
/// table (vendor or synthetic); `window`/`window_size` the buffer the sections are placed in — an address
/// this kernel can dereference (the rung-4 Normal-NC window is identity-mapped on the Orin; the fixture
/// hands a heap buffer). Prints the vocabulary lines, P8, one line per file, and exactly one `->` line.
/// Touches no GPU register.
pub fn load(mt: &crate::fs::vfs::MountTable, dir: &str, expect: &[Expect; 3], window: u64, window_size: u64) -> Outcome {
    let need = window_need(expect);
    serial_println!(
        "[ga10bfw] loader — dir={} window={:#x}..{:#x} (three ordinary files read through the VFS, sized and sha256-verified, placed 256-byte-aligned; ZERO MMIO on every path; vocabulary: LOADED | REFUSED reason=<absent|size|digest|window>)",
        dir, window, window + window_size
    );
    for (i, e) in expect.iter().enumerate() {
        serial_println!("[ga10bfw] expect role={} file={}/{} bytes={} sha256={}", ROLE[i], dir, e.name, e.size, Hex64::of(&e.sha).as_str());
    }
    // P8 — the fit, computed on the boot and printed BEFORE any read.
    serial_println!("[ga10bfw] window_need={:#x} window_have={:#x}", need, window_size);
    if window == 0 || need > window_size {
        serial_println!("[ga10bfw] -> REFUSED reason=window name=- (need={:#x} > have={:#x}: never truncate, never overlap — a wider seat in mmu_tegra is the fix, not a smaller image)", need, window_size);
        return Outcome::Refused { reason: "window", name: "-" };
    }
    let mut placed = [Placed { off: 0, pa: 0, size: 0 }; 3];
    let mut off: u64 = 0;
    for (i, e) in expect.iter().enumerate() {
        let path = alloc::format!("{}/{}", dir, e.name);
        serial_println!("[ga10bfw] about-to-read {} (VFS stat + read: a media read, not an MMIO access)", path);
        let st = match mt.stat(&path) {
            Ok(s) => s,
            Err(err) => {
                // F27: say whether the directory was there at all, and what it held.
                match mt.read_dir(dir) {
                    Ok(ents) => {
                        let mut names = alloc::string::String::new();
                        for d in ents.iter().take(16) {
                            names.push_str(&d.name);
                            names.push(' ');
                        }
                        serial_println!("[ga10bfw] dir {} listing: entries={} [{}]", dir, ents.len(), names.trim_end());
                    }
                    Err(e2) => serial_println!("[ga10bfw] dir {} listing: UNREADABLE err={} (not staged, or the volume is not the one this kernel booted from)", dir, err_name(&e2)),
                }
                serial_println!("[ga10bfw] -> REFUSED reason=absent name={} err={}", e.name, err_name(&err));
                return Outcome::Refused { reason: "absent", name: e.name };
            }
        };
        if st.size != e.size as u64 {
            serial_println!("[ga10bfw] file={} bytes={} expected={} -> REFUSED reason=size name={}", e.name, st.size, e.size, e.name);
            return Outcome::Refused { reason: "size", name: e.name };
        }
        let buf: Vec<u8> = match mt.read(&path, 0, e.size as usize) {
            Ok(v) => v,
            Err(err) => {
                serial_println!("[ga10bfw] -> REFUSED reason=absent name={} err={} (stat succeeded, read did not)", e.name, err_name(&err));
                return Outcome::Refused { reason: "absent", name: e.name };
            }
        };
        if buf.len() != e.size as usize {
            serial_println!("[ga10bfw] file={} bytes={} expected={} -> REFUSED reason=size name={} (short read)", e.name, buf.len(), e.size, e.name);
            return Outcome::Refused { reason: "size", name: e.name };
        }
        // P7 — the placement: the next 256-byte boundary; the fit was proven by P8 above.
        let pa = window + off;
        let dst = pa as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len());
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }
        // P6 — digest what LANDED, read back through the same mapping.
        let landed = unsafe { core::slice::from_raw_parts(pa as *const u8, buf.len()) };
        let mut h = crate::hash::Sha256::new();
        h.update(landed);
        let got = h.finalize();
        let ok = got == e.sha;
        serial_println!(
            "[ga10bfw] file={} bytes={} sha={} pa={:#x} off={:#x} role={} got={}",
            e.name, buf.len(), if ok { "match" } else { "MISMATCH" }, pa, off, ROLE[i], Hex64::of(&got).as_str()
        );
        if !ok {
            serial_println!("[ga10bfw] -> REFUSED reason=digest name={} (the bytes in the window are not the ledger row's file — a media fault or a staging miss, never a GPU result)", e.name);
            return Outcome::Refused { reason: "digest", name: e.name };
        }
        placed[i] = Placed { off, pa, size: e.size };
        off += align256(e.size as u64);
    }
    serial_println!(
        "[ga10bfw] -> LOADED total={} window={:#x}..{:#x} sections=3 digests_ok=3 fmccode_off={:#x} fmcdata_off={:#x} pkcparam_off={:#x}",
        off, window, window + off, placed[0].off, placed[1].off, placed[2].off
    );
    Outcome::Loaded { placed, total: off, window, end: window + off }
}

/// The QEMU fixture (`witness` only): once the USB stick is enumerated, build the mount table the shell
/// would build, find the volume (`/boot` when this kernel was found on it, else the first `/volumes/*`
/// — on virt the kernel boots from the vvfat ESP the kernel cannot see, so the stick is a data volume),
/// and run the loader twice: `GA10B/` must LOAD, `GA10BBAD/` must REFUSE with `digest`. One shot.
#[cfg(feature = "witness")]
pub fn fixture_service() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if !crate::fs::fat::source_present(crate::fs::fat::BlockSource::Usb) {
        return;
    }
    DONE.store(true, Ordering::Relaxed);
    let mt = crate::shell::vfs_mount_table();
    let prefixes = mt.prefixes();
    let mut root = alloc::string::String::new();
    if prefixes.iter().any(|p| *p == "/boot") {
        root.push_str("/boot");
    } else if let Some(p) = prefixes.iter().find(|p| p.starts_with("/volumes/")) {
        root.push_str(p);
    }
    let mut pl = alloc::string::String::new();
    for p in prefixes.iter() {
        pl.push_str(p);
        pl.push(' ');
    }
    serial_println!("[ga10bfw] fixture: mounts=[{}] root={} (witness; QEMU virt; the SYNTHETIC table, never a vendor byte)", pl.trim_end(), if root.is_empty() { "-" } else { root.as_str() });
    if root.is_empty() {
        serial_println!("[ga10bfw] fixture: no volume to read -> FAIL — the fixture image was not mounted");
        return;
    }
    // A heap window, 256-byte aligned, sized like a small seat: enough for the triple, so the P8 line
    // reads need <= have and the offsets on the wire are the ones the metal window would print.
    const WIN: usize = 64 * 1024;
    let mut backing: Vec<u8> = alloc::vec![0u8; WIN + 256];
    let base = ((backing.as_mut_ptr() as usize) + 255) & !255;
    let good = alloc::format!("{}/GA10B", root);
    let bad = alloc::format!("{}/GA10BBAD", root);
    let p1 = matches!(load(&mt, &good, &SYNTH, base as u64, WIN as u64), Outcome::Loaded { total: 0x9d00, .. });
    let p2 = matches!(load(&mt, &bad, &SYNTH, base as u64, WIN as u64), Outcome::Refused { reason: "digest", name: "acr-gsp.data.encrypt.bin.prod" });
    drop(backing);
    if p1 && p2 {
        serial_println!("[ga10bfw] fixture: pass1(GA10B)=LOADED pass2(GA10BBAD)=REFUSED-digest -> PASS ::");
    } else {
        serial_println!("[ga10bfw] fixture: pass1(GA10B)={} pass2(GA10BBAD)={} -> FAIL — see the loader lines above", if p1 { "LOADED" } else { "NOT-LOADED" }, if p2 { "REFUSED-digest" } else { "NOT-REFUSED-digest" });
    }
}
