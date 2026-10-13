# SHELLABI — the ring-3 shell's ABI (rmbp-ledger B612)

Arc base `c4c99344`, branch `exec-rmbp-shellabi`. Compiled only (R107); no QEMU run, no bench run. Every wire line
below is the shape the code prints; none is quoted from a capture. The fold's `tests midden` is the first run.

## Syscall table (una-abi `crates/una-abi/src/lib.rs`, RING3PIPE + SHELLABI block)

| nr | name | arguments | answer | x86_64 | aarch64 |
|---|---|---|---|---|---|
| 68 | `SYS_SPAWN_PATH` | path_ptr, path_len, argv_ptr, argc \| `SPAWN_PIPE` | child pid / -errno | dispatched | `-ENOSYS` (default arm) |
| 70 | `SYS_PIPE_READ` | pid, buf_ptr, buf_len | n > 0 bytes; 0 EOF; -EAGAIN empty; -EBADF no pipe; -EACCES not the launcher; len 0 = dropped count | dispatched | `-ENOSYS`, `[pipe] read … -> refused=no-launch-pipe` |
| 71 | `SYS_PIPE_WRITE` | pid, buf_ptr, buf_len | n > 0 bytes taken (at most one 32-event input ring per call), each a `INPUT_EV_KEY_DOWN` on the child's ring; len 0 = one `KEY_EOT` (0x04), answers 0; -EAGAIN ring full or the child holds the focus; -EPIPE the child exited; -ENOSYS no live pipe with that pid; -EACCES another program's pipe | dispatched (`sys_pipe_write`) | `-ENOSYS`, `[pipe] write … -> refused=no-launch-pipe` |
| 72 | `SYS_KILL` | pid | 0 killed (or its kill already armed); -ESRCH no pipe with that pid, or the child exited; -EACCES another program's pipe | dispatched (`sys_kill`, reaped by `bg_kill`) | `-ESRCH`, `[kill] pid=… by=<asid> -> refused=not-my-child` |

`EPIPE` = -32 is new in una-abi. The trust gate for 71 and 72 is the pipe: only the `(slot, SLOT_GEN)` that launched
the child with `SPAWN_PIPE` holds it, the same capability 70 checks.

## Key bytes (una-abi `KEY_*`, folded by `drivers/xhci/mod.rs::hid_key_ascii`, shared by EHCI)

| byte | constant | key | release |
|---|---|---|---|
| 0x8E | `KEY_CTRL_ENTER` | Ctrl + main-row Return (usage 0x28) | bare Enter (`\n`) |
| 0x8F | `KEY_PGUP` | PageUp (usage 0x4B), Shift or not | 0x8F |
| 0x90 | `KEY_PGDN` | PageDown (usage 0x4E), Shift or not | 0x90 |

Shift+PgUp/PgDn still deliver the keymap's ScrollPageUp/Down action beside the byte. No C0 byte was free (arrows
0x1C..=0x1F, Esc, BS/Tab/LF/CR, Ctrl+letter 0x01..=0x1A), so PgUp/PgDn take the chord range after 0x8E.

## Wire lines

Kernel (x86):

- `[pipe] write pid=<p> row=<k> -> refused=no-pipe|not-the-launcher|child-exited`
- `[kill] pid=<p> by=<k> -> refused=not-my-child`
- `[pipe] kill pid=<p> by=<k> -> ok (<bg_kill's words, e.g. killed, row reaped>)` or `-> refused=exited (…)`
- `[wc] tab -> app pid=<p> owner=<slot+1>` (a bare Tab while a live ring-3 program has the keys: the program gets it)
- `[wc] tab -> cycle cur=<c> next=<n> ring=<r>` (the shell or a dead focus: the cycle, beside `[wc-c] focus tab-cycle`)

MIDDEN.ELF:

- `:: MIDDEN: pipe feed pid=<p> bytes=<n> ok=1 ::` (a later stage's input delivered, its KEY_EOT taken) or `ok=0 err=<e>`
- `:: MIDDEN: pipeline stages=<n> last=<program as typed> rows=<r> ok=<0|1> ::` at the EOF of a program in the last stage
- `:: MIDDEN: kill ok=<0|1> via=<builtin|ctrl-c> pid=<p> [err=<e>] cancelled=<0|1> ::`
- `:: MIDDEN: stdin bytes=<n> eot=<0|1> ::` (a one-shot filter reading its ring)

## What the two new `tests midden` drives print (expected shape)

Drive `prog-pipe` — typed `^U echo hi | run /apps/MIDDEN.ELF -c cat⏎` (cwd `/apps`):

```
:: MIDDEN: plan=Say verb=echo served=1 rows=<n> ::
[pipe] open pid=<c> reader=<k> child=<s> ring=65536
[spawnpath] path=/apps/MIDDEN.ELF principal=<p> bytes=<b> note=<f> pid=<c> slot=<s> entry=<e> argc=3 -> started
:: MIDDEN: plan=Exec verb=run served=1 pid=<c> ::
:: MIDDEN: pipe feed pid=<c> bytes=3 ok=1 ::
hi                                              (the child's console write)
:: MIDDEN: stdin bytes=3 eot=1 ::               (the child)
:: MIDDEN: plan=Host verb=cat served=1 rows=<n> ::   (the child)
:: MIDDEN: oneshot rows=<n> cmds=1 status=<s> ::     (the child)
:: MIDDEN: pipe pid=<c> bytes=<n> lines=<l> ::
[pipe] close pid=<c> bytes=<b> dropped=0
:: MIDDEN: pipe eof ok=1 pid=<c> lines=<l> bytes=<b> dropped=0 rows=<r> ::
:: MIDDEN: pipeline stages=2 last=/apps/MIDDEN.ELF rows=1 ok=1 ::     <- watched
```

Drive `launch` then `kill` — `run /apps/MIDDEN.ELF -c cat⏎` (watched `:: MIDDEN: plan=Exec verb=run served=1 `), then
`kill %1⏎`:

```
[pipe] kill pid=<c> by=<k> -> ok (killed, row reaped)
:: MIDDEN: kill ok=1 via=builtin pid=<c> cancelled=0 ::              <- watched prefix `:: MIDDEN: kill ok=1 via=builtin `
:: MIDDEN: builtin=kill ok=1 rows=<n> ::
[pipe] close pid=<c> bytes=0 dropped=0
:: MIDDEN: pipe eof ok=0 pid=<c> lines=0 bytes=0 dropped=0 rows=<r> ::
```

Witness: `:: MIDDENAPP: image=/apps/MIDDEN.ELF window=elf lines=<n> plan=wc run=ok drives=cd:ok,complete:ok,complete-run:ok,search:ok,search-run:ok,up:ok,prog-pipe:ok,launch:ok,kill:ok,pipe:ok -> PASS ::`
with the decoded status `cmds=12 kind=Host verb=wc`.
