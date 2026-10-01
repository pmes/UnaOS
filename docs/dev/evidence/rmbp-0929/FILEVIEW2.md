# FILEVIEW2 — a Quarry TEXT open latched and never drained

## Finding
Boot 18 (`f18-boot1.log`): `[894236ms] [quarry] open TEXT path=/hello.txt -> fileview (latched for the render pass)` and nothing after. The PNG latched at 398580 ms (`-> facet (latched`) only reached `[facet] open` at 416016 ms: 17 s later, i.e. at the next dock press.

## Mechanism
`Act::Text` only latches (`video/quarry/live.rs` run_act, click-router depth). The drain is `quarry::service()` -> `fileview::service()` / `textedit::service()`. On x86 its only callers were the dock-press arm (`arch/x86_64/syscall.rs:7511`) and tests; the Orin pass (`main.rs` ~8898) drains it but `x86_render_service` and the inline BSP loop never did. Not a cfg or wrong-arm bug: the consumer was never called. Selection (`may_edit`) is correct: `/hello.txt` -> viewer.

## Milestones
- M1: both consumers print `[quarry] open TEXT consumed=viewer|editor|refused path= reason=`.
- M2: `console_launch_drain` (called by both x86 render loops) now also calls `quarry::service()` under `x86_64 + quarry`, folded line-neutral. `view`/`edit` shell verbs already open directly (shell.rs view_verb/edit_verb) and are unchanged.
- M3: `tests fileopen` (`quarry::live::fileopen_selftest`), spec pin in `x86-wc.spec`.

## Written
Boot 17 should show `:: FILEOPEN: root=viewer home=editor windows=2 staged=11 -> PASS ::`, and a Quarry double-click on a text file now prints `[quarry] open TEXT consumed=viewer path=...` followed by `[fileview] open win=`.
