# WHITE BOARD — 2026-08-08 (GR22 close)

The Crispy wiring arc (`wt/crispywire`, `43611837`, **unreviewed and unmerged**) drew the theme and
then stopped at five places where the kit does not carry an answer. Per the shared-source law it
**reported instead of inventing**. You are the taste gate; these are yours.

## Q1 — the kit has NO desktop/wallpaper role.

The palette is 21 chrome/content roles and nothing for the desktop behind the windows. So near-white
Crispy windows now sit on the **old invented purple `0x2D2B55`**. That is the one surviving invented
colour on the panel, and it is the biggest thing you will see. Add a desktop role to
`kits/crispy/theme.json`, or say what it should be and it gets lifted.

## Q2 — the middle and zoom controls are drawn but inert.

Three `control_box` discs are painted upper-right (darkest = close). Close works. The other two have
no verbs — the kit defines their colours, not their behaviour. Options: minimise/park (the compositor
already has `set_hidden`), maximise/restore, something else, or leave them dead. They are currently
left draggable rather than turned into dead zones.

## Q3 — no hover / pressed / disabled state for the controls.

`button_face_pressed` is a *button* role, not a control-disc role. A control that never changes under
the pointer reads as a picture. Do you want those three states in the kit?

## Q4 — focus contrast now rides the INK, not the frame.

The two title gradients differ by a few LSBs; the two ink roles differ a lot. So a focused window is
distinguished mainly by its caption ink. That preserves FOCUS-HL's "focus never moves a pixel" law —
but if it doesn't read across the room at the bench, **the fix is the kit json**, not the compositor.

## Q5 — control side and order.

Right-aligned, close leftmost. That satisfies both the kit's left-to-right dark→light ramp and P79's
"upper right". A LEFT cluster satisfies the ramp equally well. Pure taste.

---

**Known residual, not a question:** rounded corners are cut against the desktop colour rather than
sampled, because WC-H requires the pass to write every pixel of the box. Visible only when a manual
drag stacks two windows — about 31 px of the desktop colour per top corner. Tiled windows never
overlap, so it does not appear in normal use.
