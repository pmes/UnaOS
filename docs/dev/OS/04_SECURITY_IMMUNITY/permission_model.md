# Permission Model: Capabilities & Intents

## 1. The Death of "Root"
In unaOS, there is no "Root" user that can do everything. Even the Kernel has restrictions.
* **Principle of Least Privilege:** Apps start with **zero** permissions. They cannot see the webcam, the microphone, or your documents.

## 2. Dynamic Intents ("Ask on Use")
We do not ask for permissions at install time (when users blindly click "Yes"). We ask at **usage time**.
* **Scenario:** A photo editor wants to save a file.
* **The Old Way:** The app has access to your whole `Documents` folder.
* **The unaOS Way:** The OS opens a "Save" dialog. The user picks a file. The OS passes *only that specific file handle* to the app. The app never sees the rest of the folder.

## 3. The "Glass Room" (Privacy)
For apps that demand invasive permissions (like social media apps wanting your contacts):
* **Data Mocking:** The user can choose to feed "Mock Data" to the app.
* **The Result:** The app thinks it uploaded your contacts, but it actually uploaded a generated list of fake names ("John Doe," "Jane Smith"). The app functions, but your privacy remains intact.

## 4. Kernel Mechanism: Handles as Capabilities (implementation status)
The "specific file handle, never the whole folder" model of §2 is not just a UI convention — it
is the kernel's enforcement mechanism. A **handle** is an unforgeable, per-process reference that
carries **rights** and is **checked at the point of use**. The chain lands incrementally on the
aarch64 (Pi 4) track and ports to x86/Jetson after:

* **U4 (landed)** — the *structure*: a per-process handle table, keyed by address-space id (ASID).
  A child process is named by a handle into the spawner's table, so ownership is structural — a
  process can only act on handles in its own table.
* **U5 (landed, 2026-07-05)** — the *check*. A handle now carries a **rights bitmask**
  (`read`/`write`/`exec`/`grant`/`revoke`) and names a **target** (a child process, or a resource
  such as the console). Every resource syscall resolves its handle through one enforcement point
  that requires the needed right, else `-EACCES`. This makes the three capability operations real:
  * **Grant / attenuate** — a process holding `grant` on a handle can mint a *new* handle to the
    same resource for another table, but only with a **subset** of its own rights. A grant can
    **never amplify** rights (the monotonic-decrease invariant). This is exactly the "pass only
    that file handle, read-only" story, enforced.
  * **Revoke** — a process can drop a handle it owns; any later use fails.
  * **Bounded lifetime** — a process's whole handle table is cleared when its address space is torn
    down, so no capability outlives the process that held it.
  As the first routed resource, `write` to the console is now a capability (`CONSOLE` + `write`),
  granted to a process at launch — there is no ambient "stdout everyone can write".
* **U6a (landed, 2026-07-06)** — the *general object table*. A handle is now a
  `(kind, target, rights)` descriptor: the **kind** (`Child` / `Console` / and the scaffolds `File` /
  `Socket`) rides in a parallel sidecar so the value word keeps its `Empty`/`in-flight` sentinels
  untouched, and **all** kinds are first-free-allocated by one allocator. This retires U5's fixed
  console index (`CONSOLE_FD`): that index is now a *reserved* slot the allocator skips, so a process
  that both prints **and** spawns can hold a console cap alongside its child/object handles with no
  index collision — the one fragility U5's review flagged. `File`/`Socket` are scaffolds today
  (resolvable to their kind with rights-checking, but no filesystem/network syscall routes through
  them yet); they prove the table is general, and are where fs/net capabilities will attach.
* **U7 (landed 2026-07-07, the cross-process core)** — capability *transfer between processes*,
  kernel-mediated and single-writer-preserving: `SYS_XFER` deposits an **attenuated** descriptor into
  the recipient's per-ASID transfer **inbox** (the one deliberately cross-ASID surface, CAS-managed);
  `SYS_RECV` pulls it into the recipient's **own** handle row; the recipient is named by a `Child`
  handle in the sender's table (owner-scoped — no global process namespace). A **sender-owned
  transfer record** gives single-level **revoke**: the received cap goes stale at its next resolve.
  A transfer can never amplify (the same monotonic-decrease invariant as grant, now across
  processes), and the sender never writes — or revokes into — anything but its own record.
* **U6 (landed 2026-07-08, aarch64 — the by-NAME ACL)** — the file namespace itself is now ACL'd at
  `SYS_OPEN`, closing the gap that handle-cap gating left open (any process could open/create/unlink
  any name). **Owned-by-default:** an `O_CREAT` of a new name records the creating principal as the
  file's **owner** (private); an `O_PUBLIC` bit opts out to world-access. An open of an owned file is
  admitted only for the owner or a principal it **granted** (`SYS_FGRANT`, owner-scoped via a `Child`
  handle, mirroring `SYS_XFER`; `rights = 0` revokes) — the grant is an ACL edge on the *file*, so the
  grantee simply opens the name and the check admits it. The store is in-kernel and keyed by the file's
  identity, fenced by the `(ASID, ASID_GEN)` incarnation; it is the **enforcement seam** a persisted
  owner/grants store feeds. That store exists TODAY as the K1 `UNAFS.ATR` FAT-bridge sidecar (an on-disk
  owner form since K1/K2/K3 — cross-reboot enforcement metal-confirmed on real Pi 4); at **K4** it becomes
  the NATIVE UnaFS `owner`/`grants:*` typed attributes below, once a kernel UnaFS mount lands (gated on the
  ROADMAP §2 BeFS convergence: no_std port → block adapter → read-only mount → journaled writes). The
  K4-ready projection codec (the 1:1 sidecar→native-attribute string mapping) landed 2026-07-12, ahead of
  that mount. The x86 twin (U6x) is future.
* **Still ahead** — **revocation trees** (a revoked transfer cascading through the recipient's
  re-grants/re-transfers — today a derived copy escapes single-level revoke; per-cap derivation
  records + the reserved `revoke` right are that arc), the **bandy Ring-3 delegation wrapper** (so
  host-native principals delegate over the message bus), and real filesystem/network syscalls
  routing `File`/`Socket` payloads (File transfer needs descriptor migration). This is where the
  UnaFS `owner`/`grants:*` attributes of the model above become the persistent form of these live
  kernel handles.

See `docs/SECURITY.md` (the hardening ledger) and `docs/dev/OS/02_KERNEL_CORE/userspace.md`
(the syscall-level detail) for the exact mechanism and evidence.
