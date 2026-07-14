# unafs (CLI)

A command-line bridge between the host filesystem and a UnaFS vault image. It is
the operator tool for creating, inspecting, and populating a `unafs.img` from
macOS or Linux.

## Responsibilities

`unafs` is a thin executable (`tools/unafs`, binary name `unafs`) that drives
the [`unafs`](../../../libs/fs/unafs) library against a single vault image file. A
vault is a self-contained UnaFS volume — an inode-based filesystem that also
stores semantic attributes and supports content queries. The CLI exposes that
library through a set of subcommands and is the primary way to move data in and
out of a vault during development.

The subcommands map directly onto library operations:

- `init --path --size-mb` — pre-allocate the image file and `FileSystem::format`
  a fresh vault of the requested size (default `unafs.img`, 1024 MB).
- `ls --path --img` — mount the vault and list a directory.
- `put <source> <destination> --img` — read a host file and write it into the
  vault under the destination directory.
- `get <source> <destination> --img` — extract a file from the vault back to the
  host.
- `attr-set <path> <key> <value> --img` / `attr-get <path> <key> --img` — set or
  read a semantic attribute on a vault path. The value is parsed by the library's
  `parse_value`.
- `query <query> --img` — run a semantic query and print matching inodes with
  their relevance scores.

The CLI itself contains no filesystem logic; all on-disk format handling,
journaling, and indexing live in the `unafs` library.

## Key types and entry points

The binary is built on three crates:

- **`unafs` library** — the vault implementation. The CLI uses `FileDevice`
  (a host file as a block device), the `FileSystem` type alias
  (`UnaFS<FileDevice>`) and its methods `format`, `mount`, `resolve_path`, `ls`,
  `create_file`, `write_data`, `read_inode`, `read_data`, `set_attribute`,
  `get_attribute`, and `query`, plus the `parse_value` attribute parser.
- **`bandy`** — the in-process message bus. After `init`, the CLI builds an
  `SMessage::FileEvent { path, event }` and calls the vault's `publish` method
  (from the `BandyMember` trait) to announce that a vault was created. (The
  current `BandyMember` implementation logs the event rather than fanning it out
  on a live `Synapse`.)
- **`clap`** (derive), **`anyhow`**, and **`tokio`** — argument parsing, error
  context, and the async `main`. The command set is defined by the `Cli` /
  `Commands` types in `src/main.rs`; `main` is a `#[tokio::main]` `async fn`
  returning `anyhow::Result<()>`.

## Fit within UnaOS

`unafs` is a CLI vessel under `tools/`. Per the
[userspace architecture](../../../docs/dev/USERLAND/ARCHITECTURE.md), vessels are
the executables a user runs; this one composes the `unafs` storage library with
the `bandy` bus rather than the Quartzite GUI. It is the host-side operator
counterpart to the in-system storage path that vessels and the durable-memory
vault (`vein::vault`) use at runtime.

## Status

Implemented as a working operator tool: all listed subcommands drive real
library operations against a vault image. Bus integration is partial — the
post-`init` `publish` call currently logs the `FileEvent` rather than delivering
it over a `Synapse`. The vault format and semantic-query engine are provided by
the `unafs` library; see its README for the storage and indexing details.
