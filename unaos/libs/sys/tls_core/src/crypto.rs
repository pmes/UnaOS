//! `trait CryptoProvider` — exactly the primitives TLS 1.3 (RFC 8446) and X.509 path validation (RFC 5280) need.
//!
//! tls_core implements no primitive. CRYPTOCORE (`unaos/libs/sys/crypto_core`, SR27) implements this trait at the
//! fold; until then `test_provider::RustCryptoProvider` (feature `test-provider`, a test operand) does.
//!
//! Design rules:
//! * object-safe (`&dyn CryptoProvider` everywhere), no generics, no allocation in the signatures beyond `Vec` for
//!   AEAD in-place buffers;
//! * HKDF (RFC 5869) and HKDF-Expand-Label (RFC 8446 §7.1) are DEFAULT methods built on `hmac`, so a provider needs
//!   only HMAC — it may override them with a faster path;
//! * every operation a provider may not have yet (P-384, RSA) has a default body returning
//!   `CryptoError::Unsupported` with a message naming the missing primitive, and `supports_signature` tells the
//!   handshake what to offer so a server is never invited to pick something the provider cannot verify.

use alloc::vec::Vec;

/// A cryptographic failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// The provider does not implement this primitive (the message names it).
    Unsupported(&'static str),
    /// A signature did not verify.
    BadSignature,
    /// A public or private key was malformed / not on the curve / all-zero shared secret.
    BadKey,
    /// AEAD authentication failed.
    DecryptFailed,
    /// The randomness source failed.
    Rng,
    /// Anything else.
    Internal(&'static str),
}

/// Hash functions used by the TLS 1.3 cipher suites and certificate signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlg {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlg {
    pub const fn output_len(self) -> usize {
        match self {
            HashAlg::Sha256 => 32,
            HashAlg::Sha384 => 48,
            HashAlg::Sha512 => 64,
        }
    }
    /// HMAC block size (RFC 2104).
    pub const fn block_len(self) -> usize {
        match self {
            HashAlg::Sha256 => 64,
            HashAlg::Sha384 | HashAlg::Sha512 => 128,
        }
    }
}

/// The largest digest tls_core handles.
pub const MAX_HASH_LEN: usize = 64;

/// A digest / MAC / secret of up to 64 bytes, stack-allocated.
#[derive(Clone, Copy)]
pub struct Digest {
    buf: [u8; MAX_HASH_LEN],
    len: usize,
}

impl Digest {
    pub fn new(bytes: &[u8]) -> Self {
        assert!(bytes.len() <= MAX_HASH_LEN);
        let mut buf = [0u8; MAX_HASH_LEN];
        buf[..bytes.len()].copy_from_slice(bytes);
        Digest { buf, len: bytes.len() }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl core::fmt::Debug for Digest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for b in self.as_bytes() {
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

impl PartialEq for Digest {
    fn eq(&self, o: &Self) -> bool {
        crate::codec::ct_eq(self.as_bytes(), o.as_bytes())
    }
}

/// The TLS 1.3 AEADs (RFC 8446 §5.2, B.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadAlg {
    Aes128Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl AeadAlg {
    pub const fn key_len(self) -> usize {
        match self {
            AeadAlg::Aes128Gcm => 16,
            AeadAlg::Aes256Gcm | AeadAlg::ChaCha20Poly1305 => 32,
        }
    }
    pub const NONCE_LEN: usize = 12;
    pub const TAG_LEN: usize = 16;
}

/// Elliptic curves for ECDSA keys in certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcCurve {
    P256,
    P384,
}

/// A key-exchange private key held between ClientHello and ServerHello. Zeroised on drop.
pub struct KxPrivate {
    pub bytes: Vec<u8>,
}

impl Drop for KxPrivate {
    fn drop(&mut self) {
        for b in self.bytes.iter_mut() {
            // volatile-free best effort; forbid(unsafe_code) rules out write_volatile here.
            *b = 0;
        }
        core::hint::black_box(&self.bytes);
    }
}

/// The interface CRYPTOCORE implements.
pub trait CryptoProvider {
    /// Fill `out` with cryptographically secure random bytes.
    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError>;

    /// One-shot hash over the concatenation of `parts`.
    fn hash(&self, alg: HashAlg, parts: &[&[u8]]) -> Digest;

    /// HMAC (RFC 2104) over the concatenation of `parts`.
    fn hmac(&self, alg: HashAlg, key: &[u8], parts: &[&[u8]]) -> Digest;

    /// HKDF-Extract (RFC 5869 §2.2). An empty `salt` means HashLen zero octets.
    fn hkdf_extract(&self, alg: HashAlg, salt: &[u8], ikm: &[u8]) -> Digest {
        let zeros = [0u8; MAX_HASH_LEN];
        let salt = if salt.is_empty() { &zeros[..alg.output_len()] } else { salt };
        self.hmac(alg, salt, &[ikm])
    }

    /// HKDF-Expand (RFC 5869 §2.3) into `out` (at most 255*HashLen).
    fn hkdf_expand(&self, alg: HashAlg, prk: &[u8], info: &[&[u8]], out: &mut [u8]) -> Result<(), CryptoError> {
        let hl = alg.output_len();
        if out.len() > 255 * hl {
            return Err(CryptoError::Internal("hkdf output too long"));
        }
        let mut t: Digest = Digest::new(&[]);
        let mut done = 0;
        let mut counter = 1u8;
        while done < out.len() {
            let mut parts: Vec<&[u8]> = Vec::with_capacity(info.len() + 2);
            parts.push(t.as_bytes());
            parts.extend_from_slice(info);
            let c = [counter];
            parts.push(&c);
            t = self.hmac(alg, prk, &parts);
            let n = core::cmp::min(hl, out.len() - done);
            out[done..done + n].copy_from_slice(&t.as_bytes()[..n]);
            done += n;
            counter = counter.wrapping_add(1);
        }
        Ok(())
    }

    /// HKDF-Expand-Label (RFC 8446 §7.1): HkdfLabel = length(2) || "tls13 " + label (vec8) || context (vec8).
    fn hkdf_expand_label(
        &self,
        alg: HashAlg,
        secret: &[u8],
        label: &[u8],
        context: &[u8],
        out: &mut [u8],
    ) -> Result<(), CryptoError> {
        let len = (out.len() as u16).to_be_bytes();
        let full_label_len = [(6 + label.len()) as u8];
        let ctx_len = [context.len() as u8];
        self.hkdf_expand(alg, secret, &[&len, &full_label_len, b"tls13 ", label, &ctx_len, context], out)
    }

    /// The TLS 1.2 PRF (RFC 5246 §5): `P_hash(secret, label + seed)` into `out`. A DEFAULT built on `hmac`, like
    /// HKDF; CRYPTOCORE overrides it with `crypto_core::tls12_prf` (the same function, KAT-proven there).
    fn tls12_prf(&self, alg: HashAlg, secret: &[u8], label: &[u8], seed: &[&[u8]], out: &mut [u8]) -> Result<(), CryptoError> {
        let mut parts: Vec<&[u8]> = Vec::with_capacity(seed.len() + 1);
        parts.push(label);
        parts.extend_from_slice(seed);
        let mut a = self.hmac(alg, secret, &parts); // A(1)
        let mut done = 0;
        while done < out.len() {
            let mut p2: Vec<&[u8]> = Vec::with_capacity(parts.len() + 1);
            p2.push(a.as_bytes());
            p2.extend_from_slice(&parts);
            let block = self.hmac(alg, secret, &p2);
            let n = core::cmp::min(block.len(), out.len() - done);
            out[done..done + n].copy_from_slice(&block.as_bytes()[..n]);
            done += n;
            a = self.hmac(alg, secret, &[a.as_bytes()]);
        }
        Ok(())
    }

    /// SHA-1 — ONLY to match an OCSP `CertID` (RFC 6960 §4.1.1), whose name/key hashes responders compute with
    /// SHA-1. Never a signature. Default `Unsupported` (a CertID in SHA-256 still matches without it).
    fn sha1(&self, parts: &[&[u8]]) -> Result<[u8; 20], CryptoError> {
        let _ = parts;
        Err(CryptoError::Unsupported("SHA-1 (OCSP CertID): this CryptoProvider has none"))
    }

    /// AEAD seal in place: `in_out` holds the plaintext on entry and ciphertext || tag on return.
    fn aead_seal(
        &self,
        alg: AeadAlg,
        key: &[u8],
        nonce: &[u8; 12],
        aad: &[u8],
        in_out: &mut Vec<u8>,
    ) -> Result<(), CryptoError>;

    /// AEAD open in place: `in_out` holds ciphertext || tag on entry and the plaintext on return.
    fn aead_open(
        &self,
        alg: AeadAlg,
        key: &[u8],
        nonce: &[u8; 12],
        aad: &[u8],
        in_out: &mut Vec<u8>,
    ) -> Result<(), CryptoError>;

    /// Generate an X25519 key pair (RFC 7748): returns (private 32 bytes, public 32 bytes). The private scalar is
    /// drawn from `random` so a deterministic RNG reproduces a trace.
    fn x25519_keypair(&self) -> Result<(KxPrivate, [u8; 32]), CryptoError>;

    /// X25519 shared secret. Must reject the all-zero output (RFC 8446 §7.4.2).
    fn x25519_shared(&self, private: &KxPrivate, peer_public: &[u8]) -> Result<[u8; 32], CryptoError>;

    /// Generate a secp256r1 key pair: returns (private scalar 32 bytes, uncompressed SEC1 point 65 bytes).
    fn p256_keypair(&self) -> Result<(KxPrivate, Vec<u8>), CryptoError>;

    /// secp256r1 ECDH: the x-coordinate of private * peer (RFC 8446 §7.4.2). `peer_public` is an uncompressed SEC1
    /// point and must be validated on the curve.
    fn p256_ecdh(&self, private: &KxPrivate, peer_public: &[u8]) -> Result<[u8; 32], CryptoError>;

    /// ECDSA verify. `public_key` is the SEC1 point from the SubjectPublicKeyInfo, `sig_der` the DER
    /// Ecdsa-Sig-Value, `msg` the signed message (the provider hashes it with `hash`). P-256 is required; P-384 is a
    /// default `Unsupported` until a provider has it (public PKI chains use it — see the X.509 ceiling).
    fn ecdsa_verify(
        &self,
        curve: EcCurve,
        hash: HashAlg,
        public_key: &[u8],
        msg: &[u8],
        sig_der: &[u8],
    ) -> Result<(), CryptoError>;

    /// Ed25519 verify (RFC 8032, pure). `public_key` 32 bytes, `sig` 64 bytes.
    fn ed25519_verify(&self, public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError>;

    /// RSASSA-PSS verify (RFC 8017 §8.1.2), MGF1 with the same hash, salt length = hash length (RFC 8446 §4.2.3).
    /// `n`/`e` are the big-endian modulus and exponent from the RSAPublicKey.
    fn rsa_pss_verify(&self, hash: HashAlg, n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        let _ = (hash, n, e, msg, sig);
        Err(CryptoError::Unsupported("RSASSA-PSS verify: this CryptoProvider has no RSA"))
    }

    /// RSASSA-PKCS1-v1_5 verify (RFC 8017 §8.2.2) — certificate signatures only; never a TLS 1.3 CertificateVerify.
    fn rsa_pkcs1_verify(&self, hash: HashAlg, n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        let _ = (hash, n, e, msg, sig);
        Err(CryptoError::Unsupported("RSASSA-PKCS1-v1_5 verify: this CryptoProvider has no RSA"))
    }

    /// Does the provider verify this signature scheme? The ClientHello offers only schemes that answer true. The
    /// default is the CRYPTOCORE baseline: ECDSA P-256/SHA-256 and Ed25519.
    fn supports_signature(&self, scheme: crate::msgs::SignatureScheme) -> bool {
        use crate::msgs::SignatureScheme as S;
        matches!(scheme, S::EcdsaSecp256r1Sha256 | S::Ed25519)
    }

    /// Does the provider implement this AEAD? Default: all three TLS 1.3 AEADs.
    /// CTCORE: an ML-KEM-768 key pair (FIPS 203 ML-KEM.KeyGen): (decapsulation key dk 2400 B, encapsulation key ek
    /// 1184 B). Default: unsupported (the hybrid group is then not offered).
    fn mlkem768_keypair(&self) -> Result<(KxPrivate, Vec<u8>), CryptoError> {
        Err(CryptoError::Unsupported("ML-KEM-768"))
    }

    /// CTCORE: ML-KEM-768 Decaps (FIPS 203 Alg 21, implicit rejection) of a 1088-byte ciphertext.
    fn mlkem768_decaps(&self, dk: &KxPrivate, ct: &[u8]) -> Result<[u8; 32], CryptoError> {
        let _ = (dk, ct);
        Err(CryptoError::Unsupported("ML-KEM-768"))
    }

    /// Whether this provider can do a key exchange in `group` (the client offers only those).
    fn supports_group(&self, group: crate::msgs::NamedGroup) -> bool {
        !group.is_hybrid()
    }

    fn supports_aead(&self, alg: AeadAlg) -> bool {
        let _ = alg;
        true
    }

    /// Does the provider implement this hash (SHA-384 backs TLS_AES_256_GCM_SHA384)? Default: yes.
    fn supports_hash(&self, alg: HashAlg) -> bool {
        let _ = alg;
        true
    }
}
