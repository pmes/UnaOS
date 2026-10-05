#!/bin/sh
# TLSCORE oracle PKI via the openssl CLI (used when Python `cryptography` is unavailable). Same layout as gen_certs.py.
set -e
cd "$1"
cat > inter.ext <<X
basicConstraints=critical,CA:TRUE,pathlen:0
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid
X
cat > leaf.ext <<X
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=DNS:tlscore.test,DNS:localhost,IP:127.0.0.1
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid
X
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out root.key 2>/dev/null
openssl req -x509 -new -key root.key -subj "/CN=TLSCORE Oracle Root" -days 30 -sha256 \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" -out root.pem
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out inter.key 2>/dev/null
openssl req -new -key inter.key -subj "/CN=TLSCORE Oracle Intermediate" -out inter.csr
openssl x509 -req -in inter.csr -CA root.pem -CAkey root.key -CAcreateserial -days 30 -sha256 -extfile inter.ext -out inter.pem 2>/dev/null
for leaf in p256 ed25519 rsa; do
  case $leaf in
    p256) openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out $leaf.key 2>/dev/null ;;
    ed25519) openssl genpkey -algorithm ED25519 -out $leaf.key ;;
    rsa) openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out $leaf.key 2>/dev/null ;;
  esac
  openssl req -new -key $leaf.key -subj "/CN=tlscore.test" -out $leaf.csr
  openssl x509 -req -in $leaf.csr -CA inter.pem -CAkey inter.key -CAcreateserial -days 30 -sha256 -extfile leaf.ext -out $leaf.pem 2>/dev/null
  cat $leaf.pem inter.pem > ${leaf}_chain.pem
done
