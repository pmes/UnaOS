# Shell Philosophy: The "Midden" Integration

## 1. The Command Line is Primary
In unaOS, the GUI is just a visualizer for the shell. Anything you can click, you can type.
* **The Universal Shell:** All system settings are exposed as text commands.
* **Consistency:** We adhere to the POSIX standard where possible, but extend it with structured data (JSON/NuShell style) where Unix falls short.

## 2. Deep Integration of "Midden"
Your "Midden" project (Shell History Organizer) is not an app; it is the system's memory.
* **Contextual Recall:** The OS remembers not just *what* command you typed, but *where* (directory), *when* (time), and *why* (git branch context).
* **Predictive Assisstance:** Because `midden` understands context, the shell can autocomplete entire workflows, not just single words.
    * *User types:* `git p`
    * *Midden suggests:* `git push origin feature/new-kernel --force-with-lease` (because you just rebased).

## 3. The "Gneiss" Abstraction
The shell uses your "Gneiss PAL" to abstract differences between hardware.
* A script written on your Mac Hackintosh will run identically on a Pixel 10 because `gneiss` translates the underlying hardware calls (e.g., `wifi_up`) into the correct driver commands automatically.

## 4. `tste` — the in-OS self-test suite (TSTE-1)

Running tests should not require a host. From any booted shell (x86 GUI, the Orin panel, or the
serial console) the command **`tste`** runs the self-test suite and prints a PASS/FAIL/SKIP table in
the console — like `ps`, it does **not** take over the screen. The suite lives in
`crates/kernel/src/selftest.rs` (the `tste`/`selftest` shell arm just calls `selftest::run`). Every
line is mirrored to serial as `:: TSTE: <name> -> PASS/FAIL/SKIP ::`, closing with
`:: TSTE: N pass M fail K skip (+B boot) ::` — that serial evidence is the headless QEMU gate.

### One view of all tests — three sections

`tste` output is a single view of *all* tests, even the ones it cannot itself re-run:

* **`[boot-time]` (captured at boot)** — the boot-sequenced fixtures (the U-arc / M-arc EL0 tests)
  cannot be re-executed post-boot yet, so `tste` **replays** their verdicts. Every fixture funnels
  its result through the serial print path as a uniform `:: … -> PASS ::` / `-> FAIL ::` line; a
  fixed 64-entry static ring in `selftest.rs` captures `(truncated name, verdict)` from those lines
  via one additive hook at the serial `_print` seam (both arches). The hook is alloc-free, `try_lock`
  only (drops on contention, counts overflow drops), and safe from IRQ-masked print contexts — it
  changes nothing about what is printed, and excludes `tste`'s own live lines so replay can't feed
  back on itself.
* **`[live]`** — the checks that honestly re-run on demand (see the registry below).
* **`[skipped]`** — checks that cannot run in the current context, each with its reason.

### The live registry

| test | what it exercises |
| --- | --- |
| `sched.introspection` | meter counters readable + monotonic (never decrease); ≥1 CPU visible. Read-only. |
| `heap.roundtrip` | global allocator: `Vec` + `Box` alloc / write / read-back / free / re-alloc. |
| `video.geometry` | `draw_line` + `fill_triangle` on an **offscreen** `GneissPal` pixel buffer (the trait-default rasteriser), asserting pixel counts — zero touch to the visible framebuffer. |
| `sync.mutex` / `sync.rwlock` / `sync.semaphore` / `sync.channel` | uncontended API round-trip from a fresh scheduled worker task. |
| `sync.condvar` / `sync.join` | a real cross-task blocking wake-up (`Condvar` Mesa predicate loop; `spawn_joinable` + `join`). |
| `storage.mount` / `storage.rootwalk` / `storage.readfile` | FAT mount status, root-dir walk, and a known-file read (`HELLO.BIN`, length + non-zero content). **READ-ONLY** — `tste` never creates/writes/deletes. |

### Context safety (why the coordinator never blocks)

`dispatch_command` runs in different contexts per platform: the x86 GUI inline loop (which is **not**
a scheduled task), the Orin `jd2_console_pump` task, and the Pi `GUI_CHANNEL` task. On the
unscheduled x86 BSP a blocking sync primitive (`Mutex::lock`, `Semaphore::wait`) would panic or bail.
So the sync section does all blocking inside **fresh worker tasks** it spawns; the coordinator only
polls an atomic result with a **bounded, non-blocking** budget (busy-poll + `hlt`/`yield_now`, never
`sleep_ticks`). A broken or hung primitive therefore surfaces as a `FAIL`/`SKIP` line — never a shell
hang.

### SKIP policy (honesty over green)

A check **SKIPs** rather than fake a result when it genuinely cannot run here:

* the sync section SKIPs on a single-core system (no AP to run fresh workers) or if the probe worker
  does not complete within budget (no active scheduler);
* the storage section SKIPs when no FAT volume is mounted (e.g. the raw `usb.img` pattern image under
  a plain `./arroyo test`).

### The TSTE-2 horizon

Two families stay boot-sequenced and are only *replayed*, not re-run, by `tste`:

* the EL0 U-arc / M-arc fixtures (they need an EL0 program launcher usable post-boot);
* the full **cross-core CAPSTONE** stress harness (the `[live]` `sync.*` checks re-verify the
  primitives functionally on demand, but the cross-core stress runs at boot).

Making these re-runnable on demand needs a launcher refactor — **TSTE-2**. `tste`'s output footer
says so explicitly.

## 5. Native midden M1 — the framebuffer console is the x86 interactive shell

**The default x86 GUI build is the interactive path.** Historically the media flashed to the 2012
rMBP was the `usbdebug` build (`UNAOS_USBDEBUG=1`), which attaches the framebuffer console to the
serial log and then **loops forever** — it never reaches the interactive `Console` loop. Native
midden M1 makes the **default** GUI build (no `usbdebug`, no `bootlog`; `ehcihid` default-on) the
flashed shape: it paints a first frame, drops to the framebuffer prompt, and dispatches commands
typed on the internal EHCI keyboard. The interactive loop and the `Console` widget already existed
and were already reachable in the default build; M1 proves that boot mode is reachable and sound and
lands one command (`batmon`) on it. There is **no compositor, no `View`/`Scene`/`Sink`, no
`TerminalView`, no seam machinery** — those are M2+ and out of scope.

**COEXIST, with the x86 nuance.** The serial stream stays the debug/log diagnostic surface; the
framebuffer `Console` is the interactive view. On x86 there is **no second interactive shell** — the
framebuffer `Console` *is* the x86 interactive shell, and serial remains the debug/log stream.
"Coexist" here means the serial diagnostic surface stays alongside the interactive framebuffer view,
**not** that two interactive shells run. The `usbdebug` build remains available purely as the
debug/log view. Structurally M1 is a no-op: it reuses the existing x86 GUI loop
(`main.rs`, drain-until-None then present-once) and the existing `Console` widget unchanged.

**Bounded pre-first-frame path.** The rMBP has no serial console, so a hang before the first
`pal.render()` would leave a silent black screen. The default-build path from `kernel_main` to the
first frame is audited bounded: the known long pole is xHCI bring-up in `pci::init` (its timeouts are
`rdtsc`-bounded; `UNAOS_SKIP_XHCI=1` is the documented escape hatch), and nothing else on the default
path spins unbounded before the first frame.

### `batmon` — the honest SMC battery line

`batmon` prints one line summarising the SMC battery snapshot. It is a one-shot human command, so it
does a **fresh** port-I/O read (`smc::battery::snapshot()`, unthrottled) rather than the cached
boot-time snapshot the shell never refreshes — the reading reflects the battery *now*. A fresh read
once per keypress on the single-threaded BSP is safe.

**Honesty rule.** Every field on `BatterySnapshot` is an `Option`; an absent key renders the `-`
sentinel and a `None` **never** renders as a number a reader could mistake for a real value (0 mA is a
plausible amperage, so absence must not read as zero). The line mirrors the M2 witness sentinel shape.
`batmon` prints and returns — it leaves the console visible, takes no screen, does no seam work.

The command is gated `#[cfg(all(target_arch = "x86_64", feature = "smc"))]` (the SMC driver is x86
`UNAOS_SMC=1` only); every other target compiles an honest fallback that says so. Under QEMU's
key-less `isa-applesmc` all battery keys are honestly absent, so `batmon` prints the all-`-` line —
the bounded-Absent proof. The real battery is read only on the physical rMBP at an attended sitting.
