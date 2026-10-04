#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# RING3WIN (rmbp-ledger B316) — the x86 ring-3 image cap is ONE number. GATE-KNOBPARITY's shape: it builds
# nothing, it reads four files and reds when they disagree:
#   1. crates/una-abi/src/lib.rs         `pub const USER_WINDOW_BYTES: u64 = 4 << 20;` (the source)
#   2. crates/kernel/.../memory.rs       `XWIN_BYTES` must be DERIVED from (1), never a literal
#   3. arroyo                            `USER_WINDOW_BYTES=<n>` (the user-ELF build gate)
#   4. builder/src/main.rs               `const USER_WINDOW_BYTES: u64 = <n>;` (the staging assert)
# and the same for the ELF-window VA (una-abi USER_BASE_X86 + USER_XWIN_OFF vs arroyo USER_XWIN_VA_X86).
# Exit 0 agree · 1 disagree · 2 parse failure (an unchecked check is never a silent pass).
set -u
W="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
ABI="$W/crates/una-abi/src/lib.rs"; MEM="$W/crates/kernel/src/arch/x86_64/memory.rs"; ARR="$W/arroyo"; BLD="$W/builder/src/main.rs"
eval_expr() { python3 -c "import sys; print(int(eval(sys.argv[1].replace('_',''))))" "$1" 2>/dev/null; }
abi_raw="$(awk -F'= ' '/^pub const USER_WINDOW_BYTES: u64 = /{sub(/;.*/,"",$2); print $2; exit}' "$ABI")"
off_raw="$(awk -F'= ' '/^pub const USER_XWIN_OFF: u64 = /{sub(/;.*/,"",$2); print $2; exit}' "$ABI")"
base_raw="$(awk -F'= ' '/^pub const USER_BASE_X86: u64 = /{sub(/;.*/,"",$2); print $2; exit}' "$ABI")"
arr_raw="$(awk -F= '/^USER_WINDOW_BYTES=/{print $2; exit}' "$ARR")"
arrva_raw="$(awk -F'[()]' '/^USER_XWIN_VA_X86=/{print $3; exit}' "$ARR")"
bld_raw="$(awk -F'= ' '/const USER_WINDOW_BYTES: u64 = /{sub(/;.*/,"",$2); print $2; exit}' "$BLD")"
derived=0; grep -q '^pub const XWIN_BYTES: usize = una_abi::USER_WINDOW_BYTES as usize;' "$MEM" && derived=1
for v in abi_raw off_raw base_raw arr_raw arrva_raw bld_raw; do
    [ -n "${!v}" ] || { echo "  ❌ window-parity: PARSE FAILURE — ${v} not found (exit 2)"; exit 2; }
done
abi="$(eval_expr "$abi_raw")"; arr="$(eval_expr "$arr_raw")"; bld="$(eval_expr "$bld_raw")"
va="$(eval_expr "($base_raw)+($off_raw)")"; arrva="$(eval_expr "$arrva_raw")"
rc=0
[ "$derived" = 1 ] || { echo "  ❌ window-parity: memory.rs XWIN_BYTES is not derived from una_abi::USER_WINDOW_BYTES"; rc=1; }
[ "$abi" = "$arr" ] && [ "$abi" = "$bld" ] || { echo "  ❌ window-parity: USER_WINDOW_BYTES una-abi=${abi} arroyo=${arr} builder=${bld}"; rc=1; }
[ "$va" = "$arrva" ] || { echo "  ❌ window-parity: ELF-window VA una-abi=${va} arroyo=${arrva}"; rc=1; }
[ "$rc" = 0 ] && echo "  ✅ window-parity: USER_WINDOW_BYTES=${abi} (una-abi = kernel-derived = arroyo = builder), xwin_va=${va}"
exit $rc
