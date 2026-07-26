#!/bin/bash
set -u
M=/private/tmp/claude-501/-Users-pmes-src-github-com-pmes-UnaOS/0f1145d9-965b-49cb-80dd-53127c681422/scratchpad/mesa
B=/private/tmp/claude-501/-Users-pmes-src-github-com-pmes-UnaOS/0f1145d9-965b-49cb-80dd-53127c681422/scratchpad/v3dbuild
INC=(-I$M/src -I$M/include -I$M/src/compiler -I$M/src/compiler/nir -I$M/src/broadcom -I$M/src/util -I$B/gen -I$B/gen/nir -I$B/gen/compiler -I$B/gen/util/format -I$B/gen/broadcom
     -I$M/src/gallium/auxiliary -I$M/src/gallium/include)
CFLAGS=(-w -O1 -std=gnu11 -DNDEBUG -DHAVE_PTHREAD -DHAVE_STRUCT_TIMESPEC -DHAVE_TIMESPEC_GET "${INC[@]}")
mkdir -p $B/obj
: > $B/errs.log; : > $B/objs.list
compile_one() {
  local src="$1" tag="$2"
  local o="$B/obj/${tag}_$(basename ${src%.c}).o"
  if cc "${CFLAGS[@]}" -c "$src" -o "$o" 2>>"$B/errs.log"; then
    echo "$o" >> $B/objs.list
  else
    echo "FAIL: $src" >> $B/errs.log
  fi
}
for f in $M/src/compiler/nir/*.c;       do compile_one "$f" nir; done
for f in $B/gen/nir/*.c;                do compile_one "$f" gennir; done
for f in $M/src/broadcom/compiler/*.c;  do compile_one "$f" comp; done
for f in $M/src/broadcom/qpu/*.c;       do compile_one "$f" qpu; done
compile_one $M/src/broadcom/common/v3d_device_info.c common
compile_one $M/src/broadcom/common/v3d_debug.c common
compile_one $M/src/compiler/glsl_types.c cc
compile_one $M/src/compiler/shader_enums.c cc
compile_one $B/gen/compiler/builtin_types.c cc
# extra util sources appended by caller via $EXTRA (space list)
for f in ${EXTRA:-}; do compile_one "$f" util; done
compile_one $B/harness.c harness
echo "=== objs: $(wc -l < $B/objs.list) ==="
echo "=== FAILs ==="; grep "^FAIL:" $B/errs.log
echo "=== first errors ==="; grep "error:" $B/errs.log | head -20
