#!/bin/sh
# TLSCORE2 (SR58) M3 PKI, via the openssl CLI, on top of gen_certs_openssl.sh's layout in the same directory:
#   cross-signed roots   oldroot, newroot (self-signed); cross = newroot's subject+key signed by oldroot (30 d),
#                        cross_short (1 d); inter2 under newroot; leaf2 (tlscore.test) under inter2
#   name constraints     nc_inter under root: permitted DNS:ok.test, email:ok.test, URI:.ok.test, dirName O=Good;
#                        excluded DNS:bad.ok.test. Leaves nc_good, nc_dns_out, nc_excl, nc_email_out, nc_dir_out, nc_uri_out
#   OCSP (RFC 6960)      for p256 (issuer: inter): ocsp_good.der (issuer-signed, SHA-1 CertID), ocsp_good256.der
#                        (SHA-256 CertID), ocsp_revoked.der, ocsp_delegated.der (responder cert with id-kp-OCSPSigning
#                        under inter), ocsp_bogus.der (signed by the ROOT: neither the issuer nor authorised)
#   CTCORE (SR60) M2:
#   must-staple          ms (tlscore.test, TLS Feature status_request, RFC 7633) under inter: ocsp_ms_good.der,
#                        ocsp_ms_revoked.der, ocsp_ms_unknown.der (the responder does not know it)
#   CRLs (RFC 5280 §5)   by `openssl ca -gencrl` with inter's key, about the p256 leaf: crl_empty (#1),
#                        crl_revoked (#2, keyCompromise), crl_hold (#3, certificateHold), delta_remove (#4, delta of
#                        base 3: removeFromCRL), delta_add (#5, delta of base 1: keyCompromise), crl_stale (#6, nextUpdate
#                        a day ago), crl_wrongkey (#7, inter's name, root's key); root_crl_inter (root's CRL revoking
#                        inter), root_crl_empty. DER, *.der; PEM twins *.crl.pem for `openssl verify -CRLfile`.
set -e
cd "$1"
leafext() { # name, san
cat > "$1.ext" <<X
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=$2
X
}
cat > ca.ext <<X
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
X
key() { openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$1.key" 2>/dev/null; }

# ---- cross-signed roots
key oldroot; key newroot; key inter2; key leaf2
openssl req -x509 -new -key oldroot.key -subj "/CN=TLSCORE2 Old Root" -days 60 -sha256 -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" -addext "subjectKeyIdentifier=hash" -out oldroot.pem
openssl req -x509 -new -key newroot.key -subj "/CN=TLSCORE2 New Root" -days 60 -sha256 -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" -addext "subjectKeyIdentifier=hash" -out newroot.pem
openssl req -new -key newroot.key -subj "/CN=TLSCORE2 New Root" -out newroot.csr
openssl x509 -req -in newroot.csr -CA oldroot.pem -CAkey oldroot.key -CAcreateserial -days 30 -sha256 -extfile ca.ext -out cross.pem 2>/dev/null
openssl x509 -req -in newroot.csr -CA oldroot.pem -CAkey oldroot.key -CAcreateserial -days 1 -sha256 -extfile ca.ext -out cross_short.pem 2>/dev/null
openssl req -new -key inter2.key -subj "/CN=TLSCORE2 Inter2" -out inter2.csr
openssl x509 -req -in inter2.csr -CA newroot.pem -CAkey newroot.key -CAcreateserial -days 30 -sha256 -extfile inter.ext -out inter2.pem 2>/dev/null
leafext leaf2 "DNS:tlscore.test"
openssl req -new -key leaf2.key -subj "/CN=tlscore.test" -out leaf2.csr
openssl x509 -req -in leaf2.csr -CA inter2.pem -CAkey inter2.key -CAcreateserial -days 30 -sha256 -extfile leaf2.ext -out leaf2.pem 2>/dev/null

# ---- name constraints
key nc_inter
cat > nc.ext <<X
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
nameConstraints=critical,permitted;DNS:ok.test,permitted;email:ok.test,permitted;URI:.ok.test,permitted;dirName:nc_dir,excluded;DNS:bad.ok.test
[nc_dir]
O=Good
X
openssl req -new -key nc_inter.key -subj "/CN=TLSCORE2 NC Inter" -out nc_inter.csr
openssl x509 -req -in nc_inter.csr -CA root.pem -CAkey root.key -CAcreateserial -days 30 -sha256 -extfile nc.ext -out nc_inter.pem 2>/dev/null
ncleaf() { # name subject san
  key "$1"; leafext "$1" "$3"
  openssl req -new -key "$1.key" -subj "$2" -out "$1.csr"
  openssl x509 -req -in "$1.csr" -CA nc_inter.pem -CAkey nc_inter.key -CAcreateserial -days 30 -sha256 -extfile "$1.ext" -out "$1.pem" 2>/dev/null
}
ncleaf nc_good "/O=Good/CN=www.ok.test" "DNS:www.ok.test,email:a@ok.test,URI:https://x.ok.test/"
ncleaf nc_dns_out "/O=Good/CN=www.evil.test" "DNS:www.evil.test"
ncleaf nc_excl "/O=Good/CN=bad.ok.test" "DNS:bad.ok.test"
ncleaf nc_email_out "/O=Good/CN=www.ok.test" "DNS:www.ok.test,email:a@evil.test"
ncleaf nc_dir_out "/O=Evil/CN=www.ok.test" "DNS:www.ok.test"
ncleaf nc_uri_out "/O=Good/CN=www.ok.test" "DNS:www.ok.test,URI:https://x.evil.test/"

# ---- OCSP for the p256 leaf
SERIAL=$(openssl x509 -in p256.pem -noout -serial | cut -d= -f2)
END=$(date -u -d '+30 days' +%y%m%d%H%M%SZ)
REV=$(date -u -d '-1 hour' +%y%m%d%H%M%SZ)
printf 'V\t%s\t\t%s\tunknown\t/CN=tlscore.test\n' "$END" "$SERIAL" > index_good.txt
printf 'R\t%s\t%s\t%s\tunknown\t/CN=tlscore.test\n' "$END" "$REV" "$SERIAL" > index_revoked.txt
key ocsp
cat > ocsp.ext <<X
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=OCSPSigning
X
openssl req -new -key ocsp.key -subj "/CN=TLSCORE2 OCSP Responder" -out ocsp.csr
openssl x509 -req -in ocsp.csr -CA inter.pem -CAkey inter.key -CAcreateserial -days 30 -sha256 -extfile ocsp.ext -out ocsp.pem 2>/dev/null
openssl ocsp -issuer inter.pem -cert p256.pem -no_nonce -reqout req_sha1.der
openssl ocsp -sha256 -issuer inter.pem -cert p256.pem -no_nonce -reqout req_sha256.der
resp() { # index signer key reqin out
  openssl ocsp -index "$1" -rsigner "$2" -rkey "$3" -CA inter.pem -reqin "$4" -respout "$5" -ndays 1 >/dev/null 2>&1
}
resp index_good.txt inter.pem inter.key req_sha1.der ocsp_good.der
resp index_good.txt inter.pem inter.key req_sha256.der ocsp_good256.der
resp index_revoked.txt inter.pem inter.key req_sha1.der ocsp_revoked.der
resp index_good.txt ocsp.pem ocsp.key req_sha1.der ocsp_delegated.der
resp index_good.txt root.pem root.key req_sha1.der ocsp_bogus.der

# ---- CTCORE M2: must-staple
key ms
leafext ms "DNS:tlscore.test,DNS:localhost,IP:127.0.0.1"
echo "tlsfeature=status_request" >> ms.ext
openssl req -new -key ms.key -subj "/CN=tlscore.test" -out ms.csr
openssl x509 -req -in ms.csr -CA inter.pem -CAkey inter.key -CAcreateserial -days 30 -sha256 -extfile ms.ext -out ms.pem 2>/dev/null
MSERIAL=$(openssl x509 -in ms.pem -noout -serial | cut -d= -f2)
printf 'V\t%s\t\t%s\tunknown\t/CN=tlscore.test\n' "$END" "$MSERIAL" > index_ms_good.txt
printf 'R\t%s\t%s\t%s\tunknown\t/CN=tlscore.test\n' "$END" "$REV" "$MSERIAL" > index_ms_revoked.txt
: > index_ms_unknown.txt
openssl ocsp -issuer inter.pem -cert ms.pem -no_nonce -reqout req_ms.der
resp index_ms_good.txt inter.pem inter.key req_ms.der ocsp_ms_good.der
resp index_ms_revoked.txt inter.pem inter.key req_ms.der ocsp_ms_revoked.der
resp index_ms_unknown.txt inter.pem inter.key req_ms.der ocsp_ms_unknown.der

# ---- CTCORE M2: CRLs
crl() { # ca-cert ca-key index-line-or-empty out [delta-base] [extra openssl ca args...]
  cacert=$1; cakey=$2; line=$3; out=$4; base=$5; shift 5
  printf '%b' "$line" > crl_index.txt
  { echo "[ca]"; echo "default_ca=c"; echo "[c]"; echo "database=crl_index.txt"; echo "crlnumber=crlnumber_$(basename "$cacert" .pem)"
    echo "default_md=sha256"; echo "default_crl_days=7"; echo "crl_extensions=crlext"; echo "[crlext]"; [ -n "$NOAKI" ] || echo "authorityKeyIdentifier=keyid"; [ -z "$FRESH" ] || echo "freshestCRL=URI:http://crl.test/inter-delta.crl"
    if [ -n "$base" ]; then echo "2.5.29.27=critical,DER:02:01:$(printf %02x "$base")"; fi; } > crl.cnf
  openssl ca -config crl.cnf -gencrl -cert "$cacert" -keyfile "$cakey" "$@" -out "$out.crl.pem" 2>/dev/null
  openssl crl -in "$out.crl.pem" -outform DER -out "$out.der"
}
echo 01 > crlnumber_inter; echo 01 > crlnumber_root; echo 07 > crlnumber_root_wk
P=$SERIAL
FRESH=1 crl inter.pem inter.key "" crl_empty ""
crl inter.pem inter.key "R\t$END\t$REV,keyCompromise\t$P\tunknown\t/CN=tlscore.test\n" crl_revoked ""
FRESH=1 crl inter.pem inter.key "R\t$END\t$REV,certificateHold,holdInstructionReject\t$P\tunknown\t/CN=tlscore.test\n" crl_hold ""
crl inter.pem inter.key "R\t$END\t$REV,removeFromCRL\t$P\tunknown\t/CN=tlscore.test\n" delta_remove 3
crl inter.pem inter.key "R\t$END\t$REV,keyCompromise\t$P\tunknown\t/CN=tlscore.test\n" delta_add 1
crl inter.pem inter.key "" crl_stale "" -crl_lastupdate "$(date -u -d '-3 days' +%y%m%d%H%M%SZ)" -crl_nextupdate "$(date -u -d '-1 day' +%y%m%d%H%M%SZ)"
# inter's name (no AKI), root's key: openssl ca refuses a key that does not match the cert, so sign with a forged twin.
openssl req -new -key root.key -subj "/CN=TLSCORE Oracle Intermediate" -out forged.csr
openssl x509 -req -in forged.csr -signkey root.key -days 30 -sha256 -out forged_inter.pem 2>/dev/null
cp crlnumber_inter crlnumber_forged_inter
NOAKI=1 crl forged_inter.pem root.key "" crl_wrongkey ""
ISERIAL=$(openssl x509 -in inter.pem -noout -serial | cut -d= -f2)
crl root.pem root.key "" root_crl_empty ""
crl root.pem root.key "R\t$END\t$REV,keyCompromise\t$ISERIAL\tunknown\t/CN=TLSCORE Oracle Intermediate\n" root_crl_inter ""
