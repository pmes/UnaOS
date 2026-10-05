#!/bin/sh
# TLSCORE2 (SR58) M3 PKI, via the openssl CLI, on top of gen_certs_openssl.sh's layout in the same directory:
#   cross-signed roots   oldroot, newroot (self-signed); cross = newroot's subject+key signed by oldroot (30 d),
#                        cross_short (1 d); inter2 under newroot; leaf2 (tlscore.test) under inter2
#   name constraints     nc_inter under root: permitted DNS:ok.test, email:ok.test, URI:.ok.test, dirName O=Good;
#                        excluded DNS:bad.ok.test. Leaves nc_good, nc_dns_out, nc_excl, nc_email_out, nc_dir_out, nc_uri_out
#   OCSP (RFC 6960)      for p256 (issuer: inter): ocsp_good.der (issuer-signed, SHA-1 CertID), ocsp_good256.der
#                        (SHA-256 CertID), ocsp_revoked.der, ocsp_delegated.der (responder cert with id-kp-OCSPSigning
#                        under inter), ocsp_bogus.der (signed by the ROOT: neither the issuer nor authorised)
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
