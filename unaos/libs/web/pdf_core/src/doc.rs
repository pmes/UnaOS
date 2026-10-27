// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The document: objects resolved through the cross-reference, the page tree (ISO 32000-1 §7.7.3) and the
//! document information dictionary (§14.3.3).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::filters::{self, Decoded};
use crate::lexer::{Lexer, Token, find, is_white};
use crate::object::{Dict, Object, Ref, Stream, parse_from, parse_indirect, text_string};
pub use crate::xref::XrefKind;
use crate::xref::{self, XrefEntry};
use crate::{Error, MAX_DEPTH, MAX_PAGES};

/// One page, its inheritable attributes already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// /MediaBox as `[llx, lly, urx, ury]` (normalised so `ll < ur`; US Letter when absent).
    pub media_box: [f64; 4],
    /// /CropBox (the media box when absent).
    pub crop_box: [f64; 4],
    /// /Rotate, normalised to 0, 90, 180 or 270.
    pub rotate: i32,
    /// /Resources (empty when absent).
    pub resources: Dict,
    /// Every /Contents stream decoded and joined with a newline.
    pub contents: Vec<u8>,
}

/// The document information dictionary's text strings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
}

/// A decoded object stream: its data, where its objects start, and the (number, offset) pairs.
struct ObjStm {
    data: Vec<u8>,
    first: usize,
    pairs: Vec<(u32, usize)>,
}

/// A parsed PDF. Single owner; the caches make `resolve` take `&self`.
pub struct Document {
    buf: Vec<u8>,
    entries: BTreeMap<u32, XrefEntry>,
    trailer: Dict,
    kind: XrefKind,
    version: String,
    cache: RefCell<BTreeMap<(u32, u16), Object>>,
    objstms: RefCell<BTreeMap<u32, ObjStm>>,
    busy: RefCell<BTreeSet<u32>>,
    crypt: Option<crate::crypt::Handler>,
}

impl Document {
    /// Parse a PDF with the empty user password: an unencrypted file, or an encrypted one that opens without a
    /// password (the common case). [`Document::parse_with_password`] for any other.
    pub fn parse(bytes: &[u8]) -> Result<Document, Error> {
        Document::parse_with_password(bytes, b"")
    }

    /// Parse a PDF: header, cross-reference (reconstructed when broken), trailer. An /Encrypt dictionary is opened
    /// with `password`, the user or the owner password (`crate::crypt`, PDFCRYPT SR315): [`Error::Password`] when it is
    /// neither, [`Error::Encrypted`] for a standard dictionary the handler cannot read.
    pub fn parse_with_password(bytes: &[u8], password: &[u8]) -> Result<Document, Error> {
        let head = &bytes[..bytes.len().min(1024)];
        let version = match find(head, b"%PDF-", 0) {
            Some(p) => {
                let mut e = p + 5;
                while e < bytes.len() && !is_white(bytes[e]) && e - p < 16 {
                    e += 1;
                }
                String::from_utf8_lossy(&bytes[p + 5..e]).into_owned()
            }
            None => String::new(),
        };
        if let Ok(loaded) = xref::load(bytes) {
            let mut doc = Document::new(bytes, loaded.entries, loaded.trailer, loaded.kind, version.clone());
            if doc.is_encrypted() {
                doc.open_crypt(password)?;
            }
            if doc.catalog().is_some() {
                return Ok(doc);
            }
        }
        Document::rebuild(bytes, version, password)
    }

    fn new(bytes: &[u8], entries: BTreeMap<u32, XrefEntry>, trailer: Dict, kind: XrefKind, version: String) -> Self {
        Document {
            buf: bytes.to_vec(),
            entries,
            trailer,
            kind,
            version,
            cache: RefCell::new(BTreeMap::new()),
            objstms: RefCell::new(BTreeMap::new()),
            busy: RefCell::new(BTreeSet::new()),
            crypt: None,
        }
    }

    /// The reconstruction path: scan for objects, add every object stream's members, find /Root.
    fn rebuild(bytes: &[u8], version: String, password: &[u8]) -> Result<Document, Error> {
        let (entries, trailer) = xref::reconstruct(bytes);
        let mut doc = Document::new(bytes, entries, trailer, XrefKind::Rebuilt, version);
        let (mut catalog, xref_dict) = doc.scan_members();
        if !doc.trailer.contains(b"Root") {
            if let Some(d) = xref_dict {
                for (k, v) in d.iter() {
                    if matches!(k, b"Root" | b"Info" | b"Encrypt" | b"ID") {
                        doc.trailer.insert(k.to_vec(), v.clone());
                    }
                }
            }
        }
        doc.cache.borrow_mut().clear();
        if doc.is_encrypted() {
            // The object streams decode only once the key is known: scan again.
            doc.open_crypt(password)?;
            catalog = doc.scan_members().0.or(catalog);
            doc.cache.borrow_mut().clear();
        }
        if doc.catalog().is_none() {
            // An object stream may hold the catalog; look again now that its members are listed.
            if catalog.is_none() {
                let nums: Vec<u32> = doc.entries.keys().copied().collect();
                catalog = nums.into_iter().find(|&n| {
                    doc.resolve(Ref { num: n, generation: 0 })
                        .ok()
                        .is_some_and(|o| o.as_dict().is_some_and(|d| d.name(b"Type") == Some(&b"Catalog"[..])))
                });
            }
            let Some(c) = catalog else { return Err(Error::Missing) };
            doc.trailer.insert(b"Root".to_vec(), Object::Ref(Ref { num: c, generation: 0 }));
        }
        if doc.catalog().is_none() {
            return Err(Error::Missing);
        }
        Ok(doc)
    }

    /// The header's version (`1.7`), empty when the file has no `%PDF-` header.
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn xref_kind(&self) -> XrefKind {
        self.kind
    }

    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    /// How many objects the cross-reference names.
    pub fn object_count(&self) -> usize {
        self.entries.len()
    }

    fn is_encrypted(&self) -> bool {
        self.trailer.get(b"Encrypt").is_some_and(|o| !o.is_null())
    }

    /// The document catalog (/Root), when it resolves to a dictionary.
    pub fn catalog(&self) -> Option<Dict> {
        match self.get(self.trailer.get(b"Root")?) {
            Object::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// Resolve an indirect reference. A free or absent object is null (§7.3.10).
    pub fn resolve(&self, r: Ref) -> Result<Object, Error> {
        self.resolve_depth(r, 0)
    }

    fn resolve_depth(&self, r: Ref, depth: usize) -> Result<Object, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Depth);
        }
        if let Some(o) = self.cache.borrow().get(&(r.num, r.generation)) {
            return Ok(o.clone());
        }
        if !self.busy.borrow_mut().insert(r.num) {
            return Err(Error::Loop);
        }
        let out = self.load(r, depth);
        self.busy.borrow_mut().remove(&r.num);
        let obj = out?;
        self.cache.borrow_mut().insert((r.num, r.generation), obj.clone());
        Ok(obj)
    }

    fn load(&self, r: Ref, depth: usize) -> Result<Object, Error> {
        match self.entries.get(&r.num).copied() {
            None | Some(XrefEntry::Free) => Ok(Object::Null),
            Some(XrefEntry::InFile { offset, .. }) => {
                if offset >= self.buf.len() {
                    return Err(Error::Syntax("object offset past the end", offset));
                }
                let mut len_of = |lr: Ref| self.resolve_depth(lr, depth + 1).ok().and_then(|o| o.as_int());
                let (got, obj) = parse_indirect(&self.buf, offset, &mut len_of)?;
                if got.num != r.num {
                    return Err(Error::Syntax("xref offset names another object", offset));
                }
                Ok(self.decrypt(got, obj))
            }
            Some(XrefEntry::InStream { stm, idx }) => {
                self.load_objstm(stm, depth + 1)?;
                let cache = self.objstms.borrow();
                let o = cache.get(&stm).ok_or(Error::Missing)?;
                // The pair at `idx` should name this object; when it does not, look for it by number.
                let off = match o.pairs.get(idx as usize) {
                    Some(&(n, off)) if n == r.num => off,
                    _ => match o.pairs.iter().find(|(n, _)| *n == r.num) {
                        Some(&(_, off)) => off,
                        None => return Ok(Object::Null),
                    },
                };
                let at = o.first.saturating_add(off);
                if at >= o.data.len() {
                    return Err(Error::Syntax("object stream offset past its data", at));
                }
                let mut lx = Lexer::at(&o.data, at);
                match lx.next() {
                    Some(t) => parse_from(t, &mut lx, 0),
                    None => Ok(Object::Null),
                }
            }
        }
    }

    /// Decode object stream `num` (§7.5.7) into the cache once.
    fn load_objstm(&self, num: u32, depth: usize) -> Result<(), Error> {
        if self.objstms.borrow().contains_key(&num) {
            return Ok(());
        }
        let obj = self.resolve_depth(Ref { num, generation: 0 }, depth + 1)?;
        let Object::Stream(s) = obj else { return Err(Error::Syntax("object stream is not a stream", 0)) };
        let n = s.dict.get(b"N").map(|o| self.get(o)).and_then(|o| o.as_int()).unwrap_or(0);
        let first = s.dict.get(b"First").map(|o| self.get(o)).and_then(|o| o.as_int()).unwrap_or(0);
        if n < 0 || first < 0 || n as usize > crate::MAX_OBJECTS {
            return Err(Error::Syntax("object stream /N or /First", 0));
        }
        let data = self.decode(&s)?.data;
        let mut lx = Lexer::new(&data);
        let mut pairs = Vec::new();
        for _ in 0..n {
            match (lx.next(), lx.next()) {
                (Some(Token::Int(a)), Some(Token::Int(b))) if a >= 0 && b >= 0 && a <= u32::MAX as i64 => {
                    pairs.push((a as u32, b as usize))
                }
                _ => break,
            }
        }
        self.objstms.borrow_mut().insert(num, ObjStm { data, first: first as usize, pairs });
        Ok(())
    }

    /// Follow references (up to [`MAX_DEPTH`]) to a direct object; anything unresolvable is null.
    pub fn get(&self, o: &Object) -> Object {
        let mut cur = o.clone();
        for _ in 0..MAX_DEPTH {
            match cur {
                Object::Ref(r) => cur = self.resolve(r).unwrap_or(Object::Null),
                other => return other,
            }
        }
        Object::Null
    }

    /// Undo a stream's filters, resolving indirect filter parameters through this document.
    pub fn decode(&self, s: &Stream) -> Result<Decoded, Error> {
        filters::decode(s, &|o| self.get(o))
    }

    /// The page tree walked in order (§7.7.3): /Kids, inherited /Resources /MediaBox /CropBox /Rotate; a node
    /// reached twice is a loop.
    pub fn pages(&self) -> Result<Vec<Page>, Error> {
        let cat = self.catalog().ok_or(Error::Missing)?;
        let root = cat.get(b"Pages").ok_or(Error::Missing)?;
        let mut visited = BTreeSet::new();
        if let Some(r) = root.as_ref() {
            visited.insert(r);
        }
        let node = match self.get(root) {
            Object::Dict(d) => d,
            _ => return Err(Error::Missing),
        };
        let mut out = Vec::new();
        self.walk(&node, Inherit::default(), 0, &mut visited, &mut out)?;
        Ok(out)
    }

    fn walk(
        &self,
        node: &Dict,
        mut inh: Inherit,
        depth: usize,
        visited: &mut BTreeSet<Ref>,
        out: &mut Vec<Page>,
    ) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Depth);
        }
        if let Object::Dict(d) = self.get(node.get(b"Resources").unwrap_or(&Object::Null)) {
            inh.resources = Some(d);
        }
        if let Some(b) = self.rect(node.get(b"MediaBox")) {
            inh.media = Some(b);
        }
        if let Some(b) = self.rect(node.get(b"CropBox")) {
            inh.crop = Some(b);
        }
        if let Some(r) = node.get(b"Rotate").map(|o| self.get(o)).and_then(|o| o.as_int()) {
            inh.rotate = Some(r);
        }
        let is_tree = match node.name(b"Type") {
            Some(b"Pages") => true,
            Some(b"Page") => false,
            _ => node.contains(b"Kids"),
        };
        if is_tree {
            let kids = self.get(node.get(b"Kids").unwrap_or(&Object::Null));
            for kid in kids.as_array().unwrap_or(&[]) {
                if let Some(r) = kid.as_ref() {
                    if !visited.insert(r) {
                        return Err(Error::Loop);
                    }
                }
                if let Object::Dict(d) = self.get(kid) {
                    self.walk(&d, inh.clone(), depth + 1, visited, out)?;
                }
            }
            return Ok(());
        }
        if out.len() >= MAX_PAGES {
            return Err(Error::TooLarge);
        }
        let media = inh.media.unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let rot = inh.rotate.unwrap_or(0).rem_euclid(360);
        let rotate = (((rot + 45) / 90) % 4 * 90) as i32;
        out.push(Page {
            media_box: media,
            crop_box: inh.crop.unwrap_or(media),
            rotate,
            resources: inh.resources.unwrap_or_default(),
            contents: self.contents(node.get(b"Contents")),
        });
        Ok(())
    }

    /// Every /Contents stream decoded and joined with a newline; a stream that does not decode is skipped (a viewer
    /// draws what it can).
    fn contents(&self, c: Option<&Object>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut add = |o: Object| {
            if let Object::Stream(s) = o {
                if let Ok(d) = self.decode(&s) {
                    if !out.is_empty() {
                        out.push(b'\n');
                    }
                    out.extend_from_slice(&d.data);
                }
            }
        };
        match self.get(c.unwrap_or(&Object::Null)) {
            Object::Array(a) => a.iter().for_each(|o| add(self.get(o))),
            o => add(o),
        }
        out
    }

    fn rect(&self, o: Option<&Object>) -> Option<[f64; 4]> {
        let a = self.get(o?);
        let a = a.as_array()?;
        if a.len() != 4 {
            return None;
        }
        let mut v = [0.0; 4];
        for (slot, x) in v.iter_mut().zip(a) {
            *slot = self.get(x).as_f64()?;
        }
        Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
    }

    /// The /Info dictionary's text strings.
    pub fn info(&self) -> Info {
        let Object::Dict(d) = self.get(self.trailer.get(b"Info").unwrap_or(&Object::Null)) else {
            return Info::default();
        };
        let s = |k: &[u8]| d.get(k).map(|o| self.get(o)).and_then(|o| o.as_str().map(text_string));
        Info {
            title: s(b"Title"),
            author: s(b"Author"),
            subject: s(b"Subject"),
            keywords: s(b"Keywords"),
            creator: s(b"Creator"),
            producer: s(b"Producer"),
        }
    }

    /// The reconstruction scan (§7.5 fallback): every object stream's members listed in `entries` (an entry the scan
    /// already has wins), and the catalog and an xref-stream dictionary that names /Root, when found.
    fn scan_members(&mut self) -> (Option<u32>, Option<Dict>) {
        let nums: Vec<u32> = self.entries.keys().copied().collect();
        let mut extra = Vec::new();
        let mut catalog = None;
        let mut xref_dict = None;
        for num in nums {
            let Ok(obj) = self.resolve(Ref { num, generation: 0 }) else { continue };
            let Some(d) = obj.as_dict() else { continue };
            match d.name(b"Type") {
                Some(b"ObjStm") => {
                    if self.load_objstm(num, 0).is_ok() {
                        if let Some(o) = self.objstms.borrow().get(&num) {
                            for (idx, (n, _)) in o.pairs.iter().enumerate() {
                                extra.push((*n, XrefEntry::InStream { stm: num, idx: idx as u32 }));
                            }
                        }
                    }
                }
                Some(b"Catalog") => catalog = Some(num),
                Some(b"XRef") if d.contains(b"Root") => xref_dict = Some(d.clone()),
                _ => {}
            }
        }
        for (n, e) in extra {
            self.entries.entry(n).or_insert(e);
        }
        (catalog, xref_dict)
    }

    /// Open the trailer's /Encrypt with `password` (`crate::crypt::Handler::open`), then drop every object and object
    /// stream decoded before the key was known.
    fn open_crypt(&mut self, password: &[u8]) -> Result<(), Error> {
        let enc_obj = self.trailer.get(b"Encrypt").cloned().unwrap_or(Object::Null);
        let encrypt_num = enc_obj.as_ref().map(|r| r.num);
        let Object::Dict(enc) = self.get(&enc_obj) else { return Err(Error::Encrypted) };
        let id0 = match self.get(self.trailer.get(b"ID").unwrap_or(&Object::Null)) {
            Object::Array(a) => a.first().map(|o| self.get(o)).and_then(|o| o.as_str().map(<[u8]>::to_vec)),
            _ => None,
        }
        .unwrap_or_default();
        let handler = crate::crypt::Handler::open(&enc, &id0, encrypt_num, password, &|o| self.get(o))?;
        self.crypt = Some(handler);
        self.cache.borrow_mut().clear();
        self.objstms.borrow_mut().clear();
        Ok(())
    }

    /// The one decryption hook: a top-level object as parsed, through the security handler when the file has one.
    fn decrypt(&self, r: Ref, obj: Object) -> Object {
        match &self.crypt {
            Some(h) => h.decrypt_object(r, obj),
            None => obj,
        }
    }

    /// `none`, or the security handler's kind (`crate::crypt::Handler::kind`: rc4-40, rc4-128, aesv2, aesv3, ...).
    pub fn encryption(&self) -> &'static str {
        self.crypt.as_ref().map_or("none", |h| h.kind())
    }

    /// The opened security handler, when the file is encrypted.
    pub fn security(&self) -> Option<&crate::crypt::Handler> {
        self.crypt.as_ref()
    }
}

#[derive(Clone, Default)]
struct Inherit {
    resources: Option<Dict>,
    media: Option<[f64; 4]>,
    crop: Option<[f64; 4]>,
    rotate: Option<i64>,
}
