# 1. THE ANSWER ON ORIN SERIAL INPUT

**It is a kernel gap, not firmware and not wiring — with one honest caveat: the silicon has never been asked.** The Orin's console task `jd2_console_pump` (`unaos/crates/kernel/src/main.rs:2801`) drains only `pal::next_event()` at `main.rs:2855` (phase 1) and `main.rs:2935` (phase 2), and `next_event` is `pop_event()` alone (`pal.rs:1983-1985`) — it issues no MMIO — so the only `EVENT_QUEUE` producers reachable from the shell prompt are the xHCI HID decodes (`drivers/xhci/mod.rs:4765/4872/4914); meanwhile `tegra::read_byte` (`arch/aarch64/serial.rs:81-101`) is compiled into every jetson image and reachable through `arch::poll_input` (`arch/aarch64/mod.rs:292-294` → `serial.rs:357-370` → `serial.rs:284-286`), but its one surviving live call site on tegra is `pal.rs:2041-2044` inside `pump_and_poll`, whose only callers are `vug.rs:377/481` and the selftest pager `selftest.rs:381` — none of which the console pump ever enters. There is no missing register write to find: a 16550 has no RX-enable bit, `LSR.DR` + `RBR` is the whole polled-RX contract and both are already coded (`serial.rs:58-60`, `:85`, `:96`), and the file's single MMIO write in its entire life is `write_volatile(thr, …)` at `serial.rs:77`.

**The exact change**, appended to existing statements so the knob-off jetson image stays byte-identical (the panic-`Location` rule asserted at `main.rs:2717`):

```rust
while let Some(b) = unaos_kernel::arch::poll_input() { unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(b)); }
```

folded onto **four** closing braces, not two: `main.rs:2854` (phase-1 xHCI poll block), `main.rs:2915` (phase-2), `main.rs:7602` (`jd2_supstate_phase2` — without it the patch is inert on `supstate` images, `main.rs:7565`), and `xusb_tegra.rs:1937` (`kbd_pump_body`, the headless serial-only boot). Everything downstream is already source-agnostic: `handle_key` (`main.rs:2726`), the `:: tegra: JD2 — KEY …` serial echo (`main.rs:2940-2944`), `shell::dispatch_command` (`main.rs:2741` → `shell.rs:2865`).

**The caveat that must ride with the answer:** the driver's own header marks `BASE = 0x0C28_0000` "*** TO VERIFY ON THE BOARD ***" and notes UARTC is an AON-cluster port normally owned by the SPE for the TCU (`serial.rs:28-48`). Observed TX does not establish RX. So the change is correct and necessary, but it is a *question* until a board answers it — and `read_byte` swallows the diagnostic (`serial.rs:92-94` returns `None` on all-ones), so the patch must also print the raw LSR word once, or a negative result is undiagnosable.

# 2. THE CYCLE TODAY VS AFTER

### Today, one iteration (counted from `~/unaos-bench/scratch/orin11/load-card.sh` and the staged tree)

1. `UNAOS_TEGRA=1 … ./arroyo esp-jetson` → stages **10 files, 14,725,064 B** (measured over `flash/orin/render1-20260901T0347Z-c61b47e/`; `SRC.TGZ` alone is 11,952,884 B of it).
2. Write the staged `MANIFEST`, record the write in `flash/orin/MANIFEST`.
3. **Physically pull the card out of the Orin**, seat it in the host reader.
4. `udisksctl mount -b /dev/mmcblk0p1` (label `UNAOS-ORIN`).
5. Harvest the previous contents: `cp -R $MP/. $HARVEST/` — ~14.7 MB read + write.
6. `cp -R $SRC/. $MP/` + `sync` — another ~14.7 MB.
7. `sha256sum` all 10 on-card files against the staged MANIFEST (`load-card.sh:52-58`) — including the 11.9 MB `SRC.TGZ`, every time.
8. `sync`, unmount.
9. **Physically pull the card, walk it back, reseat it in the Orin.**
10. **Hard power cycle** (barrel jack), wait for boot; host side: start `line-butler.py`, set the capture mark, arm the waker.

Ten steps, **two physical card handlings, ~30 MB of host I/O, one hard power cycle** — to change a kernel by a few hundred bytes.

### Serial ceiling, and it cannot be raised

115200 8N1 = **11,520 B/s**, and the baud is not a constant we may edit: the driver deliberately does not reprogram the divisor because the UART clock is BPMP-managed (`serial.rs:41-44`). Raising it is BPMP work, not a one-line change.

| payload | bytes | raw | base64 (×4/3) | verdict |
|---|---|---|---|---|
| a shell command line (~40 B) | 40 | 3 ms | — | **send it** |
| `STAT.ELF` (an EL0 test binary) | 8,472 | 0.7 s | 1.0 s | **send it** |
| a 64 KB fixture / test vector | 65,536 | 5.7 s | 7.6 s | **send it** |
| `gzip -9 kernel.elf` (measured) | **906,368** | 79 s | **105 s** | **send it** — `selfhost/inflate.rs` already decodes gzip streaming, `ByteSource`-driven (`inflate.rs:75-83`) |
| `kernel.elf` (render1) | 2,648,624 | 230 s | 307 s | borderline |
| selfup matched-pair pak (`orin-selfupdate.md` §3.1) | 2,703,283 | 235 s | 313 s | borderline; gzip'd ≈ 940 KB ≈ **110 s** |
| full staged media, 10 files | 14,725,064 | **21 min** | 28 min | **never** |
| `SRC.TGZ` alone | 11,952,884 | **17 min** | — | **never** |

**Honest read:** shipping a raw 2.6 MB `kernel.elf` over the wire (~4 min) is roughly a *wall-clock wash* with the card ritual. It wins anyway, because it is unattended and touches no physical object — but do not sell it on speed. What is decisively worth uploading is (a) **commands**, (b) **small binaries and fixtures**, (c) **a gzip'd matched pair**, using the decompressor already in the tree. What is never worth uploading is `SRC.TGZ` or the media furniture — which is exactly why `selfup`'s payload was already narrowed to the matched pair after the whole-tree pak looped the board forever (`orin-selfupdate.md` §3.1).

### After A1 + A2 (serial RX + `reboot`), for a **non-kernel** iteration

```
printf 'tste\r' > $BUTLER_FIFO      # line-butler.py:372 already writes to the port
awk '/PASS|FAIL/' capture/line-acm0/orin.log
printf 'reboot\r' > $BUTLER_FIFO    # back to a clean state
```
**Zero card handlings, zero power cycles**, latency measured in seconds. Everything that today costs a full card ritual because the only way to type is a USB keyboard at the bench — running a different command, re-running `tste`, exercising the FAT write verbs, capturing a different witness, driving a knob-armed image through a second scenario — collapses to a `printf`.

### After A5, for a **kernel** iteration

`./arroyo esp-jetson` → `./arroyo selfup-pak` → upload (~2 min gzip'd) → `reboot`. The card ritual disappears for kernel edits too.

**Be clear about the split: RX + reboot does *not* remove the card write for kernel changes.** It removes it for everything else, and removes the power cycle in every case where the board is not wedged.

# 3. THE ARC, ORDERED

No QEMU gate can score any tegra *behaviour* — QEMU models no Tegra234 and every arroyo tegra leg is type-check only (`flash/orin/render1-…/MANIFEST`: "UNFLOWN — QEMU models no Tegra234, so this boot is the only verdict that exists"). What a gate *can* score, on every step below: `./arroyo check` both arches; a dedicated `arm-tegra-*` cfg leg compiling the **armed** polarity (a knob without a leg is a coverage hole — `arroyo:3951`); a **go-red proof** against a named fixture mutation; and an in-artifact `grep -a -o -F` census on `kernel.elf` proving the witness strings are actually in the image (never `strings`).

---

### A1 — ORINRX: wire the drain, and get both verdicts on one boot ⚑ **METAL BOOT REQUIRED**
*One session + one attended flight.*

**Touches:** `main.rs:2854`, `main.rs:2915`, `main.rs:7602`, `xusb_tegra.rs:1937` — the four-site fold from §1, all appended to existing statements. Plus, in this seat's own lane, a raw one-shot probe in `arch/aarch64/serial.rs` that prints the LSR word before `read_byte`'s all-ones guard swallows it.

**Knob it.** `orinrx`, default OFF, for the verdict flight: the risk if `BASE` is wrong is phantom `0xFF` bytes injected into the shell on every poll — `serial.rs:88-94` warns of exactly this — and a knob keeps the shipped default image byte-identical to baseline with no argument. Promote to default-on the moment `RX-LIVE` is on the wire.

**Wire witness:** the echo already exists and already lands on serial — `:: tegra: JD2 — KEY 'x' ::` (`main.rs:2940-2944`). Inject a byte through the butler FIFO, and that line is the verdict. Add the discriminator the baton asked for: `[orinrx] lsr=0x… -> RX-LIVE | RX-ZERO | RX-DEAD | RX-OPENBUS`, because `read_byte` cannot distinguish "AON UART held in reset / clock-gated" (`LSR = 0x00`) from "decode/access-error fill" (`0xDEAD….`) from "open bus" (`0xFFFF_FFFF`) — and those three point at three different follow-ons (BPMP clock/reset ungate; wrong BASE; fallback UART — `serial.rs:36-48`).

**Ride `reboot` on the same boot, last.** It costs **zero lines**: `"reboot"` is an ungated arm at `shell.rs:4084-4087` inside `dispatch_command`, registered `Avail::Always` in the verb table (`midden_core/src/lib.rs:292`), reaching `power::reboot()` (`power.rs:38`) → PSCI `SYSTEM_RESET` via `smc #0` (`power.rs:75/94`). It has **never been fired on any board** — `[orinreboot] reboot verb invoked` has 0 hits across every capture — while its sibling `shutdown` is proven live (`orin.log:25793-25794`, SMC took the machine, no RETURNED line). Two questions, one flight, because the flight is the scarce resource.

**Gate:** `./arroyo check` both arches; new `arm-tegra-rx` leg for the armed polarity + go-red proof; in-artifact census for `[orinrx]` and `RX-LIVE`. Behaviour: metal only.

**GATE-FAMILY:** the drain adds **no new name** — it calls existing `arch::poll_input` and `pal::push_event` — so the gate cannot fire. The LSR probe adds `tegra::rx_probe`, family size 1 (no `pi_`/`x86_` twin), gate silent. If a twin ever *does* look necessary, copy `power.rs`: one public name, `#[cfg]`'d bodies — identical names with cfg bodies are invisible to the gate and correctly so.

**Blocker to clear first:** `main.rs` is shared kernel-core. This seat has precedent folding tegra-gated statements there (`main.rs:2102`, `:2517`, `:2717` are all orin knobs), but get the grant from the rmbp seat recorded over ccd anyway — it is one message and it is cheap.

**Free probe worth trying at the top of that same flight, zero code, on the card already loaded:** `tste` is an ungated shell arm (`shell.rs:3929`) over an ungated module (`lib.rs:70`), and its pager's `pause` calls `pal::pump_and_poll()` (`selftest.rs:381`) — which on tegra *does* run the RX drain. If the pager pauses, a byte sent down the wire advances it. **Do not plan around it:** the pause fires only if the table exceeds `Console::page_rows` (`console.rs:170-180`) — ~60-70 rows at 1920×1200 — the table may not reach that, and `-- more (any key) --` is a *console* line, panel-visible only, not on the wire. It costs nothing to try; it proves nothing if it doesn't fire.

---

### A2 — the host cycle tool. *One session, no kernel code, no metal boot.*
**Touches:** `~/unaos-bench/tools/` only — outside the repo, so no lane and no gate exposure. Wraps the three things a loop needs: inject a line through the butler FIFO (`line-butler.py:372` already does `os.write(ser, cmd)` on a raw-115200 fd), plant a capture mark before every iteration (the `orin.log` is append-only across many boots of many images — the baton's error #2 was scoring an old boot's lines as the current image's), and re-arm the waker after `reboot`. **Wire witness:** its own mark line, bracketing each iteration. Also fix `line-butler.py:19`, whose header still asserts "the Orin is output-only — that law stands", and `docs/dev/OS/08_VIDEO/PARITY.md:2528`, whose "output-only" row reasons from six Orin captures of JD2-echo == xHCI-HID counts that were all collected in a loop that issues no UART read — a structural identity, not evidence. The row should read *"UARTC RX is never polled on the Orin's console path, so no capture to date can speak to whether the port can receive."* Same action: the row's stale cites `jd2_console_pump` 2763→**2801**, `input_service` 4787→**5060**, `pal.rs:2023-2025`→**2034-2036**. PARITY.md is a shared video doc — union-reconcile at landing.

---

### A3 — ORININBOX: a real line ring on the tegra side ⚑ **metal to confirm**
*One session.* Pushing raw into `EVENT_QUEUE` is fine for the verdict flight and wrong as a permanent transport: the ring is 64 slots, 63 usable, and drops when full (`pal.rs:767`, `:790-791`), and it is **focus-routed** — a serial byte pushed there is indistinguishable from a HID key and can be handed to a focused EL0 window. `pal.rs:2017-2021` records exactly this defect on the Pi, and `shell_inbox` is the Pi's fix.

**Touches:** widen `#[cfg(feature = "baremetal")]` on `serial.rs:564` to `#[cfg(any(feature = "baremetal", feature = "tegra"))]`. The module is **pure Rust** — 512-byte ring plus four atomics over `arch::without_interrupts`, zero `read_volatile`/`write_volatile`/MMIO constants — and all three of its dependencies are already tegra-available (`without_interrupts` ungated at `arch/aarch64/mod.rs:406`; `spin` unconditional; `Semaphore` ungated). Its `baremetal` gate records where it was *written*, not what it needs. Then point the A1 drain at `shell_inbox::offer` and add a consumer to `jd2_console_pump`'s phase-2 loop. **Buys:** focus-ring immunity, 512-byte capacity, and non-silent backpressure — `ACCEPTED`/`DELIVERED`/`DROPPED`/`HIGH` (`serial.rs:581-589`), which is what LAWS' "the wire may not lose lines" demands of anything carrying evidence.

**Wire witness:** the inbox census line plus the existing `KEY` echo; `accepted - delivered - held == 0` on a healthy boot.
**Gate:** type-check both polarities. **GATE-FAMILY:** a cfg widening on an existing name is invisible to the gate *by construction* — the gate and the design agree. ⚠ The trap to name in the commit: if you write `tegra_shell_inbox_drain` beside `shell_inbox_drain`, you have created a family. One name, cfg'd bodies. Note the merge context: this branch already carries the tree's first size-3 family, `render_service` / `x86_render_service` / `orin_render_service` (`main.rs:5273/6323/8183`); GATE-FAMILY will red on it at the merge and **that is correct — do not route around it.** Its three required answers (what is shared and why is it not extracted; which axis genuinely differs; would a parameterised call to the existing member have worked) go in the commit message, and the honest answer to the third may be *yes*, in which case the output is a convergence arc, not a ledger line.
**Do not** try to port `input_service` (`main.rs:5059`): it consumes a five-symbol `baremetal` PL011 API (`serial.rs:402/408/414/426/436`) and hardcodes a "PL011 RX interrupt live" string. Interrupt-driven Tegra RX needs a 16550 IER/IIR arming path **and a UARTC SPI number that nothing in the tree names** — M-to-L, and not this arc.

---

### A4 — ORINPASTE: byte fidelity and pacing. *One session.*
Today the only host→kernel content channel is a shell line: `handle_key`'s printable filter is `c >= 32 && c <= 126` (`main.rs:2762`), and the content verbs rebuild the payload from whitespace-split tokens (`shell.rs:2943`, `write` at `:3675`, `append` at `:3418`) — so every whitespace run collapses to one space and no byte outside 0x20..0x7E survives. FAT is *not* the ceiling: `write_span` streams sector-at-a-time (`fat.rs:2747`), `write_grow` refuses only at `end > u32::MAX` (`fat.rs:3052`), `current_input` is an uncapped `String` (`console.rs:26/51`), heap is 48 MiB.

**Touches:** a decoder arm in `shell.rs` (~50 lines) reusing `fs_write`/`fs_append` (`shell.rs:534/582`, both `&[u8]`, both unconditional — no pi/tegra cfg trap). Plus the half that actually matters: **pacing**, host-side and therefore free — send a line, wait for the prompt, send the next. Without RX interrupts a byte lands every 86.8 µs and the pump does an xHCI poll per pass; `poll_input_nonblocking` degrades to `None` on a refused `try_lock` (`serial.rs:357-371`), so the RX FIFO overruns precisely when the kernel is chatty.
**Wire witness:** write a known blob, `cat` it back, compare sha on the host. **Gate:** the decoder is arch-neutral and *is* QEMU-scoreable on the Pi/x86 legs — the first thing in this arc a gate can actually score end to end. **Lane:** `shell.rs` is shared; negotiate.

---

### A5 — ORINWIRE: the wire becomes a `selfup` byte source ⚑ **METAL** — *2+ sessions, gated on A1 positive*
The payoff, and it needs almost no new mechanism. `selfup`'s own doc states the transport seam is exactly two filenames: *"a future TCP receiver's whole job is to make `UPDATE.PAK` + `UPDATE.SHA` exist in the boot-volume root"* (`orin-selfupdate.md` §3) — a serial receiver's job is identical. And the FAT write path resolves **ADMITTED** on this board against the global handle: `program_source` → `BlockHandle::Global` (`block.rs:394-403`) → `BlockSource::Default` → `default_writable()` which is the constant `true` on tegra (`block.rs:1089`, because `baremetal = ["pi","aarch64_el0"]` is structurally unsatisfiable here). Metal-corroborated on one Tegra234 coldboot: the boot volume enumerates as **63,404,032 × 512 B** and mounts FAT32 as `handle=global` (`orin.log:17000-17025`), and four create + `write_grow` + read-back-sha cycles completed on it (`:17029-17032`). **The volume the kernel can write is the volume the board boots from** — that is what makes this replace the card write rather than sit beside it.

**Run it from the shell, post-terminus — this is load-bearing.** `orinwdt`'s 300 s POR watchdog is armed at `main.rs:2102` and disarmed at `main.rs:2717` (the terminus), while `selfup_service` runs at `main.rs:2517`, *inside* the covered window. That window is exactly what killed boot7i: the watchdog fired mid-`SRC.TGZ` and re-POR'd the board into the identical prefix, indefinitely (`orin-selfupdate.md` §3.1). A ~2-5 minute shell-driven transfer inside that window would reproduce it precisely. After the terminus the watchdog is off and the transfer is safe.

**Two things to say out loud before scoping it.** (a) `orin-selfupdate.md` §2 rejected serial partly on policy — *"the serial line is dev scaffolding — LAWS forbid building anything permanent on it"* — and partly on a now-stale number ("hours-scale at 115200 for a ~15 MiB ESP"; the payload is 2,703,283 B since §3.1 narrowed it to the matched pair). The number is dead; **the policy is not.** Build it as scaffolding behind a knob, default OFF, never on the shipped default image — the same discipline every other Orin knob already follows — and update §2's table rather than quietly contradicting it. (b) The upload must **not** route through `handle_key`: the pump prints one serial line per key (`main.rs:2940-2944`), so 3.6 MB of base64 becomes 3.6 million log lines, saturating TX and destroying the very capture the boot exists to produce. A dedicated receiving mode that suppresses the echo is part of the design, not a polish item.
**GATE-FAMILY:** the receiver is a new *byte source* for an existing verifier, not a per-platform twin of anything — `parse_header`/`apply_update` are untouched. If it acquires a name that shadows a Pi or x86 equivalent, that is the signal it was designed wrong.

# 4. WHAT THIS DOES NOT SOLVE

**Still needs a card write:**
- Any change to `BOOTAA64.EFI` if the S4 flip itself is what broke — the loader is the thing that loads the kernel, and BOOTABI's matched-pair rule means a half-flipped volume is not bootable.
- **Recovery from a kernel that dies before the shell.** This is the important one. The wire is only a channel *into a running shell*; a kernel that panics ends in `hlt_loop` (`main.rs:6949`), never a reset, and there is no shell to type at. Every image that might not reach its terminus is a card-write recovery by construction — which is why A1's knob must default OFF and why the first flight of any new mechanism is attended.
- The first flight of anything, always.
- The media furniture `selfup` deliberately excludes — `SRC.TGZ`, the demo ELFs. Those arrive on the card or not at all, and `CARD-LAYOUT.txt` warns that dropping `B43/` silently changes what a WIFI-armed boot does.

**Still needs a hard power cycle (barrel jack):**
- A wedge **after** the terminus. `boot_ok_disarm` runs at `main.rs:2717`, so past that line nothing watches the board: `wdt_tegra.rs:5-7` states the cost in the tree's own words — there is no bench power-switch hardware, and a dark boot means a human pulling the jack.
- A panic anywhere (`main.rs:6937` → `hlt_loop`, `:6949`).
- PSCI `SYSTEM_RESET` returning negative — `power.rs:96-101` prints the refusal and parks in `hlt`. **Unobserved.** The sibling `shutdown` is proven to take the machine; `reboot` has never fired. Its first firing is a real experiment, and if ATF refuses, the board goes dark and the jack comes out.
- **ORIN-SMP-3-PARK**: the pre-existing ~30% RAS Uncorrectable inside the first PSCI `CPU_ON` at `main.rs:2621`. In a loop of *N* resets you meet it in ~0.3·*N* boots. An automated loop must detect it, re-fire, and **must not read it as a regression** — that conviction has already been overturned once on this bench.

**Hazards a repeated warm-reset loop introduces:**
- **Warm ≠ cold, and the whole capture corpus is cold.** PSCI `SYSTEM_RESET` is a firmware full-system reset; every existing Orin capture is a `Boot-mode : Coldboot`. Inherited firmware state is not hypothetical here: the UEFI console width — 80 columns or not, same binary, same board — is inherited firmware state, and it cost a whole session (LAWS 2026-08-31; `PLAYBOOK-orin.md` requires every capture be read through `unwrap80.sh`). **A regression measured only across warm resets is not comparable to the corpus.** Periodically cold-boot to re-anchor.
- **Faster iteration makes boot-pinning harder, not easier.** `orin.log` is append-only across many boots of many images; the loop must plant a mark and pin the loader identity line (`KELF min=… max=… pg=…`) on *every* iteration, or the tenth iteration will be scored against the third's lines. That exact error has been made here four times in one night.
- **An unquiesced reset.** `power::reboot` flushes nothing, stops no DMA, halts no controller — no such hook exists anywhere in the tree. The cost is bounded boot latency, not corruption: the xHCI driver pays its own halt + HCRST on a controller left running (`drivers/xhci/mod.rs:464-492`), and PSCI `SYSTEM_RESET` through ATF is strictly stronger than a kernel-side halt anyway.
- **No FAT data-loss hazard, and it is worth saying so to kill the reflex.** Storage is write-through: `block::write_block` issues a synchronous BOT `WRITE(10)` and checks `CswStatus::Passed` before returning (`block.rs:1175-1218`). `sync` is a no-op by design and says so on the console (`shell.rs:3530-3534`). "Type `sync` before `reboot`" flushes nothing.
- **RX loss exactly when the kernel is loud**, and the arc must not let it be silent: `poll_input_nonblocking` degrades to `None` on a refused `try_lock` (`serial.rs:357-371`), so a chatty boot drops inbound bytes with no accounting until A3's ring lands. Until then, treat every uploaded byte as unacknowledged.

# 5. THE SINGLE HIGHEST-VALUE STEP

**A1 — ORINRX: the four-site drain fold plus the raw-LSR probe, flown on one attended boot that also fires `reboot` for the first time.**

Because the other half is already free. `reboot` is wired end to end today at zero cost (`midden_core/src/lib.rs:292` → `shell.rs:4084` → `power.rs:38` → `smc #0` at `power.rs:75`), and the FAT write path is already ADMITTED on the boot volume with metal witnesses to prove it (`orin.log:17015-17032`). **Serial RX is the only missing half of "serial upload + soft reset"** — and it is missing for the cheapest possible reason: two folded statements' worth of routing, on a read that is already compiled into every jetson image.

Everything downstream is gated on it. A2 has nothing to inject. A4 has no channel to paste into. A5's byte source cannot receive. There is no other path to a remote command channel on this board: x86 serial is TX-only, the Pi's whole RX apparatus is `baremetal`-gated and `pi` + `tegra` is a hard `compile_error!` (`serial.rs:22-23`), and the Orin's only other input is a USB keyboard that requires a human at the bench.

And it compounds. Today every question that needs a *different command typed* costs a full ten-step card ritual and a power cycle, because typing is only possible at the board. After A1, that class of question costs a `printf` — which changes not just the speed of this arc but the cost of every flight the Orin seat will ever run.

The negative result is worth nearly as much as the positive one, which is the other reason to do it first: `RX-ZERO` sends the work to BPMP clock/reset deassertion, `RX-DEAD` to a wrong BASE or a CCPLEX firewall, `RX-OPENBUS` to a fallback UART (`serial.rs:36-48`). Those are three different multi-day arcs, and one boot picks between them — but only if the probe prints the LSR word, because `read_byte` swallows it (`serial.rs:92-94`). **Flying the drain without the probe is the one way to spend the boot and learn nothing.**