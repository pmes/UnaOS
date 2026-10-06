//! CHARTER: Kernel — kernel-by-ruling
//!
//! ORIGIN (DIALOG2, rmbp-ledger B404) — WHO asked for a spawn, carried explicitly instead of guessed.
//!
//! DIALOG (B395) decided "launched from the glass" by a 3 s window after the dock's verb drain. Now the
//! caller that knows says so: the dock / Quarry launch runs inside `with(Origin::Glass, …)`, a typed shell
//! line (the console, the serial door) inside `with(Origin::Door, …)`; a spawn with no scope around it is a
//! service's (`System`: holocron, the desktop app, a fixture). The spawn reads the scope ONCE, before its
//! task exists, and keeps it per slot ([`of_slot`]) for the fault path (`Program stopped` is a dialog only
//! for a glass launch). Atomics only — the fault path reads it.
//!
//! Witness, once per spawn: `[spawn] origin=<glass/door/system> slot=<n>`.
//!
//! Owed: one global scope (a service spawning on another core during a long door command reads `door`);
//! aarch64's spawn path does not stamp it yet.

use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Origin {
    /// The person, on the glass: the dock, Quarry, a launcher.
    Glass = 1,
    /// The person, at the door: a typed shell line (console window or serial).
    Door = 2,
    /// Nobody asked: a service, the desktop, a fixture.
    System = 3,
}

impl Origin {
    pub const fn word(self) -> &'static str {
        match self {
            Origin::Glass => "glass",
            Origin::Door => "door",
            Origin::System => "system",
        }
    }
    const fn from(v: u8) -> Option<Origin> {
        match v {
            1 => Some(Origin::Glass),
            2 => Some(Origin::Door),
            3 => Some(Origin::System),
            _ => None,
        }
    }
}

/// The scope in force (0 = none -> `System`).
static SCOPE: AtomicU8 = AtomicU8::new(0);
const SLOTS: usize = 256;
static SLOT: [AtomicU8; SLOTS] = [const { AtomicU8::new(0) }; SLOTS];

/// Run `f` with `o` as the origin of every spawn it makes; the previous scope is restored after.
pub fn with<R>(o: Origin, f: impl FnOnce() -> R) -> R {
    let prev = SCOPE.swap(o as u8, Ordering::AcqRel);
    let r = f();
    SCOPE.store(prev, Ordering::Release);
    r
}

/// The origin a spawn made now would carry.
pub fn current() -> Origin {
    Origin::from(SCOPE.load(Ordering::Acquire)).unwrap_or(Origin::System)
}

/// The spawn path, before the task exists: stamp `slot` with the scope's origin and say so.
pub fn note_spawn(slot: usize) -> Origin {
    let o = current();
    if slot < SLOTS {
        SLOT[slot].store(o as u8, Ordering::Release);
    }
    serial_println!("[spawn] origin={} slot={}", o.word(), slot);
    o
}

/// The origin `slot`'s program was spawned with (`None`: never stamped).
pub fn of_slot(slot: usize) -> Option<Origin> {
    if slot >= SLOTS {
        return None;
    }
    Origin::from(SLOT[slot].load(Ordering::Acquire))
}

/// Model-only proof for `tests notice` (no spawn): the scopes nest and restore, an unscoped spawn is the system's.
pub fn fixture() -> bool {
    let prev = SCOPE.swap(0, Ordering::AcqRel);
    let none = current() == Origin::System;
    let glass = with(Origin::Glass, || current() == Origin::Glass && with(Origin::Door, current) == Origin::Door && current() == Origin::Glass);
    let restored = current() == Origin::System;
    let save = SLOT[SLOTS - 1].load(Ordering::Relaxed);
    let stamped = with(Origin::Door, || note_spawn(SLOTS - 1)) == Origin::Door && of_slot(SLOTS - 1) == Some(Origin::Door);
    SLOT[SLOTS - 1].store(save, Ordering::Relaxed);
    SCOPE.store(prev, Ordering::Release);
    none && glass && restored && stamped
}
