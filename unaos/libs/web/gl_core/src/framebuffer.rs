//! Framebuffer and renderbuffer objects (ES 2.0 §4.4) with WebGL 1.0's DEPTH_STENCIL (§6.6) and its
//! attachment-combination rule (§6.6: more than one of DEPTH, STENCIL and DEPTH_STENCIL is UNSUPPORTED), and
//! framebuffer completeness (ES 2.0 §4.4.5).

use crate::consts::*;
use crate::context::{Context, ObjKind, Param, MAX_RENDERBUFFER_SIZE_V};
use crate::object::Attach;
use crate::texture;
use alloc::vec;
use alloc::vec::Vec;

/// Colour-renderable texture images (ES 2.0 §4.4.5: RGB and RGBA at the byte and packed types; LUMINANCE,
/// ALPHA and LUMINANCE_ALPHA are not).
pub fn tex_color_renderable(format: u32, ty: u32) -> bool {
    matches!((format, ty), (RGBA, UNSIGNED_BYTE) | (RGB, UNSIGNED_BYTE) | (RGBA, UNSIGNED_SHORT_4_4_4_4) | (RGBA, UNSIGNED_SHORT_5_5_5_1) | (RGB, UNSIGNED_SHORT_5_6_5))
}

impl Context {
    fn attachment_point(attachment: u32) -> Option<usize> {
        match attachment {
            COLOR_ATTACHMENT0 => Some(0),
            DEPTH_ATTACHMENT => Some(1),
            STENCIL_ATTACHMENT => Some(2),
            DEPTH_STENCIL_ATTACHMENT => Some(3),
            _ => None,
        }
    }

    fn set_attachment(&mut self, point: usize, a: Attach) {
        let fb = self.framebuffer;
        if let Some(f) = self.framebuffers.get_mut(fb) {
            match point {
                0 => f.color = a,
                1 => f.depth = a,
                2 => f.stencil = a,
                _ => f.depth_stencil = a,
            }
        }
        self.sweep();
    }

    /// ES 2.0 §4.4.3 RenderbufferStorage. Storage is initialized to 0 (WebGL 1.0 §4.1 "resource restrictions").
    pub fn renderbuffer_storage(&mut self, target: u32, format: u32, w: i32, h: i32) {
        if target != RENDERBUFFER {
            return self.err(INVALID_ENUM);
        }
        if !matches!(format, RGBA4 | RGB565 | RGB5_A1 | DEPTH_COMPONENT16 | STENCIL_INDEX8 | DEPTH_STENCIL) {
            return self.err(INVALID_ENUM);
        }
        if w < 0 || h < 0 || w as u32 > MAX_RENDERBUFFER_SIZE_V || h as u32 > MAX_RENDERBUFFER_SIZE_V {
            return self.err(INVALID_VALUE);
        }
        let name = self.renderbuffer;
        let Some(r) = self.renderbuffers.get_mut(name) else { return self.err(INVALID_OPERATION) };
        let n = (w as usize) * (h as usize);
        r.format = format;
        r.w = w as u32;
        r.h = h as u32;
        r.color = if matches!(format, RGBA4 | RGB565 | RGB5_A1) { vec![0; n * 4] } else { Vec::new() };
        r.depth = if matches!(format, DEPTH_COMPONENT16 | DEPTH_STENCIL) { vec![0; n] } else { Vec::new() };
        r.stencil = if matches!(format, STENCIL_INDEX8 | DEPTH_STENCIL) { vec![0; n] } else { Vec::new() };
    }

    pub fn get_renderbuffer_parameter(&mut self, target: u32, pname: u32) -> Param {
        if target != RENDERBUFFER {
            self.err(INVALID_ENUM);
            return Param::Null;
        }
        let name = self.renderbuffer;
        let Some(r) = self.renderbuffers.get(name) else {
            self.err(INVALID_OPERATION);
            return Param::Null;
        };
        let sizes: [i64; 6] = match r.format {
            RGBA4 => [4, 4, 4, 4, 0, 0],
            RGB565 => [5, 6, 5, 0, 0, 0],
            RGB5_A1 => [5, 5, 5, 1, 0, 0],
            DEPTH_COMPONENT16 => [0, 0, 0, 0, 16, 0],
            STENCIL_INDEX8 => [0, 0, 0, 0, 0, 8],
            DEPTH_STENCIL => [0, 0, 0, 0, 24, 8],
            _ => [0; 6],
        };
        let empty = r.w == 0 || r.h == 0;
        let sz = |i: usize| Param::Int(if empty { 0 } else { sizes[i] });
        match pname {
            RENDERBUFFER_WIDTH => Param::Int(r.w as i64),
            RENDERBUFFER_HEIGHT => Param::Int(r.h as i64),
            RENDERBUFFER_INTERNAL_FORMAT => Param::Int(r.format as i64),
            RENDERBUFFER_RED_SIZE => sz(0),
            RENDERBUFFER_GREEN_SIZE => sz(1),
            RENDERBUFFER_BLUE_SIZE => sz(2),
            RENDERBUFFER_ALPHA_SIZE => sz(3),
            RENDERBUFFER_DEPTH_SIZE => sz(4),
            RENDERBUFFER_STENCIL_SIZE => sz(5),
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }

    /// ES 2.0 §4.4.3 FramebufferRenderbuffer.
    pub fn framebuffer_renderbuffer(&mut self, target: u32, attachment: u32, rb_target: u32, rb: u32) {
        if target != FRAMEBUFFER {
            return self.err(INVALID_ENUM);
        }
        let Some(point) = Self::attachment_point(attachment) else { return self.err(INVALID_ENUM) };
        if rb_target != RENDERBUFFER {
            return self.err(INVALID_ENUM);
        }
        if self.framebuffer == 0 {
            return self.err(INVALID_OPERATION);
        }
        if rb != 0 && self.renderbuffers.get(rb).is_none_or(|r| r.deleted) {
            return self.err(INVALID_OPERATION);
        }
        self.set_attachment(point, if rb == 0 { Attach::None } else { Attach::Rb(rb) });
    }

    /// ES 2.0 §4.4.3 FramebufferTexture2D (WebGL 1.0 §6.6: level must be 0).
    pub fn framebuffer_texture_2d(&mut self, target: u32, attachment: u32, textarget: u32, tex: u32, level: i32) {
        if target != FRAMEBUFFER {
            return self.err(INVALID_ENUM);
        }
        let Some(point) = Self::attachment_point(attachment) else { return self.err(INVALID_ENUM) };
        let Some(want) = texture::binding_target(textarget) else { return self.err(INVALID_ENUM) };
        if self.framebuffer == 0 {
            return self.err(INVALID_OPERATION);
        }
        if tex != 0 {
            if level != 0 {
                return self.err(INVALID_VALUE);
            }
            let Some(t) = self.textures.get(tex) else { return self.err(INVALID_OPERATION) };
            if t.deleted || t.target != want {
                return self.err(INVALID_OPERATION);
            }
        }
        self.set_attachment(point, if tex == 0 { Attach::None } else { Attach::Tex(tex, textarget, level) });
    }

    /// (width, height, kind) of an attachment's image, kind 0 colour / 1 depth / 2 stencil / 3 depth-stencil,
    /// None when the image is missing or not renderable at any point.
    fn attachment_image(&self, a: Attach) -> Option<(u32, u32, u8)> {
        match a {
            Attach::None => None,
            Attach::Rb(r) => {
                let r = self.renderbuffers.get(r)?;
                let kind = match r.format {
                    RGBA4 | RGB565 | RGB5_A1 => 0,
                    DEPTH_COMPONENT16 => 1,
                    STENCIL_INDEX8 => 2,
                    DEPTH_STENCIL => 3,
                    _ => return None,
                };
                Some((r.w, r.h, kind))
            }
            Attach::Tex(t, target, level) => {
                let t = self.textures.get(t)?;
                let l = t.level(texture::face_index(target)?, level as usize)?;
                if !tex_color_renderable(l.format, l.ty) {
                    return Some((l.w, l.h, 9));
                }
                Some((l.w, l.h, 0))
            }
        }
    }

    /// ES 2.0 §4.4.5 framebuffer completeness of the bound framebuffer (0 is always complete).
    pub fn framebuffer_status(&self) -> u32 {
        let Some(f) = self.framebuffers.get(self.framebuffer) else { return FRAMEBUFFER_COMPLETE };
        let pts = f.points();
        let mut dims: Option<(u32, u32)> = None;
        let mut any = false;
        for (i, a) in pts.iter().enumerate() {
            if *a == Attach::None {
                continue;
            }
            let Some((w, h, kind)) = self.attachment_image(*a) else { return FRAMEBUFFER_INCOMPLETE_ATTACHMENT };
            // "attachment complete": a non-zero image whose format suits the attachment point.
            if w == 0 || h == 0 || kind as usize != i {
                return FRAMEBUFFER_INCOMPLETE_ATTACHMENT;
            }
            any = true;
            match dims {
                None => dims = Some((w, h)),
                Some(d) if d != (w, h) => return FRAMEBUFFER_INCOMPLETE_DIMENSIONS,
                _ => {}
            }
        }
        if !any {
            return FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT;
        }
        let ds = [pts[1], pts[2], pts[3]].iter().filter(|a| **a != Attach::None).count();
        if ds > 1 {
            return FRAMEBUFFER_UNSUPPORTED;
        }
        FRAMEBUFFER_COMPLETE
    }

    pub fn check_framebuffer_status(&mut self, target: u32) -> u32 {
        if target != FRAMEBUFFER {
            self.err(INVALID_ENUM);
            return 0;
        }
        self.framebuffer_status()
    }

    pub fn get_framebuffer_attachment_parameter(&mut self, target: u32, attachment: u32, pname: u32) -> Param {
        if target != FRAMEBUFFER {
            self.err(INVALID_ENUM);
            return Param::Null;
        }
        let Some(point) = Self::attachment_point(attachment) else {
            self.err(INVALID_ENUM);
            return Param::Null;
        };
        let Some(f) = self.framebuffers.get(self.framebuffer) else {
            self.err(INVALID_OPERATION);
            return Param::Null;
        };
        let a = f.points()[point];
        match (a, pname) {
            (Attach::None, FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE) => Param::Int(NONE as i64),
            (Attach::None, FRAMEBUFFER_ATTACHMENT_OBJECT_NAME) => Param::Null,
            (Attach::Rb(_), FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE) => Param::Int(RENDERBUFFER as i64),
            (Attach::Rb(r), FRAMEBUFFER_ATTACHMENT_OBJECT_NAME) => Param::Object(ObjKind::Renderbuffer, r),
            (Attach::Tex(..), FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE) => Param::Int(TEXTURE as i64),
            (Attach::Tex(t, _, _), FRAMEBUFFER_ATTACHMENT_OBJECT_NAME) => Param::Object(ObjKind::Texture, t),
            (Attach::Tex(_, _, l), FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL) => Param::Int(l as i64),
            (Attach::Tex(_, target, _), FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE) => {
                Param::Int(if target == TEXTURE_2D { 0 } else { target as i64 })
            }
            _ => {
                self.err(INVALID_ENUM);
                Param::Null
            }
        }
    }
}
