# golden/ — seeded EMPTY on purpose

These are the reference frames of the `metal` suite: one `<state>.png` per case in `../cases/metal.toml`
(`login.png`, `desktop.png`, `quarry.png`, `settings-general.png`, `lumen.png`, and the optional
`settings-users.png`, `settings-display.png`, `settings-about.png`).

None exists until a boot has flown the `shot` verbs. **The first flown boot blesses them:**

    # on the rMBP, logged in:   shot desktop ; shot login ; shot quarry ; shot settings general ; shot lumen
    # pull /home/<u>/Shots/ off the card into a directory, then:
    tools/eyes/run.sh metal --from <dir> --accept

`--accept` copies each pulled `<STEM>.PNG` here as `<state>.png` (and the mask beside it as
`<state>.mask.png`, for reading), then scores the run against what it just wrote (0 % everywhere).
Every later boot is `tools/eyes/run.sh metal --from <dir>` — drift per state in `tools/eyes/out/metal/SCORE.md`.
Re-bless only after a change that was MEANT to move the pixels, and say which in the commit.

## Pins

| pin | arc | what moved, and why | states to re-bless |
|---|---|---|---|
| 1 | KERNELFONT2 (rmbp-ledger B363, R85) | The console's cell follows the panel's ppi (`video::dpi`: 7x16 x 2.5 = 18x40 on the 220/227-ppi rMBP, grid 160x45 on 2880x1800) and its face is DejaVu Sans Mono at `system.display.font_size` (29.75 px at the default 13); the Lumen window draws DejaVu Sans / Sans Mono from the volume instead of font8x8 (15-px lines); the Settings Display tab's Font row is a picker (`< sans >  < 13 >`). Every surface painted before the faces loaded repaints once when they do. | `desktop` (the console window), `lumen`, `settings-display`; `login`, `quarry`, `settings-general/users/about` unchanged by this pin |

No golden had been blessed when pin 1 landed (the set is still empty), so nothing is re-blessed by it: the first
flown boot of a tree carrying KERNELFONT2 blesses all of them. A golden blessed from a boot BEFORE KERNELFONT2 must
not be scored against a tree with it — the three states above drift by design.
