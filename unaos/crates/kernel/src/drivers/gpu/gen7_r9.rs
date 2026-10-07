//! GEN7R9 (B504, R101) — rung R9: the first 3D-pipeline step on the Ivy Bridge RCS.
//! CHARTER: Kernel — driver
//!
//! Design: `docs/dev/evidence/rmbp-1005/gen7r9.md`. A child of `gen7` (declared by `#[path]` in gen7.rs, as
//! `gen7_blit.rs` is), run by `tests gen7` right after R1..R7 (R80: nothing here runs at boot).
//!
//! THE RUNG: on the RCS (not the BCS), `PIPELINE_SELECT(3D)`, `STATE_BASE_ADDRESS`, and one `PIPE_CONTROL`
//! whose post-sync operation writes an immediate into a pinned GGTT page — the write landing is the proof the
//! 3D pipeline executed a command stream, the shape every later 3D rung needs first.
//!
//! THE RING HALF IS R6's. Flight 27 (`f27-boot1.log`): `gen7: r6 verdict=r6-sentinel-hit by=mt …` — R6 armed
//! the RCS (0x2030..0x203C) in a boot-bank ring page under R3's held `mt` wake and executed
//! MI_STORE_DATA_IMM. R9 is GATED on that verdict this run, never on a forcewake ack decode (§4/R3).
//!
//! THE 3D HALF HAS NO PAGE IN THE TREE. None of the three commands' opcode, dword count or bit fields is
//! transcribed in `gen7.md`, `gpu_spec.md`, `SHUTOUT-REGISTER.md` or gen7.rs; `gen7b.md` §4 lists them owed
//! (IVB 2012 PRM Vol1 Pt3, Vol2 Pt1). R83 / R95 §2: no bit is guessed — the rung DECLINES by name at the
//! first uncited row and writes nothing. The executing path is written when a row's `src` is filled.

/// One command R9 needs, and the in-tree page that pins its encoding (`None` = uncited).
struct Page {
    /// The command's PRM name, printed in the decline reason (`page-<name>-uncited`).
    name: &'static str,
    /// The witness field this row answers (`select=` / `sba=` / `pipe_control=`).
    field: &'static str,
    /// The PRM volume the row is owed from (gen7b.md §4), for the reader of the wire.
    owed: &'static str,
    /// The in-tree transcription that pins the encoding. `None` until a page is in the tree.
    src: Option<&'static str>,
}

/// The rung's citation table, in command-stream order.
const PAGES: [Page; 3] = [
    Page { name: "PIPELINE_SELECT", field: "select", owed: "IVB-PRM-Vol2-Pt1", src: None },
    Page { name: "STATE_BASE_ADDRESS", field: "sba", owed: "IVB-PRM-Vol2-Pt1", src: None },
    Page { name: "PIPE_CONTROL", field: "pipe_control", owed: "IVB-PRM-Vol1-Pt3", src: None },
];

/// R6's verdict this run (`rearm` notes it at its verdict line).
static R6: crate::sync::Mutex<Option<&'static str>> = crate::sync::Mutex::new(None);
/// R9's one-word result this boot, for the `GEN7LADDER` line and the replay.
static WORD: crate::sync::Mutex<Option<&'static str>> = crate::sync::Mutex::new(None);

/// Called by R6 (`rearm`) with its final verdict.
pub(super) fn note_r6(v: &'static str) {
    *R6.lock() = Some(v);
}

/// The `r9=` word on the `GEN7LADDER` line (`declined`, `gated`, or `not-reached` before R9 ran).
pub(super) fn word() -> &'static str {
    (*WORD.lock()).unwrap_or("not-reached")
}

/// R9 under `tests gen7`: gate on R6, then the citation table; one `[gen7] r9` witness line. Writes nothing.
pub(super) fn run() {
    let r6 = (*R6.lock()).unwrap_or("not-reached");
    let mut cited = [""; 3];
    for (i, p) in PAGES.iter().enumerate() {
        cited[i] = if p.src.is_some() { "cited" } else { "uncited" };
    }
    if r6 != "r6-sentinel-hit" {
        *WORD.lock() = Some("gated");
        serial_println!(
            "[gen7] r9 ring=rcs r6={} select={} sba={} pipe_control={} sync_val=- read=- writes=0 -> GATED reason=r6-did-not-execute-on-the-rcs",
            r6, cited[0], cited[1], cited[2]
        );
        return;
    }
    // The first row without an in-tree source declines the rung, by name (R83: no bit guessed).
    if let Some(p) = PAGES.iter().find(|p| p.src.is_none()) {
        *WORD.lock() = Some("declined");
        serial_println!(
            "[gen7] r9 ring=rcs r6={} select={} sba={} pipe_control={} sync_val=- read=- writes=0 -> DECLINED reason=page-{}-uncited field={} owed={}",
            r6, cited[0], cited[1], cited[2], p.name, p.field, p.owed
        );
        return;
    }
    // Every row cited: the executing path (R6's RCS arm, the three commands, the post-sync read-back) is
    // owed with the pages — the table cannot be filled without writing it (design §Owed).
    *WORD.lock() = Some("declined");
    serial_println!(
        "[gen7] r9 ring=rcs r6={} select=cited sba=cited pipe_control=cited sync_val=- read=- writes=0 -> DECLINED reason=exec-path-unwritten",
        r6
    );
}
