// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDF's standard security handler (PDFCRYPT, LEDGER SR315; ISO 32000-1 §7.6.3 and
//! ISO 32000-2 §7.6.4 for revision 6): opening an /Encrypt dictionary with a user or owner password and decrypting
//! strings and streams as [`crate::Document`] resolves them. The arithmetic is crypto_core's (MD5, RC4, AES, CBC,
//! SHA-2) — this module is the PDF algorithms over it, never a second primitive.
//!
//! * /Filter /Standard, /V 1 2 4 5, /R 2 3 4 5 6. Public-key handlers (/Adobe.PubSec) and any other /Filter are
//!   refused by name ([`crate::Error::Unsupported`]).
//! * R2-R4: Algorithm 2 (the file key: MD5 over the padded password, /O, /P, ID[0] and, when R >= 4 and
//!   /EncryptMetadata is false, 0xFFFFFFFF; 50 more rounds for R >= 3), checked against /U by Algorithm 4 (R2) or 5
//!   (R3+); an owner password is turned into the user password by Algorithm 7 first.
//! * R5/R6: the SHA-256 (R5) or Algorithm 2.B (R6) hash of the UTF-8 password (at most 127 bytes; SASLprep owed)
//!   with /U's (or /O's) validation salt; the file key unwrapped from /UE (or /OE) with AES-256-CBC, zero IV.
//! * Crypt filters (V4/V5): /CF, /StmF, /StrF (default /Identity), /CFM /None /V2 /AESV2 /AESV3.
//! * Objects: Algorithm 1's per-object key for RC4 and AESV2, the file key for AESV3; AES data is IV + CBC + PKCS #7.
//!   Exempt: the /Encrypt dictionary, /Type /XRef streams, /Type /Metadata streams when /EncryptMetadata is false, a
//!   stream whose first filter is /Crypt naming /Identity, and a signature dictionary's /Contents.

use alloc::vec::Vec;

use crypto_core::aes::Aes;
use crypto_core::cbc;
use crypto_core::ct::{Zeroize, ct_eq};
use crypto_core::md5::{Md5, md5};
use crypto_core::rc4::Rc4;
use crypto_core::sha2::{sha256, sha384, sha512};

use crate::Error;
use crate::object::{Dict, Object, Ref, Stream};

/// §7.6.3.3 Algorithm 2 step a: the 32-byte padding string.
pub const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08, 0x2E, 0x2E, 0x00,
    0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// How one class of data (streams or strings) is encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// /Identity or /CFM /None: stored in the clear.
    None,
    /// RC4 with the per-object key (/V 1, /V 2, /CFM /V2).
    Rc4,
    /// AES-128-CBC with the per-object key (/CFM /AESV2).
    AesV2,
    /// AES-256-CBC with the file key (/CFM /AESV3).
    AesV3,
}

/// An opened standard security handler. The file key is zeroized on drop.
pub struct Handler {
    key: Vec<u8>,
    v: i64,
    r: i64,
    stm: Method,
    strs: Method,
    filters: Vec<(Vec<u8>, Method)>,
    encrypt_metadata: bool,
    encrypt_num: Option<u32>,
    p: i32,
    owner: bool,
}

impl Drop for Handler {
    fn drop(&mut self) {
        self.key.as_mut_slice().zeroize();
    }
}

impl Handler {
    /// Open the /Encrypt dictionary `enc` (its object number `encrypt_num` when indirect) of a file whose first /ID
    /// string is `id0`, with `password` (the user or the owner password; empty for the common no-password file).
    /// `get` resolves indirect values. [`Error::Password`] when `password` is neither; [`Error::Encrypted`] for a
    /// standard dictionary this handler cannot read (missing or short /O /U, an unknown /V or /R).
    pub fn open(
        enc: &Dict,
        id0: &[u8],
        encrypt_num: Option<u32>,
        password: &[u8],
        get: &dyn Fn(&Object) -> Object,
    ) -> Result<Handler, Error> {
        let val = |k: &[u8]| get(enc.get(k).unwrap_or(&Object::Null));
        match val(b"Filter").as_name() {
            Some(b"Standard") => {}
            Some(b"Adobe.PubSec") => return Err(Error::Unsupported("encrypt: public-key security handler")),
            _ => return Err(Error::Unsupported("encrypt: security handler other than /Standard")),
        }
        let v = val(b"V").as_int().unwrap_or(0);
        let r = val(b"R").as_int().ok_or(Error::Encrypted)?;
        let o = val(b"O").as_str().map(<[u8]>::to_vec).ok_or(Error::Encrypted)?;
        let u = val(b"U").as_str().map(<[u8]>::to_vec).ok_or(Error::Encrypted)?;
        // /P is a signed 32-bit integer; some writers store it unsigned.
        let p = val(b"P").as_int().unwrap_or(0) as u32 as i32;
        let encrypt_metadata = !matches!(val(b"EncryptMetadata"), Object::Bool(false));
        let length = val(b"Length").as_int().unwrap_or(40);
        let mut h = Handler {
            key: Vec::new(),
            v,
            r,
            stm: Method::Rc4,
            strs: Method::Rc4,
            filters: Vec::new(),
            encrypt_metadata,
            encrypt_num,
            p,
            owner: false,
        };
        let n = match v {
            1 => 5,
            2 => {
                if length % 8 != 0 || !(40..=128).contains(&length) {
                    return Err(Error::Encrypted);
                }
                (length / 8) as usize
            }
            4 => {
                h.crypt_filters(enc, get)?;
                if (40..=128).contains(&length) && length % 8 == 0 && val(b"Length").as_int().is_some() {
                    (length / 8) as usize
                } else {
                    16
                }
            }
            5 => {
                h.crypt_filters(enc, get)?;
                32
            }
            _ => return Err(Error::Encrypted),
        };
        match (v, r) {
            (1 | 2 | 4, 2..=4) => {
                if o.len() < 32 || u.len() < 32 {
                    return Err(Error::Encrypted);
                }
                let key = h.file_key_r4(password, &o, id0, n);
                if h.user_ok(&key, &u, id0) {
                    h.key = key;
                } else {
                    let mut upw = h.owner_to_user(password, &o, n);
                    let key = h.file_key_r4(&upw, &o, id0, n);
                    upw.zeroize();
                    if !h.user_ok(&key, &u, id0) {
                        return Err(Error::Password);
                    }
                    h.key = key;
                    h.owner = true;
                }
            }
            (5, 5 | 6) => {
                let ue = val(b"UE").as_str().map(<[u8]>::to_vec);
                let oe = val(b"OE").as_str().map(<[u8]>::to_vec);
                if o.len() < 48 || u.len() < 48 {
                    return Err(Error::Encrypted);
                }
                let pw = &password[..password.len().min(127)];
                if ct_eq(&hash_r56(r, pw, &u[32..40], &[]), &u[..32]) {
                    let ue = ue.filter(|e| e.len() >= 32).ok_or(Error::Encrypted)?;
                    h.key = unwrap_key(&hash_r56(r, pw, &u[40..48], &[]), &ue[..32]);
                } else if ct_eq(&hash_r56(r, pw, &o[32..40], &u[..48]), &o[..32]) {
                    let oe = oe.filter(|e| e.len() >= 32).ok_or(Error::Encrypted)?;
                    h.key = unwrap_key(&hash_r56(r, pw, &o[40..48], &u[..48]), &oe[..32]);
                    h.owner = true;
                } else {
                    return Err(Error::Password);
                }
            }
            _ => return Err(Error::Encrypted),
        }
        Ok(h)
    }

    /// /CF, /StmF and /StrF (§7.6.5): every named filter's /CFM; the defaults are /Identity.
    fn crypt_filters(&mut self, enc: &Dict, get: &dyn Fn(&Object) -> Object) -> Result<(), Error> {
        if let Object::Dict(cf) = get(enc.get(b"CF").unwrap_or(&Object::Null)) {
            for (name, f) in cf.iter() {
                let Object::Dict(f) = get(f) else { continue };
                let m = match get(f.get(b"CFM").unwrap_or(&Object::Null)).as_name() {
                    None | Some(b"None") => Method::None,
                    Some(b"V2") => Method::Rc4,
                    Some(b"AESV2") => Method::AesV2,
                    Some(b"AESV3") => Method::AesV3,
                    Some(_) => return Err(Error::Unsupported("encrypt: crypt filter method")),
                };
                self.filters.push((name.to_vec(), m));
            }
        }
        self.stm = self.named(get(enc.get(b"StmF").unwrap_or(&Object::Null)).as_name())?;
        self.strs = self.named(get(enc.get(b"StrF").unwrap_or(&Object::Null)).as_name())?;
        Ok(())
    }

    /// The method a crypt filter name stands for (absent or /Identity = in the clear).
    fn named(&self, name: Option<&[u8]>) -> Result<Method, Error> {
        match name {
            None | Some(b"Identity") => Ok(Method::None),
            Some(n) => self.filters.iter().find(|(k, _)| k.as_slice() == n).map(|(_, m)| *m).ok_or(Error::Encrypted),
        }
    }

    /// Algorithm 2: the R2-R4 file key, `n` bytes.
    fn file_key_r4(&self, password: &[u8], o: &[u8], id0: &[u8], n: usize) -> Vec<u8> {
        let mut padded = pad_password(password);
        let mut h = Md5::new();
        h.update(&padded);
        h.update(&o[..32]);
        h.update(&(self.p as u32).to_le_bytes());
        h.update(id0);
        if self.r >= 4 && !self.encrypt_metadata {
            h.update(&[0xFF; 4]);
        }
        let mut d = h.finalize();
        if self.r >= 3 {
            for _ in 0..50 {
                d = md5(&d[..n]);
            }
        }
        let key = d[..n].to_vec();
        d.zeroize();
        padded.zeroize();
        key
    }

    /// Algorithm 6: does `key` reproduce /U (Algorithm 4 for R2: all 32 bytes; Algorithm 5 for R3+: the first 16)?
    fn user_ok(&self, key: &[u8], u: &[u8], id0: &[u8]) -> bool {
        if self.r == 2 {
            let mut x = PAD;
            rc4_apply(key, &mut x);
            return ct_eq(&x, &u[..32]);
        }
        let mut h = Md5::new();
        h.update(&PAD);
        h.update(id0);
        let mut x = h.finalize();
        rc4_passes(key, &mut x, false);
        ct_eq(&x, &u[..16])
    }

    /// Algorithm 7: the padded user password recovered from /O with an owner password (Algorithm 3 steps a-d for
    /// the RC4 key, then /O decrypted: once for R2, twenty passes with the key XOR 19..0 for R3+).
    fn owner_to_user(&self, password: &[u8], o: &[u8], n: usize) -> Vec<u8> {
        let mut padded = pad_password(password);
        let mut d = md5(&padded);
        if self.r >= 3 {
            for _ in 0..50 {
                d = md5(&d);
            }
        }
        let mut upw = o[..32].to_vec();
        if self.r == 2 {
            rc4_apply(&d[..n], &mut upw);
        } else {
            rc4_passes(&d[..n], &mut upw, true);
        }
        d.zeroize();
        padded.zeroize();
        upw
    }

    /// The method for one stream (the exemptions in the module doc).
    fn stream_method(&self, d: &Dict) -> Method {
        match d.name(b"Type") {
            Some(b"XRef") => return Method::None,
            Some(b"Metadata") if !self.encrypt_metadata => return Method::None,
            _ => {}
        }
        let first = |o: Option<&Object>| -> Option<Object> {
            match o? {
                Object::Array(a) => a.first().cloned(),
                x => Some(x.clone()),
            }
        };
        if first(d.get(b"Filter")).as_ref().and_then(Object::as_name) == Some(&b"Crypt"[..]) {
            let parms = first(d.get(b"DecodeParms"));
            let name = parms.as_ref().and_then(Object::as_dict).and_then(|p| p.name(b"Name"));
            return self.named(name).unwrap_or(self.stm);
        }
        self.stm
    }

    /// Decrypt one top-level object `r` as it was parsed: every string (except a signature's /Contents) and, for a
    /// stream, its data. Exempt objects come back unchanged.
    pub fn decrypt_object(&self, r: Ref, obj: Object) -> Object {
        if self.encrypt_num == Some(r.num) {
            return obj;
        }
        match obj {
            Object::Stream(s) => {
                let m = self.stream_method(&s.dict);
                let dict = self.walk_dict(r, s.dict);
                let data = self.crypt(m, r, s.data);
                Object::Stream(Stream { dict, data })
            }
            o => self.walk(r, o),
        }
    }

    fn walk(&self, r: Ref, o: Object) -> Object {
        match o {
            Object::String(b) => Object::String(self.crypt(self.strs, r, b)),
            Object::Array(a) => Object::Array(a.into_iter().map(|x| self.walk(r, x)).collect()),
            Object::Dict(d) => Object::Dict(self.walk_dict(r, d)),
            o => o,
        }
    }

    fn walk_dict(&self, r: Ref, d: Dict) -> Dict {
        let sig = d.contains(b"ByteRange");
        Dict(
            d.0.into_iter()
                .map(|(k, v)| if sig && k.as_slice() == b"Contents" { (k, v) } else { (k, self.walk(r, v)) })
                .collect(),
        )
    }

    /// Undo `m` on `data` for object `r`.
    fn crypt(&self, m: Method, r: Ref, mut data: Vec<u8>) -> Vec<u8> {
        match m {
            Method::None => data,
            Method::Rc4 => {
                let mut k = self.object_key(r, false);
                rc4_apply(&k, &mut data);
                k.zeroize();
                data
            }
            Method::AesV2 => {
                let mut k = self.object_key(r, true);
                let out = aes_cbc_open(&k, data);
                k.zeroize();
                out
            }
            Method::AesV3 => aes_cbc_open(&self.key, data),
        }
    }

    /// Algorithm 1: MD5(file key, object number (3 bytes LE), generation (2 bytes LE) [, "sAlT" for AES]) cut to
    /// min(n + 5, 16) bytes.
    fn object_key(&self, r: Ref, aes: bool) -> Vec<u8> {
        let mut h = Md5::new();
        h.update(&self.key);
        h.update(&r.num.to_le_bytes()[..3]);
        h.update(&r.generation.to_le_bytes());
        if aes {
            h.update(b"sAlT");
        }
        let mut d = h.finalize();
        let k = d[..(self.key.len() + 5).min(16)].to_vec();
        d.zeroize();
        k
    }

    /// `rc4-40`, `rc4-128`, `rc4` (another RC4 key length), `aesv2`, `aesv3`, or `identity` (crypt filters that
    /// leave streams and strings in the clear) — the stream method, else the string method.
    pub fn kind(&self) -> &'static str {
        let m = if self.stm != Method::None { self.stm } else { self.strs };
        match m {
            Method::None => "identity",
            Method::AesV2 => "aesv2",
            Method::AesV3 => "aesv3",
            Method::Rc4 => match self.key.len() {
                5 => "rc4-40",
                16 => "rc4-128",
                _ => "rc4",
            },
        }
    }

    /// True when the password given was the owner password.
    pub fn owner(&self) -> bool {
        self.owner
    }

    /// /P, the permission bits (§7.6.3.2 Table 22). Read, not enforced here: PDF4's face enforces them (owed).
    pub fn permissions(&self) -> i32 {
        self.p
    }

    /// /V and /R.
    pub fn version(&self) -> (i64, i64) {
        (self.v, self.r)
    }
}

/// Algorithm 2 step a: the first 32 bytes of the password, completed from [`PAD`].
pub fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut out = PAD;
    let n = password.len().min(32);
    out[..n].copy_from_slice(&password[..n]);
    out[n..].copy_from_slice(&PAD[..32 - n]);
    out
}

/// RC4 under `key` (5..=16 bytes here; an empty key leaves the data unchanged).
fn rc4_apply(key: &[u8], data: &mut [u8]) {
    if let Ok(mut s) = Rc4::new(key) {
        s.apply(data);
    }
}

/// Algorithm 5 step e / Algorithm 7 step b: twenty RC4 passes, the key XORed with the pass counter, counting 0..19
/// (`down` false: encryption direction) or 19..0 (`down` true: decryption).
pub fn rc4_passes(key: &[u8], data: &mut [u8], down: bool) {
    let mut k = key.to_vec();
    for step in 0..20u8 {
        let i = if down { 19 - step } else { step };
        for (kb, b) in k.iter_mut().zip(key) {
            *kb = b ^ i;
        }
        rc4_apply(&k, data);
    }
    k.zeroize();
}

/// AES-CBC with the IV as the first 16 bytes and PKCS #7 padding (§7.6.2). A body that is not whole blocks is cut to
/// whole blocks; padding that does not verify keeps every decrypted byte (a viewer shows what it can); less than one
/// IV is empty.
fn aes_cbc_open(key: &[u8], data: Vec<u8>) -> Vec<u8> {
    if data.len() < 16 {
        return Vec::new();
    }
    let Ok(aes) = Aes::new(key) else { return data };
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&data[..16]);
    let mut body = data[16..].to_vec();
    body.truncate(body.len() - body.len() % 16);
    if cbc::decrypt(&aes, &iv, &mut body).is_err() {
        return Vec::new();
    }
    if let Some(n) = cbc::pkcs7_unpad(&body) {
        body.truncate(n);
    }
    body
}

/// /UE or /OE (32 bytes) decrypted with AES-256-CBC, zero IV, no padding, under the intermediate key `ik`.
fn unwrap_key(ik: &[u8; 32], wrapped: &[u8]) -> Vec<u8> {
    let mut key = wrapped[..32].to_vec();
    if let Ok(aes) = Aes::new(ik) {
        let _ = cbc::decrypt(&aes, &[0u8; 16], &mut key);
    }
    key
}

/// The R5 / R6 password hash: SHA-256(password, salt, udata) for R5; ISO 32000-2 Algorithm 2.B for R6 (at least 64
/// rounds of K1 = 64 x (password, K, udata), E = AES-128-CBC(K[0..16], IV K[16..32]) without padding, K = SHA-256 /
/// 384 / 512 of E chosen by the first 16 bytes of E mod 3, until the last byte of E is at most round - 32).
pub fn hash_r56(r: i64, password: &[u8], salt: &[u8], udata: &[u8]) -> [u8; 32] {
    let mut first = Vec::with_capacity(password.len() + salt.len() + udata.len());
    first.extend_from_slice(password);
    first.extend_from_slice(salt);
    first.extend_from_slice(udata);
    let mut k: Vec<u8> = sha256(&first).to_vec();
    first.zeroize();
    if r >= 6 {
        let mut round: u32 = 0;
        loop {
            let unit = password.len() + k.len() + udata.len();
            let mut e = Vec::with_capacity(64 * unit);
            for _ in 0..64 {
                e.extend_from_slice(password);
                e.extend_from_slice(&k);
                e.extend_from_slice(udata);
            }
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&k[16..32]);
            // 64 x unit is a whole number of blocks; the key is 16 bytes, so neither call can fail.
            if let Ok(aes) = Aes::new(&k[..16]) {
                let _ = cbc::encrypt(&aes, &iv, &mut e);
            }
            let sum: u32 = e[..16].iter().map(|&b| b as u32).sum();
            k.as_mut_slice().zeroize();
            k = match sum % 3 {
                0 => sha256(&e).to_vec(),
                1 => sha384(&e).to_vec(),
                _ => sha512(&e).to_vec(),
            };
            round += 1;
            let last = *e.last().unwrap_or(&0) as u32;
            e.as_mut_slice().zeroize();
            if round >= 64 && last + 32 <= round {
                break;
            }
        }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&k[..32]);
    k.as_mut_slice().zeroize();
    out
}
