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
//! UPDATEUI (rmbp-ledger B726, BP-bar-8) — the FACE of the above, for Settings > About > System image:
//! * [`status`]: the running image's KELF verdict (slot, hash, sha256 head; `tests::kelf_kept`), the staged image (its
//!   `STAGE.REC` plus the [`SIDECAR`] the stage wrote: the stick's build sha, when, from where), and a SET-ASIDE image
//!   (no stage, `SHUTDOWN.REC why=fallback` — the loader's write — and `KERNEL.NEW` still on the ESP).
//! * [`volumes`] / [`candidates_in`]: the removable volumes (`fs::removable::mounted_names`, `/volumes/<stick>`) whose
//!   root carries `KERNEL.ELF` (or an `UnaOS-*.img`) AND `SRC.SHA` (the stick's own build).
//! * [`stage_from`]`(vol)` / [`stage_from_in`]: the administrator gate the `stage` verb's arm asks, then
//!   [`stage_image`]'s own path; `[update] ui stage from=<vol> sha=<8> -> staged|refused:<why>`.
//! * [`SIDECAR`] `UPDATE.REC` (`src= sha= at= kelf= [fell=<boot n>]`): the kernel's note beside the loader's two
//!   records; written before `STAGE.REC`, read only when its `kelf=` matches; the loader never reads it. `fell=` is
//!   written once, at the first DESKTOPBUILT after the loader's fallback ([`desktop_built`] -> `settle`).
//! * [`remove_set_aside`]: the pane's Remove — [`unstage`], said on the wire.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::abslot::{self, Slot, Why, Write};
use crate::fs::bootfat::{self, Writer};
use crate::fs::vfs::{DirEnt, MountTable, NodeKind, Stat, VfsBackend, VfsError, KERNEL_PRINCIPAL};

/// UPDATEUI (B726): the kernel's note beside the loader's records — where a stage came from (never read by the loader).
pub const SIDECAR: &str = "UPDATE.REC";

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
    stage_core(&mt, path, path, "-")
}

/// [`stage_image`]'s body: the source is read through `src` (the namespace, or the fixture's RAM volume), every ESP
/// write goes through the namespace's `/boot`. `from` and `sha` land in the [`SIDECAR`] (the pane's "from where").
fn stage_core(src: &MountTable, path: &str, from: &str, sha: &str) -> Result<Staged, &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let st = src.stat(path).map_err(|_| "source-missing")?;
    if st.kind != NodeKind::File || st.size == 0 {
        return Err("source-not-a-file");
    }
    let head = src.read(path, 0, (st.size as usize).min(64 << 10)).map_err(|_| "source-read")?;
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
        let buf = src.read(path, off, want).map_err(|_| "source-read")?;
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
    let side = format!("src={} sha={} at={} kelf={:#x}\n", clean(from, 40), clean(sha, 8), crate::clock::unix_now().unwrap_or(0), kelf);
    put(&mt, SIDECAR, side.as_bytes())?; // UPDATEUI (B726): before STAGE.REC, so the record stays the stage's last write
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
    drop_file(&mt, SIDECAR)?;
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
    settle(&mt);
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
    drop_file(mt, SIDECAR)?; // UPDATEUI (B726)
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

// ── UPDATEUI (rmbp-ledger B726, BP-bar-8) — the System image row's model: status, the sticks, the stage, the set-aside ──

/// A record field's text (`-` when absent).
fn field_str(rec: Option<&[u8]>, key: &[u8]) -> String {
    rec.and_then(|r| abslot::field(r, key)).and_then(|v| core::str::from_utf8(v).ok()).map(String::from).unwrap_or_else(|| String::from("-"))
}

/// A decimal record field.
fn field_dec(rec: Option<&[u8]>, key: &[u8]) -> Option<u64> {
    let v = rec.and_then(|r| abslot::field(r, key))?;
    if v.is_empty() || v.len() > 20 || !v.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(v.iter().fold(0u64, |a, &c| a.wrapping_mul(10).wrapping_add((c - b'0') as u64)))
}

/// A record value: `[A-Za-z0-9._/-]` kept, anything else `_` (a space would end the field), at most `max` bytes.
fn clean(s: &str, max: usize) -> String {
    let mut o: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-') { c } else { '_' }).take(max).collect();
    if o.is_empty() {
        o.push('-');
    }
    o
}

/// The running image, as the loader handed it over (`[boot] kelf slot= hash= sha=`).
#[derive(Clone, Debug)]
pub struct Running {
    pub slot: &'static str,
    pub hash_ok: bool,
    /// The first 8 hex of the sha256 the loader computed over the bytes it jumped to.
    pub sha: String,
}

/// A staged image: `STAGE.REC` and, when its `kelf=` matches, the [`SIDECAR`]'s provenance.
#[derive(Clone, Debug)]
pub struct StageInfo {
    pub kelf: u64,
    pub bytes: u64,
    /// The volume (or path) it was staged from; `-` when the sidecar does not match.
    pub src: String,
    /// The stick's build sha (`SRC.SHA`'s `commit`, 8 hex); `-` from the verb.
    pub sha: String,
    /// Unix seconds of the stage; 0 = the clock was not set.
    pub at: u64,
}

/// What Settings > About > System image shows.
#[derive(Clone, Debug)]
pub struct Status {
    /// `None` = no verdict (a loader older than KELFHASH).
    pub running: Option<Running>,
    pub staged: Option<StageInfo>,
    /// The loader set the trial aside: `Some(boot n)` (0 = the boot is not numbered on this build).
    pub set_aside: Option<u64>,
}

/// The System image row's status (a few small ESP reads: the settings service pass, never a painter).
pub fn status() -> Status {
    let mt = crate::shell::vfs_mount_table();
    let running = crate::tests::kelf_kept().filter(|v| v.present()).map(|v| Running {
        slot: unaos_boot_info::KELF_SLOT_NAMES[v.slot as usize],
        hash_ok: v.hash == unaos_boot_info::KELF_HASH_OK,
        sha: format!("{:02x}{:02x}{:02x}{:02x}", v.sha[0], v.sha[1], v.sha[2], v.sha[3]),
    });
    let stage = read_rec(&mt, abslot::STAGE);
    let side = read_rec(&mt, SIDECAR);
    let shut = read_rec(&mt, abslot::SHUTDOWN);
    let kelf_of_rec = |r: Option<&[u8]>| r.and_then(|r| abslot::field(r, b"kelf")).and_then(abslot::parse_hex);
    let staged = abslot::staged(stage.as_deref()).then(|| {
        let kelf = kelf_of_rec(stage.as_deref()).unwrap_or(0);
        let mine = kelf_of_rec(side.as_deref()) == Some(kelf);
        let s = if mine { side.as_deref() } else { None };
        StageInfo { kelf, bytes: field_dec(stage.as_deref(), b"bytes").unwrap_or(0), src: field_str(s, b"src"), sha: field_str(s, b"sha"), at: field_dec(s, b"at").unwrap_or(0) }
    });
    let fell = staged.is_none() && shut.as_deref().and_then(|r| abslot::field(r, b"why")) == Some(&b"fallback"[..]) && exists(&mt, abslot::NEW);
    let set_aside = fell.then(|| field_dec(side.as_deref(), b"fell").unwrap_or(0));
    Status { running, staged, set_aside }
}

/// This boot's number (SELFDIAG's), 0 when the build does not number its boots.
fn boot_n() -> u64 {
    #[cfg(feature = "selfdiag")]
    {
        crate::bootwit::state().1
    }
    #[cfg(not(feature = "selfdiag"))]
    {
        0
    }
}

/// At the boot's DESKTOPBUILT verdict: the first boot after the loader's fallback notes which boot it was (`fell=` in
/// the [`SIDECAR`], once) — the row then says "set aside (boot <n>)" on every later boot. `Some(n)` when it wrote.
fn settle(mt: &MountTable) -> Option<u64> {
    let stage = read_rec(mt, abslot::STAGE);
    let shut = read_rec(mt, abslot::SHUTDOWN);
    if abslot::staged(stage.as_deref()) || shut.as_deref().and_then(|r| abslot::field(r, b"why")) != Some(&b"fallback"[..]) || !exists(mt, abslot::NEW) {
        return None;
    }
    let side = read_rec(mt, SIDECAR);
    if side.as_deref().and_then(|r| abslot::field(r, b"fell")).is_some() {
        return None;
    }
    let n = boot_n();
    let base = side.as_deref().map(|r| String::from(core::str::from_utf8(r).unwrap_or("").trim_end())).filter(|s| !s.is_empty()).unwrap_or_else(|| String::from("src=- sha=- at=0 kelf=0x0"));
    let line = format!("{} fell={}\n", base, n);
    let _unlock = bootfat::fat_unlock(Writer::Updater, "UPDATE.REC fell=");
    let r = if bootfat::allows() { put(mt, SIDECAR, line.as_bytes()) } else { Err("esp-locked") };
    let kelf = side.as_deref().and_then(|r| abslot::field(r, b"kelf")).and_then(abslot::parse_hex).unwrap_or(0);
    match r {
        Ok(()) => serial_println!("[update] set aside kelf={:#x} boot={} (the trial never reached the desktop; Settings > About offers Remove)", kelf, n),
        Err(e) => serial_println!("[update] set aside kelf={:#x} boot={} -> note FAILED {}", kelf, n, e),
    }
    r.ok().map(|_| n)
}

/// A removable volume carrying a stageable image and its build stamp.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The leaf under `/volumes`.
    pub vol: String,
    /// The image's full path.
    pub image: String,
    /// The stick's build sha (8 hex): `SRC.SHA`'s `commit` line, else the head of its digest line.
    pub sha: String,
}

/// `SRC.SHA` (selfhost's stamp: line 1 `<64 hex>  SRC.TGZ`, then `commit <sha>`): its 8-hex build sha.
fn src_sha(text: &[u8]) -> Option<String> {
    let t = core::str::from_utf8(text).ok()?;
    let mut lines = t.lines();
    let digest = lines.next()?.split_whitespace().next()?;
    if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let commit = lines.find_map(|l| {
        let mut it = l.split_whitespace();
        match (it.next(), it.next()) {
            (Some("commit"), Some(v)) if v.len() >= 8 && v.bytes().all(|c| c.is_ascii_hexdigit()) => Some(v),
            _ => None,
        }
    });
    Some(String::from(&commit.unwrap_or(digest)[..8]).to_ascii_lowercase())
}

/// One volume's verdict: its image (`KERNEL.ELF`, else an `UnaOS-*.img`) and its `SRC.SHA`.
fn probe(src: &MountTable, vol: &str) -> Result<Candidate, &'static str> {
    let root = format!("/volumes/{}", vol);
    let rows = src.read_dir(&root).map_err(|_| "volume-unreadable")?;
    let file = |e: &&DirEnt| e.kind == NodeKind::File;
    let image = rows.iter().filter(file).find(|e| e.name.eq_ignore_ascii_case(abslot::ELF)).or_else(|| {
        rows.iter().filter(file).find(|e| {
            let n = e.name.to_ascii_lowercase();
            n.starts_with("unaos-") && n.ends_with(".img")
        })
    });
    let image = image.ok_or("no-image")?;
    let sha = rows.iter().filter(file).find(|e| e.name.eq_ignore_ascii_case("SRC.SHA")).ok_or("no-src-sha")?;
    let text = src.read(&format!("{}/{}", root, sha.name), 0, 512).map_err(|_| "src-sha-unreadable")?;
    let sha = src_sha(&text).ok_or("src-sha-bad")?;
    Ok(Candidate { vol: String::from(vol), image: format!("{}/{}", root, image.name), sha })
}

/// The volumes among `vols` (leaves under `/volumes` in `src`) that carry a stageable image and its `SRC.SHA`.
pub fn candidates_in(src: &MountTable, vols: &[String]) -> Vec<Candidate> {
    vols.iter().filter_map(|v| probe(src, v).ok()).collect()
}

/// The pane's list: the removable volumes mounted now that carry a stageable image and its `SRC.SHA`.
pub fn volumes() -> Vec<Candidate> {
    let mt = crate::shell::vfs_mount_table();
    candidates_in(&mt, &crate::fs::removable::mounted_names())
}

/// The pane's stage: `stage_from_in` over the namespace.
pub fn stage_from(vol: &str) -> Result<Staged, &'static str> {
    let mt = crate::shell::vfs_mount_table();
    stage_from_in(&mt, vol)
}

/// Stage `/volumes/<vol>`'s image through `src`: the administrator gate the `stage` verb's arm asks (R100), the
/// volume's probe (an image and its `SRC.SHA`), then [`stage_image`]'s own path. One wire line either way.
pub fn stage_from_in(src: &MountTable, vol: &str) -> Result<Staged, &'static str> {
    let mut sha = String::from("-");
    let r = (|| {
        #[cfg(feature = "login")]
        crate::fs::users::admin_authority("stage")?;
        let c = probe(src, vol)?;
        sha = c.sha.clone();
        stage_core(src, &c.image, &c.vol, &c.sha)
    })();
    match &r {
        Ok(_) => serial_println!("[update] ui stage from={} sha={} -> staged", vol, sha),
        Err(w) => serial_println!("[update] ui stage from={} sha={} -> refused:{}", vol, sha, w),
    }
    r
}

/// The pane's Remove on a set-aside image: [`unstage`] (`KERNEL.NEW`, the records and the sidecar go).
pub fn remove_set_aside() -> Result<(), &'static str> {
    let r = unstage();
    match r {
        Ok(()) => serial_println!("[update] ui remove -> removed"),
        Err(w) => serial_println!("[update] ui remove -> refused:{}", w),
    }
    r
}

/// `tests updateui`'s RAM volume: a flat, read-only name -> bytes map mounted under `/volumes/<name>` in the fixture's
/// own table (nothing in the namespace sees it).
struct RamVol {
    name: String,
    files: Vec<(String, Vec<u8>)>,
}

impl RamVol {
    fn find(&self, rel: &str) -> Option<&Vec<u8>> {
        let leaf = rel.trim_start_matches('/');
        self.files.iter().find(|(n, _)| n.eq_ignore_ascii_case(leaf)).map(|(_, d)| d)
    }
}

impl VfsBackend for RamVol {
    fn volume_name(&self) -> &str {
        &self.name
    }
    fn volume_id(&self) -> Option<u64> {
        None
    }
    fn read_dir(&self, rel: &str) -> Result<Vec<DirEnt>, VfsError> {
        if !rel.trim_start_matches('/').is_empty() {
            return Err(VfsError::NoSuchPath);
        }
        Ok(self.files.iter().map(|(n, d)| DirEnt { name: n.clone(), kind: NodeKind::File, size: d.len() as u64, mtime: None }).collect())
    }
    fn stat(&self, rel: &str) -> Result<Stat, VfsError> {
        if rel.trim_start_matches('/').is_empty() {
            return Ok(Stat { kind: NodeKind::Dir, size: 0, id: None, mtime: None });
        }
        self.find(rel).map(|d| Stat { kind: NodeKind::File, size: d.len() as u64, id: None, mtime: None }).ok_or(VfsError::NoSuchPath)
    }
    fn read(&self, rel: &str, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let d = self.find(rel).ok_or(VfsError::NoSuchPath)?;
        let a = (offset as usize).min(d.len());
        Ok(d[a..(a + len).min(d.len())].to_vec())
    }
    fn authorize_read(&self, _rel: &str, _principal: &str) -> Result<(), VfsError> {
        Ok(())
    }
}

/// The fixture's two sticks, read-only in RAM: `/volumes/UPDTEST` (a copy of the running `KERNEL.ELF` + a `SRC.SHA`
/// stamp) and `/volumes/NOSHA` (a kernel and no stamp). Returns the table and the two leaves.
pub fn fixture_table() -> Result<(MountTable, [String; 2]), &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let src = esp(abslot::ELF);
    let st = mt.stat(&src).map_err(|_| "no-running-kernel")?;
    let mut elf = Vec::new();
    let mut off = 0u64;
    while off < st.size {
        let want = ((st.size - off) as usize).min(CHUNK);
        let b = mt.read(&src, off, want).map_err(|_| "kernel-read")?;
        if b.len() != want {
            return Err("kernel-short");
        }
        elf.extend_from_slice(&b);
        off += want as u64;
    }
    let head = elf[..elf.len().min(64 << 10)].to_vec();
    let stamp = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  SRC.TGZ\ncommit 7e57ab1e00000000000000000000000000000000\n".to_vec();
    let (a, b) = (String::from("UPDTEST"), String::from("NOSHA"));
    let mut t = MountTable::new();
    t.mount(&format!("/volumes/{}", a), alloc::boxed::Box::new(RamVol { name: a.clone(), files: alloc::vec![(String::from("KERNEL.ELF"), elf), (String::from("SRC.SHA"), stamp)] }));
    t.mount(&format!("/volumes/{}", b), alloc::boxed::Box::new(RamVol { name: b.clone(), files: alloc::vec![(String::from("KERNEL.ELF"), head)] }));
    Ok((t, [a, b]))
}

/// The fixture's stand-in for the LOADER's fallback write (`abslot::Write::Fallback`: `why=fallback`, `STAGE.REC`
/// removed), then the next boot's [`settle`]. `Some(boot n)` when the note was written.
pub fn fixture_fallback() -> Option<u64> {
    let mt = crate::shell::vfs_mount_table();
    {
        let _unlock = bootfat::fat_unlock(Writer::Updater, "fixture: the loader's fallback write");
        if !bootfat::allows() || drop_file(&mt, abslot::STAGE).is_err() || put(&mt, abslot::SHUTDOWN, abslot::REC_FALLBACK).is_err() {
            return None;
        }
    }
    settle(&mt)
}
