//! Texture objects (ES 2.0 §3.7): the image arrays of a 2D or cube-map texture, the sampler parameters
//! (§3.7.4), and completeness (§3.7.10, with WebGL 1.0's non-power-of-two rule from ES 2.0 §3.8.2).
//!
//! Storage: every level image is kept EXPANDED to RGBA8 (`data`, `w × h × 4`, row 0 = the first row the
//! application supplied, i.e. t = 0) beside its declared `format`/`ty`. Packed and reduced formats are
//! quantized at upload, so sampling a 565 texture returns exactly what 565 can hold; LUMINANCE, ALPHA and
//! LUMINANCE_ALPHA expand per ES 2.0 Table 3.12 (L → (L, L, L, 1), A → (0, 0, 0, A)).
//!
//! Uploads (texImage2D, texSubImage2D, copyTex[Sub]Image2D, generateMipmap) and sampling (§3.7.7–§3.7.9) live
//! here too.

use crate::consts::*;
use crate::context::{Context, MAX_CUBE_SIZE_V, MAX_TEXTURE_SIZE_V};
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

// ---------------------------------------------------------------------------------------------- formats

/// Bytes per pixel of a client format/type pair (ES 2.0 Tables 3.4/3.5).
pub fn bytes_per_pixel(format: u32, ty: u32) -> usize {
    match ty {
        UNSIGNED_SHORT_5_6_5 | UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1 => 2,
        _ => match format {
            ALPHA | LUMINANCE => 1,
            LUMINANCE_ALPHA => 2,
            RGB => 3,
            _ => 4,
        },
    }
}

pub fn valid_format(f: u32) -> bool {
    matches!(f, ALPHA | RGB | RGBA | LUMINANCE | LUMINANCE_ALPHA)
}

pub fn valid_type(t: u32) -> bool {
    matches!(t, UNSIGNED_BYTE | UNSIGNED_SHORT_5_6_5 | UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1)
}

/// ES 2.0 Table 3.4: the packed types belong to one format each.
pub fn format_type_ok(f: u32, t: u32) -> bool {
    match t {
        UNSIGNED_SHORT_5_6_5 => f == RGB,
        UNSIGNED_SHORT_4_4_4_4 | UNSIGNED_SHORT_5_5_5_1 => f == RGBA,
        _ => true,
    }
}

/// The client bytes a `w × h` upload needs with `align` row alignment (the last row unpadded, ES 2.0 §3.6.2).
pub fn upload_size(w: usize, h: usize, format: u32, ty: u32, align: usize) -> usize {
    if w == 0 || h == 0 {
        return 0;
    }
    let row = w * bytes_per_pixel(format, ty);
    let stride = row.div_ceil(align) * align;
    stride * (h - 1) + row
}

fn rep(n: u32, bits: u32) -> u8 {
    // Bit replication of an n-bit value to 8 bits.
    let v = n << (8 - bits);
    (v | (v >> bits) | if bits < 4 { v >> (2 * bits) } else { 0 }) as u8
}

/// Re-quantize straight RGBA8 to a packed type's bits (what a 565 / 4444 / 5551 texel can hold).
fn requantize(px: &mut [u8], ty: u32) {
    let q = |v: u8, bits: u32| -> u8 {
        let max = (1u32 << bits) - 1;
        rep((v as u32 * max + 127) / 255, bits)
    };
    for p in px.chunks_exact_mut(4) {
        match ty {
            UNSIGNED_SHORT_5_6_5 => {
                p[0] = q(p[0], 5);
                p[1] = q(p[1], 6);
                p[2] = q(p[2], 5);
                p[3] = 255;
            }
            UNSIGNED_SHORT_4_4_4_4 => {
                for c in p.iter_mut() {
                    *c = q(*c, 4);
                }
            }
            UNSIGNED_SHORT_5_5_5_1 => {
                p[0] = q(p[0], 5);
                p[1] = q(p[1], 5);
                p[2] = q(p[2], 5);
                p[3] = if p[3] >= 128 { 255 } else { 0 };
            }
            _ => {}
        }
    }
}

/// Expand client pixels to RGBA8 (ES 2.0 §3.6.2 unpacking, Table 3.12's expansion of each format).
pub fn unpack(src: &[u8], w: usize, h: usize, format: u32, ty: u32, align: usize) -> Vec<u8> {
    let bpp = bytes_per_pixel(format, ty);
    let stride = (w * bpp).div_ceil(align) * align;
    let mut out = alloc::vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let o = y * stride + x * bpp;
            let d = &mut out[(y * w + x) * 4..(y * w + x) * 4 + 4];
            if o + bpp > src.len() {
                continue;
            }
            let s = &src[o..o + bpp];
            match ty {
                UNSIGNED_SHORT_5_6_5 => {
                    let v = u16::from_le_bytes([s[0], s[1]]) as u32;
                    d.copy_from_slice(&[rep((v >> 11) & 31, 5), rep((v >> 5) & 63, 6), rep(v & 31, 5), 255]);
                }
                UNSIGNED_SHORT_4_4_4_4 => {
                    let v = u16::from_le_bytes([s[0], s[1]]) as u32;
                    d.copy_from_slice(&[rep((v >> 12) & 15, 4), rep((v >> 8) & 15, 4), rep((v >> 4) & 15, 4), rep(v & 15, 4)]);
                }
                UNSIGNED_SHORT_5_5_5_1 => {
                    let v = u16::from_le_bytes([s[0], s[1]]) as u32;
                    d.copy_from_slice(&[rep((v >> 11) & 31, 5), rep((v >> 6) & 31, 5), rep((v >> 1) & 31, 5), if v & 1 != 0 { 255 } else { 0 }]);
                }
                _ => match format {
                    ALPHA => d.copy_from_slice(&[0, 0, 0, s[0]]),
                    LUMINANCE => d.copy_from_slice(&[s[0], s[0], s[0], 255]),
                    LUMINANCE_ALPHA => d.copy_from_slice(&[s[0], s[0], s[0], s[1]]),
                    RGB => d.copy_from_slice(&[s[0], s[1], s[2], 255]),
                    _ => d.copy_from_slice(s),
                },
            }
        }
    }
    out
}

/// Straight RGBA8 (a DOM source, or a framebuffer read) converted to `format` (ES 2.0 Table 3.12 / §3.7.2's
/// "the components are selected" rule: L from R, A from A).
pub fn from_rgba(px: &mut [u8], format: u32) {
    for p in px.chunks_exact_mut(4) {
        match format {
            ALPHA => {
                p[0] = 0;
                p[1] = 0;
                p[2] = 0;
            }
            LUMINANCE => {
                p[1] = p[0];
                p[2] = p[0];
                p[3] = 255;
            }
            LUMINANCE_ALPHA => {
                p[1] = p[0];
                p[2] = p[0];
            }
            RGB => p[3] = 255,
            _ => {}
        }
    }
}

fn flip_rows(px: &mut [u8], w: usize, h: usize) {
    let row = w * 4;
    for y in 0..h / 2 {
        let (a, b) = px.split_at_mut((h - 1 - y) * row);
        a[y * row..y * row + row].swap_with_slice(&mut b[..row]);
    }
}

fn premultiply(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in p.iter_mut().take(3) {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

/// The pixel source of a texImage2D / texSubImage2D call.
pub enum Pixels<'a> {
    /// `null`: zero-filled (WebGL 1.0 initializes texture storage).
    None,
    /// An ArrayBufferView's bytes in the call's format/type.
    Bytes(&'a [u8]),
    /// A DOM source (ImageData, image, canvas, video) as straight RGBA8, its own width and height.
    Rgba(&'a [u8]),
}

// ---------------------------------------------------------------------------------------------- uploads

impl Context {
    /// Shared validation of texImage2D / copyTexImage2D (ES 2.0 §3.7.1): returns the texture name and face.
    fn image_checks(&mut self, target: u32, level: i32, w: i32, h: i32, border: i32) -> Option<(u32, usize)> {
        let bt = binding_target(target)?;
        let max = if bt == TEXTURE_CUBE_MAP { MAX_CUBE_SIZE_V } else { MAX_TEXTURE_SIZE_V };
        if level < 0 || level as u32 > 31 - max.leading_zeros() {
            self.err(INVALID_VALUE);
            return None;
        }
        let lim = (max >> level) as i32;
        if w < 0 || h < 0 || w > lim || h > lim || border != 0 || (bt == TEXTURE_CUBE_MAP && w != h) {
            self.err(INVALID_VALUE);
            return None;
        }
        let n = self.bound_texture(bt);
        if n == 0 {
            self.err(INVALID_OPERATION);
            return None;
        }
        Some((n, face_index(target).unwrap()))
    }

    fn set_level(&mut self, tex: u32, face: usize, level: usize, l: Level) {
        let t = self.textures.get_mut(tex).unwrap();
        t.ensure_faces();
        let f = &mut t.faces[face];
        while f.len() <= level {
            f.push(None);
        }
        f[level] = Some(l);
    }

    /// Convert an upload to stored RGBA8 with the unpack parameters applied.
    fn convert(&self, px: &Pixels, w: usize, h: usize, format: u32, ty: u32) -> Vec<u8> {
        let mut out = match px {
            Pixels::None => return alloc::vec![0u8; w * h * 4].into_iter().enumerate().map(|(i, _)| if i % 4 == 3 && !has_alpha(format) { 255 } else { 0 }).collect(),
            Pixels::Bytes(b) => unpack(b, w, h, format, ty, self.st.unpack_alignment as usize),
            Pixels::Rgba(b) => {
                let mut v = b[..(w * h * 4).min(b.len())].to_vec();
                v.resize(w * h * 4, 0);
                v
            }
        };
        if self.st.unpack_flip_y {
            flip_rows(&mut out, w, h);
        }
        if self.st.unpack_premultiply && has_alpha(format) {
            premultiply(&mut out);
        }
        if let Pixels::Rgba(_) = px {
            from_rgba(&mut out, format);
        }
        requantize(&mut out, ty);
        out
    }

    /// texImage2D (ES 2.0 §3.7.1 with WebGL 1.0's texImage2D): `internal` must equal `format`; FLOAT is refused
    /// (OES_texture_float is not offered).
    #[allow(clippy::too_many_arguments)]
    pub fn tex_image_2d(&mut self, target: u32, level: i32, internal: u32, w: i32, h: i32, border: i32, format: u32, ty: u32, px: Pixels) {
        if binding_target(target).is_none() {
            return self.err(INVALID_ENUM);
        }
        if !valid_format(format) || !valid_type(ty) {
            return self.err(INVALID_ENUM);
        }
        if !valid_format(internal) {
            return self.err(INVALID_VALUE);
        }
        let Some((tex, face)) = self.image_checks(target, level, w, h, border) else { return };
        if internal != format || !format_type_ok(format, ty) {
            return self.err(INVALID_OPERATION);
        }
        let (wu, hu) = (w as usize, h as usize);
        match &px {
            Pixels::Bytes(b) => {
                if b.len() < upload_size(wu, hu, format, ty, self.st.unpack_alignment as usize) {
                    return self.err(INVALID_OPERATION);
                }
            }
            Pixels::Rgba(b) => {
                if b.len() < wu * hu * 4 {
                    return self.err(INVALID_OPERATION);
                }
            }
            Pixels::None => {}
        }
        let data = self.convert(&px, wu, hu, format, ty);
        self.set_level(tex, face, level as usize, Level { w: w as u32, h: h as u32, format, ty, data });
    }

    /// texSubImage2D (ES 2.0 §3.7.2): the level must exist and the format/type match it.
    #[allow(clippy::too_many_arguments)]
    pub fn tex_sub_image_2d(&mut self, target: u32, level: i32, x: i32, y: i32, w: i32, h: i32, format: u32, ty: u32, px: Pixels) {
        let Some(bt) = binding_target(target) else { return self.err(INVALID_ENUM) };
        if !valid_format(format) || !valid_type(ty) {
            return self.err(INVALID_ENUM);
        }
        if level < 0 {
            return self.err(INVALID_VALUE);
        }
        let n = self.bound_texture(bt);
        if n == 0 {
            return self.err(INVALID_OPERATION);
        }
        let face = face_index(target).unwrap();
        let Some(l) = self.textures.get(n).and_then(|t| t.level(face, level as usize)) else { return self.err(INVALID_OPERATION) };
        let (lw, lh, lf, lt) = (l.w as i64, l.h as i64, l.format, l.ty);
        if x < 0 || y < 0 || w < 0 || h < 0 || x as i64 + w as i64 > lw || y as i64 + h as i64 > lh {
            return self.err(INVALID_VALUE);
        }
        if format != lf || ty != lt || !format_type_ok(format, ty) {
            return self.err(INVALID_OPERATION);
        }
        let (wu, hu) = (w as usize, h as usize);
        match &px {
            Pixels::Bytes(b) if b.len() < upload_size(wu, hu, format, ty, self.st.unpack_alignment as usize) => return self.err(INVALID_OPERATION),
            Pixels::Rgba(b) if b.len() < wu * hu * 4 => return self.err(INVALID_OPERATION),
            _ => {}
        }
        let data = self.convert(&px, wu, hu, format, ty);
        let l = self.textures.get_mut(n).unwrap().faces[face][level as usize].as_mut().unwrap();
        for r in 0..hu {
            let d = ((y as usize + r) * l.w as usize + x as usize) * 4;
            l.data[d..d + wu * 4].copy_from_slice(&data[r * wu * 4..(r + 1) * wu * 4]);
        }
    }

    /// The current read framebuffer's colour as RGBA8 over [x, x+w) × [y, y+h), outside texels 0 (WebGL 1.0
    /// "Reading Pixels Outside the Framebuffer"); also whether it carries alpha.
    fn read_region(&mut self, x: i32, y: i32, w: usize, h: usize) -> Option<(Vec<u8>, bool)> {
        self.fresh_drawing_buffer();
        let t = self.take_target();
        if !t.has_color {
            self.put_target(t);
            return None;
        }
        let alpha = !matches!(t.fmt, crate::raster::Fmt::Rgb8 | crate::raster::Fmt::Rgb565);
        let mut out = alloc::vec![0u8; w * h * 4];
        for r in 0..h {
            let sy = y as i64 + r as i64;
            for c in 0..w {
                let sx = x as i64 + c as i64;
                if sx < 0 || sy < 0 || sx >= t.w as i64 || sy >= t.h as i64 {
                    continue;
                }
                let si = (sy as usize * t.w as usize + sx as usize) * 4;
                out[(r * w + c) * 4..(r * w + c) * 4 + 4].copy_from_slice(&t.color[si..si + 4]);
                if !alpha {
                    out[(r * w + c) * 4 + 3] = 255;
                }
            }
        }
        let g = self.db.generation;
        self.put_target(t);
        self.db.generation = g;
        Some((out, alpha))
    }

    /// The bound framebuffer's colour attachment is `tex` (a copy from it into it is a feedback loop).
    fn reads_from(&self, tex: u32) -> bool {
        self.framebuffers.get(self.framebuffer).is_some_and(|f| matches!(f.color, crate::object::Attach::Tex(n, _, _) if n == tex))
    }

    /// copyTexImage2D (ES 2.0 §3.7.2): the internal format must be satisfiable from the framebuffer
    /// (no alpha formats from an alpha-less buffer).
    #[allow(clippy::too_many_arguments)]
    pub fn copy_tex_image_2d(&mut self, target: u32, level: i32, internal: u32, x: i32, y: i32, w: i32, h: i32, border: i32) {
        if binding_target(target).is_none() {
            return self.err(INVALID_ENUM);
        }
        if !valid_format(internal) {
            return self.err(INVALID_ENUM);
        }
        let Some((tex, face)) = self.image_checks(target, level, w, h, border) else { return };
        if self.framebuffer_status() != FRAMEBUFFER_COMPLETE {
            return self.err(INVALID_FRAMEBUFFER_OPERATION);
        }
        if self.reads_from(tex) {
            return self.err(INVALID_OPERATION);
        }
        let Some((mut px, alpha)) = self.read_region(x, y, w as usize, h as usize) else { return self.err(INVALID_OPERATION) };
        if has_alpha(internal) && !alpha {
            return self.err(INVALID_OPERATION);
        }
        from_rgba(&mut px, internal);
        self.set_level(tex, face, level as usize, Level { w: w as u32, h: h as u32, format: internal, ty: UNSIGNED_BYTE, data: px });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn copy_tex_sub_image_2d(&mut self, target: u32, level: i32, xo: i32, yo: i32, x: i32, y: i32, w: i32, h: i32) {
        let Some(bt) = binding_target(target) else { return self.err(INVALID_ENUM) };
        if level < 0 {
            return self.err(INVALID_VALUE);
        }
        let n = self.bound_texture(bt);
        if n == 0 {
            return self.err(INVALID_OPERATION);
        }
        let face = face_index(target).unwrap();
        let Some(l) = self.textures.get(n).and_then(|t| t.level(face, level as usize)) else { return self.err(INVALID_OPERATION) };
        let (lw, lh, lf, lt) = (l.w as i64, l.h as i64, l.format, l.ty);
        if xo < 0 || yo < 0 || w < 0 || h < 0 || xo as i64 + w as i64 > lw || yo as i64 + h as i64 > lh {
            return self.err(INVALID_VALUE);
        }
        if self.framebuffer_status() != FRAMEBUFFER_COMPLETE {
            return self.err(INVALID_FRAMEBUFFER_OPERATION);
        }
        if self.reads_from(n) {
            return self.err(INVALID_OPERATION);
        }
        let Some((mut px, alpha)) = self.read_region(x, y, w as usize, h as usize) else { return self.err(INVALID_OPERATION) };
        if has_alpha(lf) && !alpha {
            return self.err(INVALID_OPERATION);
        }
        from_rgba(&mut px, lf);
        requantize(&mut px, lt);
        let (wu, hu) = (w as usize, h as usize);
        let l = self.textures.get_mut(n).unwrap().faces[face][level as usize].as_mut().unwrap();
        for r in 0..hu {
            let d = ((yo as usize + r) * l.w as usize + xo as usize) * 4;
            l.data[d..d + wu * 4].copy_from_slice(&px[r * wu * 4..(r + 1) * wu * 4]);
        }
    }

    /// compressedTexImage2D / compressedTexSubImage2D: no compressed format is offered
    /// (COMPRESSED_TEXTURE_FORMATS is empty), so every format is INVALID_ENUM.
    pub fn compressed_tex_image(&mut self) {
        self.err(INVALID_ENUM);
    }

    /// generateMipmap (ES 2.0 §3.7.11): a 2×2 box filter per level; the base must be power-of-two (and the cube
    /// cube-complete), else INVALID_OPERATION.
    pub fn generate_mipmap(&mut self, target: u32) {
        let Some(n) = self.tex_for(target) else { return };
        let t = self.textures.get(n).unwrap();
        let faces = if target == TEXTURE_CUBE_MAP { 6 } else { 1 };
        let Some(b) = t.base(0) else { return self.err(INVALID_OPERATION) };
        if !is_pot(b.w) || !is_pot(b.h) || (faces == 6 && !t.cube_complete()) {
            return self.err(INVALID_OPERATION);
        }
        let t = self.textures.get_mut(n).unwrap();
        for f in 0..faces {
            let Some(base) = t.base(f).cloned() else { continue };
            let q = max_level(base.w, base.h);
            let mut prev = base.clone();
            let face = &mut t.faces[f];
            face.truncate(1);
            for _ in 1..=q {
                let (w, h) = ((prev.w / 2).max(1), (prev.h / 2).max(1));
                let mut data = alloc::vec![0u8; (w * h * 4) as usize];
                for y in 0..h {
                    for x in 0..w {
                        let mut acc = [0u32; 4];
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let sx = (2 * x + dx).min(prev.w - 1);
                            let sy = (2 * y + dy).min(prev.h - 1);
                            let si = ((sy * prev.w + sx) * 4) as usize;
                            for c in 0..4 {
                                acc[c] += prev.data[si + c] as u32;
                            }
                        }
                        let di = ((y * w + x) * 4) as usize;
                        for c in 0..4 {
                            data[di + c] = ((acc[c] + 2) / 4) as u8;
                        }
                    }
                }
                requantize(&mut data, prev.ty);
                let l = Level { w, h, format: prev.format, ty: prev.ty, data };
                face.push(Some(l.clone()));
                prev = l;
            }
        }
    }
}

fn has_alpha(format: u32) -> bool {
    matches!(format, ALPHA | LUMINANCE_ALPHA | RGBA)
}

// ---------------------------------------------------------------------------------------------- sampling

fn wrap(i: i64, size: u32, mode: u32) -> usize {
    let n = size as i64;
    (match mode {
        CLAMP_TO_EDGE => i.clamp(0, n - 1),
        MIRRORED_REPEAT => {
            let m = i.rem_euclid(2 * n);
            if m >= n { 2 * n - 1 - m } else { m }
        }
        _ => i.rem_euclid(n),
    }) as usize
}

fn texel(l: &Level, x: usize, y: usize) -> [f32; 4] {
    let i = (y * l.w as usize + x) * 4;
    let d = &l.data;
    if i + 4 > d.len() {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [d[i] as f32 / 255.0, d[i + 1] as f32 / 255.0, d[i + 2] as f32 / 255.0, d[i + 3] as f32 / 255.0]
}

/// One level, NEAREST or LINEAR (ES 2.0 §3.7.7 eqs. 3.18–3.20).
fn filter(t: &Texture, l: &Level, s: f32, tc: f32, linear: bool) -> [f32; 4] {
    let u = s as f64 * l.w as f64;
    let v = tc as f64 * l.h as f64;
    if !linear {
        let i = wrap(fm::floor(u) as i64, l.w, t.wrap_s);
        let j = wrap(fm::floor(v) as i64, l.h, t.wrap_t);
        return texel(l, i, j);
    }
    let (u0, v0) = (fm::floor(u - 0.5), fm::floor(v - 0.5));
    let (a, b) = ((u - 0.5 - u0) as f32, (v - 0.5 - v0) as f32);
    let (i0, i1) = (wrap(u0 as i64, l.w, t.wrap_s), wrap(u0 as i64 + 1, l.w, t.wrap_s));
    let (j0, j1) = (wrap(v0 as i64, l.h, t.wrap_t), wrap(v0 as i64 + 1, l.h, t.wrap_t));
    let (t00, t10, t01, t11) = (texel(l, i0, j0), texel(l, i1, j0), texel(l, i0, j1), texel(l, i1, j1));
    let mut o = [0.0; 4];
    for c in 0..4 {
        o[c] = (1.0 - a) * (1.0 - b) * t00[c] + a * (1.0 - b) * t10[c] + (1.0 - a) * b * t01[c] + a * b * t11[c];
    }
    o
}

/// ES 2.0 §3.7.5 Table 3.21: the cube face and its (s, t) for a direction.
pub fn cube_face(r: [f32; 3]) -> (usize, f32, f32) {
    let (ax, ay, az) = (r[0].abs(), r[1].abs(), r[2].abs());
    let (face, sc, tc, ma) = if ax >= ay && ax >= az {
        if r[0] >= 0.0 { (0, -r[2], -r[1], ax) } else { (1, r[2], -r[1], ax) }
    } else if ay >= az {
        if r[1] >= 0.0 { (2, r[0], r[2], ay) } else { (3, r[0], -r[2], ay) }
    } else if r[2] >= 0.0 {
        (4, r[0], -r[1], az)
    } else {
        (5, -r[0], -r[1], az)
    };
    let ma = if ma == 0.0 { 1.0 } else { ma };
    (face, (sc / ma + 1.0) * 0.5, (tc / ma + 1.0) * 0.5)
}

/// The (s, t) of direction `r` projected on a fixed `face` (for the derivative of a cube lookup).
fn cube_st_on(face: usize, r: [f32; 3]) -> (f32, f32) {
    let (sc, tc, ma) = match face {
        0 => (-r[2], -r[1], r[0]),
        1 => (r[2], -r[1], -r[0]),
        2 => (r[0], r[2], r[1]),
        3 => (r[0], -r[2], -r[1]),
        4 => (r[0], -r[1], r[2]),
        _ => (-r[0], -r[1], -r[2]),
    };
    let ma = if ma.abs() < 1e-20 { 1e-20 } else { ma.abs() };
    ((sc / ma + 1.0) * 0.5, (tc / ma + 1.0) * 0.5)
}

use crate::fmath as fm;
use crate::vm::Lod;

/// Sample a complete texture (ES 2.0 §3.7.7–§3.7.9): λ from the LOD source, the magnification switch c, the
/// minification filters with and without mipmaps.
pub fn sample(t: &Texture, cube: bool, coord: [f32; 3], lod: Lod) -> [f32; 4] {
    let (face, s, tc) = if cube { cube_face(coord) } else { (0, coord[0], coord[1]) };
    let Some(base) = t.base(face) else { return [0.0, 0.0, 0.0, 1.0] };
    let (w, h) = (base.w as f64, base.h as f64);
    let lambda: f64 = match lod {
        Lod::Bias(b) => b as f64,
        Lod::Explicit(l) => l as f64,
        Lod::Grad { dx, dy, bias } => {
            let d = |g: [f32; 3]| -> f64 {
                let (ds, dt) = if cube {
                    let (s1, t1) = cube_st_on(face, [coord[0] + g[0], coord[1] + g[1], coord[2] + g[2]]);
                    (s1 - s, t1 - tc)
                } else {
                    (g[0], g[1])
                };
                let (du, dv) = (ds as f64 * w, dt as f64 * h);
                fm::sqrt(du * du + dv * dv)
            };
            let rho = d(dx).max(d(dy));
            if rho <= 0.0 { f64::NEG_INFINITY } else { fm::log2(rho) + bias as f64 }
        }
    };
    let c = if t.mag_filter == LINEAR && matches!(t.min_filter, NEAREST_MIPMAP_NEAREST | NEAREST_MIPMAP_LINEAR) { 0.5 } else { 0.0 };
    if lambda <= c || lambda.is_nan() {
        return filter(t, base, s, tc, t.mag_filter == LINEAR);
    }
    let q = max_level(base.w, base.h);
    let lvl = |d: usize| t.level(face, d.min(q)).unwrap_or(base);
    match t.min_filter {
        NEAREST => filter(t, base, s, tc, false),
        LINEAR => filter(t, base, s, tc, true),
        NEAREST_MIPMAP_NEAREST | LINEAR_MIPMAP_NEAREST => {
            let d = if lambda <= 0.5 { 0 } else { (fm::ceil(lambda + 0.5) - 1.0).max(0.0) as usize };
            filter(t, lvl(d), s, tc, t.min_filter == LINEAR_MIPMAP_NEAREST)
        }
        _ => {
            let linear = t.min_filter == LINEAR_MIPMAP_LINEAR;
            let fl = fm::floor(lambda).max(0.0);
            let d1 = (fl as usize).min(q);
            let d2 = (d1 + 1).min(q);
            let f = (lambda - fl).clamp(0.0, 1.0) as f32;
            let (a, b) = (filter(t, lvl(d1), s, tc, linear), filter(t, lvl(d2), s, tc, linear));
            let mut o = [0.0; 4];
            for k in 0..4 {
                o[k] = (1.0 - f) * a[k] + f * b[k];
            }
            o
        }
    }
}
