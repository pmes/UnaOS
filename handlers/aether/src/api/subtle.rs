//! AETHERWPT2 (LEDGER SR135): `crypto.subtle` (W3C Web Cryptography API) over UnaOS's own crypto_core — the
//! symmetric half: `digest` (SHA-1 / SHA-256 / SHA-384 / SHA-512), HMAC (`generateKey`, `importKey` /
//! `exportKey` "raw", `sign`, `verify`), AES-GCM and AES-CTR (`generateKey`, `importKey` / `exportKey` "raw",
//! `encrypt`, `decrypt`), HKDF and PBKDF2 (`importKey` "raw", `deriveBits`, `deriveKey`), and `CryptoKey`.
//!
//! The split follows `api::platform`: the arithmetic is a handful of natives here (each answers an ArrayBuffer,
//! or null for the specification's OperationError); the WebIDL shape — normalizing an algorithm (§18.4.4), the
//! usage and length checks, the general steps of each method (§14.3), promise rejection for every error — is the
//! JS prelude [`SUBTLE`]. Not here (AETHERWPT2 gap O7): ECDSA / ECDH / Ed25519 / X25519 / RSA (crypto_core holds
//! their primitives), the `jwk` / `spki` / `pkcs8` formats, `wrapKey` / `unwrapKey`, AES-KW, and AES-CBC, whose
//! decryption needs the AES inverse cipher crypto_core's `aes` does not have (encrypt-only).

use crate::js::idl::arg;
use crypto_core::aes::Aes;
use crypto_core::gcm::AesGcm;
use crypto_core::hmac::Hmac;
use crypto_core::{Digest, Sha1, Sha256, Sha384, Sha512};
use js_core::builtins::typedarray::new_array_buffer;
use js_core::vm::*;

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

fn bytes_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> Vec<u8> {
    let v = arg(vm, ctx, i);
    crate::js::net::buffer_bytes(vm, &v).unwrap_or_default()
}

fn num_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<f64> {
    let v = arg(vm, ctx, i);
    vm.to_number(&v)
}

/// A new ArrayBuffer holding `data`.
fn array_buffer(vm: &mut Vm, data: Vec<u8>) -> JsResult<Value> {
    let b = new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(b).kind {
        d.data = data;
    }
    Ok(Value::Object(b))
}

fn answer(vm: &mut Vm, r: Option<Vec<u8>>) -> JsResult<Value> {
    match r {
        Some(b) => array_buffer(vm, b),
        None => Ok(Value::Null),
    }
}

fn digest_with<D: Digest>(data: &[u8]) -> Vec<u8> {
    let mut h = D::new();
    h.update(data);
    let mut out = vec![0u8; D::OUTPUT_LEN];
    h.finalize_into(&mut out);
    out
}

/// FIPS 180-4 by WebCrypto name.
pub fn digest(name: &str, data: &[u8]) -> Option<Vec<u8>> {
    Some(match name {
        "SHA-1" => digest_with::<Sha1>(data),
        "SHA-256" => digest_with::<Sha256>(data),
        "SHA-384" => digest_with::<Sha384>(data),
        "SHA-512" => digest_with::<Sha512>(data),
        _ => return None,
    })
}

fn hmac_with<D: Digest>(key: &[u8], data: &[u8]) -> Vec<u8> {
    let m = Hmac::<D>::new(key);
    let mut out = vec![0u8; D::OUTPUT_LEN];
    m.mac_into(data, &mut out);
    out
}

/// HMAC (RFC 2104) under the named hash.
pub fn hmac(hash: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    Some(match hash {
        "SHA-1" => hmac_with::<Sha1>(key, data),
        "SHA-256" => hmac_with::<Sha256>(key, data),
        "SHA-384" => hmac_with::<Sha384>(key, data),
        "SHA-512" => hmac_with::<Sha512>(key, data),
        _ => return None,
    })
}

/// HKDF (RFC 5869) of `len` bytes; None when `len` is over 255 hash lengths.
pub fn hkdf(hash: &str, ikm: &[u8], salt: &[u8], info: &[u8], len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; len];
    let ok = match hash {
        "SHA-1" => crypto_core::hkdf::hkdf::<Sha1>(salt, ikm, info, &mut out),
        "SHA-256" => crypto_core::hkdf::hkdf::<Sha256>(salt, ikm, info, &mut out),
        "SHA-384" => crypto_core::hkdf::hkdf::<Sha384>(salt, ikm, info, &mut out),
        "SHA-512" => crypto_core::hkdf::hkdf::<Sha512>(salt, ikm, info, &mut out),
        _ => return None,
    };
    ok.ok().map(|_| out)
}

/// PBKDF2-HMAC (RFC 8018 §5.2) of `len` bytes.
pub fn pbkdf2(hash: &str, password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; len];
    let ok = match hash {
        "SHA-1" => crypto_core::pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, iterations, &mut out),
        "SHA-256" => crypto_core::pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut out),
        "SHA-384" => crypto_core::pbkdf2::pbkdf2_hmac::<Sha384>(password, salt, iterations, &mut out),
        "SHA-512" => crypto_core::pbkdf2::pbkdf2_hmac::<Sha512>(password, salt, iterations, &mut out),
        _ => return None,
    };
    ok.ok().map(|_| out)
}

/// AES-GCM (SP 800-38D): encryption answers `ciphertext || tag[..tag_len]`; decryption takes the same shape and
/// answers the plaintext, None when the tag does not verify (or the IV is empty, or a length is out of range).
pub fn aes_gcm(encrypt: bool, key: &[u8], iv: &[u8], aad: &[u8], tag_len: usize, data: &[u8]) -> Option<Vec<u8>> {
    let g = AesGcm::new(key).ok()?;
    if !crypto_core::gcm::tag_len_ok(tag_len) {
        return None;
    }
    if encrypt {
        let mut buf = data.to_vec();
        let tag = g.encrypt_in_place_detached(iv, aad, &mut buf).ok()?;
        buf.extend_from_slice(&tag[..tag_len]);
        Some(buf)
    } else {
        if data.len() < tag_len {
            return None;
        }
        let (ct, tag) = data.split_at(data.len() - tag_len);
        let mut buf = ct.to_vec();
        g.decrypt_in_place_detached(iv, aad, &mut buf, tag).ok()?;
        Some(buf)
    }
}

/// AES-CTR (SP 800-38A §6.5) with WebCrypto's counter: the rightmost `length` bits of the 16-byte counter block
/// increment modulo 2^length, the rest stays fixed; None when the data needs more blocks than the counter has
/// values (the counter would repeat), or a length is out of range.
pub fn aes_ctr(key: &[u8], counter: &[u8], length: u32, data: &[u8]) -> Option<Vec<u8>> {
    let a = Aes::new(key).ok()?;
    if counter.len() != 16 || length == 0 || length > 128 {
        return None;
    }
    let blocks = data.len().div_ceil(16) as u128;
    if length < 128 && blocks > (1u128 << length) {
        return None;
    }
    let mask: u128 = if length == 128 { u128::MAX } else { (1u128 << length) - 1 };
    let mut ctr = u128::from_be_bytes(counter.try_into().ok()?);
    let mut out = data.to_vec();
    for chunk in out.chunks_mut(16) {
        let mut ks = ctr.to_be_bytes();
        a.encrypt_block(&mut ks);
        for (b, k) in chunk.iter_mut().zip(ks.iter()) {
            *b ^= k;
        }
        ctr = (ctr & !mask) | (ctr.wrapping_add(1) & mask);
    }
    Some(out)
}

fn n_digest(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = str_arg(vm, ctx, 0)?;
    let data = bytes_arg(vm, ctx, 1);
    let r = digest(&name, &data);
    answer(vm, r)
}

fn n_hmac(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let hash = str_arg(vm, ctx, 0)?;
    let (key, data) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2));
    let r = hmac(&hash, &key, &data);
    answer(vm, r)
}

/// `__subtle_hmac_verify(hash, key, data, signature)`: HMAC verify (WebCrypto §29.3: the signature equals the
/// MAC), compared in constant time; a signature of another length is false.
fn n_hmac_verify(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let hash = str_arg(vm, ctx, 0)?;
    let (key, data, sig) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
    let ok = hmac(&hash, &key, &data).is_some_and(|m| m.len() == sig.len() && crypto_core::ct::ct_eq(&m, &sig));
    Ok(Value::Bool(ok))
}

fn n_hkdf(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let hash = str_arg(vm, ctx, 0)?;
    let (ikm, salt, info) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
    let len = num_arg(vm, ctx, 4)? as usize;
    let r = hkdf(&hash, &ikm, &salt, &info, len);
    answer(vm, r)
}

fn n_pbkdf2(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let hash = str_arg(vm, ctx, 0)?;
    let (pw, salt) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2));
    let iterations = num_arg(vm, ctx, 3)? as u32;
    let len = num_arg(vm, ctx, 4)? as usize;
    let r = pbkdf2(&hash, &pw, &salt, iterations, len);
    answer(vm, r)
}

fn n_aes_gcm(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let enc = arg(vm, ctx, 0);
    let encrypt = vm.to_boolean(&enc);
    let (key, iv, aad) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
    let tag_len = num_arg(vm, ctx, 4)? as usize;
    let data = bytes_arg(vm, ctx, 5);
    let r = aes_gcm(encrypt, &key, &iv, &aad, tag_len, &data);
    answer(vm, r)
}

fn n_aes_ctr(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (key, counter) = (bytes_arg(vm, ctx, 0), bytes_arg(vm, ctx, 1));
    let length = num_arg(vm, ctx, 2)? as u32;
    let data = bytes_arg(vm, ctx, 3);
    let r = aes_ctr(&key, &counter, length, &data);
    answer(vm, r)
}

/// Installs the natives and the prelude into a realm whose platform prelude defined `crypto` (a page, or a
/// dedicated worker: `api::platform::init` / `init_worker`).
pub fn init(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__subtle_digest", 2, n_digest as NativeFn),
        ("__subtle_hmac", 3, n_hmac),
        ("__subtle_hmac_verify", 4, n_hmac_verify),
        ("__subtle_hkdf", 5, n_hkdf),
        ("__subtle_pbkdf2", 5, n_pbkdf2),
        ("__subtle_aes_gcm", 6, n_aes_gcm),
        ("__subtle_aes_ctr", 4, n_aes_ctr),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    if let Err(e) = vm.run_script_str(SUBTLE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("subtle-prelude-failed:{}", crate::js::clip(&m, 64)));
        eprintln!("[aether] subtle prelude failed: {m}");
    }
}

/// The SubtleCrypto / CryptoKey shapes over the natives above (WebCrypto §13, §14, §18.4.4).
const SUBTLE: &str = r#"
(function () {
    if (typeof crypto !== 'object' || crypto === null || typeof __subtle_digest !== 'function') { return; }
    var op = function (o, n, f) { Object.defineProperty(o, n, { value: f, writable: true, enumerable: true, configurable: true }); };
    var getter = function (o, n, f) { Object.defineProperty(o, n, { get: f, enumerable: true, configurable: true }); };
    var err = function (name, msg) { return new DOMException(msg, name); };
    var str = function (v) {
        if (typeof v === 'symbol') { throw new TypeError('Cannot convert a Symbol value to a string'); }
        return String(v);
    };
    // BufferSource → a copy of its bytes (WebCrypto §14.3: "get a copy of the bytes held by").
    var bytesOf = function (v, what) {
        if (v instanceof ArrayBuffer) { return new Uint8Array(v.slice(0)); }
        if (typeof SharedArrayBuffer !== 'undefined' && v instanceof SharedArrayBuffer) { return new Uint8Array(v.slice(0)); }
        if (ArrayBuffer.isView(v)) { return new Uint8Array(v.buffer, v.byteOffset, v.byteLength).slice(); }
        throw new TypeError(what + " is not of type '(ArrayBuffer or ArrayBufferView)'.");
    };
    // [EnforceRange] integer conversion (WebIDL §3.2.4.9).
    var enforce = function (v, max, what) {
        var n = +v;
        if (!isFinite(n)) { throw new TypeError(what + ' is not a finite number.'); }
        n = Math.trunc(n);
        if (n < 0 || n > max) { throw new TypeError(what + ' is outside the range [0, ' + max + '].'); }
        return n === 0 ? 0 : n;
    };
    var USAGES = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits', 'wrapKey', 'unwrapKey'];
    var usagesOf = function (v) {
        if (v === null || typeof v !== 'object' || typeof v[Symbol.iterator] !== 'function') {
            throw new TypeError("The provided value cannot be converted to a sequence.");
        }
        var out = [];
        for (var u of v) {
            u = str(u);
            if (USAGES.indexOf(u) < 0) { throw new TypeError("The provided value '" + u + "' is not a valid enum value of type KeyUsage."); }
            out.push(u);
        }
        return out;
    };
    // The usage intersection, in KeyUsage enumeration order.
    var canonical = function (us) { return USAGES.filter(function (u) { return us.indexOf(u) >= 0; }); };
    var only = function (us, allowed) {
        for (var i = 0; i < us.length; i++) {
            if (allowed.indexOf(us[i]) < 0) { throw err('SyntaxError', "Cannot create a key using the specified key usages."); }
        }
    };
    var abuf = function (u8) { return u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength); };

    // ---- CryptoKey (WebCrypto §13) ---------------------------------------------------------------
    var KS = new WeakMap();
    var ks = function (o) {
        var s = (o !== null && typeof o === 'object') ? KS.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var isKey = function (o) { return o !== null && typeof o === 'object' && KS.has(o); };
    class CryptoKey {
        constructor() { throw new TypeError('Illegal constructor'); }
    }
    getter(CryptoKey.prototype, 'type', function () { return ks(this).type; });
    getter(CryptoKey.prototype, 'extractable', function () { return ks(this).extractable; });
    getter(CryptoKey.prototype, 'algorithm', function () { return ks(this).algObj; });
    getter(CryptoKey.prototype, 'usages', function () { return ks(this).usageObj; });
    Object.defineProperty(CryptoKey.prototype, Symbol.toStringTag, { value: 'CryptoKey', configurable: true });
    var algView = function (a) {
        var o = { name: a.name };
        if (a.length !== undefined) { o.length = a.length; }
        if (a.hash !== undefined) { o.hash = { name: a.hash }; }
        return o;
    };
    var mkKey = function (alg, extractable, usages, data) {
        var k = Object.create(CryptoKey.prototype);
        KS.set(k, { type: 'secret', extractable: extractable, alg: alg, algObj: algView(alg), usages: usages, usageObj: usages.slice(), data: data });
        return k;
    };

    // ---- normalize an algorithm (WebCrypto §18.4.4) -------------------------------------------------
    var HASHES = ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
    var BLOCK = { 'SHA-1': 512, 'SHA-256': 512, 'SHA-384': 1024, 'SHA-512': 1024 };
    // op → registered algorithm name → [member, type, required] in dictionary member order.
    var hashM = ['hash', 'hash', true];
    var REG = {
        digest: { 'SHA-1': [], 'SHA-256': [], 'SHA-384': [], 'SHA-512': [] },
        generateKey: { 'HMAC': [hashM, ['length', 'ulong', false]], 'AES-GCM': [['length', 'ushort', true]], 'AES-CTR': [['length', 'ushort', true]] },
        importKey: { 'HMAC': [hashM, ['length', 'ulong', false]], 'AES-GCM': [], 'AES-CTR': [], 'HKDF': [], 'PBKDF2': [] },
        exportKey: { 'HMAC': [], 'AES-GCM': [], 'AES-CTR': [] },
        sign: { 'HMAC': [] },
        verify: { 'HMAC': [] },
        encrypt: { 'AES-GCM': [['additionalData', 'bytes', false], ['iv', 'bytes', true], ['tagLength', 'octet', false]], 'AES-CTR': [['counter', 'bytes', true], ['length', 'octet', true]] },
        deriveBits: { 'HKDF': [hashM, ['info', 'bytes', true], ['salt', 'bytes', true]], 'PBKDF2': [hashM, ['iterations', 'ulong', true], ['salt', 'bytes', true]] },
        'get key length': { 'HMAC': [hashM, ['length', 'ulong', false]], 'AES-GCM': [['length', 'ushort', true]], 'AES-CTR': [['length', 'ushort', true]], 'HKDF': [], 'PBKDF2': [] },
    };
    REG.decrypt = REG.encrypt;
    var normalize = function (alg, opname) {
        var a;
        if (typeof alg === 'string') { a = { name: alg }; }
        else if (alg !== null && (typeof alg === 'object' || typeof alg === 'function')) { a = alg; }
        else { a = { name: str(alg) }; }
        var nameV = a.name;
        if (nameV === undefined) { throw new TypeError("Failed to read the 'name' property from 'Algorithm': Required member is undefined."); }
        var name = str(nameV);
        var reg = REG[opname], found = null;
        for (var k in reg) { if (k.toLowerCase() === name.toLowerCase()) { found = k; } }
        if (found === null) { throw err('NotSupportedError', "Algorithm: Unrecognized name"); }
        var out = { name: found };
        var members = reg[found];
        for (var i = 0; i < members.length; i++) {
            var m = members[i], v = a[m[0]];
            if (v === undefined) {
                if (m[2]) { throw new TypeError("Failed to read the '" + m[0] + "' property from '" + found + "Params': Required member is undefined."); }
                continue;
            }
            if (m[1] === 'hash') { out.hash = normalize(v, 'digest').name; }
            else if (m[1] === 'bytes') { out[m[0]] = bytesOf(v, "The '" + m[0] + "' member"); }
            else if (m[1] === 'ulong') { out[m[0]] = enforce(v, 4294967295, "The '" + m[0] + "' member"); }
            else if (m[1] === 'ushort') { out[m[0]] = enforce(v, 65535, "The '" + m[0] + "' member"); }
            else { out[m[0]] = enforce(v, 255, "The '" + m[0] + "' member"); }
        }
        return out;
    };
    var need = function (n, min, opname) {
        if (n < min) { throw new TypeError("Failed to execute '" + opname + "' on 'SubtleCrypto': " + min + ' arguments required, but only ' + n + ' present.'); }
    };
    var keyArg = function (k, what) {
        if (!isKey(k)) { throw new TypeError("Failed to execute '" + what + "' on 'SubtleCrypto': parameter is not of type 'CryptoKey'."); }
        return KS.get(k);
    };
    var random = function (n) { return crypto.getRandomValues(new Uint8Array(n)); };
    var maskBits = function (u8, bits) {
        var extra = u8.length * 8 - bits;
        if (extra > 0) { u8[u8.length - 1] &= (0xFF << extra) & 0xFF; }
        return u8;
    };

    // ---- the algorithms ------------------------------------------------------------------------------
    var AES = ['AES-GCM', 'AES-CTR'];
    var generate = function (n, extractable, usages) {
        if (n.name === 'HMAC') {
            only(usages, ['sign', 'verify']);
            var len = n.length === undefined ? BLOCK[n.hash] : n.length;
            if (len === 0) { throw err('OperationError', 'HMAC key length must not be zero.'); }
            return mkKey({ name: 'HMAC', hash: n.hash, length: len }, extractable, canonical(usages), maskBits(random(Math.ceil(len / 8)), len));
        }
        only(usages, ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey']);
        if ([128, 192, 256].indexOf(n.length) < 0) { throw err('OperationError', 'AES key length must be 128, 192 or 256 bits.'); }
        return mkKey({ name: n.name, length: n.length }, extractable, canonical(usages), random(n.length / 8));
    };
    var importRaw = function (n, data, extractable, usages) {
        if (n.name === 'HMAC') {
            only(usages, ['sign', 'verify']);
            var len = data.length * 8;
            if (len === 0) { throw err('DataError', 'HMAC key data must not be empty.'); }
            if (n.length !== undefined) {
                if (n.length > len || n.length <= len - 8) { throw err('DataError', 'The HMAC length does not match the key data.'); }
                len = n.length;
            }
            return mkKey({ name: 'HMAC', hash: n.hash, length: len }, extractable, canonical(usages), maskBits(data, len));
        }
        if (AES.indexOf(n.name) >= 0) {
            only(usages, ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey']);
            if ([16, 24, 32].indexOf(data.length) < 0) { throw err('DataError', 'AES key data must be 128, 192 or 256 bits.'); }
            return mkKey({ name: n.name, length: data.length * 8 }, extractable, canonical(usages), data);
        }
        // HKDF, PBKDF2 (§31.4.3, §30.4.3): raw only, never extractable.
        only(usages, ['deriveKey', 'deriveBits']);
        if (extractable) { throw err('SyntaxError', n.name + ' keys are not extractable.'); }
        return mkKey({ name: n.name }, false, canonical(usages), data);
    };
    var deriveBitsOf = function (n, key, length) {
        if (length === null || length % 8 !== 0) { throw err('OperationError', 'The length must be a multiple of 8 bits.'); }
        if (n.name === 'PBKDF2' && n.iterations === 0) { throw err('OperationError', 'PBKDF2 iterations must not be zero.'); }
        if (length === 0) { return new ArrayBuffer(0); }
        var r = n.name === 'HKDF'
            ? __subtle_hkdf(n.hash, key.data, n.salt, n.info, length / 8)
            : __subtle_pbkdf2(n.hash, key.data, n.salt, n.iterations, length / 8);
        if (r === null) { throw err('OperationError', 'The requested length is too long.'); }
        return r;
    };
    var keyLength = function (n) {
        if (AES.indexOf(n.name) >= 0) {
            if ([128, 192, 256].indexOf(n.length) < 0) { throw err('OperationError', 'AES key length must be 128, 192 or 256 bits.'); }
            return n.length;
        }
        if (n.name === 'HMAC') {
            if (n.length === undefined) { return BLOCK[n.hash]; }
            if (n.length === 0) { throw new TypeError('HMAC key length must not be zero.'); }
            return n.length;
        }
        return null;
    };
    var cipher = function (enc, n, key, data) {
        var r;
        if (n.name === 'AES-GCM') {
            var tl = n.tagLength === undefined ? 128 : n.tagLength;
            if ([32, 64, 96, 104, 112, 120, 128].indexOf(tl) < 0) { throw err('OperationError', 'The AES-GCM tagLength is not supported.'); }
            if (!enc && data.length < tl / 8) { throw err('OperationError', 'The ciphertext is shorter than the tag.'); }
            r = __subtle_aes_gcm(enc, key.data, n.iv, n.additionalData === undefined ? new Uint8Array(0) : n.additionalData, tl / 8, data);
        } else {
            if (n.counter.length !== 16) { throw err('OperationError', 'The AES-CTR counter must be 16 bytes.'); }
            if (n.length === 0 || n.length > 128) { throw err('OperationError', 'The AES-CTR length must be between 1 and 128.'); }
            r = __subtle_aes_ctr(key.data, n.counter, n.length, data);
        }
        if (r === null) { throw err('OperationError', 'The operation failed for an operation-specific reason'); }
        return r;
    };
    // Shared general steps: the algorithm name must match the key's, and the key must allow the usage.
    var check = function (n, key, usage) {
        if (n.name !== key.alg.name) { throw err('InvalidAccessError', 'The requested operation is not valid for the provided key.'); }
        if (key.usages.indexOf(usage) < 0) { throw err('InvalidAccessError', 'The requested operation is not valid for the provided key.'); }
    };
    var finish = function (k) {
        if ((ks(k).type === 'secret' || ks(k).type === 'private') && ks(k).usages.length === 0) {
            throw err('SyntaxError', 'Usages cannot be empty when creating a key.');
        }
        return k;
    };
    // Every method returns a promise; argument conversion and every step's exception reject it (WebIDL §3.6.8).
    var run = function (f) {
        return new Promise(function (resolve) { resolve(f()); });
    };

    class SubtleCrypto {
        constructor() { throw new TypeError('Illegal constructor'); }
    }
    var SM = {
        digest(algorithm, data) {
            var args = arguments;
            return run(function () {
                need(args.length, 2, 'digest');
                var bytes = bytesOf(data, "Failed to execute 'digest' on 'SubtleCrypto': parameter 2");
                var n = normalize(algorithm, 'digest');
                return __subtle_digest(n.name, bytes);
            });
        },
        generateKey(algorithm, extractable, keyUsages) {
            var args = arguments;
            return run(function () {
                need(args.length, 3, 'generateKey');
                var ex = !!extractable, us = usagesOf(keyUsages);
                var n = normalize(algorithm, 'generateKey');
                return finish(generate(n, ex, us));
            });
        },
        importKey(format, keyData, algorithm, extractable, keyUsages) {
            var args = arguments;
            return run(function () {
                need(args.length, 5, 'importKey');
                var f = str(format);
                if (['raw', 'spki', 'pkcs8', 'jwk'].indexOf(f) < 0) { throw new TypeError("The provided value '" + f + "' is not a valid enum value of type KeyFormat."); }
                var data = f === 'jwk' ? null : bytesOf(keyData, "Failed to execute 'importKey' on 'SubtleCrypto': parameter 2");
                if (f === 'jwk' && (keyData === null || typeof keyData !== 'object')) { throw new TypeError("Failed to execute 'importKey' on 'SubtleCrypto': parameter 2 is not a JsonWebKey."); }
                var ex = !!extractable, us = usagesOf(keyUsages);
                var n = normalize(algorithm, 'importKey');
                if (f !== 'raw') { throw err('NotSupportedError', "The '" + f + "' key format is not supported."); }
                return finish(importRaw(n, data, ex, us));
            });
        },
        exportKey(format, key) {
            var args = arguments;
            return run(function () {
                need(args.length, 2, 'exportKey');
                var f = str(format);
                if (['raw', 'spki', 'pkcs8', 'jwk'].indexOf(f) < 0) { throw new TypeError("The provided value '" + f + "' is not a valid enum value of type KeyFormat."); }
                var k = keyArg(key, 'exportKey');
                if (!REG.exportKey[k.alg.name]) { throw err('NotSupportedError', 'The key algorithm does not support export.'); }
                if (!k.extractable) { throw err('InvalidAccessError', 'The key is not extractable.'); }
                if (f !== 'raw') { throw err('NotSupportedError', "The '" + f + "' key format is not supported."); }
                return abuf(k.data);
            });
        },
        sign(algorithm, key, data) {
            var args = arguments;
            return run(function () {
                need(args.length, 3, 'sign');
                var k = keyArg(key, 'sign');
                var bytes = bytesOf(data, "Failed to execute 'sign' on 'SubtleCrypto': parameter 3");
                var n = normalize(algorithm, 'sign');
                check(n, k, 'sign');
                return __subtle_hmac(k.alg.hash, k.data, bytes);
            });
        },
        verify(algorithm, key, signature, data) {
            var args = arguments;
            return run(function () {
                need(args.length, 4, 'verify');
                var k = keyArg(key, 'verify');
                var sig = bytesOf(signature, "Failed to execute 'verify' on 'SubtleCrypto': parameter 3");
                var bytes = bytesOf(data, "Failed to execute 'verify' on 'SubtleCrypto': parameter 4");
                var n = normalize(algorithm, 'verify');
                check(n, k, 'verify');
                return __subtle_hmac_verify(k.alg.hash, k.data, bytes, sig);
            });
        },
        encrypt(algorithm, key, data) {
            var args = arguments;
            return run(function () {
                need(args.length, 3, 'encrypt');
                var k = keyArg(key, 'encrypt');
                var bytes = bytesOf(data, "Failed to execute 'encrypt' on 'SubtleCrypto': parameter 3");
                var n = normalize(algorithm, 'encrypt');
                check(n, k, 'encrypt');
                return cipher(true, n, k, bytes);
            });
        },
        decrypt(algorithm, key, data) {
            var args = arguments;
            return run(function () {
                need(args.length, 3, 'decrypt');
                var k = keyArg(key, 'decrypt');
                var bytes = bytesOf(data, "Failed to execute 'decrypt' on 'SubtleCrypto': parameter 3");
                var n = normalize(algorithm, 'decrypt');
                check(n, k, 'decrypt');
                return cipher(false, n, k, bytes);
            });
        },
        deriveBits(algorithm, baseKey) {
            var args = arguments;
            return run(function () {
                need(args.length, 2, 'deriveBits');
                var k = keyArg(baseKey, 'deriveBits');
                var lv = args[2], length = lv === undefined || lv === null ? null : (+lv >>> 0);
                var n = normalize(algorithm, 'deriveBits');
                check(n, k, 'deriveBits');
                return deriveBitsOf(n, k, length);
            });
        },
        deriveKey(algorithm, baseKey, derivedKeyType, extractable, keyUsages) {
            var args = arguments;
            return run(function () {
                need(args.length, 5, 'deriveKey');
                var k = keyArg(baseKey, 'deriveKey');
                var ex = !!extractable, us = usagesOf(keyUsages);
                var n = normalize(algorithm, 'deriveBits');
                var imp = normalize(derivedKeyType, 'importKey');
                var lenAlg = normalize(derivedKeyType, 'get key length');
                check(n, k, 'deriveKey');
                var bits = deriveBitsOf(n, k, keyLength(lenAlg));
                return finish(importRaw(imp, new Uint8Array(bits), ex, us));
            });
        },
        wrapKey(format, key, wrappingKey, wrapAlgorithm) {
            var args = arguments;
            return run(function () {
                need(args.length, 4, 'wrapKey');
                throw err('NotSupportedError', 'wrapKey is not supported.');
            });
        },
        unwrapKey(format, wrappedKey, unwrappingKey, unwrapAlgorithm, unwrappedKeyAlgorithm, extractable, keyUsages) {
            var args = arguments;
            return run(function () {
                need(args.length, 7, 'unwrapKey');
                throw err('NotSupportedError', 'unwrapKey is not supported.');
            });
        },
    };
    Object.keys(SM).forEach(function (k) { op(SubtleCrypto.prototype, k, SM[k]); });
    Object.defineProperty(SubtleCrypto.prototype, Symbol.toStringTag, { value: 'SubtleCrypto', configurable: true });
    var subtle = Object.create(SubtleCrypto.prototype);
    Object.defineProperty(crypto, 'subtle', { get: function () { return subtle; }, enumerable: true, configurable: true });
    [CryptoKey, SubtleCrypto].forEach(function (C) {
        Object.defineProperty(globalThis, C.name, { value: C, writable: true, enumerable: false, configurable: true });
    });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Known answers: FIPS 180-4 "abc", RFC 4231 case 2, RFC 5869 case 1, RFC 6070-style PBKDF2-SHA256
    /// (password/salt, 1 iteration), SP 800-38A F.5.1 (CTR-AES128 first block), and a GCM round trip that
    /// fails closed on a flipped tag bit.
    #[test]
    fn subtle_primitives_known_answers() {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(&digest("SHA-256", b"abc").unwrap()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&hmac("SHA-256", b"Jefe", b"what do ya want for nothing?").unwrap()), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let ikm = [0x0bu8; 22];
        let salt: Vec<u8> = (0u8..=0x0c).collect();
        let info: Vec<u8> = (0xf0u8..=0xf9).collect();
        assert_eq!(
            hex(&hkdf("SHA-256", &ikm, &salt, &info, 42).unwrap()),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
        assert_eq!(hex(&pbkdf2("SHA-256", b"password", b"salt", 1, 32).unwrap()), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        let key = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let ctr = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff];
        let pt = [0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a];
        assert_eq!(hex(&aes_ctr(&key, &ctr, 64, &pt).unwrap()), "874d6191b620e3261bef6864990db6ce");
        assert!(aes_ctr(&key, &ctr, 1, &[0u8; 33]).is_none(), "three blocks under a 1-bit counter repeat it");
        let sealed = aes_gcm(true, &key, &[7u8; 12], b"aad", 16, b"hello").unwrap();
        assert_eq!(aes_gcm(false, &key, &[7u8; 12], b"aad", 16, &sealed).unwrap(), b"hello");
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(aes_gcm(false, &key, &[7u8; 12], b"aad", 16, &bad).is_none());
    }
}
