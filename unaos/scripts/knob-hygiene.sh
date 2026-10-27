#!/usr/bin/env bash
# knob-hygiene.sh — GATE-KNOB: every `feature = "X"` in a cfg is declared, and every declared
# feature is used by one.
#
# A cfg on an UNDECLARED feature is always false. It does not fail to build and it does not fail
# `check` — rustc emits `unexpected cfg condition value` and the gate throws the warning away. So the
# code under it is dead while READING as live, on every board, and the `not` arm is taken
# unconditionally. pi 6 found the first instance (`pidesk`, 7 sites in main.rs and video/menubar.rs
# on hw-pi4 — arch-neutral files, so it reaches x86 the moment that lands).
#
# ⚠ COMMENTS ARE STRIPPED FIRST, AND THAT IS THE WHOLE DIFFICULTY.
# The naive form of this check — grep the feature names out of the sources and set-difference them
# against [features] — was proposed as "pure set-difference, cannot false-positive". It can. Run it
# on hw-rmbp and it reports `pidesk` PHANTOM, on the strength of a DOC COMMENT at
# video/menubar.rs:77 that merely quotes the cfg expression in prose. A gate that reds on a sentence
# teaches people to disable it. Hence the `sed 's@//.*@@'` below, and hence the false-positive
# fixture in the go-red proof: prose mentioning a feature must stay GREEN.
#
# usage: knob-hygiene.sh
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO="$HERE/../crates/kernel/Cargo.toml"
SRC="$HERE/../crates/kernel/src"

# `default` is Cargo's own and is correct while empty: it is declared by the manifest format, not by
# anyone intending a knob, so it can never have a cfg site.
DEAD_OK="default"

declared="$(awk '/^\[features\]/{f=1;next} /^\[/{f=0} f && /^[A-Za-z0-9_-]+[ \t]*=/{sub(/[ \t]*=.*/,"");print}' "$CARGO" | sort -u)"
# GATEREVIEW F10: lines are JOINED before the match — a `feature =` / `"name"` split over two lines was invisible
# (a phantom so written passed); the extraction is probed on that shape below, and `--selftest` stops after the probes.
extract() { sed -E 's@//.*@@' | tr '\n' ' ' | grep -oE 'feature[[:space:]]*=[[:space:]]*"[A-Za-z0-9_-]+"' | sed -E 's/.*"([^"]+)"/\1/' | sort -u; }
used="$(find "$SRC" -name '*.rs' -exec cat {} + | extract)"
[ "$(printf '#[cfg(all(x,\n    feature =\n    "zzsplit"))]\n// feature = "zzcomment"\n' | extract)" = zzsplit ] || { echo "GATE-KNOB: control FAILED — a feature split over lines was not extracted (or a comment was). No verdict." >&2; exit 2; }

# CONTROL PROBE, the same idea the knob->leg check uses with `vugpar`, and for the same reason: a
# regex that matched NOTHING would report "0 phantoms" and read as a clean tree. A zero has to be
# distinguishable from a broken pattern, so both sides must contain a feature that certainly exists.
for _probe in wc witness; do
  printf '%s\n' "$declared" | grep -qx "$_probe" || { echo "GATE-KNOB: control FAILED — '$_probe' not parsed out of [features]; the manifest parser is broken. No verdict." >&2; exit 2; }
  printf '%s\n' "$used"     | grep -qx "$_probe" || { echo "GATE-KNOB: control FAILED — '$_probe' not found in any cfg; the source scan is broken. No verdict." >&2; exit 2; }
done

[ "${1:-}" = --selftest ] && { echo "GATE-KNOB selftest: controls ok (manifest parse, cfg scan, split-line extract)"; exit 0; }
phantom="$(comm -13 <(printf '%s\n' "$declared") <(printf '%s\n' "$used"))"
dead="$(comm -23 <(printf '%s\n' "$declared") <(printf '%s\n' "$used") | grep -vxF "$DEAD_OK" || true)"

rc=0
if [ -n "$phantom" ]; then
  rc=1
  echo "GATE-KNOB FAILED — cfg on an UNDECLARED feature (always false; the code under it is dead and reads as live):" >&2
  for f in $phantom; do
    echo "  PHANTOM: $f" >&2
    grep -rn --include='*.rs' -E "feature[[:space:]]*=[[:space:]]*\"$f\"" "$SRC" | sed -E 's@^@    @' | head -8 >&2
  done
  echo "  FIX: declare it in crates/kernel/Cargo.toml [features], or delete the cfg and keep the arm that was actually running." >&2
fi
if [ -n "$dead" ]; then
  rc=1
  echo "GATE-KNOB FAILED — feature declared but named by no cfg (a knob wired to nothing):" >&2
  for f in $dead; do echo "  DEAD: $f" >&2; done
fi

# TRAILING-COMMENT PHANTOM (orin 13, 2026-09-05, LEDGER P7): a `#[cfg(...)]` appended AFTER a line's
# trailing `//` comment is prose. It compiles nothing and `check` stays green — PRTSCR-ORIN shipped that
# way for two hours; a union merge did it again to ORINRX's census fold. Red when CODE precedes the `//`
# on the line (so a pure comment or doc-comment line that quotes a cfg expression stays GREEN — the
# prose fixture again), and `#[cfg(` sits after it.
# Shape of the hazard, not the word: CODE before the `//`, then `#[cfg(...)]`, then a STATEMENT — an
# identifier with a call and a `;` (or a block `{`) — still on the same line. A `} // end #[cfg(...)] block`
# note or a comment that merely mentions a cfg has no statement after the attribute and stays green.
#
# ⚠ THE END-OF-LINE ANCHOR WAS REMOVED (KEYDOORS F0, orin 16, 2026-09-06), AND THAT REMOVAL IS THE
# WHOLE POINT OF THIS PARAGRAPH. Until then the pattern ended `...(\(.*\).*;|\{)[[:space:]]*$`, i.e.
# the buried statement had to be the LAST thing on the line. main.rs:2948 was a fold carrying TWO
# comments: A10's prose was written at column 139, ahead of TABKEY's `#[cfg(feature = "tegra_el0")]
# if …wc_shell_focus_key(ev) { continue; }` at column ~1600, and TABKEY's own prose ran on after it.
# The buried call therefore had a second `//` behind it, the `$` did not match, and this gate — the
# one check in the tree aimed squarely at this hazard — reported OK on a LIVE regression that had
# silently disabled <TAB> on the Orin for an entire arc. Nothing else could see it either: the line
# count never moved (so the kernel8.img byte-identity proof is silent), `check` is green (the callee
# is a `pub fn` in a lib crate, so no dead-code warning fires), and `git grep` still printed the line.
# One anchor, one missed regression. Do not put it back: a buried call is buried whether or not more
# prose follows it.
#
# GO-RED, MEASURED (2026-09-06): the pre-fix `39e4b6c7:main.rs` scanned with the OLD pattern -> 0
# hits; with the pattern below -> 1 hit, main.rs:2948, the real defect. The fixed tree -> 0 hits with
# both. Reproduce: ~/unaos-bench/scratch/orin16/keydoors-fix/regex-gored.sh
#
# ONE pattern, held in a variable, used by BOTH the tree scan and the control probe — so the probe
# can never certify a pattern the scan is not actually running.
TRAILING_RE='^[^/]*[^[:space:]/][^/]*//.*#\[cfg\([^]]*\)\][[:space:]]*[A-Za-z_:][A-Za-z0-9_:]*.*(\(.*\).*;|\{)'
# CONTROL PROBE, the discipline the phantom/dead checks above already follow: a pattern that matched
# NOTHING would report "0 trailing-comment cfg" and read as a clean tree, so a zero has to be
# distinguishable from a broken regex. Two fixtures, both required, because this check has two ways
# to rot — it can stop catching the hazard, or start catching prose:
#   HAZARD fixture   must MATCH (the F0 shape: code, comment, buried cfg+call, MORE comment)
#   PROSE  fixture   must NOT match (a pure comment line quoting a cfg expression — the false
#                    positive that made the naive form of this whole gate unusable, see the top note)
_hz='    foo(); // prose #[cfg(feature = "wc")] bar::baz(ev); // and still more prose'
_pr='    // see the #[cfg(feature = "wc")] bar::baz(ev); fold above'
printf '%s\n' "$_hz" | grep -qE "$TRAILING_RE" || { echo "GATE-KNOB: control FAILED — the TRAILING-COMMENT pattern no longer matches its own hazard fixture; it would report 0 on any tree. No verdict." >&2; exit 2; }
printf '%s\n' "$_pr" | grep -vE '^[[:space:]]*//' | grep -qE "$TRAILING_RE" && { echo "GATE-KNOB: control FAILED — the TRAILING-COMMENT pattern now matches a PROSE fixture; it would red on a sentence, which teaches people to disable it. No verdict." >&2; exit 2; }
trailing="$(grep -rn --include='*.rs' -E "$TRAILING_RE" "$SRC" | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)"
if [ -n "$trailing" ]; then
  rc=1
  echo "GATE-KNOB FAILED — a #[cfg(...)] and the statement it guards sit AFTER a trailing // comment on a code line (they are PROSE: the call does not happen, and neither the line count, nor \`check\`, nor \`git grep\` can tell you so):" >&2
  printf '%s\n' "$trailing" | sed -E 's@^@  TRAILING: @' | cut -c1-160 >&2
  echo "  FIX: move the attribute AND its statement ahead of every comment on the line. The rule for folds in this tree is CODE FIRST, ALL OF IT, THEN THE COMMENTS." >&2
fi
[ "$rc" -eq 0 ] && echo "GATE-KNOB: OK — $(printf '%s\n' "$declared" | grep -c .) features declared, $(printf '%s\n' "$used" | grep -c .) named by a cfg, 0 phantom, 0 dead, 0 trailing-comment cfg"
# HOST-LINT (TOOLCHAINLINT, rmbp-ledger B1023) — the host-portability half of this gate, owed by TOOLCHAIN-ONE (B1018) and
# ARROYOMAC (B1011). `toolchain`: nothing passes `+<toolchain>` to cargo/rustc/rustup (rust-toolchain.toml is THE pin).
# `hostidiom`: the GNU-only idioms ARROYOMAC ported must not return to arroyo or scripts/*.sh; a site OFF the flight-image path
# sits on scripts/host-idiom-allow.txt (`<file>:<fixed substring> — <why>`), SHRINK-ONLY: a row that matches no hit reds.
# The discipline above: each pattern lives in ONE place used by BOTH the scan and its control; a failed control is exit 2.
# Escapes are bracket expressions (`[|]`, `[+]`, `[$]`) so BSD and GNU ERE read them alike. Bash-3.2 clean (macOS /bin/bash).
HL_ROOT="$HERE/.."
HL_ALLOW="$HERE/host-idiom-allow.txt"
HL_TC_RE='(^|[^A-Za-z0-9_./-])(cargo|rustc|rustup)[[:space:]]+[+][A-Za-z0-9$"{]|"[+](nightly|stable|beta)[-0-9]*"'
hl_code() { grep -vE '^[^:]+:[0-9]+:[[:space:]]*(#|//)' || true; }   # drop whole-line comments from `file:n:text` (prose stays green)
hl_self() { grep -v '^scripts/knob-hygiene[.]sh:' || true; }   # this gate's own fixture lines are not sites
for _f in "$HL_ROOT/arroyo" "$HL_ROOT/builder/src/main.rs" "$HL_ALLOW"; do
  [ -r "$_f" ] || { echo "HOST-LINT: control FAILED — $_f unreadable. No verdict." >&2; exit 2; }
done
for _t in 'cargo +nightly build' 'rustc +nightly-2026-10-26 -V' '    cargo +"$tc" check' 'Command::new("cargo").arg("+nightly")'; do
  printf '%s\n' "$_t" | grep -qE -- "$HL_TC_RE" || { echo "HOST-LINT: control FAILED — the toolchain pattern misses its hazard '$_t'. No verdict." >&2; exit 2; }
done
for _t in '# Fields: <cargo-feature[+cofeature]> <SPACE> <aarch64|x86_64|any>' 'cargo build --release'; do
  printf '%s\n' "$_t" | grep -qE -- "$HL_TC_RE" && { echo "HOST-LINT: control FAILED — the toolchain pattern matches the non-hazard '$_t'. No verdict." >&2; exit 2; }
done
[ -z "$(printf 'arroyo:1:    # sha256sum x\nm.rs:2:  // cargo +nightly\n' | hl_code)" ] || { echo "HOST-LINT: control FAILED — the comment filter kept a comment line. No verdict." >&2; exit 2; }
hl_fail="" hl_ntc=0 hl_nid=0 hl_nst=0 hl_allowed=0 hl_seen="" hl_used=""
hl_tc="$(cd "$HL_ROOT" && LC_ALL=C grep -HnE -- "$HL_TC_RE" arroyo builder/src/main.rs scripts/*.sh scripts/*.py 2>/dev/null | hl_code | hl_self)"
while IFS= read -r _h; do
  [ -n "$_h" ] || continue
  hl_ntc=$((hl_ntc + 1)); _r="${_h#*:}"
  hl_fail="${hl_fail}✖ HOST-LINT: ${_h%%:*}:${_r%%:*} toolchain — drop the +<toolchain>: rust-toolchain.toml is THE pin (TOOLCHAIN-ONE, B1018)"$'\n'
done <<EOF
$hl_tc
EOF
hl_rows="$(grep -nvE '^[[:space:]]*(#|$)' "$HL_ALLOW" || true)"   # `<allow line>:<file>:<substring> — <why>`
# One row per idiom: name~~ERE~~ported-if ERE (the line already carries the portable fallback)~~hazard fixture~~remedy.
while IFS= read -r _row <&3; do
  [ -n "$_row" ] || continue
  _name="${_row%%~~*}"; _r="${_row#*~~}"; _re="${_r%%~~*}"; _r="${_r#*~~}"; _ok="${_r%%~~*}"; _r="${_r#*~~}"; _hz="${_r%%~~*}"; _rem="${_r#*~~}"
  printf '%s\n' "$_hz" | grep -qE -- "$_re" || { echo "HOST-LINT: control FAILED — row '$_name' no longer matches its own hazard fixture. No verdict." >&2; exit 2; }
  if [ -n "$_ok" ] && printf '%s\n' "$_hz" | grep -qE -- "$_ok"; then echo "HOST-LINT: control FAILED — row '$_name': its ported-if clears its own hazard fixture. No verdict." >&2; exit 2; fi
  _hits="$(cd "$HL_ROOT" && LC_ALL=C grep -HnE -- "$_re" arroyo scripts/*.sh 2>/dev/null | hl_code | hl_self)"
  if [ "$_name" = stat-c ]; then   # WITNESS: arroyo's `_fsize` is a RAW hit (the scan read the file) that the ported-if clears
    printf '%s\n' "$_hits" | grep -qE '^arroyo:[0-9]+:_fsize[(][)]' || { echo "HOST-LINT: control FAILED — arroyo's _fsize line is not a raw stat-c hit; the scan is broken. No verdict." >&2; exit 2; }
    printf '%s\n' "$_hits" | grep -E -- "$_ok" | grep -qE '^arroyo:[0-9]+:_fsize[(][)]' || { echo "HOST-LINT: control FAILED — the stat-c ported-if does not clear arroyo's _fsize line. No verdict." >&2; exit 2; }
  fi
  [ -n "$_ok" ] && _hits="$(printf '%s\n' "$_hits" | LC_ALL=C grep -vE -- "$_ok" || true)"
  while IFS= read -r _h; do
    [ -n "$_h" ] || continue
    _loc="${_h%%:*}"; _r="${_h#*:}"; _ln="${_r%%:*}"; _text="${_r#*:}"
    case "$hl_seen" in *"|$_loc:$_ln|"*) continue ;; esac   # one finding per line: the first row wins
    hl_seen="$hl_seen|$_loc:$_ln|"; _allow=""
    while IFS= read -r _ar; do
      [ -n "$_ar" ] || continue
      _an="${_ar%%:*}"; _a="${_ar#*:}"; _af="${_a%%:*}"; _ap="${_a#*:}"; _ap="${_ap%% — *}"
      [ "$_af" = "$_loc" ] || continue
      case "$_text" in *"$_ap"*) _allow=1; hl_used="$hl_used|$_an|"; break ;; esac
    done <<EOF
$hl_rows
EOF
    if [ -n "$_allow" ]; then hl_allowed=$((hl_allowed + 1))
    else hl_nid=$((hl_nid + 1)); hl_fail="${hl_fail}✖ HOST-LINT: $_loc:$_ln $_name — $_rem"$'\n'; fi
  done <<EOF
$_hits
EOF
done 3<<'TABLE'
stat-c~~(^|[^A-Za-z0-9_-])stat[[:space:]]+-c~~stat[[:space:]]+-c[^|]*[|][|][^|]*stat[[:space:]]+-f~~mtime=$(stat -f %m "$d" 2>/dev/null || stat -c %Y "$d")~~_fsize (GNU `stat -c` FIRST, BSD `stat -f` second: GNU -f is the filesystem mode and exits 0)
nproc~~(^|[^A-Za-z0-9_-])nproc([^A-Za-z0-9_-]|$)~~hw[.]ncpu~~make -s -j"$(nproc)"~~_ncpu
sha256sum~~(^|[^A-Za-z0-9_-])sha256sum([^A-Za-z0-9_-]|$)~~shasum|command -v sha256sum~~sum=$(sha256sum "$f" | cut -d' ' -f1)~~sha256_of
sha512sum~~(^|[^A-Za-z0-9_-])sha512sum([^A-Za-z0-9_-]|$)~~shasum|command -v sha512sum~~sum=$(sha512sum "$f" | cut -d' ' -f1)~~sha512_of
date-d~~(^|[^A-Za-z0-9_-])date[[:space:]]+(-[A-Za-z]+[[:space:]]+)*(-d|--date)~~~~t=$(date -d "$s" +%s)~~compare YYYY-MM-DD as strings (_rustc_floor's way)
sed-i~~(^|[^A-Za-z0-9_-])sed[[:space:]]+(-[A-Za-z]+[[:space:]]+)*-[A-Za-z]*i([[:space:]]|$)~~~~sed -i 's/a/b/' "$f"~~sed -i.orig (suffix attached: GNU and BSD both) or a temp file + mv
readlink-f~~(^|[^A-Za-z0-9_-])readlink[[:space:]]+(-[A-Za-z]+[[:space:]]+)*-[A-Za-z]*f~~~~d=$(readlink -f "$0")~~cd "$(dirname "$f")" && pwd
grep-P~~(^|[^A-Za-z0-9_-])grep[[:space:]]+(-[A-Za-z]+[[:space:]]+)*(-[A-Za-z]*P|--perl-regexp)~~~~n=$(grep -oP '\d+' "$f")~~grep -E
timeout~~(^|[;&|({`]|[[:space:]](if|then|do|else|while|until|exec|sudo|!))[[:space:]]*timeout[[:space:]]+(-[A-Za-z]+[[:space:]]+)*[0-9]~~~~    timeout 30 qemu-system-x86_64 -nographic~~a background child and kill after the wall (macOS has no timeout)
cp-reflink~~(^|[^A-Za-z0-9_-])cp[[:space:]][^|;]*--reflink~~~~cp --reflink=auto a b~~plain cp
du-b~~(^|[^A-Za-z0-9_-])du[[:space:]]+(-[A-Za-z]+[[:space:]]+)*(-[A-Za-z]*b|--bytes|--apparent-size)~~~~n=$(du -b "$f" | cut -f1)~~_fsize
bash4-case~~[$][{][A-Za-z_][A-Za-z0-9_]*([[][^]]*[]])?[,^]~~~~d="target/${tag,,}"~~tr '[:upper:]' '[:lower:]' (ARROYO-BASH32: macOS /bin/bash is 3.2)
truncate~~(^|[^A-Za-z0-9_.-])truncate[[:space:]]+-s~~~~truncate -s 64M "$img"~~dd if=/dev/zero of=<f> bs=1 count=0 seek=<n> (prepare_aarch64's way)
TABLE
while IFS= read -r _ar; do
  [ -n "$_ar" ] || continue
  case "$hl_used" in *"|${_ar%%:*}|"*) ;; *) hl_nst=$((hl_nst + 1)); hl_fail="${hl_fail}✖ HOST-LINT: scripts/host-idiom-allow.txt:${_ar%%:*} stale — ${_ar#*:} matches no hit (shrink-only: delete the row)"$'\n' ;; esac
done <<EOF
$hl_rows
EOF
if [ -n "$hl_fail" ]; then
  rc=1; printf '%s' "$hl_fail" >&2
  echo "HOST-LINT FAILED — toolchain=$hl_ntc hostidiom=$hl_nid stale=$hl_nst allowed=$hl_allowed" >&2
else
  echo "✔ HOST-LINT: toolchain=0 hostidiom=0 allowed=$hl_allowed"
fi
exit $rc
