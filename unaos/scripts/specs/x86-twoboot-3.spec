# x86-twoboot-3.spec — KEYBOOT4 (rmbp-ledger B864), BOOT 3 of `./arroyo twoboot --card`: a FRESH native card (the metal
# shape — `esp_x86_unafs_card`'s target/unaos-x86-card.img: GPT p1 ESP + p2 UnaFS carrying /apps /lib /system), the
# first-user form typed by the leg's typist, the root KEYED by the first administrator's password.
#
#   QEMU gate:  ./arroyo twoboot --card   (arroyo names this file in its `twoboot)` case, so GATE-SPECROOTS resolves it
#               GATED) → unaos/target/twoboot-3.log; `scripts/twoboot.sh` judges the capture. The plain `./arroyo twoboot`
#               leg never boots this file (the seat's ruling: a card-boot hang under TCG never reds the standing leg).
#
# WHY. KEYBOOT3 (B856) put `[keyboot] unlock form up` / `[keyboot] unlock kdf_ms= ->` on the wire, but no QEMU boot was
# ever keyed: boots 1-2 run on the builder's FAT16 card and stick, where `keyboot_probe` finds no UnaFS partition
# (`root=none hold=none`) and `keyboot_key_root` refuses (no native root). This boot keys one, as flight 37's boot 1 did
# on metal (`[keyboot] root keyed user=una handle=Sdhc …`); boot 4 (x86-twoboot-4.spec) is the keyed boot. Lines read
# off fs/users.rs (`keyboot_hold`, `keyboot_key_root`, `installer_create_user`) and video/login.rs (`kb_words_pending`).
# The typist types `una` / `correct-horse` twice into the form, the boot-1 keys. The 24 words' screen needs a click
# (Continue), so the boot ends there: the key slot is on the volume before the words are shown.

# A fresh native root: probed, unkeyed, no hold.
REQUIRE \[keyboot\] root=unkeyed hold=none
REQUIRE \[login\] installer: first-user form open
REQUIRE \[login\] installer: create-user user=una id=\d+
# The first administrator's password keys the bound root (Argon2id at the kernel's RING_KDF costs).
REQUIRE \[keyboot\] root keyed user=una handle=\S+ kdf=argon2id
FORBID \[keyboot\] root not keyed
FORBID \[keyboot\] first-user keying=deferred
FORBID :: FIRSTBOOT: .* -> incoherent
COMPLETE \[login\] recovery words shown
