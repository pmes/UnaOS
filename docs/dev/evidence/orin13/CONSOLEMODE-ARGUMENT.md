# CONSOLEMODE — should the loader `SetMode` the Orin's UEFI console to its widest text mode?

Executor CONSOLEMODE, orin 13, 2026-09-05. Read-only; tree `hw-jetson@077a8fa1`. Evidence order:
the tree and the bench captures first, upstream firmware source second (labelled), UEFI-spec
reasoning third (labelled). Nothing here was QEMU-verified against the Orin; the Orin numbers come
from `~/unaos-bench/capture/line-acm0/{orin,unknown,raw}.log`.

## 0. The finding that reframes the question

**The 80-column boots are exactly the F11 "Please select boot device" boots, and nothing else.**
Every other Orin boot inherits the 240x56 mode the firmware itself switched into for its banner.
So "the firmware offers a 240-column mode nothing asks for" is not what the wire shows — the
firmware *uses* 240x56 on every boot, and the loader inherits it on every auto-boot. The 80-column
regime is the firmware's boot-manager menu (`BmSetConsoleMode(FALSE)` → mode 0 = 80x25) drawing
its popup and then launching the SD option without switching back.

Census over all three captures (39 boots in `raw.log`, of which `orin.log`/`unknown.log` are
subsets), keyed on: the popup text `Please select boot device` before the loader's first record,
the count of `ESC[=3h` (TerminalDxe's mode-0 string) in that boot, and the loader records' widths:

```
raw.log, 39 boots:
  menu=0 setmode80(=3h)=1 exact80=0 over80=17 maxlen=240|165  => wide     (31 boots)
  menu=1 setmode80(=3h)=2 exact80=16|17 over80=0 maxlen=80    => WRAPPED  ( 7 boots)
  menu=1 setmode80(=3h)=4 exact80=0 over80=17 maxlen=165      => wide     ( 1 boot: menu → UEFI Shell
                                                                 → startup.nsh; sequence on the wire
                                                                 "=3h 8;56;240t =3h 8;56;240t MENU =3h
                                                                 NSH SHELLPROMPT =3h 8;56;240t")
```

No exceptions in 39 boots: **wrapped ⇔ menu boot whose last console-mode commit before launch was
mode 0.** The one menu boot that ended wide is the one where the Shell was entered and the Shell
restored 240x56 (`ESC[8;056;240t`) before launching. The last `SetMode` before the loader decides
the wrap column. WRAPWITNESS (`a7490e81`, and `WRAPWITNESS.md` §"ESC[=3h … not a discriminator")
had this as "suggestive of a Boot Manager menu draw, not proven"; the popup text proves it.

Two more wire facts that matter below:

* **The 240x56 mode also hard-wraps — at 240.** In 18 wide boots a 243-column loader record
  (`main.rs@1228`-era) is split at exactly 240 with a real CRLF and a 3-byte `RD)` continuation
  (`awk` over `orin.log`: `next-after-240: len=3 [RD)]` ×18). The "max 240" measured by orin 12 was
  the first physical line of a 243-column record. So the driver wraps at *the current mode's
  column count*, whatever it is; 240x56 is not a wrap-free mode, it is a wrap-at-240 mode.
* **The firmware announces geometry to the terminal on every SetMode.** Every boot carries
  `ESC[2J ESC[004D ESC[=3h ESC[2J ESC[009D` (mode 0) followed by
  `ESC[2J ESC[004D ESC[8;056;240t ESC[2J ESC[016D` (mode 240x56, an xterm resize sequence),
  then the banner positioned with `ESC[105C` — which only lands where intended on a ≥110-column
  terminal. Distinct sequences on the whole wire: `ESC[Nm ESC[ND ESC[NJ ESC[N;NH ESC[NC ESC[=Nh
  ESC[N;N;Nt` — 24× `ESC[8;056;240t`, never any other resize value.

## 1. What the code does today (file:line, `hw-jetson@077a8fa1`)

`unaos/crates/bootloader/src/main.rs`:

* **:447** `uefi::helpers::init()` — installs the `uefi` crate's logger, whose output IS ConOut:
  `uefi-0.38.0/src/helpers/logger.rs:30-34` does `system::with_stdout(|stdout| LOGGER.set_output(stdout))`.
  Every `log::info!` in the loader is a `SimpleTextOutput.OutputString` on ConOut, in 128-UCS-2-char
  flushes, with `\n` rewritten to `\r\n` (`proto/console/text/output.rs:171-218`). There is no
  direct UART write anywhere in the loader; no `println!`/`print!` either (grep: none).
* **:467-483** CONGEOM. `with_stdout(|out| …)` iterates `out.modes()` (QueryMode over 0..MaxMode),
  takes the widest by columns, reads `current_mode()`, and prints
  `CON cur={}x{} wide={}x{} n={}`. Read-only by design (:462-466 comment): **nothing in the loader
  calls `set_mode` on ConOut.** The only `set_mode` in the file is GOP's (:643), a different protocol.
* **:305-334** `bootdiag_conout()` — renders ConOut's device path; `bootdiag` feature only
  (`arroyo:1769`, `UNAOS_BOOTDIAG=1`). **Never flown on metal**: no `BOOTDIAG:` line exists in any
  capture. Had it flown, the device path would name the terminal type (VenMsg GUID
  `7D916D80-5BB1-458C-A48F-E25FDD51EF94` = TtyTerm), which is the fact §2 turns on.
* **:778-792** the identity witness `KELF min= max= pg=`, budgeted to 76/78 columns so it survives
  mode 0. Verified in the one metal boot that had CONGEOM (boot 22 of `orin.log`, the F11 boot):
  `[ INFO]: crates/bootloader/src/main.rs@792: KELF min=0x0 max=0x2da2a8 pg=731` = 76 columns, intact.
* **:1055** `boot::exit_boot_services(...)`. The `uefi` crate calls `helpers::exit()` →
  `logger::disable()` first (`uefi-0.38.0/src/boot.rs:1408`, `helpers/mod.rs:57-60`): the logger's
  output pointer is nulled, so **no loader byte reaches ConOut after EBS** (the post-EBS `log::`
  calls at :1122/:1165/:1170/:1185 are inside `install_tegra_acpi_shim`, which runs *before* :1055).
* **The kernel never touches ConOut.** `grep -ri conout|simple_text|stdout_handle unaos/crates/kernel/src`
  → nothing. With `tegra`, `SerialPort` drives UARTC at `0x0C28_0000` directly with a bounded THRE
  poll (`unaos/crates/kernel/src/arch/aarch64/serial.rs:27-79`); it inherits the UART's clock/baud,
  not any UEFI console state. Whatever the loader does to ConOut ends at EBS.

Readers: the standing law is `awk '/pattern/' <log>`; wraps are rejoined by
`~/unaos-bench/tools/unwrap80.sh` (header: "firmware state we inherit, not set"; keys only on the
`[LEVEL]: ` logger prefix; join rule "exactly 80 then continue"; 5/6 fixtures).

## 2. What `SetMode` would change, and what it cannot

### Upstream firmware source (labelled: edk2 `TerminalDxe/TerminalConOut.c`, master; NVIDIA ships this driver unmodified — `NVIDIA.common.dsc.inc` lists `MdeModulePkg/Universal/TerminalDxe/TerminalDxe.inf` and sets `PcdDefaultTerminalType|4` = TtyTerm)

`TerminalConOutSetMode(This, ModeNumber)`:
1. `EFI_UNSUPPORTED` if `ModeNumber >= MaxMode`. **No early return when the mode is already current.**
2. `ModeNumber == 0` → emits `mSetModeString = ESC[=3h`; any other mode → emits
   `ESC[8;RRR;CCCt` with the mode's rows/columns (the exact `ESC[8;056;240t` on the wire).
3. `ClearScreen` (= `ESC[2J` + cursor home) **before and after** the mode string.

`TerminalConOutOutputString`, after writing the character that fills the last column:

```c
if ((TerminalDevice->TerminalType == TerminalTypeTtyTerm) && !TerminalDevice->OutputEscChar) {
  CrLfStr = "\r\n"; SerialIo->Write(...)   // "Print CR LF to synchronize the terminal with the driver"
}
```

That is the whole wrap mechanism: **a CRLF injected by the driver, only for terminal type TtyTerm,
at `MaxColumn` of the current mode.** It is what produced "exactly 80 then a continuation" and
"exactly 240 then `RD)`".

### The QEMU discrepancy, explained

`ArmVirtQemu.dsc` sets `PcdDefaultTerminalType|1` (VT100) unless built with `TTY_TERMINAL=TRUE`.
The Ubuntu AAVMF the bench uses (`UEFI firmware (version 2025.11-3ubuntu7)`) reports
`CON cur=100x31 wide=100x31 n=2`, emits the same `ESC[=3h` / `ESC[8;031;100t` pair, and then renders
a 101-column loader record (`@961 boot volume FAT serial …`) with no split
(`scratch/orin13/test-arm-final.log`: records=11, exact80=0, exact100=0, maxlen=101). A TtyTerm
driver would have split it at 100. So QEMU's console is VT100: **geometry is identical in kind, the
terminal type differs, and only TtyTerm wraps.** The prior seat's "geometry is not a wrap predictor"
is right for the wrong reason: geometry *is* the wrap column on the Orin; on QEMU there is no wrap
column at all. `cur=` predicts the Orin's wrap exactly once you know the driver is TtyTerm.

### Is 240x56 a real serial-terminal mode?

Yes, by construction and by demonstration. The `ESC[8;056;240t` is emitted by *TerminalDxe* from
*its own* `TerminalConsoleModeData[ModeNumber]` — a GOP text mode cannot produce that byte string
on the serial wire. ConSplitter only lists modes every attached ConOut device supports (spec
reasoning, labelled), so `n=6` with `wide=240x56` means the serial terminal driver supports 240x56
whether or not a GraphicsConsole is also attached (WRAPWITNESS showed both wrapped and wide boots
with and without a 1920x1200 GOP). And the firmware has driven the loader's 17 records through
240x56 on 32 of 39 captured boots without a garbled byte.

### What SetMode(widest) WOULD do (UEFI spec §SimpleTextOutput.SetMode, labelled: "the device is in the geometry for the requested mode, and the device has been cleared to the current background color with the cursor at (0,0)")

* On an F11 boot: switch the wrap column from 80 to 240. Every current loader record (max 165 on
  recent loaders) then renders on one physical line. The wire receives, once,
  `ESC[2J ESC[001;001H ESC[8;056;240t ESC[2J …` — byte-identical in kind to what the firmware
  already sends 1-3 times per boot before the loader starts.
* On an auto-boot: **nothing useful** — the console is already 240x56 — but the call still emits
  two `ESC[2J` and the resize string (no same-mode early return). Hence gate it on `cur_c < wide_c`.
* On a live human terminal (minicom/screen), the screen clears once more. The capture file is
  unaffected: `ESC[2J` is bytes to it, and everything printed before the call stays in the file.

### What SetMode CANNOT do

* It cannot stop wrapping: TtyTerm wraps at whatever `MaxColumn` is. A record >240 would still
  split (none exists today: maxlen 165; the 243-column `@1228` line is gone from recent loaders).
* It cannot change the kernel's console: post-EBS the logger is off and the kernel owns UARTC.
* It cannot retroactively fix archives: `unwrap80.sh` stays for pre-change captures and for
  the F1 archive fixture — and stays correct (it is a no-op on wide captures: joins=0 measured).
* It cannot be QEMU-verified: on `virt` `cur == wide` (100x31, n=2) and on x86 QEMU `cur == wide`
  (160x42, n=4), so a `cur < wide` gate never fires under QEMU. QEMU proves only that the build and
  the BEFORE/AFTER lines compile and print.
* It does not remove the 80-column budget on the identity line: the KELF line must still fit
  mode 0, because the BEFORE-line regime (a failed or refused SetMode) is exactly an 80-column boot.

## 3. The two positions

### FOR — call `SetMode(widest)`, narrowly gated

1. **The evidence says it is the mode the firmware already runs the wire in.** 32/39 boots
   inherited 240x56; the firmware's own Shell path re-selects it before launching (boot 34). The
   loader would be asking for the firmware's default, not an exotic mode.
2. **The mechanism is known, not guessed.** Upstream `TerminalDxe` under `PcdDefaultTerminalType=4`:
   wrap = driver CRLF at `MaxColumn`; SetMode = two clears + one resize string. Both fingerprints
   are on the wire already, many times, on this exact firmware (`t23x_general 39.2.0-gcid-45755727`).
3. **The blast radius is one code path.** Gated on `cur_c < wide_c` (and on tegra234 DTB, which
   `dtb_is_tegra234` at :1041 already computes), it is a no-op on every auto-boot and on both QEMU
   arches, and touches only F11 boots — the boots that already went through two firmware SetModes.
4. **It ends a whole class of misreads.** The `Kernel ELF:` misread cost orin 11 a session; 14 of
   15 call sites are still over budget "by design"; every capture of an F11 boot must be piped
   through a bench tool before `awk` can be trusted. One call at :483 makes the loader's own log
   readable line-wise on every boot, with no refits and no read-time tool for new captures.
5. **The identity anchor is protected by ordering.** Print `CON cur=` (BEFORE), SetMode, print
   `CON set=` (AFTER); KELF comes ~300 lines later. A garbled wire after the call still carries the
   BEFORE line and is itself the verdict.

### AGAINST — leave it inherited

1. **The problem is procedural, not firmware.** The wrap happens only when the operator presses
   F11. Auto-boot picks the SD (`UEFI SanDisk SS32G` is first in the boot-device list and 31 boots
   auto-launched into the loader). "Don't use F11; if you must, use `s` → Shell → launch" costs zero
   code and zero metal risk. Changing every Orin boot's console geometry to fix an operator habit is
   the wrong layer.
2. **The console we hand off through is not ours.** The BDS put mode 0 in place deliberately
   (`BmSetConsoleMode(FALSE)` is edk2's "restore standard console before booting an option"). A
   loader that overrides it is asserting a policy the firmware author chose otherwise. It is also
   the kind of edit `crates/bootloader` is shared for — x86 metal (the rMBP's ConOut is a
   GraphicsConsole on the panel) would need its own answer, so the change must be aarch64/tegra-gated
   to stay in lane, and that is one more `#[cfg]` seam in a file that already has eleven.
3. **`unwrap80.sh` already works, retroactively, on all 14 sites**, and a no-op on wide captures.
   The residual is a read-time pipe, not lost data. WRAPWITNESS's own conclusion was "the remaining
   question is not worth a power cycle."
4. **Unverifiable in QEMU and unobservable when it works.** The gate never fires under QEMU; on
   metal it fires only on F11 boots. A latent failure (a future firmware that lists a mode its
   serial driver mis-drives) would surface only on the bench, only on the menu path, as a blank or
   garbled loader window — and the loader window is where boot diagnosis starts.
5. **Two extra `ESC[2J`s per menu boot** on a wire that humans sometimes watch live, and one more
   line per boot in a log the seats already call over-instrumented (CONGEOM's own commit withdrew a
   gate for that reason).

## 4. Recommendation

**Do it, narrowly, and let one F11 boot decide.** The AGAINST case's strongest point (it is an
operator habit) is real, but the census shows the habit is exercised (7 of 39 boots, spread across
the whole capture history), the mechanism is fully understood from upstream source, and the
firmware itself performs the exact call the loader would make. The gated form has no effect on the
32/39 auto-boots and cannot fire under QEMU, so its entire risk is confined to the boots that are
unreadable today anyway.

Shape (an arc's job, not this executor's): at :483, after the existing `CON cur=` line —
`if dtb_is_tegra234 && cur_c < wide_c { set_mode(widest) }`, then always print
`CON set=<c>x<r> <ok|err=Status|kept>`; never fatal; keep KELF under 80 regardless. Prefer a build
knob (`UNAOS_CONWIDE=1` → bootloader feature, the `bootdiag`/`jb8lever` pattern at `arroyo:1769-1773`)
for the first flight; promote to default only after the test below.

**The ONE metal test** (one card write, two power cycles):
1. Stage the knob build; write the card (`orin-card-autowrite.sh`, sha-verified, as every stage).
2. Boot A — auto-boot (no key). Expect `CON cur=240x56 wide=240x56 n=6` (first-ever metal
   measurement of the wide regime's `cur=`) and `CON set=… kept`; loader window `over80>0, exact80=0`
   as today.
3. Boot B — F11 → `UEFI SanDisk … SD Device`. Expect `CON cur=80x25 wide=240x56 n=6`, then on the
   same wire `ESC[2J ESC[001;001H ESC[8;056;240t ESC[2J` immediately followed by
   `CON set=240x56 ok`, then KELF at 76 and the `@961 FAT serial` / GOP lines at 101+ columns
   **unsplit** (`awk` count: exact80=0 in the loader window). `unwrap80.sh --report` must show joins=0.
   Verdict is a one-liner: `awk '/CON /' <log>` shows both lines and no logger record is exactly 80.

**Cost of a failed test: one power cycle, no card rewrite to recover the box.** The loader's
control flow does not depend on ConOut (SetMode's `Status` is logged and ignored; the logger swallows
errors), EBS still happens, and the kernel drives UARTC directly, so the worst case is an unreadable
loader window between the BEFORE line and the kernel's first UART byte — which is the current F11
state. Reverting the *build* is one ordinary card write of the previous stage's `BOOTAA64.EFI` (or
just the next flight without the knob); the firmware is never touched, so there is no brick path.
