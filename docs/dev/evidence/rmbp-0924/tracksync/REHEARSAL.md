# TRACKSYNC — the aarch64 boot-13 login rehearsals (cloud session, 2026-09-24)

Container: QEMU 8.2.2 TCG, no KVM, no bench. Serial logs read with `awk 'index($0,"<tag>")'`.
Tree: `claude/optimistic-ramanujan-r3qyu5` after the hw-jetson merge 2dda36e8 (TRACKSYNC blocks in the
three platform queues). Type-check on the merged tip: `UNAOS_LEDGER_STRICT=1 UNAOS_K8REACH_STRICT=1
./arroyo check` → `kernel cfg coverage OK (86 legs)`, GATE-VERBS GREEN, GATE-LEDGER 245 (+0 against the
baseline); rc=1 from the host suites only (the crates that need OpenSSL/ALSA/GTK headers this box lacks,
and `matrix --test finder`'s `write_to_readonly_dir_surfaces_loud_denial`, which needs a non-root user —
this container runs as root, so a read-only directory is writable and the test sees `Ok`).

## 1. virt (the Orin's QEMU stand-in for the login store), `UNAOS_GICV3=1 UNAOS_VIRT_EL0=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_FATIMG=sf ./arroyo test-arm 150`

First attempt: exit 4, NO VERDICT — `builder/fat-sf.img` did not exist in this checkout
(`./arroyo fat-img` makes it; the verb said so and produced no serial bytes rather than a pass).
Second attempt: **rc=0**, ladder `[test-arm/gicv3]` reached `:: CAPSTONE COMPLETE` at line 381 of 381,
census 4/4. No `-> FAIL`, no fault text. The login lines:

```
[login] root password unset row=created -> set-password screen deferred to the desktop ignition (LOGIN14)
:: LOGIN-ROOTPW: -> SKIP — no screen built in this image (the x86 `wc` lane is where this claim is proven) ::
:: LOGIN-BOOTROOT: session=root(uid0) root_at_boot=true desk_screen=closed nodesk_screen=closed still_root=true screen_built=false -> PASS ::
[users] adduser fixture: x86 lane only (the verb and the prompt are arch-neutral and compiled here; the leg runs on the x86 login lane)
:: LOGIN-EPOCH: path=HOME/una/EPOCH.TXT created=true owned=true same_session_ok=true refused_while_closed=true relogin=true stale_refused=true stale_kind=0 fresh_ok=true fresh_kind=5 epoch=1->2 deleted=true -> PASS ::
:: LOGIN: users+session create=ok verify=ok wrong=refused login=ok principal=user:una#14647715 linked=true home=exists acl=ok epoch=ok logout=ok users=2 volume=el0-fat -> PASS ::
:: LOGIN-HARD: kat=ok iters=74074 ms=207 v2_rows=1 migrated=1 legacy_verify=ok migrated_verify=ok wrong=refused unknown=refused floor=10000 -> PASS ::
:: LOGIN-IDENT: a_uid=14647717 b_uid=14647718 a2_uid=14647719 slot_reused=true owner_ok=true same_slot_refused=true same_name_refused=true reason=recycled-id -> PASS ::
:: LOGIN-KOWN: pred=ok resolver=refused errno=-13,-13 reason=kernel-owned -> PASS ::
:: LOGIN-RAND: source=jitter distinct=true nonzero=true same_source=true salts_differ=true draws=10 epoch_bits=64 -> PASS ::
```

Read: root's row is created UNSET at the first service pass and the alert is DEFERRED to the desktop
ignition — exactly LOGIN14's aarch64 path — and this image builds NO screen (`screen_built=false`), so
`LOGIN-ROOTPW` SKIPs by name instead of claiming anything, and the adduser leg says it runs on the x86
lane. The seven other login fixtures (EPOCH, users+session, HARD, IDENT, KOWN, RAND, BOOTROOT) PASS on
the el0-fat volume. So on aarch64 the store half of boot 13 is proven; the SCREEN half needs an image
that ignites a desktop — the Pi's `kernel8` or the Orin's card.

## 2. Pi raspi4b, `UNAOS_PIDESK=1 UNAOS_WC=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 ./arroyo kernel8-test 180`

Attempt 1: exit 2 at banner-cert, BEFORE QEMU — `feature=loginst witness=<UNREGISTERED> hits=- ->
UNREGISTERED`, `noverdict=1`. **SO54**: `loginst` had no row in `scripts/banner-cert.sh`. Row added
(`loginst|:: LOGIN: users+session|-|measured`); measured hits of that literal:

| artifact | hits |
|---|---|
| `target/pi_baremetal/kernel8.img` (this build) | 5 |
| `target/aarch64-unaos/release/unaos-kernel` (the virt login run) | 5 |
| `target/x86_64-unaos/release/unaos-kernel` (loginst OFF, control) | 0 |

Re-certified the same artifact: `feature=loginst … hits=5 -> OK`, `noverdict=0`, exit 0. The `login`
row also holds on it (`[login] screen open window=` hits=4), i.e. this image BUILDS the screen.

Attempt 2 (row in place): the build and the certification pass, then
`qemu-system-aarch64: unsupported machine type` three times — **QEMU 8.2.2 has no `raspi4b`** (it was
added in QEMU 9.0). `-machine help` here lists raspi0/1ap/2b/3ap/3b only. The verb reports it as
`HARNESS FLAKE — QEMU never came up`; it is not a flake, it is an absent machine type, and the wording is
noted in QUEUE.md §5. **The Pi screen rehearsal is therefore owed on the bench (QEMU ≥ 9) and is not
claimed here.** The artifact it would run is certified.
