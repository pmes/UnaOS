// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — handler. PDF-KEYSTORE (LEDGER SR382): sign a PDF with a key HOLOCRON holds (CODEX §2 Secrets owns
//! the key; Aether owns the PDF). No key, parser or CMS code lives here: pdf_core's `Document::sign` writes the update
//! and the CMS through a `SignerKey::Remote` built from the signer certificate (`Remote::for_certificate`), and each
//! signature is Holocron's answer to Sign's long form (`Request::SignWith`, verb 150) on Holocron's OWN socket
//! (`holocron::client::Client`, SO_PEERCRED-stamped — never the bandy Synapse, which has no principal). Only the digest
//! of the signed attributes leaves Aether; only the signature comes back.

use std::cell::RefCell;
use std::io;
use std::path::Path;

use holocron::client::Client;
use holocron::holocron_core::wire::{self, Reply, Request, key_kind, sign_alg, status};
use pdf_core::Document;
use pdf_core::sig::cms::{Hash, Remote, Scheme, SignerKey};
use pdf_core::sig::{SignRequest, SubFilter};

/// Sign `original` (the bytes `doc` was parsed from) into the signature field `field` (an unsigned /Sig field, else a
/// new invisible one on page 1) with the key `<ns>/<name>` Holocron holds at the bus socket `sock`; `chain[0]` is the
/// key's certificate (its public key picks the scheme: RSA PKCS#1 v1.5 or P-256 over SHA-256, Ed25519), the whole chain
/// goes into the CMS (ETSI.CAdES.detached). `time` is /M and signingTime; `entropy` feeds an encrypted file's IVs.
/// The new file, or why not (Holocron's answer when it refused).
#[allow(clippy::too_many_arguments)]
pub fn pdf_sign_with_holocron(
    doc: &Document,
    original: &[u8],
    field: &str,
    chain: &[&[u8]],
    ns: &str,
    name: &str,
    sock: &Path,
    time: i64,
    entropy: &[u8; 32],
) -> Result<Vec<u8>, String> {
    sign_through(doc, original, field, chain, ns, name, time, entropy, &mut |r| Client::connect(sock)?.call(r))
}

/// The long-form `alg` byte for a scheme.
fn alg(s: Scheme) -> u8 {
    let h = |h: Hash| match h {
        Hash::Sha256 => sign_alg::SHA256,
        Hash::Sha384 => sign_alg::SHA384,
        Hash::Sha512 => sign_alg::SHA512,
    };
    match s {
        Scheme::Pkcs1(x) | Scheme::P256(x) => h(x),
        Scheme::Pss(x) => h(x) | sign_alg::PSS,
        Scheme::Ed25519 => sign_alg::NONE,
    }
}

/// The key kind Holocron must answer for a scheme.
fn kind(s: Scheme) -> u8 {
    match s {
        Scheme::Pkcs1(_) | Scheme::Pss(_) => key_kind::RSA,
        Scheme::P256(_) => key_kind::P256,
        Scheme::Ed25519 => key_kind::ED25519,
    }
}

/// [`pdf_sign_with_holocron`] over any transport `call` (one Holocron request in, its reply out).
#[allow(clippy::too_many_arguments)]
fn sign_through(
    doc: &Document,
    original: &[u8],
    field: &str,
    chain: &[&[u8]],
    ns: &str,
    name: &str,
    time: i64,
    entropy: &[u8; 32],
    call: &mut dyn FnMut(&Request) -> io::Result<Reply>,
) -> Result<Vec<u8>, String> {
    let call = RefCell::new(call);
    let why: RefCell<Option<String>> = RefCell::new(None);
    let ask = |s: Scheme, tbs: &[u8]| -> Result<Vec<u8>, String> {
        let req = Request::SignWith { ns: ns.into(), key: name.into(), alg: alg(s), data: tbs.to_vec() };
        let reply = {
            let mut c = call.borrow_mut();
            (*c)(&req).map_err(|e| format!("holocron: {e}"))?
        };
        if reply.status != status::OK {
            return Err(format!("holocron: {ns}/{name} {} ({})", status::word(reply.status), reply.status)); // KEYSIGNONE (SR393): the reason named
        }
        match wire::decode_sign(&reply.body) {
            Some((k, sig)) if k == kind(s) => Ok(sig.to_vec()),
            Some((k, _)) => Err(format!("holocron: {ns}/{name} is key kind {k}, not the certificate's")),
            None => Err("holocron: empty Sign reply".into()),
        }
    };
    let cb = |s: Scheme, tbs: &[u8]| {
        let r = ask(s, tbs);
        if let Err(e) = &r {
            *why.borrow_mut() = Some(e.clone());
        }
        r
    };
    let first = *chain.first().ok_or_else(|| "no signer certificate".to_string())?;
    let remote = Remote::for_certificate(first, Hash::Sha256, false, &cb).map_err(|e| format!("{e:?}"))?;
    let req = SignRequest {
        field,
        page: 0,
        key: SignerKey::Remote(remote),
        chain,
        subfilter: SubFilter::CadesDetached,
        name: None,
        reason: None,
        location: None,
        time,
        certify: None,
    };
    doc.sign(original, &req, entropy).map_err(|e| why.borrow_mut().take().unwrap_or_else(|| format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use holocron::holocron_core::pkcs8;
    use holocron::holocron_core::seal::{Entropy, EntropyError};
    use pdf_core::sig::cms::SecretKey;
    use pdf_core::sig::{Chain, Changes, Trust, Verdict};
    use tls_core::key::{AlgParams, ec_private_build, pkcs8_build};
    use tls_core::x509::build::{self, CertSpec};
    use tls_core::x509::oid;
    use tls_core::x509::verify::TrustStore;

    /// 2026-10-27T12:00:00Z, inside the certificates' validity.
    const AT: i64 = 1_793_102_400;
    const ENTROPY: [u8; 32] = [7; 32];

    struct Fixed;
    impl Entropy for Fixed {
        fn fill(&mut self, b: &mut [u8]) -> Result<(), EntropyError> {
            b.fill(7);
            Ok(())
        }
    }

    /// One page, no form.
    fn pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] >>",
        ];
        let mut b = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(b.len());
            b.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = b.len();
        let mut t = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
        for o in offs {
            t += &format!("{o:010} 00000 n \n");
        }
        t += &format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1);
        b.extend_from_slice(t.as_bytes());
        b
    }

    fn cert(serial: u8, subject: &str, key: &SignerKey<'_>, issuer: &SignerKey<'_>, ca: bool) -> Vec<u8> {
        let spki = key.spki();
        let alg = issuer.cert_alg();
        let spec = CertSpec {
            serial: &[serial],
            issuer: "PDF-KEYSTORE KAT CA",
            subject,
            not_before: 1_735_689_600,
            not_after: 2_082_672_000,
            spki: &spki,
            ca,
        };
        let tbs = build::tbs(&spec, &alg);
        build::certificate(&tbs, &alg, &issuer.sign(&tbs))
    }

    /// The signer key (P-256), its PKCS#8 file (what Holocron holds), the leaf and the CA certificates.
    fn fixture() -> (SecretKey, Vec<u8>, Vec<u8>, Vec<u8>) {
        let d = [0x11u8; 32];
        let k = SecretKey::from_bytes(&d).unwrap();
        let file = pkcs8_build(oid::EC_PUBLIC_KEY, &AlgParams::Oid(oid::PRIME256V1), &ec_private_build(&d, &k.public_key().to_sec1_uncompressed()));
        let ca = SecretKey::from_bytes(&[0x33; 32]).unwrap();
        let cak = SignerKey::P256(&ca, Hash::Sha256);
        let ca_der = cert(1, "PDF-KEYSTORE KAT CA", &cak, &cak, true);
        let leaf_der = cert(2, "PDF-KEYSTORE KAT Signer", &SignerKey::P256(&k, Hash::Sha256), &cak, false);
        (k, file, leaf_der, ca_der)
    }

    /// A fake Holocron fulfiller: the request crosses the wire codec, and `sign/doc` is answered by holocron_core's own
    /// `pkcs8::sign` over `file` (the function the service runs); anything else is NOT_FOUND.
    fn fake(file: Vec<u8>) -> impl FnMut(&Request) -> io::Result<Reply> {
        move |r| {
            let back = Request::decode(r.verb(), &r.encode_body()).map_err(|s| io::Error::other(format!("status {s}")))?;
            Ok(match &back {
                Request::SignWith { ns, key, alg, data } if ns == "sign" && key == "doc" => match pkcs8::sign(&file, *alg, data, &mut Fixed) {
                    Ok(b) => Reply::ok(b),
                    Err(s) => Reply::err(s),
                },
                _ => Reply::err(status::NOT_FOUND),
            })
        }
    }

    #[test]
    fn holocron_signer_matches_local_and_verifies() {
        let (k, file, leaf, ca) = fixture();
        let chain = [leaf.as_slice(), ca.as_slice()];
        let original = pdf();
        let doc = Document::parse(&original).unwrap();
        let out = sign_through(&doc, &original, "Signature1", &chain, "sign", "doc", AT, &ENTROPY, &mut fake(file)).unwrap();
        let local = SignRequest {
            field: "Signature1",
            page: 0,
            key: SignerKey::P256(&k, Hash::Sha256),
            chain: &chain,
            subfilter: SubFilter::CadesDetached,
            name: None,
            reason: None,
            location: None,
            time: AT,
            certify: None,
        };
        assert_eq!(out, doc.sign(&original, &local, &ENTROPY).unwrap(), "the Holocron-signed file is the local one, byte for byte");
        let signed = Document::parse(&out).unwrap();
        let mut store = TrustStore::new();
        store.add_der(&ca).unwrap();
        let trust = Trust { store: &store, at: AT };
        let sigs = signed.signatures(&out);
        assert_eq!(sigs.len(), 1);
        let v = sigs[0].verify(&signed, &out, Some(&trust));
        assert!(matches!(v, Verdict::Signed { chain: Chain::Trusted, changes: Changes::None, .. }), "{v:?}");
    }

    #[test]
    fn holocron_refusal_is_the_reason() {
        let (_, file, leaf, ca) = fixture();
        let chain = [leaf.as_slice(), ca.as_slice()];
        let original = pdf();
        let doc = Document::parse(&original).unwrap();
        let err = sign_through(&doc, &original, "Signature1", &chain, "sign", "missing", AT, &ENTROPY, &mut fake(file)).unwrap_err();
        assert_eq!(err, "holocron: sign/missing not-found (-2)");
    }
}
