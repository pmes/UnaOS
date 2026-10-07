# SPLASHSTALL (rmbp-ledger B510): the kernel's steps under the splash

Branch `exec-rmbp-splashstall`, cut from 668cdd95. No new knob.

## Why
FLIGHT26.md §2 LOADERSTALL, re-read by B490: two boots sat on the held splash's word "Starting" for
minutes, and neither had any wire. "Starting" was `stage-resolve`'s label (`fs/users.rs`), painted by
`splash::step_label` at about 13 s. The splash's own 5 s bound (`splash::hold_service`) is polled from the
device-service pass (`video/desktop_uefi.rs:698`), and the store steps run in that same pass
(`users::service`). So a step that never returns also stops the bound from firing. The splash stays up,
it shows a word that says nothing, and nothing reaches the wire or the glass.

## What
1. **Every step under the held splash prints a begin line and an end line** (`fs/bootstep.rs` `begin` / `Step::end`):
   - `[boot] step=<name> begin at_ms=<kernel ms> budget_ms=<n>`, printed once per step. A step that is
     begun again while it is still live keeps its first start. The users store's `Busy` retries work
     this way: across passes the boot is waiting on that one step.
   - `[boot] step=<name> end ms=<n> budget_ms=<n> over=<0|1> blocks_read=<n> blocks_written=<n> cmds=<n> <extra>`
2. **The steps.** The steps under the held splash, in order, with the label the splash shows for each:

   | step | splash word | begins | ends | budget_ms | from |
   |---|---|---|---|---|---|
   | `store-wait` | Waiting for the disk | `users::service`, splash held, store not ready | first ready pass, or `hold_release` (`by=`) | 5000 | the hold's bound (no wire line before B510) |
   | `users-load` | Reading the volume | `users::service` | load ok, or the refusal bound (`store=none passes=`) | 249 | f26-boot.log `ms=83` ×3 (f24 83, f25 82, f27 81) |
   | `root-mount` | Mounting the system volume | `boot80_root_and_seed` | same fn | 3891 | f24-boots.log `ms=1297` ×3 (f25 1281, f26 590, f27 192) |
   | `stage-resolve` | Resolving the session (was "Starting") | `users::service` | same block | 927 | f27-boot1.log `ms=309` ×3 (f26 300, f24 200) |
   | `first-screen` | Preparing the screen | after stage-resolve, only while the splash is held | `splash::hold_release` (`by=first-screen`, `by=timeout`, or the login screen's own release) | 5000 | the hold's bound (no wire line before B510) |

   `assoc-seed` (flights 24–26) is no longer a boot step: FILETYPES (B423) moved it to `login ok`.
   Kernel bring-up before the hold (ACPI, SMP, PCI, the takeover) is timed by `bootpace` stamps, not
   steps. The splash shows no word during that stretch.
3. **Watchdog.** On x86 it is its own kernel task, `bootwatch`, at PRIO_HIGH. It runs on
   `sibling_online_cpu`, which is not the stepping task's core, and it is spawned at the first step's
   begin. Every 250 ms it reads the live table with `try_lock`; if the table is busy it skips that read.
   For a step past its budget it prints, then repeats every 5 s until the step ends:
   - on the wire: `[boot] step=<name> OVER budget_ms=<n> elapsed_ms=<n> last=<witness> cmds=<n>`
   - on the panel, in the band under the splash word: `<label> (<name>) is taking <s.d> s, budget <s.d> s, last <witness>, <n> commands`
   
   `last=` is the step's last witness:
   - `begin`
   - root-mount: `mount-table`
   - stage-resolve: `store-read`, `stage-witness`, `published`, `desktop-build`, `login-screen`, `create-user-form`

   The panel write takes the held surface with `try_lock` and `video::WRITER` with `try_lock`, then
   blits the band straight onto the panel. It makes no `wm` call, so a wedged compositor cannot block
   it, and a compositor that is still alive re-composites the same pixels. When a step ends or the next
   step's word is painted, the band is cleared. The task exits when no step is live and the splash is
   released.
4. **Tally.** `:: BOOT: firmware->loader=… lines=<n> steps=<n> slowest=<name>:<ms> over=<n> ::`. That
   line prints inside `stage-resolve` (`stage_witness`), so its tally counts only the steps that have
   ended by then. `tests splash` has the whole set.
5. **The test.** `tests splash` prints
   `:: SPLASH: steps=<n> slowest=<name>:<ms> over=<n> live=<n> watch=<task|none> -> PASS|FAIL|SKIP ::`.
   It is PASS when at least one step ran, none went over budget and none is still live.

## Expected wire on the next rMBP boot (a first boot, installer)
```
[splash] hold OPEN win=1 2880x1800 (until users::stage_resolve; 5000 ms bound)
[boot] step=users-load begin at_ms=<n> budget_ms=249
[boot] step=users-load end ms=81 budget_ms=249 over=0 blocks_read=35 blocks_written=0 cmds=79 store=fat via=sdhc users=0
[boot] step=root-mount begin at_ms=<n> budget_ms=3891
[boot] step=root-mount end ms=192 budget_ms=3891 over=0 … root=attrs
[boot] step=stage-resolve begin at_ms=<n> budget_ms=927
:: BOOT: firmware->loader=… loader->desktop=… total=… lines=<n> steps=2 slowest=root-mount:192 over=0 ::
[boot] step=stage-resolve end ms=309 budget_ms=927 over=0 … installer
[boot] step=first-screen begin at_ms=<n> budget_ms=5000
[boot] step=first-screen end ms=<n> budget_ms=5000 over=0 … by=<first-screen|timeout|…>
tests splash -> :: SPLASH: steps=4 slowest=root-mount:192 over=0 live=0 watch=task -> PASS ::
```
A stall looks like `[boot] step=stage-resolve OVER budget_ms=927 elapsed_ms=<n> last=<witness> cmds=<n>`
every 5 s. The same facts appear on the glass under "Resolving the session".
If there is no wire, the panel line is the read.

## Owed
- aarch64 has no watchdog task. The begin and end lines and the tally do run there.
- A wedge that holds the sibling core's run queue with interrupts masked silences the watchdog as well.
- The two `hold-bound` budgets get their first measurement on the next flight.
