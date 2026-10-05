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
| 2 | UIMETRICS (rmbp-ledger B372, R85 item 11) | The furniture follows the panel's ppi through ONE runtime `ui::Metrics` (`video::dpi`'s scale, 2.5 at the bench's 2880x1800 / 221 ppi): the menu bar and every title strip 34 -> 85 px, the frame 5 -> 13, the gap 12 -> 30, the control discs 24 -> 30 (base 12, the seat; 12 px at 1.0), the menu bar's battery and crystal 24 -> 60 (their own base), the chrome text cell 9x20 -> 23x50 (captions, the bar, the crystal menu, dock labels), the dock tile 28 -> 70; and the kernel windows (Settings, Activity, login, Quarry, the viewer, the editor, the installer, Pulse) are drawn at scale 1 at their real pixel size (Settings 1300x1120, login 1100x600) with `Face::Ui` at `font_size` x ppi / 96 (29.9 px at 13) instead of being laid out at 520x448 / 440x240 on the 7x16 cell and blown up by the compositor. At a 96-ppi panel (scale 1.0) every length is byte-identical to before except the discs (24 -> 12). | ALL: `login`, `desktop`, `quarry`, `settings-general/users/display/about`, `lumen` (its window chrome) |

No golden had been blessed when pin 1 landed (the set is still empty), so nothing is re-blessed by it: the first
flown boot of a tree carrying KERNELFONT2 blesses all of them. Pin 2 (UIMETRICS) moves every state: a golden blessed
from a boot before UIMETRICS must not be scored against a tree with it — the first flown boot carrying it re-blesses
the whole set at 2880x1800 / 221 ppi. A golden blessed from a boot BEFORE KERNELFONT2 must
not be scored against a tree with it — the three states above drift by design.
