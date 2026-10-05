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
