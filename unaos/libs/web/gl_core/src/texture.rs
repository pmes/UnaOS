//! Texture objects (ES 2.0 §3.7): the image arrays of a 2D or cube-map texture, the sampler parameters
//! (§3.7.4), and completeness (§3.7.10, with WebGL 1.0's non-power-of-two rule from ES 2.0 §3.8.2).
//!
//! Storage: every level image is kept EXPANDED to RGBA8 (`data`, `w × h × 4`, row 0 = the first row the
//! application supplied, i.e. t = 0) beside its declared `format`/`ty`. Packed and reduced formats are
//! quantized at upload, so sampling a 565 texture returns exactly what 565 can hold; LUMINANCE, ALPHA and
//! LUMINANCE_ALPHA expand per ES 2.0 Table 3.12 (L → (L, L, L, 1), A → (0, 0, 0, A)).

use crate::consts::*;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct Level {
    pub w: u32,
    pub h: u32,
    pub format: u32,
    pub ty: u32,
    pub data: Vec<u8>,
}

pub struct Texture {
    /// 0 until first bound, then TEXTURE_2D or TEXTURE_CUBE_MAP (ES 2.0 §3.7.13: fixed by the first bind).
    pub target: u32,
    /// One face for 2D, six for a cube (in TEXTURE_CUBE_MAP_POSITIVE_X.. order); each a level array.
    pub faces: Vec<Vec<Option<Level>>>,
    pub min_filter: u32,
    pub mag_filter: u32,
    pub wrap_s: u32,
    pub wrap_t: u32,
    pub deleted: bool,
}

impl Default for Texture {
    fn default() -> Self {
        // ES 2.0 Table 6.10 initial values.
        Texture {
            target: 0,
            faces: Vec::new(),
            min_filter: NEAREST_MIPMAP_LINEAR,
            mag_filter: LINEAR,
            wrap_s: REPEAT,
            wrap_t: REPEAT,
            deleted: false,
        }
    }
}

/// The face index of an image target (TEXTURE_2D → 0, a cube face → 0..6), None if not an image target.
pub fn face_index(target: u32) -> Option<usize> {
    match target {
        TEXTURE_2D => Some(0),
        TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z => Some((target - TEXTURE_CUBE_MAP_POSITIVE_X) as usize),
        _ => None,
    }
}

/// The texture target an image target belongs to.
pub fn binding_target(target: u32) -> Option<u32> {
    match target {
        TEXTURE_2D => Some(TEXTURE_2D),
        TEXTURE_CUBE_MAP_POSITIVE_X..=TEXTURE_CUBE_MAP_NEGATIVE_Z => Some(TEXTURE_CUBE_MAP),
        _ => None,
    }
}

pub fn is_pot(v: u32) -> bool {
    v != 0 && v & (v - 1) == 0
}

pub fn uses_mipmaps(min: u32) -> bool {
    !matches!(min, NEAREST | LINEAR)
}

/// floor(log2(max(w, h))): the last level of a full mipmap chain (ES 2.0 §3.7.10, q = p + floor(log2(maxsize))).
pub fn max_level(w: u32, h: u32) -> usize {
    let m = w.max(h).max(1);
    (31 - m.leading_zeros()) as usize
}

impl Texture {
    pub fn ensure_faces(&mut self) {
        let n = if self.target == TEXTURE_CUBE_MAP { 6 } else { 1 };
        while self.faces.len() < n {
            self.faces.push(Vec::new());
        }
    }

    pub fn level(&self, face: usize, level: usize) -> Option<&Level> {
        self.faces.get(face).and_then(|f| f.get(level)).and_then(|l| l.as_ref())
    }

    pub fn base(&self, face: usize) -> Option<&Level> {
        self.level(face, 0)
    }

    /// The mipmap chain of `face` is complete (ES 2.0 §3.7.10): levels 0..=q exist, each halving (floor, min 1),
    /// all with the same format and type.
    fn chain_complete(&self, face: usize) -> bool {
        let Some(b) = self.base(face) else { return false };
        let q = max_level(b.w, b.h);
        let (mut w, mut h) = (b.w, b.h);
        for l in 1..=q {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            match self.level(face, l) {
                Some(x) if x.w == w && x.h == h && x.format == b.format && x.ty == b.ty => {}
                _ => return false,
            }
        }
        true
    }

    /// Cube completeness (ES 2.0 §3.7.10): six base images, square, equal size, same format and type.
    pub fn cube_complete(&self) -> bool {
        let Some(b) = self.base(0) else { return false };
        if b.w != b.h || b.w == 0 {
            return false;
        }
        (1..6).all(|f| matches!(self.base(f), Some(x) if x.w == b.w && x.h == b.h && x.format == b.format && x.ty == b.ty))
    }

    /// Whether sampling this texture returns its texels; an incomplete texture samples (0, 0, 0, 1)
    /// (ES 2.0 §3.8.2). Includes the NPOT rule: a non-power-of-two texture is incomplete when its min filter
    /// uses mipmaps or either wrap mode is not CLAMP_TO_EDGE.
    pub fn complete(&self) -> bool {
        let Some(b) = self.base(0) else { return false };
        if b.w == 0 || b.h == 0 {
            return false;
        }
        let npot = !is_pot(b.w) || !is_pot(b.h);
        if npot && (uses_mipmaps(self.min_filter) || self.wrap_s != CLAMP_TO_EDGE || self.wrap_t != CLAMP_TO_EDGE) {
            return false;
        }
        let faces = if self.target == TEXTURE_CUBE_MAP { 6 } else { 1 };
        if faces == 6 && !self.cube_complete() {
            return false;
        }
        if uses_mipmaps(self.min_filter) {
            return (0..faces).all(|f| self.chain_complete(f));
        }
        true
    }
}
