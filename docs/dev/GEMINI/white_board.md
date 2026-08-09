# WHITE BOARD — 2026-08-09 (GR23)

Questions for Peter, each with the background to answer it. Nothing else lives here.
The five Crispy taste questions are all ANSWERED and have moved off the board — they are
built or building (theme on the glass, paper ported, minimise via `set_hidden`; A4 and A5
withdrawn/deferred by your ruling).

---

## Q1 — THE BIG ONE: what is the next major arc? You set a target; I need the priority.

You said, at the trackpad: *"that console output should be going through midden and so should
the terminal that is the acting desktop… everything should be an elessar workspace."* That is
a DIRECTIVE, and I read it as the real shape of the desktop ladder's LAUNCH+EDIT rung — the
kernel converging onto the userspace model the tree already documents, not new apps.

What that means concretely (from `docs/dev/USERLAND/ARCHITECTURE.md` + `handlers/midden`):
- The x86 console prints straight from the kernel. In the model it should emit
  `SMessage::TerminalOutput` (the bandy variant already exists) and the console window is a
  *view* rendering it — not the kernel writing its own row.
- The kernel shell's dispatch (`storm`/`bg`/`jobs`/`kill`) is a second interpreter. Midden's
  charter (`Midden::execute(&str) -> SMessage`) owns that. The kernel shell should become a
  second shell over midden's core, not a rival.
- The "acting desktop" (console + PULSE + vug arrangement) is hand-wired today. In the model
  it is an **elessar workspace** — a `WorkspaceState` value the reserved `platforms/unaos`
  quartzite backend renders. The compositor I built this round is the *renderer beneath* that
  backend, not the desktop's owner.
- The honest boundary is the same one tabula hit: midden's core is nearly host-free, but its
  view is GTK and the Synapse is a Tokio channel — ring-3 has no std. The tree already shows
  the pattern in `unaos/libs/` (fs/input/pwm/helm all split a `no_std` core): a `no_std`
  midden core + a bounded in-kernel message ring standing in for the Synapse.

**The engineering is mine to design and brief. The decision that is yours: priority.** Is this
convergence the next big arc the fleet turns to — or does it sit behind more of the radio/3D
capability climb (Q2), or behind finishing the visual pass? Rank them, or name the first rung
you want. I'd start with the smallest true step — midden-core split + kernel console speaking
`TerminalOutput` through a no_std ring — which is the point at which "everything is an elessar
workspace" starts being true on metal.

## Q2 — The radios and 3D: which climb, and how far, now?

You asked "what about 3d wifi and bluetooth jobs." State of each:
- **Bluetooth — ready to climb NOW, seat's own lane.** L0/L1/L2 done; L2 flew clean on AR
  (1 device, mandatory off, HID survived). Next rung is **L3: connect to one LE device**
  (`LE Create Connection` → connection-complete → an ACL channel → the start of L2CAP). Pure
  HCI over the primitive L2 hardened; no firmware. This is the honest next rung.
- **WiFi — NOT blocked by signing or tooling** (both corrected). Blocked because the d11 PSM
  is a full real-time MAC — no minimal echo image reaches S6, and S5's HT-PHY wall stands.
  So authoring WiFi firmware is NOT a small arc. The decidable WiFi writes are: the
  **UCODEREV SHM probe** (identifies the resident microcode; writes `SHM_CONTROL` only, no
  upload) — **this one is already written on `wt/wifis4a`, unreviewed, see Q3** — and beyond
  it **S4's reset prologue**, the first real write taking the core off firmware's config.
- **3D — the decisive road, but GATED.** Authoring the Kepler FECS context-switch program is
  tractable (our falcon ucode already runs; Kepler is pre-secure-boot). But it can't start
  until the kepler lane's **FENCE** fix lands (the ucode is byte-correct but has never been
  uploaded — wrong IMEM register, no AINCW, verify deleted; I've ordered it fixed clean), and
  the RAMFC constants stay UNAUDITED (clean-room §5) until a Group-A layout exists.

**Your call:** do the radios/3D get fleet effort now, and if so which — BT-L3, the WiFi
prologue, both? Or do they wait behind Q1's convergence?

## Q3 — `wt/wifis4a` exists but I never had your go. Merge-after-review, or shelve?

The conservative UCODEREV SHM probe from Q2 is written, gated, and reachable-verified on
`wt/wifis4a` — but it landed while I'd told you I was *awaiting your call* on radio arcs, and
it writes the radio's silicon (`SHM_CONTROL` only — no data port, no MACCTL, no upload; safe
by the executor's own audit), so it is the first MMIO write in that driver. It has NOT had the
adversarial review every arc here gets. I am holding it unmerged. **Review-and-merge, or
shelve?** (One correction it found is worth keeping either way: b43's only hard ucode reject is
`fwrev <= 0x128` — the 351/410/598 numbers are frame-header generations, not gates; the doc
said otherwise.)

## Q4 — Where does the Crispy kit actually live? The shared-source law has lost its teeth.

`kits/crispy/theme.json` at `us-crispy-modern` `0787ba9f` — the law's source of record — is
**unreachable from this repo** (the git object does not exist here). The paper port held the law
by triangulating three *in-tree* records (a past commit's json blob, the `theme.rs`/`engine.md`
transcriptions, and the live generator `libs/quartzite/src/surface.rs`), which agree to the bit
— but nobody in this repo can diff against the named revision. Every future Crispy lift hits the
same wall. **Is the kit in another repo/worktree I should wire in, or do you want the law
re-based on an in-tree source of record?**

## Q5 — Paper under EVERY app window, or leave it on the installer well?

The mandate was paper as the CONTENT surface. It's ported and live — but only on `instgui`'s
content well, because that is the ONLY kernel-drawn content surface: the compositor *reads* app
surfaces and never writes them (apps paint their own mapped zero-pages). Putting paper under
real app windows means **pre-painting mapped surfaces at `SYS_WIN_CREATE`**, which moves the
WC-B fixture checksum and would wipe fbcon's/instgui's own pre-paints unless sequenced. It's a
decidable arc. This is the most "just tell me and I'll do it" question on the board — but it has
a fixture cost and it couples to Q1 (if the workspace model owns content surfaces, ownership
moves anyway). **Build it, or leave paper on the well until Q1 is decided?**
