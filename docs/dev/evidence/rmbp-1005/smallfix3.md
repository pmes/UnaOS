# SMALLFIX3 (rmbp-ledger B416) — the merge17 fold's hygiene

Cut d27ec897 (merge17). ATTRCOLUMNS, DIALOG2, ROOTDISK2, FIRSTUSER, SETTINGSFILES and APPRES M6 are NOT on this
tree: their halves are the patches at the foot (P1–P7, each `git apply --check`-clean against its branch tip;
P8 against this tree once they are folded). The seat applies them after the fold, in order.

## Findings (from the sources, not the hand-backs)
1. **`/boot` writers.** Neither named fixture WRITES the boot FAT: VFSROUTE leg 5 is `remove_attr("/boot/VFSROUTE.TXT")`,
   which FAT answers with the trait default `Unsupported` before any `authorize_write` (it stays on `/boot` on
   purpose — the leg proves the FAT has no attributes; `/var/tmp` is UnaFS and would make it vacuous). Quarry ops'
   `/boot/QOPS.TXT` is a pure `ns_check` (outside the home) — moved to `/var/tmp/QOPS.TXT` anyway, same refusal.
   The real writer the grep finds is **XVOL** (`shell.relics.mv.xvol`, direction 2 `fs_write(far)`): its far
   volume is the boot FAT and its skip guard asks `write_veto`, which on ROOTDISK2 does not know R99 → the write is
   refused and the leg reds. Fix (P1): `FatBackend::write_veto` asks `bootfat::veto` first, so XVOL prints its own
   `… refuses writes — shell.relics.mv.xvol skipped` and nothing writes; `bootfat` counts non-probe refusals
   (`writers()`), which `tests smallfix3` prints as `boot_writers=`.
2. **Codes.** `INPUT_EV_DIALOG_ANSWER` 10 → **11** (P2); `PrefDeclare` 20 → **23** in una-abi and prefs_core wire
   (P3; the syscall arms, vein_ring3 and Lumen read the constant — no literal anywhere); `Action::GetInfo` 42 →
   **47** (P4; no branch of the wave uses 47). This tree adds `una_abi::INPUT_EV_ALL` / `BUS_VERB_ALL` with const
   assertions (a clash fails the build on every leg and the host) and `smallfix3::ACTIONS` + an exhaustive match
   with a const assertion over `clipboard::action_code` (P8 appends the new names). Theme: `CRISPY_ROWS` /
   `PC_ROWS` are slices now (`&[Binding]`, counts by `.len()`; 60 and 36 on this tree) — an arc adding a row no
   longer edits the count line; on the fold take the slice line and keep both sides' rows.
3. **FIRSTUSER** (P5): `BootStage::CreateUser` dropped (`stage_of_store` never produces it; `STAGE` 1 = Desktop
   now — the login FORM's `State::CreateUser` is the first-user form and stays); `bootfat::fat_unlock` asks
   `users::admin_authority("boot-fat-write")` once (refused → the guard holds nothing, the gate stays shut,
   `[boot] fat unlock refused …`); DIALOG2's `power_row` sets `loginwindow::POWER_ROW` at its first paint (P2b).
   ATTRCOLUMNS' `getinfo::open` appends `settingsfiles::info_lines(path)` as `setting` rows (P6).
4. **LAWS** (seat approves): the Pi note now reads "unafs v3 volume capped at 2 GiB (the Pi's; the x86 card's v7
   image is sized content + 25 % + 512 MiB, floor 4 GiB, ROOTDISK2 B401 — `UNAOS_X86_UNAFS_MB` raises it, never
   lowers)".
5. **svg**: `unaos/res/facet/app.res` declares `image/svg+xml`, `facet.unares` repacked (`una-res pack`, 6588
   bytes) on this tree; P7 is the row in APPRES M6's `(mime, icon, name)` shape. The fold conflicts on the binary
   `facet.unares` (M6 changed `signature`): re-run `cargo run -p una-res -- pack unaos/res/facet
   unaos/res/facet/facet.unares` on the merged `app.res`.
6. **`help tests`** runs `tests list`, which now prints each fixture with its arc (`smallfix3::arc_of`: the
   fixture's own name upper-cased, or the `ARCS` row where they differ — 151 registered names on this tree).
7. **`#[allow(dead_code)]`**: none added this wave (diff hw-rmbp 2da613b3..d27ec897 and every exec-rmbp branch
   against it: 0 `+` lines). Nothing to remove.

## Seam
Kernel — kernel-by-ruling (`src/smallfix3.rs`, a read-only check module and the arc table); the ABI checks live
in una-abi (the shared crate both rings and the host link). No store.

## Milestones
M1 codes unique by construction (una-abi lists + asserts, Action list + assert, theme slices) · M2 `tests
smallfix3` + `help tests`/`tests list` arcs · M3 QOPS path, facet svg doctype + repack, LAWS line · M4 this doc
and P1–P8.

## Witness
`tests smallfix3` → `:: SMALLFIX3: boot_writers=<0|unarmed> event_codes=unique bus_verbs=unique
action_codes=unique theme_rows=derived -> PASS :: events=<n> verbs=<n> actions=<n> crispy_rows=<n> pc_rows=<n> ::`
(this tree: events=10 verbs=22 actions=46 crispy_rows=60 pc_rows=36, boot_writers=unarmed; after P1+P8 on the
x86 card: boot_writers=0 events=11 verbs=26 actions=47). `help tests` prints `<fixture>  <ARC>` rows (≥ 40).
XVOL on the card: `:: relics: /boot refuses writes — shell.relics.mv.xvol skipped ::` (or `/volumes/boot`).

## Owed
The patches (seat, after the fold). Re-count the theme rows on the merged tree is automatic (`.len()`); the
`ARCS` table names arcs for fixtures registered on THIS tree — later fixtures named after their arc need no row.

### P1 — ROOTDISK2: write_veto asks R99, bootfat::writers, fat_unlock asks admin_authority (needs FIRSTUSER folded)
```diff
--- a/unaos/crates/kernel/src/fs/bootfat.rs
+++ b/unaos/crates/kernel/src/fs/bootfat.rs
@@ -51,6 +51,7 @@
         return false;
     }
     REFUSED.fetch_add(1, Ordering::Relaxed);
+    if !rel.contains("R99PROBE") { WRITERS.fetch_add(1, Ordering::Relaxed); } // SMALLFIX3 (B416): a refusal that is not `tests bootfat`'s own probe is a fixture writing to /boot
     serial_println!("[boot] fat write refused path=/boot{}{} by={} (R99: sacred) ::", root, rel, principal);
     true
 }
@@ -58,17 +59,28 @@
 /// The named unlock. Held only by the installer/updater for one write; dropped = relocked.
 pub struct FatUnlock {
     by: &'static str,
+    /// SMALLFIX3 (B416): `false` when the administrator's authority refused it — the gate stays shut.
+    held: bool,
 }
 
 /// Open the boot FAT for ONE named write (`by` = `installer` or `updater`, `for_what` = what is written).
 pub fn fat_unlock(by: &'static str, for_what: &str) -> FatUnlock {
+    // SMALLFIX3 (B416, R100): the one writer path asks the administrator's authority once (`[auth] admin=… for=boot-fat-write`).
+    #[cfg(feature = "login")]
+    if crate::fs::users::admin_authority("boot-fat-write").is_err() {
+        serial_println!("[boot] fat unlock refused by={} for={} (R100: not the administrator) ::", by, for_what);
+        return FatUnlock { by, held: false };
+    }
     UNLOCKS.fetch_add(1, Ordering::AcqRel);
     serial_println!("[boot] fat unlocked by={} for={} (R99: installer/updater only; relocks on drop) ::", by, for_what);
-    FatUnlock { by }
+    FatUnlock { by, held: true }
 }
 
 impl Drop for FatUnlock {
     fn drop(&mut self) {
+        if !self.held {
+            return;
+        }
         UNLOCKS.fetch_sub(1, Ordering::AcqRel);
         serial_println!("[boot] fat relocked by={} ::", self.by);
     }
@@ -143,3 +155,18 @@
     drop(u);
     if open && !allows() { "installer" } else { "broken" }
 }
+
+// SMALLFIX3 (rmbp-ledger B416) — the boot FAT's refusals, asked BEFORE a write and counted after one. Tail.
+/// Refusals of a write that was NOT `tests bootfat`'s own `R99PROBE` — a fixture that still writes to `/boot`.
+static WRITERS: AtomicU32 = AtomicU32::new(0);
+
+/// `FatBackend::write_veto`, first: the reason a write to `volume` would be refused now, without saying it on
+/// the wire (the veto is a question, the refusal in [`refuse`] is the event).
+pub fn veto(volume: &str) -> Option<&'static str> {
+    if volume == BOOT_VOLUME && !allows() { Some("the boot FAT is sacred (R99): read-only for every principal") } else { None }
+}
+
+/// `tests smallfix3`'s `boot_writers=`: `None` when the gate is not armed (a FAT-root boot: the FAT is `/`).
+pub fn writers() -> Option<u32> {
+    if sacred() { Some(WRITERS.load(Ordering::Relaxed)) } else { None }
+}
--- a/unaos/crates/kernel/src/fs/vfs.rs
+++ b/unaos/crates/kernel/src/fs/vfs.rs
@@ -1524,7 +1524,7 @@
     /// the same answer the block layer gives, with the REASON attached, so the operator line names
     /// the mechanism that said no instead of a bare `-ENOTSUP`.
     fn write_veto(&self) -> Option<&'static str> {
-        self.source.write_veto()
+        crate::fs::bootfat::veto(&self.volume).or_else(|| self.source.write_veto()) // SMALLFIX3 (B416): R99 asked BEFORE the write, so a fixture's skip guard (XVOL's `write_veto`) sees the sacred FAT
     }
 
     fn rename(&self, from_rel: &str, to_rel: &str, principal: &str) -> Result<(), VfsError> {
```

### P2 — DIALOG2: INPUT_EV_DIALOG_ANSWER = 11; P2b power_row sets loginwindow::POWER_ROW (needs FIRSTUSER folded)
```diff
--- a/unaos/crates/kernel/src/video/login.rs
+++ b/unaos/crates/kernel/src/video/login.rs
@@ -2382,6 +2382,7 @@
     if !power_row_on() {
         return;
     }
+    super::loginwindow::POWER_ROW.store(true, Ordering::Release); // SMALLFIX3 (B416): FIRSTUSER's `login_window=users+power` reads DIALOG2's row
     let k = PFOCUS.load(Ordering::Relaxed);
     for i in 0..3u8 {
         let (x, y, w, h) = ctl_rect(Ctl::Power(i), false);
--- a/unaos/crates/una-abi/src/lib.rs
+++ b/unaos/crates/una-abi/src/lib.rs
@@ -1761,7 +1761,7 @@
 const _: () = assert!(BUS_VERB_DIALOG > BUS_VERB_PREF_CHANGED && BUS_VERB_TOAST < BUS_VERB_REGISTER);
 /// The answer to a dialog/sheet, delivered to the POSTER's input ring (never to the focused slot). Payload
 /// `[15:8]` = the caller's token, `[7:0]` = the button index (left to right; the default is the last).
-pub const INPUT_EV_DIALOG_ANSWER: u64 = 10;
+pub const INPUT_EV_DIALOG_ANSWER: u64 = 11; // SMALLFIX3 (B416): 10 is APPMENU2's INPUT_EV_CLOSE_REQ
 /// The field separator inside a dialog body (ASCII unit separator).
 pub const DIALOG_SEP: u8 = 0x1F;
 /// Buttons per dialog (the default is the rightmost — the last).
```

### P3 — SETTINGSFILES: PrefDeclare = 23
```diff
--- a/unaos/crates/una-abi/src/lib.rs
+++ b/unaos/crates/una-abi/src/lib.rs
@@ -1755,4 +1755,4 @@
 // =================================================================================================
 
 /// Bus verb: declare a program's settings stanza.
-pub const BUS_VERB_PREF_DECLARE: u8 = 20;
+pub const BUS_VERB_PREF_DECLARE: u8 = 23; // SMALLFIX3 (B416): 20..=22 are DIALOG2's DIALOG/SHEET/TOAST
--- a/unaos/libs/sys/prefs_core/src/wire.rs
+++ b/unaos/libs/sys/prefs_core/src/wire.rs
@@ -37,7 +37,7 @@
 pub const VERB_LIST: u8 = 18;
 pub const VERB_CHANGED: u8 = 19;
 /// SETTINGSFILES (B407, R98): a program declares its `app.<name>.*` stanza (body: [`crate::declare::body`]).
-pub const VERB_DECLARE: u8 = 20;
+pub const VERB_DECLARE: u8 = 23; // SMALLFIX3 (B416): una_abi::BUS_VERB_PREF_DECLARE (20 is DIALOG2's)
 /// una-abi `BUS_BODY_MAX`: the reply ceiling.
 pub const BODY_MAX: usize = 4096;
 
```

### P4 + P6 — ATTRCOLUMNS: GetInfo = 47; getinfo joins settingsfiles::info_lines (needs SETTINGSFILES folded)
```diff
--- a/unaos/crates/kernel/src/video/clipboard.rs
+++ b/unaos/crates/kernel/src/video/clipboard.rs
@@ -418,7 +418,7 @@
         Action::WinSizeLeft => 35,
         Action::WinSizeRight => 36,
         Action::WinSizeUp => 37,
-        Action::WinSizeDown => 38, Action::Minimize => 39, Action::CycleApp => 40, Action::ClearView => 41, Action::GetInfo => 42, // WINDOWLIST · LUMENBIN (⌘K, delivered to ring 3 as INPUT_EV_ACTION 41)
+        Action::WinSizeDown => 38, Action::Minimize => 39, Action::CycleApp => 40, Action::ClearView => 41, Action::GetInfo => 47, // SMALLFIX3 (B416): 42..=46 are APPMENU2's system chords. WINDOWLIST · LUMENBIN (⌘K, delivered to ring 3 as INPUT_EV_ACTION 41)
     }
 }
 
--- a/unaos/crates/kernel/src/video/keymap.rs
+++ b/unaos/crates/kernel/src/video/keymap.rs
@@ -165,7 +165,7 @@
     WinSizeLeft,
     WinSizeRight,
     WinSizeUp,
-    WinSizeDown, Minimize /* WINDOWLIST: ⌘M, code 39 */, CycleApp /* WINDOWLIST: ⌘` — next window of the SAME app, code 40 */, ClearView /* LUMENBIN: ⌘K — clear the focused view (LUMEN.ELF's transcript), code 41 */, GetInfo /* ATTRCOLUMNS (B402): ⌘I — Quarry's Get Info, code 42 */,
+    WinSizeDown, Minimize /* WINDOWLIST: ⌘M, code 39 */, CycleApp /* WINDOWLIST: ⌘` — next window of the SAME app, code 40 */, ClearView /* LUMENBIN: ⌘K — clear the focused view (LUMEN.ELF's transcript), code 41 */, GetInfo /* ATTRCOLUMNS (B402): ⌘I — Quarry's Get Info, code 47 (SMALLFIX3) */,
 }
 
 impl Action {
--- a/unaos/crates/kernel/src/video/quarry/getinfo.rs
+++ b/unaos/crates/kernel/src/video/quarry/getinfo.rs
@@ -46,11 +46,17 @@
     let kind = if matches!(st.kind, NodeKind::Dir) { "folder" } else { "file" };
     let when = st.mtime.map(af::fmt_when).unwrap_or_else(|| String::from("-"));
     let head = alloc::format!("{}  {} bytes  modified {}", kind, st.size, when);
-    let (rows, note) = match af::info_rows(&mt, path) {
+    let (mut rows, note) = match af::info_rows(&mt, path) {
         Ok(r) => (r, None),
         Err(VfsError::Unsupported) => (Vec::new(), Some("this volume carries no typed attributes (FAT)")),
         Err(_) => (Vec::new(), Some("attributes unreadable")),
     };
+    // SMALLFIX3 (B416): a settings file (SETTINGSFILES B407, R98) shows its auto-saved line and every key in the
+    // SAME inspector — one Get Info, not a second notice (`ops.rs`'s Show Info joins here).
+    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
+    for l in crate::video::settingsfiles::info_lines(path) {
+        rows.push(af::InfoRow { key: l, ty: "setting", val: String::new(), when: None });
+    }
     serial_println!("[getinfo] path={} attrs={} {}", path, rows.len(), head);
     for r in rows.iter() {
         serial_println!("[getinfo]   {} ({}) = {} changed={}", r.key, r.ty, r.val, r.when.map(af::fmt_when).unwrap_or_else(|| String::from("-")));
```

### P5 — FIRSTUSER: BootStage::CreateUser dropped
```diff
--- a/unaos/crates/kernel/src/boot.rs
+++ b/unaos/crates/kernel/src/boot.rs
@@ -8,7 +8,7 @@
 //!
 //! ONE gate, [`phase`], answers for the whole boot:
 //!
-//! * [`Phase::Setter`] — the store is not read yet, or it says Installer / CreateUser (root's password, the first user);
+//! * [`Phase::Setter`] — the store is not read yet, or it says Installer (the first-user form, R100);
 //! * [`Phase::LoginScreen`] — the store has users (boot 2) and no session has opened since the boot;
 //! * [`Phase::Desktop`] — otherwise; LATCHED by the first session open ([`session_opened`], from
 //!   `login::close_into_session`) or the installer's own `user-created` advance ([`ignite`]), so a later Log Out
@@ -77,7 +77,7 @@
             return Phase::Setter;
         }
         match users::boot_stage() {
-            users::BootStage::Installer | users::BootStage::CreateUser => Phase::Setter,
+            users::BootStage::Installer => Phase::Setter,
             users::BootStage::Desktop => {
                 if users::boot2_resolved() {
                     Phase::LoginScreen
--- a/unaos/crates/kernel/src/fs/users.rs
+++ b/unaos/crates/kernel/src/fs/users.rs
@@ -2877,7 +2877,7 @@
 // with root's password set and no user rows, a create-user dialog; only then the desktop, FOR THAT USER.
 // ONE predicate decides, derived from the loaded store and published once per stage change:
 //   root password unset               -> Installer   (the setter is the whole glass)
-//   root password set, no user rows   -> CreateUser  (name + password twice; the adduser path)
+//   (R100, SMALLFIX3: no root step — no user rows is the Installer, whose one screen is the first-user form)
 //   else                              -> Desktop
 // Every desktop tenant asks [`desktop_allowed`]: the STAT.ELF launch, the witness launcher (`winx_launcher`),
 // the furniture compose (`strip::compose_all`). The store is known ~400 ms after the takeover on the rMBP
@@ -2889,15 +2889,13 @@
 #[derive(Clone, Copy, PartialEq, Eq)]
 pub enum BootStage {
     Installer,
-    CreateUser,
-    Desktop,
+    Desktop, // SMALLFIX3 (B416): R100's `CreateUser` stage retired — `stage_of_store` never produced it; the first-user form IS the Installer's screen
 }
 
 impl BootStage {
     pub fn word(self) -> &'static str {
         match self {
             BootStage::Installer => "installer",
-            BootStage::CreateUser => "create-user",
             BootStage::Desktop => "desktop",
         }
     }
@@ -2924,8 +2922,7 @@
 pub fn boot_stage() -> BootStage {
     match STAGE.load(core::sync::atomic::Ordering::Acquire) {
         0 => BootStage::Installer,
-        1 => BootStage::CreateUser,
-        2 => BootStage::Desktop,
+        1 => BootStage::Desktop,
         _ => BootStage::Installer,
     }
 }
@@ -2963,7 +2960,7 @@
     serial_println!(
         ":: FIRSTBOOT: stage={} root={} users={} desktop_ignited={} why={} -> {} ::",
         if login { "login-screen" } else { st.word() }, if root_set { "locked" } else { "ROW" }, users, up, why,
-        if (st == BootStage::Installer && root_set && users == 0 && !up) || (st == BootStage::CreateUser && root_set && users == 0 && !up) || (up && root_set && users > 0) || (login && st == BootStage::Desktop && root_set && users > 0) || why == "no-store" { "PASS" } else { "FAIL" }
+        if (st == BootStage::Installer && root_set && users == 0 && !up) || (up && root_set && users > 0) || (login && st == BootStage::Desktop && root_set && users > 0) || why == "no-store" { "PASS" } else { "FAIL" }
     ); crate::bootpace::boot_line(); // QUIETBOOT M4 (R80): the boot's one measurement, after its first stage line.
 }
 
@@ -2980,7 +2977,6 @@
     } else {
     serial_println!("[login] installer: stage={} (R77: {})", st.word(), match st {
         BootStage::Installer => "R100: no user — the first-user form is the whole glass (that user is the administrator; root is locked); no desktop, no programs, no fixtures",
-        BootStage::CreateUser => "root's password is set and there is no user — the create-user form",
         BootStage::Desktop => "the desktop ignites",
     });
     }
@@ -3013,7 +3009,7 @@
             serial_println!("[login] boot 2: login screen deferred to the glass (the screen's window needs the surface)");
         }
     }
-    if st == BootStage::CreateUser || (st == BootStage::Installer && TABLE.lock().loaded) { // FIRSTUSER (R100): the Installer's one screen is the first-user form
+    if st == BootStage::Installer && TABLE.lock().loaded { // FIRSTUSER (R100): the Installer's one screen is the first-user form
         if DESKTOP_IGNITED.load(core::sync::atomic::Ordering::Acquire) {
             screen_create_user();
         } else {
@@ -3092,7 +3088,7 @@
 /// the session for THAT user and the desktop. `Err` is a one-line reason for the form; nothing is printed
 /// of the password.
 pub fn installer_create_user(name: &[u8], password: &[u8]) -> Result<(), &'static str> {
-    if !matches!(boot_stage(), BootStage::CreateUser | BootStage::Installer) || !TABLE.lock().loaded || user_count() != 0 {
+    if !matches!(boot_stage(), BootStage::Installer) || !TABLE.lock().loaded || user_count() != 0 {
         return Err("not the create-user stage");
     }
     create_user_rules(name, password)?;
```

### P7 — APPRES M6: the svg row in (mime, icon, name)
```diff
--- a/unaos/crates/kernel/src/fs/assoc.rs
+++ b/unaos/crates/kernel/src/fs/assoc.rs
@@ -56,6 +56,7 @@
     (ft::IMAGE_BMP, "image", "BMP image"),
     (ft::IMAGE_WEBP, "image", "WebP image"),
     (ft::IMAGE_QOI, "image", "QOI image"),
+    (ft::IMAGE_SVG, "image", "SVG image"), // SMALLFIX2 (B391, R94) row in APPRES M6's shape (SMALLFIX3 B416): facet declares image/svg+xml in unaos/res/facet/app.res
     (ft::AUDIO_FLAC, "sound", "FLAC audio"),
     (ft::AUDIO_OGG, "sound", "Ogg audio"),
     (ft::AUDIO_MPEG, "sound", "MP3 audio"),
```

### P8 — this tree after the fold: the new codes join the unique lists, GetInfo joins ACTIONS, boot_writers from bootfat
```diff
--- a/unaos/crates/kernel/src/smallfix3.rs
+++ b/unaos/crates/kernel/src/smallfix3.rs
@@ -24,7 +24,7 @@
     Action::SnapLeft, Action::SnapRight, Action::SnapZoom, Action::SnapRestore, Action::WinNudgeLeft,
     Action::WinNudgeRight, Action::WinNudgeUp, Action::WinNudgeDown, Action::WinSizeLeft, Action::WinSizeRight,
     Action::WinSizeUp, Action::WinSizeDown, Action::Minimize, Action::CycleApp, Action::QuitApp, Action::CloseWindow,
-    Action::HideApp, Action::OpenSettings, Action::ForceQuit, Action::ClearView,
+    Action::HideApp, Action::OpenSettings, Action::ForceQuit, Action::ClearView, Action::GetInfo,
 ];
 
 /// Exhaustive on purpose (no `_` arm): a new variant does not compile until it is named here — and the
@@ -40,7 +40,7 @@
         | Action::SnapRight | Action::SnapZoom | Action::SnapRestore | Action::WinNudgeLeft | Action::WinNudgeRight
         | Action::WinNudgeUp | Action::WinNudgeDown | Action::WinSizeLeft | Action::WinSizeRight | Action::WinSizeUp
         | Action::WinSizeDown | Action::Minimize | Action::CycleApp | Action::QuitApp | Action::CloseWindow
-        | Action::HideApp | Action::OpenSettings | Action::ForceQuit | Action::ClearView => true,
+        | Action::HideApp | Action::OpenSettings | Action::ForceQuit | Action::ClearView | Action::GetInfo => true,
     }
 }
 
@@ -75,7 +75,7 @@
 /// tree/boot (no `fs::bootfat`, or a FAT-root boot where the FAT is `/`). ROOTDISK2's fold replaces the body
 /// with `bootfat`'s non-probe refusal count (smallfix3.md, patch P1).
 pub fn boot_writers() -> Option<u32> {
-    None
+    crate::fs::bootfat::writers()
 }
 
 /// `(fixture, arc)` for the fixtures NOT named after their arc. Every other fixture's arc is its name,
--- a/unaos/crates/una-abi/src/lib.rs
+++ b/unaos/crates/una-abi/src/lib.rs
@@ -1770,7 +1770,7 @@
 /// Every input-ring event type (`INPUT_EV_*` the ring carries).
 pub const INPUT_EV_ALL: &[u64] = &[
     INPUT_EV_KEY_DOWN, INPUT_EV_KEY_UP, INPUT_EV_MOUSE_REL, INPUT_EV_MOUSE_ABS, INPUT_EV_BUTTON, INPUT_EV_WHEEL,
-    INPUT_EV_ACTION, INPUT_EV_MENU_PICK, INPUT_EV_WIN_RESIZE, INPUT_EV_CLOSE_REQ,
+    INPUT_EV_ACTION, INPUT_EV_MENU_PICK, INPUT_EV_WIN_RESIZE, INPUT_EV_CLOSE_REQ, INPUT_EV_DIALOG_ANSWER,
 ];
 /// Every kernel bus verb tag (the registrable range `>= BUS_VERB_FULFIL_MIN` is the bus's, not listed). The
 /// HOLOCRON band is listed by its two ends; [`codes_unique_u8`] also refuses any other tag inside the band.
@@ -1778,7 +1778,8 @@
     BUS_VERB_LS, BUS_VERB_CAT, BUS_VERB_CP, BUS_VERB_WRITE, BUS_VERB_RM, BUS_VERB_MV, BUS_VERB_MENU_PUBLISH,
     BUS_VERB_MENU_CLEAR, BUS_VERB_MENU_GET, BUS_VERB_NOTICE, BUS_VERB_ATTR_SET, BUS_VERB_ATTR_GET, BUS_VERB_ATTR_LIST,
     BUS_VERB_ATTR_QUERY, BUS_VERB_ATTR_STAT, BUS_VERB_PREF_GET, BUS_VERB_PREF_SET, BUS_VERB_PREF_LIST,
-    BUS_VERB_PREF_CHANGED, BUS_VERB_REGISTER, BUS_VERB_HOLOCRON_FIRST, BUS_VERB_HOLOCRON_LAST,
+    BUS_VERB_PREF_CHANGED, BUS_VERB_REGISTER, BUS_VERB_HOLOCRON_FIRST, BUS_VERB_HOLOCRON_LAST, BUS_VERB_DIALOG,
+    BUS_VERB_SHEET, BUS_VERB_TOAST, BUS_VERB_PREF_DECLARE,
 ];
 /// No two entries equal (and none zero). `const` so the assertion below runs in the compiler.
 pub const fn codes_unique_u64(v: &[u64]) -> bool {
```

