// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (R85: "the window will need to be raised, might as well do it now")
//!
//! WINDOW2 (rmbp-ledger B361) — `tests window`, the metal witness that the ring-3 window is 64 MiB and that
//! the three consumers it was raised for now fit:
//!
//! * `alloc48m`  — `/apps/BIG.ELF` (crates/user-big): SYS_SBRK of 48 MiB, a word written and read back on every
//!   page, handed back (exit bit 0x10). RING3WIN's window could not hold it.
//! * `fixed20m`  — a synthesized static Linux ET_EXEC whose image spans 0x400000..0x1801000 (a 20 MiB `.bss`):
//!   the old 16 MiB `IMAGE_LIMIT` refused it at parse. It writes the last and the first `.bss` word and
//!   `exit_group(42)`s. Needs `linuxabi`.
//! * `lld_pie`   — `/apps/LLD.LNX` (when staged) parses as a STATIC-PIE (ET_DYN, no PT_INTERP) placed at
//!   `linuxabi::elf::PIE_BASE` — R85's "relinked as PIE". `tests selfbuild5` runs it.
//! * `kdf_ring3` — `/apps/HOLOCRON.ELF --kdf-selftest` derives Argon2id at Holocron's metal parameters (48 MiB,
//!   t 3, p 4) IN RING 3; the kernel recomputes the key through SYS_KDF's body (`keyring::kdf`) and compares
//!   23 bits. SYS_KDF (66) stays as a kernel service; Holocron no longer calls it for a ring it made.
//!
//! Wire: `:: WINDOW2: bytes=67108864 image_limit=67108864 alloc48m=ok fixed20m=ok lld_pie=ok kdf_ring3=ok -> PASS ::`.
//! A leg whose fixture is not staged says `skip(<why>)`; any `fail(…)` = FAIL; all ok = PASS; else SKIP.

use alloc::string::String;
use alloc::vec::Vec;

fn read_app(path: &str) -> Option<Vec<u8>> {
    let mt = crate::shell::vfs_mount_table();
    let full = crate::shell::vfs_path(path);
    let st = mt.stat(&full).ok()?;
    if st.size == 0 || st.size as usize > crate::arch::syscall::user_image_cap() {
        return None;
    }
    mt.read(&full, 0, st.size as usize).ok().filter(|b| b.len() as u64 == st.size)
}

fn run(name: &'static str, bytes: &[u8], ms: u64, argv: &[&str]) -> Result<u32, String> {
    use crate::arch::syscall::RunOutcome;
    match crate::arch::syscall::run_user_image_argv(name, bytes, ms, argv) {
        Ok((RunOutcome::Exited(s), _)) => Ok(s as u32),
        Ok((RunOutcome::Faulted, _)) => Err(String::from("fault")),
        Ok((RunOutcome::Timeout, _)) => Err(String::from("timeout")),
        Err(e) => Err(String::from(e)),
    }
}

fn alloc48m() -> String {
    let Some(b) = read_app("/apps/BIG.ELF") else { return String::from("skip(no-big-elf)") };
    match run("window2", &b, 10_000, &["big"]) {
        Ok(s) => {
            serial_println!("[window2] big status={:#x} bits={:#x}", s, s & 0xFF);
            if s & 0x10 != 0 { String::from("ok") } else { alloc::format!("fail(bits={:#x})", s & 0xFF) }
        }
        Err(e) => alloc::format!("fail({})", e),
    }
}

fn kdf_ring3() -> String {
    let Some(b) = read_app("/apps/HOLOCRON.ELF") else { return String::from("skip(no-holocron-elf)") };
    #[cfg(feature = "lumen")]
    {
        let t0 = crate::arch::ticks();
        let st = match run("window2", &b, 60_000, &["holocron", "--kdf-selftest"]) {
            Ok(s) => s,
            Err(e) => return alloc::format!("fail({})", e),
        };
        let ring3_ms = crate::arch::ticks().saturating_sub(t0);
        if st & 0xFF != 1 {
            return alloc::format!("fail(flags={:#x})", st & 0xFF);
        }
        let mut req = [0u8; una_abi::KDF_HDR_LEN + una_abi::KDF_PW_MAX + una_abi::KDF_SALT_MAX];
        let Some(n) = una_abi::kdf_request(una_abi::WINDOW2_KDF_M_KIB, una_abi::WINDOW2_KDF_T, una_abi::WINDOW2_KDF_P, una_abi::WINDOW2_KAT_PW, &una_abi::WINDOW2_KAT_SALT, &mut req) else {
            return String::from("fail(request)");
        };
        let t1 = crate::arch::ticks();
        let key = match crate::keyring::kdf(&req[..n]) {
            Ok(k) => k,
            Err(e) => return alloc::format!("fail(kernel-kdf={})", e),
        };
        let kernel_ms = crate::arch::ticks().saturating_sub(t1);
        let want = u32::from_le_bytes([key[0], key[1], key[2], key[3]]) & 0x7FFF_FF00;
        serial_println!("[window2] kdf ring3_status={:#x} want={:#x} ring3_ms={} kernel_ms={}", st, want, ring3_ms, kernel_ms);
        if st & 0x7FFF_FF00 == want { String::from("ok") } else { String::from("fail(key-mismatch)") }
    }
    #[cfg(not(feature = "lumen"))]
    {
        let _ = b;
        String::from("skip(no-lumen: SYS_KDF's body is not built)")
    }
}

/// The 20 MiB fixed-address Linux image: ehdr + 2 phdrs + code, one page of file.
#[cfg(feature = "linuxabi")]
fn fixed20m_image() -> Vec<u8> {
    const BASE: u64 = 0x40_0000;
    const BSS: u64 = BASE + 0x1000;
    const BSS_LEN: u64 = 20 << 20;
    let code: [u8; 45] = [
        0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, // mov rax, <last bss qword>
        0x48, 0xC7, 0x00, 0x2A, 0, 0, 0, // mov qword [rax], 42
        0x48, 0x8B, 0x38, // mov rdi, [rax]
        0x48, 0xBB, 0, 0, 0, 0, 0, 0, 0, 0, // mov rbx, <first bss qword>
        0x48, 0x89, 0x3B, // mov [rbx], rdi
        0x48, 0x8B, 0x3B, // mov rdi, [rbx]
        0xB8, 0xE7, 0, 0, 0, // mov eax, 231 (exit_group)
        0x0F, 0x05, // syscall
        0xEB, 0xFE, // jmp $
    ];
    let mut c = code;
    c[2..10].copy_from_slice(&(BSS + BSS_LEN - 8).to_le_bytes());
    c[22..30].copy_from_slice(&BSS.to_le_bytes());
    let code_off: u64 = 64 + 2 * 56;
    let mut b = alloc::vec![0u8; 4096];
    let w16 = |b: &mut [u8], o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let w32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let w64 = |b: &mut [u8], o: usize, v: u64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
    b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    b[4] = 2; // ELF64
    b[5] = 1; // little-endian
    b[6] = 1; // EV_CURRENT
    w16(&mut b, 16, 2); // ET_EXEC
    w16(&mut b, 18, 62); // EM_X86_64
    w32(&mut b, 20, 1);
    w64(&mut b, 24, BASE + code_off); // e_entry
    w64(&mut b, 32, 64); // e_phoff
    w16(&mut b, 52, 64); // e_ehsize
    w16(&mut b, 54, 56); // e_phentsize
    w16(&mut b, 56, 2); // e_phnum
    // PT_LOAD R+X: the header page with the code.
    let p = 64;
    w32(&mut b, p, 1);
    w32(&mut b, p + 4, 5);
    w64(&mut b, p + 8, 0);
    w64(&mut b, p + 16, BASE);
    w64(&mut b, p + 24, BASE);
    w64(&mut b, p + 32, code_off + c.len() as u64);
    w64(&mut b, p + 40, code_off + c.len() as u64);
    w64(&mut b, p + 48, 0x1000);
    // PT_LOAD RW: 20 MiB of .bss, no file bytes.
    let p = 64 + 56;
    w32(&mut b, p, 1);
    w32(&mut b, p + 4, 6);
    w64(&mut b, p + 8, 0);
    w64(&mut b, p + 16, BSS);
    w64(&mut b, p + 24, BSS);
    w64(&mut b, p + 32, 0);
    w64(&mut b, p + 40, BSS_LEN);
    w64(&mut b, p + 48, 0x1000);
    b[code_off as usize..code_off as usize + c.len()].copy_from_slice(&c);
    b
}

fn fixed20m() -> String {
    #[cfg(feature = "linuxabi")]
    {
        use crate::arch::linuxabi::elf;
        let img = fixed20m_image();
        let plan = match elf::parse(&img) {
            Ok(p) => p,
            Err(e) => return alloc::format!("fail(parse: {})", e),
        };
        let span = plan.segs.iter().map(|s| s.vaddr + s.memsz).max().unwrap_or(0) - 0x40_0000;
        let path = alloc::format!("{}window2-fixed20m.lnx", crate::arch::linuxabi::sys::home_prefix());
        let full = crate::shell::vfs_path(&path);
        let mt = crate::shell::vfs_mount_table();
        let _ = mt.unlink(&full, crate::fs::vfs::KERNEL_PRINCIPAL);
        if mt.create(&full, crate::fs::vfs::NodeKind::File, crate::fs::vfs::KERNEL_PRINCIPAL).is_err()
            || mt.write(&full, 0, &img, crate::fs::vfs::KERNEL_PRINCIPAL).map(|n| n != img.len()).unwrap_or(true)
        {
            return String::from("skip(no-writable-home)");
        }
        let r = crate::arch::linuxabi::run_path(&path, &[path.as_str()], 10_000, false, &mut |l| serial_println!("[window2] fixed20m: {}", l));
        let _ = mt.unlink(&full, crate::fs::vfs::KERNEL_PRINCIPAL);
        match r {
            Ok(rep) => {
                serial_println!("[window2] fixed20m span={} exit={} ms={}", span, rep.exit, rep.ms);
                if rep.exit == "42" { String::from("ok") } else { alloc::format!("fail(exit={})", rep.exit) }
            }
            Err(e) => alloc::format!("fail(load: {})", e),
        }
    }
    #[cfg(not(feature = "linuxabi"))]
    {
        String::from("skip(no-linuxabi)")
    }
}

fn lld_pie() -> String {
    #[cfg(feature = "linuxabi")]
    {
        use crate::arch::linuxabi::elf;
        let mt = crate::shell::vfs_mount_table();
        let full = crate::shell::vfs_path("/apps/LLD.LNX");
        let Ok(st) = mt.stat(&full) else { return String::from("skip(lld-not-staged)") };
        let head = match mt.read(&full, 0, (st.size as usize).min(65536)) {
            Ok(h) => h,
            Err(_) => return String::from("fail(read)"),
        };
        match elf::parse_sized(&head, st.size) {
            Ok(p) if elf::is_pie(&p) => {
                serial_println!("[window2] lld size={} entry={:#x} segs={} base={:#x}", st.size, p.entry, p.segs.len(), elf::PIE_BASE);
                String::from("ok")
            }
            Ok(_) => String::from("fail(not-pie: an ET_EXEC lld overlays the kernel's low identity map)"),
            Err(e) => alloc::format!("fail({})", e),
        }
    }
    #[cfg(not(feature = "linuxabi"))]
    {
        String::from("skip(no-linuxabi)")
    }
}

/// `tests window`.
pub fn selftest() {
    let bytes = una_abi::USER_WINDOW_BYTES;
    #[cfg(feature = "linuxabi")]
    let image_limit = alloc::format!("{}", crate::arch::linuxabi::elf::IMAGE_LIMIT);
    #[cfg(not(feature = "linuxabi"))]
    let image_limit = String::from("off");
    let pt0 = crate::arch::memory::xwin_pt_live();
    let a = alloc48m();
    let k = kdf_ring3();
    let f = fixed20m();
    let l = lld_pie();
    // The heap PTs the two ring-3 runs wired must be back once their slots are released (bounded wait).
    let dl = crate::arch::ticks() + 2_000;
    while crate::arch::memory::xwin_pt_live() != pt0 && crate::arch::ticks() < dl {
        crate::arch::sched::yield_now();
    }
    let pt1 = crate::arch::memory::xwin_pt_live();
    serial_println!("[window2] xwin_pt_live before={} after={}", pt0, pt1);
    let legs = [&a, &f, &l, &k];
    let verdict = if legs.iter().any(|s| s.starts_with("fail")) || pt1 != pt0 {
        "FAIL"
    } else if legs.iter().all(|s| s.as_str() == "ok") {
        "PASS"
    } else {
        "SKIP"
    };
    serial_println!(
        ":: WINDOW2: bytes={} image_limit={} alloc48m={} fixed20m={} lld_pie={} kdf_ring3={} -> {} ::",
        bytes, image_limit, a, f, l, k, verdict
    );
}
