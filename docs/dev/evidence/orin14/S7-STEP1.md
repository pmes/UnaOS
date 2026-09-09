# S7 step 1 — the Pi render member behind `RenderWait` (patch + byte-identity proof)

Seat: orin 14, executor S7STEP1. Tree: `hw-jetson` at `2a04fb4a` (TIP), worktree branch
`worktree-agent-a02292887f3387e5d`. Design: [`S7-CONVERGENCE.md`](S7-CONVERGENCE.md) §3–§4 step 1.
Ledger: `docs/dev/LEDGER.md` S7 (the seat ticks it with the grant; this document does not).

**Lane.** The edit is in `unaos/crates/kernel/src/main.rs` outside the tegra region (the Pi member,
`#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]`, plus the file tail). Both need the rmbp
grant (shared kernel core) and a pi ack (the knob-off byte-identity baseline moves — §3). Neither is in
hand, so the code is delivered as [`S7-STEP1.patch`](S7-STEP1.patch) (a `git format-patch` of the code
commit on the private worktree branch) and NOT merged anywhere. No `video/` file is touched; `arroyo`,
`Cargo.toml` and the ledgers are untouched.

## 1. What the patch does

One body, `render_pass<W: RenderWait + InputOwner + Furniture>(cpu: usize) -> !`, converted IN PLACE
from the Pi member (`main.rs:5273-5693`, 421 lines, same 421 lines after); `Wake`, the three traits,
`ChannelWait` and the `render_service` shim appended at the FILE TAIL (`main.rs:8627-8790`, +164
lines). Everything added carries the Pi member's own gate, `cfg(all(target_arch = "aarch64", feature =
"baremetal"))`.

| design item (§3.2) | landed as | lines |
|---|---|---|
| 1. wait + match | `let wake = w.wait();` (`:5370`); `match wake { Wake::Input(ev) if W::OWNS_INPUT => match ev { …the Pi arms verbatim… _ => {} }, Wake::Tick => strip_tick = true, Wake::Retire \| Wake::Input(_) => {} }` (`:5395`, `:5464-5470`). The Pi's `Event::Timer` arm is gone; `ChannelWait::wait` maps Timer → `Wake::Tick`. | 5370, 5395, 5464-5470 |
| 2. serial-inbox drain | `if W::OWNS_INPUT { SERIAL_WAKE_PENDING.store(…); … } }` — opened on `:5497`, closed by a folded `} }` on `:5514` | 5497, 5514 |
| 3. furniture | `if W::STRIP && strip_dirty … else if W::STRIP && strip_tick` (`:5525`, `:5528`); `#[cfg(feature = "desktop_firmware")] { if W::PULSEWIN { pulsewin::service(); } }` folded on `:5532` | 5525, 5528, 5532 |
| 4. census | `w.census(dirty);` (`:5663`); the 27 lines of the inline `[sched6]` block become prose (the block is `ChannelWait::census`, verbatim) | 5637, 5663-5691 |
| construction | `let mut w = W::new(cpu);` on `:5366`, exactly where `s6_last_ms = arch::ms()` stood | 5360-5366 |

`ChannelWait { t0, passes, composites, cyc, last_ms }` — the pre-S7 `s6_*` locals and `t0`, one for one.
`wait` = `GUI_CHANNEL.recv()` → `GUI_RECV.fetch_add` → `t0 = now_cycles()` → `passes += 1` → map; `census`
= `composites += presented` → `cyc += now − t0` → the 5 s `[sched6]` line + `prio_witness()` → reset.
`InputOwner::OWNS_INPUT = true`; `Furniture::{STRIP = true, PULSEWIN = cfg!(desktop_firmware),
DOCK_REOPEN = INSTGUI = STACK_PROBE = false}`. The shim: `fn render_service(arg: usize) {
render_pass::<ChannelWait>(arg) }` — name, cfg and the spawn site `:1441-1447` unchanged.

Expected wire: identical (`[sched6]`, `[shellwin-pi]`, `[serfocus]` unchanged in text and cadence).

### Choices where the design was under-specified

1. **`render_pass` constructs `W` (`W::new(cpu)`) and is `#[inline(never)]`**, instead of the
   illustrative `fn render_pass<W>(w: &mut W, cpu)`. Reason (codegen, not taste): the census
   accumulators were register-promoted locals; a `&mut` to a struct in the shim's frame is a memory
   object, and without `inline(never)` LLVM folds a single-caller internal function into its caller
   (the body would move to the shim's slot). `cpu` is the spawn argument (0 today), forwarded by the
   shim; the x86 impl (step 3) keeps its `cpu` in its own state, as §3.1 already shows.
2. **The trait block is gated on the Pi member's cfg, not the §3.4 union.** At step 1 the body still
   names `baremetal`-only items directly (`GUI_RECV`, `SERIAL_WAKE_PENDING`, `shell_inbox`,
   `serfocus_witness`, `click1_dispatch`); a union gate would be a red check on every non-Pi leg. Step
   2 widens the gate together with the cfg-erasure of those statements (§3.2 item 2 already says the
   drain is "cfg-erased exactly as today" on the Orin — that cfg is step 2's edit).
3. **`try_next` is declared, not called.** The Pi drains one event per pass; the burst loop is the x86
   fold (step 3). `#[allow(dead_code)]` on `try_next`, `Wake::Retire` and the three step-2/3 furniture
   consts, nowhere else.
4. **Prose reclaims lines.** The 27 census lines and the 3 `s6_*` declarations are replaced by comment
   lines (PARITY §5.3: "fit new prose into the line count already there"); the prose is the S7 rationale
   and the step-2/3 pointers, not filler.

## 2. GATE-FAMILY count

Counted as the design counts it (§4: distinct pass-loop BODIES; the assertion it names):

```
$ grep -c 'fn render_service\|fn x86_render_service\|fn orin_render_service' unaos/crates/kernel/src/main.rs
3        # before (2a04fb4a) and after — unchanged
$ grep -c 'fn render_pass' unaos/crates/kernel/src/main.rs
1        # after (0 before)
```

Step 1 does not change the count: **3 → 3**. Step 2 (Orin: `CounterPollWait` + shim replacing
`orin_render_service`'s 138-line body) makes it **2**; step 3 (x86) makes it **1** and strikes the entry.

## 3. Byte identity — measured

Baseline built at TIP `2a04fb4a` in this worktree; "after" built at the code commit. Same toolchain,
same tree path, same `K8_FEATS` (`baremetal,skip_xhci`), same x86 feature banner
(`ehcihid,kbdwit,sdhcblk,smolnet,wc`).

| image | command | before (TIP) | after (step 1) | verdict |
|---|---|---|---|---|
| Pi knob-off `target/pi_baremetal/kernel8.img` | `cd unaos && ./arroyo kernel8` | `d73a8981d65bd24e254567934f0f2d21b3307b4a761408618d576623e2669fb0` (1,254,984 B) | `ade3e3ed9306a85ffdb4f3361c8a26f26518f3cdf3b442bc5a8a6364eff34a46` (1,254,984 B) | **MOVES** — design §4 outcome (b), cause stated before the build; every byte accounted below |
| x86 `target/x86_64-unaos/release/unaos-kernel` | `cd unaos && UNAOS_WC=1 ./arroyo build` | `ef01d942e617a3507edbd8031a130534242ed4d0970b568ca73fe908dab89b30` | `ef01d942e617a3507edbd8031a130534242ed4d0970b568ca73fe908dab89b30` | **IDENTICAL** (`cmp` clean) — the line-neutrality proof: every x86 panic `Location` below the Pi region kept its line |

The cause stated BEFORE the after-build (scratch `PROGRESS.md`, 18:11): (1) a new `render_service` shim;
(2) the spawn site's function pointer now targets the shim; (3) the monomorphised body may re-codegen.
What the measurement adds: the dominant effect is (3') a **function-order permutation** — the
monomorphised generic sorts ahead of every DefIndex-ordered item of the root CGU — not the shim's 8 bytes.

### 3.1 Section headers (`llvm-objdump -h`)

Identical for every loaded section: `.text.boot 0x4c @0x80000`, `.text 0xf4564 @0x80800`, `.rodata
0x35a64 @0x174d70`, `.data 0x7e48 @0x1aa800`, `.bss 0xee04c8 @0x200000`. Only `.symtab` (+0x30, one
symbol) and `.strtab` (+0x48) differ, and neither is in the flat image (`strings -a kernel8.img | grep -c
render_service` = 0 on both).

### 3.2 Differing bytes per section (`cmp -l | wc -l` = 515,641)

| section | differing / size | class |
|---|---|---|
| `.text.boot` | 2 / 76 | one `bl`/`adrp` immediate into shifted `.text` |
| `.text` | 460,350 / 1,000,804 | the function-order permutation (§3.3) — every function behind the moved block is at a new address, so every PC-relative reference into or out of it changes; plus the body's own re-codegen (§3.4) and the shim |
| `.rodata` | 55,289 / 219,748 | the same permutation seen from data: function-pointer tables and jump tables carry `.text` addresses; the root CGU's constant blob is re-emitted in the new item order |
| `.data` | 0 / 32,328 | — |
| padding | 0 | — |

### 3.3 The permutation, function by function (`S7-STEP1-fncmp.py`)

`llvm-objdump -d` of both ELFs, split per symbol, normalised for layout facts only (the address column;
`adr`/`adrp` targets; the page-offset of every `add`/`ldr`/`str` that uses an `adrp`'d register; LLD's
`adrp+add` ↔ `nop+adr` relaxation; `.llvm.<cgu-hash>` and `anon.*` suffixes; in-function branch targets
as `SELF+off`). Call targets and every other immediate are compared verbatim.

```
$ python3 docs/dev/evidence/orin14/S7-STEP1-fncmp.py kernel-pi-before.elf kernel-pi-after.elf
functions: before=1351 after=1352 common=1351 identical(mod-reloc)=1350 differing=1
only-before: []
only-after: ['_RINvCs3PWhTdrlkm8_12unaos_kernel11render_passNtB2_11ChannelWaitEB2_']
DIFF _RNvCs3PWhTdrlkm8_12unaos_kernel14render_service: 603 -> 2 insns
```

So **1350 of 1351 functions are instruction-for-instruction identical modulo relocation**; the one that
differs is `render_service` itself, which became the 2-instruction shim. What moved (`llvm-nm -n`):
before, the root CGU's `.text` began with 14 non-local generic instantiations (`ui_status::draw<TargetPal>`
… `GneissPal::draw_text`) followed by its DefIndex-ordered items (`handle_key`, `kernel_main`, …,
`render_service` @`0x84248`, …, `__rust_boot`); after, `render_pass::<ChannelWait>` is first (@`0x80800`),
then the DefIndex-ordered items (`render_service` shim @`0x82284`), then the 14 generics. Every other
symbol keeps its relative order.

### 3.4 The body: `render_service` (0xa08 B, 642 insns) → `render_pass::<ChannelWait>` (0x9cc B, 627 insns)

Same referenced-symbol set (statics, functions, constants — set equality checked); same 26 distinct
call targets; the call MULTISET differs by exactly one `bl shell_inbox::take` — the pre-S7 code carried
the serial-drain block (`stlrb` of `SERIAL_WAKE_PENDING` + `bl take`) tail-duplicated at two block
exits, the new code emits it once (one release store per pass either way). The constant multiset differs
by `cmp #0x2; b.le` → `cmp #0x1 / #0x3` (the Event-tag dispatch tree re-shaped around the `Wake`
niche) and `cmp #0x270; b.ls` → `cmp #0x271; b.lo` (the same predicate). The rest is block order and
register assignment (`x27`/`x28` swapped, stack slots `#0x20`→`#0x18` etc.). Net −60 B; with the +8 B
shim, −52 B of code, absorbed by the 0x800-aligned pad before `__exception_vectors` (1396 → 1448 B);
that is why `.text`'s size did not move.

### 3.5 Verdict

Not byte-identical. Not a `.strtab`/symbol-name-only difference either — the code layout is permuted
and one function (the converted body) is re-lowered. **No function other than the body changed its
instruction stream**, `.data` is untouched, and the body's differences are LLVM block-layout and
register-allocation choices over the same operations, calls and constants. This is the design's outcome
(b) — a hash move for a stated cause — and it needs the pi seat's re-base of the knob-off baseline at the
fold, as CAPREVOKE's did (orin13 LANDING-REPORT `:43-44`). Line-neutrality is proven separately by the
x86 image (§3 table): a Pi-region edit that moved a line would have moved every x86 panic `Location`
below it.

Could step 1 be made byte-identical? Not with a generic body: the permutation is rustc's mono-item
order (generic instances sort before local DefIndex items within a CGU), independent of what the body
contains. A non-generic body would be identical to the pre-S7 image only by being the pre-S7 image.

## 4. Gates

| gate | command | result |
|---|---|---|
| type-check both arches | `cd unaos && ./arroyo check` | exit 0; 65 ✅ / 0 ❌ leg lines; `✅ kernel cfg coverage OK (45 legs)`; `✅ knob→leg coverage OK` |
| aarch64 virt | `./arroyo test-arm 60` | exit 0; `✅ aarch64 test complete` (serial-arm.log) |
| Pi regression, bench geometry | `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | exit 0; `✅ MBENCH PASS — 119/119 required witnesses, 0 forbidden hit(s), 20256 lines scanned` (M = REQUIRE+COUNT of `scripts/specs/pi4-regression.spec` = 119); banner `⚡ kernel features: baremetal,skip_xhci,witness`; 40 `[sched6] passes=…/s composites=…/s mean=… cyc/pass (dirty-paced strip@250ms)` lines on the wire, unchanged in shape |
| x86 with the compositor | `UNAOS_WC=1 ./arroyo test 150` | exit 0; banner `⚡ kernel features: witness,ehcihid,kbdwit,sdhcblk,smolnet,wc`; `✅ Test run complete` |

## 5. Commits (private worktree branch, unpushed; NOT on `hw-jetson`)

| # | sha | subject |
|---|---|---|
| 1 | `102304e6` (parent `2a04fb4a`) | `video/render: S7 step 1 — …` (code; = `S7-STEP1.patch`) |
| 2 | this commit | `docs/evidence: S7STEP1 — the patch and its byte-identity proof` (docs only) |

Scratch (build logs, both ELFs and images, the disassembly diffs): `~/unaos-bench/scratch/orin14/s7step1/`.
