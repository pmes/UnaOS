# DOORSPLIT — one consumer per serial-door line (rmbp-ledger B634)

Branch `exec-rmbp-doorsplit`, parent `aa3c9c70`. Code files: `unaos/crates/kernel/src/serialdoor.rs`,
`unaos/crates/kernel/src/main.rs` (two key-arm lines, folded line-neutral; line count unchanged).

## The defect (flight 34, f34-boot1.log 15:05:38Z; DOCKSHELL2.md §DOORSPLIT)

The x86 render key arm chose the door's consumer PER BYTE: `if shell_id != WIN_NONE` sent the byte to the shell
window's `shell_console`, otherwise `serialdoor::key` fed the headless `HEAD` console. A shell window that closed
while `tests una` was half typed gave the first bytes to the window and the tail `una` + Enter to the headless
console, which ran `Exec UNA.ELF`. The window teardown (`main.rs:6927`, `shell_console = Console::new()`) also
discarded the window's half of the line.

## Mechanism

- `serialdoor.rs:43` `LINE_AT: AtomicU8` (0 idle, 1 window, 2 headless) with `LINE_WIN` (the window's
  `id << 32 | gen`) and `MIRROR` (the held window's input line).
- `serialdoor.rs:67` `to_window(c, id, wgen, partial)`: the key arm's route decision. A keyboard key (door bit
  clear) answers `id != 0`, as before. A door key (`steer`, `:72`) latches the route on the line's first byte from
  the LIVE shell window (printing `[serialdoor] line route=window|headless at=first-byte win=<id>:<gen> -> held`);
  a window-held line whose window is gone (id 0) or replaced (different id/gen) is moved into `HEAD` by
  `adopt(&MIRROR)` and continues headless; a headless-held line stays headless when a window opens. CR/LF/^C clear
  the latch after the byte is routed, so the Enter goes to the same consumer as the rest of the line.
- `serialdoor.rs:107` `window_saw(line)`: after the window's `handle_key`, the window's `current_input` as it now
  stands is mirrored while a line is held for the window. The mirror, not the torn-down console, is what an
  adoption moves (the teardown at `main.rs:6927` resets the console before the next door byte can arrive).
- `serialdoor.rs:117` `adopt(partial)`: `[serialdoor] adopt from=window bytes=N -> headless`; `HEAD`'s line becomes
  the partial.
- `serialdoor.rs:58` `route_word()`: the held route while a line is open (the `[serialdoor] key=… -> shell(…)`
  witness and `video/lag.rs:232`'s headless timing read it); the backdrop-console arm stays first.
- `main.rs:6821` `if shell_id != WIN_NONE {` became `if serialdoor::to_window(c, shell_id, shell_gen,
  &mut shell_console.current_input) {`; `main.rs:6822` appends `serialdoor::window_saw(&shell_console.current_input);`
  after `handle_key`. The existing `else if take_door() { key(c); }` (`:6827`) is the headless arm, unchanged: a
  door byte `to_window` refuses still carries its door bit there.

## Fixture

`tests doorsplit` (`serialdoor.rs:248`, registered beside `tests door` at `:221`, no serial needed). It feeds
`tests una` + Enter through `steer` with the door bit set, with a simulated window (a local line updated as
`handle_key` would, reported through `window_saw`) and the real headless console (`key`, printable bytes only; the
Enter is steered, never submitted). The latch, mirror and `HEAD` line are saved first and restored after.

- held: window open throughout; the window has the whole line, headless nothing, latch clear after Enter.
- adopt: the window closes after `tes`; headless completes `tests una`, no window byte after the split.
- reverse: begun headless, a window opens after `tes`; headless has the whole line, the window nothing.

Expected: `:: DOORSPLIT: held=ok adopt=ok reverse=ok -> PASS ::`, preceded by three
`[serialdoor] line route=… at=first-byte win=… -> held` lines (window 126:1, window 126:1, headless 0:0) and one
`[serialdoor] adopt from=window bytes=3 -> headless`.

## aarch64 (brief item 5)

The hold does not apply as specified. The door module's state (`serialdoor::imp`) is `x86_64` + `wc` only, and the
door bit comes from `ftdirx::claim_origin` (the x86 xHCI FTDI console). The aarch64 render key arm
(`main.rs:5436`, `windowed = desktop && shell_id != WIN_NONE`, `:5432`) has no door identity: UART and keyboard
bytes are the same `pal::Event::Key`, so there is no "door line" to latch. `arch/aarch64/syscall.rs:7270`'s
`line_watch_check` is the ring-3 console OUTPUT watcher for fixtures, not a key route. The same per-burst split
shape exists there between the shell window and the panel console when the window closes mid-line, for every key,
not only serial. Holding it would change keyboard routing on aarch64, so nothing was changed there.

## Compile

`cd unaos/crates/kernel && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/root/unaos-bench/scratch/cloud/wave32/target cargo
+nightly check -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z
json-target-spec --target ../../x86_64-unknown-none.json --features "$(cat
/root/unaos-bench/scratch/cloud/wave32/kfeats24.txt)"`: rc=0. The first run returned rc=101 because `gen` is a
reserved keyword in edition 2024; the parameters were renamed `wgen` and the second run was green.
