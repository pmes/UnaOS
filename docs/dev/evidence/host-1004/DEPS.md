# DEPS — the R83 dependency audit (SR31)

Branch `exec-host-deps`, cut from `676ca0e9`. Ledger row SR31 (status as found: "open — DEPS on branch
exec-host-deps"). Rulings: R83 (amends LAWS §Dependencies). Generated table: [`docs/dev/DEPS.md`](../../DEPS.md).
Tool: `tools/deps-audit/run.py` (+ `classify.toml` verdicts, `pins.toml` allowlist).

## Finding

At `676ca0e9` the tree declared 103 crates.io crates (226 declarations, 38 manifests). 34 rows were behind
crates.io's latest stable: 20 needed a manifest edit (semver-major), 14 only a `cargo update`. 45 of the 103
do the work of a capability UnaOS claims (chicken wire); 58 are utilities.

After M2 + M4: **0 rows behind** outside the allowlist (`run.py --check`, policy `any`, index refreshed
2026-10-05: 103 crates, 0 failing, rc=0), 7 pins, each with a reason and a ledger id.

## Milestones

| M | what | commits |
|---|---|---|
| M1 | `run.py` inventory → `docs/dev/DEPS.md` | `ec95944a` |
| M2 | every dependency to latest stable, one commit per crate naming from → to | 33 commits `3c101fb0..78ffde6f`, + `366adbc9` (boa 0.22 test) |
| M3 | chicken-wire table (CUT / OWED / NOT WANTED per row) in DEPS.md; seat-tip rows classified ahead; run.py pre-release fix | this commit |
| M4 | `run.py --check` gate + `pins.toml` (reason + ledger per pin; `consumers`, `until`) | `44726af9` |

### M2 — what moved

Lock-only (`cargo update -p`, one commit each): blake3 1.8.3→1.8.7, cpal 0.18.1→0.18.2, glib 0.22.7→0.22.10,
glib-sys 0.22.6→0.22.9, gobject-sys 0.22.6→0.22.9, gtk4 0.11.4→0.11.5, ignore 0.4.25→0.4.33, ringbuf
0.5.1→0.5.2, wgpu 30.0.0→30.0.1, wincode 0.6.0→0.6.2 (root lock); cfg-if 1.0.4→1.0.5, crc32fast 1.5.0→1.5.2,
lazy_static 1.5.0→1.5.1, log 0.4.33→0.4.34, spin 0.12.1→0.12.3 (unaos/ lock); log 0.4.33→0.4.34
(orin-xhci-repro lock).

Semver-major (manifest edit; callers fixed where the API broke):

| crate | from → to | consumer | caller change |
|---|---|---|---|
| dirs | 6.0.0 → 7.0.0 | principia, unafs_bench | none |
| base64 | 0.22.1 → 0.23.1 | aether | none |
| taffy | 0.12.2 → 0.14.0 | aether | `min_size`/`max_size` are `Size<LengthPercentageAuto>` (new `css::min_max` narrowing); measure closure takes `LayoutInput`, returns `LayoutOutput` |
| resvg | 0.47.0 → 0.48.1 | aether | none |
| html5ever | 0.39.0 → 0.40.1 | aether | comment only (no code names html5ever) |
| cssparser + selectors | 0.37.0 → 0.38.0, 0.39.0 → 0.41.0 | aether | `ParserInput` gone (`Parser::new(&str)`); `ParseError` lost its lifetime |
| reqwest | 0.12.28 → 0.13.5 | aether | none (native-tls/openssl subtree leaves the lock) |
| boa_engine + boa_gc | 0.21.1 → 0.22.0 | aether | `JsArray::new` returns `JsResult` (6 sites, `?`); `TimeoutJob::is_cancelled` → `cancelled` |
| gix | 0.85.0 → 0.88.0 | vaire | tree-diff callback error type is `gix_error::Exn` (turbofish dropped) |
| smoltcp | =0.13.1 → =0.14.0 | unaos-kernel (x86_64 + aarch64 blocks) | none on x86 (edit only; see kernel leg) |
| uart_16550 | 0.6.0 → 0.8.1 | unaos-kernel x86_64 | none; 0.8 `Config::default()` enables no UART interrupts (the kernel installs no COM1 RX handler), 0.7 stops gating TX on CTS |
| uefi | 0.38.0 → 0.41.0 | bootloader, orin-xhci-repro | `BlockIO::read_blocks` takes `&mut self` (bootloader LBA-0 probe) |

Transitive refreshes (`cargo update`, no downgrades): root lock 40 crates moved, 6 left the graph (the commit
subject says 38 — the count is 40); unaos/ lock 13; orin-xhci-repro 7; every `user-*` lock already current.
The bare root update proposed `crypto-common 0.1.7 → 0.1.6` (to buy `generic-array 0.14.9`); refused with
`--precise 0.1.7` — LAWS: never a downgrade.

### Pins (`tools/deps-audit/pins.toml`)

| crate | scope | ledger | why |
|---|---|---|---|
| bincode | all | SR31 | 3.0.0 is a tombstone (`compile_error!("https://xkcd.com/2347/")`); dies with arc UNAFSCODEC |
| rand_core | all (tls-spike) | SR28 | embedded-tls 0.19 (latest) needs `^0.6.3`; dies when `tls_core` replaces embedded-tls |
| cxx-qt, cxx-qt-lib, cxx-qt-build | all | SR31 | Quartzite `qt` embassy: no Qt 6 SDK in the container to prove a 0.9 → 0.10 caller move |
| gtk4, glib | `vug` only | SR31 | Vug is outside every workspace (commented out of root members, no lockfile): unbuildable as declared |

Every pin carries `until` (the newest stable it anticipated): a newer release re-opens the gate.

## Proof

- **Root host suites, base `676ca0e9` (tree-identical to M1 `ec95944a`)**: `cargo test --workspace --release
  --no-fail-fast` rc=101 — `gdk4-sys` and `graphene-sys` build scripts fail: the container has no GTK 4 /
  graphene / libadwaita SDK (`pkg-config`), pulled by `una` (its `gtk4` dependency is unconditional on Linux).
  `--exclude una`: 105 suites, **798 passed, 1 failed**: `matrix --test finder
  write_to_readonly_dir_surfaces_loud_denial` — the test runs as root, which ignores the 0o555 it sets.
- **Root host suites, tip**: `--exclude una` at `78ffde6f` (M2 done): 105 suites, 797 passed, **2 failed** —
  the same matrix root-permission test, plus ONE new red: aether
  `test_runtime_limit_in_promise_reaction_does_not_kill_the_process`, which pinned a boa 0.21 PANIC that boa
  0.22 fixed (`into_opaque` is fallible; the limit leaves the promise job as an `Err`). The test now pins the
  0.22 shape (`366adbc9`: not poisoned, the chain does not settle, script still runs; `cargo test --release -p
  aether` 84/84). Then the exact brief command `cargo test --workspace --release --no-fail-fast` at
  `366adbc9`: **106 suites, 803 passed, 1 failed** (matrix root-permission, pre-existing), rc=101 for that one
  test only. `una` built this time: a GTK 4.14.5 / graphene 1.10.8 SDK was installed in the shared container
  between the base run and the tip run, so the base's `gdk4-sys`/`graphene-sys` red was environmental and
  the base-vs-tip comparison is the `--exclude una` pair (798/1 vs 798/1 after the test fix).
- **Kernel leg** (once, at the end of M2, from `unaos/crates/kernel`): `cargo +nightly check --release
  --target ../../x86_64-unaos.json -Z build-std=core,compiler_builtins,alloc
  -Z build-std-features=compiler-builtins-mem -Z json-target-spec --features
  selfhost,ehcihid,kbdwit,smc,sdw,sdhcblk,login,loginst,installdemo,smolnet,wc,nvidia-kepler-vblank,instgui,linuxabi,busreg,beam,quarry,facet,sdwrite,ftdirx,usbnet,ahci,ahci-write,root-prefer-ahci,unafs,hda,hda-tone`
  → **rc=0** (`smoltcp v0.14.0` and `uart_16550 v0.8.1` compiled under `smolnet`; 75 warnings, none from the
  bumped crates' call sites). Target deleted.
- **Bootloader / repro** (not the kernel leg, cheap, because uefi broke an API): bootloader
  `x86_64-unknown-uefi` rc=0, `aarch64-unknown-uefi` rc=0; orin-xhci-repro `aarch64-unknown-uefi` rc=0.
- **Gate**: `run.py --check` rc=0 on the tip; removing the bincode pin makes it rc=1
  (`BEHIND bincode … requirement 2.0.0 does not admit 3.0.0`).

## Since the cut (seat tip `caf3c986`, scanned with this branch's run.py, not merged here)

- VEINTLS (SR36) removed `embedded-tls`, `embedded-io` and `rand_core` (tls-spike): at the fold the
  `embedded-tls` CUT row and the `rand_core` pin go stale (`--check` says so: "pin … is stale").
- TLSCORE added nine RustCrypto crates (`hmac`, `aes-gcm`, `chacha20poly1305`, `x25519-dalek`, `p256`,
  `p384`, `ed25519-dalek`, `rsa 0.10.0-rc.18`, `getrandom`) plus `sha2`, all optional behind tls_core's
  `test-provider` — a test oracle never linked into a product; classified here in advance as NOT WANTED
  (oracle), the product primitive being `crypto_core` (SR27). `rsa`'s pre-release requirement exposed a
  run.py false positive (a requirement AHEAD of latest stable counted as behind); fixed in M3.
- FACETPIXEL (SR43) and VP8CORE (SR40) moved Facet's decoding and Aether's WebP onto `pixel_core`, but
  `handlers/aether` still declares `image = "0.25"` at `caf3c986`, so the `image` row stays CUT/open.
- EYES' Aether bumps (`base64 0.23`, `resvg` optional; `d6c205d1` on exec-host-merge1) are not on the seat
  tip; at `caf3c986` the seat is still 25 rows behind (aether's boa/cssparser/selectors/html5ever/reqwest/
  resvg/taffy, smoltcp, uart_16550, uefi, gix, dirs, the lock-only rows) — exactly what this branch's M2
  moves. Expect a merge conflict on `handlers/aether/Cargo.toml` (base64/resvg lines) and the root
  `Cargo.lock` (regenerate it with `cargo update`, then refuse the crypto-common downgrade as in M2).

## Honest ceiling

- The `--features gtk` faces of aule/matrix/midden/tabula/vaire and Quartzite were not built (the GTK SDK
  appeared mid-arc; `una` did build and test at the tip); gtk4/glib moved within 0.11/0.22 only (lock), so
  no caller change was owed there. The Qt embassy cannot be built here (no Qt 6 SDK).
- The aarch64 smoltcp users (`net4`, `vnet`, `net6`, `genet`) are not type-checked by this arc; 0.14's
  changelog shows no API break on the `Device`/`Interface`/socket surface they use, but the arm leg is owed.
- arroyo's `bootdiag,jb8lever` bootloader feature leg was not run.
- `unaos/builder/Cargo.lock` and `unaos/crates/kernel/Cargo.lock` are tracked but unused (both crates are
  members of the `unaos/` workspace, whose root lock cargo reads); left as found.

## Owed

- The 21 OWED chicken-wire rows each want a ledger row (proposed arcs: JSCORE, HTMLCORE, CSSCORE, LAYOUTCORE,
  FONTCORE, SVGCORE, HTTPCORE, NETCORE, UNAFSCODEC, GITCORE, INFERCORE; plus deleting Lumen's unused
  `wincode`). NETCORE needs Peter: ROADMAP §1b chose smoltcp (2026-07-12), R83 says built-in first.
- The cxx-qt 0.10 move on a Qt-equipped host; Vug's gtk4 0.11 move when it rejoins the workspace.
- Wire `run.py --check` into a fold gate (it needs the network or a warm `.cache/`; `--offline` reads the
  cache only).
