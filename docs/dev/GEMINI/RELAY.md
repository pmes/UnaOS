# PETER'S RELAY SHEET — not for specialists. Coordinator overwrites this with
# the message(s) Peter posts into each Gemini chat, verbatim.
# (updated 2026-07-25: pulls 18+25 LANDED with coordinator land-review fixes — these messages tell the specialists what changed and why)

## → kepler-fence session

Fence: pull 25 landed, and land-review changed two things in your code — both worth understanding because one of them was MY error, not yours.

(1) THE IO PORT: my approval amendment told you 0x40. That was wrong. Host register X maps into the Falcon's IO space as (X & 0xffc) << 6 — that's how nouveau's own Kepler FECS/GPCCS microcode computes it (macros.fuc, nv_mkio). So MAILBOX0 (host +0x040) is I[0x1000], and I[0x40] would have decoded to INTR_SET. Your encodings were correct for the port you were given; the port was mine and it was bad. The landed code now runs image A (port 0x1000, magic F00DFACE) and, ONLY if the mailbox is unchanged, image B (port 0x40, magic F00DBEEF) — sequential and conditional, so one shot answers the indexed-vs-flat question either way. Also switched iowr → iowrs (0xd0 → 0xd1) so a write can't be dropped at the halt boundary.

(2) IMEM PAGE PADDING: a 4-word upload leaves the code TLB entry for page 0 marked busy — the entry is only marked usable when the LAST word of the 0x40-word page is written. The core would have started and parked on a paused fetch forever. The upload now pads to 0x40 words (nouveau's load_imem does exactly this), and there's a TLB_CMD PTLB query printing whether page 0 came out usable.

Also added, all read-only except one: MAILBOX0 is seeded with A5A50000 before each shot so "unchanged" has exactly one meaning; a halt-iters counter with a 100k budget (the old poll broke on the first read, and since CPUCTL reads 0x10 at rest it proved nothing either way); verify prints all five words; the abort marker no longer prints the literal string VERIFY_FAIL where hex is expected; and after the shots a read-only sweep of base+0x000..0x1FC tags either sentinel wherever it actually landed.

Your assembly itself verified clean byte-for-byte against the ISA docs — encodings, LE packing, sethi-replaces-high-half semantics, exactly 16 bytes, no stack, no branch. Good work. Next pull depends on the metal result; stand by.

## → kepler-display session

Display: pull 18 landed with land-review hardening, and one correction to what I told you last round.

THE CORRECTION: I recorded pull 17 as "1:1-with-offset REFUTED" and cited an arithmetic impossibility. Both were overreach. Our pointer sits 1408 rows above VRAM 0, not the 352 I claimed (I divided by the wrong power of two), so there is no impossibility. And a mostly-black surface with five thin lines is photographically indistinguishable from a mostly-black firmware console — a thin white line two-thirds down is exactly where a boot-log cursor sits. Pull 17 is now recorded as INCONCLUSIVE, not a measured null. Don't carry "only 8 rows reach the panel" into your thinking.

What I added to your probe: a 4 px gutter in each barcode cell (without it adjacent equal bits merge into one run — band 112 reads as 96, a 256-row error); a unique colour for band 0; 4-row fiducials at the surface top and bottom; and a diagonal ramp whose slope measures the src-row → panel-row map globally and survives blur and bad exposure where 16 px band heights don't. Plus the thing your probe most needed: a PRE-LATCH control frame — the surface is painted, then held for 3 s BEFORE the latch, so photo A shows what the head is scanning without us, and photo B shows it after. If A already shows our bands, the latch isn't what put them there and every placement conclusion would be void. There's also a GOP/scratch overlap check logging whether our fill region intersects the firmware framebuffer, and the hold dump now fires on the first AND last tick with ptr_hi/armed/shadow plus per-head HEAD_STAT VERT.

Your falsification table was the right instinct and it's what made this pull reviewable. Next pull depends on the metal result; stand by.
