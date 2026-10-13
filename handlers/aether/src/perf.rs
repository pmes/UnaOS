//! AETHERPAINTPERF (LEDGER SR130): the engine's work counters and the scoping switch.
//!
//! CHARTER: Aether owns the browser (docs/CODEX.md §2); this is its own instrumentation, inside the live handler.
//!
//! Four counters say how much of the document one frame paid for:
//!
//! * `cascaded` — elements run through the cascade (`css::apply_stylesheets*`: a scoped restyle re-cascades
//!   only the elements a dirty root reaches and reuses the last cascade for the rest);
//! * `laid_out` — taffy nodes the taffy passes of `layout::remeasure` re-laid: before each pass, the dirty boxes
//!   reachable from the root through dirty boxes (`LayoutTree::relaid`). AETHERPAINTPERF2 (SR132): a relayout keeps
//!   the previous tree, so a clean box is a cache hit and is not counted (the box build and the style rewrites still
//!   run over the whole tree: see docs/dev/evidence/aetherpaintperf/AETHERPAINTPERF2.md);
//! * `painted` — boxes whose paint ran (`render::render_frame`: the paint index skips a box whose subtree's
//!   ink bounds meet no damage rect); `skipped` counts the boxes it skipped;
//! * `glyph_hits` — glyph runs drawn from the rasterized-run cache instead of being shaped and rasterized.
//!
//! [`counters`] is the running total since [`reset`]; [`last_frame`] is the last `render_frame`'s share. With
//! `AETHER_PERF=1` every frame prints its share on stderr as one wire line,
//! `:: paintperf: cascaded=… laid_out=… painted=… glyph_hits=… ::`.
//!
//! [`set_scoped`] turns every scope off for the calling thread (the "before" of a measurement and the
//! whole-page oracle of a byte-identity check); `AETHER_NO_SCOPE=1` starts threads with it off.

use std::cell::Cell;

/// One set of counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Elements cascaded.
    pub cascaded: u64,
    /// Taffy nodes laid out.
    pub laid_out: u64,
    /// Boxes painted.
    pub painted: u64,
    /// Boxes the paint index skipped.
    pub skipped: u64,
    /// Glyph runs drawn from the run cache.
    pub glyph_hits: u64,
    /// Glyph runs shaped and rasterized (and stored).
    pub glyph_misses: u64,
    /// Restyles that re-cascaded only dirty roots, and restyles that cascaded the whole document.
    pub scoped_restyles: u64,
    pub whole_restyles: u64,
    /// Frames painted.
    pub frames: u64,
}

thread_local! {
    /// Counts since the last frame ended, and the running total since `reset`.
    static PENDING: Cell<Counters> = const { Cell::new(Counters {
        cascaded: 0, laid_out: 0, painted: 0, skipped: 0, glyph_hits: 0, glyph_misses: 0, scoped_restyles: 0, whole_restyles: 0, frames: 0,
    }) };
    static TOTAL: Cell<Counters> = const { Cell::new(Counters {
        cascaded: 0, laid_out: 0, painted: 0, skipped: 0, glyph_hits: 0, glyph_misses: 0, scoped_restyles: 0, whole_restyles: 0, frames: 0,
    }) };
    static LAST: Cell<Counters> = const { Cell::new(Counters {
        cascaded: 0, laid_out: 0, painted: 0, skipped: 0, glyph_hits: 0, glyph_misses: 0, scoped_restyles: 0, whole_restyles: 0, frames: 0,
    }) };
    /// Whether the scopes run on this thread (None: not decided yet — the environment decides).
    static SCOPED: Cell<Option<bool>> = const { Cell::new(None) };
}

fn bump(f: impl Fn(&mut Counters)) {
    for cell in [&PENDING, &TOTAL] {
        cell.with(|c| {
            let mut v = c.get();
            f(&mut v);
            c.set(v);
        });
    }
}

pub(crate) fn add_cascaded(n: u64) {
    bump(|c| c.cascaded += n);
}
pub(crate) fn add_laid_out(n: u64) {
    bump(|c| c.laid_out += n);
}
pub(crate) fn add_painted(n: u64, skipped: u64) {
    bump(|c| {
        c.painted += n;
        c.skipped += skipped;
    });
}
pub(crate) fn add_glyph(hit: bool) {
    bump(|c| if hit { c.glyph_hits += 1 } else { c.glyph_misses += 1 });
}
pub(crate) fn add_restyle(scoped: bool) {
    bump(|c| if scoped { c.scoped_restyles += 1 } else { c.whole_restyles += 1 });
}

/// The running totals since [`reset`].
pub fn counters() -> Counters {
    TOTAL.with(|c| c.get())
}

/// The last painted frame's share (the work done between the frame before it and its own end).
pub fn last_frame() -> Counters {
    LAST.with(|c| c.get())
}

/// Zeroes the totals and the pending frame.
pub fn reset() {
    TOTAL.with(|c| c.set(Counters::default()));
    PENDING.with(|c| c.set(Counters::default()));
}

/// The counters' wire line.
pub fn wire_line(c: &Counters) -> String {
    format!(
        ":: paintperf: cascaded={} laid_out={} painted={} glyph_hits={} ::",
        c.cascaded, c.laid_out, c.painted, c.glyph_hits
    )
}

fn perf_env() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("AETHER_PERF").is_some_and(|v| v == "1"))
}

/// A frame ended (`AetherEngine::render_frame`): its share becomes [`last_frame`]; `AETHER_PERF=1` prints it.
pub(crate) fn end_frame() {
    bump(|c| c.frames += 1);
    let frame = PENDING.with(|c| c.replace(Counters::default()));
    LAST.with(|c| c.set(frame));
    if perf_env() {
        eprintln!("{}", wire_line(&frame));
    }
}

/// Whether the restyle scope, the paint index, the glyph-run cache and the animation damage scope run on this
/// thread.
pub fn scoped() -> bool {
    SCOPED.with(|s| match s.get() {
        Some(v) => v,
        None => {
            let v = std::env::var_os("AETHER_NO_SCOPE").is_none_or(|v| v != "1");
            s.set(Some(v));
            v
        }
    })
}

/// Turns the scopes on or off for this thread (off: every restyle cascades the whole document, every frame
/// paints every box it reaches, no glyph run is cached, an animation frame damages the viewport).
pub fn set_scoped(on: bool) {
    SCOPED.with(|s| s.set(Some(on)));
}
