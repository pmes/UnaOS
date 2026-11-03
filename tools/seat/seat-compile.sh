#!/bin/bash
# the seat's fast compile of the main checkout: four explicit kernel checks (the matrix rows arroyo's `check` runs, minus the rest of its matrix)
W=/root/unaos-bench/scratch/cloud/wave36; tip=$(git -C /home/user/UnaOS log -1 --format=%h); cd /home/user/UnaOS/unaos/crates/kernel || exit 1
NF="-Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec"
leg() { name=$1; tgt=$2; feats=$3; CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=$W/target-seat cargo check --release $NF --target /home/user/UnaOS/unaos/$(basename $tgt) ${feats:+--features "$feats"} > $W/seat-$name.log 2>&1; echo "$name rc=$? tip=$tip"; grep -E "^error" $W/seat-$name.log | sort -u | head -3; }
leg x86all ../../x86_64-unaos.json "$(cat $W/kfeats-x86all.txt)"
leg x86knoboff ../../x86_64-unaos.json ""
leg orinimage ../../aarch64-unaos.json "$(cat $W/kfeats-orin-image.txt)"
leg tegraconwin ../../aarch64-unaos.json "$(cat $W/kfeats-tegra-conwin.txt)"
