# KEPLERGOLD — the GK107 bunker cutter and the GR golden context (rmbp-ledger B656)

Branch exec-rmbp-keplergold, parent ce310661 (8da376cb + RESUME.md). Rulings: **R119** (the bunker is every user's),
**R127** (Peter, "1": the four files are cut from NVIDIA's own driver into the bunker, b43-fwcutter style,
`drivers/nvidia-gk107/` with SHA256SUMS and PROVENANCE, digests pinned in-tree), R95 §2 / R110 (nouveau and envytools
read for FACTS only), R80 (nothing at boot: `tests kgr`). Predecessor: KEPLERUCODE (B637,
docs/dev/evidence/keplerucode/keplerucode.md). The facts were gathered by the previous executor and are recorded in
[RESUME.md](RESUME.md) §3–§4; this file says what was built from them.

**Citation form.** No verbatim channel to nouveau or envytools was open from the cloud session (§3). Every fact is
cited by FUNCTION or ARRAY name (RESUME.md §3/§4), never by file:line, except the KEPLERUCODE citations that carried
lines (keplerucode.md §4, e.g. `gf100_grctx_generate` = ctxgf100.c:1436-1558). One extra fact question was asked this
session (WebFetch of nouveau gr/gf100.c, 2026-10-14): `gf100_gr_oneinit`'s topology reads, `gf100_gr_oneinit_tiles`,
`gf100_gr_oneinit_sm_id`'s loop order and `gf100_gr_init`'s order — answered without line numbers (§2.0).

## 1. The bunker recipe — `unaos/scripts/kepler-bunker.sh`

Usage line (the script's own):

    usage: kepler-bunker.sh [--order nvc0,nvc8,...] <NVIDIA-Linux-x86_64-325.15.run | NVIDIA-Linux-x86_64-325.15/> <bunker>/drivers/nvidia-gk107

Runs on the BENCH only and downloads nothing; `--help` prints the package URL
(`http://us.download.nvidia.com/XFree86/Linux-x86_64/325.15/NVIDIA-Linux-x86_64-325.15.run`, the x86 form beside it —
extract_firmware.py prints the x86 form). The walk is envytools `extract_firmware.py`'s PGRAPH-archive arm (MIT,
Copyright 2013 Ilia Mirkin), GK107 only:

| step | extract_firmware.py identifier (RESUME.md §3) | the script |
|---|---|---|
| versions | `VERSIONS`, `ARCHES` | refuses a version/arch outside them |
| order | `ARCHIVE_ORDERS["325.15"]` (15 chips, nve7 = index 10) | 325.15 built in; any other version needs `--order` |
| input | `kernel/nv-kernel.o` of the extracted package | `.run` extracted with `--extract-only --target <bunker>/.extract-<ver>` (removed after), or an extracted tree |
| streams | gzip magic `1f 8b 08` (`re.finditer`), each to the next magic or EOF | same, `bytes.find` |
| archive | `zlib.decompressobj(-zlib.MAX_WBITS).decompress(stream[10:])`, first LE u32 == 0 | same |
| entries | `<II` header, `<III` (id, length, offset) at 8 + 12 i | same |
| names | `ARCHIVE_FILES = {0: fuc409d, 1: fuc409c, 2: fuc41ad, 3: fuc41ac}` | same; written WITHOUT the `nve7_` prefix (kepler_ucode.rs `SET`) |
| padding | code files `+= 0x200 - len % 0x200` zero bytes, unconditionally | same (the bytes equal a nouveau user's `nve7_fuc409c`) |
| count | "Unexpected quantity of archives in blob, graph fw likely wrong." (a warning there) | a REFUSAL here (exit 3) |

The script MEASURES the package's sha256 (extract_firmware.py checks none — the arc's "its exact version and sha256
from extract_firmware.py" has no sha256 to cite) and writes PROVENANCE: `driver=`, `url=`, `package_sha256=`,
`nv_kernel_o_sha256=`, `recipe=`, one `entry archive=10 stream_offset= id= name= length= offset= written=` line per
file, `date=` — R127's "the driver version, its URL, the offsets, the date".

Exercised once on a SYNTHETIC tree (a fabricated `nv-kernel.o` of fifteen gzip archives plus one non-archive stream,
built in the executor's scratch and deleted): rc=0, archive 10 cut, `fuc409c` 600 -> 1024 bytes, `fuc41ac` 512 -> 1024
(the unconditional pad), data files unpadded; a malformed name refused rc=2. No NVIDIA byte was involved.

**The pin rows** — the script's last four lines, pasted unedited into `unaos/firmware/kepler.pins`:

    fecs_code <sha256 of fuc409c>
    fecs_data <sha256 of fuc409d>
    gpccs_code <sha256 of fuc41ac>
    gpccs_data <sha256 of fuc41ad>

kepler.pins carries this as comment rows only; no digest was typed (none was measured here).

## 2. The golden context — `unaos/crates/kernel/src/drivers/gpu/kepler_ctx.rs`

`kepler_fifo::ctx` (`#[path]` beside `gr4`/`ucode`, inside kgr's `all(x86_64, nvidia-kepler, nvidia-kepler-takeover)`
gate; gpu/mod.rs untouched). Called once from `kgr::tests_kgr` right after `ucode::load_and_start` returned
`Ok(ctx_size)` and before `fifo_init` (nouveau: `gf100_gr_chan_new` generates after `gf100_gr_init_ctxctl_ext`).

### 2.0 Stages and their refusals (each prints `[kgr] ctx stage=<name> rows=N`, a refusal `-> refused(<why>)`)

| stage | nouveau (by name) | writes | refusal |
|---|---|---|---|
| topology | `gf100_gr_oneinit`: gpc_nr = 0x409604 & 0x1f, rop_nr = bits 20:16, tpc_nr[g] = GPC_UNIT(g,0x2608), ppc_tpc_mask = GPC_UNIT(g,0x0c30); `_tiles` (row offset table, weighted round-robin); `_sm_id` (TPC outer, GPC inner) | none | `topology …` |
| lists | — (§2.1) | none | `lists-short(<first slot>)` |
| presume | `gk104_gr_init` must have run (RESUME.md §4.5) | none | `gr-init-unported` |
| window | ctx_size ≤ 0x80000, attrib_cb fits GOLD_SPAN | none | `ctx-size=…`, `attrib-cb=…` |
| fe-power / fecs-reset / fe-auto / power | §4.1 steps 1, 3, 4, 5 | 0x404170, 0x409614, 0x40802c | `fe-power-wall`, `fecs-halted-after-reset …` |
| image | steps 6–7: zero CB_RESERVED + size, 0x210/0x214 = image \| 4 | VRAM | — |
| bind | step 8: `gf100_gr_fecs_bind_pointer` (method 0x3), image +0x1c/+0x20/+0x28/+0x2c | FECS mailbox | `bind-eio`, `bind-wall` |
| mmio, buffers, floorsweep, icmd, mthd, main | step 9: `gf100_grctx_generate_main` with gk104's hooks (§4.2 items 1–18) | GR | `idle-wall(…)`, `icmd-wall(…)` |
| save | step 10: `gf100_gr_fecs_wfi_golden_save` (method 0x9), 0x409b00 bit 31 cleared | FECS mailbox | `save-eio`, `save-wall` |
| copy | steps 11–12: size bytes read out, 0x210/0x214 zeroed | VRAM | `image-all-zero` |
| done | `gf100_gr_chan_bind` (§4.6): golden copied to the channel context, header words, 0x210/0x214 = chan \| 4, `ucode::set_golden` | VRAM | — |

Caller refusals (kgr): `no-gold-window` (the arm-time reservation failed), `gold-beyond-pde0` (the window is outside the
identity PT's span).

### 2.1 The lists

31 lists in gk104's order (RESUME.md §4.3): hub 11, gpc_0 5, zcull 1, gpc_1 3, tpc 5, ppc 3, icmd 1, mthd 2. In hand
and transcribed: `gf119_grctx_init_prop_0` (8), `gf119_grctx_init_gpc_unk_1` (5), `gf119_grctx_init_crstr_0` (8)
(RESUME.md §4.4). The other 28 are EMPTY slots carrying their array name, source file and the expected row count (21
rows in hand / 677 rows expected across the 21 counted lists; 10 lists' counts unknown). The census refuses on the
first short slot BEFORE any GPU write, so a partial table never runs. The gf100 `gr_init` lists answered this session
(`gf100_gr_pack_mmio`) are a DIFFERENT family (`gf100_gr_init_*`, not `gf100_grctx_init_*`) and were not used.

### 2.2 RESUME.md §4.1's open question — is the firmware restarted after the FECS reset?

**No.** `gf100_gr_fecs_reset` writes 0x409614 = 0x70, then masks 0x700 in — the context-switch engine and context
reset controls, not the falcon's CPUCTL (0x409100) — and nouveau's generator, a few writes later (step 8), sends
method 0x3 through the same mailbox the firmware answers; nothing in the generator reloads IMEM or restarts the falcon
(RESUME.md §4.1 step 3: "gf100.c `gf100_gr_fecs_reset` only; nothing reloads IMEM after it"). So nouveau's order only
works if the firmware survives, and this tree does not re-run kepler_ucode's start. It READS FECS CPUCTL after the
reset and refuses `fecs-halted-after-reset cpuctl=…` if HALTED (bit 4) is set. Inference from nouveau's order, not
measured: the first bench flight that reaches `stage=fecs-reset` decides it.

### 2.3 Memory

`kgr::arm` reserves `GOLD_SPAN` = 0x240000 after the kgr window; `tests_kgr` maps it identity under the same PDE[0].
Layout: image allocation 0x0 (CB_RESERVED 0x80000 + up to 0x80000), channel copy 0x100000, bundle 0x180000 (0x3000),
pagepool 0x184000 (0x8000), attrib_cb 0x18C000 (0x20 × 0xb23 × tpc_total). The generator binds the kgr channel's own
instance block (nouveau uses a scratch one).

### 2.4 Design inferences (unverified)

* The channel's mmio list is EMPTY (+0x10/+0x14/+0x18 = 0): nouveau's per-channel list re-applies pagepool / bundle /
  attrib for the channel's own buffers; here the channel shares the golden generation's buffers and VM, whose register
  values the save captured.
* `wait_idle`'s "no FIFO ctxsw in progress" term is taken as true: no channel is bound when the generator runs.
* `num_tpc_per_gpc` packs four words (8 GPCs each, GPC_MAX 32); RESUME.md wrote "i in 0..3".

## 3. The channel block (RESUME.md §2, re-stated)

curl and gh of raw.githubusercontent.com were denied in the previous session, the GitHub MCP reaches only pmes/unaos,
`add_repo torvalds/linux` was denied, and WebFetch answers fact questions without line numbers and without
reproducing long tables. This session made ONE further WebFetch (gf100.c topology/tiles/sm_id/init order, §2.0) and no
other fetch. Consequence: the register lists stay slots; their transcription with file:line is the next rung.

## 4. The expected wire

QEMU: no Kepler, kgr is never armed — the existing kgr SKIP stands, nothing new prints.

rMBP, today (lists unfilled), after the bench pins the set:

    [kgr] armed chid=1 rl=… eng=… reset=… win=0x… gold=0x… -> tests kgr (…)
    [kgr] ucode fuc409c role=fecs_code bytes=… sha256=… -> pinned          (x4)
    [kgr] ucode start … ctx_size=0x… size_us=… -> running
    [kgr] ctx stage=topology rows=0 gpc_nr=1 rop_nr=… tpc_nr=[…] tpc_total=… ppc_tpc_mask=[…] row_off=… tile=[…] sm=…
    [kgr] ctx stage=lists rows=21/677 lists=31 uncounted=10 short=28 -> refused(gf100_grctx_init_main_0)
    :: KGR: fw=MATCH ext-start=ok golden=refused(lists-short(gf100_grctx_init_main_0)) set-object=gated bind=ok … -> DECLINED reason=no-golden-context ::

With the set absent: `:: KGR: fw=absent ext-start=- golden=- set-object=declined …` (FECS halted, KGR4 `PASS
(ucode-gated)` as before). With the lists transcribed and gk104_gr_init ported, the arc's shape:
`:: KGR: fw=MATCH ext-start=ok golden=ok(size=0x…) set-object=ok bind=ok … -> PASS ::`.
