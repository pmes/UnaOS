# CLOUD-ENV — making the cloud seat's container ready on its first command

Read at 6423d5758 (2026-11-01). The container the cloud seat runs in starts fresh: the repository is cloned, the
pinned nightly comes from `rust-toolchain.toml` on the first `cargo` call, nothing else is there. What a session
spends before it can fold: the first kernel `cargo check` per leg (build-std, about 6-8 minutes each), the first
host suite build (`aether --lib` about 9 minutes, the rest 1-4), and the scratch scripts that lived outside the
tree until `tools/seat/`. QEMU is not installed, so the seat has never run `./arroyo test` here: every fold's
battery is the bench's. Four cores, 15 GiB, about 250 GiB of disk of which the two target dirs take 8 GiB.

## Setup script (Environment > Edit > Setup script)

It runs once per new session, one directory above the repository, before the first command.

```sh
set -e
cd UnaOS
# the toolchain the tree pins, with the pieces build-std needs, and the two kernel targets
rustup toolchain install nightly-2026-10-26 --component rust-src,llvm-tools --profile minimal
rustup target add x86_64-unknown-none aarch64-unknown-none --toolchain nightly-2026-10-26
# the QEMU battery (./arroyo test, test-arm, kernel8-test) and the image tools
sudo apt-get update -qq
sudo apt-get install -y -qq qemu-system-x86 qemu-system-arm qemu-utils mtools dosfstools \
  libasound2-dev pkg-config libgtk-4-dev libpango1.0-dev
# the crates, so a code-only session can `cargo metadata --locked --offline` and build without the network
cargo fetch --locked
(cd unaos && cargo fetch --locked)
# the seat's scratch and its scripts
mkdir -p /root/unaos-bench/scratch/cloud/wave36/tmp
cp tools/seat/* /root/unaos-bench/scratch/cloud/wave36/
```

If `apt-get` is refused by the network policy, add the distribution mirrors under Allowed domains (keep "Allow
package managers" ticked); the gtk and pango packages are what makes `cargo test -p quartzite --features gtk`
honest on the seat instead of a known red.

## Environment variables (Environment > Edit > Environment variables)

| variable | value | why |
|---|---|---|
| `CARGO_INCREMENTAL` | `0` | the seat's builds are one-shot per fold; incremental doubles the target dirs for nothing |
| `CARGO_BUILD_JOBS` | `4` | the container's four cores; the executors' private builds share them (`2` while legs run) |
| `CARGO_PROFILE_DEV_DEBUG` / `CARGO_PROFILE_TEST_DEBUG` | `0` | no debuginfo on host suites: a third of the build time and disk |
| `CARGO_PROFILE_DEV_PACKAGE_AETHER_OPT_LEVEL` | `0` | the aether suite's one slow crate |
| `TMPDIR` | `/root/unaos-bench/scratch/cloud/wave36/tmp` | scratch is never /tmp (the standing order) |
| `PKG_CONFIG_PATH` | `/root/unaos-bench/scratch/cloud/wave36/nle1/alsa/pc` | only until `libasound2-dev` is in the setup script; then drop it |
| `UNAOS_SEAT` | `cloud` | the scripts and the ledgers name the seat |

Not variables: `UNAOS_*` kernel knobs stay per command (a knob in the environment would arm every build).

## Network policy

Default is enough for cargo and GitHub. The research agents were refused `chromium.googlesource.com` and
`reddit.com`; add them under Allowed domains only for a research session, not for the fold seat.

## Caches worth keeping

The container is reclaimed with its disk; nothing persists but the pushed branch. So: push after every fold
(the seat does), keep the arc writers' launch files in the executors' hand-back text (they are), and treat
`host-target-seat` / `target-seat` as rebuildable. A persistent build cache is the one thing a setup script
cannot give back; the next best is `cargo fetch` in the script so the rebuild is compile time only.
