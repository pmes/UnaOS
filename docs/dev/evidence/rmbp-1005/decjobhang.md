# DECJOBHANG (rmbp-ledger B386) — flights 24/25: `tests play flac` → `dec spawn … cpu=0`, then nothing

## Finding (the wire, then the code)
- Flight 24 card 3's reboot carries the line the seat's premise missed: Quarry's first coded open —
  `[play] dec spawn path=/system/test-f/TEST.AAC jid=1 stack=65536 cpu=0` →
  `[stack] OVERFLOW task=play-dec stack=0x20d43968..0x20d53968 fault=0x20d42c08 … via=df -> task halted` (same second).
  The task DID run on cpu 0: it ran off its 64 KiB stack in the AAC constructor. Measured (`-Z emit-stack-sizes`,
  x86_64-unaos.json, release, this tree): `open_arm::adts` 39352 + `AdtsStream::new` 39528 B; `mp4::open` 40424 +
  `AacSource::new` 39496; `ogg::open` 19832 + `OggOpus::new` 9208 + `OpusDecoder::new` 25240 + `SilkDecoder::new`
  16824 (MP3HANG's "largest frame left 9352" was not this tree's AAC/MP4/Opus arms). FLAC's chain is < 2 KiB.
- The halted task never reaches `dec_exit`, so `DEC_LIVE` stays set for the rest of the boot: every later open —
  M4A, MP3, OGG, OPUS, even FLAC — is `PLAYWAV … reason=decoder busy (stage=demux) -> REFUSED` and the watchdog
  re-prints the SAME dead job's `coded stall stage=demux frame=0 calls=0 ms=<growing>`. "ZERO decoder calls" is
  one dead job seen eight times, not eight decoders that never ran.
- The FLAC wedge (both flights, a clean boot) is a different shape: after `dec spawn … cpu=0` the WHOLE wire is
  silent (no `PWR` rollup, no `wc-h`, no stall line) until the power button. `cpu=0` is the BSP — the ONLY core
  that advances the global ms clock (`apic::ticks`) — and `with_unafs` runs every UnaFS transaction under
  `without_interrupts`; the decoder's reads (`/system/test-f` is UnaFS on sdhc) put masked spans on the clock core.
  The waiting shell (`tests play`, a `delay_us` spin on the render task) and the watchdog both measure with that
  clock, so once the BSP stops ticking nothing in the machine can name the fault — the silence matches. Not
  proven to the instruction (no symbol on the wire); the fix takes the clock core and the clock out of the path.
- `other_dispatching_cpu()` returns the LOWEST dispatching core other than the caller's: on x86 that is always 0.

## Seam
Kernel — fulfiller of the play verb (hda_play.rs tail, `dec_*`); placement through `smp::worker_cpu` (the
existing pool), no new store, no new knob (rides `hda-tone`).

## Milestones
- M1 `play-dec` on a worker core (never cpu 0, never the caller's core); stack sized to the measured chain
  (160 KiB: worst ~97 KiB = 79920 constructor + VFS read chain ~16 KiB, + margin); a dead job releases the decoder
  (the watchdog's abort clears liveness; a straggler's exit clears it only for its own jid).
- M2 the guard on the TSC clock (not the BSP's ms tick); the clock starts at the task's FIRST beat; a job not
  running 500 ms after its spawn is named `[play] dec not-scheduled jid= cpu= ms=`; `[play] dec run jid= cpu=
  wait_ms=` is the task's first line.
- M3 `tests play` never blocks the shell: it queues the formats and returns; the service tick plays them one by
  one, prints each verdict, then `:: DECJOB: spawned= ran= on=worker shell_blocked_ms= -> PASS ::`.

## Witness (metal)
```
[play] dec spawn path=/system/test-f/TEST.FLAC jid=1 stack=163840 cpu=2 on=worker
[play] dec run jid=1 cpu=2 wait_ms=<n>
[play] open path=/system/test-f/TEST.FLAC format=flac codec=Flac …
:: PLAYCODEC: path=/system/test-f/TEST.FLAC format=flac codec=Flac … -> PASS :: …
[play] dec exit jid=1 why=eos … stack high=<n> of 163840
:: DECJOB: spawned=7 ran=7 on=worker shell_blocked_ms=<small> -> PASS ::
```
and from Quarry, each of MP3/OGG/OPUS/M4A: `dec spawn … on=worker` → `dec run` → `PLAYCODEC … -> PASS`.

## Owed
- audio_core's AAC/MP4/Opus constructors still build ~40 KiB values on the stack (`AacDecoder.chans`, Opus state):
  the source-side bound (heap them, as MP3HANG M1 did for MP3) is the shared core's, not this arc's.
- The FLAC wedge's exact instruction (masked span vs a lock on the clock core) is not on the wire; the next
  flight's `dec run` / `not-scheduled` lines and a live `PWR` rollup tell it apart.
- A preempted holder of a raw lock on the BSP freezing the global clock is a general hazard (any task on cpu 0).
