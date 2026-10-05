# facet-view

The UnaOS **vessel** for pictures: a window over the [Facet](../../handlers/facet) images
handler (CODEX §2, *The Canvas*). Formerly `vessels/facet`; renamed when the handler took the name.

## What it is

Wiring and lifecycle only, like every vessel: it starts a Bandy `Synapse`, serves Facet on it
(`facet::serve`), and runs `facet_view::Viewer` between the window and Facet. The vessel decodes,
edits and scales nothing itself — window input becomes `FacetCommand`s, Facet's `ImageRendered`
frames become `SurfaceBlit`s for quartzite's image surface.

## Usage

```
facet-view <image>                                              # the window (macOS)
facet-view <image> --size 320x240 --keys "++r]x" --out shot.png # headless witness, any platform
facet-view --help                                               # the shortcut table
```

The headless witness drives the very same controller over the very same bus from a key script
(one character per shortcut, `{Up}` `{Down}` `{Left}` `{Right}` for arrows) and writes the last
frame as a PNG.

## Shortcuts

| Keys | Action |
| --- | --- |
| `+` / `=`, `-` | zoom in / out (x1.25); the wheel zooms x1.1 |
| `0` / `1` | fit (shrink, never enlarge) / 100 % |
| `r` / `R` | turn the view clockwise / counter-clockwise |
| `h` / `v` | flip the view left-right / top-bottom |
| arrows | pan 32 px |
| `]` / `[` | edit: rotate the picture clockwise / counter-clockwise |
| `m` | edit: mirror left-right |
| `x` | edit: crop to the visible region (through any view turn/flip) |
| `b` / `B`, `c` / `C` | edit: brightness, contrast 0.9x / 1.1x |
| `u` / `U` / `!` | undo / redo / reset (undoable) |
| `e` | export `<name>.facet.png` beside the file |

View state (zoom, pan, view turn/flip) is display-only; edits go to Facet's non-destructive list.

## Status

The controller is tested against a live Facet (`cargo test -p facet-view`: frames byte-equal
Facet's renders, crop-to-visible through every view transform, export, errors). The window is
quartzite's AppKit image surface (macOS); other backends follow quartzite. Owed: a macOS
eye-witness run, a GPU blit for `SurfaceBlit` (the old FACET-GPU textured quad presented a static
frame with view-side zoom and is not carried over), drag-pan in surface mode.
