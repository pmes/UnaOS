#!/bin/sh
# CTCORE (SR60) oracle: build OpenSSL 3.5 (ML-KEM, X25519MLKEM768) for tests/m11_hybrid.rs — the host's OpenSSL is
# 3.0.13. Static `apps/openssl` only (no docs/tests/shared libs, ~3 min on 3 cores, ~400 MB while building).
#   sh build_openssl35.sh [DEST]      DEST defaults to <repo>/target/openssl35 (gitignored, removed with target/)
# then run the test with OPENSSL35=<DEST>/apps/openssl (or leave DEST at the default, which the test finds).
set -e
TAG=openssl-3.5.9
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="${1:-$HERE/../../../../../../target/openssl35}"
if [ ! -d "$DEST/.git" ]; then git clone -q --depth 1 --branch "$TAG" https://github.com/openssl/openssl.git "$DEST"; fi
cd "$DEST"
[ -f Makefile ] || ./Configure no-docs no-tests no-shared >/dev/null
make -j"$(nproc)" build_programs >/dev/null
./apps/openssl version
