//! `dsp` — the Signal Processing Graph's codec face (docs/CODEX.md §3: "Audio and Video codecs").
//! Each codec family lives in a `no_std` core under `unaos/libs/media/` that the kernel links too; this
//! module is where Ring 3 handlers (Stria, Aether, Facet) reach them.
pub mod audio;
