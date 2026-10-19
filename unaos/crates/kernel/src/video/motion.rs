//! CHARTER: Kernel — wm
//!
//! MOTIONKIT (rmbp-ledger B648) — `draw::motion`, the glass's MOTION PRIMITIVE: an eased value stepped by the frame
//! clock. GLASSKIT (B628) drew the controls and SETTINGSCRAFT (B630) drew Settings with them, and left owed "a motion
//! primitive (no pill ease)": the sidebar pill jumped, the toggle knob snapped, the segmented thumb teleported.
//! The spec is DESIGN-LANGUAGE §2.6 (docs/dev/OS/05_USER_EXPERIENCE/DESIGN-LANGUAGE.md): durations by ROLE
//! ([`FAST`] 100, [`BASE`] 160, [`ENTER`] 240, [`EXIT`] 160), the named curves as cubic Béziers evaluated from a
//! 64-entry Q16 lookup table (integer only, like the rest of the compositor), and the reduce-motion fallback
//! (toggles change in one frame; a spatial slide becomes a cut). The clock is the desktop's service pass and the
//! 16 ms frame (ui_guidelines.md §1): the pass asks [`Frame::due`] whether a frame is owed and repaints ONCE per
//! frame while a tween runs, never while idle.
//!
//! * [`Curve`] — `Standard` (0.2, 0, 0, 1), `Decelerate` (0, 0, 0, 1), `Accelerate` (0.3, 0, 1, 1): each a
//!   `[u32; 65]` table built at compile time (`bisect` solves the Bézier's x(s) = x for each of the 64 steps and
//!   reads y(s)); [`Curve::at`] interpolates linearly between entries. No float anywhere (draw.rs has none either).
//! * [`Tween`] — `(start, end, t0, duration, curve)`; [`Tween::value`] at a `now` in ms, [`Tween::done`].
//! * [`Glides`] — a fixed table of keyed values a painter asks for its control's CURRENT position: the first
//!   sight of a key records it, a later different target starts a tween from where the value is (a flip mid-slide
//!   reverses from there), and a finished tween is cleared on the paint that drew its end. No heap (draw.rs's
//!   paint-path contract).
//! * [`Frame`] — the repaint gate: `due(now, running)` is true at most once per [`FRAME_MS`] and only while
//!   something runs; it counts what it granted.
//! * [`reduced`] — Reduce motion: `system.accessibility.reduce_motion` (PREFS-SCHEMA.md), as the Settings window
//!   latched it at the login's load and on every PrefChanged naming it (A11YPANE B655: `settings::reduce_motion`,
//!   one atomic, no bus call on a paint path). [`REDUCE_MOTION`] is the default the latch starts from; MOTIONKIT
//!   shipped it as the one body while the schema had no key.
//!
//! `tests motion` (registered in tests.rs's settings block) samples every curve at 0/¼/½/¾/1 against the reference
//! values (the Bézier solved in double precision offline, ±[`SAMPLE_TOL`] Q16), drives a tween through a simulated
//! pass clock and counts the repaints the gate grants (idle: 0; running: ≤ duration / 16 ms + 1), and times
//! [`SAMPLES`] evaluations:
//! `:: MOTION: curves=3 samples=ok idle_repaints=0 run_repaints=<n> budget_us=<n> -> PASS ::`
//! (`budget_us` is what [`SAMPLES`] `value()` calls cost on the running machine — a thousand frames of one tween —
//! the ceiling a real frame's three or four evaluations sit far under).

/// Q16 one: the fixed point every curve, progress and knob position is in.
pub const ONE: u32 = 1 << 16;
const ONE_I: i64 = ONE as i64;

/// DESIGN-LANGUAGE §2.6's durations by role (ms). `motion.instant` is 0 (no tween: [`Glides::to`] jumps).
pub const FAST: u32 = 100;
/// Toggle, checkbox, segmented thumb, tab switch — the three Settings motions.
pub const BASE: u32 = 160;
pub const ENTER: u32 = 240;
pub const EXIT: u32 = 160;

/// The frame (ui_guidelines.md §1): one repaint per 16 ms while a tween runs.
pub const FRAME_MS: u64 = 16;

/// Reduce motion (DESIGN-LANGUAGE §2.6's no-motion fallback): the default of the Settings latch (the schema's).
pub const REDUCE_MOTION: bool = false;

/// Is motion reduced? Every tween the glides start reads this once; `true` makes every slide a one-frame cut.
/// A11YPANE (B655): the Accessibility pane's key, as Settings applied it (`settings::reduce_motion`, an atomic load).
#[inline]
pub fn reduced() -> bool {
    crate::video::settings::reduce_motion()
}

/// A11YFILTERS (B705) — THE motion seam: every animation duration is read through here, `ms` while motion is full,
/// 0 under Reduce motion (a zero duration is a one-frame cut: [`Glides::to`] jumps, [`Tween::progress`] is done at
/// once). The Settings pill, toggle knob and segmented thumb read `dur(BASE)`; the desktop's shimmer is off on the
/// same latch (`desktop::shimmer_idle`). Wire: `[a11y] … motion=<full|reduced>`.
#[inline]
pub fn dur(ms: u32) -> u32 {
    dur_for(ms, reduced())
}

/// [`dur`] for a given latch (pure; `tests a11yfilters` reads both ends).
#[inline]
pub const fn dur_for(ms: u32, reduced: bool) -> u32 {
    if reduced { 0 } else { ms }
}

// ── the curves ──────────────────────────────────────────────────────────────────────────────────────────────────

/// A cubic Bézier `(0,0) (x1,y1) (x2,y2) (1,1)` at `s` (all Q16): the x or the y coordinate, by the two control
/// values passed.
const fn bez(p1: i64, p2: i64, s: i64) -> i64 {
    let u = ONE_I - s;
    let a = (((u * u) >> 16) * s) >> 16;
    let b = (((s * s) >> 16) * u) >> 16;
    let c = (((s * s) >> 16) * s) >> 16;
    ((3 * a * p1) >> 16) + ((3 * b * p2) >> 16) + c
}

/// The 64-step table of one curve: for each x = i/64, the parameter s with x(s) = x (twenty bisections, one Q16
/// unit of resolution), then y(s), clamped to the unit range; the ends pinned exactly.
const fn build(x1: i64, y1: i64, x2: i64, y2: i64) -> [u32; 65] {
    let mut t = [0u32; 65];
    let mut i = 0;
    while i <= 64 {
        let x = (i as i64) << 10;
        let (mut lo, mut hi) = (0i64, ONE_I);
        let mut k = 0;
        while k < 20 {
            let m = (lo + hi) / 2;
            if bez(x1, x2, m) < x { lo = m } else { hi = m }
            k += 1;
        }
        let y = bez(y1, y2, (lo + hi) / 2);
        t[i] = if y < 0 { 0 } else if y > ONE_I { ONE } else { y as u32 };
        i += 1;
    }
    t[0] = 0;
    t[64] = ONE;
    t
}

/// 0.2 and 0.3 in Q16 — the only two control values that are not 0 or 1.
const P02: i64 = 13107;
const P03: i64 = 19661;

static STANDARD: [u32; 65] = build(P02, 0, 0, ONE_I);
static DECELERATE: [u32; 65] = build(0, 0, 0, ONE_I);
static ACCELERATE: [u32; 65] = build(P03, 0, ONE_I, ONE_I);

/// DESIGN-LANGUAGE §2.6's named easings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Curve {
    /// `ease.standard` (0.2, 0, 0, 1): the controls' curve.
    Standard,
    /// `ease.decelerate` (0, 0, 0, 1): surfaces entering.
    Decelerate,
    /// `ease.accelerate` (0.3, 0, 1, 1): surfaces leaving.
    Accelerate,
}

impl Curve {
    pub const ALL: [Curve; 3] = [Curve::Standard, Curve::Decelerate, Curve::Accelerate];

    fn table(self) -> &'static [u32; 65] {
        match self {
            Curve::Standard => &STANDARD,
            Curve::Decelerate => &DECELERATE,
            Curve::Accelerate => &ACCELERATE,
        }
    }

    /// The eased value of a progress `t` (Q16, clamped to `0 ..= ONE`), Q16: the table entry at `t * 64`,
    /// interpolated linearly to the next.
    pub fn at(self, t: u32) -> u32 {
        let t = t.min(ONE);
        let tb = self.table();
        let i = (t >> 10) as usize;
        if i >= 64 { return ONE; }
        let f = (t & 1023) as u64;
        let (a, b) = (tb[i] as u64, tb[i + 1] as u64);
        (a + ((b.saturating_sub(a) * f) >> 10)) as u32
    }
}

/// `start + (end - start) * t`, `t` Q16.
#[inline]
pub fn lerp(start: i32, end: i32, t: u32) -> i32 {
    start + (((end - start) as i64 * t.min(ONE) as i64) >> 16) as i32
}

// ── the tween ───────────────────────────────────────────────────────────────────────────────────────────────────

/// An eased value from `start` to `end` over `dur` ms from `t0` (the service pass's `arch::ms()` clock).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tween {
    pub start: i32,
    pub end: i32,
    pub t0: u64,
    pub dur: u32,
    pub curve: Curve,
}

impl Tween {
    pub const fn new(start: i32, end: i32, t0: u64, dur: u32, curve: Curve) -> Tween {
        Tween { start, end, t0, dur, curve }
    }
    /// Raw progress at `now`, Q16 (`ONE` once `dur` has elapsed; a zero duration is done at once).
    pub fn progress(&self, now: u64) -> u32 {
        let el = now.saturating_sub(self.t0);
        if self.dur == 0 || el >= self.dur as u64 { return ONE; }
        ((el << 16) / self.dur as u64) as u32
    }
    /// The eased value at `now`.
    pub fn value(&self, now: u64) -> i32 {
        lerp(self.start, self.end, self.curve.at(self.progress(now)))
    }
    pub fn done(&self, now: u64) -> bool {
        self.progress(now) >= ONE
    }
}

// ── the glides: keyed values a painter asks for ─────────────────────────────────────────────────────────────────

/// One keyed value: where it settles (`at` — the running tween's end) and the tween carrying it there.
#[derive(Clone, Copy, Debug)]
struct Glide {
    key: u32,
    at: i32,
    tween: Option<Tween>,
}

/// How many values one window may have in motion (Settings: the pill, a pane's toggles, a segmented thumb).
pub const GLIDES: usize = 16;

/// A fixed table of [`Glide`]s. A key seen for the first time records its target with no motion (the window's
/// first paint never animates); a key whose target moved starts a tween from the value drawn now; a key with no
/// free slot jumps.
pub struct Glides {
    slots: [Glide; GLIDES],
}

impl Default for Glides {
    fn default() -> Self {
        Glides::new()
    }
}

impl Glides {
    pub const fn new() -> Glides {
        Glides { slots: [Glide { key: 0, at: 0, tween: None }; GLIDES] }
    }

    /// The value to draw `key` at this frame, given its `target`: the tween's value while one runs, else the
    /// target. `key` is never 0. A finished tween is cleared here, on the paint that draws its end.
    pub fn to(&mut self, key: u32, target: i32, now: u64, dur: u32, curve: Curve) -> i32 {
        let Some(g) = self.slot(key) else { return target };
        if g.key != key {
            *g = Glide { key, at: target, tween: None };
            return target;
        }
        let cur = match g.tween {
            Some(t) if !t.done(now) => t.value(now),
            Some(_) => { g.tween = None; g.at }
            None => g.at,
        };
        if target != g.at {
            g.at = target;
            if reduced() || dur == 0 {
                g.tween = None;
                return target;
            }
            g.tween = Some(Tween::new(cur, target, now, dur, curve));
            return cur;
        }
        cur
    }

    /// Is any value still in motion (including one whose final frame is not yet drawn)?
    pub fn running(&self) -> bool {
        self.slots.iter().any(|g| g.tween.is_some())
    }

    /// Forget every key (a window closing, or a fixture starting afresh).
    pub fn clear(&mut self) {
        self.slots = [Glide { key: 0, at: 0, tween: None }; GLIDES];
    }

    /// The slot holding `key`, else a free one, else a settled one to evict; `None` when every slot is in motion.
    fn slot(&mut self, key: u32) -> Option<&mut Glide> {
        let i = self.slots.iter().position(|g| g.key == key)
            .or_else(|| self.slots.iter().position(|g| g.key == 0))
            .or_else(|| self.slots.iter().position(|g| g.tween.is_none()))?;
        Some(&mut self.slots[i])
    }
}

// ── the frame gate ──────────────────────────────────────────────────────────────────────────────────────────────

/// The repaint gate a service pass asks: one repaint per [`FRAME_MS`] while `running`, none while idle.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    last: u64,
    /// Repaints granted since the window opened (the "count it").
    pub count: u32,
}

impl Frame {
    pub const fn new() -> Frame {
        Frame { last: 0, count: 0 }
    }
    /// `true` when a frame is owed: something runs and a frame has passed since the last one granted.
    pub fn due(&mut self, now: u64, running: bool) -> bool {
        if !running || now.saturating_sub(self.last) < FRAME_MS { return false; }
        self.last = now;
        self.count += 1;
        true
    }
}

// ── `tests motion` ──────────────────────────────────────────────────────────────────────────────────────────────

/// The curves at 0/¼/½/¾/1 solved offline in double precision (Q16): standard, decelerate, accelerate.
const REF: [[u32; 5]; 3] = [
    [0, 39795, 57530, 63929, ONE],
    [0, 45256, 58319, 63992, ONE],
    [0, 8422, 24381, 43750, ONE],
];
/// The table's integer rounding is within four units of the references; this is the tolerance printed as `ok`.
pub const SAMPLE_TOL: u32 = 16;
/// How many `value()` calls the cost line times.
pub const SAMPLES: u64 = 1024;
/// The simulated pass clock's step (ms): passes come faster than frames, the gate must still grant one per frame.
const PASS_MS: u64 = 4;

/// Register `tests motion` (from the tests registry's settings block).
pub fn selftest() {
    // (1) the samples: each curve's table at the quarter points against the offline references.
    let mut samples = true;
    for (ci, c) in Curve::ALL.iter().enumerate() {
        for (k, want) in REF[ci].iter().enumerate() {
            let got = c.at((k as u32) * (ONE / 4));
            if got.abs_diff(*want) > SAMPLE_TOL { samples = false; serial_println!("[motion] sample curve={} k={} got={} want={}", ci, k, got, want); }
        }
        // Monotone, pinned at the ends.
        let mut prev = 0;
        for t in (0..=ONE).step_by(256) {
            let v = c.at(t);
            if v < prev { samples = false; serial_println!("[motion] non-monotone curve={} t={} v={} prev={}", ci, t, v, prev); }
            prev = v;
        }
        if c.at(0) != 0 || c.at(ONE) != ONE { samples = false; }
    }
    // (2) the repaint counts: a glide driven by a simulated pass clock through one BASE tween, then idle.
    let (mut g, mut f) = (Glides::new(), Frame::new());
    let mut now = 1000u64;
    let _ = g.to(1, 0, now, BASE, Curve::Standard); // first sight: recorded, no motion
    let mut idle = 0u32;
    for _ in 0..(400 / PASS_MS) { now += PASS_MS; if f.due(now, g.running()) { idle += 1; } }
    let _ = g.to(1, 100, now, BASE, Curve::Standard); // the change's own repaint starts the tween
    let (mut run, mut last_v, mut monotone) = (0u32, 0i32, true);
    for _ in 0..(400 / PASS_MS) {
        now += PASS_MS;
        if f.due(now, g.running()) {
            run += 1;
            let v = g.to(1, 100, now, BASE, Curve::Standard);
            if v < last_v { monotone = false; }
            last_v = v;
        }
    }
    let settled = last_v == 100 && !g.running();
    let run_ok = run >= 1 && run as u64 <= BASE as u64 / FRAME_MS + 1 && monotone && settled;
    // (3) the cost: SAMPLES evaluations of one tween across its life.
    let t = Tween::new(0, 1000, 0, BASE, Curve::Standard);
    let (t0, _) = super::now_ns();
    let mut acc = 0i64;
    for i in 0..SAMPLES { acc += t.value(i * BASE as u64 / SAMPLES) as i64; }
    let us = super::now_ns().0.saturating_sub(t0) / 1000;
    let ok = samples && idle == 0 && run_ok && acc > 0;
    serial_println!(
        ":: MOTION: curves={} samples={} idle_repaints={} run_repaints={} budget_us={} -> {} ::",
        Curve::ALL.len(), if samples { "ok" } else { "FAIL" }, idle, run, us, if ok { "PASS" } else { "FAIL" }
    );
}
