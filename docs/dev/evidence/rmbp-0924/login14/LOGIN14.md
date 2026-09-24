# LOGIN14: root's password chosen on the glass at boot 13; a new account's at its first login

Cloud session (focus rmbp), branch `claude/optimistic-ramanujan-r3qyu5`, parent `f54acf6c` (hw-rmbp),
2026-09-24. Ledgered as rmbp-ledger **B198** (R65). Mechanism and the amended FLIGHT-13 line list:
`docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md` §9. Spec: `unaos/scripts/specs/x86-login.spec`
(LOGIN-ROOTPW and the rewritten LOGIN-ADDUSER blocks).

## Box

Cloud container, no KVM (TCG). nightly 2026-07-14 (`nightly-2026-07-15`, aliased as `nightly`),
`x86_64` 0.15.5, QEMU 8.2.2 (`UNAOS_QEMU_MACHINE=pc-q35-8.2`), OVMF 4M aliased to the 2M names the
builder searches. The toolchain window this forced is in the rmbp-queue STATE of 2026-09-24.

## Gates

| gate | result |
|---|---|
| `UNAOS_QEMU_MACHINE=pc-q35-8.2 UNAOS_WC=1 UNAOS_QUARRY=1 UNAOS_FTDIRX=1 UNAOS_LOGIN=1 UNAOS_LOGINST=1 UNAOS_QEMU_FULL=1 ./arroyo test 240` | rc=0; completion marker reached (serial line 2824), full wall 242.2 s; `awk 'index($0,"-> FAIL")'` = 0 lines |
| `./arroyo mbench --replay target/serial.log --spec scripts/specs/x86-login.spec --platform x86` | rc=0; MBENCH PASS 51/51 required, 0 forbidden, 3258 lines |
| GO-RED: `set_first_password` mutated to skip the write, same test line | rc=1; the line below reads `set=false verify=FAIL -> FAIL —`; file restored, `git status` clean |
| `./arroyo check` (final tree) | see the rmbp-queue LOGIN14 row (appended when it reported) |

## The wire (green run)

```text
[login] boot session=root desktop=false screen=closed (R63: the machine boots to the root desktop; the login screen opens at the root session's Log Out, never at boot)
[login] root password unset row=created -> set-password screen (LOGIN14/R65: chosen at the keyboard, twice; never on the wire)
[login] set-password screen open user=root login_after=false in_place=false (LOGIN14/R65: the password is typed twice here and never printed)
[login] root password unset row=present -> set-password screen (LOGIN14/R65: chosen at the keyboard, twice; never on the wire)
[login] set-password screen open user=root login_after=false in_place=false (LOGIN14/R65: the password is typed twice here and never printed)
[login] set-password user=root retype mismatch (nothing written; the form stays)
[users] password set user=root first=true (LOGIN14/R65: chosen at the keyboard, never on the line or the wire)
[login] set-password screen closed user=root (the root desktop continues)
[users] password set user=boot13 first=true (LOGIN14/R65: chosen at the keyboard, never on the line or the wire)
```

```text
:: LOGIN-ROOTPW: reset=true deferred=false opened=true keys_routed=true mismatch_kept=true set=true verify=ok wrong=refused screen=closed root_after=true -> PASS ::
:: LOGIN-ADDUSER: root=true created=unset:true prompted_at_adduser=false unset_verify=refused passwd_prompted=true echo=none uid=16486564 set=true verify=ok dup=exists empty=empty-password mismatch=mismatch on_line=password-on-line passwd_on_line=password-on-line -> PASS ::
:: LOGIN-ROOTOUT: root_before=true empty_refused=no-users pid=21 root_stamped=true others=0 root_after=false pid_gone=true window_gone=true screen=up screen_window=true not_root=not-root keys_routed=true name_typed=true tab=password wrong=denied login=boot13 cleaned=true -> PASS ::
```

## The wire (go-red run)

```text
:: LOGIN-ROOTPW: reset=true deferred=false opened=true keys_routed=true mismatch_kept=true set=false verify=FAIL wrong=refused screen=closed root_after=true -> FAIL — ::
```

## Reading

The desktop ignition ran BEFORE the store loaded on this QEMU boot (`deferred=false`, one
`row=created` line at the load, the fixture's own `row=present` after it re-drove the ignition). The
other order (store first, `deferred=true`, the screen opened by `boot_session`) is built and compiled
but has no runtime reading yet: the rMBP's SD card versus render-service race decides which the metal
shows, and §9.2's line 2 names both.
