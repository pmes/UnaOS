# CHECK ffec19c33 — hw-rmbp tip, the bench (gates only; the cloud: 'gates only today, boot tomorrow'), 2026-10-28T20:13Z

Tree: `ffec19c33 2026-10-28 Merge cloud tip c2d86af47 into hw-rmbp: the 2026-10-28 overnight (503 arcs, B850..B1090 / SR190..SR463 / Z1..Z115) over the bench's Facet f` — the gate tree detached at ffec19c33, clean. Host: the bench toolbox. Run: `gate38g.sh` (`tools/trust-bundle`, `./arroyo check`, `./arroyo gates`, `./arroyo foldleg` under image-line.env + AETHER/MIDDEN/UNA/SONDE + the two bunker FW paths, `./arroyo knoboff beam`).

| leg | rc | span (UTC) | the line |
|---|---|---|---|
| trust-bundle | 0 | 16:38:41Z→16:38:45Z | `inters:  https://ccadb.my.salesforce-sites.com/mozilla/PublicAllIntermediateCertsWithPEMCSV — 2526 certificates (the CA ones join the untrusted pool),` |
| check | 1 | 16:38:45Z→20:11:15Z | `[0;31m❌ check FAILED — a host gate is red or missing (./arroyo gates rc=1; see target/gates/<name>.log)[0m` |
| gates | 1 | 20:11:15Z→20:11:47Z | `GATES: 25 run 16 green -> FAIL  (logs: target/gates/)` |
| foldleg | 0 | 20:11:47Z→20:13:09Z | `FOLDLEG: x86 kernel+loader built under image-line.env @3e6d45b10 blob=08c35316 features=59 -> PASS rc=0` |
| knoboff-beam | 2 | 20:13:09Z→20:13:09Z | `         [0;33m./arroyo knoboff beam ae50c3fdec437c4f0b6dab44233598a34913f088[0m   # you merged your arc INTO trunk (landing)` |

## gates
    gate=charter rc=1 ❌ GATE-CHARTER: /home/pmes/unaos-bench/scratch/rmbp-0915/gate9/unaos/crates/kernel/src/drivers/nvme.rs declares no owner — add a header line `CHARTER: <CODEX handler|Kernel> — <s
    gate=arch rc=1 ❌ GATE-ARCH: new sharedcore|video/facetedit_layers.rs — see docs/dev/STRUCTURAL_GATES.md §GATE-ARCH (fix it; a NEW key never gets a baseline row)
    gate=attrkeys rc=0 GATE-ATTRKEYS: files=2071 findings=9 baseline=9 new=0 stale=0
    gate=status rc=1 status-check: rows=413 confirmed=173 open=12 parked=4 refuted=17 unflown=207 claiming-uncited=4 baseline=0 -> FAIL
    gate=prefs rc=1 prefs-schema-check: declared=81 referenced=76 undeclared=1 -> FAIL
    gate=appearance rc=1 appearance-check: literals_outside_theme=5 files=1 certified=0 allow_rows=21 allowed_hits=24 stale=0 plants=32 painters=9 unlisted=0 ring3=4 core_literals=333
    gate=deps rc=1 deps-audit --check (policy any): 90 crates, 2 failing
    gate=knob rc=0 ✔ HOST-LINT: toolchain=0 hostidiom=0 allowed=44
    gate=verbs rc=1 GATE-VERBS: RED — dispatch arm with NO HOST_VERBS entry (B47's shape: an arm that can never be reached): nle vid 
    gate=banner rc=0 banner-cert --registry: rows=120 measured=95 unmeasured=17 findings=0 -> PASS
    gate=k8reach rc=1 ❌ k8-reach UNREGISTERED: UNAOS_DISPLAY_ICC UNAOS_FACETEDIT UNAOS_NVME_4K UNAOS_NVME_DISK — knob(s) with no K8_FEATS arm and no registry row. Setting one and flashing kernel8 chan
    gate=sanity rc=0 GATE-SANITY: files=445 const_asserts=17 fixture_asserts=0 findings=0 -> PASS
    gate=image-line rc=0 ✅ image-line OK: 52 kernel knobs in arroyo's map AND the builder's, 1 [media] knob(s) arroyo stages and nothing compiles, nothing unknown, in knob order
    gate=knob-parity-orin rc=0 ✅ image-line[orin] OK: 19 kernel knobs in arroyo's top-level _feats map (esp_jetson), every feature in Cargo.toml and banner-cert; 14 shared in arroyo's map AND the buil
    gate=knob-parity-pi rc=0 ✅ image-line[pi] OK: 23 kernel knobs in kernel8()'s K8_FEATS arms, every feature in Cargo.toml and banner-cert; 15 shared in arroyo's map AND the builder's, 8 BOARD_ONLY_p
    gate=mbench-selftest rc=0 ✅ SELF-TEST PASS — 55/55
    gate=specscore rc=0 ✅ SPECSCORE SELF-TEST PASS — 48/48
    gate=blueprint rc=0 BLUEPRINT: sheets=15 ids=297 anchors-checked findings=0 -> PASS
    gate=nonet rc=0 NONET: furniture_fns=1118 edges=13490 walk=paths net=smolnet+net_fetch+net_dns+netring3 allow=1 findings=0 -> PASS
    gate=nonetreach rc=0 NONET-REACH: roots=5358 edges=20356 reach=0 -> PASS
    gate=neutral rc=0 NEUTRAL: files=364 findings=111 allowed=111 new=0 stale=0 -> PASS
    gate=firmware rc=0 FIRMWARE: census=git files=4 pins=4 digests=12 other=0 findings=0 -> PASS
    gate=locktype rc=1 LOCKTYPE: files=445 raw=2 allowed=7 stale=0 -> FAIL
    gate=knob-parity-zenbook rc=0 ✅ image-line OK: 28 kernel knobs in arroyo's map AND the builder's, 1 [media] knob(s) arroyo stages and nothing compiles, nothing unknown, in knob order
    gate=speccov rc=0 SPEC-COVERAGE: lanes=2 unscored=0 -> PASS
    GATES: 25 run 16 green -> FAIL  (logs: target/gates/)

## foldleg
    FOLDLEG: tree=/home/pmes/unaos-bench/scratch/rmbp-0915/gate9/unaos line=/home/pmes/unaos-bench/scratch/rmbp-0915/gate9/unaos/scripts/image-line.env @3e6d45b10 blob=08c35316 knobs=53 caller_knobs_cleared=58 skipped=none
    ⚡ kernel features: witness,selfhost,wcg-paygo,wcdvalve,logts,ehcihid,kbdwit,smc,sdw,sdhcblk,bt,btc,a2dp,tickless,login,installdemo,smolnet,wifi,wifi2,wifi3,wifi4,wifi9,wifi10,nvidia-kepler,nvidia-kepler-takeover,noaspm,nvidia-kepler-ce,
    FOLDLEG: builder features (the list cargo got): witness,selfhost,logts,wcg-paygo,wcdvalve,quarry,facet,ehcihid,kbdwit,smc,sdwrite,sdw,sdhcblk,ahci,ahci-write,ahciroot,unafs,hda,hda-tone,usbnet,wifi,wifi2,wifi3,wifi4,wifi9,wifi10,bt,btc,
    FOLDLEG: selfbuild=not-armed path=builder --build-only llvm_src_touched=no build_s=80 log=target/foldleg-builder.log
    FOLDLEG: x86 kernel+loader built under image-line.env @3e6d45b10 blob=08c35316 features=59 -> PASS rc=0
    FOLDLEG: runtime_s=82

## knoboff beam
             Which parent is the tree WITHOUT your change depends on the merge direction, and
             knoboff will not guess (both directions are sanctioned — LAWS §3). Name it:
             [0;33m./arroyo knoboff beam c2d86af473b71be98c9ddbfe485df1eb93486928[0m   # you merged the track/trunk IN (executor)
             [0;33m./arroyo knoboff beam ae50c3fdec437c4f0b6dab44233598a34913f088[0m   # you merged your arc INTO trunk (landing)
             Measured 2026-09-12 on exec-orin-net6: the HEAD~1 default returned 1 with
             1,210,312 x86 bytes differing; the second parent returned 0 with the control
             fired. A refusal is the conservative answer — a 1 sends you hunting a line
             shift that is not there, and exit 2 can never be read as green.

## check — the red rows: cfg feature set and first error (for SEATLEGS)
- **x86-all** rc=101 errors=2 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0004]: non-exhaustive patterns: `BlockSource::Nvme` not covered` at `--> crates/kernel/src/arch/x86_64/sched.rs:6397`
- **arm-pi** rc=101 errors=4 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0433]: cannot find `keyring` in the crate root` at `--> crates/kernel/src/fs/users.rs:3945`
- **x86-vsyncpace** rc=101 errors=2 — target+features: `../../x86_64-unaos.json nvidia-kepler,nvidia-kepler-takeover,wc,witness,vsyncpace,ehcihid,smolnet` — first: `error[E0433]: cannot find `live` in `quarry`` at `--> crates/kernel/src/tests.rs:192`
- **arm-tegra-render** rc=101 errors=4 — target+features: `../../aarch64-unaos.json witness,logts,wedge2,sched_demo,rast,installdemo,tegra,tegrasmp,smpprobe,xcarve,xcarve_scrub,xcarve_crcrq,xcarve_relink,pcieprobe,pcie2,pcie3,net4,sdmmc,sdmmc_arm,install_target,vnet,tegra_el0,desktop_firmware,livecon,quarry,orinconwin,orindesk,orinclick,orinrender,deskcascade,holocron,login,loginst` — first: `error[E0433]: cannot find `keyring` in the crate root` at `--> crates/kernel/src/fs/users.rs:3945`
- **arm-tegra-supstate-el0-desk** rc=101 errors=3 — target+features: `../../aarch64-unaos.json witness,logts,wedge2,sched_demo,rast,installdemo,tegra,tegrasmp,smpprobe,xcarve,xcarve_scrub,xcarve_crcrq,xcarve_relink,pcieprobe,pcie2,pcie3,net4,sdmmc,sdmmc_arm,install_target,vnet,tegra_el0,desktop_firmware,livecon,quarry,supstate,busreg` — first: `error[E0425]: cannot find value `mtpphone_selftest` in this scope` at `--> crates/kernel/src/tests.rs:176`
- **arm-desk-noel0** rc=101 errors=5 — target+features: `../../aarch64-unaos.json witness,logts,wedge2,sched_demo,rast,desktop_firmware,livecon,wcg-paygo,quarry,orinvpar` — first: `error[E0432]: unresolved import `crate::arch::aarch64::syscall`` at `--> crates/kernel/src/tests.rs:4193`
- **arm-virt-el0** rc=101 errors=2 — target+features: `../../aarch64-unaos.json witness,logts,wedge2,sched_demo,rast,virt_el0,login,loginst` — first: `error[E0433]: cannot find `keyring` in the crate root` at `--> crates/kernel/src/tests.rs:6922`
- **arm-virt-apps-min** rc=101 errors=2 — target+features: `../../aarch64-unaos.json virt_el0,login,midden,una,lumen,aether,stria` — first: `error[E0425]: cannot find function `session_gen_reads` in module `crate::fs::users`` at `--> crates/kernel/src/tests.rs:7198`
- **arm-virt-apps-nosd** rc=101 errors=2 — target+features: `../../aarch64-unaos.json virt_el0,login,midden,una,lumen` — first: `error[E0425]: cannot find function `session_gen_reads` in module `crate::fs::users`` at `--> crates/kernel/src/tests.rs:7198`
- **arm-pi-sdw-ro** rc=101 errors=4 — target+features: `../../aarch64-unaos.json witness,logts,wedge2,sched_demo,rast,installdemo,skip_xhci,bootlog,pi,baremetal,unafs,vugpar,smp7,smp7_retry,k2_leave,smp8,core3probe,v3d,v3d_deep,v3d_qpu,v3d_dispdone,v3d69_reenable,v3d79_minimal,v3d81,v3d81_qpu,v3d81_display,v3d_firstkick,piusb,genet,nettest,nsspan,piinstall,piinstall_arm,piinstall_confirm,pirast,desktop_firmware,v3d_unarmclose,quarry,v3d_basedaim,v3d_tsaim,v3d_armedclose,v3d_bincontent,v3d_dispatchdisc,v3d_hfirst,v3d_vpmprobe,fatperf,livecon,holocron,hcronst,deadman,wcg-paygo,prtscrst,facet,v3d_singleseg,login,loginst,sdw-ro` — first: `error[E0433]: cannot find `keyring` in the crate root` at `--> crates/kernel/src/fs/users.rs:3945`
- **x86-all-nowitness** rc=101 errors=3 — target+features: `../../x86_64-unaos.json nvidia-kepler,nvidia-kepler-takeover,nvidia-kepler-fifo,wc,instgui,intel-ivb,unaos_ivb,gmux_igd,wcg-paygo,skip_xhci,bootlog,logts,wedge2,sched_demo,usbdebug,mtraw,irqstorage,botfaultinject,botring64,botwedge,videobench,videocap,noportsw,ehciscout,ehciscout_run,ehciconfig,ehciconfig_run,ehcihid,kbdwit,smolnet,thermprobe,pcilink,vromprobe,smc,smcwalk,rast,rastmc,installdemo,pfwire_selftest,sdw,pcicensus,bcmarecon,bcmaS1,wxnro,sdhcblk,bt,btc,a2dp,wifi,nvidia-kepler-ce,gen7,wifi2,quarry,livecon,wifival,noaspm,wifi3,holocron,hcronst,deadman,wcdvalve,wedgeinj,btdir,prtscrst,uvc,uac` — first: `error[E0433]: cannot find `wcg` in `super`` at `--> crates/kernel/src/video/wcpar.rs:149`
- **x86-zenbook-image** rc=101 errors=3 — target+features: `../../x86_64-unaos.json witness,selfhost,wcg-paygo,wcdvalve,logts,tickless,login,installdemo,wifi,wc,deadman,linuxabi,busreg,lumen,stria,netring3,selfdiag,videoplayer,quarry,facet,ftdirx,usbnet,nvme,unafs,hda,hda-tone,uvc,ioapic` — first: `error[E0599]: no method named `read_block` found for struct `unafs::BlockAdapter<S>` in the current scope` at `--> crates/kernel/src/drivers/dwi2c.rs:1623`
- **x86-mix-0** rc=101 errors=3 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0425]: cannot find value `mtpphone_selftest` in this scope` at `--> crates/kernel/src/tests.rs:176`
- **x86-mix-1** rc=101 errors=4 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0425]: cannot find value `mtpphone_selftest` in this scope` at `--> crates/kernel/src/tests.rs:176`
- **x86-mix-2** rc=101 errors=7 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0433]: cannot find `keyring` in the crate root` at `--> crates/kernel/src/fs/users.rs:3945`
- **x86-mix-3** rc=101 errors=2 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0004]: non-exhaustive patterns: `BlockSource::Nvme` not covered` at `--> crates/kernel/src/drivers/sdhc.rs:4045`
- **x86-mix-4** rc=101 errors=8 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0433]: cannot find `status` in `video`` at `--> crates/kernel/src/drivers/ehci/../bt/avctp.rs:293`
- **x86-mix-6** rc=101 errors=11 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0432]: unresolved import `crate::video::status`` at `--> crates/kernel/src/tests.rs:5940`
- **x86-mix-8** rc=101 errors=9 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0432]: unresolved import `crate::video::status`` at `--> crates/kernel/src/tests.rs:5940`
- **x86-mix-9** rc=101 errors=6 — target+features: `(derived mix leg: see GATE-CFG-MIX in arroyo)` — first: `error[E0433]: cannot find `lidsleep` in `video`` at `--> crates/kernel/src/tests.rs:6351`

## check (tail)
    gate=knob-parity-zenbook rc=0 ✅ image-line OK: 28 kernel knobs in arroyo's map AND the builder's, 1 [media] knob(s) arroyo stages and nothing compiles, nothing unknown, in knob order
    gate=speccov rc=0 SPEC-COVERAGE: lanes=2 unscored=0 -> PASS
    GATES: 25 run 16 green -> FAIL  (logs: target/gates/)
    [0;31m❌ check FAILED — a host gate is red or missing (./arroyo gates rc=1; see target/gates/<name>.log)[0m
