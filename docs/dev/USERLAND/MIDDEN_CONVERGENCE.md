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

### Design: `TERM_RING` — the bounded in-kernel terminal ring

* **Type.** `Channel<TerminalMsg>` where `TerminalMsg` is a fixed-size record — a small inline
  `[u8; N]` plus a length and a kind tag, **not** an `alloc::String`. The reason is the same one
  behind `bootlog`'s ring and the FTDI capture ring: producers include contexts that are IRQ-masked
  or hold the print lock, where an allocation is not permitted.
* **Capacity: 64 records**, matching `GUI_CHANNEL_X86`. Rationale: the consumer is the same render
  task, on the same core, at the same tempo; a deeper ring only buys latency before the same drop.
* **Drop policy: drop-newest, and count.** Never block a producer, never overwrite history. A
  dropped record increments an overflow counter that the ring reports, so a truncated session is
  *visibly* truncated. (This is `selftest.rs`'s existing rule for its boot-replay ring — `try_lock`
  only, drop on contention, count overflow drops — and it is right for the same reason.)
* **Relation to `GUI_CHANNEL_X86`: a sibling, never a replacement, and never nested.** The GUI
  channel carries *input* (`pal::Event`) toward the render task; `TERM_RING` carries *output*
  (terminal messages) toward whatever is rendering the console. Merging them would be a deadlock
  waiting to happen: `dispatch_command` runs **inside** the render task, so a producer that pushed
  onto the channel the render task is blocked on would be pushing into a queue only it can drain.
* **Fan-out.** The Synapse is MPMC; `Channel` is not. That difference is real and does not need
  solving in Ring 0 yet: today there is exactly one consumer (the console window). When a second
  arrives (a log sink, a `TerminalView`), the ring gains a small fixed subscriber array — bounded,
  not a broadcast tree.

**M1 does not build this ring**, and that is deliberate. With one producer and one consumer on the
same core the message goes straight from `midden_core::plan` to `render_message(console, &msg)` —
adding a queue between two statements in the same function would be ceremony, and a ring nobody
drains is a bug factory. The design is recorded here so that M2 (the console window as a real view,
and a second consumer) has a decided answer rather than an invented one.

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
artifact.** Today the numbers are written down at least three times — `arch/x86_64/syscall.rs`,
`arch/aarch64/syscall.rs`, and again by hand inside each EL0 blob (`midden.rs` re-declares its own
`SYS_*` constants and its own register convention). Nothing enforces that they agree; a mismatch is
a silent wrong-syscall, not a compile error.

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
   *not* one on x86, where it must fall through and start `VUG.ELF`.)
1. **Exact match.** `VUG.ELF` typed in full is `VUG.ELF`. Elision can never steal from an exact
   hit, so no file is ever unreachable by its own spelling.
2. **Extension-elided, as typed** — `vug` → `vug.elf`.
3. **Extension-elided, upper-cased** — `vug` → `VUG.ELF`. FAT short names are upper-case on disk;
   this is the arm that actually fires on the boot volume.

Only the **last path component** is elided (`DOCS/vug` → `DOCS/vug.elf`, never `DOCS.ELF/vug`), and
a leaf that already carries a `.` is never re-suffixed — which is what stops `vug.txt` from quietly
launching `VUG.ELF`.

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
  core, and the surviving `other =>` arm means only *"this verb exists but this build does not
  carry it"* — a different fact, given its own sentence.
* **`help`, `echo`, `ver`/`version`, `gneiss` and the empty line moved into the core**, and their
  output reaches the panel through `render_message(console, &Message)` — Ring 0 rendering a
  terminal message off the shared core.
* **Bare-name launch with `.elf` elision** (x86), per §5. `bare_exec` now receives the core's
  resolved on-disk `name` alongside the `typed` word and quotes whichever the reader recognises.
* **Witnesses, both able to fail.**
  * Live, one per dispatched line:
    `:: [midden] cmd="help" -> TerminalOutput len=N ::`,
    `:: [midden] cmd="ls" -> Host verb=ls ::`,
    `:: [midden] resolve "vug" -> VUG.ELF ::`.
  * Boot fixture (`witness` battery, both arches, `shell::midden_witness`): four checks in the
    uniform `:: TSTE: <name> -> PASS/FAIL ::` shape — `midden.dispatch` (a core verb is answered
    in-core with real text), `midden.route` (a host verb is routed with its args intact),
    `midden.resolve` (`vug` → `VUG.ELF`), `midden.precedence` (a verb beats a program of the same
    stem). Each FAIL line prints what it got.
* **The pi4 regression spec gates all of it.** `scripts/specs/pi4-regression.spec` gains five
  `REQUIRE`s (the four fixture verdicts plus the live `resolve "vug" -> VUG.ELF` line) and one
  `FORBID` (`:: TSTE: midden\.\w+ -> FAIL`), taking the gate from **93 to 98 required witnesses**.
  A fixture that merely printed would not be a gate; these fail the build.
* **10 host unit tests** in the core (`cargo test -p midden_core`), including the one that matters
  most: `verb_ness_follows_the_build` — the assertion that `vug` is a verb on aarch64 and a program
  name on x86.

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
