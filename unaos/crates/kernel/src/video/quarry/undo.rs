//! CHARTER: Matrix — kernel-by-ruling R50
//! (Quarry is the Finder by R50 — `dragdrop.rs`'s header, the same owner; the arc wrote "Kernel — quarry", a seam GATE-CHARTER does not carry.)
//!
//! UNDOMOVE (rmbp-ledger B716; BP-bar-14, research B §6: every file operation the user makes is undoable with one
//! key, the way text is) — Quarry's ONE undo journal. Per session, in RAM, at most [`DEPTH`] entries. A child module
//! of [`super`] (`live`) like `ops`/`dragdrop`, so it reads the model without widening it; the `video::quarry` seam
//! re-exports it as `video::quarry::undo`.
//!
//! * Every user verb that changes the namespace runs through ONE wrapper here ([`rename`] — a move when the folder
//!   changes, a rename when it does not —, [`copy`] — a duplicate when the folder is the source's own —, [`mkdir`],
//!   [`trash`], [`restore`]), which runs the SAME op body the menu ran before (`ops::op_*`, `fs::trash::*`) and, on
//!   success, pushes `{verb, from, to, inode, when}`. A push clears the redo stack.
//! * ⌘Z (`Action::Undo`, [`undo`]) pops the newest entry and applies its inverse: a move/rename renames back, a Trash restores
//!   (trash_core's own `restore`), a Restore goes back to the Trash, a copy's copy goes to the Trash, a new folder is
//!   removed while it is still empty. ⌘⇧Z (`Action::Redo`, [`redo`]) re-applies the verb. The inverse runs the op bodies
//!   directly, never the wrappers, so applying it never pushes.
//! * An inverse that no longer applies — the object is gone, its inode is not the one the verb left, the name it
//!   would take is occupied, the folder is no longer empty, the Trash no longer holds it — is REFUSED: a toast says
//!   why, and the entry is dropped (it can never apply again).
//! * The Trash toast carries `Undo` (notify `ACT_UNDO`, the entry's seq as its argument; TOASTCAP's 5 s card); a press
//!   undoes that entry while it is still the newest. The Edit menu's Undo/Redo rows read [`menu_label`].
//!
//! * TRASHTOAST (B747): a Trash verb over several items is ONE entry and ONE toast ([`batch_begin`]/[`batch_end`]).
//!
//! Wire: `[quarry] undo push verb=<v> from=<p> to=<p> depth=<n> at_ms=<t>`, `[quarry] undo verb=<v> from=<p> to=<p>
//! -> ok|refused:<why>`, `[quarry] redo verb=<v> from=<p> to=<p> -> ok|refused:<why>`. `tests undomove` (the
//! `tests.rs` tail) → `:: UNDOMOVE: pushed=4 undone=4 redone=4 refused=1 leftovers=0 -> PASS ::`.
//!
//! Lock discipline: `JOURNAL` is a leaf; an entry is popped under it, applied with no lock held, and pushed back
//! (onto the other stack) under it again. The listing refresh re-takes `MODEL` afterwards, as `ops` does.

use super::*;
use crate::fs::vfs::{MountTable, KERNEL_PRINCIPAL};
use core::sync::atomic::{AtomicU64, AtomicU8};

/// The journal's depth (per session, RAM only).
pub const DEPTH: usize = 64;

/// A namespace verb Quarry can undo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    Move,
    Rename,
    Copy,
    Duplicate,
    NewFolder,
    Trash,
    Restore,
}

impl Verb {
    /// The wire token.
    pub const fn name(self) -> &'static str {
        match self {
            Verb::Move => "move",
            Verb::Rename => "rename",
            Verb::Copy => "copy",
            Verb::Duplicate => "duplicate",
            Verb::NewFolder => "new-folder",
            Verb::Trash => "trash",
            Verb::Restore => "restore",
        }
    }
    /// The words a person reads (`Undo <label>`, `Can't undo <label>`).
    pub const fn label(self) -> &'static str {
        match self {
            Verb::Move => "Move",
            Verb::Rename => "Rename",
            Verb::Copy => "Copy",
            Verb::Duplicate => "Duplicate",
            Verb::NewFolder => "New Folder",
            Verb::Trash => "Trash",
            Verb::Restore => "Restore",
        }
    }
    const fn code(self) -> u8 {
        match self {
            Verb::Move => 1,
            Verb::Rename => 2,
            Verb::Copy => 3,
            Verb::Duplicate => 4,
            Verb::NewFolder => 5,
            Verb::Trash => 6,
            Verb::Restore => 7,
        }
    }
}

/// The Edit menu's rows, composed once (the painter runs per scanline: no allocation there). Index = `code - 1`.
const UNDO_LABELS: [&str; 7] = ["Undo Move", "Undo Rename", "Undo Copy", "Undo Duplicate", "Undo New Folder", "Undo Trash", "Undo Restore"];
const REDO_LABELS: [&str; 7] = ["Redo Move", "Redo Rename", "Redo Copy", "Redo Duplicate", "Redo New Folder", "Redo Trash", "Redo Restore"];
/// The widest of them, in glyphs — the Edit dropdown is sized for it so a live label never clips.
pub const LABEL_MAX: usize = 15;
const _: () = assert!(UNDO_LABELS[4].len() == LABEL_MAX && REDO_LABELS[4].len() == LABEL_MAX);

/// One journal entry: what the verb did, forward. `from`/`to` per verb: Move/Rename/Copy/Duplicate = the source and
/// the path the verb produced; NewFolder = `-` and the folder; Trash = the original path and its name in the Trash;
/// Restore = the name in the Trash and the original path. `held` is the Trash name an inverse parked a copy under
/// (Copy/Duplicate undone), so its redo restores the same object.
#[derive(Clone)]
pub struct Entry {
    pub seq: u64,
    pub verb: Verb,
    pub from: String,
    pub to: String,
    /// The object's identity after the verb (`Stat::id`; `None` on FAT, where only presence is checked).
    pub inode: Option<u64>,
    /// Uptime ms at the push.
    pub when: u64,
    held: String,
    /// TRASHTOAST (B747): a Trash verb over several items is ONE entry — `(original path, name in the Trash)` per
    /// item, in the order trashed (empty for every other entry, and for a one-item Trash).
    members: Vec<(String, String)>,
}

/// The two stacks. `undo` newest last; `redo` newest last.
#[derive(Default)]
pub struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

static JOURNAL: crate::sync::Mutex<Journal> = crate::sync::Mutex::new(Journal { undo: Vec::new(), redo: Vec::new() });
static SEQ: AtomicU64 = AtomicU64::new(0);
/// The newest entry's verb code on each stack (0 = empty), mirrored for the menu painter (no lock per scanline).
static TOP_UNDO: AtomicU8 = AtomicU8::new(0);
static TOP_REDO: AtomicU8 = AtomicU8::new(0);

fn mirror(j: &Journal) {
    TOP_UNDO.store(j.undo.last().map_or(0, |e| e.verb.code()), Ordering::Relaxed);
    TOP_REDO.store(j.redo.last().map_or(0, |e| e.verb.code()), Ordering::Relaxed);
}

fn mt() -> MountTable {
    crate::shell::vfs_mount_table()
}

fn id_of(t: &MountTable, p: &str) -> Option<u64> {
    t.stat(p).ok().and_then(|s| s.id)
}

/// Push a verb's entry (a new verb clears the redo stack). Returns the entry's seq.
fn push(verb: Verb, from: &str, to: &str, inode: Option<u64>) -> u64 {
    push_with(verb, from, to, inode, Vec::new())
}

fn push_with(verb: Verb, from: &str, to: &str, inode: Option<u64>, members: Vec<(String, String)>) -> u64 {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let when = crate::arch::ms();
    let depth = {
        let mut j = JOURNAL.lock();
        if j.undo.len() >= DEPTH {
            j.undo.remove(0);
        }
        j.undo.push(Entry { seq, verb, from: String::from(from), to: String::from(to), inode, when, held: String::new(), members });
        j.redo.clear();
        mirror(&j);
        j.undo.len()
    };
    serial_println!("[quarry] undo push verb={} from={} to={} depth={} at_ms={}", verb.name(), from, to, depth, when);
    seq
}

// ── the verbs (the wrappers the menu, the keys and the drop call) ──────────────────────────────────────────────

/// Move or rename `src` to `dst` (`ops::op_rename`); a different folder is a Move, the same folder a Rename.
pub fn rename(src: &str, dst: &str) -> Result<(), String> {
    ops::op_rename(src, dst)?;
    let verb = if parent(src).eq_ignore_ascii_case(&parent(dst)) { Verb::Rename } else { Verb::Move };
    push(verb, src, dst, id_of(&mt(), dst));
    Ok(())
}

/// Copy `src` into folder `dir` (`ops::op_copy`); into its own folder it is a Duplicate. Returns the new path.
pub fn copy(src: &str, dir: &str) -> Result<String, String> {
    let dst = ops::op_copy(src, dir)?;
    let verb = if parent(src).eq_ignore_ascii_case(dir) { Verb::Duplicate } else { Verb::Copy };
    push(verb, src, &dst, id_of(&mt(), &dst));
    Ok(dst)
}

/// A new folder at `path` (`ops::op_mkdir`).
pub fn mkdir(path: &str) -> Result<(), String> {
    ops::op_mkdir(path)?;
    push(Verb::NewFolder, "-", path, id_of(&mt(), path));
    Ok(())
}

/// Move `path` to the Trash (`fs::trash::trash`, trash_core). Returns the trashed name. The toast carries `Undo`.
/// Inside a [`batch_begin`]/[`batch_end`] pair the item is recorded silently: the batch is ONE entry and ONE toast.
pub fn trash(path: &str) -> Result<String, String> {
    let name = crate::fs::trash::trash(path)?;
    {
        let mut b = BATCH.lock();
        if let Some(v) = b.as_mut() {
            v.push((String::from(path), name.clone()));
            let n = v.len();
            drop(b);
            serial_println!("[quarry] trash batch add path={} name={} n={}", path, name, n);
            return Ok(name);
        }
    }
    let seq = push(Verb::Trash, path, &name, None);
    trash_toast(b"Moved to Trash", leaf(path).as_bytes(), seq);
    Ok(name)
}

// ── TRASHTOAST (rmbp-ledger B747; crawl A 26) — one toast and one entry per Trash verb ──────────────────────────
// A verb over N items (`ops::do_trash` over the selection) opens a batch; each `trash` records `(path, name)`
// silently; `batch_end` pushes ONE entry (a one-item batch is exactly the classic entry and toast) and posts ONE
// toast whose `Undo` (the entry's seq, BP-video-29) restores every item, newest first. `BATCH` is a leaf lock.

static BATCH: crate::sync::Mutex<Option<Vec<(String, String)>>> = crate::sync::Mutex::new(None);
/// Trash toasts posted (every one goes through [`trash_toast`]) — `tests trashtoast`'s `toasts=`.
static TRASH_TOASTS: AtomicU64 = AtomicU64::new(0);

fn trash_toast(title: &[u8], line: &[u8], seq: u64) {
    TRASH_TOASTS.fetch_add(1, Ordering::Relaxed);
    let _ = crate::video::notify::post_full(b"Quarry", title, line, b"Undo", crate::video::notify::ACT_UNDO, &seq.to_le_bytes());
}

/// Trash toasts posted so far this boot.
pub fn trash_toasts() -> u64 {
    TRASH_TOASTS.load(Ordering::Relaxed)
}

/// Open a Trash batch (an already-open batch stays the one batch).
pub fn batch_begin() {
    let mut b = BATCH.lock();
    if b.is_none() {
        *b = Some(Vec::new());
    }
}

/// Close the batch: nothing trashed = nothing; one item = the classic entry and toast; N items = ONE entry and ONE
/// toast `Moved <n> items to Trash` / `Undo`. Returns the entry's seq (`None` = nothing was trashed).
pub fn batch_end() -> Option<u64> {
    let v = BATCH.lock().take()?;
    match v.len() {
        0 => None,
        1 => {
            let (path, name) = &v[0];
            let seq = push(Verb::Trash, path, name, None);
            trash_toast(b"Moved to Trash", leaf(path).as_bytes(), seq);
            Some(seq)
        }
        n => {
            let first = v[0].0.clone();
            let title = alloc::format!("Moved {} items to Trash", n);
            let line = leaf(&parent(&first));
            let seq = push_with(Verb::Trash, &first, &alloc::format!("items:{}", n), None, v);
            trash_toast(title.as_bytes(), line.as_bytes(), seq);
            serial_println!("[quarry] trash batch items={} toasts=1 seq={}", n, seq);
            Some(seq)
        }
    }
}

/// Restore the trashed `name` to its original path (`fs::trash::restore`). Returns that path.
pub fn restore(name: &str) -> Result<String, String> {
    let orig = crate::fs::trash::restore(name)?;
    push(Verb::Restore, name, &orig, None);
    Ok(orig)
}

// ── applying an entry ───────────────────────────────────────────────────────────────────────────────────────────

/// `true` = the inverse (undo), `false` = the verb again (redo). `Err(why)` = it no longer applies.
fn apply(e: &mut Entry, back: bool) -> Result<(), &'static str> {
    let t = mt();
    let same = |p: &str, id: Option<u64>| match (id, id_of(&t, p)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    match e.verb {
        Verb::Move | Verb::Rename => {
            let (now, then) = if back { (e.to.clone(), e.from.clone()) } else { (e.from.clone(), e.to.clone()) };
            if t.stat(&now).is_err() {
                return Err("gone");
            }
            if !same(&now, e.inode) {
                return Err("changed");
            }
            if !now.eq_ignore_ascii_case(&then) && t.stat(&then).is_ok() { // crawl A 28: a case-only rename finds itself on a case-insensitive volume
                return Err("occupied");
            }
            ops::op_rename(&now, &then).map_err(|_| "rename-failed")
        }
        Verb::Copy | Verb::Duplicate => {
            if back {
                if t.stat(&e.to).is_err() {
                    return Err("gone");
                }
                if !same(&e.to, e.inode) {
                    return Err("changed");
                }
                e.held = crate::fs::trash::trash(&e.to).map_err(|_| "trash-refused")?;
                Ok(())
            } else {
                if e.held.is_empty() {
                    return Err("not-in-trash");
                }
                if t.stat(&e.to).is_ok() {
                    return Err("occupied");
                }
                let p = crate::fs::trash::restore(&e.held).map_err(|_| "not-in-trash")?;
                e.held.clear();
                e.inode = id_of(&t, &p);
                Ok(())
            }
        }
        Verb::NewFolder => {
            if back {
                let st = t.stat(&e.to).map_err(|_| "gone")?;
                if !matches!(st.kind, NodeKind::Dir) || !same(&e.to, e.inode) {
                    return Err("changed");
                }
                let full = t.read_dir(&e.to).map(|v| v.iter().any(|d| d.name != "." && d.name != "..")).unwrap_or(true);
                if full {
                    return Err("not-empty");
                }
                t.remove_dir(&e.to, KERNEL_PRINCIPAL).map_err(|_| "remove-failed")
            } else {
                if t.stat(&e.to).is_ok() {
                    return Err("occupied");
                }
                ops::op_mkdir(&e.to).map_err(|_| "mkdir-failed")?;
                e.inode = id_of(&t, &e.to);
                Ok(())
            }
        }
        Verb::Trash if !e.members.is_empty() => {
            // TRASHTOAST (B747): the whole verb — every original must be free / present before anything moves.
            if back {
                if e.members.iter().any(|(orig, _)| t.stat(orig).is_ok()) {
                    return Err("occupied");
                }
                let mut ok = true;
                for (_, name) in e.members.iter().rev() {
                    ok &= crate::fs::trash::restore(name).is_ok();
                }
                if ok { Ok(()) } else { Err("not-in-trash") }
            } else {
                if e.members.iter().any(|(orig, _)| t.stat(orig).is_err()) {
                    return Err("gone");
                }
                let mut ok = true;
                for (orig, name) in e.members.iter_mut() {
                    match crate::fs::trash::trash(orig) {
                        Ok(n) => *name = n,
                        Err(_) => ok = false,
                    }
                }
                if ok { Ok(()) } else { Err("trash-refused") }
            }
        }
        Verb::Trash => {
            if back {
                if t.stat(&e.from).is_ok() {
                    return Err("occupied");
                }
                crate::fs::trash::restore(&e.to).map(|_| ()).map_err(|_| "not-in-trash")
            } else {
                if t.stat(&e.from).is_err() {
                    return Err("gone");
                }
                e.to = crate::fs::trash::trash(&e.from).map_err(|_| "trash-refused")?;
                Ok(())
            }
        }
        Verb::Restore => {
            if back {
                if t.stat(&e.to).is_err() {
                    return Err("gone");
                }
                e.from = crate::fs::trash::trash(&e.to).map_err(|_| "trash-refused")?;
                Ok(())
            } else {
                if t.stat(&e.to).is_ok() {
                    return Err("occupied");
                }
                crate::fs::trash::restore(&e.from).map(|_| ()).map_err(|_| "not-in-trash")
            }
        }
    }
}

/// The sentence the refusal toast shows for a `why` token.
fn why_text(why: &str) -> &'static str {
    match why {
        "gone" => "the item is no longer where the change left it",
        "changed" => "the item was replaced since",
        "occupied" => "another item now has that name",
        "not-empty" => "the folder has items in it now",
        "not-in-trash" => "the Trash no longer holds it",
        "trash-refused" => "the Trash refused it",
        "nothing" => "there is nothing to undo",
        "not-latest" => "a later change came after it",
        _ => "the file system refused it",
    }
}

fn refused_toast(verb: Verb, why: &str, back: bool) {
    let title = alloc::format!("Can't {} {}", if back { "undo" } else { "redo" }, verb.label());
    let _ = crate::video::notify::post_full(b"Quarry", title.as_bytes(), why_text(why).as_bytes(), b"", crate::video::notify::ACT_NONE, &[]);
}

/// Re-read the shown folder and say what happened in the status line (Quarry may be closed: then nothing).
fn refresh(said: String) {
    if let Some(m) = MODEL.lock().as_mut() {
        let c = m.cwd.clone();
        m.invalidate();
        m.show(&c);
        m.status = Some(said);
        m.settle();
    }
}

/// Pop the newest entry of one stack (when `seq` is given, only if it is that entry), apply it, and move it to the
/// other stack — or drop it, refused. Returns the verb, or the refusal token.
fn step(back: bool, seq: Option<u64>) -> Result<Verb, &'static str> {
    let op = if back { "undo" } else { "redo" };
    let popped = {
        let mut j = JOURNAL.lock();
        let st = if back { &mut j.undo } else { &mut j.redo };
        match (st.last().map(|e| e.seq), seq) {
            (None, _) => None,
            (Some(top), Some(want)) if top != want => Some(Err(st.last().map(|e| e.verb).unwrap_or(Verb::Move))),
            _ => st.pop().map(Ok),
        }
    };
    let mut e = match popped {
        None => {
            serial_println!("[quarry] {} verb=- from=- to=- -> refused:nothing", op);
            return Err("nothing");
        }
        Some(Err(v)) => {
            serial_println!("[quarry] {} verb={} from=- to=- -> refused:not-latest", op, v.name());
            refused_toast(v, "not-latest", back);
            return Err("not-latest");
        }
        Some(Ok(e)) => e,
    };
    let r = apply(&mut e, back);
    let verb = e.verb;
    match r {
        Ok(()) => {
            serial_println!("[quarry] {} verb={} from={} to={} -> ok", op, verb.name(), e.from, e.to);
            let mut j = JOURNAL.lock();
            if back { j.redo.push(e) } else { j.undo.push(e) }
            mirror(&j);
            drop(j);
            refresh(alloc::format!("{} {}", if back { "undid" } else { "redid" }, verb.label()));
            repaint();
            Ok(verb)
        }
        Err(why) => {
            serial_println!("[quarry] {} verb={} from={} to={} -> refused:{}", op, verb.name(), e.from, e.to, why);
            mirror(&JOURNAL.lock());
            refused_toast(verb, why, back);
            refresh(alloc::format!("{} {} refused ({})", op, verb.label(), why_text(why)));
            repaint();
            Err(why)
        }
    }
}

/// ⌘Z (`Action::Undo`) / Edit > Undo: the newest entry's inverse.
pub fn undo() -> Result<Verb, &'static str> {
    step(true, None)
}

/// ⌘⇧Z (`Action::Redo`) / Edit > Redo: the newest undone entry again.
pub fn redo() -> Result<Verb, &'static str> {
    step(false, None)
}

/// The Trash toast's `Undo` (notify `ACT_UNDO`, `arg` = the entry's seq LE): undo THAT entry while it is the newest.
/// Returns the notify witness token.
pub fn from_toast(arg: &[u8]) -> &'static str {
    let mut b = [0u8; 8];
    let k = arg.len().min(8);
    b[..k].copy_from_slice(&arg[..k]);
    match step(true, Some(u64::from_le_bytes(b))) {
        Ok(_) => "quarry-undone",
        Err(_) => "quarry-undo-refused",
    }
}

/// The Edit menu row's live label: `Undo <verb>` / `Redo <verb>` while Quarry holds the keyboard and the stack is
/// non-empty, else `None` (the row stays the greyed default).
pub fn menu_label(redo: bool) -> Option<&'static str> {
    let code = if redo { TOP_REDO.load(Ordering::Relaxed) } else { TOP_UNDO.load(Ordering::Relaxed) };
    if code == 0 || !is_open() || wm::focus_asid() != OWNER {
        return None;
    }
    let t = if redo { &REDO_LABELS } else { &UNDO_LABELS };
    t.get(code as usize - 1).copied()
}

/// Entries on (undo, redo).
pub fn depth() -> (usize, usize) {
    let j = JOURNAL.lock();
    (j.undo.len(), j.redo.len())
}

/// `tests undomove`: set the session's journal aside (the fixture runs on an empty one) …
pub fn fixture_take() -> Journal {
    let mut j = JOURNAL.lock();
    let out = core::mem::take(&mut *j);
    mirror(&j);
    out
}

/// … and put it back afterwards (the fixture's own entries are discarded).
pub fn fixture_put(saved: Journal) {
    let mut j = JOURNAL.lock();
    *j = saved;
    mirror(&j);
}

/// crawl A 1 — Log Out (`desktopbuild::teardown`): the journal is the SESSION's. Both stacks dropped, so the next
/// user's ⌘Z / Edit > Undo can never replay the previous user's renames, Trash restores or folder removals on
/// their paths. The seq counter keeps counting (an old toast's `Undo` then finds no entry: `refused:nothing`).
pub fn clear() {
    let mut j = JOURNAL.lock();
    *j = Journal::default();
    mirror(&j);
    drop(j);
    *BATCH.lock() = None;
}

/// TRASHBATCHDOCK (rmbp-ledger B762) — the Trash was emptied (the dock's Empty Trash): drop every entry whose replay
/// would reach into the Trash, since nothing there can come back. Undo stack: every Trash (one item or a TRASHTOAST
/// batch). Redo stack: every Restore (its redo restores from the Trash) and every Copy/Duplicate whose undone copy is
/// parked there (`held`). An undone Trash — its items back where they were — stays redoable. Returns how many dropped.
pub fn drop_trash_entries() -> usize {
    let mut j = JOURNAL.lock();
    let before = j.undo.len() + j.redo.len();
    j.undo.retain(|e| e.verb != Verb::Trash);
    j.redo.retain(|e| !(e.verb == Verb::Restore || (matches!(e.verb, Verb::Copy | Verb::Duplicate) && !e.held.is_empty())));
    mirror(&j);
    before - (j.undo.len() + j.redo.len())
}
