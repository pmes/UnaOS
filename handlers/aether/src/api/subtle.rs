//! AETHERWPT2 (LEDGER SR135): `crypto.subtle` (W3C Web Cryptography API) over UnaOS's own crypto_core — the
//! symmetric half: `digest` (SHA-1 / SHA-256 / SHA-384 / SHA-512), HMAC (`generateKey`, `importKey` /
//! `exportKey` "raw", `sign`, `verify`), AES-GCM and AES-CTR (`generateKey`, `importKey` / `exportKey` "raw",
//! `encrypt`, `decrypt`), HKDF and PBKDF2 (`importKey` "raw", `deriveBits`, `deriveKey`), and `CryptoKey`.
//!
//! AETHERWPT3 (LEDGER SR138) adds the rest of the registered algorithms crypto_core can carry: RSASSA-PKCS1-v1_5,
//! RSA-PSS and RSA-OAEP (crypto_core::rsa_private), ECDSA and ECDH on P-256 / P-384 (crypto_core::p256 / p384),
//! Ed25519 and X25519, AES-CBC and AES-KW (crypto_core::aes's inverse cipher); the `spki` / `pkcs8` / `jwk`
//! formats (api/der.rs for the ASN.1); `wrapKey` / `unwrapKey`. Refused by name: P-521 (no curve in crypto_core),
//! Ed448 / X448, multi-prime RSA JWKs, and the tentative Modern Algorithms (ML-KEM / ML-DSA / ChaCha20-Poly1305
//! / AES-OCB / KMAC / Argon2) — NotSupportedError, as for any unregistered name.
//!
//! The split follows `api::platform`: the arithmetic is natives here (each answers an ArrayBuffer, or null for
//! the specification's OperationError / DataError); the WebIDL shape — normalizing an algorithm (§18.4.4), the
//! usage and length checks, the general steps of each method (§14.3), promise rejection for every error — is the
//! JS prelude [`SUBTLE`].

use crate::api::der;
use crate::js::idl::arg;
use crypto_core::aes::Aes;
use crypto_core::cbc;
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

// ---- AETHERWPT3 (SR138): the asymmetric algorithms, AES-CBC, AES-KW and the key formats ---------------------

/// AES-CBC (SP 800-38A §6.2) with the PKCS #7 padding WebCrypto §26.6 prescribes; None for an IV that is not 16
/// bytes, a ciphertext that is not whole blocks, or padding that does not verify.
pub fn aes_cbc(encrypt: bool, key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    // CRYPTOONE (SR330): crypto_core's one CBC and its PKCS #7 pad / unpad (an empty ciphertext unpads to None).
    let a = Aes::new(key).ok()?;
    let iv: [u8; 16] = iv.try_into().ok()?;
    let mut buf = data.to_vec();
    if encrypt {
        buf.resize(data.len() + 16 - data.len() % 16, 0);
        cbc::pkcs7_pad(&mut buf, data.len())?;
        cbc::encrypt(&a, &iv, &mut buf).ok()?;
    } else {
        cbc::decrypt(&a, &iv, &mut buf).ok()?;
        let n = cbc::pkcs7_unpad(&buf)?;
        buf.truncate(n);
    }
    Some(buf)
}

/// AES Key Wrap (RFC 3394 §2.2.1 / §2.2.2, the default IV A6A6A6A6A6A6A6A6); None for data that is not a
/// multiple of 8 bytes (at least 16 to wrap, 24 to unwrap) or an unwrap whose integrity check fails.
pub fn aes_kw(wrap: bool, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let a = Aes::new(key).ok()?;
    const IV: [u8; 8] = [0xA6; 8];
    if data.len() % 8 != 0 {
        return None;
    }
    if wrap {
        if data.len() < 16 {
            return None;
        }
        let n = data.len() / 8;
        let mut aa = IV;
        let mut r: Vec<[u8; 8]> = data.chunks(8).map(|c| c.try_into().unwrap()).collect();
        for j in 0..6 {
            for i in 0..n {
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&aa);
                b[8..].copy_from_slice(&r[i]);
                a.encrypt_block(&mut b);
                let t = (n * j + i + 1) as u64;
                aa.copy_from_slice(&b[..8]);
                for (x, y) in aa.iter_mut().zip(t.to_be_bytes()) {
                    *x ^= y;
                }
                r[i].copy_from_slice(&b[8..]);
            }
        }
        let mut out = aa.to_vec();
        for x in r {
            out.extend_from_slice(&x);
        }
        Some(out)
    } else {
        if data.len() < 24 {
            return None;
        }
        let n = data.len() / 8 - 1;
        let mut aa: [u8; 8] = data[..8].try_into().ok()?;
        let mut r: Vec<[u8; 8]> = data[8..].chunks(8).map(|c| c.try_into().unwrap()).collect();
        for j in (0..6).rev() {
            for i in (0..n).rev() {
                let t = (n * j + i + 1) as u64;
                let mut b = [0u8; 16];
                b[..8].copy_from_slice(&aa);
                for (x, y) in b[..8].iter_mut().zip(t.to_be_bytes()) {
                    *x ^= y;
                }
                b[8..].copy_from_slice(&r[i]);
                a.decrypt_block(&mut b);
                aa.copy_from_slice(&b[..8]);
                r[i].copy_from_slice(&b[8..]);
            }
        }
        if !crypto_core::ct::ct_eq(&aa, &IV) {
            return None;
        }
        Some(r.concat())
    }
}

fn random(n: usize) -> Vec<u8> {
    crate::api::platform::entropy(n)
}

/// The named curves crypto_core implements (WebCrypto §23.3 / §24.3: P-256, P-384; P-521 is refused by name).
fn curve_len(curve: &str) -> Option<usize> {
    match curve {
        "P-256" => Some(32),
        "P-384" => Some(48),
        _ => None,
    }
}

/// A private scalar's public point (SEC 1 uncompressed); None for a scalar outside [1, n-1].
pub fn ec_public(curve: &str, d: &[u8]) -> Option<Vec<u8>> {
    match curve {
        "P-256" => Some(crypto_core::p256::SecretKey::from_bytes(d.try_into().ok()?).ok()?.public_key().to_sec1_uncompressed().to_vec()),
        "P-384" => Some(crypto_core::p384::SecretKey::from_bytes(d.try_into().ok()?).ok()?.public_key().to_sec1_uncompressed().to_vec()),
        _ => None,
    }
}

/// A SEC 1 point (uncompressed or compressed), validated, re-encoded uncompressed.
pub fn ec_point(curve: &str, b: &[u8]) -> Option<Vec<u8>> {
    match curve {
        "P-256" => Some(crypto_core::p256::PublicKey::from_sec1(b).ok()?.to_sec1_uncompressed().to_vec()),
        "P-384" => Some(crypto_core::p384::PublicKey::from_sec1(b).ok()?.to_sec1_uncompressed().to_vec()),
        _ => None,
    }
}

/// ECDSA (WebCrypto §23.4.1): the message hashed with `hash`, signed with an RFC 6979 nonce, raw `r || s`.
pub fn ec_sign(curve: &str, d: &[u8], hash: &str, data: &[u8]) -> Option<Vec<u8>> {
    let h = digest(hash, data)?;
    match curve {
        "P-256" => Some(crypto_core::p256::SecretKey::from_bytes(d.try_into().ok()?).ok()?.sign_prehashed(&h).to_vec()),
        "P-384" => Some(crypto_core::p384::SecretKey::from_bytes(d.try_into().ok()?).ok()?.sign_prehashed(&h).to_vec()),
        _ => None,
    }
}

/// ECDSA verify (WebCrypto §23.4.2): a signature that is not `2 · curve length` bytes is false.
pub fn ec_verify(curve: &str, public: &[u8], hash: &str, data: &[u8], sig: &[u8]) -> bool {
    let Some(h) = digest(hash, data) else { return false };
    match curve {
        "P-256" => {
            let (Ok(k), Ok(s)) = (crypto_core::p256::PublicKey::from_sec1(public), <&[u8; 64]>::try_from(sig)) else { return false };
            crypto_core::p256::verify_prehashed(&k, &h, s).is_ok()
        }
        "P-384" => {
            let (Ok(k), Ok(s)) = (crypto_core::p384::PublicKey::from_sec1(public), <&[u8; 96]>::try_from(sig)) else { return false };
            crypto_core::p384::verify_prehashed(&k, &h, s).is_ok()
        }
        _ => false,
    }
}

/// ECDH (WebCrypto §24.4.3): the shared point's x-coordinate.
pub fn ec_dh(curve: &str, d: &[u8], public: &[u8]) -> Option<Vec<u8>> {
    match curve {
        "P-256" => {
            let k = crypto_core::p256::SecretKey::from_bytes(d.try_into().ok()?).ok()?;
            Some(k.diffie_hellman(&crypto_core::p256::PublicKey::from_sec1(public).ok()?).ok()?.to_vec())
        }
        "P-384" => {
            let k = crypto_core::p384::SecretKey::from_bytes(d.try_into().ok()?).ok()?;
            Some(k.diffie_hellman(&crypto_core::p384::PublicKey::from_sec1(public).ok()?).ok()?.to_vec())
        }
        _ => None,
    }
}

/// A fresh private scalar in [1, n-1] (rejection sampling) and its public point.
fn ec_generate(curve: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let len = curve_len(curve)?;
    for _ in 0..64 {
        let d = random(len);
        if let Some(p) = ec_public(curve, &d) {
            return Some((d, p));
        }
    }
    None
}

/// Ed25519 / X25519 public keys (RFC 8032 §5.1.5, RFC 7748 §6.1).
pub fn okp_public(name: &str, d: &[u8]) -> Option<Vec<u8>> {
    let d: &[u8; 32] = d.try_into().ok()?;
    match name {
        "Ed25519" => Some(crypto_core::ed25519::SigningKey::from_seed(d).public_key().to_vec()),
        "X25519" => Some(crypto_core::x25519::public_key(d).to_vec()),
        _ => None,
    }
}

fn rsa_private(c: &[Vec<u8>]) -> Option<crypto_core::rsa_private::PrivateKey> {
    if c.len() != 8 {
        return None;
    }
    crypto_core::rsa_private::PrivateKey::from_components(&c[0], &c[1], &c[2], &c[3], &c[4], &c[5], &c[6], &c[7]).ok()
}

/// RSASSA-PKCS1-v1_5 (`pss` false) or RSA-PSS signature of `data` hashed with `hash` (WebCrypto §20.4.1 /
/// §21.4.1).
pub fn rsa_sign(pss: bool, hash: &str, comps: &[Vec<u8>], data: &[u8], salt_len: usize) -> Option<Vec<u8>> {
    let key = rsa_private(comps)?;
    let h = digest(hash, data)?;
    if salt_len > key.public_key().size() {
        return None;
    }
    let mut out = vec![0u8; key.public_key().size()];
    let r = if pss {
        let salt = random(salt_len);
        match hash {
            "SHA-1" => key.sign_pss::<Sha1>(&h, &salt, &mut out),
            "SHA-256" => key.sign_pss::<Sha256>(&h, &salt, &mut out),
            "SHA-384" => key.sign_pss::<Sha384>(&h, &salt, &mut out),
            "SHA-512" => key.sign_pss::<Sha512>(&h, &salt, &mut out),
            _ => return None,
        }
    } else {
        key.sign_pkcs1v15(&h, &mut out)
    };
    r.ok().map(|_| out)
}

pub fn rsa_verify(pss: bool, hash: &str, n: &[u8], e: &[u8], data: &[u8], sig: &[u8], salt_len: usize) -> bool {
    let (Ok(k), Some(h)) = (crypto_core::rsa_private::PublicKey::new(n, e), digest(hash, data)) else { return false };
    if !pss {
        return k.verify_pkcs1v15(&h, sig).is_ok();
    }
    match hash {
        "SHA-1" => k.verify_pss::<Sha1>(&h, salt_len, sig).is_ok(),
        "SHA-256" => k.verify_pss::<Sha256>(&h, salt_len, sig).is_ok(),
        "SHA-384" => k.verify_pss::<Sha384>(&h, salt_len, sig).is_ok(),
        "SHA-512" => k.verify_pss::<Sha512>(&h, salt_len, sig).is_ok(),
        _ => false,
    }
}

/// RSA-OAEP encrypt (WebCrypto §22.4.1) with MGF1 over the key's hash.
pub fn rsa_encrypt(hash: &str, n: &[u8], e: &[u8], label: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let k = crypto_core::rsa_private::PublicKey::new(n, e).ok()?;
    let mut out = vec![0u8; k.size()];
    let r = match hash {
        "SHA-1" => k.encrypt_oaep::<Sha1>(label, data, &random(20), &mut out),
        "SHA-256" => k.encrypt_oaep::<Sha256>(label, data, &random(32), &mut out),
        "SHA-384" => k.encrypt_oaep::<Sha384>(label, data, &random(48), &mut out),
        "SHA-512" => k.encrypt_oaep::<Sha512>(label, data, &random(64), &mut out),
        _ => return None,
    };
    r.ok().map(|_| out)
}

/// RSA-OAEP decrypt (WebCrypto §22.4.2).
pub fn rsa_decrypt(hash: &str, comps: &[Vec<u8>], label: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let key = rsa_private(comps)?;
    let mut out = vec![0u8; key.public_key().size()];
    let n = match hash {
        "SHA-1" => key.decrypt_oaep::<Sha1>(label, data, &mut out),
        "SHA-256" => key.decrypt_oaep::<Sha256>(label, data, &mut out),
        "SHA-384" => key.decrypt_oaep::<Sha384>(label, data, &mut out),
        "SHA-512" => key.decrypt_oaep::<Sha512>(label, data, &mut out),
        _ => return None,
    }
    .ok()?;
    out.truncate(n);
    Some(out)
}

/// RSA key generation (WebCrypto §20.8 "generate key"): None for an unsupported size or exponent.
pub fn rsa_generate(bits: usize, e: &[u8]) -> Option<Vec<Vec<u8>>> {
    use crypto_core::rsa_private::{Part, PrivateKey};
    let mut rng = |b: &mut [u8]| b.copy_from_slice(&random(b.len()));
    let key = PrivateKey::generate(bits, e, &mut rng).ok()?;
    let mut out = Vec::with_capacity(8);
    let mut buf = vec![0u8; crypto_core::rsa_private::MAX_BYTES];
    for p in [Part::N, Part::E, Part::D, Part::P, Part::Q, Part::Dp, Part::Dq, Part::Qi] {
        let n = key.part_be(p, &mut buf);
        out.push(buf[..n].to_vec());
    }
    Some(out)
}

fn bufs_arg(vm: &mut Vm, ctx: &CallCtx, from: usize, n: usize) -> Vec<Vec<u8>> {
    (from..from + n).map(|i| bytes_arg(vm, ctx, i)).collect()
}

fn list(vm: &mut Vm, items: Vec<Vec<u8>>) -> JsResult<Value> {
    let mut vals = Vec::with_capacity(items.len());
    for b in items {
        vals.push(array_buffer(vm, b)?);
    }
    Ok(Value::Object(vm.new_array(vals)))
}

fn n_aes_cbc(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let enc = arg(vm, ctx, 0);
    let encrypt = vm.to_boolean(&enc);
    let (key, iv, data) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
    let r = aes_cbc(encrypt, &key, &iv, &data);
    answer(vm, r)
}

fn n_aes_kw(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let w = arg(vm, ctx, 0);
    let wrap = vm.to_boolean(&w);
    let (key, data) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2));
    let r = aes_kw(wrap, &key, &data);
    answer(vm, r)
}

/// `__subtle_ec(op, curve, a, b, c, d)`: `generate` → [d, point]; `public` (d) → point; `point` (bytes) →
/// uncompressed point; `sign` (d, hash, data) → r||s; `verify` (point, hash, data, sig) → bool; `dh` (d,
/// point) → x. null for every failure.
fn n_ec(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    let curve = str_arg(vm, ctx, 1)?;
    match op.as_str() {
        "generate" => match ec_generate(&curve) {
            Some((d, p)) => list(vm, vec![d, p]),
            None => Ok(Value::Null),
        },
        "public" => {
            let d = bytes_arg(vm, ctx, 2);
            let r = ec_public(&curve, &d);
            answer(vm, r)
        }
        "point" => {
            let b = bytes_arg(vm, ctx, 2);
            let r = ec_point(&curve, &b);
            answer(vm, r)
        }
        "sign" => {
            let d = bytes_arg(vm, ctx, 2);
            let hash = str_arg(vm, ctx, 3)?;
            let data = bytes_arg(vm, ctx, 4);
            let r = ec_sign(&curve, &d, &hash, &data);
            answer(vm, r)
        }
        "verify" => {
            let p = bytes_arg(vm, ctx, 2);
            let hash = str_arg(vm, ctx, 3)?;
            let (data, sig) = (bytes_arg(vm, ctx, 4), bytes_arg(vm, ctx, 5));
            Ok(Value::Bool(ec_verify(&curve, &p, &hash, &data, &sig)))
        }
        "dh" => {
            let (d, p) = (bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
            let r = ec_dh(&curve, &d, &p);
            answer(vm, r)
        }
        _ => Ok(Value::Null),
    }
}

/// `__subtle_okp(op, name, a, b, c)`: `generate` → [d, public]; `public` (d) → public; `sign` (d, data) →
/// Ed25519 signature; `verify` (public, data, sig) → bool; `dh` (d, public) → X25519 secret, null when it is
/// all zeros (RFC 7748 §6.1, WebCrypto §32.4.2).
fn n_okp(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    let name = str_arg(vm, ctx, 1)?;
    match op.as_str() {
        "generate" => {
            let d = random(32);
            match okp_public(&name, &d) {
                Some(p) => list(vm, vec![d, p]),
                None => Ok(Value::Null),
            }
        }
        "public" => {
            let d = bytes_arg(vm, ctx, 2);
            let r = okp_public(&name, &d);
            answer(vm, r)
        }
        "sign" => {
            let (d, data) = (bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
            let r = <&[u8; 32]>::try_from(&d[..]).ok().map(|s| crypto_core::ed25519::SigningKey::from_seed(s).sign(&data).to_vec());
            answer(vm, r)
        }
        "verify" => {
            let (p, data, sig) = (bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3), bytes_arg(vm, ctx, 4));
            let ok = match (<&[u8; 32]>::try_from(&p[..]), <&[u8; 64]>::try_from(&sig[..])) {
                (Ok(p), Ok(s)) => crypto_core::ed25519::verify(p, &data, s).is_ok(),
                _ => false,
            };
            Ok(Value::Bool(ok))
        }
        "dh" => {
            let (d, p) = (bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3));
            let r = match (<&[u8; 32]>::try_from(&d[..]), <&[u8; 32]>::try_from(&p[..])) {
                (Ok(d), Ok(p)) => crypto_core::x25519::diffie_hellman(d, p).ok().map(|x| x.to_vec()),
                _ => None,
            };
            answer(vm, r)
        }
        _ => Ok(Value::Null),
    }
}

/// `__subtle_rsa(op, …)`: `generate` (bits, e) → the eight components; `check` (8 components) → bool;
/// `checkPublic` (n, e) → bool; `sign` (pss, hash, 8 components, data, saltLength) → signature;
/// `verify` (pss, hash, n, e, data, sig, saltLength) → bool; `encrypt` (hash, n, e, label, data);
/// `decrypt` (hash, 8 components, label, data). null for every failure.
fn n_rsa(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    match op.as_str() {
        "generate" => {
            let bits = num_arg(vm, ctx, 1)? as usize;
            let e = bytes_arg(vm, ctx, 2);
            match rsa_generate(bits, &e) {
                Some(c) => list(vm, c),
                None => Ok(Value::Null),
            }
        }
        "check" => {
            let c = bufs_arg(vm, ctx, 1, 8);
            Ok(Value::Bool(rsa_private(&c).is_some()))
        }
        "checkPublic" => {
            let (n, e) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2));
            Ok(Value::Bool(crypto_core::rsa_private::PublicKey::new(&n, &e).is_ok()))
        }
        "sign" => {
            let p = arg(vm, ctx, 1);
            let pss = vm.to_boolean(&p);
            let hash = str_arg(vm, ctx, 2)?;
            let c = bufs_arg(vm, ctx, 3, 8);
            let data = bytes_arg(vm, ctx, 11);
            let salt = num_arg(vm, ctx, 12)? as usize;
            let r = rsa_sign(pss, &hash, &c, &data, salt);
            answer(vm, r)
        }
        "verify" => {
            let p = arg(vm, ctx, 1);
            let pss = vm.to_boolean(&p);
            let hash = str_arg(vm, ctx, 2)?;
            let (n, e, data, sig) = (bytes_arg(vm, ctx, 3), bytes_arg(vm, ctx, 4), bytes_arg(vm, ctx, 5), bytes_arg(vm, ctx, 6));
            let salt = num_arg(vm, ctx, 7)? as usize;
            Ok(Value::Bool(rsa_verify(pss, &hash, &n, &e, &data, &sig, salt)))
        }
        "encrypt" => {
            let hash = str_arg(vm, ctx, 1)?;
            let (n, e, label, data) = (bytes_arg(vm, ctx, 2), bytes_arg(vm, ctx, 3), bytes_arg(vm, ctx, 4), bytes_arg(vm, ctx, 5));
            let r = rsa_encrypt(&hash, &n, &e, &label, &data);
            answer(vm, r)
        }
        "decrypt" => {
            let hash = str_arg(vm, ctx, 1)?;
            let c = bufs_arg(vm, ctx, 2, 8);
            let (label, data) = (bytes_arg(vm, ctx, 10), bytes_arg(vm, ctx, 11));
            let r = rsa_decrypt(&hash, &c, &label, &data);
            answer(vm, r)
        }
        _ => Ok(Value::Null),
    }
}

fn params_out(p: &der::Params) -> (Value, Value) {
    match p {
        der::Params::Absent => (crate::js::idl::s("absent"), Value::Null),
        der::Params::Null => (crate::js::idl::s("null"), Value::Null),
        der::Params::Oid(o) => (crate::js::idl::s("oid"), crate::js::idl::s(o)),
        der::Params::Other => (crate::js::idl::s("other"), Value::Null),
    }
}

fn params_in(kind: &str, oid: &str) -> der::Params {
    match kind {
        "null" => der::Params::Null,
        "oid" => der::Params::Oid(oid.to_string()),
        _ => der::Params::Absent,
    }
}

/// `__subtle_der(op, …)` — the key containers (api/der.rs): `spki` / `pkcs8` (bytes) → [algorithm OID,
/// parameters kind, parameters OID, key octets]; `buildSpki` / `buildPkcs8` (OID, kind, parameter OID, key);
/// `rsaPublic` (bytes) → [n, e]; `buildRsaPublic` (n, e); `rsaPrivate` (bytes) → 8 components;
/// `buildRsaPrivate` (8 components); `ecPrivate` (bytes) → [d, curve OID, public]; `buildEcPrivate` (d,
/// public); `octets` / `buildOctets`. null for anything malformed.
fn n_der(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    match op.as_str() {
        "spki" | "pkcs8" => {
            let b = bytes_arg(vm, ctx, 1);
            let parsed = if op == "spki" { der::spki_parse(&b) } else { der::pkcs8_parse(&b) };
            let Some((oid, params, key)) = parsed else { return Ok(Value::Null) };
            let (k, po) = params_out(&params);
            let key = array_buffer(vm, key)?;
            Ok(Value::Object(vm.new_array(vec![crate::js::idl::s(&oid), k, po, key])))
        }
        "buildSpki" | "buildPkcs8" => {
            let oid = str_arg(vm, ctx, 1)?;
            let kind = str_arg(vm, ctx, 2)?;
            let poid = str_arg(vm, ctx, 3)?;
            let key = bytes_arg(vm, ctx, 4);
            let p = params_in(&kind, &poid);
            let out = if op == "buildSpki" { der::spki_build(&oid, &p, &key) } else { der::pkcs8_build(&oid, &p, &key) };
            array_buffer(vm, out)
        }
        "rsaPublic" => {
            let b = bytes_arg(vm, ctx, 1);
            match der::rsa_public_parse(&b) {
                Some((n, e)) => list(vm, vec![n, e]),
                None => Ok(Value::Null),
            }
        }
        "buildRsaPublic" => {
            let (n, e) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2));
            array_buffer(vm, der::rsa_public_build(&n, &e))
        }
        "rsaPrivate" => {
            let b = bytes_arg(vm, ctx, 1);
            match der::rsa_private_parse(&b) {
                Some(c) => list(vm, c),
                None => Ok(Value::Null),
            }
        }
        "buildRsaPrivate" => {
            let c = bufs_arg(vm, ctx, 1, 8);
            array_buffer(vm, der::rsa_private_build(&c))
        }
        "ecPrivate" => {
            let b = bytes_arg(vm, ctx, 1);
            let Some((d, curve, public)) = der::ec_private_parse(&b) else { return Ok(Value::Null) };
            let d = array_buffer(vm, d)?;
            let c = match curve {
                Some(c) => crate::js::idl::s(&c),
                None => Value::Null,
            };
            let p = match public {
                Some(p) => array_buffer(vm, p)?,
                None => Value::Null,
            };
            Ok(Value::Object(vm.new_array(vec![d, c, p])))
        }
        "buildEcPrivate" => {
            let (d, p) = (bytes_arg(vm, ctx, 1), bytes_arg(vm, ctx, 2));
            array_buffer(vm, der::ec_private_build(&d, &p))
        }
        "octets" => {
            let b = bytes_arg(vm, ctx, 1);
            let r = der::octets_parse(&b);
            answer(vm, r)
        }
        "buildOctets" => {
            let b = bytes_arg(vm, ctx, 1);
            array_buffer(vm, der::octets(&b))
        }
        _ => Ok(Value::Null),
    }
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
        ("__subtle_aes_cbc", 4, n_aes_cbc),
        ("__subtle_aes_kw", 3, n_aes_kw),
        ("__subtle_ec", 6, n_ec),
        ("__subtle_okp", 5, n_okp),
        ("__subtle_rsa", 13, n_rsa),
        ("__subtle_der", 9, n_der),
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

/// The SubtleCrypto / CryptoKey shapes over the natives above (WebCrypto §13, §14, §18.4.4; AETHERWPT3 adds the
/// asymmetric families, AES-CBC / AES-KW, the spki / pkcs8 / jwk formats and wrapKey / unwrapKey).
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
    // KeyUsage (WebCrypto §13, with the encapsulation usages of the Modern Algorithms draft the tests enumerate).
    var USAGES = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits', 'wrapKey', 'unwrapKey',
                  'encapsulateKey', 'encapsulateBits', 'decapsulateKey', 'decapsulateBits'];
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
    var within = function (us, allowed) { return canonical(us.filter(function (u) { return allowed.indexOf(u) >= 0; })); };
    var only = function (us, allowed) {
        for (var i = 0; i < us.length; i++) {
            if (allowed.indexOf(us[i]) < 0) { throw err('SyntaxError', "Cannot create a key using the specified key usages."); }
        }
    };
    var abuf = function (u8) { return u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength); };
    var u8 = function (b) { return b instanceof Uint8Array ? b : new Uint8Array(b); };
    var FORMATS = ['raw', 'spki', 'pkcs8', 'jwk'];
    var formatOf = function (v) {
        var f = str(v);
        if (FORMATS.indexOf(f) < 0) { throw new TypeError("The provided value '" + f + "' is not a valid enum value of type KeyFormat."); }
        return f;
    };

    // ---- base64url (RFC 4648 §5, unpadded, as JWK carries it) ------------------------------------------------
    var B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
    var b64u = function (b) {
        b = u8(b);
        var out = '', i;
        for (i = 0; i + 2 < b.length; i += 3) {
            var v = (b[i] << 16) | (b[i + 1] << 8) | b[i + 2];
            out += B64[v >> 18] + B64[(v >> 12) & 63] + B64[(v >> 6) & 63] + B64[v & 63];
        }
        if (b.length - i === 1) { out += B64[b[i] >> 2] + B64[(b[i] & 3) << 4]; }
        else if (b.length - i === 2) { var w = (b[i] << 8) | b[i + 1]; out += B64[w >> 10] + B64[(w >> 4) & 63] + B64[(w & 15) << 2]; }
        return out;
    };
    var unb64u = function (s, what) {
        if (typeof s !== 'string') { throw err('DataError', "The JWK member '" + what + "' is missing."); }
        if (s.length % 4 === 1) { throw err('DataError', "The JWK member '" + what + "' is not valid base64url."); }
        var out = [], acc = 0, bits = 0;
        for (var i = 0; i < s.length; i++) {
            var v = B64.indexOf(s[i]);
            if (v < 0) { throw err('DataError', "The JWK member '" + what + "' is not valid base64url."); }
            acc = (acc << 6) | v; bits += 6;
            if (bits >= 8) { bits -= 8; out.push((acc >> bits) & 255); }
        }
        return new Uint8Array(out);
    };
    // JsonWebKey (WebCrypto §15) converted as the WebIDL dictionary.
    var JWK_STR = ['alg', 'crv', 'd', 'dp', 'dq', 'e', 'k', 'kty', 'n', 'p', 'q', 'qi', 'use', 'x', 'y'];
    var jwkOf = function (v, what) {
        if (v === null || (typeof v !== 'object' && typeof v !== 'function')) {
            throw new TypeError(what + " is not a JsonWebKey.");
        }
        var o = {};
        JWK_STR.forEach(function (m) { var x = v[m]; if (x !== undefined) { o[m] = str(x); } });
        if (v.ext !== undefined) { o.ext = !!v.ext; }
        if (v.key_ops !== undefined) {
            var ko = v.key_ops;
            if (ko === null || typeof ko !== 'object' || typeof ko[Symbol.iterator] !== 'function') { throw new TypeError("The 'key_ops' member cannot be converted to a sequence."); }
            o.key_ops = [];
            for (var x of ko) { o.key_ops.push(str(x)); }
        }
        if (v.oth !== undefined) { o.oth = v.oth; }
        return o;
    };
    // The checks every JWK import makes after its kty (WebCrypto §§20-32 "import key", jwk branch).
    var jwkChecks = function (jwk, ex, us, use) {
        if (us.length > 0 && jwk.use !== undefined && jwk.use !== use) { throw err('DataError', "The JWK 'use' member is not '" + use + "'."); }
        if (jwk.key_ops !== undefined) {
            var seen = {};
            for (var i = 0; i < jwk.key_ops.length; i++) {
                if (seen[jwk.key_ops[i]]) { throw err('DataError', "The JWK 'key_ops' member has a duplicate."); }
                seen[jwk.key_ops[i]] = true;
            }
            for (var j = 0; j < us.length; j++) {
                if (jwk.key_ops.indexOf(us[j]) < 0) { throw err('DataError', "The JWK 'key_ops' member does not allow the requested usages."); }
            }
        }
        if (jwk.ext === false && ex) { throw err('DataError', "The JWK is not extractable."); }
    };
    var textBytes = function (s) { return new TextEncoder().encode(s); };

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
    // KeyAlgorithm / AesKeyAlgorithm / HmacKeyAlgorithm / RsaHashedKeyAlgorithm / EcKeyAlgorithm (§§16-24).
    var algView = function (a) {
        var o = { name: a.name };
        if (a.modulusLength !== undefined) { o.modulusLength = a.modulusLength; o.publicExponent = new Uint8Array(a.publicExponent); }
        if (a.length !== undefined) { o.length = a.length; }
        if (a.hash !== undefined) { o.hash = { name: a.hash }; }
        if (a.namedCurve !== undefined) { o.namedCurve = a.namedCurve; }
        return o;
    };
    var mkKey = function (alg, extractable, usages, data, type) {
        var k = Object.create(CryptoKey.prototype);
        KS.set(k, { type: type || 'secret', extractable: extractable, alg: alg, algObj: algView(alg), usages: usages, usageObj: usages.slice(), data: data });
        return k;
    };

    // ---- normalize an algorithm (WebCrypto §18.4.4) -------------------------------------------------
    var HASHES = ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
    var BLOCK = { 'SHA-1': 512, 'SHA-256': 512, 'SHA-384': 1024, 'SHA-512': 1024 };
    // op → registered algorithm name → [member, type, required] in dictionary member order (inherited first,
    // then lexicographic: WebIDL §3.2.17).
    var hashM = ['hash', 'hash', true];
    var rsaGen = [['modulusLength', 'ulong', true], ['publicExponent', 'bigint', true], hashM];
    var curveM = [['namedCurve', 'string', true]];
    var aesLen = [['length', 'ushort', true]];
    var REG = {
        digest: { 'SHA-1': [], 'SHA-256': [], 'SHA-384': [], 'SHA-512': [] },
        generateKey: { 'HMAC': [hashM, ['length', 'ulong', false]], 'AES-GCM': aesLen, 'AES-CTR': aesLen, 'AES-CBC': aesLen, 'AES-KW': aesLen,
                       'RSASSA-PKCS1-v1_5': rsaGen, 'RSA-PSS': rsaGen, 'RSA-OAEP': rsaGen, 'ECDSA': curveM, 'ECDH': curveM, 'Ed25519': [], 'X25519': [] },
        importKey: { 'HMAC': [hashM, ['length', 'ulong', false]], 'AES-GCM': [], 'AES-CTR': [], 'AES-CBC': [], 'AES-KW': [], 'HKDF': [], 'PBKDF2': [],
                     'RSASSA-PKCS1-v1_5': [hashM], 'RSA-PSS': [hashM], 'RSA-OAEP': [hashM], 'ECDSA': curveM, 'ECDH': curveM, 'Ed25519': [], 'X25519': [] },
        exportKey: { 'HMAC': [], 'AES-GCM': [], 'AES-CTR': [], 'AES-CBC': [], 'AES-KW': [], 'RSASSA-PKCS1-v1_5': [], 'RSA-PSS': [], 'RSA-OAEP': [],
                     'ECDSA': [], 'ECDH': [], 'Ed25519': [], 'X25519': [] },
        sign: { 'HMAC': [], 'RSASSA-PKCS1-v1_5': [], 'RSA-PSS': [['saltLength', 'ulong', true]], 'ECDSA': [hashM], 'Ed25519': [] },
        encrypt: { 'AES-GCM': [['additionalData', 'bytes', false], ['iv', 'bytes', true], ['tagLength', 'octet', false]], 'AES-CTR': [['counter', 'bytes', true], ['length', 'octet', true]],
                   'AES-CBC': [['iv', 'bytes', true]], 'RSA-OAEP': [['label', 'bytes', false]] },
        deriveBits: { 'HKDF': [hashM, ['info', 'bytes', true], ['salt', 'bytes', true]], 'PBKDF2': [hashM, ['iterations', 'ulong', true], ['salt', 'bytes', true]],
                      'ECDH': [['public', 'key', true]], 'X25519': [['public', 'key', true]] },
        wrapKey: { 'AES-KW': [] },
        'get key length': { 'HMAC': [hashM, ['length', 'ulong', false]], 'AES-GCM': aesLen, 'AES-CTR': aesLen, 'AES-CBC': aesLen, 'AES-KW': aesLen, 'HKDF': [], 'PBKDF2': [] },
    };
    REG.verify = REG.sign;
    REG.decrypt = REG.encrypt;
    REG.unwrapKey = REG.wrapKey;
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
            else if (m[1] === 'bigint') {
                if (!(v instanceof Uint8Array)) { throw new TypeError("The '" + m[0] + "' member is not of type 'Uint8Array'."); }
                out[m[0]] = new Uint8Array(v);
            }
            else if (m[1] === 'string') { out[m[0]] = str(v); }
            else if (m[1] === 'key') {
                if (!isKey(v)) { throw new TypeError("The '" + m[0] + "' member is not of type 'CryptoKey'."); }
                out[m[0]] = v;
            }
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
    var maskBits = function (b, bits) {
        var extra = b.length * 8 - bits;
        if (extra > 0) { b[b.length - 1] &= (0xFF << extra) & 0xFF; }
        return b;
    };
    var opFail = function () { return err('OperationError', 'The operation failed for an operation-specific reason'); };
    var orFail = function (r) { if (r === null) { throw opFail(); } return r; };

    // ---- the algorithm families ----------------------------------------------------------------------
    var AES = ['AES-GCM', 'AES-CTR', 'AES-CBC', 'AES-KW'];
    var RSA = ['RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP'];
    var EC = ['ECDSA', 'ECDH'];
    var OKP = ['Ed25519', 'X25519'];
    var aesUsages = function (name) { return name === 'AES-KW' ? ['wrapKey', 'unwrapKey'] : ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey']; };
    // [public usages, private usages] per asymmetric algorithm.
    var PAIR = {
        'RSASSA-PKCS1-v1_5': [['verify'], ['sign']], 'RSA-PSS': [['verify'], ['sign']], 'RSA-OAEP': [['encrypt', 'wrapKey'], ['decrypt', 'unwrapKey']],
        'ECDSA': [['verify'], ['sign']], 'ECDH': [[], ['deriveKey', 'deriveBits']], 'Ed25519': [['verify'], ['sign']], 'X25519': [[], ['deriveKey', 'deriveBits']],
    };
    var OID = {
        rsa: '1.2.840.113549.1.1.1', ec: '1.2.840.10045.2.1', ecdh: '1.3.132.1.12', ed25519: '1.3.101.112', x25519: '1.3.101.110',
        'P-256': '1.2.840.10045.3.1.7', 'P-384': '1.3.132.0.34', 'P-521': '1.3.132.0.35',
    };
    var curveOfOid = function (o) { return o === OID['P-256'] ? 'P-256' : o === OID['P-384'] ? 'P-384' : o === OID['P-521'] ? 'P-521' : undefined; };
    var CURVES = ['P-256', 'P-384', 'P-521'];
    var CURVE_LEN = { 'P-256': 32, 'P-384': 48, 'P-521': 66 };
    // P-521 is a registered curve crypto_core does not implement: refused by name.
    var curveOk = function (c) {
        if (c === 'P-521') { throw err('NotSupportedError', 'The P-521 curve is not supported.'); }
    };
    var rsaAlgObj = function (n, mod, e) { return { name: n.name, modulusLength: mod, publicExponent: u8(e), hash: n.hash }; };
    var bitLength = function (b) {
        b = u8(b);
        var i = 0;
        while (i < b.length && b[i] === 0) { i++; }
        if (i === b.length) { return 0; }
        return (b.length - i - 1) * 8 + (32 - Math.clz32(b[i]));
    };
    var strip = function (b) { b = u8(b); var i = 0; while (i < b.length - 1 && b[i] === 0) { i++; } return b.slice(i); };
    var rsaParts = function (d) { return [d.n, d.e, d.d, d.p, d.q, d.dp, d.dq, d.qi]; };

    // generate key (§§20-32 "generate key")
    var generate = function (n, extractable, usages) {
        if (n.name === 'HMAC') {
            only(usages, ['sign', 'verify']);
            var len = n.length === undefined ? BLOCK[n.hash] : n.length;
            if (len === 0) { throw err('OperationError', 'HMAC key length must not be zero.'); }
            return mkKey({ name: 'HMAC', hash: n.hash, length: len }, extractable, canonical(usages), maskBits(random(Math.ceil(len / 8)), len));
        }
        if (AES.indexOf(n.name) >= 0) {
            only(usages, aesUsages(n.name));
            if ([128, 192, 256].indexOf(n.length) < 0) { throw err('OperationError', 'AES key length must be 128, 192 or 256 bits.'); }
            return mkKey({ name: n.name, length: n.length }, extractable, canonical(usages), random(n.length / 8));
        }
        var pair = PAIR[n.name];
        only(usages, pair[0].concat(pair[1]));
        var pubData, privData, alg;
        if (RSA.indexOf(n.name) >= 0) {
            var e = n.publicExponent;
            if (n.modulusLength < 256 || n.modulusLength > 8192 || n.modulusLength % 8 !== 0) { throw err('OperationError', 'The modulus length is not supported.'); }
            var c = orFail(__subtle_rsa('generate', n.modulusLength, e));
            c = c.map(u8);
            alg = rsaAlgObj(n, n.modulusLength, e);
            pubData = { n: c[0], e: c[1] };
            privData = { n: c[0], e: c[1], d: c[2], p: c[3], q: c[4], dp: c[5], dq: c[6], qi: c[7] };
        } else if (EC.indexOf(n.name) >= 0) {
            if (CURVES.indexOf(n.namedCurve) < 0) { throw err('NotSupportedError', 'The named curve is not supported.'); }
            curveOk(n.namedCurve);
            var g = orFail(__subtle_ec('generate', n.namedCurve));
            alg = { name: n.name, namedCurve: n.namedCurve };
            pubData = { pub: u8(g[1]) };
            privData = { d: u8(g[0]), pub: u8(g[1]) };
        } else {
            var o = orFail(__subtle_okp('generate', n.name));
            alg = { name: n.name };
            pubData = { pub: u8(o[1]) };
            privData = { d: u8(o[0]), pub: u8(o[1]) };
        }
        return {
            publicKey: mkKey(alg, true, within(usages, pair[0]), pubData, 'public'),
            privateKey: mkKey(alg, extractable, within(usages, pair[1]), privData, 'private'),
        };
    };

    // import key, secret families (HMAC §29.4.4, AES §§25-28, HKDF §31.4.3, PBKDF2 §30.4.3)
    var HMAC_ALG = { 'SHA-1': 'HS1', 'SHA-256': 'HS256', 'SHA-384': 'HS384', 'SHA-512': 'HS512' };
    var aesJwkAlg = function (name, bytes) { return 'A' + (bytes * 8) + (name === 'AES-KW' ? 'KW' : name.slice(4)); };
    var importSecret = function (f, n, data, ex, us) {
        if (n.name === 'HKDF' || n.name === 'PBKDF2') {
            if (f !== 'raw') { throw err('NotSupportedError', "The '" + f + "' key format is not supported for " + n.name + '.'); }
            only(us, ['deriveKey', 'deriveBits']);
            if (ex) { throw err('SyntaxError', n.name + ' keys are not extractable.'); }
            return mkKey({ name: n.name }, false, canonical(us), data);
        }
        var hmac = n.name === 'HMAC';
        only(us, hmac ? ['sign', 'verify'] : aesUsages(n.name));
        var bytes;
        if (f === 'raw') { bytes = data; }
        else if (f === 'jwk') {
            if (data.kty !== 'oct') { throw err('DataError', "The JWK 'kty' member is not 'oct'."); }
            bytes = unb64u(data.k, 'k');
            if (hmac) {
                if (data.alg !== undefined && data.alg !== HMAC_ALG[n.hash]) { throw err('DataError', "The JWK 'alg' member does not match the hash."); }
            } else {
                if ([16, 24, 32].indexOf(bytes.length) < 0) { throw err('DataError', 'AES key data must be 128, 192 or 256 bits.'); }
                if (data.alg !== undefined && data.alg !== aesJwkAlg(n.name, bytes.length)) { throw err('DataError', "The JWK 'alg' member does not match the key."); }
            }
            jwkChecks(data, ex, us, hmac ? 'sig' : 'enc');
        } else { throw err('NotSupportedError', "The '" + f + "' key format is not supported for " + n.name + '.'); }
        if (hmac) {
            var len = bytes.length * 8;
            if (len === 0) { throw err('DataError', 'HMAC key data must not be empty.'); }
            if (n.length !== undefined) {
                if (n.length > len || n.length <= len - 8) { throw err('DataError', 'The HMAC length does not match the key data.'); }
                len = n.length;
            }
            return mkKey({ name: 'HMAC', hash: n.hash, length: len }, ex, canonical(us), maskBits(bytes, len));
        }
        if ([16, 24, 32].indexOf(bytes.length) < 0) { throw err('DataError', 'AES key data must be 128, 192 or 256 bits.'); }
        return mkKey({ name: n.name, length: bytes.length * 8 }, ex, canonical(us), bytes);
    };

    // import key, RSA (§20.8 / §21.8 / §22.8)
    var RSA_JWK = {
        'RSASSA-PKCS1-v1_5': { 'SHA-1': 'RS1', 'SHA-256': 'RS256', 'SHA-384': 'RS384', 'SHA-512': 'RS512' },
        'RSA-PSS': { 'SHA-1': 'PS1', 'SHA-256': 'PS256', 'SHA-384': 'PS384', 'SHA-512': 'PS512' },
        'RSA-OAEP': { 'SHA-1': 'RSA-OAEP', 'SHA-256': 'RSA-OAEP-256', 'SHA-384': 'RSA-OAEP-384', 'SHA-512': 'RSA-OAEP-512' },
    };
    var rsaKey = function (n, d, ex, us, priv) {
        var bits = bitLength(d.n);
        var alg = rsaAlgObj(n, bits, strip(d.e));
        return mkKey(alg, priv ? ex : true, canonical(us), d, priv ? 'private' : 'public');
    };
    var importRsa = function (f, n, data, ex, us) {
        var pair = PAIR[n.name];
        if (f === 'spki') {
            only(us, pair[0]);
            var s = __subtle_der('spki', data);
            if (s === null || s[0] !== OID.rsa) { throw err('DataError', 'The key is not an RSA SubjectPublicKeyInfo.'); }
            var pk = __subtle_der('rsaPublic', s[3]);
            if (pk === null || !__subtle_rsa('checkPublic', pk[0], pk[1])) { throw err('DataError', 'The RSA public key is malformed.'); }
            return rsaKey(n, { n: u8(pk[0]), e: u8(pk[1]) }, ex, us, false);
        }
        if (f === 'pkcs8') {
            only(us, pair[1]);
            var p8 = __subtle_der('pkcs8', data);
            if (p8 === null || p8[0] !== OID.rsa) { throw err('DataError', 'The key is not an RSA PrivateKeyInfo.'); }
            var c = __subtle_der('rsaPrivate', p8[3]);
            if (c === null) { throw err('DataError', 'The RSA private key is malformed.'); }
            c = c.map(u8);
            if (!__subtle_rsa.apply(null, ['check'].concat(c))) { throw err('DataError', 'The RSA private key is inconsistent.'); }
            return rsaKey(n, { n: c[0], e: c[1], d: c[2], p: c[3], q: c[4], dp: c[5], dq: c[6], qi: c[7] }, ex, us, true);
        }
        if (f === 'jwk') {
            var priv = data.d !== undefined;
            only(us, priv ? pair[1] : pair[0]);
            if (data.kty !== 'RSA') { throw err('DataError', "The JWK 'kty' member is not 'RSA'."); }
            jwkChecks(data, ex, us, n.name === 'RSA-OAEP' ? 'enc' : 'sig');
            if (data.alg !== undefined && data.alg !== RSA_JWK[n.name][n.hash]) { throw err('DataError', "The JWK 'alg' member does not match the algorithm."); }
            var kd = { n: unb64u(data.n, 'n'), e: unb64u(data.e, 'e') };
            if (priv) {
                if (data.oth !== undefined) { throw err('NotSupportedError', 'Multi-prime RSA keys are not supported.'); }
                ['d', 'p', 'q', 'dp', 'dq', 'qi'].forEach(function (m) { kd[m] = unb64u(data[m], m); });
                if (!__subtle_rsa.apply(null, ['check'].concat(rsaParts(kd)))) { throw err('DataError', 'The RSA private key is inconsistent.'); }
            } else if (!__subtle_rsa('checkPublic', kd.n, kd.e)) { throw err('DataError', 'The RSA public key is malformed.'); }
            return rsaKey(n, kd, ex, us, priv);
        }
        throw err('NotSupportedError', "The '" + f + "' key format is not supported for " + n.name + '.');
    };

    // import key, EC (§23.8 ECDSA, §24.8 ECDH)
    var ES_ALG = { 'ES256': 'P-256', 'ES384': 'P-384', 'ES512': 'P-521' };
    var ecKey = function (n, curve, d, ex, us, priv) {
        return mkKey({ name: n.name, namedCurve: curve }, priv ? ex : true, canonical(us), d, priv ? 'private' : 'public');
    };
    var importEc = function (f, n, data, ex, us) {
        var pair = PAIR[n.name];
        var curve = n.namedCurve;
        var pt;
        if (f === 'raw') {
            if (CURVES.indexOf(curve) < 0) { throw err('DataError', 'The named curve is not supported.'); }
            only(us, pair[0]);
            curveOk(curve);
            pt = __subtle_ec('point', curve, data);
            if (pt === null) { throw err('DataError', 'The EC point is not valid.'); }
            return ecKey(n, curve, { pub: u8(pt) }, ex, us, false);
        }
        if (f === 'spki') {
            only(us, pair[0]);
            var s = __subtle_der('spki', data);
            if (s === null || !(s[0] === OID.ec || (n.name === 'ECDH' && s[0] === OID.ecdh))) { throw err('DataError', 'The key is not an EC SubjectPublicKeyInfo.'); }
            if (s[1] !== 'oid') { throw err('DataError', 'The EC parameters are not a named curve.'); }
            var c = curveOfOid(s[2]);
            if (c === undefined) { throw err('DataError', 'The named curve is not supported.'); }
            if (c !== curve) { throw err('DataError', 'The named curve does not match.'); }
            curveOk(c);
            pt = __subtle_ec('point', c, s[3]);
            if (pt === null) { throw err('DataError', 'The EC point is not valid.'); }
            return ecKey(n, c, { pub: u8(pt) }, ex, us, false);
        }
        if (f === 'pkcs8') {
            only(us, pair[1]);
            var p8 = __subtle_der('pkcs8', data);
            if (p8 === null || !(p8[0] === OID.ec || (n.name === 'ECDH' && p8[0] === OID.ecdh))) { throw err('DataError', 'The key is not an EC PrivateKeyInfo.'); }
            if (p8[1] !== 'oid') { throw err('DataError', 'The EC parameters are not a named curve.'); }
            var c8 = curveOfOid(p8[2]);
            if (c8 === undefined) { throw err('DataError', 'The named curve is not supported.'); }
            if (c8 !== curve) { throw err('DataError', 'The named curve does not match.'); }
            curveOk(c8);
            var ep = __subtle_der('ecPrivate', p8[3]);
            if (ep === null || (ep[1] !== null && ep[1] !== p8[2])) { throw err('DataError', 'The EC private key is malformed.'); }
            var dd = u8(ep[0]);
            if (dd.length !== CURVE_LEN[c8]) { throw err('DataError', 'The EC private key has the wrong length.'); }
            pt = __subtle_ec('public', c8, dd);
            if (pt === null) { throw err('DataError', 'The EC private key is not valid.'); }
            if (ep[2] !== null) {
                var given = __subtle_ec('point', c8, ep[2]);
                if (given === null || b64u(given) !== b64u(pt)) { throw err('DataError', 'The EC public key does not match the private key.'); }
            }
            return ecKey(n, c8, { d: dd, pub: u8(pt) }, ex, us, true);
        }
        if (f === 'jwk') {
            var priv = data.d !== undefined;
            only(us, priv ? pair[1] : pair[0]);
            if (data.kty !== 'EC') { throw err('DataError', "The JWK 'kty' member is not 'EC'."); }
            if (data.crv !== curve || CURVES.indexOf(curve) < 0) { throw err('DataError', "The JWK 'crv' member does not match the named curve."); }
            jwkChecks(data, ex, us, n.name === 'ECDH' ? 'enc' : 'sig');
            if (n.name === 'ECDSA' && data.alg !== undefined && ES_ALG[data.alg] !== undefined && ES_ALG[data.alg] !== curve) { throw err('DataError', "The JWK 'alg' member does not match the curve."); }
            curveOk(curve);
            var len = CURVE_LEN[curve];
            var x = unb64u(data.x, 'x'), y = unb64u(data.y, 'y');
            if (x.length !== len || y.length !== len) { throw err('DataError', 'The EC coordinates have the wrong length.'); }
            var raw = new Uint8Array(1 + 2 * len);
            raw[0] = 4; raw.set(x, 1); raw.set(y, 1 + len);
            pt = __subtle_ec('point', curve, raw);
            if (pt === null) { throw err('DataError', 'The EC point is not valid.'); }
            if (priv) {
                var dj = unb64u(data.d, 'd');
                if (dj.length !== len) { throw err('DataError', 'The EC private key has the wrong length.'); }
                var pj = __subtle_ec('public', curve, dj);
                if (pj === null || b64u(pj) !== b64u(pt)) { throw err('DataError', 'The EC public key does not match the private key.'); }
                return ecKey(n, curve, { d: dj, pub: u8(pt) }, ex, us, true);
            }
            return ecKey(n, curve, { pub: u8(pt) }, ex, us, false);
        }
        throw err('NotSupportedError', "The '" + f + "' key format is not supported for " + n.name + '.');
    };

    // import key, Ed25519 (§33.8) and X25519 (§32.8)
    var importOkp = function (f, n, data, ex, us) {
        var pair = PAIR[n.name];
        var oid = n.name === 'Ed25519' ? OID.ed25519 : OID.x25519;
        var okpKey = function (d, priv) { return mkKey({ name: n.name }, priv ? ex : true, canonical(us), d, priv ? 'private' : 'public'); };
        if (f === 'raw') {
            only(us, pair[0]);
            if (data.length !== 32) { throw err('DataError', 'The key must be 32 bytes.'); }
            return okpKey({ pub: data }, false);
        }
        if (f === 'spki') {
            only(us, pair[0]);
            var s = __subtle_der('spki', data);
            if (s === null || s[0] !== oid || s[1] !== 'absent') { throw err('DataError', 'The key is not an ' + n.name + ' SubjectPublicKeyInfo.'); }
            var pub = u8(s[3]);
            if (pub.length !== 32) { throw err('DataError', 'The key must be 32 bytes.'); }
            return okpKey({ pub: pub }, false);
        }
        if (f === 'pkcs8') {
            only(us, pair[1]);
            var p8 = __subtle_der('pkcs8', data);
            if (p8 === null || p8[0] !== oid || p8[1] !== 'absent') { throw err('DataError', 'The key is not an ' + n.name + ' PrivateKeyInfo.'); }
            var d = __subtle_der('octets', p8[3]);
            if (d === null || d.byteLength !== 32) { throw err('DataError', 'The private key must be 32 bytes.'); }
            d = u8(d);
            return okpKey({ d: d, pub: u8(__subtle_okp('public', n.name, d)) }, true);
        }
        if (f === 'jwk') {
            var priv = data.d !== undefined;
            only(us, priv ? pair[1] : pair[0]);
            if (data.kty !== 'OKP') { throw err('DataError', "The JWK 'kty' member is not 'OKP'."); }
            if (data.crv !== n.name) { throw err('DataError', "The JWK 'crv' member is not '" + n.name + "'."); }
            if (n.name === 'Ed25519' && data.alg !== undefined && data.alg !== 'Ed25519' && data.alg !== 'EdDSA') { throw err('DataError', "The JWK 'alg' member is not 'Ed25519' or 'EdDSA'."); }
            jwkChecks(data, ex, us, n.name === 'Ed25519' ? 'sig' : 'enc');
            var x = unb64u(data.x, 'x');
            if (x.length !== 32) { throw err('DataError', 'The public key must be 32 bytes.'); }
            if (priv) {
                var dj = unb64u(data.d, 'd');
                if (dj.length !== 32) { throw err('DataError', 'The private key must be 32 bytes.'); }
                var pj = u8(__subtle_okp('public', n.name, dj));
                if (b64u(pj) !== b64u(x)) { throw err('DataError', 'The public key does not match the private key.'); }
                return okpKey({ d: dj, pub: x }, true);
            }
            return okpKey({ pub: x }, false);
        }
        throw err('NotSupportedError', "The '" + f + "' key format is not supported for " + n.name + '.');
    };

    var importOf = function (f, n, data, ex, us) {
        if (RSA.indexOf(n.name) >= 0) { return importRsa(f, n, data, ex, us); }
        if (EC.indexOf(n.name) >= 0) { return importEc(f, n, data, ex, us); }
        if (OKP.indexOf(n.name) >= 0) { return importOkp(f, n, data, ex, us); }
        return importSecret(f, n, data, ex, us);
    };

    // export key (§§20-33 "export key")
    var exportOf = function (f, k) {
        var name = k.alg.name, d = k.data;
        var jwkTail = function (j) { j.key_ops = k.usages.slice(); j.ext = k.extractable; return j; };
        if (k.type === 'secret') {
            if (f === 'raw') { return abuf(d); }
            if (f === 'jwk') {
                var j = { kty: 'oct', k: b64u(d) };
                j.alg = name === 'HMAC' ? HMAC_ALG[k.alg.hash] : aesJwkAlg(name, d.length);
                return jwkTail(j);
            }
            throw err('NotSupportedError', "The '" + f + "' key format is not supported for " + name + '.');
        }
        var pub = k.type === 'public';
        if (f === 'spki' && !pub) { throw err('InvalidAccessError', 'Only a public key exports as spki.'); }
        if (f === 'pkcs8' && pub) { throw err('InvalidAccessError', 'Only a private key exports as pkcs8.'); }
        if (RSA.indexOf(name) >= 0) {
            if (f === 'spki') { return __subtle_der('buildSpki', OID.rsa, 'null', '', __subtle_der('buildRsaPublic', d.n, d.e)); }
            if (f === 'pkcs8') { return __subtle_der('buildPkcs8', OID.rsa, 'null', '', __subtle_der.apply(null, ['buildRsaPrivate'].concat(rsaParts(d)))); }
            if (f === 'jwk') {
                var r = { kty: 'RSA', alg: RSA_JWK[name][k.alg.hash], n: b64u(strip(d.n)), e: b64u(strip(d.e)) };
                if (!pub) { ['d', 'p', 'q', 'dp', 'dq', 'qi'].forEach(function (m) { r[m] = b64u(strip(d[m])); }); }
                return jwkTail(r);
            }
            throw err('NotSupportedError', "The 'raw' key format is not supported for " + name + '.');
        }
        if (EC.indexOf(name) >= 0) {
            var curve = k.alg.namedCurve, len = CURVE_LEN[curve];
            if (f === 'raw') {
                if (!pub) { throw err('InvalidAccessError', 'Only a public key exports as raw.'); }
                return abuf(d.pub);
            }
            if (f === 'spki') { return __subtle_der('buildSpki', OID.ec, 'oid', OID[curve], d.pub); }
            if (f === 'pkcs8') { return __subtle_der('buildPkcs8', OID.ec, 'oid', OID[curve], __subtle_der('buildEcPrivate', d.d, d.pub)); }
            var e = { kty: 'EC', crv: curve, x: b64u(d.pub.slice(1, 1 + len)), y: b64u(d.pub.slice(1 + len)) };
            if (!pub) { e.d = b64u(d.d); }
            return jwkTail(e);
        }
        var oid = name === 'Ed25519' ? OID.ed25519 : OID.x25519;
        if (f === 'raw') {
            if (!pub) { throw err('InvalidAccessError', 'Only a public key exports as raw.'); }
            return abuf(d.pub);
        }
        if (f === 'spki') { return __subtle_der('buildSpki', oid, 'absent', '', d.pub); }
        if (f === 'pkcs8') { return __subtle_der('buildPkcs8', oid, 'absent', '', __subtle_der('buildOctets', d.d)); }
        var o = { kty: 'OKP', crv: name, x: b64u(d.pub) };
        if (name === 'Ed25519') { o.alg = 'Ed25519'; }
        if (!pub) { o.d = b64u(d.d); }
        return jwkTail(o);
    };

    var deriveBitsOf = function (n, key, length) {
        if (n.name === 'ECDH' || n.name === 'X25519') {
            var pk = KS.get(n.public);
            if (pk.type !== 'public') { throw err('InvalidAccessError', "The 'public' member is not a public key."); }
            if (pk.alg.name !== key.alg.name) { throw err('InvalidAccessError', "The 'public' key is for another algorithm."); }
            var secret;
            if (n.name === 'ECDH') {
                if (pk.alg.namedCurve !== key.alg.namedCurve) { throw err('InvalidAccessError', "The 'public' key is on another curve."); }
                secret = u8(orFail(__subtle_ec('dh', key.alg.namedCurve, key.data.d, pk.data.pub)));
            } else {
                secret = u8(orFail(__subtle_okp('dh', 'X25519', key.data.d, pk.data.pub)));
            }
            if (length === null) { return abuf(secret); }
            if (length > secret.length * 8) { throw err('OperationError', 'The requested length is longer than the shared secret.'); }
            return abuf(maskBits(secret.slice(0, Math.ceil(length / 8)), length));
        }
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
        } else if (n.name === 'AES-CTR') {
            if (n.counter.length !== 16) { throw err('OperationError', 'The AES-CTR counter must be 16 bytes.'); }
            if (n.length === 0 || n.length > 128) { throw err('OperationError', 'The AES-CTR length must be between 1 and 128.'); }
            r = __subtle_aes_ctr(key.data, n.counter, n.length, data);
        } else if (n.name === 'AES-CBC') {
            if (n.iv.length !== 16) { throw err('OperationError', 'The AES-CBC iv must be 16 bytes.'); }
            r = __subtle_aes_cbc(enc, key.data, n.iv, data);
        } else {
            // RSA-OAEP (§22.4): encrypt needs the public key, decrypt the private one.
            if (key.type !== (enc ? 'public' : 'private')) { throw err('InvalidAccessError', 'The key is the wrong type for this operation.'); }
            var label = n.label === undefined ? new Uint8Array(0) : n.label;
            r = enc ? __subtle_rsa('encrypt', key.alg.hash, key.data.n, key.data.e, label, data)
                    : __subtle_rsa.apply(null, ['decrypt', key.alg.hash].concat(rsaParts(key.data), [label, data]));
        }
        return orFail(r);
    };
    var kw = function (wrap, key, data) { return orFail(__subtle_aes_kw(wrap, key.data, data)); };
    var signOf = function (n, k, data) {
        if (k.type === 'public') { throw err('InvalidAccessError', 'A public key cannot sign.'); }
        if (n.name === 'HMAC') { return __subtle_hmac(k.alg.hash, k.data, data); }
        if (n.name === 'ECDSA') { return orFail(__subtle_ec('sign', k.alg.namedCurve, k.data.d, n.hash, data)); }
        if (n.name === 'Ed25519') { return orFail(__subtle_okp('sign', 'Ed25519', k.data.d, data)); }
        var pss = n.name === 'RSA-PSS';
        return orFail(__subtle_rsa.apply(null, ['sign', pss, k.alg.hash].concat(rsaParts(k.data), [data, pss ? n.saltLength : 0])));
    };
    var verifyOf = function (n, k, sig, data) {
        if (n.name === 'HMAC') { return __subtle_hmac_verify(k.alg.hash, k.data, data, sig); }
        if (k.type !== 'public') { throw err('InvalidAccessError', 'Only a public key verifies.'); }
        if (n.name === 'ECDSA') { return __subtle_ec('verify', k.alg.namedCurve, k.data.pub, n.hash, data, sig); }
        if (n.name === 'Ed25519') { return __subtle_okp('verify', 'Ed25519', k.data.pub, data, sig); }
        var pss = n.name === 'RSA-PSS';
        return __subtle_rsa('verify', pss, k.alg.hash, k.data.n, k.data.e, data, sig, pss ? n.saltLength : 0);
    };
    // Shared general steps: the algorithm name must match the key's, and the key must allow the usage.
    var check = function (n, key, usage) {
        if (n.name !== key.alg.name) { throw err('InvalidAccessError', 'The requested operation is not valid for the provided key.'); }
        if (key.usages.indexOf(usage) < 0) { throw err('InvalidAccessError', 'The requested operation is not valid for the provided key.'); }
    };
    // generateKey / importKey / deriveKey / unwrapKey step: a secret or private key with no usages is a
    // SyntaxError; a CryptoKeyPair is checked by its private key.
    var finish = function (k) {
        var probe = isKey(k) ? k : k.privateKey;
        if ((ks(probe).type === 'secret' || ks(probe).type === 'private') && ks(probe).usages.length === 0) {
            throw err('SyntaxError', 'Usages cannot be empty when creating a key.');
        }
        return k;
    };
    // Every method returns a promise; argument conversion and every step's exception reject it (WebIDL §3.6.8).
    var run = function (f) {
        return new Promise(function (resolve) { resolve(f()); });
    };
    var keyData = function (f, keyData, what) {
        if (f === 'jwk') { return jwkOf(keyData, what); }
        return bytesOf(keyData, what);
    };
    // wrapKey / unwrapKey normalize as their own operation, then as encrypt / decrypt (§14.3.11 step 2).
    var normalizeWrap = function (alg, opname, fallback) {
        try { return normalize(alg, opname); }
        catch (e) {
            if (!(e instanceof DOMException) || e.name !== 'NotSupportedError') { throw e; }
            return normalize(alg, fallback);
        }
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
        importKey(format, keyData_, algorithm, extractable, keyUsages) {
            var args = arguments;
            return run(function () {
                need(args.length, 5, 'importKey');
                var f = formatOf(format);
                var data = keyData(f, keyData_, "Failed to execute 'importKey' on 'SubtleCrypto': parameter 2");
                var ex = !!extractable, us = usagesOf(keyUsages);
                var n = normalize(algorithm, 'importKey');
                return finish(importOf(f, n, data, ex, us));
            });
        },
        exportKey(format, key) {
            var args = arguments;
            return run(function () {
                need(args.length, 2, 'exportKey');
                var f = formatOf(format);
                var k = keyArg(key, 'exportKey');
                if (!REG.exportKey[k.alg.name]) { throw err('NotSupportedError', 'The key algorithm does not support export.'); }
                if (!k.extractable) { throw err('InvalidAccessError', 'The key is not extractable.'); }
                return exportOf(f, k);
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
                return signOf(n, k, bytes);
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
                return verifyOf(n, k, sig, bytes);
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
                return finish(importOf('raw', imp, new Uint8Array(bits), ex, us));
            });
        },
        wrapKey(format, key, wrappingKey, wrapAlgorithm) {
            var args = arguments;
            return run(function () {
                need(args.length, 4, 'wrapKey');
                var f = formatOf(format);
                var k = keyArg(key, 'wrapKey');
                var wk = keyArg(wrappingKey, 'wrapKey');
                var n = normalizeWrap(wrapAlgorithm, 'wrapKey', 'encrypt');
                check(n, wk, 'wrapKey');
                if (!REG.exportKey[k.alg.name]) { throw err('NotSupportedError', 'The key algorithm does not support export.'); }
                if (!k.extractable) { throw err('InvalidAccessError', 'The key is not extractable.'); }
                var exported = exportOf(f, k);
                var bytes = f === 'jwk' ? textBytes(JSON.stringify(exported)) : new Uint8Array(exported);
                return n.name === 'AES-KW' ? kw(true, wk, bytes) : cipher(true, n, wk, bytes);
            });
        },
        unwrapKey(format, wrappedKey, unwrappingKey, unwrapAlgorithm, unwrappedKeyAlgorithm, extractable, keyUsages) {
            var args = arguments;
            return run(function () {
                need(args.length, 7, 'unwrapKey');
                var f = formatOf(format);
                var wrapped = bytesOf(wrappedKey, "Failed to execute 'unwrapKey' on 'SubtleCrypto': parameter 2");
                var uk = keyArg(unwrappingKey, 'unwrapKey');
                var ex = !!extractable, us = usagesOf(keyUsages);
                var n = normalizeWrap(unwrapAlgorithm, 'unwrapKey', 'decrypt');
                var imp = normalize(unwrappedKeyAlgorithm, 'importKey');
                check(n, uk, 'unwrapKey');
                var bytes = u8(n.name === 'AES-KW' ? kw(false, uk, wrapped) : cipher(false, n, uk, wrapped));
                var data;
                if (f === 'jwk') {
                    var parsed;
                    try { parsed = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
                    catch (e) { throw err('DataError', 'The unwrapped key is not a JSON Web Key.'); }
                    data = jwkOf(parsed, 'The unwrapped key');
                } else { data = bytes; }
                return finish(importOf(f, imp, data, ex, us));
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

    /// AETHERWPT3: SP 800-38A F.2.1 (CBC-AES128 first block, then the PKCS #7 block), RFC 3394 §4.1 (128-bit KEK
    /// wrapping 128 bits), and their inverses; a corrupted wrap fails the integrity check.
    #[test]
    fn subtle_cbc_and_kw_known_answers() {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let key = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let iv: Vec<u8> = (0u8..16).collect();
        let pt = [0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a];
        let ct = aes_cbc(true, &key, &iv, &pt).unwrap();
        assert_eq!(ct.len(), 32);
        assert_eq!(hex(&ct[..16]), "7649abac8119b246cee98e9b12e9197d");
        assert_eq!(aes_cbc(false, &key, &iv, &ct).unwrap(), pt);
        assert!(aes_cbc(false, &key, &iv, &ct[..16]).is_none(), "a block of ciphertext without its padding block");
        // CRYPTOONE (SR330): byte pins from an independent oracle (`openssl enc -aes-{128,256}-cbc`, PKCS #7), plaintext
        // byte i = 7i+3, lengths across the pad edges; then the refusals (`openssl enc -nopad` blocks whose last bytes are
        // 0x00, 0x11 and a mismatched 03 02), an IV of 15 bytes and a ciphertext of 17 bytes.
        let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let k256 = unhex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
        for (k, n, want) in [
            (&key[..], 0, "c84af0b613435d5d9182801a9bd9320b"),
            (&key[..], 1, "120670e9d142292550e51267f1fca68c"),
            (&key[..], 15, "5423281508b6c0a4654ab8d0d005cd87"),
            (&key[..], 16, "0fa02a8340a0687ca4413328a063ed24a876931ec46e29d83efd7a03d34c73a5"),
            (&key[..], 17, "0fa02a8340a0687ca4413328a063ed24e93a2a7a7773fae5f8460332905722d0"),
            (&key[..], 33, "0fa02a8340a0687ca4413328a063ed248ae61fb0dfdb689e3ef022124fd852c88ad65a30376aea476002b0c535051c52"),
            (&k256[..], 0, "7e9248e5d829ca7593f0c549db2f5b8c"),
            (&k256[..], 1, "93606db43c3c82de8827b5d48df8507d"),
            (&k256[..], 15, "efad87737244744a330a65ee2b54ab4c"),
            (&k256[..], 16, "6b0cc2fa3f07c100597c412b40bebb35aaff3c06224594b97c07c93a608948b9"),
            (&k256[..], 17, "6b0cc2fa3f07c100597c412b40bebb35a55f0e5be5d776302735611fccf5a43c"),
            (&k256[..], 33, "6b0cc2fa3f07c100597c412b40bebb353f19f3cbbe9a00468597d4db1eb7229b05e3731af89121e3fda0e33ba996bc03"),
        ] {
            let p: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let c = aes_cbc(true, k, &iv, &p).unwrap();
            assert_eq!(hex(&c), want, "encrypt, {}-bit key, {n} bytes", k.len() * 8);
            assert_eq!(aes_cbc(false, k, &iv, &c).unwrap(), p, "decrypt, {}-bit key, {n} bytes", k.len() * 8);
        }
        for bad in ["eb593b41c96ffe158076a92d64c0c365", "cf7827403708b0b521a0a8b5b1dc2ba7", "1d32e73ed751219edca9e4ec134fdaca"] {
            assert!(aes_cbc(false, &key, &iv, &unhex(bad)).is_none(), "bad padding {bad}");
        }
        assert!(aes_cbc(true, &key, &iv[..15], &pt).is_none(), "a 15-byte IV");
        assert!(aes_cbc(false, &key, &iv[..15], &ct).is_none(), "a 15-byte IV");
        assert!(aes_cbc(false, &key, &iv, &ct[..17]).is_none(), "a ciphertext that is not whole blocks");
        assert!(aes_cbc(false, &key, &iv, &[]).is_none(), "an empty ciphertext");
        let kek: Vec<u8> = (0u8..16).collect();
        let data = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let wrapped = aes_kw(true, &kek, &data).unwrap();
        assert_eq!(hex(&wrapped), "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
        assert_eq!(aes_kw(false, &kek, &wrapped).unwrap(), data);
        let mut bad = wrapped.clone();
        bad[3] ^= 1;
        assert!(aes_kw(false, &kek, &bad).is_none());
    }
}
