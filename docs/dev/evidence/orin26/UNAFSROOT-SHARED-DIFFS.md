# UNAFSROOT — the shared-core diffs owed to rmbp for ack (orin 26, 2026-09-11)

Branch `exec-orin26-unafsroot`, base: the gated-green orin 25 fold `63654d93` merged with the
three-commit `exec-orin24-unafsroot` branch (`eb98ba90`, `ec687e18`, `a81f38c7`).

**The note.** The orin 24 baton owes rmbp the `unaos/crates/kernel/src/main.rs` and
`unaos/crates/kernel/src/drivers/block.rs` diffs from `a81f38c7` for ack before the branch moves.
Measured on this branch — `git diff 63654d93...a81f38c7 -- unaos/crates/kernel/src/main.rs
unaos/crates/kernel/src/drivers/block.rs | wc -c` prints `0`, and `git diff --stat
63654d93...a81f38c7` lists five files: `docs/dev/evidence/orin24/CARDREADY-GEOM.md`,
`unaos/arroyo`, `unaos/crates/kernel/src/fs/bootdisk.rs`, `unaos/scripts/make-pi-img.sh`,
`unaos/scripts/specs/jetson-sync1.spec`. So the two named diffs are EMPTY: `a81f38c7` changed
neither file, and the baton's line names files the commit did not touch. What the branch DOES
change in shared kernel core (the rmbp lane under LAWS §3 "Lanes") is `fs/bootdisk.rs`, in two
commits: `a81f38c7` (the `bind_root` split and the four mount witnesses) and this round's leg-6
fixture. Both are reproduced verbatim below so the ack can be given on the text, not on a
description of it. Nothing in `drivers/`, `fs/vfs.rs`, `fs/unafs.rs` or `fs/fat.rs` is modified by
either commit; the orin seat cannot message rmbp from an executor, so this file is the handoff.

## Diff 1 — `a81f38c7` (orin 24), `unaos/crates/kernel/src/fs/bootdisk.rs`

```diff
diff --git a/unaos/crates/kernel/src/fs/bootdisk.rs b/unaos/crates/kernel/src/fs/bootdisk.rs
index eb79be8d..7179ab6c 100644
--- a/unaos/crates/kernel/src/fs/bootdisk.rs
+++ b/unaos/crates/kernel/src/fs/bootdisk.rs
@@ -1220,23 +1220,90 @@ pub fn bind(mt: &mut crate::fs::vfs::MountTable) {
     }
 
     let Some(found) = s.root else { return };
-    let src = found.source;
+    bind_root(mt, found.source, unafs_state(found.source), announce);
+}
+
+/// UNAFSROOT (orin 24): the ROOT DISK's three mounts, as a function of ONE fact — the
+/// [`unafs_state`] string for the disk this kernel was found on.
+///
+/// It is a separate function from [`bind`] for a reason that is not tidiness. Everything above it
+/// needs real hardware: a survey, a FAT mount per source, a shared-mount bind. This does not — it
+/// needs a state string and a table — so the OS's own layout rule can be driven with both answers
+/// at the real call, on the real code path, by [`homesoil_selftest`] leg 6. Before this split the
+/// rule was reachable only by booting a card that HAD a native volume, which is precisely the card
+/// nobody had (render12, 2026-09-09: `unafs=absent`, so `/` had never once been the native volume
+/// on this board and the branch had never executed).
+///
+/// * `/`     — the disk's native UnaFS volume when `unafs` is `present`, else the FAT boot volume.
+///             `present` already means BOTH "this disk carries a volume" AND "the shared mount is
+///             riding this disk" (see [`unafs_state`]); the other three values
+///             (`absent`, `present-on-other-handle`, `unbuilt`) all mean the FAT, each for its own
+///             stated reason, and collapsing them here is what keeps that reasoning in one place.
+/// * `/boot` — the FAT volume the kernel was found in, always, native root or not. The kernel is
+///             loaded by firmware out of FAT, so it can never live at the root of the native
+///             volume; `/boot` is where it does live and that is the whole of this line's content.
+/// * `/apps` — the SAME FAT volume rooted at `APPS/`, under the SAME volume NAME as `/boot` so
+///             `same_volume("/boot", "/apps")` stays true about one card. Programs resolve from the
+///             FAT boot volume whether or not `/` is native — a native `/` does not move them.
+///
+/// THE POSTURE ON THE WIRE. Each mount says `rw=`, and each `rw=` is sampled from the thing being
+/// mounted rather than derived a second time: the FAT mounts read `FatBackend::read_only()` off the
+/// very backend handed to `mt.mount`, and the native root reads `BlockSource::write_veto()` — the
+/// single definition both of those forward to, and the same predicate `block::write_block` itself
+/// enforces. For a `Default`-sourced root that resolves to FRGUARD's `default_writable()`, a
+/// RUNTIME state, so there is deliberately no fixed expectation for it anywhere: the wire reports
+/// what it read. Nothing here weakens, bypasses or re-implements that gate.
+pub(crate) fn bind_root(
+    mt: &mut crate::fs::vfs::MountTable,
+    src: BlockSource,
+    unafs: &str,
+    announce: bool,
+) {
+    use crate::fs::vfs::{FatBackend, KERNEL_PRINCIPAL};
+
+    #[cfg(target_arch = "aarch64")]
+    let native_root = unafs == "present";
+    // `NativeBackend` is `#[cfg(target_arch = "aarch64")]` in fs/vfs.rs, so on a build that does not
+    // have the type there is no native root to bind whatever the state string says. This cannot
+    // change a real boot's answer — [`unafs_state`] returns `"unbuilt"` on those targets — but it
+    // makes leg 6's `present` case HONEST on x86_64 (it asserts the FAT fallback there, and says so
+    // on the wire) instead of asking for a mount the type system does not have.
+    #[cfg(not(target_arch = "aarch64"))]
+    let native_root = {
+        let _ = unafs;
+        false
+    };
 
-    let native_root = unafs_state(src) == "present";
     #[cfg(target_arch = "aarch64")]
     if native_root {
         mt.mount("/", alloc::boxed::Box::new(crate::fs::vfs::NativeBackend::new("native")));
+        if announce {
+            // The discriminating words are LITERALS in the format string, not a `{}` carrying a
+            // runtime kind — the `/volumes/` lesson three screens up, same reason: an artifact
+            // census (`LC_ALL=C grep -a -o -F`) must be able to find the sentence it certifies.
+            serial_println!(
+                "[vfs] root mount / = native unafs volume source={} rw={} ::",
+                src.name(),
+                if src.write_veto().is_none() { "yes" } else { "no" }
+            );
+        }
     }
     if !native_root {
-        mt.mount(
-            "/",
-            alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)),
-        );
+        let be = FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src);
+        let rw = !be.read_only();
+        mt.mount("/", alloc::boxed::Box::new(be));
+        if announce {
+            serial_println!(
+                "[vfs] root mount / = fat boot volume source={} rw={} ::",
+                src.name(),
+                if rw { "yes" } else { "no" }
+            );
+        }
     }
-    mt.mount(
-        "/boot",
-        alloc::boxed::Box::new(FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src)),
-    );
+
+    let boot = FatBackend::new_source("boot", KERNEL_PRINCIPAL, true, src);
+    let boot_rw = !boot.read_only();
+    mt.mount("/boot", alloc::boxed::Box::new(boot));
     mt.mount(
         "/apps",
         alloc::boxed::Box::new(
@@ -1244,6 +1311,18 @@ pub fn bind(mt: &mut crate::fs::vfs::MountTable) {
                 .rooted(crate::fs::fat::APPS_DIR),
         ),
     );
+    if announce {
+        serial_println!(
+            "[vfs] boot mount /boot = fat boot volume source={} rw={} ::",
+            src.name(),
+            if boot_rw { "yes" } else { "no" }
+        );
+        serial_println!(
+            "[vfs] apps mount /apps = fat boot volume source={} rooted={} ::",
+            src.name(),
+            crate::fs::fat::APPS_DIR
+        );
+    }
 }
 
 /// HOMESOIL: 11 label bytes as 22 lowercase hex digits, un-interpreted. Fixed width by the type, so
@@ -1491,6 +1570,7 @@ fn homesoil_selftest() {
         fat::same_device(&synth_dev(0, 100), &synth_dev(0, 100)),
         if leg5 { "PASS" } else { "FAIL" }
     );
+
 }
 
 /// CLONEALIAS: a synthetic `BlockDeviceInfo` for the fixture — the two fields
```

## Diff 2 — orin 26, `unaos/crates/kernel/src/fs/bootdisk.rs` (homesoil leg 6)

Appended below after the commit that carries it; the text is `git show <sha> -- unaos/crates/kernel/src/fs/bootdisk.rs`.

