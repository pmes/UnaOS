// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF-WRITE (SR325): new PDF 1.4 files — [`PageBuilder`] (path operators, images), [`Builder::finish`], and
//! [`from_rgba`], one image per page. Written through the one file writer (`serialize::write_file`); compressed through
//! pixel_core's one compressor. PDF-WRITE-FONTS (SR338): [`PageBuilder::text`] shows text in a face loaded by
//! [`Builder::font`], embedded as a TrueType subset (`fonts.rs`).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use core::cell::RefCell;

use svg_core::geom::{Path, Seg};

use super::fonts::{self, FontHandle};

use super::serialize::{text_bytes, write_file, write_num};
use crate::Error;
use crate::object::{Dict, Object, Ref, Stream};

/// FlateDecode bytes through pixel_core (`None`: the compressor declined; the stream is then written unfiltered).
pub(super) fn flate(data: &[u8]) -> Option<Vec<u8>> {
    pixel_core::deflate::compress(data).ok()
}

fn name(n: &[u8]) -> Object {
    Object::Name(n.to_vec())
}

/// An image XObject's pixels: 8 bits per component, top row first.
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    /// `width * height * 3` bytes, /DeviceRGB, written FlateDecode.
    Rgb { width: u32, height: u32, data: Vec<u8> },
    /// `width * height` bytes, /DeviceGray, written FlateDecode.
    Gray { width: u32, height: u32, data: Vec<u8> },
    /// A baseline JPEG file passed through as /DCTDecode; `components` 1 (Gray), 3 (RGB) or 4 (CMYK).
    Jpeg { width: u32, height: u32, components: u8, data: Vec<u8> },
}

impl Image {
    fn check(&self) -> Result<(), Error> {
        let (w, h, ok) = match self {
            Image::Rgb { width, height, data } => (*width, *height, data.len() as u64 == *width as u64 * *height as u64 * 3),
            Image::Gray { width, height, data } => (*width, *height, data.len() as u64 == *width as u64 * *height as u64),
            Image::Jpeg { width, height, components, data } => (*width, *height, matches!(components, 1 | 3 | 4) && !data.is_empty()),
        };
        if w == 0 || h == 0 || !ok {
            return Err(Error::Unsupported("image size or data length"));
        }
        Ok(())
    }

    fn to_stream(&self) -> Stream {
        let (w, h, space, data, dct) = match self {
            Image::Rgb { width, height, data } => (*width, *height, &b"DeviceRGB"[..], data, false),
            Image::Gray { width, height, data } => (*width, *height, &b"DeviceGray"[..], data, false),
            Image::Jpeg { width, height, components, data } => {
                let space: &[u8] = match components {
                    1 => b"DeviceGray",
                    4 => b"DeviceCMYK",
                    _ => b"DeviceRGB",
                };
                (*width, *height, space, data, true)
            }
        };
        let mut d = Dict::new();
        d.insert(b"Type".to_vec(), name(b"XObject"));
        d.insert(b"Subtype".to_vec(), name(b"Image"));
        d.insert(b"Width".to_vec(), Object::Int(w as i64));
        d.insert(b"Height".to_vec(), Object::Int(h as i64));
        d.insert(b"ColorSpace".to_vec(), name(space));
        d.insert(b"BitsPerComponent".to_vec(), Object::Int(8));
        if dct {
            d.insert(b"Filter".to_vec(), name(b"DCTDecode"));
            return Stream { dict: d, data: data.clone() };
        }
        match flate(data) {
            Some(z) => {
                d.insert(b"Filter".to_vec(), name(b"FlateDecode"));
                Stream { dict: d, data: z }
            }
            None => Stream { dict: d, data: data.clone() },
        }
    }
}

/// One page: its /MediaBox (points) and a content stream built from §8 operators. Text is drawn as paths (the caller's
/// glyph outlines through [`PageBuilder::path`]) or shown in an embedded face ([`PageBuilder::text`], SR338).
#[derive(Clone, Debug, PartialEq)]
pub struct PageBuilder {
    pub media_box: [f64; 4],
    content: Vec<u8>,
    images: Vec<Image>,
    /// The ids of the [`FontHandle`]s this page shows text in (its /Font resources).
    fonts: BTreeSet<u32>,
}

impl PageBuilder {
    /// A `width` x `height` point page (origin bottom left).
    pub fn new(width: f64, height: f64) -> Self {
        PageBuilder { media_box: [0.0, 0.0, width, height], content: Vec::new(), images: Vec::new(), fonts: BTreeSet::new() }
    }

    /// The content stream so far.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    fn op(&mut self, nums: &[f64], op: &str) -> &mut Self {
        for &n in nums {
            write_num(&mut self.content, n);
            self.content.push(b' ');
        }
        self.content.extend_from_slice(op.as_bytes());
        self.content.push(b'\n');
        self
    }

    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(&[x, y], "m")
    }
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(&[x, y], "l")
    }
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> &mut Self {
        self.op(&[x1, y1, x2, y2, x3, y3], "c")
    }
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.op(&[x, y, w, h], "re")
    }
    pub fn close(&mut self) -> &mut Self {
        self.op(&[], "h")
    }

    /// An svg_core path (a glyph outline, a vector shape) as m / l / c / h; a quadratic is raised to a cubic.
    pub fn path(&mut self, p: &Path) -> &mut Self {
        let (mut cur, mut start) = ((0.0, 0.0), (0.0, 0.0));
        for seg in &p.segs {
            match *seg {
                Seg::Move(q) => {
                    self.move_to(q.0, q.1);
                    cur = q;
                    start = q;
                }
                Seg::Line(q) => {
                    self.line_to(q.0, q.1);
                    cur = q;
                }
                Seg::Quad(c, q) => {
                    let c1 = (cur.0 + 2.0 / 3.0 * (c.0 - cur.0), cur.1 + 2.0 / 3.0 * (c.1 - cur.1));
                    let c2 = (q.0 + 2.0 / 3.0 * (c.0 - q.0), q.1 + 2.0 / 3.0 * (c.1 - q.1));
                    self.curve_to(c1.0, c1.1, c2.0, c2.1, q.0, q.1);
                    cur = q;
                }
                Seg::Cubic(a, b, q) => {
                    self.curve_to(a.0, a.1, b.0, b.1, q.0, q.1);
                    cur = q;
                }
                Seg::Close => {
                    self.close();
                    cur = start;
                }
            }
        }
        self
    }

    /// `f`: fill the path, nonzero winding.
    pub fn fill(&mut self) -> &mut Self {
        self.op(&[], "f")
    }
    /// `f*`: fill the path, even-odd.
    pub fn fill_even_odd(&mut self) -> &mut Self {
        self.op(&[], "f*")
    }
    pub fn stroke(&mut self) -> &mut Self {
        self.op(&[], "S")
    }
    pub fn fill_stroke(&mut self) -> &mut Self {
        self.op(&[], "B")
    }
    pub fn end_path(&mut self) -> &mut Self {
        self.op(&[], "n")
    }
    /// `W n`: intersect the clip with the path (nonzero) and paint nothing.
    pub fn clip(&mut self) -> &mut Self {
        self.op(&[], "W n")
    }
    pub fn save(&mut self) -> &mut Self {
        self.op(&[], "q")
    }
    pub fn restore(&mut self) -> &mut Self {
        self.op(&[], "Q")
    }
    /// `cm`: concatenate `[a b c d e f]` to the CTM.
    pub fn transform(&mut self, m: [f64; 6]) -> &mut Self {
        self.op(&m, "cm")
    }
    pub fn line_width(&mut self, w: f64) -> &mut Self {
        self.op(&[w], "w")
    }
    /// Components 0..=1.
    pub fn fill_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.op(&[r, g, b], "rg")
    }
    pub fn stroke_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.op(&[r, g, b], "RG")
    }
    pub fn fill_gray(&mut self, g: f64) -> &mut Self {
        self.op(&[g], "g")
    }

    /// Draw `img` with the unit square mapped by `m` (`[w 0 0 h x y]` places it at x, y, w x h points):
    /// `q m cm /ImN Do Q`, the image an XObject of this page.
    pub fn image(&mut self, img: Image, m: [f64; 6]) -> Result<&mut Self, Error> {
        img.check()?;
        let k = self.images.len();
        self.images.push(img);
        self.save();
        self.transform(m);
        self.content.extend_from_slice(format!("/Im{k} Do\n").as_bytes());
        Ok(self.restore())
    }

    /// PDF-WRITE-FONTS (SR338): `BT /F{id} size Tf x y Td <cids> Tj ET` — `s` in `font` at `size` points with its
    /// baseline origin at (x, y), one glyph per character through font_core's cmap (2-byte CIDs, Identity-H), painted
    /// with the current fill colour.
    pub fn text(&mut self, font: &FontHandle, size: f64, x: f64, y: f64, s: &str) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let (id, cids) = {
            let mut st = font.0.borrow_mut();
            let cids = st.encode(s);
            (st.id, cids)
        };
        self.fonts.insert(id);
        self.content.extend_from_slice(format!("BT\n/F{id} ").as_bytes());
        self.op(&[size], "Tf");
        self.op(&[x, y], "Td");
        self.content.push(b'<');
        for b in cids {
            self.content.push(HEX[(b >> 4) as usize]);
            self.content.push(HEX[(b & 15) as usize]);
        }
        self.content.extend_from_slice(b"> Tj\nET\n");
        self
    }
}

/// A new document: pages in order and an optional title.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    pages: Vec<PageBuilder>,
    title: Option<String>,
    /// PDF-WRITE-FONTS (SR338): the faces loaded by [`Builder::font`], `/F{index}`.
    fonts: Vec<FontHandle>,
}

impl Builder {
    pub fn new() -> Self {
        Builder::default()
    }

    pub fn page(&mut self, p: PageBuilder) -> &mut Self {
        self.pages.push(p);
        self
    }

    /// The /Info /Title.
    pub fn title(&mut self, t: &str) -> &mut Self {
        self.title = Some(String::from(t));
        self
    }

    /// PDF-WRITE-FONTS (SR338): load a TrueType face for [`PageBuilder::text`]; embedded once per document, as the
    /// subset of the glyphs the pages show, when `finish` runs. CFF / OpenType-CFF faces are refused by name.
    pub fn font(&mut self, data: &[u8]) -> Result<FontHandle, Error> {
        let st = fonts::load(self.fonts.len() as u32, data)?;
        let h = FontHandle(Rc::new(RefCell::new(st)));
        self.fonts.push(h.clone());
        Ok(h)
    }

    /// The PDF 1.4 file: 1 the catalog, 2 the page tree, then each page, its content stream (FlateDecode) and its
    /// image XObjects; then each used font's five objects (SR338); /Info last when titled.
    pub fn finish(self) -> Vec<u8> {
        let r = |num: u32| Ref { num, generation: 0 };
        let mut objs: Vec<(Ref, Object)> = Vec::new();
        let mut kids = Vec::new();
        let font_refs = font_numbers(&self.pages, &self.fonts);
        let mut next = 3u32;
        for p in self.pages {
            let (page_num, content_num) = (next, next + 1);
            next += 2;
            let mut xobjects = Dict::new();
            for (k, img) in p.images.iter().enumerate() {
                xobjects.insert(format!("Im{k}").into_bytes(), Object::Ref(r(next)));
                objs.push((r(next), Object::Stream(img.to_stream())));
                next += 1;
            }
            let mut res = Dict::new();
            if !xobjects.is_empty() {
                res.insert(b"XObject".to_vec(), Object::Dict(xobjects));
            }
            if !p.fonts.is_empty() {
                res.insert(b"Font".to_vec(), Object::Dict(font_resources(&p.fonts, &font_refs)));
            }
            let mut page = Dict::new();
            page.insert(b"Type".to_vec(), name(b"Page"));
            page.insert(b"Parent".to_vec(), Object::Ref(r(2)));
            page.insert(b"MediaBox".to_vec(), Object::Array(p.media_box.iter().map(|&v| Object::Real(v)).collect()));
            page.insert(b"Resources".to_vec(), Object::Dict(res));
            page.insert(b"Contents".to_vec(), Object::Ref(r(content_num)));
            objs.push((r(page_num), Object::Dict(page)));
            let mut cd = Dict::new();
            let data = match flate(&p.content) {
                Some(z) => {
                    cd.insert(b"Filter".to_vec(), name(b"FlateDecode"));
                    z
                }
                None => p.content.clone(),
            };
            objs.push((r(content_num), Object::Stream(Stream { dict: cd, data })));
            kids.push(Object::Ref(r(page_num)));
        }
        // SR338: `next` is now `font_numbers`' base; with no text nothing is added and the numbering is SR325's.
        for (id, num) in &font_refs {
            objs.extend(fonts::objects(&self.fonts[*id as usize].0.borrow(), *num));
            next = next.max(num + 5);
        }
        let mut pages = Dict::new();
        pages.insert(b"Type".to_vec(), name(b"Pages"));
        pages.insert(b"Count".to_vec(), Object::Int(kids.len() as i64));
        pages.insert(b"Kids".to_vec(), Object::Array(kids));
        let mut cat = Dict::new();
        cat.insert(b"Type".to_vec(), name(b"Catalog"));
        cat.insert(b"Pages".to_vec(), Object::Ref(r(2)));
        objs.push((r(1), Object::Dict(cat)));
        objs.push((r(2), Object::Dict(pages)));
        let mut trailer = Dict::new();
        trailer.insert(b"Root".to_vec(), Object::Ref(r(1)));
        if let Some(t) = &self.title {
            let mut info = Dict::new();
            info.insert(b"Title".to_vec(), Object::String(text_bytes(t)));
            info.insert(b"Producer".to_vec(), Object::String(b"UnaOS pdf_core".to_vec()));
            objs.push((r(next), Object::Dict(info)));
            trailer.insert(b"Info".to_vec(), Object::Ref(r(next)));
        }
        objs.sort_by_key(|(r, _)| r.num);
        write_file("1.4", &objs, &trailer)
    }
}

/// One frame to print: `width * height * 4` straight RGBA bytes, top row first.
#[derive(Clone, Copy, Debug)]
pub struct RgbaPage<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// "Print what you see" (PDF4's print path): each frame one page of `width * 72 / dpi` x `height * 72 / dpi` points
/// holding one RGB image XObject that fills it, alpha composited over white (the paper).
pub fn from_rgba(pages: &[RgbaPage<'_>], dpi: f64) -> Result<Vec<u8>, Error> {
    if !(dpi > 0.0 && dpi.is_finite()) {
        return Err(Error::Unsupported("from_rgba dpi"));
    }
    let mut b = Builder::new();
    for p in pages {
        let n = p.width as usize * p.height as usize;
        if p.width == 0 || p.height == 0 || p.rgba.len() != n * 4 {
            return Err(Error::Unsupported("from_rgba frame size"));
        }
        let mut rgb = Vec::with_capacity(n * 3);
        for px in p.rgba.chunks_exact(4) {
            let a = px[3] as u32;
            for &c in &px[..3] {
                rgb.push(((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8);
            }
        }
        let (w, h) = (p.width as f64 * 72.0 / dpi, p.height as f64 * 72.0 / dpi);
        let mut pb = PageBuilder::new(w, h);
        pb.image(Image::Rgb { width: p.width, height: p.height, data: rgb }, [w, 0.0, 0.0, h, 0.0, 0.0])?;
        b.page(pb);
    }
    Ok(b.finish())
}

/// PDF-WRITE-FONTS (SR338): each font a page uses -> its Type0 object number, five numbers per font after every page's
/// objects (`3 + sum(2 + images)`), so the page, content and image numbers stay those of a document without text.
fn font_numbers(pages: &[PageBuilder], loaded: &[FontHandle]) -> BTreeMap<u32, u32> {
    let base = 3 + pages.iter().map(|p| 2 + p.images.len() as u32).sum::<u32>();
    let used: BTreeSet<u32> = pages.iter().flat_map(|p| p.fonts.iter().copied()).filter(|&id| (id as usize) < loaded.len()).collect();
    used.into_iter().enumerate().map(|(j, id)| (id, base + 5 * j as u32)).collect()
}

/// A page's /Font dictionary: `/F{id}` -> its Type0 font.
fn font_resources(ids: &BTreeSet<u32>, refs: &BTreeMap<u32, u32>) -> Dict {
    let mut d = Dict::new();
    for id in ids {
        if let Some(&num) = refs.get(id) {
            d.insert(format!("F{id}").into_bytes(), Object::Ref(Ref { num, generation: 0 }));
        }
    }
    d
}
