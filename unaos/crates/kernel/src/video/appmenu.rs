// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// APPMENU (R73, arc (a)) — the kernel's registry of ring-3-published menus, the neighbour of `wm.rs`.
//
// An app publishes a menu TREE over the frozen bus (`BUS_VERB_MENU_PUBLISH`, body = header + 40-byte
// wire items, `una_abi`); this registry holds at most one tree per OWNER, keyed by the kernel-derived
// owner id (`row + 1`, the same key `wm` keys windows by) — NEVER by anything the frame says. The bar
// (`winmenu`) draws it through its existing tenant path; a pick comes back to the owner by identity
// (slot k's owner, fixed at publish), never to the focused slot — leg 1 of the ledger in
// `video/menubar.rs` (THE MENU PROTOCOL). Refusals are WHOLE (caps are refusals, not truncations).
//
// Locking: the owner words are lock-free atomics (`reap` runs from `close_owner`, interrupts masked, and
// touches nothing else); the tree bytes sit behind a mutex taken with `try_lock` only.
//
// Known limits, stated rather than hidden: (1) a tree is attached to the bar for the owner's windows
// that EXIST at publish time (a window created later picks it up on the next publish); (2) the bar's
// tables want `&'static` rows, so each DISTINCT tree is leaked once — bounded by `LEAK_MAX`, identical
// re-publishes cost nothing; (3) x86-only, the board the input ring and bus dispatch live on.
#![cfg(target_arch = "x86_64")]

use super::winmenu::{self, MenuItem, MenuTitle, MENU_TITLES_MAX};
use super::wm;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use una_abi::{
    MenuWireItem, INPUT_EV_MENU_PICK, MENU_FLAGS_KNOWN, MENU_FLAG_SUBMENU, MENU_HDR_LEN, MENU_ITEMS_MAX,
    MENU_ITEM_LEN, MENU_LABEL_MAX, MENU_WIRE_VERSION,
};

// Negative errnos, as the bus reply's status carries them (the syscall file's own values).
const E_PERM: i64 = -1;
const E_AGAIN: i64 = -11;
const E_INVAL: i64 = -22;

/// Registry slots: one per winmenu slot, so every published owner can reach the bar.
const SLOTS: usize = 4;
/// Distinct trees that may be leaked into `'static` rows over a boot.
const LEAK_MAX: u32 = 64;

#[derive(Clone, Copy)]
struct Entry {
    count: usize,
    items: [MenuWireItem; MENU_ITEMS_MAX],
    titles: Option<&'static [MenuTitle]>,
}
const EMPTY: Entry = Entry { count: 0, items: [MenuWireItem::ZERO; MENU_ITEMS_MAX], titles: None };

/// The owner of each slot (0 = free). Lock-free; the claim and the reap are compare-exchanges.
static OWN: [AtomicU64; SLOTS] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
/// Bit `id-1` set = window `id` of that slot's owner was handed to `winmenu::publish`.
static WINS: [AtomicU64; SLOTS] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static TREES: spin::Mutex<[Entry; SLOTS]> = spin::Mutex::new([EMPTY; SLOTS]);
static LEAKS: AtomicU32 = AtomicU32::new(0);

const _: () = assert!(wm::MAX_WINDOWS <= 64); // `WINS` is one word per slot

/// The kernel-derived owner id of the calling bus row, or `None` for the shared window / a bad row.
fn owner_of_row(row: usize) -> Option<u64> {
    if row < crate::arch::memory::USER_SLOTS { Some(row as u64 + 1) } else { None }
}

fn slot_of(asid: u64) -> Option<usize> {
    (0..SLOTS).find(|&k| OWN[k].load(Ordering::Acquire) == asid)
}

/// Validate a publish body and return `(count, depth)`. WHOLE refusal, with the reason named.
fn validate(body: &[u8]) -> Result<(usize, usize), &'static str> {
    if body.len() < MENU_HDR_LEN {
        return Err("shape");
    }
    if body[0] != MENU_WIRE_VERSION || body[2] != 0 || body[3] != 0 {
        return Err("version");
    }
    let count = body[1] as usize;
    if count > MENU_ITEMS_MAX {
        return Err("items-cap");
    }
    if count == 0 {
        return Err("empty");
    }
    if body.len() != MENU_HDR_LEN + count * MENU_ITEM_LEN {
        return Err("shape");
    }
    let mut titles = 0usize;
    let mut any_child = false;
    for i in 0..count {
        let it = match MenuWireItem::from_bytes(&body[MENU_HDR_LEN + i * MENU_ITEM_LEN..][..MENU_ITEM_LEN]) {
            Some(it) => it,
            None => return Err("shape"),
        };
        let n = it.label_len as usize;
        if n > MENU_LABEL_MAX {
            return Err("label-cap");
        }
        if it.label[..n].iter().any(|&b| !(0x20..0x7f).contains(&b)) || it.label[n..].iter().any(|&b| b != 0) || it._pad != [0; 3] {
            return Err("label-bytes");
        }
        if it.flags & !MENU_FLAGS_KNOWN != 0 {
            return Err("flags");
        }
        if it.parent == 0 {
            // A top-level item is a TITLE: a nonzero id, a submenu, a name.
            if it.flags & MENU_FLAG_SUBMENU == 0 || it.id == 0 || n == 0 {
                return Err("title");
            }
            titles += 1;
        } else {
            any_child = true;
            if it.flags & MENU_FLAG_SUBMENU != 0 {
                return Err("depth");
            }
            let mut found = false;
            for j in 0..count {
                let p = MenuWireItem::from_bytes(&body[MENU_HDR_LEN + j * MENU_ITEM_LEN..][..MENU_ITEM_LEN]);
                if let Some(p) = p {
                    if p.parent == 0 && p.id == it.parent {
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return Err("orphan");
            }
        }
    }
    if titles == 0 || titles > MENU_TITLES_MAX {
        return Err("titles-cap");
    }
    Ok((count, if any_child { 2 } else { 1 }))
}

/// Flatten the validated wire items into the bar's `'static` rows (leaked once per distinct tree).
fn build_titles(items: &[MenuWireItem]) -> &'static [MenuTitle] {
    let leak_str = |it: &MenuWireItem| -> &'static str {
        let s = core::str::from_utf8(&it.label[..it.label_len as usize]).unwrap_or("");
        Box::leak(alloc::string::String::from(s).into_boxed_str())
    };
    let mut titles: Vec<MenuTitle> = Vec::new();
    for t in items.iter().filter(|t| t.parent == 0) {
        let mut rows: Vec<MenuItem> = Vec::new();
        for c in items.iter().filter(|c| c.parent == t.id) {
            // winmenu's flag bits 0..2 match the wire's; bit 3 there is FLAG_APPNAME, so it is masked.
            rows.push(MenuItem { id: c.id, label: leak_str(c), flags: c.flags & 0b111 });
        }
        titles.push(MenuTitle { label: leak_str(t), items: Box::leak(rows.into_boxed_slice()) });
    }
    Box::leak(titles.into_boxed_slice())
}

macro_rules! pick_fns {
    ($($f:ident => $k:expr),*) => { $(fn $f(id: u32) { pick_slot($k, id) })* };
}
pick_fns!(pick0 => 0, pick1 => 1, pick2 => 2, pick3 => 3);
/// The bar's pick sink for slot `k`: a bare `fn` cannot carry an owner, so each slot has its own — the
/// owner is a property of the slot, fixed when the tree was published, not of who has focus.
static PICK_FNS: [fn(u32); SLOTS] = [pick0, pick1, pick2, pick3];

/// Deliver a pick to slot `k`'s OWNER's input ring, by identity.
fn pick_slot(k: usize, id: u32) {
    let asid = OWN[k].load(Ordering::Acquire);
    let delivered = asid != 0
        && crate::arch::x86_64::syscall::user_input_push_owner(asid, una_abi::input_ev_pack(INPUT_EV_MENU_PICK, id as u64));
    serial_println!("[menubar] pick owner={} item={} delivered={}", asid, id, delivered);
}

/// Hand `asid`'s tree to the bar for each window that owner has right now; returns `(attached, seen)`.
fn attach(k: usize, asid: u64, titles: &'static [MenuTitle]) -> (u32, u32) {
    let (mut ok, mut seen) = (0u32, 0u32);
    for id in 1..=(wm::MAX_WINDOWS as u32) {
        if wm::owner_of(id) == Some(asid) {
            seen += 1;
            if winmenu::publish(id, titles, PICK_FNS[k]) {
                ok += 1;
                WINS[k].fetch_or(1u64 << (id - 1), Ordering::AcqRel);
            }
        }
    }
    (ok, seen)
}

/// `BUS_VERB_MENU_PUBLISH` for the calling row. Returns the reply status (0 = taken).
pub fn verb_publish(row: usize, body: &[u8]) -> i64 {
    let Some(asid) = owner_of_row(row) else { return E_PERM };
    let (count, depth) = match validate(body) {
        Ok(v) => v,
        Err(why) => {
            serial_println!(":: APPMENU: verb=publish owner={} reason={} -> REFUSED ::", asid, why);
            return E_INVAL;
        }
    };
    let mut items = [MenuWireItem::ZERO; MENU_ITEMS_MAX];
    for i in 0..count {
        if let Some(it) = MenuWireItem::from_bytes(&body[MENU_HDR_LEN + i * MENU_ITEM_LEN..][..MENU_ITEM_LEN]) {
            items[i] = it;
        }
    }
    // Claim (or find) this owner's slot.
    let k = match slot_of(asid) {
        Some(k) => k,
        None => match (0..SLOTS).find(|&k| OWN[k].compare_exchange(0, asid, Ordering::AcqRel, Ordering::Acquire).is_ok()) {
            Some(k) => k,
            None => {
                serial_println!(":: APPMENU: verb=publish owner={} reason=registry-full -> REFUSED ::", asid);
                return E_AGAIN;
            }
        },
    };
    let mut g = match TREES.try_lock() {
        Some(g) => g,
        None => {
            return E_AGAIN; // (a fresh claim stays; the app's retry fills it, and `reap` frees it)
        }
    };
    let same = g[k].count == count && g[k].titles.is_some() && g[k].items[..count] == items[..count];
    let titles = if same {
        g[k].titles.unwrap_or(&[])
    } else {
        if LEAKS.load(Ordering::Relaxed) >= LEAK_MAX {
            serial_println!(":: APPMENU: verb=publish owner={} reason=leak-cap -> REFUSED ::", asid);
            if g[k].titles.is_none() {
                OWN[k].store(0, Ordering::Release); // a claim that never got a tree is not an owner
            }
            return E_AGAIN;
        }
        LEAKS.fetch_add(1, Ordering::Relaxed);
        build_titles(&items[..count])
    };
    g[k] = Entry { count, items, titles: Some(titles) };
    drop(g);
    let (bar, seen) = attach(k, asid, titles);
    serial_println!(":: APPMENU: verb=publish owner={} items={} depth={} bar={}/{} -> PASS ::", asid, count, depth, bar, seen);
    serial_println!(":: APPMENU: owner={} items={} published=1 -> PASS ::", asid, count);
    0
}

/// Drop `asid`'s entry and its bar trees. Lock-free (the reap path runs with interrupts masked).
fn drop_owner(asid: u64) -> bool {
    let Some(k) = slot_of(asid) else { return false };
    let wins = WINS[k].swap(0, Ordering::AcqRel);
    for b in 0..wm::MAX_WINDOWS {
        if wins & (1u64 << b) != 0 {
            winmenu::clear(b as u32 + 1);
        }
    }
    OWN[k].compare_exchange(asid, 0, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// `BUS_VERB_MENU_CLEAR` for the calling row.
pub fn verb_clear(row: usize, body: &[u8]) -> i64 {
    let Some(asid) = owner_of_row(row) else { return E_PERM };
    if !body.is_empty() {
        return E_INVAL;
    }
    let had = drop_owner(asid);
    serial_println!(":: APPMENU: verb=clear owner={} had={} -> PASS ::", asid, had);
    0
}

/// `BUS_VERB_MENU_GET`: body empty (own tree) or 8 bytes LE (that owner's). Fills `out` with the tree in
/// the publish shape, or leaves it EMPTY when the owner has none — never a stale or dead tree.
pub fn verb_get(row: usize, body: &[u8], out: &mut Vec<u8>) -> i64 {
    let Some(me) = owner_of_row(row) else { return E_PERM };
    let target = match body.len() {
        0 => me,
        8 => u64::from_le_bytes([body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7]]),
        _ => return E_INVAL,
    };
    let Some(k) = slot_of(target) else { return 0 };
    let g = match TREES.try_lock() {
        Some(g) => g,
        None => return E_AGAIN,
    };
    if OWN[k].load(Ordering::Acquire) != target || g[k].titles.is_none() {
        return 0; // reaped between the two reads
    }
    out.extend_from_slice(&[MENU_WIRE_VERSION, g[k].count as u8, 0, 0]);
    for it in g[k].items[..g[k].count].iter() {
        out.extend_from_slice(&it.to_bytes());
    }
    0
}

/// M4 — reap a dead owner's entry (called from `wm::close_owner`, where its windows are reaped).
pub fn reap(asid: u64) {
    if drop_owner(asid) {
        serial_println!(":: APPMENU: owner={} closed reaped=true -> PASS ::", asid);
    }
}

/// Whether `asid` holds a published tree (the fixture's registry probe).
pub fn has(asid: u64) -> bool {
    slot_of(asid).is_some()
}

/// Deliver a pick for `asid`'s tree by identity (the fixture's and the bar's shared path).
pub fn deliver_pick(asid: u64, id: u32) -> bool {
    match slot_of(asid) {
        Some(k) => {
            pick_slot(k, id);
            true
        }
        None => false,
    }
}
