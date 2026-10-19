//! CHARTER: Principia — kernel-by-ruling (ABIMAGE B683, BP-bar-8: an update stages a second image; the loader switches only after a user restart)
//!
//! ABIMAGE (rmbp-ledger B683) — the kernel's half of the A/B image: STAGE, the user's SHUTDOWN record, PROMOTE. The
//! records and the loader's decision are `install::abslot` (one file, compiled by both the kernel and the loader);
//! the mechanism is `docs/dev/OS/10_INSTALL/ab-image.md`. Every ESP write here is the UPDATER's, through
//! `bootfat::fat_unlock(Writer::Updater, …)` (BP-storage-4, R99), and is said on the wire.
//!
//! * [`stage_image`]`(path)`: `STAGE.REC` removed first (a torn stage is never staged), the image copied to
//!   `KERNEL.NEW` in 1 MiB chunks and re-read against its CRC-32, `SHUTDOWN.REC := why=staged` (a stale `why=user` from
//!   an earlier restart never arms THIS stage), and `STAGE.REC` written LAST. `[update] staged kelf=<max_vaddr> …`.
//! * [`note_user_shutdown`]: from `power::prelude` — the one funnel of the user's Restart / Shut Down (shell verbs and
//!   the crystal menu; the panic screen's reset bypasses it on purpose) — `SHUTDOWN.REC := why=user`, only when a
//!   stage is present (no write on an ordinary restart).
//! * [`desktop_built`]`(pass)`: at the boot's DESKTOPBUILT verdict. A PASS on a TRIAL boot (stage present, `why=trial`,
//!   which only the loader writes, only when it jumped to `KERNEL.NEW`) promotes: rename ELF->OLD, NEW->ELF, unlink
//!   OLD, remove `STAGE.REC`, `SHUTDOWN.REC := why=promoted`. The loader opens the other slot when its pick is
//!   missing, so a promotion torn between the two renames still boots the image that just passed.
//!
//! * [`verb`]: `stage <path>` (STAGEVERB B690) — the operator's word over [`stage_image`]; it prints what was staged
//!   or why it was refused, and the switch stays the loader's after the user's own restart.
//!
//! `tests abimage`: stage a copy of the running kernel, assert the record, assert `abslot::decide`'s four cases,
//! then remove the stage it made; then the same through the `stage` verb (its printed line and the record it left).
//!
//! KELFSTAGE (rmbp-ledger B720, BP-boot-17): `KELF.SHA` follows the slots. The stage writes the slot-B row (sha256 of
//! the RE-READ `KERNEL.NEW`, `kelfsha::kelf_of`'s `kelf=`) before `STAGE.REC`; the promotion, AFTER both renames,
//! respells it as the slot-A row (`kelfsha::promoted`) so `hash=ok` survives an update; a failed rename leaves the old
//! row; the unstage drops the B row. The promotion is decided by [`should_promote`]: a trial that the running kernel
//! is slot B (the loader's verdict) — which is also how the loader's A->B hash fallback (`why=trial`, no stage) ends.
//! `tests kelfstage` (tests.rs tail).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::abslot::{self, Slot, Why, Write};
use crate::fs::bootfat::{self, Writer};
use crate::fs::vfs::{MountTable, NodeKind, KERNEL_PRINCIPAL};

// KELFSTAGE (B720): the KELF.SHA record — the very file the UEFI loader checks and the builder writes, compiled here by
// path (`crates/bootloader/src/kelfsha.rs`), so the stage's row and the loader's check cannot disagree.
#[allow(dead_code)]
#[path = "../../../bootloader/src/kelfsha.rs"]
pub(crate) mod kelfsha;

const CHUNK: usize = 1 << 20;

fn esp(name: &str) -> String {
    format!("/boot/{}", name)
}

fn exists(mt: &MountTable, name: &str) -> bool {
    mt.stat(&esp(name)).is_ok()
}

/// One record off the ESP (`None` = absent or unreadable — the loader reads it the same way).
fn read_rec(mt: &MountTable, name: &str) -> Option<Vec<u8>> {
    let p = esp(name);
    let st = mt.stat(&p).ok()?;
    mt.read(&p, 0, (st.size as usize).min(abslot::REC_MAX)).ok()
}

/// Replace a whole small file (create, or truncate to 0) and write `bytes`.
fn put(mt: &MountTable, name: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let p = esp(name);
    if mt.stat(&p).is_ok() {
        mt.truncate(&p, 0, KERNEL_PRINCIPAL).map_err(|_| "truncate")?;
    } else {
        mt.create(&p, NodeKind::File, KERNEL_PRINCIPAL).map_err(|_| "create")?;
    }
    match mt.write(&p, 0, bytes, KERNEL_PRINCIPAL) {
        Ok(n) if n == bytes.len() => Ok(()),
        _ => Err("write"),
    }
}

fn drop_file(mt: &MountTable, name: &str) -> Result<(), &'static str> {
    if !exists(mt, name) {
        return Ok(());
    }
    mt.unlink(&esp(name), KERNEL_PRINCIPAL).map_err(|_| "unlink")
}

/// The loader's `KELF max=`: the highest `vaddr + memsz` over the image's PT_LOAD segments (`elf_core`, the one reader).
fn kelf_of(head: &[u8]) -> Option<u64> {
    let mut segs = [elf_core::Seg::default(); elf_core::MAX_SEGS];
    let (n, _) = elf_core::read_phdrs(head, &mut segs)?;
    segs[..n].iter().map(|s| s.vaddr.checked_add(s.memsz)).try_fold(0u64, |m, e| e.map(|e| m.max(e)))
}

/// What a stage wrote.
#[derive(Clone, Copy, Debug)]
pub struct Staged {
    pub kelf: u64,
    pub bytes: u64,
    pub crc: u32,
    /// KELFSTAGE: the sha256 of `KERNEL.NEW` as re-read, the slot-B row's.
    pub sha: [u8; 32],
}

/// KELFSTAGE: `KELF.SHA`'s bytes (at most `kelfsha::MAX`); `None` when absent.
fn read_kelf(mt: &MountTable) -> Option<Vec<u8>> {
    let p = esp(kelfsha::NAME);
    let st = mt.stat(&p).ok()?;
    mt.read(&p, 0, (st.size as usize).min(kelfsha::MAX)).ok()
}

/// KELFSTAGE: `KELF.SHA` as the namespace holds it (`tests kelfstage`).
pub fn kelf_record() -> Option<Vec<u8>> {
    read_kelf(&crate::shell::vfs_mount_table())
}

/// KELFSTAGE: replace `slot`'s row (`None` drops it). No record and nothing to add = nothing written.
fn kelf_put_row(mt: &MountTable, slot: u8, row: Option<&[u8]>) -> Result<(), &'static str> {
    let old = read_kelf(mt);
    if old.is_none() && row.is_none() {
        return Ok(());
    }
    let mut out = [0u8; kelfsha::MAX];
    let n = kelfsha::with_row(old.as_deref().unwrap_or(&[]), slot, row, &mut out).ok_or("kelfsha-full")?;
    put(mt, kelfsha::NAME, &out[..n]).map_err(|_| "kelfsha-write")
}

/// KELFSTAGE: the record after KERNEL.NEW became KERNEL.ELF — the B row respelled as A; with no B row the A row would
/// describe the outgoing image, so the record is removed (`no-record` = `unverified`, never a lockout). The word says
/// which.
fn kelf_promote(mt: &MountTable) -> &'static str {
    let Some(old) = read_kelf(mt) else { return "none" };
    let mut out = [0u8; kelfsha::MAX];
    match kelfsha::promoted(&old, &mut out) {
        Some(n) => match put(mt, kelfsha::NAME, &out[..n]) {
            Ok(()) => "row-A",
            Err(_) => "write-FAILED",
        },
        None => match drop_file(mt, kelfsha::NAME) {
            Ok(()) => "dropped-no-B-row",
            Err(_) => "drop-FAILED",
        },
    }
}

/// KELFSTAGE: THE PROMOTION DECISION. `booted` is the slot the loader jumped to (`BootInfo::kelf`, 0 = A, 1 = B;
/// `None` for a loader older than KELFHASH). With a verdict: a trial (`why=trial`) promotes iff the kernel running is
/// slot B — the user-restart trial and the loader's A->B hash fallback alike, and never a B->A fallback (the torn
/// KERNEL.NEW stays where it is). Without one: ABIMAGE's rule, staged and a trial.
pub fn should_promote(staged: bool, trial: bool, booted: Option<u8>) -> bool {
    trial && match booted {
        Some(slot) => slot == 1,
        None => staged,
    }
}

/// Stage `path` (any readable path in the namespace) as `KERNEL.NEW`. The running image is untouched; the switch is
/// the loader's, after the user's own restart.
pub fn stage_image(path: &str) -> Result<Staged, &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|_| "source-missing")?;
    if st.kind != NodeKind::File || st.size == 0 {
        return Err("source-not-a-file");
    }
    let head = mt.read(path, 0, (st.size as usize).min(64 << 10)).map_err(|_| "source-read")?;
    let kelf = kelf_of(&head).ok_or("not-a-kernel-elf")?;
    let _unlock = bootfat::fat_unlock(Writer::Updater, "KERNEL.NEW + KELF.SHA + STAGE.REC");
    if !bootfat::allows() {
        return Err("esp-locked");
    }
    heal(&mt)?;
    drop_file(&mt, abslot::STAGE)?;
    drop_file(&mt, abslot::OLD)?;
    let new = esp(abslot::NEW);
    if mt.stat(&new).is_ok() {
        mt.truncate(&new, 0, KERNEL_PRINCIPAL).map_err(|_| "new-truncate")?;
    } else {
        mt.create(&new, NodeKind::File, KERNEL_PRINCIPAL).map_err(|_| "new-create")?;
    }
    let mut crc = crate::hash::Crc32::new();
    let mut off = 0u64;
    while off < st.size {
        let want = ((st.size - off) as usize).min(CHUNK);
        let buf = mt.read(path, off, want).map_err(|_| "source-read")?;
        if buf.len() != want {
            return Err("source-short");
        }
        crc.update(&buf);
        match mt.write(&new, off, &buf, KERNEL_PRINCIPAL) {
            Ok(n) if n == want => {}
            _ => return Err("new-write"),
        }
        off += want as u64;
    }
    let crc = crc.finish();
    let mut back = crate::hash::Crc32::new();
    let mut sha = crypto_core::sha2::Sha256::new();
    let mut off = 0u64;
    while off < st.size {
        let want = ((st.size - off) as usize).min(CHUNK);
        let buf = mt.read(&new, off, want).map_err(|_| "new-reread")?;
        back.update(&buf);
        sha.update(&buf);
        off += want as u64;
    }
    if back.finish() != crc || mt.stat(&new).map(|s| s.size).ok() != Some(st.size) {
        return Err("new-verify");
    }
    let sha = sha.finalize();
    put(&mt, abslot::SHUTDOWN, abslot::REC_STAGED)?;
    // KELFSTAGE (B720): the slot-B row, BEFORE STAGE.REC (a stage whose row is not written is no stage).
    let mut row = [0u8; kelfsha::ROW_MAX];
    let rn = kelfsha::row(&sha, abslot::NEW, kelfsha::SLOT_B, kelfsha::kelf_of(&head).unwrap_or(kelf), &mut row);
    kelf_put_row(&mt, kelfsha::SLOT_B, Some(&row[..rn]))?;
    let mut rec = [0u8; abslot::REC_MAX];
    let n = abslot::stage_record(kelf, st.size, crc, &mut rec);
    put(&mt, abslot::STAGE, &rec[..n])?;
    let short = kelfsha::short(&sha);
    serial_println!("[update] staged kelf={:#x} bytes={} crc={:#010x} row=B sha={} from={} (loader switches after a user restart)", kelf, st.size, crc, core::str::from_utf8(&short).unwrap_or("?"), path);
    Ok(Staged { kelf, bytes: st.size, crc, sha })
}

/// `stage <path>` (STAGEVERB B690, BP-bar-8): stage a kernel ELF as `KERNEL.NEW` through [`stage_image`]. The shell's
/// arm (`shell.rs`) and `tests abimage`'s verb leg call this one fn; `out` is the console line.
pub fn verb(out: &mut dyn FnMut(&str), args: &[&str]) {
    let [path] = args else {
        out("usage: stage <path>   (copies a kernel ELF to /boot/KERNEL.NEW; your next Restart boots it, and the current image comes back if it never reaches the desktop)");
        return;
    };
    match stage_image(path) {
        Ok(s) => out(&format!("stage {}: staged kelf={:#x} bytes={} crc={:#010x} — Restart when you choose; nothing restarts on its own", path, s.kelf, s.bytes, s.crc)),
        Err(w) => out(&format!("stage {}: refused ({}) — no new image armed", path, w)),
    }
}

/// Remove a stage (`tests abimage`'s cleanup; also an operator's "never mind").
pub fn unstage() -> Result<(), &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let _unlock = bootfat::fat_unlock(Writer::Updater, "unstage");
    if !bootfat::allows() {
        return Err("esp-locked");
    }
    heal(&mt)?;
    drop_file(&mt, abslot::STAGE)?;
    drop_file(&mt, abslot::NEW)?;
    kelf_put_row(&mt, kelfsha::SLOT_B, None)?; // KELFSTAGE: no KERNEL.NEW, no B row
    drop_file(&mt, abslot::SHUTDOWN)
}

/// `power::prelude`: the user asked for this restart / shut down. Arms the staged image (only when one is staged).
pub fn note_user_shutdown(action: &str) {
    let mt = crate::shell::vfs_mount_table();
    let stage = read_rec(&mt, abslot::STAGE);
    if !abslot::staged(stage.as_deref()) {
        return;
    }
    let _unlock = bootfat::fat_unlock(Writer::Updater, "SHUTDOWN.REC why=user");
    let r = if bootfat::allows() { put(&mt, abslot::SHUTDOWN, abslot::REC_USER) } else { Err("esp-locked") };
    match r {
        Ok(()) => serial_println!("[update] shutdown record why=user act={} (the staged image boots next)", action),
        Err(e) => serial_println!("[update] shutdown record why=user act={} -> FAILED {} (the current image boots next)", action, e),
    }
}

/// The boot's DESKTOPBUILT verdict. A PASS on the loader's TRIAL boot of `KERNEL.NEW` promotes it to `KERNEL.ELF`.
pub fn desktop_built(pass: bool) {
    let mt = crate::shell::vfs_mount_table();
    let stage = read_rec(&mt, abslot::STAGE);
    let staged = abslot::staged(stage.as_deref());
    let shut = read_rec(&mt, abslot::SHUTDOWN);
    let trial = shut.as_deref().and_then(|r| abslot::field(r, b"why")) == Some(&b"trial"[..]);
    let booted = crate::tests::kelf_booted_slot();
    if !should_promote(staged, trial, booted) {
        if trial && booted == Some(0) {
            // KELFSTAGE: the loader fell back from a torn KERNEL.NEW to A on its own trial: never promote the torn file.
            serial_println!("[update] trial ran slot=A (the loader fell back from KERNEL.NEW): not promoted");
        }
        return; // no trial, or this boot is the current image: nothing to promote
    }
    let kelf = stage.as_deref().and_then(|r| abslot::field(r, b"kelf")).and_then(abslot::parse_hex).unwrap_or(0);
    let via = if staged { "stage" } else { "kelf-fallback" };
    if !pass {
        serial_println!("[update] trial kelf={:#x} via={} desktopbuilt=FAIL: not promoted (the loader falls back next boot)", kelf, via);
        return;
    }
    match promote(&mt) {
        Ok(k) => serial_println!("[update] promoted NEW -> ELF kelf={:#x} via={} kelfsha={} (desktopbuilt PASS on the trial boot)", kelf, via, k),
        Err(e) => serial_println!("[update] promote kelf={:#x} via={} -> FAILED {} (the loader falls back next boot)", kelf, via, e),
    }
}

/// Finish a promotion torn between its two renames (no `KERNEL.ELF`, `KERNEL.NEW` present — the image the loader
/// booted in that state) BEFORE anything overwrites or removes `KERNEL.NEW`, which is then the only kernel.
fn heal(mt: &MountTable) -> Result<(), &'static str> {
    if exists(mt, abslot::ELF) || !exists(mt, abslot::NEW) {
        return Ok(());
    }
    mt.rename(&esp(abslot::NEW), &esp(abslot::ELF), KERNEL_PRINCIPAL).map_err(|_| "heal-rename")?;
    let k = kelf_promote(mt); // KELFSTAGE: the record follows the rename
    serial_println!("[update] healed a torn promotion: KERNEL.NEW -> KERNEL.ELF kelfsha={}", k);
    drop_file(mt, abslot::OLD)
}

/// Ok carries the KELF.SHA rewrite's word (KELFSTAGE): the record is rewritten only AFTER both renames held.
fn promote(mt: &MountTable) -> Result<&'static str, &'static str> {
    let _unlock = bootfat::fat_unlock(Writer::Updater, "promote KERNEL.NEW -> KERNEL.ELF");
    if !bootfat::allows() {
        return Err("esp-locked");
    }
    if !exists(mt, abslot::NEW) {
        return Err("new-missing");
    }
    drop_file(mt, abslot::OLD)?;
    mt.rename(&esp(abslot::ELF), &esp(abslot::OLD), KERNEL_PRINCIPAL).map_err(|_| "rename-elf-old")?;
    mt.rename(&esp(abslot::NEW), &esp(abslot::ELF), KERNEL_PRINCIPAL).map_err(|_| "rename-new-elf")?;
    let k = kelf_promote(mt);
    drop_file(mt, abslot::OLD)?;
    drop_file(mt, abslot::STAGE)?;
    put(mt, abslot::SHUTDOWN, abslot::REC_PROMOTED).map(|()| k)
}

/// `tests abimage` — stage a copy of the running kernel, assert the record, assert the loader's decision function's
/// four cases, remove the stage; stage it again through [`verb`], assert its line and record, remove that stage.
pub fn selftest() {
    let s = abslot::REC_STAGED;
    let cases: [(&str, Option<&[u8]>, Option<&[u8]>, Slot, Why, Write); 4] = [
        ("unstaged", None, Some(abslot::REC_USER), Slot::Elf, Why::Normal, Write::None),
        ("user", Some(&b"stage=1 kelf=0x1"[..]), Some(abslot::REC_USER), Slot::New, Why::UserRestart, Write::Trial),
        ("trial", Some(&b"stage=1 kelf=0x1"[..]), Some(abslot::REC_TRIAL), Slot::Elf, Why::Fallback, Write::Fallback),
        ("crash", Some(&b"stage=1 kelf=0x1"[..]), Some(s), Slot::Elf, Why::Normal, Write::None),
    ];
    let mut dec_ok = 0u32;
    for (name, st, sh, slot, why, w) in cases {
        let d = abslot::decide(st, sh);
        let ok = d.slot == slot && d.why == why && d.write == w;
        dec_ok += ok as u32;
        serial_println!("[update] abimage case={} slot={} why={} -> {}", name, d.slot.word(), d.why.word(), if ok { "ok" } else { "WRONG" });
    }
    let src = esp(abslot::ELF);
    let (stage_w, rec_ok, kelf) = match stage_image(&src) {
        Ok(st) => {
            let mt = crate::shell::vfs_mount_table();
            let rec = read_rec(&mt, abslot::STAGE);
            let shut = read_rec(&mt, abslot::SHUTDOWN);
            let r = rec.as_deref();
            let ok = abslot::staged(r)
                && r.and_then(|r| abslot::field(r, b"kelf")).and_then(abslot::parse_hex) == Some(st.kelf)
                && r.and_then(|r| abslot::field(r, b"crc")).and_then(abslot::parse_hex) == Some(st.crc as u64)
                && shut.as_deref() == Some(abslot::REC_STAGED)
                && abslot::decide(r, shut.as_deref()).slot == Slot::Elf;
            (String::from("staged"), ok, st.kelf)
        }
        Err(e) => (format!("refused:{}", e), false, 0),
    };
    let cleaned = unstage().is_ok();
    // STAGEVERB (B690): the operator's verb on a copy of the same image — its line names the kelf the record holds.
    let mut said = String::new();
    verb(&mut |s: &str| said.push_str(s), &[src.as_str()]);
    let verb_ok = {
        let mt = crate::shell::vfs_mount_table();
        let rec = read_rec(&mt, abslot::STAGE);
        let r = rec.as_deref();
        let k = r.and_then(|r| abslot::field(r, b"kelf")).and_then(abslot::parse_hex);
        abslot::staged(r) && k.is_some_and(|k| said.contains(&format!("staged kelf={:#x} ", k)))
    };
    serial_println!("[update] abimage verb=stage said=\"{}\" -> {}", said, if verb_ok { "ok" } else { "WRONG" });
    let cleaned = unstage().is_ok() && cleaned;
    let pass = dec_ok == 4 && rec_ok && verb_ok && cleaned;
    serial_println!(
        ":: ABIMAGE: decide={}/4 stage={} kelf={:#x} record={} verb={} unstaged={} -> {} ::",
        dec_ok, stage_w, kelf, if rec_ok { "ok" } else { "bad" }, if verb_ok { "ok" } else { "bad" }, cleaned as u8, if pass { "PASS" } else { "FAIL" }
    );
}
