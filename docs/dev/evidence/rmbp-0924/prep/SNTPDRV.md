# SNTPDRV — prep

## The finding

`docs/dev/QUEUE.md:119` (orin session, 2026-09-13): "**SNTP IS DRIVEN FROM A NIC DRIVER, AND
THERE ARE THREE CLIENTS.**" — "the ONLY caller of `smolnet::witness_tick_sntp` is a statement
inside `drivers/e1000.rs:1184 service_net`... The Pi therefore did not inherit a client, it got a
SECOND copy on genet... The Jetson has NONE." Peter: "i'm curious why sntp doesn't 'just work'" …
"that sounds like a hack." Row asks for ONE client, three drivers under it; bringing e1000 onto the
shared net6 surface (retiring the other two clients) is named "its own arc" needing Peter's go.

## Mechanism (as read today — one client already landed since the row was written)

Since the row, a fourth client (ORINTIME, `ad549988`) landed `net_sntp_client.rs`, gated
`#[cfg(all(feature="sntp6", feature="net6", target_arch="aarch64"))]` (`lib.rs:46`). NIC-agnostic:
names no driver, talks only to `net_phy::net6` (`open`/`bind`/`sendto`/`recvfrom`/`gateway`/
`resolver`/`dns`); `service_tick()` (`net_sntp_client.rs:291`, `sync_now` `:202`) is called from
net6's OWN status fn at `net_phy.rs:1041` (folded onto its `ok` return, comment: "the deliberate
refusal of the x86 shape"). `net6::register_nic` (`net_phy.rs:844`) is called by
`rtl8168_tegra.rs:5733` and `virtio_net.rs:643` — both current aarch64 NICs already share this seam.

Still stapled to drivers, exactly as the row describes:
- **x86 smolnet**: `e1000.rs:1184` `service_net()` calls `smolnet::witness_tick_sntp()`
  (`:1219`, impl `smolnet.rs:1638`) directly, guarded `all(smolnet, target_arch="x86_64")`. No net6
  involvement — talks to smolnet's own `NET_DEVICE` (`smolnet.rs:1290-1660`, SOCK-1..8 + SNTP-X86).
- **Pi genet**: `arch/aarch64/genet.rs` carries a SECOND SNTP state machine, "PI-NET-16"
  (`sntp_step` `:2999`, `SntpState` `:1595-1600`, parse used at `:3025`), run from genet's own poll
  loop — not net6, not `net_sntp_client`.
- **USBNET (AX88179, SO56)**: `xhci/usbnet.rs`, `service_usbnet()` driven from `xhci/mod.rs:13611`
  /`:17220`, moves frames but calls no SNTP client. `e1000.rs:1039/1047/1053` fold it into the SAME
  `NET_DEVICE` accessors x86 smolnet uses (`.or_else` fallbacks), so a USB dongle's frames already
  reach smolnet's link and x86's SNTP call already covers it once that link exists; the driver
  itself still knows nothing about SNTP.

Net6/aarch64 is now ONE client, driver-agnostic (done). x86 (smolnet+e1000, and transitively
USBNET-under-smolnet) and Pi's genet PI-NET-16 remain the two hack-shaped copies the row names.
Unifying all three onto one `net::service_tick()` is the "its own arc" Peter has not yet ruled on.

## Plan

**M1 — arch-neutral seam, no behavior change.** Add `net::service_tick()` (new `net.rs`, or a fn
beside `net_sntp.rs`) that on x86_64 calls exactly what `e1000.rs:1219` calls today, and on
aarch64+net6 calls exactly what `net_phy.rs:1041` calls today — a re-dispatch, not new logic.
Touches: new `net.rs`; `drivers/e1000.rs:1219` (call `crate::net::service_tick()` instead);
`net_phy.rs:1041` (same). Witness: `:: SNTP: link=e1000 synced=<bool> offset_ms=<n> -> PASS ::`
(x86) / `link=net6` (aarch64) — NEW, additive beside `SNTP-X86-GATE`/`[net16]` so no current pin
breaks. Go-red: comment out the new call site on x86; the NEW `:: SNTP:` REQUIRE (below) reds,
the old pins don't.

**M2 — migrate Pi genet PI-NET-16 onto net6.** If genet registers via `NicOps` like
`rtl8168_tegra.rs`/`virtio_net.rs` do (Open Q1), delete `genet.rs`'s `sntp_step`/`SntpState`
(`:1569-3060`-ish); `net_phy.rs:1041`'s existing call covers it — retires a whole client. Witness:
M1's `link=net6` line fires on Pi; `[net16] sntp ...` lines stop. Go-red: un-register genet's
NicOps, show the line never appears on a Pi capture.

**M3 — retire x86 smolnet's own SNTP hang-off (Peter's ruling required — "its own arc," "touches
x86 boot").** Give e1000 a `NicOps` impl + `register_nic` call, delete `smolnet.rs`'s SNTP-X86
block (`:1598-1830`-ish) and `e1000.rs:1219`'s direct call, leaving `net::service_tick()` as the
only call on every arch. DO NOT START without that ruling. Witness: `:: SNTP: link=e1000 ...` now
comes from net6's service fn, not `service_net`. Go-red: `LC_ALL=C grep -a -o -F SNTP-X86-GATE` on
a fresh capture must find nothing once the old block is deleted.

**M4 — USBNET awareness (confirm, likely no code).** After M3, verify the AX88179/CDC-ECM path
needs no direct SNTP call: it's already framed under `NET_DEVICE` via the `.or_else` fallbacks
(`e1000.rs:1039/1047/1053`) that `service_tick()`'s x86 arm already polls. Witness: boot with no
e1000 present, a USB dongle up, capture the same `:: SNTP:` line firing over it (naming per Open Q3).

## Spec pins

`unaos/scripts/specs/x86-witness.spec` (read `:435-450`, `:1370-1385`) pins the SNTP fixture's
cleanup (FORBID on a stale civil-time anchor breaking the `clock=unarmed` guard, emitter
`smolnet.rs:1787`) but not `SNTP-X86-GATE` itself in the ranges read — grep the full file before
M1 to confirm no `REQUIRE :: SNTP-X86-GATE` exists elsewhere that M3's deletion would break.

For M1 (additive, safe now):
```
REQUIRE :: SNTP: link=(e1000|net6) synced=(true|false) offset_ms=-?[0-9]+ -> PASS ::
FORBID  :: SNTP: .* -> FAIL ::
```
For M3 (only after Peter rules, add once the old line is actually deleted):
```
FORBID SNTP-X86-GATE
```
No regex look-around used (LAWS-compliant `.spec` grammar).

## Open questions

1. Does `genet.rs` call `net6::register_nic`, or own its NIC path outside `NicOps` entirely? (Not
   found in this pass — `grep -n register_nic unaos/crates/kernel/src/arch/aarch64/genet.rs` first;
   settles M2's size.)
2. Peter's ruling on M3 (e1000 onto net6) — row states this explicitly needs his go-ahead. M1/M2
   can proceed without it; M3 cannot.
3. M4's `link=` value with USBNET active and no e1000 silicon: `link=usbnet` distinctly, or
   `link=e1000` (the smoltcp `Device` adapter's name regardless of physical NIC)? Needs a ruling
   so `link=` stays meaningful across e1000/USBNET/net6.

## Next-session start

1. `grep -n "register_nic\|NicOps" unaos/crates/kernel/src/arch/aarch64/genet.rs` — settles Open
   question 1 and therefore M2's actual scope.
2. `grep -n "SNTP-X86-GATE\|SNTP" unaos/scripts/specs/x86-witness.spec` (full file, not just the
   ranges read here) to confirm no existing REQUIRE on the old line before M1's new REQUIRE is
   added.
3. Draft `net::service_tick()` per M1 (see `## Draft code (unbuilt)` below) as a real diff, run
   `./arroyo check` (type-check only, no boot) before anything else.

## Draft code (unbuilt)

New `unaos/crates/kernel/src/net.rs` — re-dispatch only, no new sync logic:

```rust
// SNTPDRV M1: the arch-neutral drive seam. One call site both drivers and any future periodic
// hook can share; each arm is exactly today's existing call, moved, not rewritten.
pub fn service_tick() {
    #[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
    crate::smolnet::witness_tick_sntp();
    #[cfg(all(feature = "sntp6", feature = "net6", target_arch = "aarch64"))]
    crate::net_sntp_client::service_tick();
}
```

`drivers/e1000.rs`, anchor: existing call at line 1219 — replace with:

```rust
    crate::net::service_tick(); // SNTPDRV M1: was a direct smolnet call; now the shared seam.
```

`net_phy.rs`, anchor: existing fold at line 1041 — replace with:

```rust
        crate::net::service_tick(); ok // SNTPDRV M1: same seam x86 now calls.
```
