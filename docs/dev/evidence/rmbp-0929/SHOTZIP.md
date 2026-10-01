# SHOTZIP — a real deflate encoder and a streaming screenshot writer (R76: written, not run)

## Finding (boot 17, `f17-boot1.log`)
Peter: "i got the screenshot to open but it took forever".
`[143325ms] :: PRTSCR: SCREEN0.PNG 2880x1800 name_from=clock-unset -> capturing (15555053 bytes reserved …`
then `[187295ms] … 15555053 bytes -> OK :: source=sdhc … dir=HOME/UNA/Desktop ::` — 44 s.
15 555 053 = 2880x1800x3 + headers: `video/png.rs` wrote STORED deflate blocks (BTYPE=00), so the file
was the raw image, and the card took 220 KB/s of it. The whole 15.5 MB was also reserved up front.

## Mechanism
- `video/png.rs` — `PngEncoder`: Sub filter (type 1) per row -> one fixed-Huffman block (BTYPE=01) with
  greedy LZ77 over a 32 KiB window (64 KiB ring, 3-byte multiplicative hash -> `head[32768]`,
  `prev[32768]` chain, `MAX_CHAIN`=32, stop at 128, len-3 matches beyond 4096 dropped). Output is cut
  into a 33-byte header piece, IDAT chunks of <= `CHUNK` (64 KiB) and IEND by `next_piece`.
  Memory ~0.5 MiB, constant. `Verify` is a streaming fixed-Huffman inflater fed every IDAT payload; its
  length + Adler-32 + EOB agreement with the encoder is the PASS (`selfhost::inflate` is feature-gated
  and wants a whole stream behind a pull source, so it was not reused).
- `video/prtscr.rs` — `Phase::Encode`/`Phase::Write` collapse into `Phase::Stream` (`Job::unit`): write
  the waiting piece in `SLICE_WRITE` slices; else cut the next piece; else encode rows until a chunk is
  ready; else `finish`; else verdict. The directory entry is created when the first piece (signature +
  IHDR) is cut, i.e. before any pixel is read, as the reservation used to guarantee.

## Choices
- Filter: **Sub**. A flat fill becomes a zero run, a gradient a constant run; both are distance-1
  matches that cost ~13 bits per 258 bytes. One subtract per byte; no per-row heuristic.
- Greedy, not lazy matching (lazy: ~3% for twice the probing).

## Witness
`:: SHOTZIP: raw= deflated= ratio=N.Nx enc_ms= filter=sub chain=32 blocks=fixed-huffman verify=inflate -> PASS|FAIL ::`
(PASS = streaming inflate reproduced length and Adler AND deflated < raw), printed once at the verdict,
before `:: SHOTMOUNT:`. The `-> capturing` line now says `streamed in IDAT chunks of <= 65536 bytes, 0
bytes reserved; stored-size bound N`. The verdict gains `bytes_written=<n> chunks=<n>` in the SECOND
segment, after `dir=`; `<n> bytes -> OK ::` is untouched (scorers key on it).

## Measured (host rustc -O on the exact png.rs, 2880x1800; the metal has not run)
- Synthetic desktop (flat panels, gradient strip, noisy text-like pixels, dark fill): raw 15 553 800 ->
  deflated 2 219 868 (**7.0x**), 34 chunks; python3 `zlib.decompress` of the concatenated IDATs gives
  exactly raw, every chunk CRC checks, no chunk > 64 KiB. A real desktop is flatter than the noise
  here, so 5-10x is the expectation, not a promise.
- Worst case, pure random pixels: 16 402 064 (+5.5%, fixed-Huffman 9-bit literals), 251 chunks,
  verifier agrees. Whole host run incl. generating the image and the verify pass: 159 ms (desktop),
  558-681 ms (noise).
- M3 by reasoning: host ~0.2-0.7 s for the whole loop; an Ivy Bridge at 2.6 GHz is not more than ~3x
  slower per byte than this host's core, giving ~0.5 s typical, ~2 s pessimistic noise ceiling, and the
  chain cap of 32 bounds the per-byte cost (32 compares x 15.5 M bytes worst). `enc_ms` is
  `arch::ms()` deltas around the encode units (1 ms ticks, so +-1 ms per ~30 units of noise; it includes
  IRQ time, excludes the card writes and the verifier). Knobs if it reads long: `MAX_CHAIN` 32 -> 8,
  `HASH_BITS` stays 15.
- Expected card time: ~2.2 MB / 220 KB/s ~ 10 s against 44 s (flatter desktop: less).

## Spec pins
None. `grep PRTSCR specs/*.spec`: no QEMU lane takes a capture (PRTSCR-DIR-FIX/REFUSE are fixtures;
`PRTSCR-ST` is behind `UNAOS_PRTSCRST` and no lane sets it). Metal boot 17+ is the gate.

## Written
Boot 18 should show, on the screenshot verb/key:
`:: PRTSCR: SCREEN0.PNG 2880x1800 … -> capturing (streamed in IDAT chunks of <= 65536 bytes, 0 bytes reserved; …) ::`
`:: SHOTZIP: raw=15553800 deflated=<~1.5-3M> ratio=<5-10>.Nx enc_ms=<<2000> filter=sub chain=32 … -> PASS ::`
`:: SHOTMOUNT: via=vfs … -> PASS ::`
`:: PRTSCR: SCREEN0.PNG 2880x1800 <bytes> bytes -> OK :: source=sdhc … bytes_written=<bytes> chunks=<~25-50> ::`
and the open-the-file time short. Compiler notes: `Phase::Stream` borrows (`chunk` slice across the
`busy_retry` closure, disjoint `st.enc`/`st.buf`), `crate::arch::ms()` on both arches, `Shot.chunks`.
