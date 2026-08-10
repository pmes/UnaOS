# Midden convergence — the kernel shell and the shell handler become one

**Status.** Assessment + M1 landed (`shell: one interpreter, and it is midden's`).
**Scope.** What the kernel shell does today, how it splits, what stands in for the Bandy bus in
Ring 0, whether UnaOS userspace needs `std`, and how a program launches without typing `.elf`
while `ls` keeps showing it.

This document exists because
[`docs/dev/OS/05_USER_EXPERIENCE/shell_philosophy.md`](../OS/05_USER_EXPERIENCE/shell_philosophy.md)
§5 already claimed *"Native midden M1 — the framebuffer console is the x86 interactive shell"*,
and it was not true. The framebuffer console was an **unrelated second interpreter**:
`unaos/crates/kernel/src/shell.rs::dispatch_command`, a ~1300-line `match` that split its own
lines, wrote its own help text, and invented `"Unknown command"` in a `_ =>` arm — sharing not one
line of code with `handlers/midden`, the handler whose charter *is* the shell.

**And there was a third.** `unaos/crates/user-blob/src/midden.rs` (581 lines, `no_std`, flat blob,
3 800 bytes on disk) is `MIDDEN.BIN` — a real **EL0 program**, carried on the Pi boot media, that
parses `ls` / `cat` / `cp` / `mv` / `rm` / `write` command text into **typed bus frames**
(`arch/aarch64/bus.rs`), issues them over `SYS_MSEND` / `SYS_MRECV`, and prints the typed replies.
It is gated by `scripts/specs/pi4-regression.spec` (the `BANDY-RT` / `EQ` / `WR` / `EQ2` / `ACL`
witnesses) and has been flying for arcs. It is, almost word for word, Peter's *"some of the simpler
terminal commands are translated by midden directly into the system calls bypassing tradition
app"* — already true, on aarch64, **with no `std` anywhere**. §4 turns on that fact.

The direction is Peter's, 2026-08-08:

> *"we do not want a kernel shell it must be shut down, obviously. some of the simpler terminal
> commands are translated by midden directly into the system calls bypassing tradition app… we
> should also do like other OSes and not make people type .elf. we are following BeOS/BeFS mime
> scheme but i do like the .elf and extensions help distinguish listings in the shell. we need std
> in userspace, no?"*

---

## 1. What the kernel shell actually does today

`dispatch_command` registers **78 verb spellings** (counting aliases). Classified by what they
*are*, not by where they live:

### (a) Thin wrappers over a kernel service — midden should own the parse, the kernel keeps the call

These read one line, call one kernel entry point, and format the answer. The parsing, the usage
text, the flag conventions and the error wording are shell work; only the call is kernel work.

| Verbs | Kernel service behind them |
| --- | --- |
| `ls` `dir` `cd` `pwd` `cat` `type` `head` `tail` `find` `du` `stat` `xd` | `fs::fat::mount` + `resolve_path` / `normalize_path` (x86); `pi_ls_collect` over unafs (aarch64) |
| `touch` `append` `rm` `del` `mkdir` `md` `rmdir` `rd` `cp` `copy` `mv` `move` `ren` `rename` `sync` | the same FAT walk plus the write-through path |
| `vfs` | `MountTable` create/write/truncate/unlink (aarch64 only; x86 has no `VfsBackend` impl) |
| `uls` `ucat` `utouch` `uwrite` `umkdir` `urm` `usnaps` `usnap` `usnapdrop` `usnapls` `usnapcat` | the native unafs volume + its retained-root/snapshot API (aarch64) |
| `netinfo` `ping` `arp` `connect` `udpsend` `get` | the smoltcp socket layer |
| `date` `setdate` `time` `uptime` | `crate::clock` (`unix_now` / `set` / `iso8601_now`) |
| `run` `bg` `jobs` `kill` `storm` | `arch::syscall::{run_user_image, spawn_user_image_bg, bg_poll, bg_kill}` |
| `diskinfo` `usbinfo` `fatinfo` `read` `write` | the block/USB/FAT geometry readers |
| `shutdown` `off` | the platform power path |

**Every one of these is a candidate for Peter's "translated directly into the system calls".** They
are already syscall-shaped; what is missing is that the *caller* should be midden, in userspace,
issuing `sys_*` — not a `match` arm inside Ring 0. That is M3 (see §6), and it is **not** gated on
`std`: `MIDDEN.BIN` already does exactly this for `ls`/`cat`/`cp`/`mv`/`rm`/`write` on the Pi, in
`no_std`, over the typed bus. What M3 needs is the shared command table (this arc), an alloc-free
`parse()` (§2), and the frozen syscall ABI (§4) — not a `std` port.

### (b) Genuine kernel-only machinery — these stay in Ring 0 whatever happens

| Verbs | Why it cannot leave |
| --- | --- |
| `panic` | deliberately enters the kernel's own exception path |
| `clear` | manipulates the `Console` widget's own buffer, not a file or a syscall |
| `sched` `ps` `top` | read per-CPU run-queue meters that exist only inside the scheduler |
| `tste` `selftest` | the in-OS self-test suite (`selftest::run`), including boot-replay of fixtures no userspace can re-run |
| `bootlog` | the boot-milestone ring — a static array written from driver bring-up sites |
| `batmon` | a fresh port-I/O SMC read (x86, `UNAOS_SMC=1`) |
| `vug` `pulse` `v3d` | full-screen kernel-drawn views that take the panel (`took_screen`) |

Note the asymmetry worth keeping: `vug`/`pulse` are registered **only on aarch64**. On x86 the same
word is not a verb at all and falls through to bare-name launch — which is precisely how `vug`
starts `VUG.ELF` on the rMBP. Any command table that ignores that distinction turns a working
launch into a refusal. (§5, and `midden_core::Avail`.)

### (c) Things midden already has, or should simply own

| Verb | Disposition |
| --- | --- |
| `help` | **moved to the core.** Help *is* the command table describing itself; it cannot live anywhere but where the table lives. |
| `echo` | **moved to the core.** Pure string work. |
| `ver` `version` `gneiss` | **moved to the core.** The shell's own identity. |
| `""` (empty line) | **moved to the core** as `Plan::Nothing`. |
| `_ =>` "Unknown command" | **moved to the core.** This was the worst of it: the *decision that a word is not a command* was a fallthrough arm, so the kernel and the handler could never agree on what a command is. |
| `cd` / `pwd` | `handlers/midden` already implements both, host-side, with a held cwd. The kernel holds its own `CWD` static. Two implementations of one concept — reconciled in M2, not M1, because the kernel's cwd is consumed by every FAT verb. |
| `ls` / `touch` | `handlers/midden`'s README documents both as returning `TerminalOutput` / `FileSystemEvent`; both are stubs there and real here. The kernel is the one with a filesystem, so the *implementations* converge downward, not upward. |

---

## 2. The split: what a `no_std` midden core looks like

### What in `handlers/midden/src/lib.rs` is portable, precisely

The file is 216 lines. Host-bound, line by line:

| Item | Binding | Portable? |
| --- | --- | --- |
| `use anyhow::Result` | std | no — but only used in the `BandyMember` impl |
| `use bandy::{BandyMember, SMessage}` | std + Tokio (Synapse is a Tokio broadcast channel) | no |
| `use std::path::{Path, PathBuf}` | std | no — `PathBuf` is the held cwd's type |
| `use std::process::Command` | std, and *host-OS-specific* | **no, and never** |
| `create_view()` (GTK4 `ScrolledWindow`/`TextView`) | `#[cfg(feature = "gtk")]` already | already optional |
| `CommandOutput { stdout, stderr, exit_status }` | pure data | **yes** |
| `Midden::execute(&str)` — tokenize + dispatch on first word | pure logic | **yes** |
| `Midden::change_dir` — join, canonicalize, is_dir | logic + 2 filesystem questions | **yes, behind a trait** |
| `Midden::run_external` — `Command::new().output()` | spawns a host process | **no** — this is the piece that must not converge; on UnaOS the answer is a program load, not a POSIX fork |
| `BandyMember for Midden` (a `println!` stub) | std | no |

So the honest reading: **`execute` is nearly host-free already**, exactly as the brief says — the
tokenizer and the dispatch are pure, and the only two real dependencies are (i) a filesystem
question (`is_dir`, `canonicalize`) and (ii) a way to run a program. Everything else is either
already feature-gated (GTK) or is a stub (`BandyMember`).

The blocker is not `execute`. It is that `handlers/midden` and `shell.rs` had **different command
tables**, so there was nothing to extract *to*.

### The third implementation, and the constraint it imposes

`unaos/crates/user-blob/src/midden.rs` already parses a command line in `no_std`, at EL0, and turns
it into syscalls. It is the most advanced piece of the convergence and it is **not** a candidate to
link `midden_core` today, for a reason worth writing down because it shapes the core's future API:

> *"flat blob <= one 4 KiB code page, fully position-independent, .text+.rodata only (NO
> .data/.bss — no mutable statics), and NO panic paths: every buffer access below goes through
> raw-pointer helpers with statically-sound bounds, because a single reachable `core::panicking`
> call drags the fmt machinery into a blob with a 4 KiB budget."*
> — `midden.rs`, build discipline

`midden_core` is `no_std` but **not alloc-free**: `plan()` returns owned `String`s and `help()`
builds a `Vec<String>`. That is right for the kernel console (which has a heap) and wrong for a
3 800-byte flat blob. The fix is not to make the blob bigger; it is to give the core a **second,
allocation-free entry point** — a borrowing `parse(&str) -> Parsed<'_>` that names the verb and the
argument spans without owning anything, with today's `plan()` layered on top of it. Then all three
sites share one table: the kernel console, the EL0 program, and the host handler. That is the
single highest-value follow-on this assessment identifies, and it is small.

### Where the core belongs, under the tree's own convention

`docs/dev/USERLAND/ARCHITECTURE.md` §1 states the convention already:

> Ring 0-embeddable cores live under `unaos/libs/` in device-class subdirs — `no_std`,
> `forbid(unsafe_code)`, pulled by the kernel with `default-features = false`: `fs/` (the UnaFS
> format core both rings share), `input/` (ibus RC-receiver decode), `pwm/` (actuators), and
> `sys/helm/` (the safety interlock — system authority, not a device class).

The shell is system authority, not a device class — the same reason `helm` is under `sys/`.
Therefore:

```
unaos/libs/sys/midden_core/          # crate `midden_core`
```

`no_std`, `forbid(unsafe_code)`, **zero dependencies** (not even our own crates), a member of the
ROOT host workspace, pulled by the kernel as a path dep with `default-features = false`. Crate name
is `midden_core` and not `midden` only because Cargo requires unique package names within a
workspace and `handlers/midden` holds that name.

> **Doc drift found while checking this.** ARCHITECTURE.md §1 lists `input/` and `pwm/` as living
> under `unaos/libs/`. On disk they are at `libs/input/ibus` and `libs/pwm/pca9685` (root
> workspace), and `unaos/libs/` holds only `fs/` and `sys/`. The *rule* is right; two of the four
> examples name the wrong path. Not fixed here (outside this arc's files) — flagged for the
> integrator.

### The split, as implemented (M1)

The core answers exactly one question — *what does this line mean?* — and returns a `Plan`:

| `Plan` | Meaning | Who acts |
| --- | --- | --- |
| `Nothing` | empty line | nobody |
| `Say(Message)` | the core answered in full | the ring renders the message |
| `Exec { typed, name }` | a bare name resolved to a program on disk | the ring loads and runs it |
| `Host { verb, rest }` | a verb the core knows, only the ring can perform | the ring's service layer |

The core holds: the **command table** (`CORE_VERBS`, `HOST_VERBS` with per-verb `Avail`), the
**parser**, the **bare-name resolver**, the **help text**, and the **refusal wording**. It holds no
`#[cfg(target_arch)]` at all — a shell core that only tells the truth on one arch is not a shared
core — so the build describes itself to the core through a `Facts` struct that the kernel fills in
with `cfg!()` at one call site.

`Plan::Host` is the honest boundary marker: the kernel still *performs* `ls`/`cat`/`ping`/`storm`,
but it no longer *decides*. There is exactly one command table, and it is midden's.

---

## 3. The bus: what stands in for Synapse in Ring 0

`bandy::Synapse` is a thin wrapper over a **Tokio broadcast channel, buffer 1024**. Ring 0 has no
Tokio, no async runtime, and no heap budget for a 1024-deep MPMC ring on the x86 GUI path.

**The kernel already has the right primitive, and it is not a new one.** `GUI_CHANNEL_X86`
(`main.rs`) is a `arch::sched::Channel<pal::Event>` — a **64-slot** bounded channel between the USB
pump (producer) and `x86_render_service` (consumer, which blocks on `.recv()`). It already carries
the discipline this needs, including the rule that a full-screen command parks the render task
inside `dispatch_command` so nothing may be pushed while it cannot drain, and a depth witness
(`sent - recv`).

### `TERM_RING` — the bounded in-kernel terminal transport (BUILT, M2)

**M1 did not build this ring, and that was deliberate.** With one producer and one consumer on the
same core the message went straight from `midden_core::plan` to `render_message(console, &msg)`;
adding a queue between two statements in the same function would have been ceremony, and a ring
nobody drains is a bug factory. **M2's TERM_RING arc built it** — `crates/kernel/src/termring.rs`,
with the drain wired at `main.rs`'s `handle_key` and a witness on the pi4 chain. What follows is
what exists, not what was planned; where the built thing diverges from M1's sketch, the divergence
and its reason are stated rather than quietly edited away.

* **Role: a TRANSPORT, not the scrollback.** The ring carries lines from a producer toward the task
  that owns the console view; `Console::history` remains the display store. This split is what makes
  the drop policy below correct. Drop-newest is right for a transport — the backlog is the symptom,
  and the newest line is the one the producer could not afford to wait for — and exactly wrong for a
  scrollback, which would then stop showing the present. Two roles, two objects, one policy each:
  `termring` drops newest, `Console::place` drops oldest.
* **Type: `serial_ring::LineRing<64, 240>`, NOT `arch::sched::Channel<TerminalMsg>`.** This is the
  one place the build diverges from the M1 sketch above, and the reasons are ones this very section
  supplies without having drawn the conclusion. `Channel` has no `try_send`; its buffer is a
  *sleeping* `Mutex<VecDeque>`; and its `send`/`recv` assert they are running on a scheduled task.
  Each of those is fatal to the producer contexts this section itself names — IRQ-masked code and
  code holding the print lock may not sleep, may not allocate, and may not be on a task at all — and
  a blocking `send` from inside `dispatch_command` would push onto a queue only the blocked render
  task can drain, which is the deadlock the "sibling, never nested" bullet warns about. `LineRing`
  (`serial_ring.rs`, SERWIT-1's machinery made reusable) is the same 64-slot bound with none of it:
  three atomics plus per-slot atomics, no Mutex, no allocation, no reentrancy into `serial_println!`,
  const-constructible as a `static`, and a `Staged::Full` return that hands the caller a *counted
  refusal* instead of a wait. The record is still fixed-size and inline — `[u8; 240]` plus a length —
  for exactly the reason the sketch gave.
* **Capacity: 64 records**, matching `GUI_CHANNEL_X86`, unchanged from the M1 ruling: the consumer is
  the same render task, on the same core, at the same tempo, so a deeper ring only buys latency
  before the same drop. 240 bytes per record is comfortably wider than a panel row at any scale this
  kernel drives (1920 px at the scale-2 cell is 120 cells); a longer line is sealed in place with
  `serial_ring::TRUNCATION_MARK` and counted as a tear, never silently shortened.
* **Drop policy: drop-newest, and count.** Never block a producer, never overwrite history. The
  ledger is a `serial_ring::TapCounters` (`termring::TERM_TAP`) under the same conservation law the
  four SERWIT-2 mirror taps satisfy —
  `submitted == absorbed + dropped + suppressed + in_flight` — and `termring::service()` announces
  un-announced loss on the wire as a `:: termring: … == witness ::` line, on change only
  (`take_pending` self-rate-limits, so a lossless ring prints nothing at all). `TERM_TAP` is
  deliberately *not* enrolled in `serial_ring::taps()`: that array is the census of taps on the
  serial wire, and this ring is not one of them.
  * *Correction to the M1 text.* The parenthesis above used to cite `selftest.rs`'s boot-replay ring
    as "`try_lock` only, drop on contention, count overflow drops". That has not been true since
    **SERWIT-2**: the replay ring's `Mutex<BootRing>` is gone (`selftest.rs`, `BootSlots`). A writer
    claims its index with one `fetch_add` and publishes the slot with one release store, so there is
    no lock to contend and no contention-drop path at all — the only loss left is the ring genuinely
    filling, derived from the monotonic claim counter. `termring` follows *that* rule, the lock-free
    one, not the one the old text described.
* **A transport drop costs ORDER, not content.** `Console::println` stages through the ring and then
  drains it, because on today's surfaces the producer runs *on* the consumer's task. If the ring
  refuses a record, the console places the line directly in `history`, so an operator never loses
  output; the record is still charged as `dropped`, because what the transport lost is real — that
  line's FIFO position relative to records still in flight. Counting it keeps the ledger honest
  about a transport that could not carry its offered traffic; the fallback keeps the panel complete
  while it is.
* **The drain site.** `main.rs`'s `handle_key`, immediately after `dispatch_command` returns and
  immediately before the post-command `console.draw(pal)`: the first moment the render task owns the
  view again, which is the exclusive-drainer contract `LineRing::drain` requires. It runs
  unconditionally (including for a `took_screen` command, which has already restored the console),
  followed by `termring::service()`. Both are no-ops on an empty, lossless ring — which is what
  makes the aarch64 path, where the same `handle_key` is the BSP GUI loop's, byte-identical.
* **Relation to `GUI_CHANNEL_X86`: a sibling, never a replacement, and never nested.** Unchanged and
  now structural: the GUI channel carries *input* (`pal::Event`) toward the render task; `TERM_RING`
  carries *output* away from producers toward whoever renders the console. They are different types
  on purpose — the input channel may block a USB pump; the output transport may block nobody.
* **Fan-out.** Still one consumer (the console view), still not a broadcast tree. When a second
  arrives (a log sink, a `TerminalView`), the ring gains a small fixed subscriber array. Nothing in
  the built shape forecloses that: `drain` is already a `FnMut(&str)` over whole records.
* **The witness.** `termring::termring_selftest` (`witness`-gated, wired on the pi4 chain in
  `arch/aarch64/syscall.rs`, `REQUIRE`d by `scripts/specs/pi4-regression.spec`) parks the consumer,
  offers 80 records, and asserts four independently-failable properties: the bound and the refusal
  (exactly 64 accepted, 16 refused, 64 in flight); drop-NEWEST with order and byte round-trip (the
  survivors are sequences `0..64` — drop-OLDEST would return `16..80` and fail the first comparison);
  truncation sealed with `TRUNCATION_MARK` and counted; and the conservation law over the whole
  fixture. Every count is decoded onto the verdict line, so the gate cannot be satisfied by a leg
  that merely printed.

---

## 4. Does UnaOS userspace need `std`? — the answer for Peter

**Start from what is already true: UnaOS runs a userspace program that translates shell commands
into syscalls, today, with no `std` at all.** `MIDDEN.BIN` does it on every Pi boot. So `std` is not
what stands between UnaOS and "midden in userspace" — that already works. What `std` buys is
narrower, and more valuable, than capability: it buys **handler and vessel reuse**.

**Short answer: yes, eventually, and it is the right end state — but not next, and not as one
project.** The recommendation is to *keep* the `no_std` cores as the convergence mechanism and
schedule `std` as a deliberate, separately-funded arc once there is a stable syscall ABI to build
it on. Concretely: **do not start `std` this quarter; do start the thing that makes `std` cheap.**

### What porting `std` to a `*-unaos` target actually requires

This is not a switch. Rust's `std` on a new OS needs, in rough dependency order:

1. **A target specification** — `x86_64-unknown-unaos` / `aarch64-unknown-unaos` JSON specs, then
   (for anything sustainable) a tier-3 target upstreamed into `rustc`, because out-of-tree specs
   need `-Z build-std` and a nightly toolchain forever.
2. **A stable syscall ABI.** `std` is a *client* of the OS; today UnaOS's syscall surface is still
   moving (`sys_write`, `sys_exit`, `sys_spawn`, `sys_wait`, the socket family). Porting `std`
   against a moving ABI means porting it repeatedly.
3. **A libc, or a std shim.** Two real options:
   - **Port a libc** (a `newlib`/`relibc`-shaped effort) and let `std` sit on it. Large, and it
     drags in a C toolchain and a POSIX shape UnaOS has explicitly said it does not want to layer
     ("gneiss gets inside the machine rather than layering a foreign userspace on top").
   - **Write `library/std/src/sys/pal/unaos`** directly — Rust's platform abstraction layer, the
     same slot `hermit`, `wasi` and `uefi` occupy. No libc, straight to UnaOS syscalls. This is
     the right shape for UnaOS and is roughly a dozen modules.
4. **The PAL modules themselves**, each gated on a kernel capability. Checked against the tree
   rather than assumed — and UnaOS is further along here than it looks:
   * `alloc` — **have it** (the kernel's global allocator).
   * `fs` — **mostly have it.** There is a real object/handle table on **both** arches: `SYS_OPEN`
     by name returning a handle, `SYS_READ` / `SYS_WRITE` / `SYS_SEEK`, per-handle capabilities and
     ACLs, `O_CREAT`, a reserved `CONSOLE_FD`, and cross-process capability transfer (U6 / U6b /
     U7 / U9 / U10, all spec-gated). What `std::fs` additionally needs is directory enumeration
     through a handle and file metadata — an increment, not a foundation.
   * `process` — **partially there**: `sys_spawn` returns a handle, `sys_wait` reaps.
   * `stdio` — effectively there via `CONSOLE_FD`.
   * `net` — needs a socket handle model over the existing smoltcp layer.
   * `time`, `os`/`env`/`args` — small.
   * `thread` + `thread_local`, `sync` — **the real gap**: TLS and a userspace thread primitive
     plus a futex-equivalent. This is the expensive half, and it is kernel work, not `std` work.
   * unwinding/panic — see below.
5. **Unwinding.** Either ship `panic=abort` everywhere (cheap, and honest for a young OS) or port
   the unwinder. Recommend `panic=abort` initially and say so out loud.

**Rough size.** Because the handle table already exists, a first `pal/unaos` that runs "hello world
+ read a file + spawn a process" is a **multi-arc effort, on the order of 4–8 focused arcs** — and
most of that is target-spec and toolchain plumbing, not new kernel work. A `std` complete enough
that *handlers and vessels build unmodified* — the actual goal — additionally needs threads, TLS,
sync and net, and is realistically **a quarter of work**, the bulk of it kernel-side (TLS, futex,
socket handles) rather than in `std` itself. Without a frozen ABI first, double it and expect
rework.

### What it buys

Exactly the thing the convergence is for, and *only* that thing: **handlers and vessels run
unmodified.** Not new capability — `MIDDEN.BIN` proves the capability exists without it — but the
end of writing everything twice. `midden`,
`vein`, `junct`, `tabula` and the vessels are ordinary Rust programs today. With `std` on UnaOS
they become ordinary Rust programs *on UnaOS* — no second implementation, no `no_std` rewrite per
handler, no divergence between "the host version" and "the real version". That is the end state,
and nothing else reaches it.

### The alternatives, honestly

| Option | What it gives | What it costs |
| --- | --- | --- |
| **A. `no_std` shared cores + a std-only host shell** (today, and what M1 extends) | Immediate. Every core is shared by construction. `unafs`, `helm`, and now `midden_core` prove the pattern works — and `MIDDEN.BIN` proves a `no_std` program can drive real syscalls at EL0. | Only the *portable* half of each handler converges. `run_external`, GTK, Tokio and anything else host-shaped stays behind. The system is real but the handlers are not yet running on it. |
| **B. A UnaOS-specific portability layer — `gneiss_pal` pointed at UnaOS** | `libs/gneiss_pal` is *already* this: "The Ring 3 kernel: filesystem, networking, geometry, DSP, windowing, paths, persistence. Prevents handlers re-implementing host services." Handlers that go through `gneiss_pal` instead of `std` directly become portable to UnaOS by adding one backend, not by porting `std`. | Only works for handlers that actually route through it; every direct `std::fs` / `std::process` call is a hole. Needs an audit and a discipline ("handlers call gneiss, never std"). |
| **C. Port `std`** | Handlers and vessels unmodified. | The quarter above, and a nightly/tier-3 toolchain commitment. |

### Recommendation

**B now, C later, and A continuously.** In order:

1. **Keep extracting `no_std` cores** (A) — it is free convergence and it is already the tree's
   convention. `midden_core` is the third. **First follow-on: the alloc-free `parse()` entry point
   described in §2**, so `MIDDEN.BIN` shares the table too and the interpreter count goes from three
   to one for real.
2. **Make `gneiss_pal` the law** (B) — audit handlers for direct `std::fs`/`std::process`/`std::net`
   use, route them through `gneiss_pal`, and add a `platforms/unaos` backend to it the way
   quartzite already reserves one. This is the cheapest path to "handlers run on UnaOS", it needs
   **no toolchain work at all**, and every hour spent on it is an hour `std` will not have to
   re-do — the PAL surface it forces you to define *is* the syscall surface `std` will need.
3. **Then `std`** (C), once the ABI stops moving.

**The first step is not any of that, and it is smaller than it sounds.** It is the one prerequisite
both B and C need and neither can fake: **make the syscall ABI a single, versioned, shared
artifact.** Today the numbers are written down in **six or more places, and they have already
diverged** — this is a live defect, not a tidiness argument. The two kernel dispatchers
(`arch/x86_64/syscall.rs`, `arch/aarch64/syscall.rs`) each hold a list, and then **every EL0 blob
re-declares its own** `SYS_*` constants and its own register convention by hand: user-stat,
user-pulse, user-blob/`midden.rs`, user-vug. Nothing enforces that any of them agree, and they do
not — aarch64 defines `SYS_REPORT=3`, `SYS_GETPID=6`, `SYS_MSEND`/`SYS_MRECV`=19/20,
`SYS_FB_MAP`/`SYS_FB_PRESENT`=24/25, `SYS_WIN_MOVE=31` and `SYS_WIN_CLOSE=32`, none of which x86
has; x86 adds `SYS_WIN_PRESENT_ROWS=33` and `SYS_CPUPULSE=49`, which aarch64 lacks. A blob compiled
against the wrong list does not fail to build; it issues a **silently wrong syscall**. That makes
the shared-ABI crate below the highest-value small arc in this document, and it is scoped as its own
arc rather than folded into the shell work.

The fix is a small `no_std` crate — `unaos/libs/sys/abi`, exactly the convention this arc used for
`midden_core` — holding the syscall numbers, the argument/return conventions, the error numbers and
the handle/capability flags, consumed by the kernel dispatcher **and** by every userspace program.
It is perhaps a day's work, it removes a live class of bug immediately, and it is the artifact a
`gneiss_pal` UnaOS backend binds to and that `library/std/src/sys/pal/unaos` would be written
against. Freeze it, version it, then build upward. Everything else in this section is cheaper once
it exists and more expensive if it does not.

---

## 5. The `.elf` question — launch without it, list with it

Peter wants both halves, and both halves are reasonable: *don't make people type `.elf`*, and
*extensions help distinguish listings in the shell*. They are only in tension if you assume the
extension is the mechanism. It is not — it is a **name**, and BeOS proved you can keep the name and
still not need it.

### The rule: elision is a property of RESOLUTION, never of storage or display

Nothing is renamed. Nothing is hidden. `ls` prints `STAT.ELF` because the file is called
`STAT.ELF`. Only the *lookup* is allowed to try the suffix.

### Resolution order for a bare word (implemented, `midden_core::resolve_exec`)

0. **Is it a verb on this build?** If yes, it is the verb — full stop. Precedence is absolute and
   needs no tie-break rule. (`Avail` makes "on this build" real: `vug` is a verb on aarch64 and is
   *not* one on x86, where it must fall through and start `VUG.ELF`.) The question is asked
   **case-insensitively**, through `midden_core::canon_verb` — see "The case rule" below, which is
   part of this same precedence and not a separate nicety.
1. **Exact match.** `VUG.ELF` typed in full is `VUG.ELF`. Elision can never steal from an exact
   hit, so no file is ever unreachable by its own spelling.
2. **Extension-elided, as typed** — `vug` → `vug.elf`. **On the kernel this is the arm that fires.**
   `FatVolume::is_file` walks FAT with `eq_ignore_ascii_case` (short names are stored upper-case, so
   the walk has always had to), which means the probe for `vug.elf` MATCHES the on-disk `VUG.ELF`
   and the resolver returns the string `"vug.elf"`.
3. **Extension-elided, ASCII-upper-cased** — `vug` → `VUG.ELF`. **Latent, not live.** Arm 2 always
   wins on any case-insensitive `Volume`, which is every volume the kernel has today; this arm is
   reached only by a case-SENSITIVE backend — the boot fixture's exact-match `NameList`, a future
   UnaFS mount, a host `std::fs` implementation. It is kept because it makes those backends behave
   like the FAT one for one extra probe, and it is documented as latent so that no gate is ever
   written against a spelling the kernel does not emit. (Consequence, and it is the reason this
   paragraph exists: the live x86 serial line is `:: [midden] resolve "vug" -> vug.elf ::`. The
   `-> VUG.ELF` spelling belongs to the fixture alone. The genuine on-disk name is available one
   layer down as `bare_exec`'s `canon`, read back out of the directory entry.)
   The uppercase transform is `to_ascii_uppercase`, deliberately: Unicode casing changes a token's
   length (`ß` → `SS`, `ﬁ` → `FI`), and a resolver must not probe for names the user never typed.

Only the **last path component** is elided (`DOCS/vug` → `DOCS/vug.elf`, never `DOCS.ELF/vug`), and
a leaf that already carries a `.` is never re-suffixed — which is what stops `vug.txt` from quietly
launching `VUG.ELF`.

### The case rule: verbs win in ANY case, and that is a security property

The FAT resolver underneath has always been case-insensitive. So if the verb table were consulted
case-**sensitively**, typing `LS` — with caps lock on, or simply mirroring the 8.3 spelling `ls`
itself prints — would miss the table, fall through to resolution, probe `LS.elf`, match a dropped
`LS.ELF` case-insensitively, and **launch it**. The same for `RM`, `Cat`, `Kill`, `Write` and the
capital variant of all 78 verbs. That is precisely the shadowing this section calls a security
problem, reachable through the one spelling the rule did not cover.

So `is_verb` and `plan` both look the word up through `canon_verb` (ASCII lower-case), and
`Plan::Host` carries the **canonical** verb outward — the ring's service `match` arms are lower-case
literals, and handing back a raw `LS` would land in the `other =>` arm and print "the verb exists;
this kernel does not carry it" about a verb this kernel carries. Only the verb is folded: arguments
and `Plan::Exec`'s `typed` keep the user's own spelling, because a file name is not a verb name.
Pinned by `a_verb_wins_in_any_case_and_arrives_canonical` (`LS`/`Ls`/`lS`/`ls` all dispatch as `ls`
against a volume that does carry `LS.ELF`).

**The collision, stated rather than papered over.** `stat` is a verb *and* `STAT.ELF` is staged on
the volume. Under rule 0 the verb wins, so a bare `stat` will not launch `STAT.ELF`; it is reachable
as `STAT.ELF`, `stat.elf`, or `run ./STAT.ELF`. This is the correct trade (a shell where a dropped
file can shadow `ls` is a security problem, not a convenience), it is pinned by a boot fixture
(`midden.precedence`) so a later "improvement" cannot land it quietly, and it is the one place
Peter's example and the implementation differ — deliberately.

### BeOS/BeFS MIME on a filesystem with no attributes

BeFS carried the MIME type as a **file attribute** (`BEOS:TYPE`), written at creation and sniffed
on demand. FAT has no attribute store at all. The options, cheapest first:

| Option | How | Cost | Verdict |
| --- | --- | --- | --- |
| **Sniff on open (ELF magic)** | The kernel *already reads* the first bytes and checks `\x7FELF` before every launch (`bare_exec`). "Is this executable" is answered from content, at the only moment it matters. | Zero — it is already there. | **Recommended first step.** |
| **Extension → type map** | A static table in `midden_core` (`.elf` → `application/x-executable`, `.txt` → `text/plain`, …) for the cases where content sniffing is not worth a read (a listing of 400 files). | Tiny. Already half-built: `EXEC_EXTS`. | Recommended second step, for `ls`. |
| **Sidecar file** (`.types` per directory, or `FILE.ELF.type`) | Store what FAT cannot. | Doubles the entry count, breaks on any copy that does not know about it, and is invisible to every other OS that touches the card. | **No.** |
| **UnaFS attribute feature** | Give UnaFS what BeFS had — real named attributes on an inode, including `BEOS:TYPE`. UnaFS is ours and already has snapshots and ACLs; attributes are in its natural grain. | A real UnaFS arc. | **Yes — this is the right long-run home**, and it is why the MIME scheme should be designed against UnaFS, not FAT. FAT gets sniff + extension-map and that is honestly all it can support. |

**What `ls` shows.** Unchanged: the real on-disk name, `STAT.ELF`, extension visible — that was
Peter's explicit request and it costs nothing. The type, when the extension-map or a UnaFS
attribute makes it available, belongs in `ls -l` as its own column, next to size and mtime — not as
a decoration on the name. Names stay names.

**Smallest correct step (done in M1):** extension-elided *resolution* only, `.elf` only, with `ls`
untouched. No type store, no sidecar, no format change. The MIME scheme proper waits for UnaFS
attributes, where it can be done the way BeFS did it rather than faked.

---

## 6. What M1 implements, and what it defers

### Implemented

* **`unaos/libs/sys/midden_core`** — `no_std`, `forbid(unsafe_code)`, zero dependencies, root
  workspace member, pulled by the kernel with `default-features = false`. Holds the command table
  (`CORE_VERBS` + `HOST_VERBS` with per-verb `Avail`), the parser (`plan`), the bare-name resolver
  (`resolve_exec`), the help text (`help`), the `Message` type (variant-for-variant with
  `bandy::SMessage`'s terminal arm), and the `Volume` trait — the single filesystem question
  resolution is permitted to ask.
* **The kernel calls THROUGH it.** `dispatch_command`'s first act is `midden_core::plan(...)`. The
  old `_ =>` fallthrough is gone: an unknown word is now a `Message::TerminalError` produced by the
  core. The surviving `other =>` arm is a **drift net, and it is unreachable today** — every `Avail`
  in `HOST_VERBS` mirrors the `#[cfg]` on its match arm exactly (checked arm by arm across all 78
  spellings), so the set of "verbs this build cannot perform" is empty and nothing reaches it. It is
  kept because that agreement is a hand-maintained invariant across two files with no compiler check
  behind it (the match is over `&str`, so a missing arm is not a non-exhaustive-match error): add a
  verb to the table and forget its arm, or narrow an arm's `cfg` without narrowing its `Avail`, and
  the drift surfaces there as a named sentence on the panel instead of as a word that silently does
  nothing. No example is given at the arm on purpose — the obvious candidates are all wrong (`uls`
  on x86 is `Avail::Aarch64` and never becomes a `Host` plan; `top` has no `cfg` at all and prints
  its own aarch64-only message from inside its arm; `vug`'s `Avail` tracks the v3d `cfg`) — and a
  case that genuinely reaches it would be a **bug in the table**, not a documented behaviour.
* **`help`, `echo`, `ver`/`version`, `gneiss` and the empty line moved into the core**, and their
  output reaches the panel through `render_message(console, &Message)` — Ring 0 rendering a
  terminal message off the shared core.
* **Bare-name launch with `.elf` elision** (x86), per §5. `bare_exec` receives the core's chosen
  `name` alongside the `typed` word and quotes whichever the reader recognises. `name` is the
  spelling the resolver LOADS, **not** the on-disk spelling — on FAT those differ (§5, arm 2); the
  real 8.3 name is `canon`, read back from the directory entry by the re-resolve, and that is what
  the serial refusal lines quote.
* **Witnesses, both able to fail.**
  * Live, one per dispatched line:
    `:: [midden] cmd="help" -> TerminalOutput len=N ::`,
    `:: [midden] cmd="ls" -> Host verb=ls ::`,
    `:: [midden] resolve "vug" -> vug.elf ::` (the as-typed arm; see §5 for why the on-disk
    `VUG.ELF` is not what this line carries).
  * Boot fixture (`witness` battery, both arches, `shell::midden_witness`): four checks in the
    uniform `:: TSTE: <name> -> PASS/FAIL ::` shape — `midden.dispatch` (a core verb is answered
    in-core with real text), `midden.route` (a host verb is routed with its args intact),
    `midden.resolve` (`vug` → `VUG.ELF` against the fixture's exact-match `NameList`),
    `midden.precedence` (a verb beats a program of the same stem). Each FAIL line prints what it
    got.
* **Both regression specs gate it, on both arches.** `scripts/specs/pi4-regression.spec` gains four
  `REQUIRE`s (the fixture verdicts) and one `FORBID` (`:: TSTE: midden\.\w+ -> FAIL`), taking the
  gate from **93 to 97 required witnesses**; `scripts/specs/x86-witness.spec` gains the same four
  and the same `FORBID`, because `midden_witness` is called under
  `#[cfg(all(target_arch = "x86_64", feature = "witness"))]` too and was printing four ungated PASS
  lines on the rMBP. Deliberately NOT required on either side: the fixture's companion echo
  `:: [midden] resolve "vug" -> VUG.ELF ::`. It restates the `midden.resolve` verdict over the same
  comparison (arithmetic, not coverage) and its spelling is the fixture's, not the live shell's.
* **12 host unit tests** in the core (`cargo test -p midden_core`), and they are a GATE:
  `./arroyo check` runs them after the userspace legs, so the table cannot drift behind a green
  compile. Two carry the most: `verb_ness_follows_the_build` (`vug` is a verb on aarch64 and a
  program name on x86) and `a_verb_wins_in_any_case_and_arrives_canonical` (§5's case rule — `LS`
  lists the directory even with `LS.ELF` on the volume).

### Deferred, named

* **The kernel's ~1300-line service `match` stays.** It performs the verbs; it no longer decides
  them. Moving the *implementations* to userspace is M3, whose real prerequisites are the alloc-free
  `parse()` (§2) and the frozen syscall ABI (§4's first step) — not `std`.
* **`cd`/`pwd` remain kernel-side.** The kernel's `CWD` static is read by every FAT verb; unifying
  it with `handlers/midden`'s held `PathBuf` is M2.
* **`handlers/midden` is not yet rewired onto the core.** M1 built the core and converted the
  kernel — the ring that had no shell. Converting the handler is mechanical (`Plan` → `SMessage`,
  four variants) and belongs with the M2 view work, so the two land together rather than leaving
  the handler half-migrated.
* **`MIDDEN.BIN` (the EL0 program) is untouched.** It keeps its own hand-rolled parser until the
  core grows the alloc-free `parse()` described in §2 — linking today's `alloc`-using core into a
  3 800-byte panic-free flat blob would blow its budget, which is a worse outcome than three
  parsers for one more arc.
* **`TERM_RING` is designed (§3) but not built** — one producer, one consumer, same core.
* **No `std` port**, no MIME store, no new apps, no compositor work.

---

## See also

* [`shell_philosophy.md`](../OS/05_USER_EXPERIENCE/shell_philosophy.md) §5 — the claim this arc makes true.
* [`ARCHITECTURE.md`](ARCHITECTURE.md) §1–2 — handlers, the `unaos/libs/` core convention, Bandy.
* [`../OS/02_KERNEL_CORE/userspace.md`](../OS/02_KERNEL_CORE/userspace.md) — the ring-3 model and the loader `run`/`bg` reach.
* `unaos/libs/sys/midden_core/src/lib.rs` — the core, and the rationale in its doc comments.
* `handlers/midden/README.md` — the handler's documented command surface.
* `unaos/crates/user-blob/src/midden.rs` — `MIDDEN.BIN`, the EL0 program that already turns command
  text into typed bus frames and syscalls, and `unaos/crates/kernel/src/arch/aarch64/bus.rs` — the
  typed wire it speaks.
