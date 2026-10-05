#!/bin/sh
# SELFBUILD6 (B360): build the dyn probe — a musl PIE + libdyn.so + libplug.so — and libc.so / libgcc_s.so.1 relinked from
# the Rust musl target's self-contained libc.a / libunwind.a (what arroyo stages for rustc). clang + ld.lld only; no musl-gcc.
#   build.sh <musl self-contained dir> <out dir>
set -eu
SC=$1; OUT=$2; HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
# musl links its libc.so with -lgcc: the complex-multiply helpers (__mulsc3/__muldc3/__mulxc3) come from libgcc.a.
LIBGCC=${LIBGCC:-$(gcc -print-libgcc-file-name)}
ld.lld -shared --eh-frame-hdr -z now --whole-archive "$SC/libc.a" --no-whole-archive "$LIBGCC" -soname libc.so -o "$OUT/libc.so"
clang --target=x86_64-unknown-linux-musl -nostdinc -ffreestanding -O1 -fPIC -c "$HERE/gcc_helpers.c" -o "$OUT/gcc_helpers.o"
# libgcc_s.so.1 = LLVM libunwind + the one libgcc helper the musl rust-lld imports (__popcountdi2@GCC_3.4).
ld.lld -shared --eh-frame-hdr -z now --whole-archive "$SC/libunwind.a" --no-whole-archive "$OUT/gcc_helpers.o" -soname libgcc_s.so.1 "$OUT/libc.so" -o "$OUT/libgcc_s.so.1"
# rustc links proc-macros with -lgcc_s: the link-time name.
ln -sf libgcc_s.so.1 "$OUT/libgcc_s.so"
CF="--target=x86_64-unknown-linux-musl -nostdinc -ffreestanding -fno-builtin -O1 -c"
clang $CF -fPIC "$HERE/libdyn.c" -o "$OUT/libdyn.o"
clang $CF -fPIC "$HERE/libplug.c" -o "$OUT/libplug.o"
clang $CF -fPIE "$HERE/hello.c" -o "$OUT/hello.o"
ld.lld -shared --eh-frame-hdr -soname libdyn.so -o "$OUT/libdyn.so" "$SC/crti.o" "$SC/crtbeginS.o" "$OUT/libdyn.o" "$OUT/libc.so" "$SC/crtendS.o" "$SC/crtn.o"
ld.lld -shared --eh-frame-hdr -soname libplug.so -o "$OUT/libplug.so" "$SC/crti.o" "$SC/crtbeginS.o" "$OUT/libplug.o" "$OUT/libdyn.so" "$OUT/libc.so" "$SC/crtendS.o" "$SC/crtn.o"
ld.lld -pie --eh-frame-hdr -z now --dynamic-linker /lib/ld-musl-x86_64.so.1 -rpath '$ORIGIN' -o "$OUT/hello" "$SC/Scrt1.o" "$SC/crti.o" "$SC/crtbeginS.o" \
    "$OUT/hello.o" "$OUT/libdyn.so" "$OUT/libc.so" "$SC/crtendS.o" "$SC/crtn.o"
rm -f "$OUT"/*.o
