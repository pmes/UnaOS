# midden — shell / command interpreter

Midden is the UnaOS shell handler: it parses command-line input and turns it
into `SMessage`s on the Bandy bus. It is the terminal capability in the
userspace handler set described in
[`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md).

## Status

**Design-stage / early stub.** The command surface exists and produces real
messages, but most commands return placeholder output and the handler is not yet
wired into the Synapse. See [Implemented](#implemented-today) vs.
[Planned](#planned) below.

## What it does

The crate exposes two pieces:

- **`create_view() -> (Widget, TextBuffer)`** — builds a read-only, monospaced
  GTK4 console (`ScrolledWindow` + `TextView`) and returns the widget plus its
  `TextBuffer`. A vessel embeds the widget in its window and appends terminal
  output to the buffer.
- **`Midden`** — the interpreter. It holds the current working path and a
  filesystem handle placeholder. Its core method is:

  ```rust
  pub fn execute(&mut self, command: &str) -> Result<SMessage>
  ```

  `execute` tokenizes the input and dispatches on the first word, returning an
  `SMessage` that the caller publishes or renders. Recognized commands:

  | Command  | Result message            | Notes                          |
  | -------- | ------------------------- | ------------------------------ |
  | `ls`     | `TerminalOutput`          | stub — does not yet read a dir |
  | `pwd`    | `TerminalOutput`          | returns the tracked path       |
  | `touch`  | `FileSystemEvent`         | emits intent; no write yet     |
  | `help`   | `TerminalOutput`          | lists available commands       |
  | (empty)  | `NoOp`                    |                                |
  | (other)  | `TerminalOutput`          | "Unknown command"              |
  | `touch` w/o arg | `TerminalError`    | usage message                  |

## How it plugs into Bandy

Midden communicates by value, not by direct calls. `execute` returns one of the
terminal `SMessage` variants defined in
[`libs/bandy`](../../libs/bandy/src/signals.rs):
`NoOp`, `TerminalOutput(String)`, `TerminalError(String)`, and
`FileSystemEvent(String)`. The containing vessel is responsible for publishing
these on the `Synapse` and feeding `TerminalOutput`/`TerminalError` back into the
console `TextBuffer`.

`Midden` also implements `bandy::BandyMember`, whose `publish(topic, msg)` is
currently a debug `println!` placeholder.

## Implemented today

- GTK4 console view (`create_view`).
- Command tokenizer and dispatch over `ls`, `pwd`, `touch`, `help`.
- `pwd` returns the tracked current path; `touch` emits a `FileSystemEvent`.
- `BandyMember` impl (stub).

## Planned

- Real filesystem access through the `unafs` dependency (currently imported but
  unused): `ls` enumerating directories, `touch` creating files.
- Subscribing to and firing on the `Synapse` rather than returning a single
  message and logging via `println!`.
- Command history, working-directory navigation (`cd`), and integration with the
  `elessar` workspace-context detection (also a dependency).

## Build

Midden is a member of the UnaOS workspace and builds host-native (it depends on
GTK4):

```bash
cargo build -p midden
```

## See also

- [`docs/dev/USERLAND/ARCHITECTURE.md`](../../docs/dev/USERLAND/ARCHITECTURE.md) — userspace component model (vessels / handlers / libraries).
- [`docs/CODEX.md`](../../docs/CODEX.md) — the handler manifest.
- [`libs/bandy`](../../libs/bandy) — `SMessage`, `Synapse`, `BandyMember`.
