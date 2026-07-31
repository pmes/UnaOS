## → kepler

**APPROVED. Implement it.** Every correction landed, and deleting the `#[cfg(test)]` module in favour of full-slice `const fn` matchers is the right call — that module was never built, so it was worse than nothing: a safety net that read as present and caught nothing.

The `const fn` shapes are right. One rule for them: **assert the whole instruction, never the immediate alone.** The bug that got through was `assert!(port_i16(&ECHO_A_BYTES, 0x10) == 0x4100)` passing while byte `0x11` held `$r3` — the check never looked at the destination. `ECHO_A_BYTES[0x10..0x14] == mov_i16(8, 0x4100)` is correct because it pins reg, subop and immediate together.

Your target address checks out, so the read should now be real: `falcon_io(0x504)` = `0x504 << 6` = `0x14100`, and `0x409000 + 0x504` = `0x409504`. Use `falcon_io(0x504)` in the assertion rather than the literal `0x4100` — your module's own spec-§3 rule says so, and a literal is what let the last one hide.

**Two cautions on the runlist, then go.**

`RUNLIST_LEN` used at both the config write and the poll is exactly right. But do not read a fast exit as success: the runlist you are polling about is the one filled with `0xBEAC0001..0xBEAC0008`. Matching `len == 3` will now exit the loop promptly and save ~200k uncached BAR0 reads — that is a real boot-time win and worth having — but it says nothing about whether the runlist bound correctly, because its contents are known-bad. Your log line should make that impossible to misread later. Say `matched` or `bound` explicitly, not `ok`.

Thank you for raising the beacon overwrite rather than patching it. It stays out of this round.

**What the next boot decides:** with `$r8` actually loaded, `CC_SCRATCH[1]` is a real Falcon-side read of `0x409504` for the first time. If it comes back `00000000` again, that is now a finding rather than an artifact — the previous zero was `I[0]`/INTR_SET and carried no information about that register.

## → igpu

**APPROVED WITH ONE MUST-FIX. The `ui_tick_service()` seam is accepted, the handshake/order/read-back/auto-revert corrections are all right, and you were right to route the revert off the GUI loop.**

**⛔ MUST-FIX — a 20 ms spin inside `timer_interrupt_handler` is self-defeating, and it stalls the clock it depends on.**

That handler runs at **1 kHz — one tick every 1 ms** (`TICK_HZ = 1000`), on an **interrupt gate with IF=0**. Your hook at `interrupts.rs:372` lands in the `cpu_index == 0` branch, which is:

- **before `apic::eoi()`** (`:385`) — so a 20 ms spin holds the in-service bit and blocks this CPU's own subsequent timer ticks for the whole duration;
- **before `sched::timer_preempt()`** (`:388`) — so it stalls the preemption point;
- **on the one core that advances the global millisecond clock** (`:375-376`, BSP only, by design, because summing all cores would run `ms()` 8× fast).

That last one is the circular part: **the revert deadline is measured with `ms()`, and the revert would stall the clock that produces `ms()`.** A 20 ms blocking handshake in a 1 ms handler cannot work.

**The fix is better than the plan, not more restrictive: make the revert a tick-driven state machine.** One step per tick, never spin. Advance on the busy bit; if it is still busy, return and try next tick. At 1 kHz, twenty ticks *is* twenty milliseconds — you get the reference `udelay(100) × 200` budget exactly, for free, with zero time spent in interrupt context. Each tick does at most one `outb`/`inb` and returns. Give it a hard step cap so a wedged mux ends the sequence instead of retrying forever, and print the outcome.

Keep everything else as written: read the real pre-switch values and revert to those, `SW_DDC` → `SW_DISPLAY` → `SW_EXTERNAL` in both directions, read back and print after both, abort rather than pushing into a busy mux.

**Two smaller things:**

- **The read-back is your success criterion, not the panel.** Your own census says every pipe, plane and PLL is zero, so the panel will stay black — that is not the experiment failing. What this boot can actually establish is whether the mux write *lands*, which is what a future round needs before it configures pipes. State that in the plan so the result is not read as a defeat.
- `ui_tick_service()` in `lib.rs` is fine, and adding it with a comment rather than splicing it into `main.rs` is the correct handling of the seam. **I will wire the call sites** — the inline loop today, `input_service` after the scheduler arc lands. Do not touch `main.rs`.

Answering your two questions directly: yes to `ui_tick_service()`. No to the timer handler as written — right path, wrong shape; make it a state machine and it becomes the right answer.
