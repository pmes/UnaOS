//! AETHERPAINTPERF (LEDGER SR130): rasterized glyph runs kept across frames.
//!
//! `draw_shaped` shapes a run (`Advancer::place`, itself cached by fonts::shape), pre-blends the ink
//! (`raster::preblend`) and asks the rasterizer for every glyph's coverage at the run's pen (text_host keeps each
//! glyph mask; the per-glyph placement, the quarter-pixel phase and the pre-blend still run per frame). A repaint
//! draws the same runs at the same pens again — a caret blink, a keystroke, a colour animation repaint every run
//! under their damage. This cache keeps a run's emitted pixels — (x, y, coverage after the pre-blend), in the
//! order the rasterizer emitted them — keyed by everything that decides them: the text, the face selection, the
//! size, the letter and word spacing, the direction, the ink colour, the pen's exact bits (x and baseline) and
//! the web-font generation (a loaded `@font-face` changes the faces a family resolves to). A hit replays the
//! pixels through the same surface / damage / clip test the rasterizer callback applies, so the frame is the
//! frame a miss paints. Least recently used runs are dropped past [`CAP_BYTES`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::fonts::lines::Advancer;

/// The bytes the cache keeps at most (runs larger than [`RUN_CAP`] are never kept).
const CAP_BYTES: usize = 8 << 20;
const RUN_CAP: usize = 256 << 10;

/// What decides a run's pixels.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Key {
    text: String,
    family: u16,
    weight: u16,
    italic: bool,
    stretch: u16,
    size: u32,
    letter_spacing: u32,
    word_spacing: u32,
    dir: u8,
    color: (u8, u8, u8),
    x: u32,
    baseline: u32,
    generation: u64,
}

/// A run's emitted pixels and the advance `draw_shaped` returns for it.
pub(super) struct Run {
    pub px: Vec<(i32, i32, u8)>,
    pub advance: f32,
}

#[derive(Default)]
struct Cache {
    /// Each run with its last use and its size in bytes.
    map: HashMap<Key, (Rc<Run>, u64, usize)>,
    bytes: usize,
    clock: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

pub(super) fn key(text: &str, adv: &Advancer, x: f32, baseline: f32, color: (u8, u8, u8)) -> Key {
    use font_core::bidi::Direction;
    Key {
        text: text.to_string(),
        family: adv.sel.family,
        weight: adv.sel.weight,
        italic: adv.sel.italic,
        stretch: adv.sel.stretch,
        size: adv.size.to_bits(),
        letter_spacing: adv.letter_spacing.to_bits(),
        word_spacing: adv.word_spacing.to_bits(),
        dir: match adv.dir {
            Direction::Auto => 0,
            Direction::Ltr => 1,
            Direction::Rtl => 2,
        },
        color,
        x: x.to_bits(),
        baseline: baseline.to_bits(),
        generation: crate::fonts::webfont::generation(),
    }
}

/// The run kept under `k`, marked used.
pub(super) fn get(k: &Key) -> Option<Rc<Run>> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.clock += 1;
        let now = c.clock;
        let e = c.map.get_mut(k)?;
        e.1 = now;
        Some(e.0.clone())
    })
}

/// Keeps `run` under `k`; past the cap the least recently used runs go until a quarter of it is free.
pub(super) fn put(k: Key, run: Run) {
    let bytes = run.px.len() * std::mem::size_of::<(i32, i32, u8)>() + k.text.len() + std::mem::size_of::<Key>() + 32;
    if bytes > RUN_CAP {
        return;
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.clock += 1;
        let now = c.clock;
        if let Some((_, _, old)) = c.map.insert(k, (Rc::new(run), now, bytes)) {
            c.bytes -= old;
        }
        c.bytes += bytes;
        if c.bytes > CAP_BYTES {
            let mut by_age: Vec<(u64, Key)> = c.map.iter().map(|(k, v)| (v.1, k.clone())).collect();
            by_age.sort_by_key(|(t, _)| *t);
            for (_, k) in by_age {
                if c.bytes <= CAP_BYTES * 3 / 4 {
                    break;
                }
                if let Some((_, _, b)) = c.map.remove(&k) {
                    c.bytes -= b;
                }
            }
        }
    });
}
