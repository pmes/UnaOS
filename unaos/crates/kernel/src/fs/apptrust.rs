//! CHARTER: Kernel — fs-core
//!
//! APPTRUST (rmbp-ledger B467) — a program SIGHTED on a removable volume is not a registrant until the user says
//! so. OPENERTRUST (B447) closed the file side (a document's `una:preferred` names a REGISTRANT or is ignored) and
//! named the boundary left open: a registrant was whatever APPRES sighted, so a program on a USB stick or the
//! camera card joined `una:apps` on the type object by being SEEN and was from then on a legitimate opener.
//! SECREVIEW's rule: ring-3 input never widens the kernel's trust by itself.
//!
//! **The seam** — no new store. The trust of a program is a fact of WHERE it is: [`trust_of`] asks the mount table
//! whether the volume claiming the path is the same storage as one of the SYSTEM mounts (`/`, `/boot`, `/apps`,
//! `/lib`); anything else (a removable volume, another disk under `/volumes`, a stale `/volumes/<gone>` path the
//! root would claim, a path no mount claims) is FOREIGN. `appres::sight_in` asks it once per sight: a root program
//! is cached and published as before; a foreign one WRITES NOTHING (no attribute on the stick, no signature object,
//! no `una:apps` line) and is listed here, in RAM, for this boot — [`foreign_rows`]. `appres::registrants` and
//! `assoc::opener_for_in` refuse a foreign path wherever it comes from (an `una:apps` line from before this arc, a
//! registry `una:preferred`), and `opener_of_preferred` never maps a signature to a foreign program.
//!
//! **The ask** — Quarry's Open With lists the foreign programs that declare the file's type under a divider as
//! `<name>  on <volume>`; a pick asks ONCE through DIALOG2 ([`request`]): `Open <file> with <program> from
//! <volume>?`, buttons `Copy to Apps` · `Open` · `Cancel` (Cancel the default, rightmost; Esc cancels). `Open`
//! trusts that program for THIS SESSION (RAM: [`granted`]); `Copy to Apps` asks `users::admin_authority` and
//! copies the program into `/apps` (ROOTACL's tree), where its next sight is a root sight and it becomes a
//! registrant like any other; then the file opens with the copy.
//!
//! Wire: `[appres] sighted <path> volume=<v> trust=<root|foreign>` (appres), `[apptrust] ask prog=<p> file=<f>
//! volume=<v> posted=<0|1>`, `[apptrust] answer=<open|copy|cancel> prog=<p>`, `[apptrust] open prog=<p>
//! granted=session`, `[apptrust] copy <src> -> <dst> registrant=<yes|no>` (or `refused=<why>`), and `tests apptrust`
//! → `:: APPTRUST: foreign=listed registrant=refused asked=once copied=registrant rebind=asked icon=root launch=asked
//! copy=rootacl -> PASS ::` (APPTRUST2 B478: the grant binds volume, path and stamp — `[apptrust] grant volume= path=
//! stamp=`; a direct launch asks — `[apptrust] launch prog= -> ask`; Copy to Apps writes `via=rootacl`).
//! Design: `docs/dev/evidence/rmbp-1005/apptrust.md`, `apptrust2.md`.
//!
//! **REASK (B681, R130 law 2)** — `Open` is the user's word, not a session's: [`answer`] records it in Principia's store
//! (`prefs::decide`, `system.decisions.apptrust.<key>`, BOUND to the program's volume fingerprint and stamp — another
//! program at the path asks again), and [`request`] consults it (`prefs::decided`) before any ask: `[apptrust] open
//! prog= file= granted=decision`. It survives reboots and the calendar; only Settings > Privacy's Revoke drops it, and
//! with no decision a RAM grant for the path is dropped too (the store is the authority). `tests reask` →
//! `:: REASK: domains=<n> first_ask=1 reasks=0 after_revoke=1 -> PASS ::`.
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::fs::appres::Registrant;
use crate::fs::vfs::{same_storage, AttrValue, MountTable, NodeKind, KERNEL_PRINCIPAL};

/// The mounts whose storage is the system's: a program on one of them is a root program.
const SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/apps", "/lib"];

/// Where a program is trusted from (the target of Copy to Apps).
pub const APPS_DIR: &str = "/apps";

/// The volume name a path is on, for the wire and the Open With row: the leaf under `/volumes`, else the
/// backend's own name.
pub fn volume_of(mt: &MountTable, path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/volumes/") {
        if let Some(n) = rest.split('/').next().filter(|n| !n.is_empty()) {
            return String::from(n);
        }
    }
    mt.volume_name(path).unwrap_or_else(|_| String::from("unknown"))
}

/// Is `path` on the system's storage? `false` for a builtin id (not a path) never arises: callers ask for paths.
pub fn is_root(mt: &MountTable, path: &str) -> bool {
    let Ok((b, rel)) = mt.resolve(path) else { return false };
    // A `/volumes/<n>/…` path the ROOT claims as a plain directory is a volume that is not mounted (unplugged):
    // stale, never the system's. (`/volumes/UnaOS/…` is redirected by `rootdisk` and arrives without the prefix.)
    if path.starts_with("/volumes/") && rel.trim_start_matches('/').starts_with("volumes/") {
        return false;
    }
    SYSTEM_MOUNTS.iter().any(|m| matches!(mt.resolve(m), Ok((s, _)) if same_storage(b, s)))
}

/// Is the opener `op` a FOREIGN program path? A builtin id (no leading `/`) is never foreign.
pub fn is_foreign(mt: &MountTable, op: &str) -> bool {
    op.starts_with('/') && !is_root(mt, op)
}

/// `(volume, root?)` for a program at `path`.
pub fn trust_of(mt: &MountTable, path: &str) -> (String, bool) {
    (volume_of(mt, path), is_root(mt, path))
}

/// One foreign program sighted this boot.
#[derive(Clone)]
struct Foreign {
    path: String,
    volume: String,
    name: String,
    signature: String,
    doctypes: Vec<String>,
}

static FOREIGN: crate::sync::Mutex<Vec<Foreign>> = crate::sync::Mutex::new(Vec::new());
/// APPTRUST2 (B478): a session grant is bound to WHAT was trusted, not where it was — the volume's VOLID fingerprint
/// (`MountTable::volume_id`, what `same_storage` compares), the path, and APPRES's stamp (`<mtime>:<size>`).
#[derive(Clone, PartialEq)]
pub struct Bind {
    pub vol: Option<u64>,
    pub stamp: String,
}

/// One `Open` the user said this session.
#[derive(Clone)]
struct Grant {
    path: String,
    bind: Bind,
}

/// Programs the user said `Open` to this session (RAM; nothing outlives the boot).
static GRANTS: crate::sync::Mutex<Vec<Grant>> = crate::sync::Mutex::new(Vec::new());
/// Dialogs posted by [`request`].
static ASKS: AtomicU32 = AtomicU32::new(0);

/// APPRES's foreign branch: remember a sighted foreign program (replacing an earlier sight of the same path).
pub fn note_foreign(path: &str, volume: &str, name: &str, signature: &str, doctypes: &[String]) {
    let mut f = FOREIGN.lock();
    f.retain(|x| x.path != path);
    f.push(Foreign { path: String::from(path), volume: String::from(volume), name: String::from(name), signature: String::from(signature), doctypes: doctypes.to_vec() });
}

/// Was `path` sighted as foreign this boot? (No mount table: a memo check for APPRES's signature lookup.)
pub fn sighted_foreign(path: &str) -> bool {
    FOREIGN.try_lock().map(|f| f.iter().any(|x| x.path == path)).unwrap_or(true)
}

/// The foreign programs that declare `mime` and that `live` says are still there, with their volumes.
fn foreign_rows_by(mime: &str, live: impl Fn(&str) -> bool) -> Vec<(Registrant, String)> {
    let f = FOREIGN.lock().clone();
    f.into_iter()
        .filter(|x| x.doctypes.iter().any(|d| d == mime) && live(&x.path))
        .map(|x| {
            let name = if x.name.is_empty() { String::from(x.path.rsplit('/').next().unwrap_or(&x.path)) } else { x.name };
            (Registrant { opener: x.path, signature: x.signature, name }, x.volume)
        })
        .collect()
}

/// Open With's second list: the foreign programs sighted this boot that declare `mime` and still stat.
pub fn foreign_rows(mt: &MountTable, mime: &str) -> Vec<(Registrant, String)> {
    foreign_rows_by(mime, |p| mt.stat(p).is_ok())
}

/// The program at `prog` as it is NOW: its volume's fingerprint and its stamp (`-` when it does not stat).
pub fn bind_of(mt: &MountTable, prog: &str) -> Bind {
    let vol = mt.volume_id(prog).ok().flatten();
    let stamp = mt.stat(prog).map(|s| alloc::format!("{}:{}", s.mtime.unwrap_or(0), s.size)).unwrap_or_else(|_| String::from("-"));
    Bind { vol, stamp }
}

/// REASK (B681): the decision's bind — what the user's `Open` was given to (`vol=<fingerprint>;stamp=<mtime:size>`).
fn bind_word(b: &Bind) -> String {
    alloc::format!("vol={};stamp={}", vol_word(b.vol), b.stamp)
}

fn vol_word(v: Option<u64>) -> String {
    v.map(|x| alloc::format!("{}", x)).unwrap_or_else(|| String::from("none"))
}

/// Did the user say `Open` to THIS program (same volume, path and stamp) this session? A grant at the path whose
/// volume or stamp differs is a different program: it is dropped (said on the wire) and the caller asks again.
pub fn granted_bound(prog: &str, b: &Bind) -> bool {
    let mut g = GRANTS.lock();
    let Some(i) = g.iter().position(|x| x.path == prog) else { return false };
    if g[i].bind == *b {
        return true;
    }
    let why = if g[i].bind.vol != b.vol { "volume" } else { "stamp" };
    serial_println!("[apptrust] grant stale path={} reason={} was={}/{} now={}/{} -> ask", prog, why, vol_word(g[i].bind.vol), g[i].bind.stamp, vol_word(b.vol), b.stamp);
    g.remove(i);
    false
}

/// Did the user say `Open` to `prog` this session (any binding; the test's read)?
pub fn granted(prog: &str) -> bool {
    GRANTS.lock().iter().any(|g| g.path == prog)
}

fn grant(prog: &str, volume: &str, b: &Bind) {
    let mut g = GRANTS.lock();
    g.retain(|x| x.path != prog);
    g.push(Grant { path: String::from(prog), bind: b.clone() });
    drop(g);
    serial_println!("[apptrust] grant volume={}#{} path={} stamp={}", volume, vol_word(b.vol), prog, b.stamp);
}

fn leaf(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// APPTRUST2 (B478): the principal Copy to Apps writes as — the logged-in session's `user:<name>` when ROOTACL
/// (B456) calls it the administrator (`rootacl::admin_principal`, R100); otherwise the copy is refused.
pub fn copy_principal() -> Result<String, &'static str> {
    #[cfg(not(feature = "login"))] { Err("no-admin-session") } // no session store without `login`
    #[cfg(feature = "login")] {
    let mut nb = [0u8; crate::fs::users::NAME_MAX];
    let Some(n) = crate::fs::users::whoami(&mut nb) else { return Err("no-admin-session") };
    let name = core::str::from_utf8(&nb[..n]).map_err(|_| "no-admin-session")?;
    let p = alloc::format!("user:{}", name);
    if crate::fs::rootacl::admin_principal(&p) { Ok(p) } else { Err("not-admin") }
    }
}

/// **Copy to Apps**: the administrator's act that makes a foreign program a registrant. APPTRUST2 (B478): the
/// write goes through ROOTACL's `/apps` writer — `create`/`write` run as the administrator's session principal
/// ([`copy_principal`]) and `rootacl::write_verdict` decides; no administrator session → refused with the alert.
/// Refuses an existing `/apps/<leaf>` (never overwrites), then sights the copy (a ROOT sight: cached, published in
/// `una:apps`). Returns the copy's path.
pub fn copy_to_apps(mt: &MountTable, src: &str) -> Result<String, &'static str> {
    let say = |why: &'static str| {
        serial_println!("[apptrust] copy {} -> {}/{} refused={} via=rootacl", src, APPS_DIR, leaf(src), why);
        why
    };
    if !is_foreign(mt, src) {
        return Err(say("not-foreign"));
    }
    let who = match copy_principal() {
        Ok(p) => p,
        Err(why) => {
            #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
            let _ = crate::video::dialog::refused(crate::video::dialog::WHAT_SYSTEM_FILES, b"Copy to Apps needs the administrator's session (log in as the administrator)");
            return Err(say(why));
        }
    };
    let st = mt.stat(src).map_err(|_| say("no-source"))?;
    if !matches!(st.kind, NodeKind::File) {
        return Err(say("not-a-file"));
    }
    let dst = alloc::format!("{}/{}", APPS_DIR, leaf(src));
    if mt.stat(&dst).is_ok() {
        return Err(say("exists"));
    }
    let bytes = mt.read(src, 0, st.size as usize).map_err(|_| say("read"))?;
    if bytes.len() as u64 != st.size {
        return Err(say("short-read"));
    }
    mt.create(&dst, NodeKind::File, &who).map_err(|e| say(if matches!(e, crate::fs::vfs::VfsError::Denied) { "rootacl-denied" } else { "create" }))?;
    if mt.write(&dst, 0, &bytes, &who).map(|n| n != bytes.len()).unwrap_or(true) {
        let _ = mt.unlink(&dst, &who);
        return Err(say("write"));
    }
    let reg = crate::fs::appres::sight_in(mt, &dst).map(|a| a.doctypes.iter().all(|m| crate::fs::assoc::registrants_in(mt, m).iter().any(|r| r.opener == dst))).unwrap_or(false);
    serial_println!("[apptrust] copy {} -> {} bytes={} registrant={} via=rootacl principal={}", src, dst, bytes.len(), if reg { "yes" } else { "no" }, who);
    Ok(dst)
}

// ── The ask (DIALOG2) — the desktop's half ───────────────────────────────────────────────────────────────────────

/// What the user is being asked about, and what an answer latched for the service pass.
#[derive(Clone)]
pub struct Ask {
    pub prog: String,
    pub file: String,
    pub mime: String,
    pub volume: String,
}

/// `true` = Open (granted), `false` = Copy to Apps.
static ASKING: crate::sync::Mutex<Option<(Ask, Bind)>> = crate::sync::Mutex::new(None);
static PENDING: crate::sync::Mutex<Option<(Ask, bool)>> = crate::sync::Mutex::new(None);

/// A pick of the foreign `prog` for `file`: `true` = granted this session already (the caller opens now);
/// `false` = the dialog was posted (or could not be: said on the wire), the answer arrives through the hook.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn request(a: Ask) -> bool {
    use crate::video::dialog::{self, Act, Dlg, Icon};
    let bind = bind_of(&crate::shell::vfs_mount_table(), &a.prog); // APPTRUST2 (B478): what is trusted is bound at the ask
    // REASK (B681, R130 law 2): the user's `Open`, kept in Principia's store, answers before any ask.
    if crate::prefs::decided(crate::prefs::decide::APPTRUST, &a.prog, &bind_word(&bind)).as_deref() == Some("open") {
        if !granted_bound(&a.prog, &bind) {
            grant(&a.prog, &a.volume, &bind);
        }
        serial_println!("[apptrust] open prog={} file={} granted=decision", a.prog, a.file);
        return true;
    }
    if granted_bound(&a.prog, &bind) {
        // No decision for it (revoked, or never persisted): the store is the authority, the RAM grant goes with it.
        GRANTS.lock().retain(|g| g.path != a.prog);
        serial_println!("[apptrust] grant dropped prog={} reason=no-decision -> ask", a.prog);
    }
    let msg = if a.file == a.prog { alloc::format!("Open {} from {}?", leaf(&a.prog), a.volume) } else { alloc::format!("Open {} with {} from {}?", leaf(&a.file), leaf(&a.prog), a.volume) };
    let info = alloc::format!("{} is on {}, not on the system volume.\nOpen trusts this copy; Settings > Privacy revokes it.\nCopy to Apps installs it (administrator).", leaf(&a.prog), a.volume); // REASK (B681): asked once
    let mut d = Dlg::new(Icon::Caution, b"Open With", msg.as_bytes(), info.as_bytes(), &[b"Copy to Apps", b"Open", b"Cancel"]);
    d.user = true;
    d.act = Act::Hook(answer as fn(bool) as usize);
    let posted = {
        let mut g = ASKING.lock();
        let p = dialog::post(d);
        if p {
            *g = Some((a.clone(), bind));
        }
        p
    };
    if posted {
        ASKS.fetch_add(1, Ordering::Relaxed);
    }
    serial_println!("[apptrust] ask prog={} file={} volume={} posted={}", a.prog, a.file, a.volume, posted as u8);
    false
}

/// DIALOG's answer (press/key route; queue-only): `ok` = the default (Cancel). The button index tells Open from
/// Copy to Apps from Esc.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn answer(ok: bool) {
    let Some((a, bind)) = ASKING.lock().take() else { return };
    let ix = crate::video::dialog::last_button();
    let word = match (ok, ix) {
        (false, 0) => "copy",
        (false, 1) => "open",
        _ => "cancel",
    };
    serial_println!("[apptrust] answer={} prog={}", word, a.prog);
    match word {
        "open" => {
            grant(&a.prog, &a.volume, &bind);
            crate::prefs::decide(crate::prefs::decide::APPTRUST, &a.prog, &bind_word(&bind), "open"); // REASK (B681): the user's word, kept (queue-only here)
            *PENDING.lock() = Some((a, true));
        }
        "copy" => *PENDING.lock() = Some((a, false)),
        _ => {}
    }
}

/// The service pass's half: the answered ask, if any — `(ask, true)` open it with the foreign program (granted),
/// `(ask, false)` copy it to `/apps` and open with the copy (the caller runs [`copy_to_apps`]).
pub fn take_pending() -> Option<(Ask, bool)> {
    PENDING.try_lock().and_then(|mut p| p.take())
}

// ── APPTRUST2 (B478) — a foreign program opened DIRECTLY asks the same question ─────────────────────────────────

/// The direct-launch door (`quarry::openers::open("launch", …)` — Quarry's double-click and the Launcher's file
/// pick): `true` = launch now (a root program, or this foreign program granted this session); `false` = the DIALOG2
/// ask was posted (file = the program; the answer runs on Quarry's service pass) or could not be, and nothing runs.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn launch_gate(mt: &MountTable, prog: &str) -> bool {
    if !is_foreign(mt, prog) {
        return true;
    }
    let volume = volume_of(mt, prog);
    let ok = request(Ask { prog: String::from(prog), file: String::from(prog), mime: String::from(crate::fs::filetype::UNAOS_ELF), volume: volume.clone() });
    serial_println!("[apptrust] launch prog={} volume={} -> {}", prog, volume, if ok { "granted=session" } else { "ask" });
    ok
}

/// No dialog in this build: a foreign program is never launched directly (fail closed, said once per try).
#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn launch_gate(mt: &MountTable, prog: &str) -> bool {
    if !is_foreign(mt, prog) {
        return true;
    }
    serial_println!("[apptrust] launch prog={} volume={} -> refused=no-dialog", prog, volume_of(mt, prog));
    false
}

// ── `tests apptrust` (R80: run only when asked) ───────────────────────────────────────────────────────────────────

/// The probe's type — its own object in the registry, created and removed by the test.
const PROBE_MIME: &str = "application/x-apptrust-probe";
const PROBE_SIG: &str = "org.unaos.apptrust-probe";

/// `:: APPTRUST: foreign=listed registrant=refused asked=once copied=registrant -> PASS ::`
/// * `foreign` — a probe program sighted from a volume that is not mounted (`/volumes/apptrust-probe/…`, which
///   [`trust_of`] must call foreign) through APPRES's own admission: it is in [`foreign_rows`], not in
///   `registrants`.
/// * `registrant` — the same path written into the probe type's `una:apps` by hand (a line from before this arc)
///   is still refused by `registrants`, and a probe document whose `una:preferred` names it does not open with it.
/// * `asked` — two picks of a foreign program: ONE dialog (headless), its `Open` grants the session, the second
///   pick opens without asking; a Cancel on another grants nothing.
/// * `copied` — the probe admitted at `/apps/<leaf>` (a root sight, what Copy to Apps' copy is) IS a registrant.
///   `skip` on a root that takes no attributes (nothing to publish into).
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let fpath = "/volumes/apptrust-probe/PROBE.ELF";
    let rpath = "/apps/APTPROBE.ELF";
    forget_probe_decisions(); // REASK (B681): an `Open` from an earlier run is a decision in the store — the probe starts unasked
    let (vol, root) = trust_of(&mt, fpath);
    let (_, apps_root) = trust_of(&mt, rpath);
    let classify = !root && apps_root && vol == "apptrust-probe";
    let obj = crate::fs::assoc::object_path(PROBE_MIME);
    let sig_obj = crate::fs::appres::signature_object(PROBE_SIG);
    let attrs = mt.list_attrs("/", KERNEL_PRINCIPAL).is_ok() && (mt.stat(&obj).is_ok() || mt.create(&obj, NodeKind::File, KERNEL_PRINCIPAL).is_ok());

    // foreign=listed: APPRES's admission of a foreign sight lists it and publishes nothing.
    let probe = crate::fs::appres::probe_app(fpath, PROBE_SIG, PROBE_MIME);
    let (_, on) = crate::fs::appres::admit(&mt, &probe, &vol, root);
    let listed = foreign_rows_by(PROBE_MIME, |_| true).iter().any(|(r, v)| r.opener == fpath && v == "apptrust-probe");
    let published = attrs && matches!(mt.get_attr(&obj, crate::fs::appres::KEY_APPS, KERNEL_PRINCIPAL), Ok(AttrValue::Str(s)) if s.lines().any(|l| l == fpath));
    let foreign_ok = classify && listed && on == "foreign" && !published && sighted_foreign(fpath);

    // registrant=refused: a planted `una:apps` line and a planted per-file preference are both refused.
    let mut refused = !crate::fs::assoc::registrants_in(&mt, PROBE_MIME).iter().any(|r| r.opener == fpath);
    let mut planted = "no";
    if attrs {
        planted = "yes";
        let _ = mt.set_attr(&obj, crate::fs::appres::KEY_APPS, AttrValue::Str(String::from(fpath)), KERNEL_PRINCIPAL);
        refused &= !crate::fs::assoc::registrants_in(&mt, PROBE_MIME).iter().any(|r| r.opener == fpath);
        let (op, _) = crate::fs::assoc::opener_for_in(&mt, &obj, PROBE_MIME);
        refused &= op != fpath;
        let _ = mt.set_attr(&obj, crate::fs::assoc::PREFERRED_KEY, AttrValue::Str(String::from(fpath)), KERNEL_PRINCIPAL);
        let (op, _) = crate::fs::assoc::opener_for_in(&mt, &obj, PROBE_MIME);
        refused &= op != fpath;
        let _ = mt.remove_attr(&obj, crate::fs::assoc::PREFERRED_KEY, KERNEL_PRINCIPAL);
        let _ = mt.remove_attr(&obj, crate::fs::appres::KEY_APPS, KERNEL_PRINCIPAL);
    }

    // asked=once
    let asked = ask_once(fpath);

    // copied=registrant: the copy's sight is a root sight.
    let copied = if attrs {
        let copy = crate::fs::appres::probe_app(rpath, PROBE_SIG, PROBE_MIME);
        let (_, on) = crate::fs::appres::admit(&mt, &copy, "root", true);
        let reg = on != "foreign" && crate::fs::assoc::registrants_in(&mt, PROBE_MIME).iter().any(|r| r.opener == rpath);
        if reg { "registrant" } else { "refused" }
    } else {
        "skip"
    };
    // Leave nothing: the probe type object, its signature object, the foreign row, any grant.
    if attrs {
        let _ = mt.unlink(&obj, KERNEL_PRINCIPAL);
        let _ = mt.unlink(&sig_obj, KERNEL_PRINCIPAL);
    }
    FOREIGN.lock().retain(|x| x.path != fpath);
    GRANTS.lock().retain(|g| g.path != fpath && !g.path.ends_with("PROBE2.ELF"));
    crate::fs::appres::forget(fpath);
    crate::fs::appres::forget(rpath);

    // APPTRUST2 (B478): rebind / icon / launch / copy.
    let rebind = rebind_probe();
    let icon = crate::fs::appres::icon_probe();
    let launch = launch_probe(&mt);
    let (copy, copy_admin) = if attrs { copy_probe(&mt) } else { ("skip", String::from("-(no-attr-root)")) }; // UIREDS26 (B559): `copied`'s precondition — a root that takes no attributes is the read-only boot FAT (R99), nothing to copy into
    GRANTS.lock().retain(|g| !g.path.starts_with("/volumes/apptrust-probe/"));
    forget_probe_decisions();

    let ok = foreign_ok && refused && asked != "FAIL" && copied != "refused" && rebind != "FAIL" && icon == "root" && launch != "FAIL" && copy != "REFUSED";
    serial_println!(":: APPTRUST: foreign={} registrant={} asked={} copied={} rebind={} icon={} launch={} copy={} -> {} :: classify={} volume={} planted={} asks={} grants={} copy_admin={}",
        if foreign_ok { "listed" } else { "MISSING" }, if refused { "refused" } else { "ACCEPTED" }, asked, copied, rebind, icon, launch, copy,
        if ok { "PASS" } else { "FAIL" }, classify, vol, planted, ASKS.load(Ordering::Relaxed), GRANTS.lock().len(), copy_admin);
}

/// REASK (B681): the probe programs' decisions leave the store (the fixture's `Open` is not the user's).
fn forget_probe_decisions() {
    for p in ["PROBE.ELF", "PROBE2.ELF", "PROBE3.ELF", "PROBE4.ELF"] {
        let k = crate::prefs::decision_key(crate::prefs::decide::APPTRUST, &alloc::format!("/volumes/apptrust-probe/{}", p));
        if crate::prefs::decisions().iter().any(|d| d.key == k) {
            let _ = crate::prefs::revoke(&k);
        }
    }
}

#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn ask_once(fpath: &str) -> &'static str {
    use crate::video::dialog::{self, Answer};
    if dialog::is_up() {
        return "skip";
    }
    let was = dialog::headless(true);
    let a0 = ASKS.load(Ordering::Relaxed);
    let ask = |p: &str| Ask { prog: String::from(p), file: String::from("/tmp/probe.txt"), mime: String::from(PROBE_MIME), volume: String::from("apptrust-probe") };
    let first = request(ask(fpath));
    dialog::open_now();
    let up = dialog::is_up();
    dialog::answer(Answer::Button(1)); // Open
    let pend = take_pending().map(|(a, open)| open && a.prog == fpath).unwrap_or(false);
    let second = request(ask(fpath));
    // Another program, Cancel (the default): nothing granted, nothing latched.
    let other = "/volumes/apptrust-probe/PROBE2.ELF";
    let _ = request(ask(other));
    dialog::open_now();
    dialog::answer(Answer::Default);
    let cancelled = !granted(other) && take_pending().is_none();
    dialog::headless(was);
    let asks = ASKS.load(Ordering::Relaxed) - a0;
    if !first && up && pend && second && granted(fpath) && cancelled && asks == 2 { "once" } else { "FAIL" }
}

#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn ask_once(_fpath: &str) -> &'static str {
    "skip"
}

/// `rebind`: a grant at a path whose stamp/volume is not the program's now is dropped and the ask is posted again.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn rebind_probe() -> &'static str {
    use crate::video::dialog::{self, Answer};
    if dialog::is_up() {
        return "skip";
    }
    let p3 = "/volumes/apptrust-probe/PROBE3.ELF";
    let was = dialog::headless(true);
    let a0 = ASKS.load(Ordering::Relaxed);
    GRANTS.lock().push(Grant { path: String::from(p3), bind: Bind { vol: Some(1), stamp: String::from("0:1") } });
    let r = request(Ask { prog: String::from(p3), file: String::from("/tmp/probe.txt"), mime: String::from(PROBE_MIME), volume: String::from("apptrust-probe") });
    dialog::open_now();
    dialog::answer(Answer::Default);
    let ok = !r && ASKS.load(Ordering::Relaxed) - a0 == 1 && !granted(p3) && take_pending().is_none();
    dialog::headless(was);
    if ok { "asked" } else { "FAIL" }
}

/// `launch`: the direct-launch door asks for a foreign program (Cancel) and lets a root one through unasked.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn launch_probe(mt: &MountTable) -> &'static str {
    use crate::video::dialog::{self, Answer};
    if dialog::is_up() {
        return "skip";
    }
    let was = dialog::headless(true);
    let a0 = ASKS.load(Ordering::Relaxed);
    let r = launch_gate(mt, "/volumes/apptrust-probe/PROBE4.ELF");
    dialog::open_now();
    dialog::answer(Answer::Default);
    let root = launch_gate(mt, "/apps/APTPROBE.ELF");
    let ok = !r && root && ASKS.load(Ordering::Relaxed) - a0 == 1 && take_pending().is_none();
    dialog::headless(was);
    if ok { "asked" } else { "FAIL" }
}

#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn rebind_probe() -> &'static str {
    "skip"
}

#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn launch_probe(mt: &MountTable) -> &'static str {
    if launch_gate(mt, "/volumes/apptrust-probe/PROBE4.ELF") { "FAIL" } else { "refused" }
}

/// `copy`: Copy to Apps' writer is ROOTACL's — with an administrator session a file is created and written under
/// `/apps` as that principal (and `anon` is refused there); without one, [`copy_principal`] refuses (the copy is).
/// `("rootacl" | "REFUSED", detail)`.
fn copy_probe(mt: &MountTable) -> (&'static str, String) {
    let p = match copy_principal() {
        Ok(p) => p,
        Err(why) => return ("rootacl", alloc::format!("none({})", why)),
    };
    const F: &str = "/apps/APTCOPY.TMP";
    const G: &str = "/apps/APTANON.TMP";
    let _ = mt.unlink(F, KERNEL_PRINCIPAL);
    let _ = mt.unlink(G, KERNEL_PRINCIPAL);
    let made = mt.create(F, NodeKind::File, &p).is_ok() && mt.write(F, 0, b"apptrust2", &p).is_ok();
    let anon = matches!(mt.create(G, NodeKind::File, "anon"), Err(crate::fs::vfs::VfsError::Denied));
    let _ = mt.unlink(G, KERNEL_PRINCIPAL);
    let _ = mt.unlink(F, KERNEL_PRINCIPAL);
    (if made { "rootacl" } else { "REFUSED" }, alloc::format!("{} anon={}", p, if anon { "refused" } else { "ADMITTED" }))
}

// ── REASK (rmbp-ledger B681, R130 law 2) — `tests reask` ─────────────────────────────────────────────────────────

/// `tests reask` (R80: run only when asked; headless, model-only dialogs):
/// `:: REASK: domains=<n> first_ask=1 reasks=0 after_revoke=1 -> PASS ::`.
/// * `first_ask` — a foreign probe program picked once: ONE ask, answered `Open` (the decision is recorded); the three
///   refusal seams (`dialog::reask_refusals`: opener, system files, FWPIN's toast) each reach the glass once.
/// * `reasks` — the decisions persisted, then THREE reboots (the store's RAM cleared and its files read back,
///   `prefs::reload_from_store`; apptrust's session grants and the refusal router's repeat memory cleared) and the
///   civil clock 90 days on (a `clock::FixtureClock`, never the live clock): every pick opens with no ask, and no
///   refusal is shown again. Asks + refusals shown again = 0.
/// * `after_revoke` — the probe's decision revoked (Settings > Privacy's call): the next pick asks once (Cancel: nothing
///   recorded).
/// * `domains` — the seams that held: apptrust plus each refusal domain shown once and never again (4 = all).
/// Everything the fixture recorded is revoked at the end.
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn reask_selftest() {
    use crate::clock::AnchorSink;
    use crate::video::dialog::{self, Answer};
    if dialog::is_up() {
        serial_println!(":: REASK: -> SKIP reason=dialog-up ::");
        return;
    }
    const PROG: &str = "/volumes/reask-probe/REASK.ELF";
    let key = crate::prefs::decision_key(crate::prefs::decide::APPTRUST, PROG);
    let ask = || Ask { prog: String::from(PROG), file: String::from("/tmp/reask.txt"), mime: String::from(PROBE_MIME), volume: String::from("reask-probe") };
    let forget = || {
        if crate::prefs::decisions().iter().any(|d| d.key == key) {
            let _ = crate::prefs::revoke(&key);
        }
        dialog::reask_forget();
        GRANTS.lock().retain(|g| g.path != PROG);
    };
    forget();
    let was = dialog::headless(true);
    // The fixture's civil clock (CONSOLEFIX M1): anchored at the live second, or the arc's date when the clock is unset.
    let base = crate::clock::try_unix_now().unwrap_or_else(|| crate::clock::unix_from_civil(2026, 10, 19, 12, 0, 0));
    let mono = crate::clock::mono_ticks().unwrap_or(0);
    let mut fc = crate::clock::FixtureClock::new();
    fc.plant(base, mono, crate::clock::ClockSource::Manual);
    crate::prefs::decision_clock(fc.unix_now());

    // 1. The first ask, answered Open; each refusal seam shown once.
    let a0 = ASKS.load(Ordering::Relaxed);
    let r1 = request(ask());
    dialog::open_now();
    dialog::answer(Answer::Button(1)); // Open
    let _ = take_pending();
    let first_ask = ASKS.load(Ordering::Relaxed) - a0;
    let shown = dialog::reask_refusals();
    let flushed = crate::prefs::decisions_flush();

    // 2. Three reboots, then 90 days on the civil clock: no ask, no refusal shown again.
    let a1 = ASKS.load(Ordering::Relaxed);
    let (mut opened, mut reshown) = (0u32, 0u32);
    for step in 0..4u32 {
        if step < 3 {
            GRANTS.lock().retain(|g| g.path != PROG);
            let keys = crate::prefs::reload_from_store();
            serial_println!("[reask] reboot={} keys={} (RAM cleared, the store read back)", step + 1, keys);
        } else {
            fc.plant(base + 90 * 86_400, mono, crate::clock::ClockSource::Manual);
            crate::prefs::decision_clock(fc.unix_now());
            serial_println!("[reask] civil clock +90 d (fixture) now={}", fc.unix_now().unwrap_or(0));
        }
        if request(ask()) {
            opened += 1;
        } else {
            dialog::open_now();
            dialog::answer(Answer::Default); // Cancel
            let _ = take_pending();
        }
        reshown |= dialog::reask_refusals();
    }
    let reasks = (ASKS.load(Ordering::Relaxed) - a1) + reshown.count_ones();

    // 3. Revoke: the next pick asks.
    let revoked = crate::prefs::revoke(&key);
    let a2 = ASKS.load(Ordering::Relaxed);
    let r3 = request(ask());
    dialog::open_now();
    dialog::answer(Answer::Default); // Cancel: nothing recorded
    let _ = take_pending();
    let after_revoke = ASKS.load(Ordering::Relaxed) - a2;
    let cancel_kept = crate::prefs::decisions().iter().any(|d| d.key == key);

    forget();
    crate::prefs::decision_clock(None);
    fc.clear();
    dialog::headless(was);
    let legs = dialog::REASK_LEGS.len() as u32;
    let refusal_held = (shown & !reshown).count_ones();
    let domains = (first_ask == 1 && opened == 4) as u32 + refusal_held;
    let ok = !r1 && first_ask == 1 && reasks == 0 && revoked && !r3 && after_revoke == 1 && !cancel_kept && domains == 1 + legs;
    serial_println!(":: REASK: domains={} first_ask={} reasks={} after_revoke={} -> {} :: opened={}/4 shown={:#x} reshown={:#x} flushed={} revoked={} cancel_recorded={} store={}",
        domains, first_ask, reasks, after_revoke, if ok { "PASS" } else { "FAIL" }, opened, shown, reshown, flushed, revoked as u8, cancel_kept as u8, crate::prefs::path());
}

/// `tests apptrust` registration, once (rides `appres::ensure_tests`).
pub fn ensure_tests() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("apptrust", selftest);
    }
}

// ── NETGRANT (rmbp-ledger B707, BP-security-16, BP-net-12, BP-bar-10/12) — the network is a grant, not a default ─────
//
// Research B §4 ("network is a grant, not a default") and R130 law 2: a ring-3 program's first outbound reach ASKS once —
// `<app> wants to reach <ip:port>` · `Always` · `Allow Once` · `Never` (Never the default, rightmost; Esc dismisses = this
// run is refused, nothing kept) — and `Always`/`Never` are the user's word, kept in Principia's store through REASK's one
// path (`prefs::decide`, `system.decisions.net.<app>`, bind `-`), consulted before the ask is raised, listed on Settings
// > Privacy with a Revoke (the list is domain-generic: `net: <app>` is one more kind of row). `Allow Once` (and a Never
// or an Always while `system.privacy.remember` is off) is THIS RUN's: a RAM row keyed by the asking task.
//
// **The seam** — x86 `sys_connect` (SYS_CONNECT 44) and `sys_sendto` (SYS_SENDTO 42), the only doors a ring-3 socket has
// to the wire, call [`net_gate`] BEFORE `smolnet::stack_connect` / `stack_sendto` (a LINE-NEUTRAL fold each, the
// syscall.rs tail rule): a refusal returns `-EPERM` and the stack is never entered, so no SYN and no datagram is queued
// (held at the socket layer by `tests netgrant`: the stack's outbound ledger, `net_tick::LEDGER`, moves 0). While the
// ask is up the door answers the non-blocking verbs' own retry word (`-EINPROGRESS` for a connect, `-EAGAIN` for a
// datagram) and nothing is queued. Kernel-origin traffic never reaches this gate: SNTP, DHCP, the kernel's resolver
// (`SYS_RESOLVE` included — the kernel resolves for ring 3, BP-net-6) and the shell verbs call smolnet directly.
//
// **Who asks** — the program the launcher named: `wm::app_name_of(slot + 1)` (every `bg`, bare-name, Quarry and
// `SYS_SPAWN_PATH` launch arms it), else the program name of `argv[0]` on the slot's args page (a foreground `run`;
// the page is kernel-written and read-only to ring 3). A slot with neither is a kernel fixture blob (the SOCK ladder,
// `SYS_SPAWN`'s fixed child): no user launched it, nothing names it, it is not asked.
//
// Wire: `[net] grant app=<name> dst=<ip:port> decision=<once|always|never|asked>` (said when the answer for a task and
// destination changes, not per poll), `[netgrant] answer=<always|once|never|dismissed> app= kept=<0|1>`.

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// `-EPERM` (Linux 1): the user said Never — the refusal on the wire (una-abi names no EPERM yet).
pub const NET_EPERM: i64 = -1;
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// The asks posted (the fixture's count).
static NET_ASKS: AtomicU32 = AtomicU32::new(0);

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// The ask on the glass: the program, the asking task, the destination.
#[derive(Clone)]
struct NetAsk {
    app: String,
    task: u64,
    dst: String,
}

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
static NET_ASKING: crate::sync::Mutex<Option<NetAsk>> = crate::sync::Mutex::new(None);
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// This run's answers: `(task, app, allow)` — `Allow Once`, a dismissal, and any answer the store did not keep.
static NET_RUN: crate::sync::Mutex<Vec<(u64, String, bool)>> = crate::sync::Mutex::new(Vec::new());
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// Run rows kept at most (oldest dropped; a dropped row only means that run is asked again).
const NET_RUN_MAX: usize = 64;
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// The last `[net] grant` line said: `(task, ip, port, decision)`.
static NET_SAID: crate::sync::Mutex<Option<(u64, [u8; 4], u16, u8)>> = crate::sync::Mutex::new(None);

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
const NET_WORDS: [&str; 4] = ["once", "always", "never", "asked"];

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// Say the decision for `task` → `ip:port` once per change (the door is polled; the wire is not).
fn net_say(app: &str, task: u64, ip: [u8; 4], port: u16, d: u8) {
    let Some(mut g) = NET_SAID.try_lock() else { return };
    if *g == Some((task, ip, port, d)) {
        return;
    }
    *g = Some((task, ip, port, d));
    drop(g);
    match net_name_of(ip) { Some(nm) => serial_println!("[net] grant app={} dst={}({}.{}.{}.{}:{}) decision={}", app, nm, ip[0], ip[1], ip[2], ip[3], port, NET_WORDS[d as usize]), None => serial_println!("[net] grant app={} dst={}.{}.{}.{}:{} decision={}", app, ip[0], ip[1], ip[2], ip[3], port, NET_WORDS[d as usize]) } // NET6GRANT (SR151): the host the program resolved, from SYS_RESOLVE's reverse cache (in-place edit, line count kept)
}

/// The program a ring-3 socket's slot runs, by the launcher's name (module note above); `None` = a kernel fixture blob.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn net_app_of(row: usize) -> Option<String> {
    if row >= crate::procslot::SHARED_ROW {
        return None;
    }
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    let n = crate::video::wm::app_name_of(row as u64 + 1, &mut nb);
    if n > 0 {
        return core::str::from_utf8(&nb[..n]).ok().map(String::from);
    }
    // The caller's own CR3 is live (syscall context): its args page is mapped at `USER_ARGS_VA` for every slot
    // (`memory::args_map`, ring-3 RO + NX), written only by the launcher.
    let va = una_abi::USER_ARGS_VA;
    let page = unsafe { core::slice::from_raw_parts(va as *const u8, 4096) };
    let a = una_abi::Args::parse(page, va)?;
    let w = core::str::from_utf8(a.get(0)?).ok()?;
    let p = crate::video::wm::program_name(w);
    if p.is_empty() { None } else { Some(String::from(p)) }
}

/// THE door's check (x86 `sys_connect` with `stream`, `sys_sendto` without): `Ok` = the stack may be entered;
/// `Err(-EPERM)` = refused; `Err(-EINPROGRESS | -EAGAIN)` = asking, nothing queued, ring 3 polls again.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
pub fn net_gate(row: usize, ip: [u8; 4], port: u16, stream: bool) -> Result<(), i64> {
    let Some(app) = net_app_of(row) else { return Ok(()) };
    let task = crate::arch::sched::current_id().unwrap_or(0);
    net_gate_for(&app, task, row as u64 + 1, ip, port, stream)
}

/// [`net_gate`]'s body for a named program: the store's decision, then this run's answer, then the ask.
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
pub fn net_gate_for(app: &str, task: u64, owner: u64, ip: [u8; 4], port: u16, stream: bool) -> Result<(), i64> {
    let wait = if stream { -115 } else { una_abi::EAGAIN }; // EINPROGRESS: the connect's own retry word
    // try_lock throughout: this runs in the IF-masked syscall path and never spins (a busy lock = poll again).
    match crate::prefs::decided_quiet(crate::prefs::decide::NET, app, "-").as_deref() {
        Some("always") => {
            net_say(app, task, ip, port, 1);
            return Ok(());
        }
        Some("never") => {
            net_say(app, task, ip, port, 2);
            return Err(NET_EPERM);
        }
        _ => {}
    }
    let Some(run) = NET_RUN.try_lock() else { return Err(wait) };
    if let Some(&(_, _, allow)) = run.iter().rev().find(|(t, a, _)| *t == task && a == app) {
        drop(run);
        net_say(app, task, ip, port, if allow { 0 } else { 2 });
        return if allow { Ok(()) } else { Err(NET_EPERM) };
    }
    drop(run);
    let Some(mut asking) = NET_ASKING.try_lock() else { return Err(wait) };
    if asking.is_some() {
        return Err(wait); // this program's ask (or another's) is on the glass: wait for the answer
    }
    let ipp = alloc::format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port); let nm = net_name_of(ip); let dst = match &nm { Some(n) => alloc::format!("{}({})", n, ipp), None => ipp.clone() }; // NET6GRANT (SR151): `<name>(<ip:port>)` when SYS_RESOLVE answered this ip (in-place edit)
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        use crate::video::dialog::{self, Act, Dlg, Icon};
        let msg = match &nm { Some(n) => alloc::format!("{} wants to reach {} ({})", app, n, ipp), None => alloc::format!("{} wants to reach {}", app, ipp) }; // NET6GRANT (SR151): the ask names the host
        let info = alloc::format!("Always and Never are kept; Settings > Privacy revokes them.\nAllow Once lets this run of {} through.", app);
        let mut d = Dlg::new(Icon::Caution, b"Network", msg.as_bytes(), info.as_bytes(), &[b"Always", b"Allow Once", b"Never"]);
        d.cancel = Some(2); // Esc is a refusal of this run, never the leftmost `Always`
        d.owner = owner;
        d.act = Act::Hook(net_answer as fn(bool) as usize);
        if !dialog::post(d) {
            return Err(wait); // another alert holds the queue: this ask is posted on a later poll
        }
        *asking = Some(NetAsk { app: String::from(app), task, dst });
        drop(asking);
        NET_ASKS.fetch_add(1, Ordering::Relaxed);
        net_say(app, task, ip, port, 3);
        Err(wait)
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        // No dialog in this build: nobody can be asked, so nothing reaches the wire without a kept Always.
        let _ = (owner, &mut asking);
        serial_println!("[net] grant app={} dst={} decision=never why=no-dialog", app, dst);
        Err(NET_EPERM)
    }
}

/// DIALOG's answer (press/key route; queue-only): `Always` 0 · `Allow Once` 1 · `Never` 2 (the default, `ok`); Esc
/// resolves to Never's index with `ok = false` — a dismissal, this run refused, nothing kept.
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64", feature = "wc"), all(feature = "net6", target_arch = "aarch64", feature = "desktop_firmware")))]
fn net_answer(ok: bool) {
    let Some(a) = NET_ASKING.lock().take() else { return };
    let word = match (ok, crate::video::dialog::last_button()) {
        (_, 0) => "always",
        (_, 1) => "once",
        (true, _) => "never",
        _ => "dismissed",
    };
    if word == "always" || word == "never" {
        crate::prefs::decide(crate::prefs::decide::NET, &a.app, "-", word); // REASK's one path (queue-only here)
    }
    let kept = crate::prefs::decisions().iter().any(|d| d.key == crate::prefs::decision_key(crate::prefs::decide::NET, &a.app));
    {
        let mut r = NET_RUN.lock();
        r.retain(|(t, n, _)| !(*t == a.task && *n == a.app));
        if r.len() >= NET_RUN_MAX {
            r.remove(0);
        }
        r.push((a.task, a.app.clone(), word == "always" || word == "once"));
    }
    serial_println!("[netgrant] answer={} app={} dst={} kept={}", word, a.app, a.dst, kept as u8);
}

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// The asks posted since boot (`tests netgrant`).
pub fn net_asks() -> u32 {
    NET_ASKS.load(Ordering::Relaxed)
}

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// `tests netgrant`'s clean slate: `app`'s kept decision revoked, its run rows and any ask of its dropped.
pub fn net_forget(app: &str) {
    let k = crate::prefs::decision_key(crate::prefs::decide::NET, app);
    if crate::prefs::decisions().iter().any(|d| d.key == k) {
        let _ = crate::prefs::revoke(&k);
    }
    NET_RUN.lock().retain(|(_, n, _)| n != app);
    let mut g = NET_ASKING.lock();
    if g.as_ref().map(|x| x.app == app).unwrap_or(false) {
        *g = None;
    }
}

// ── NET6GRANT (LEDGER SR151, BP-security-17, BP-net-12, BP-bar-10/12) — TAIL-APPENDED: the aarch64 door, and the name ──
//
// M1 — aarch64's NET6 socket doors (`net6_sys_connect`, `net6_sys_sendto` in `arch/aarch64/syscall.rs`) call
// [`net_gate_asid`] before `net_phy::net6::connect` / `sendto`, the same ONE check x86's `sys_connect`/`sys_sendto` call:
// [`net_gate_for`] is shared, so the decision order (the store's kept answer, this run's, then ONE ask) and the decision
// key (`system.decisions.net.<app>`, per app) are x86's, unchanged. Only "who asks" is per arch: on aarch64 the
// compositor owner IS the ASID (`wm::owner_of_launch` is the identity here), so the program is `wm::app_name_of(asid)`,
// else `argv[0]` on the caller's args page (`USER_ARGS_VA_ARM`, kernel-written, read through the bound-checked
// `copy_from_user`); neither = a kernel fixture blob (the NET6 EL0 fixture), not asked — x86's rule.
//
// M2 — the ask names the host: aarch64's `sys_resolve` records each answer here ([`net_name_note`]: ip -> the name the
// program asked for, the last [`NET_NAMES_MAX`] answers, newest wins for an ip two names share). The ask then reads
// `<app> wants to reach <name> (<ip:port>)` and the wire `[net] grant app= dst=<name>(<ip:port>) decision=`; an ip
// nobody resolved stays `<ip:port>`. The name is display only: it never enters the decision key.

#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// SYS_RESOLVE answers remembered for the ask (oldest dropped).
const NET_NAMES_MAX: usize = 32;
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
/// `(ip, name)`, oldest first.
static NET_NAMES: crate::sync::Mutex<Vec<([u8; 4], String)>> = crate::sync::Mutex::new(Vec::new());

/// SYS_RESOLVE's record: `name` answered `ip`. try_lock (the IF-masked syscall path never spins; a busy lock costs the
/// ask its name, never the resolve).
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
pub fn net_name_note(name: &[u8], ip: [u8; 4]) {
    let Ok(s) = core::str::from_utf8(name) else { return };
    if s.is_empty() || ip == [0, 0, 0, 0] {
        return;
    }
    let Some(mut g) = NET_NAMES.try_lock() else { return };
    g.retain(|(i, _)| *i != ip);
    if g.len() >= NET_NAMES_MAX {
        g.remove(0);
    }
    g.push((ip, String::from(s)));
}

/// The name SYS_RESOLVE last answered `ip` with, if it is still among the remembered answers.
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
fn net_name_of(ip: [u8; 4]) -> Option<String> {
    NET_NAMES.try_lock()?.iter().rev().find(|(i, _)| *i == ip).map(|(_, n)| n.clone())
}

/// The destination the ask on the glass names (`tests netgrant`'s name leg), if an ask is up.
#[cfg(any(all(feature = "smolnet", target_arch = "x86_64"), all(feature = "net6", target_arch = "aarch64")))]
pub fn net_asking_dst() -> Option<String> {
    NET_ASKING.try_lock()?.as_ref().map(|a| a.dst.clone())
}

/// THE door's check on aarch64 (`net6_sys_connect` with `stream`, `net6_sys_sendto` without), for the caller's `asid`:
/// `Ok` = the stack may be entered; `Err(-EPERM)` refused; `Err(-EINPROGRESS | -EAGAIN)` asking, nothing queued.
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
pub fn net_gate_asid(asid: u64, ip: [u8; 4], port: u16, stream: bool) -> Result<(), i64> {
    let Some(app) = net_app_of_asid(asid) else { return Ok(()) };
    let task = crate::arch::sched::current_id().unwrap_or(0);
    net_gate_for(&app, task, asid, ip, port, stream)
}

/// The program the caller's address space runs, by the launcher's name (the note above); `None` = a kernel fixture blob.
#[cfg(all(feature = "net6", target_arch = "aarch64"))]
fn net_app_of_asid(asid: u64) -> Option<String> {
    if asid == 0 {
        return None;
    }
    let mut nb = [0u8; crate::video::wm::MAX_TITLE];
    let n = crate::video::wm::app_name_of(asid, &mut nb);
    if n > 0 {
        return core::str::from_utf8(&nb[..n]).ok().map(String::from);
    }
    // The caller's TTBR0 is live (syscall context); its args page is a legal read source (`xwin::range_ok`).
    let va = una_abi::USER_ARGS_VA_ARM;
    let mut page = alloc::vec![0u8; 4096];
    crate::arch::syscall::copy_from_user(&mut page, va, 4096).ok()?;
    let a = una_abi::Args::parse(&page[..], va)?;
    let w = core::str::from_utf8(a.get(0)?).ok()?;
    let p = crate::video::wm::program_name(w);
    if p.is_empty() { None } else { Some(String::from(p)) }
}

// ── UACMIC (rmbp-ledger B723, BP-bar-12, R130) — the microphone is a grant, the camera's ask ──────────────────────────
// ONE domain word for REASK's store: a ring-3 program reaches `audio::source` only with `system.decisions.microphone.
// <app>` kept as `allow` (the same ask as the camera: `<app> wants the microphone`, Not now · Allow); the kernel's own
// recorder fixture names `audio::KERNEL_RECORDER` and is not asked. Settings > Privacy lists it with every decision.
/// The decisions' `<domain>` segment for the microphone (`system.decisions.microphone.*`).
pub const MIC_DOMAIN: &str = "microphone";
