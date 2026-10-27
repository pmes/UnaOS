//! CHARTER: the Surface contract's damage geometry (handler: Aether, CODEX §2, whose paint the rects describe; seam: this
//! host lib, beside `tetra`'s surface contract). AETHERMACGPU-ENGINEDAMAGE (LEDGER SR341): ONE copy of the rect-list
//! arithmetic a damage `SurfaceBlit` needs: the sender's [`merge`] and [`cut`] (`tetra::SurfaceDamage::frame_rects`), and
//! the receivers' [`spans`] (GTK `image_view`, AppKit `image_view::set_damage` / `gpu_view::page_damage`), each rect then
//! written by the existing `tetra::surface_splice` / `gpu_view::replace_rect`. Pure, so the host tests read it.

use std::ops::Range;

/// A rect `(x, y, w, h)` in surface pixels.
pub type Rect = (u32, u32, u32, u32);

/// At most this many rects ride one blit; more are merged (the pair whose union grows least first).
pub const MAX_RECTS: usize = 8;

/// Past this many input rects the list is taken as its bounding box (the pairwise merge is quadratic).
const MAX_INPUT: usize = 64;

/// `(x0, y0, x1, y1)`, exclusive ends.
type Edges = (u32, u32, u32, u32);

fn area(e: Edges) -> u64 {
    (e.2 - e.0) as u64 * (e.3 - e.1) as u64
}

fn union(a: Edges, b: Edges) -> Edges {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// Overlapping or touching (a shared edge or corner): merged for free.
fn meets(a: Edges, b: Edges) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// Unite every overlapping or touching pair until none is left.
fn unite(v: &mut Vec<Edges>) {
    'again: loop {
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                if meets(v[i], v[j]) {
                    let b = v.swap_remove(j);
                    v[i] = union(v[i], b);
                    continue 'again;
                }
            }
        }
        return;
    }
}

/// The sender's rect list for a `width x height` frame: each rect clipped to the frame (empties dropped), overlapping or
/// touching rects united, at most [`MAX_RECTS`] left. `None` = send the frame whole (an empty frame, or the rects cover at
/// least 3/4 of it); `Some(empty)` = nothing changed.
pub fn merge(rects: &[Rect], width: u32, height: u32) -> Option<Vec<Rect>> {
    let frame = width as u64 * height as u64;
    if frame == 0 {
        return None;
    }
    let mut v: Vec<Edges> = rects
        .iter()
        .filter_map(|&(x, y, w, h)| {
            let x1 = (x as u64 + w as u64).min(width as u64) as u32;
            let y1 = (y as u64 + h as u64).min(height as u64) as u32;
            (x < x1 && y < y1).then_some((x, y, x1, y1))
        })
        .collect();
    if v.len() > MAX_INPUT {
        let b = v.iter().copied().reduce(union).expect("non-empty");
        v = vec![b];
    }
    unite(&mut v);
    while v.len() > MAX_RECTS {
        // The rects are disjoint now, so a union's growth is its area less the two it replaces.
        let mut best = (u64::MAX, 0, 1);
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                let grow = area(union(v[i], v[j])).saturating_sub(area(v[i]) + area(v[j]));
                if grow < best.0 {
                    best = (grow, i, j);
                }
            }
        }
        let b = v.swap_remove(best.2);
        v[best.1] = union(v[best.1], b);
        unite(&mut v); // a merge can make the result meet a third rect
    }
    let covered: u64 = v.iter().map(|&e| area(e)).sum();
    if covered * 4 >= frame * 3 {
        return None;
    }
    Some(v.into_iter().map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0, y1 - y0)).collect())
}

/// The rects' rows out of `pixels` (a frame of 4-byte pixels, stride `width * 4`), concatenated in list order: the
/// `pixels` of a damage blit. A row that falls outside `pixels` is skipped (the rects come from [`merge`], inside it).
pub fn cut(pixels: &[u8], width: u32, rects: &[Rect]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut out = Vec::with_capacity(rects.iter().map(|r| r.2 as usize * r.3 as usize * 4).sum());
    for &(x, y, w, h) in rects {
        for row in y as usize..(y + h) as usize {
            let at = row * stride + x as usize * 4;
            if let Some(src) = pixels.get(at..at + w as usize * 4) {
                out.extend_from_slice(src);
            }
        }
    }
    out
}

/// The receivers' walk of a damage blit: for each rect of a `width x height` frame, its byte range in the blit's `pixels`
/// (`len` bytes; each rect's rows stride `w * 4`, in list order). `None` when a rect is empty or leaves the frame, or the
/// rects' bytes do not sum to `len` (the receiver drops the blit; the sender's next whole frame repairs it).
pub fn spans(rects: &[Rect], width: u32, height: u32, len: usize) -> Option<Vec<(Rect, Range<usize>)>> {
    let mut at = 0usize;
    let mut out = Vec::with_capacity(rects.len());
    for &r in rects {
        let (x, y, w, h) = r;
        if w == 0 || h == 0 || x as u64 + w as u64 > width as u64 || y as u64 + h as u64 > height as u64 {
            return None;
        }
        let end = at.checked_add(w as usize * h as usize * 4)?;
        out.push((r, at..end));
        at = end;
    }
    (at == len).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECTS, cut, merge, spans};

    /// Clip, drop empties, unite overlapping and touching rects, cap at MAX_RECTS, and >= 3/4 cover goes whole.
    #[test]
    fn damage_merge_kat() {
        assert_eq!(merge(&[], 10, 10), Some(vec![]));
        assert_eq!(merge(&[(0, 0, 1, 1)], 0, 10), None, "an empty frame goes whole");
        assert_eq!(merge(&[(8, 8, 5, 5), (3, 3, 0, 2), (20, 0, 1, 1)], 10, 10), Some(vec![(8, 8, 2, 2)]), "clipped, empties dropped");
        assert_eq!(merge(&[(0, 0, 2, 2), (1, 1, 2, 2)], 10, 10), Some(vec![(0, 0, 3, 3)]), "overlap united");
        assert_eq!(merge(&[(0, 0, 2, 2), (2, 0, 2, 2)], 10, 10), Some(vec![(0, 0, 4, 2)]), "a shared edge united");
        assert_eq!(merge(&[(0, 0, 1, 1), (5, 5, 1, 1)], 10, 10), Some(vec![(0, 0, 1, 1), (5, 5, 1, 1)]), "apart stays apart");
        // Twelve 1x1 rects on a diagonal-free grid (every other column, two rows apart) are capped at MAX_RECTS.
        let many: Vec<_> = (0..12u32).map(|i| ((i % 6) * 4, (i / 6) * 40, 1, 1)).collect();
        let m = merge(&many, 100, 100).expect("small");
        assert!(m.len() <= MAX_RECTS, "{} rects", m.len());
        for &(x, y, _, _) in &many {
            assert!(m.iter().any(|&(rx, ry, rw, rh)| x >= rx && x < rx + rw && y >= ry && y < ry + rh), "({x},{y}) still covered");
        }
        assert_eq!(merge(&[(0, 0, 10, 8)], 10, 10), None, "80% cover goes whole");
        assert_eq!(merge(&[(0, 0, 10, 7)], 10, 10), Some(vec![(0, 0, 10, 7)]), "70% stays rects");
    }

    /// Each rect's byte range in list order; a rect off the frame, an empty rect or a length mismatch drop the blit.
    #[test]
    fn damage_spans_kat() {
        let s = spans(&[(1, 0, 1, 1), (0, 1, 2, 1)], 3, 2, 12).expect("fits");
        assert_eq!(s, vec![((1, 0, 1, 1), 0..4), ((0, 1, 2, 1), 4..12)]);
        assert!(spans(&[(2, 0, 2, 1)], 3, 2, 8).is_none(), "leaves the frame");
        assert!(spans(&[(0, 0, 0, 1)], 3, 2, 0).is_none(), "empty rect");
        assert!(spans(&[(0, 0, 1, 1)], 3, 2, 8).is_none(), "length mismatch");
        assert_eq!(spans(&[], 3, 2, 0), Some(vec![]));
    }

    /// cut then a splice per span (the receivers' path) rebuilds the changed frame from the old one.
    #[test]
    fn damage_cut_kat() {
        let (w, h) = (4u32, 3u32);
        let old = vec![0u8; (w * h * 4) as usize];
        let mut new = old.clone();
        for (i, b) in new.iter_mut().enumerate() {
            let px = i / 4;
            let (x, y) = (px as u32 % w, px as u32 / w);
            if (x == 1 && y == 0) || (y == 2 && x < 3) {
                *b = i as u8 + 1;
            }
        }
        let rects = merge(&[(1, 0, 1, 1), (0, 2, 3, 1)], w, h).expect("rects");
        let rows = cut(&new, w, &rects);
        assert_eq!(rows.len(), (1 + 3) * 4);
        let mut kept = old.clone();
        for (r, range) in spans(&rects, w, h, rows.len()).expect("spans") {
            assert!(crate::tetra::surface_splice(&mut kept, w, h, r, &rows[range]));
        }
        assert_eq!(kept, new);
    }
}
