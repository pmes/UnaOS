# BP-power — the power verbs, the SMC, the lid and sleep (read at e1c9dcd0; SLEEP B691 folded in)

## 1. What it is
- `power.rs` is where every power VERB lands, one word on every UnaOS: `reboot`, `shutdown`, `sleep_request` (`unaos/crates/kernel/src/power.rs:57` `unaos/crates/kernel/src/power.rs:67` `unaos/crates/kernel/src/power.rs:359`). The mechanism behind each is per platform.
- x86: reboot is the FADT RESET_REG then the 8042 pulse (`unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:566`); shutdown is ACPI S5 from the `\_S5_` package found by a narrow byte scan, no AML interpreter (`unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:124` `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:360`). aarch64 non-Pi: PSCI via SMC. Pi: an honest park.
- The Apple SMC (`unaos/crates/kernel/src/drivers/smc.rs`, knob `smc`) is a polled key/value device on ports 0x300/0x304, READ only: battery keys (`BRSC B0AC B0FC B0RM`), the adapter, the lid key `MSLD` (`unaos/crates/kernel/src/drivers/smc.rs:434` `unaos/crates/kernel/src/drivers/smc.rs:1423`).
- LIDSLEEP (B431, knob `lidsleep`, on the rMBP line `unaos/scripts/image-line.env:48`) reads `MSLD` (1 s awake) and blanks the backlight through the gmux (`unaos/crates/kernel/src/video/backlight.rs:485`). Before B691 the lid ladder was unapplied; `MSLD` has never been seen to flip on metal (every flight: `[smc] lid=open raw=0`).
- SLEEP (B691) makes the blank a STATE, `idle`: backlight off, the panel held (compositor and cursor refuse), every core in the scheduler's `sti; hlt`, three named wakers. S3 is not entered: the tree cannot hold it (§5).
- DIMIDLE blanks the panel after N idle minutes; it and the sleep state are refusal terms of the compositor (`unaos/crates/kernel/src/video/dimidle.rs:1-14`; `unaos/crates/kernel/src/video/mod.rs:839-850`).

## 2. Invariants
| id | statement | anchor | held by |
|---|---|---|---|
| BP-power-1 | A power verb never returns on failure silently: it witnesses, dispatches the platform mechanism, and either the platform acts or the refusal line prints and the core parks in `hlt_loop`. | `unaos/crates/kernel/src/power.rs:1-13` | `[pwrreboot]` / `[pwrshutoff]` lines |
| BP-power-2 | Every Restart/Shut Down runs `prelude` (dirent flush, windows closed, HDA stopped, ABIMAGE's `why=user`) and prints ONE `:: POWER: action= … -> going ::` as the last line before the firmware call. | `unaos/crates/kernel/src/power.rs:336` `unaos/crates/kernel/src/power.rs:349` | the `:: POWER:` line |
| BP-power-3 | Every x86 power port drains the staging ring and the FTDI mirror ring first (a power verb has no next print). | `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:360` `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:566` | `ring drained` + `ftdi flushed` lines; `s5drain_selftest` |
| BP-power-4 | ACPI facts are read from the tables, never guessed; no AML is executed (byte scans of `\_S5_`/`\_S3_` packages only). | `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:124` `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:757` | `[pwrshutoff]` discover line; `[power] s3 census` |
| BP-power-5 | The SMC is READ only: `READ_CMD` and key-by-index; `WRITE_CMD` is never issued; every handshake is bounded (`Stuck`, never a spin) and the conversation lock is a bounded, yielding take. | `unaos/crates/kernel/src/drivers/smc.rs:1-40` `unaos/crates/kernel/src/drivers/smc.rs:434` | `[smc]` STOP-NOTE lines; PUMPWEDGE names a holder |
| BP-power-6 | Sleep is ONE state with ONE owner: `video::lidsleep::sleep_request` enters it (lid close, the login window's Sleep, `tests sleep`), `wake(src)` leaves it; `power::sleep` only records and prints. `power::sleep_request` is the arch-neutral verb. | `unaos/crates/kernel/src/video/lidsleep.rs:194` `unaos/crates/kernel/src/video/lidsleep.rs:213`; `unaos/crates/kernel/src/power.rs:386` | `[lidsleep] sleep_request` / `[lidsleep] wake` lines |
| BP-power-7 | The state is `idle`, never claimed as S3: backlight off (gmux 0x74 := 0), the panel held by the refusal term `sleep-panel-off`, every core in `hlt` (C1) between 1 kHz ticks — or, under the `tickless` knob, every idle core halted to its nearest deadline (BP-cores-18; the line then says `tick=tickless-idle`) — USB up. CHANGED by TICKLESS (B765, seat: wave36 arc tickless) (the FTDI wire, the keyboard waker). The line says each part. | `unaos/crates/kernel/src/power.rs:372-384` `unaos/crates/kernel/src/power.rs:412`; `unaos/crates/kernel/src/video/mod.rs:848` | `[power] sleep enter=idle … s3=unreachable` |
| BP-power-8 | Every wake has a NAMED source (`lid`, `power-key`, `input`, `tests`); any other is counted `unexplained`. The lid is polled every 200 ms while asleep (1 s awake). | `unaos/crates/kernel/src/video/lidsleep.rs:37` `unaos/crates/kernel/src/video/lidsleep.rs:102-131`; `unaos/crates/kernel/src/power.rs:444` | `:: SLEEP: … unexplained=0` |
| BP-power-9 | The wake repaints (the DIMIDLE repaint) and THEN prints `wake_ms` (detect → first composite returned); the battery is read after the frame, never inside `wake_ms`. | `unaos/crates/kernel/src/video/lidsleep.rs:122` `unaos/crates/kernel/src/power.rs:455` | `[power] sleep enter=idle wake src= ms=` |
| BP-power-10 | The drain is MEASURED from the SMC (`B0RM`/`B0FC` at entry and after the wake frame), printed only across ≥ 60 s of sleep on a falling charge; otherwise `unmeasured` with the reason. | `unaos/crates/kernel/src/power.rs:421` `unaos/crates/kernel/src/power.rs:455` | `:: SLEEP: … drain_pct_per_h= … drain=<measured or reason>` |
| BP-power-11 | The power key is the FIXED-feature button only (PM1x_STS PWRBTN_STS, polled, write-1-to-clear of bit 8 alone); a control-method button (FADT flags bit 4) is named absent, not guessed. A latch from before the sleep is cleared at entry. | `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:827` `unaos/crates/kernel/src/arch/x86_64/acpi_power.rs:835` | `[power] s3 census … pwrbtn=`; `stale_pwrbtn=` |
| BP-power-12 | Nothing here runs at boot but the read-only lid poll: `tests lidsleep` (the backlight blink) and `tests sleep` (enter, 1.5 s, leave) are registered, R80. | `unaos/crates/kernel/src/tests.rs:322-328`; `unaos/crates/kernel/src/video/lidsleep.rs:226` `unaos/crates/kernel/src/video/lidsleep.rs:248` | `:: LIDSLEEP:` and `:: SLEEP:` lines |

## 3. Seams
- Callers of `power::sleep_request`: the login window's Sleep (`unaos/crates/kernel/src/video/login.rs:2509`). The lid ladder calls `lidsleep::sleep_request("lid")` from its poll.
- `lidsleep::service` rides `dimidle::service` (`unaos/crates/kernel/src/video/dimidle.rs:106`) on the service pass: the entry's black fill and `power::sleep::measure_enter`, the three wakers, the poll, the wake's repaint and `first_frame`.
- The panel off/on seam is `video::panel_refuse_term` (one term per owner); the backlight writer is `backlight::off_via` / `set_raw_via`.
- `acpi_power::s3_census` / `pwrbtn_take` read the FADT, the FACS and the DSDT/SSDTs; they write only PWRBTN_STS.

## 4. Wire (the bench: lid close, wait, lid open)
`[smc] lid=closed raw=<b> was=0 flips=<n>` → `[lidsleep] sleep_request via=lid backlight_off=ok … panel=held wake=lid,power-key,input` →
`[power] sleep enter=idle via=lid backlight=ok panel=refused cores=hlt-c1 tick=<1khz|tickless-idle> usb=up s3=unreachable` →
`[power] s3 census facs= waking_vec= s3_pkg= pts= wak= pwrbtn= aml_interp=none bsp_resume=unwritten kepler_post=unwritten xhci_restore=unwritten -> s3=unreachable state=idle` (once per boot) →
`[power] sleep measured full_mah= rem_mah= stale_pwrbtn=` … `[smc] lid=open raw=0 was=<b>` → `[lidsleep] wake via=lid restored=<raw> readback=<raw>` →
`[power] sleep enter=idle wake src=lid ms=<n> slept_ms=<n> lid_poll_ms=200` → `:: SLEEP: state=idle wake_ms=<n> drain_pct_per_h=<n.nn or unmeasured> wakes=<n> unexplained=0 -> PASS|FAIL|SKIP reason=no-gmux :: …`.

## 5. Ruled, unbuilt / open
- ACPI S3 (and the bar's "lid-open → first frame" through a resume): unreachable from this tree — no AML interpreter for `_PTS`/`_WAK` and the wake GPEs, no BSP resume path through the FACS waking vector (the AP trampoline at 0x8000 is SIPI-only), no Kepler re-POST (the desktop runs on the firmware-initialised GPU), no xHCI/AHCI save/restore (xHCI §4.23.2). The census line names the firmware's half.
- A tickless idle is BUILT under the `tickless` knob by TICKLESS (B765, seat: wave36 arc tickless; cores.md BP-cores-18): an idle core arms its LAPIC timer one-shot to its nearest deadline (`unaos/crates/kernel/src/arch/x86_64/sched.rs` `pub mod tickless`; `apic::timer_oneshot_us`). The ms clock was already the TSC (CLOCKCORE B397); the lid poll and DIMIDLE ride the usb-pump's 1-tick sleep, a sleeper deadline, and need no registration of their own. Unflown; the knob is off on every image line until the seat wires it. Deeper C-states (MWAIT hints) are not built.
- USB port suspend is not done: the FTDI console and the internal keyboard are the wire and a waker.
- `MSLD`'s polarity (`nonzero = closed`) is still rung 0's hypothesis; the first bench lid close answers it. The waking key is not swallowed (DIMIDLE swallows its own; LIDSLEEP's seat answer, owed).
- BP-bar-20's 8 h drain (≤ 2 %, ≤ 1 %/h) is the bench's measurement; `idle` keeps the Kepler scanning out and the CPU package out of deep C-states, so the bar is not expected to hold until a deeper state exists.

## 6. Read further
- `docs/dev/evidence/rmbp-1005/lidsleep.md` — LIDSLEEP's rung ledger and its S3 walls.
- `docs/dev/OS/02_KERNEL_CORE/serial_transport.md` — why power verbs drain first.
- `docs/dev/OS/rmbp-ledger.md` rows B431 (LIDSLEEP), B430 (LOGINWINDOW), B683 (ABIMAGE), B691 (SLEEP).
