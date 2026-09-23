#!/bin/bash
# CURSORFLK PROBE (scratch, reverted after) — force Class 3's foreign pop on demand: after PTRDEAD's
# backlog leg has pushed its 192 synthetic (+1,-1) motions, spin (bounded 200 ms) until SOME OTHER
# drain has popped the ring, i.e. hand the fold accumulator to `x86_input_service` deterministically
# instead of by timer-preemption luck. Line-neutral (same-line append after the push loop's `}`).
F=/home/pmes/unaos-bench/scratch/rmbp-0915/cursorflk/unaos/crates/kernel/src/arch/x86_64/syscall.rs
L=7165
[ "$(sed -n "${L}p" "$F")" = "    }" ] || { echo "probe: line $L is not the push loop's close"; exit 1; }
[ "$(sed -n "$((L-1))p" "$F" | tr -d ' ')" = "crate::pal::push_pointer_report(Some(Event::Mouse{x:1,y:-1}),None);" ] || { echo "probe: anchor mismatch"; exit 1; }
sed -i "${L}s/^    }\$/    } { let t0 = crate::arch::ms(); while evq_pops() == pop0 \&\& crate::arch::ms().wrapping_sub(t0) < 200 { core::hint::spin_loop(); } serial_println!(\"[cursorflk-probe] backlog handed to a foreign drain after {}ms pops={}\", crate::arch::ms().wrapping_sub(t0), evq_pops() - pop0); }/" "$F"
sed -n "${L}p" "$F"
