//! VUGSPREAD — the ARCH-NEUTRAL half of the work-stealing repair.
//!
//! Both schedulers are arch files by necessity (`arch/x86_64/sched.rs` owns `SCHED`/`RUN_QUEUES`
//! and an APIC-ms clock; `arch/aarch64/sched.rs` owns `rq()`/`CUR_PRIO` and a CNTPCT clock), and
//! nothing here tries to unify them. What IS unifiable is the POLICY the VUGSPREAD arc landed on
//! x86 and PARITY.md §6.6c owed the Pi: three numbers and two predicates that answer
//!
//!   * which victims an idle core is allowed to see (the FLOOR), and
//!   * how long a task that just moved is left alone (the COOLDOWN BRAKE).
//!
//! Those were `const`s and `fn`s inside `arch/x86_64/sched.rs`, so the Pi could not call them and
//! a Pi port would have been a second copy that drifts. They live here instead; both arches call
//! in, and each keeps only the plumbing that is genuinely arch-bound (how you ask "is this core
//! running something", where milliseconds come from).
//!
//! No `cfg(target_arch)` appears in this file, and none should. A hardware difference that really
//! justified different numbers would be a reason to add a parameter, not a `cfg`.
//!
//! Design-twin note (the standing law: a policy tuned for a contended machine must be checked on an
//! idle one). The floor is the aggressive half and the cooldown is its brake, and they are tuned as
//! a pair. On an idle machine the floor's relaxation is what makes a lone packed core visible at
//! all; the cooldown is what stops two idle cores trading that core's task back and forth once it
//! is. Loosening one without the other is the failure mode, in either regime.

/// Ready-queue depth at which an idle core may steal from a victim that is itself IDLE (between
/// tasks). Leaving the last ready task at home is the classic ping-pong guard: a core that is about
/// to dispatch its only ready task must not have it taken out from under it.
pub const STEAL_MIN_DEPTH: usize = 2;

/// Base of the per-task cooldown window: how long a task that has migrated ONCE must sit on its new
/// home before another idle core may take it.
pub const STEAL_COOLDOWN_MS: u64 = 16;

/// Escalation cap for [`steal_cooldown_ms`] — `16 << 4 = 256` ms is the terminal window. Chosen so
/// the worst case stays imperceptible (a quarter-second residency floor, paid only by a task
/// already re-stolen four times) while being far beyond the wake/block cadence that drives the
/// ping-pong (~0.5–16 ms), so escalation TERMINATES the cycle rather than stretching it.
pub const STEAL_COOLDOWN_ESC_CAP: u32 = 4;

/// The ready-queue depth at which an idle core may take one of a victim's tasks.
///
/// `victim_running` = that core has a task dispatched right now (x86: `current_prio != PRIO_IDLE`;
/// aarch64: `CUR_PRIO != PRIO_NONE`). This is the whole of the floor repair, and the reason it is a
/// repair rather than a tuning knob: a run queue holds only READY tasks — the task a core is
/// EXECUTING is not in it. So a flat floor of 2 means a core needs THREE runnable tasks before an
/// idle core judges it loaded, and the packing this arc exists for — a vug's parent and one of its
/// workers time-sharing one core while other cores sit at 0% — sits at queue depth ONE and is
/// invisible by construction.
///
/// A victim that is running something carries that task PLUS its queue, so depth 1 already means two
/// runnable tasks. A victim at idle is between tasks and is about to dispatch the very task we would
/// take, so it keeps the stricter floor.
///
/// Best-effort by design: the caller's `victim_running` read may be a tick stale. Being wrong costs
/// at most one extra migration and can never lose or duplicate a task, because the decision is
/// re-taken under the victim's own lock.
#[inline]
#[must_use]
pub const fn steal_floor(victim_running: bool) -> usize {
    if victim_running { 1 } else { STEAL_MIN_DEPTH }
}

/// The per-task ESCALATING cooldown window: how long a task with `migrations` past moves must sit on
/// its current home before it may be stolen again.
///
/// It escalates because a FLAT window does not stop a ping-pong, it only stretches its period — a
/// flat brake reads "refusals climbing" and "re-migrations climbing" at the same time, refusing and
/// serving the same oscillation. Doubling per re-steal reaches a window that outlasts the
/// wake/block cadence driving it, at which point the task settles. `migrations` deliberately never
/// decays: the recency gate (`migrate_ms != 0` and the elapsed test) bounds the whole mechanism at
/// [`STEAL_COOLDOWN_ESC_CAP`] doublings, so an old counter cannot freeze a task forever.
#[inline]
#[must_use]
pub const fn steal_cooldown_ms(migrations: u32) -> u64 {
    let esc = if migrations < STEAL_COOLDOWN_ESC_CAP { migrations } else { STEAL_COOLDOWN_ESC_CAP };
    STEAL_COOLDOWN_MS << esc
}

/// `true` if this task migrated too recently to be stolen again — the ping-pong brake's predicate.
///
/// `migrate_ms == 0` means "never migrated" and always clears, so the FIRST corrective steal is
/// never delayed: the brake only ever damps a re-steal, it never blocks the repair itself. That
/// asymmetry is the point — a fleet settling reads one migration per task and no cooldown skips at
/// all.
///
/// `now_ms` must be a clock both cores agree on (x86 `APIC_TICKS`, aarch64 CNTPCT-derived), read
/// ONCE per steal attempt so every candidate in one walk is judged against one instant.
#[inline]
#[must_use]
pub const fn steal_cooled(migrations: u32, migrate_ms: u64, now_ms: u64) -> bool {
    migrate_ms != 0 && now_ms.saturating_sub(migrate_ms) < steal_cooldown_ms(migrations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_sees_two_on_one_packing() {
        // The defect the arc is named for: one running task + one ready behind it reads depth 1.
        assert_eq!(steal_floor(true), 1);
        // ...and the ping-pong guard survives it: an idle victim still keeps its last ready task.
        assert_eq!(steal_floor(false), STEAL_MIN_DEPTH);
    }

    #[test]
    fn cooldown_escalates_and_terminates() {
        assert_eq!(steal_cooldown_ms(0), 16);
        assert_eq!(steal_cooldown_ms(1), 32);
        assert_eq!(steal_cooldown_ms(4), 256);
        // Capped: a task re-stolen twenty times is still bounded at a quarter second.
        assert_eq!(steal_cooldown_ms(20), 256);
    }

    #[test]
    fn first_migration_is_never_delayed() {
        assert!(!steal_cooled(0, 0, 1_000_000));
        // A task that moved 1 ms ago is held; the same task 300 ms later is free even at the cap.
        assert!(steal_cooled(0, 1_000, 1_001));
        assert!(!steal_cooled(4, 1_000, 1_300));
    }
}
