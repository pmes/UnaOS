# DESIGN-LANGUAGE — UnaOS's own interface language

*DESIGNLANG (LEDGER SR141), 2026-10-13. A survey of the field, then the decisions. Docs only: this file changes no
pixel. The craft arcs that follow apply it, one component at a time, using the appendix (§9) to find the code.*

Peter, verbatim, 2026-10-14:

> "settings also kind of looks roughly patched in rather than a smooth apple like look/crafted interface … it's
> giving more a early x-windows vibe opposed to refined mac" · "we want people to see our screenshots and be strongly
> compelled to try unaos" · "i don't want to spend time rewriting what other more qualified individuals have done.
> please go out on your own here on interface design to see what's out there and stop pinpointing this to the one
> work of gnome or apple"

> "this is your strength -- to take the idea of interfacing OSes for people and doing the best you can to come up
> with a powerful representation of what the current best practices have to offer knowing most people prefer mac or
> win and coming up with clones skirting the line of legality not to steal but for people's comfort. they deserve a
> ONE OS -- UnaOS"

So this document has one language and two comfort looks on top of it: **Una** (the default, where someone from the
Mac feels at home) and **Softy** (where someone from Windows does). Both are drawn from the same tokens. A section
on THE LEGAL LINE (§7) says what may be followed and what may not, with the cases.

**How to read the numbers.** Every length is in **pt**, UnaOS's logical point. One pt is `video::dpi`'s scale in
device pixels (§2.1). Every colour is given as sRGB hex and was derived in OKLCH (§2.3). Every duration is in ms.
Sources are cited as `[S#]` (design) and `[L#]` (law). The list is §10. A claim marked *(via search)* was read
from a search engine's summary of the page because the page itself was behind this session's egress policy. Those
claims are secondary and say so.

**Relation to `ui_guidelines.md`.** That file's §1 (16 ms) and §3 (workspace) stand. Its §2 ("we reject modern
white space") and §4 (OLED `#000000` default, "no decorative colours") are **superseded for visual design** by
this file. The new rule is density *modes* (§2.2) and a neutral dark that is not pure black (§2.3), for the reasons
those sections give. The edit to `ui_guidelines.md` is owed. This arc may touch only this file and LEDGER.md, so
the exact change is in the report (LAWS: a fix that needs another file is reported, not made).

---

## 1. THE SURVEY

One row per system. "Learn" is what we take, as principle or measurement. "Avoid" is what we do not take.

### 1.1 Published design systems

| System | What it does well | What to learn | What to avoid | Sources |
|---|---|---|---|---|
| **Apple HIG** (macOS) | A small, fixed text ramp (macOS Body 13 pt / 16 line). Controls sized for a pointer (28 pt default, 20 pt minimum). Contrast floors stated per size. A menu bar whose order never changes. Materials kept to the *functional* layer. | Body 13 pt for the Mac look. A 28 pt control default. 4.5:1 up to 17 pt and 3:1 from 18 pt. "Disable, don't hide" menu items. Standard shortcuts kept. Translucency only on the control layer, never the content layer. | Liquid Glass's live refraction: it costs a GPU we do not have, and it is the newest *distinctive* Apple trade dress. SF Pro: its licence forbids use outside Apple platforms [L12]. | [S1] [S2] [S3] [S4] [S5] [S6] [S7] [S8] [S9] |
| **Material 3** (Google) | Everything is a token: motion (50–1000 ms in named steps), easing (standard `0.2,0,0,1`), shape (4/8/12/16/28 px corners), state layers (hover 8 %, focus/pressed 12 %, drag 16 %), and elevation levels. | State layers: a translucent overlay of the content colour instead of a new colour per state. The easing set. Durations named by *role*, not by number. | Big radii everywhere (28 px dialogs read as a phone on a desktop). Tonal-palette colour from the wallpaper: it makes a screenshot's colours accidental. | [S10] [S11] [S12] [S13] [S14] [S75] |
| **Fluent 2** (Microsoft) | Shadows in two layers: an ambient 0-offset blur and a key offset blur, with fixed alpha pairs (light 12 %/14 %). Radii stated per object class (Windows 11: 8 px top-level, 4 px in-page, square when maximized). A dense 2/4/6/8/10/12/16/20/24/32 spacing set. Segoe UI Variable with an automatic optical-size axis. | The two-layer shadow recipe. The rule that radii follow object class and that snapped or maximized windows go square. Body 14/20 for the Windows look. Mica, a material sampled from the wallpaper *once* instead of every frame. | Acrylic: a live backdrop blur on every frame. Segoe and Fluent icons: Microsoft's licence keeps them on Windows [L13]. | [S15] [S16] [S17] [S18] [S19] [S20] [S21] [S44] [S45] [S46] [S47] |
| **GNOME HIG / libadwaita** | Radii *derived* from each other: window = button + 6, popover = menu + 6, which is the concentric-corner rule written as code. A three-layer window shadow plus a 1 px outline. The accent colour in OKLab, with lightness clamped so accent-as-text always reads (L ≤ 0.5 light, ≥ 0.85 dark). | Derived radii. The window-shadow structure. **The accent-lightness clamp**: we adopt it (§2.3). | Header bars that merge every control into the title. They hide the menu bar, which our R21 forbids. | [S22] [S23] [S24] [S26] |
| **KDE HIG / Breeze / Kirigami** | Everything is in units: gridUnit 18, spacing 4/6/8, durations 50/100/200/400. Breeze's metrics are exact (checkbox 20, button min-width 80, menu-item margins 4). Plasma 6 made a floating panel the default and gave it a shadow. | Durations on a power-of-two ladder. Spacing tied to the type size. The floating-panel idea for our dock. | Too many knobs on the surface. The HIG itself warns that spacing ratios shift with the user's font. | [S27] [S28] [S69] [S77] |
| **elementary HIG** | "Accessible configuration": every setting is code, bugs, tests and documentation, so ship sane defaults instead of preferences. | Settings stays short. A default must be good enough that nobody looks for the switch. | — | [S76] |
| **COSMIC (libcosmic)** | Spacing 0/4/8/12/16/24/32/48/64/128. **Density modes** (Compact, Standard, Spacious) that remap the same token names. Radii 4/8/16/32. Chose dual-Kawase blur for its frosted glass because it is cheaper than Gaussian. | Density modes are how we reconcile ui_guidelines' "information density" with Peter's "refined". A cheaper blur if we ever blur live. | — | [S29] [S30] [S70] |
| **Carbon** (IBM) | An 8 px mini-unit. A type scale generated by a formula (`Yn = Yn-1 + (floor((n-2)/4)+1)·2`: 12,14,16,18,20,24,28,32,36,42…). | A type ramp that is a *rule*, not a list. | Corporate flatness: zero radius, zero shadow. It reads as a web app, not an OS. | [S31] [S32] |
| **Atlassian** | An 8 px base unit with tokens named by percentage (`space.200` = 16 px). Four elevation levels (sunken, default, raised, overlay). In dark, higher surfaces get *lighter* because shadows vanish there. | **Dark elevation by lightness**: higher surfaces are lighter in dark mode. The four-level vocabulary. | — | [S48] [S49] |
| **Radix Colors / shadcn/ui** | A 12-step scale per hue, each step with a job: 1–2 backgrounds, 3–5 component fills, 6–8 borders (7 = focus ring), 9–10 solid fills and their hover, 11–12 text. shadcn's components are copied into the product and owned, not depended on. | Steps named by job; our token names (§2.3) do the same. Owning the component code matches our no-dependency stance (R83). | — | [S37] [S78] |
| **Polaris** (Shopify) | Spacing in 4 px steps named ×100 (`space-100` = 4 px). Motion in 50 ms steps. A plain ease of `0.25,0.1,0.25,1`. | A 4 px grid with an 8 px rhythm is the field's consensus. Polaris, Fluent, COSMIC and M3 all agree. | — | [S33] [S34] |
| **Base** (Uber) | One primary button per screen. A 4 px spacing scale *(via search)*. | One primary action per view (dialogs, Settings panes). | Its proprietary face (Uber Move) has no place here. | [S79] |

### 1.2 Desktop shells people praise

| Shell | What it does well | What to learn | What to avoid | Sources |
|---|---|---|---|---|
| **macOS** | The global menu bar and the Dock. Traffic-light controls top-left. Large, quiet window shadows. One accent. | The comfort layout of the Una look (§5). | Its exact hues, the squircle icon plate, the apple, its fonts and icons (§7). | [S1]–[S9] |
| **Windows 11** | A centred taskbar with Start. Snap Layouts from the maximize button. 8/4 px radii. Mica. A 32 px title bar. | The comfort layout of the Softy look (§6). Snap layouts are worth having in *both* looks. | Its Start glyph, Segoe and Fluent icons, its wallpaper (§7). | [S44]–[S47] [S80b] |
| **COSMIC** | Auto-tiling that falls back to floating per workspace. Density modes. A cheap frosted glass. | Tiling as an *option*, not the default. | — | [S29] [S70] |
| **Plasma 6** | A floating panel by default that "de-floats" against a maximized window, with a shadow. | The dock floats 8 pt above the bottom edge and goes flush when a window is maximized. | Breeze's dated look, which even its reviewers call for replacing. | [S69] |
| **elementary OS** | Restraint: few settings, consistent spacing, a calm palette. | Restraint. | — | [S76] |
| **Haiku** | The yellow tab: the title is a grabbable tab, not a full-width bar. Stack & Tile is built on it. | A distinct, identifiable window head is a *brand*. Ours is §8. | The 2001 widget set. | [S67] |
| **SerenityOS** | "A love letter to '90s user interfaces": total coherence from one hand. | Coherence beats novelty. Every surface from one token table. | The 90s look itself. It is the "early X" vibe Peter named. | [S68] |
| **Deepin** | Called "strikingly beautiful". A dock with two modes (Mac-like dock or Windows-like panel). | **One shell, two layouts**: the precedent for Una and Softy. | Thin functionality behind a pretty face, the complaint reviewers made. | [S72] |
| **Zorin OS** | Layouts that copy macOS, Windows 11 and Windows Classic, chosen in one Appearance pane, to comfort switchers. | Proof that the comfort-look idea works. It is the closest analogue to Una and Softy. | Zorin's macOS layout is a GNOME skin. Our looks share one engine and differ in layout, density and idiom, not by a theme file. | [S73] |
| **Steam Deck UI** | A controller-first interface good enough that Valve made it the desktop Big Picture mode. Universal search. | Focus must be visible and navigable without a pointer. It is the TV/Pi case of §2.9. | — | [S74] |

### 1.3 Browser chrome

| Browser | What it does well | What to learn | What to avoid | Sources |
|---|---|---|---|---|
| **Chrome (Refresh 2023 + vertical tabs 2026)** | Taller, rounder tabs and omnibox. Lighter tab dividers. Hover shows a rounded pill, not an outline. A site-controls chip replaced the lock. Outline icons. **Native vertical tabs (April 2026)**: a left sidebar with full titles that collapses and keeps groups. | Our top-tab and side-tab specs (§3.10). Peter: "i am really liking the side tabs" (R118). | — | [S53] [S54] |
| **Arc** | Tabs in a sidebar. Spaces with colours. Pinned versus today tabs (today tabs archive after 12 h). Auto-hide. | Side tabs designed for *dozens* of tabs: groups, pins, collapse. | Unfamiliar everything ("almost nothing in Arc works the way you expect"). Development stopped in May 2025 *(via search)*. | [S52] |
| **Zen** | Firefox-based vertical tabs, workspaces, a compact mode that hides the sidebar until the edge is touched. | Compact mode for the side strip: a 48 pt favicon rail. | — | [S56] |
| **Safari 15** | Compact tabs merged with the address field. Page-tinted chrome. | **What not to do.** Gruber: tabs that "look like buttons", an address that shrinks unreadable, and an active tab that cannot be told from an inactive one. Apple added the separate tab bar back. | Merging tabs and the omnibox. Tinting the chrome from the page. | [S55] |
| **Orion** | Safari's look plus tree-style vertical tabs. Lightweight. | Vertical *tree* tabs are wanted even by Safari users. | — | [S57] |
| **Vivaldi** | Tab stacks (one tab's width for a group), a two-level stack bar, tiling. | Stacks as the group model in the top strip. | Settings sprawl. | [S58] |

### 1.4 Applications people call beautiful

| App | What it does well | What to learn | What to avoid | Sources |
|---|---|---|---|---|
| **Linear** | A redesign to cut visual noise and raise density and hierarchy. A theme generator in LCH (via search) so hues have equal weight. | Generate themes perceptually (OKLCH, §2.3). Cut chrome noise before adding polish. | — | [S51] |
| **Raycast** | One command field. Shortcuts shown inline as keycaps. Treats ~50 ms as the latency budget *(third-party study)*. | Our Launcher (Cmd-Space) shows its chords as keycaps. The 16 ms rule (ui_guidelines §1) is already ours. | — | [S65] |
| **Things 3** | Charm through *interaction*: the Magic Plus button you drag to where the task belongs. | Delight is in direct manipulation, not decoration. | — | [S62] |
| **Bear** | Typography-first, colour used sparingly. 2017 Apple Design Award. | Let type carry the hierarchy, not boxes. | — | [S63] |
| **Craft** | Mac App of the Year 2021 for speed and simplicity. Native. | Native speed reads as quality. | — | [S64] |
| **Warp** | The terminal reorganised into blocks (command plus output). A real text editor for the input. | The console can be a designed surface, not a grid of cells. | — | [S61] |
| **Zed** | The UI is drawn like a game: a few primitives (rects, shadows, text, icons, images), each with its own shader, at 120 fps. | **Our compositor already works this way**: a few integer primitives. Make the primitives excellent and everything gets better. | — | [S60] |
| **Figma (UI3)** | Shipped floating panels, then reverted to fixed panels because they slowed heavy users (cramped canvas, content hidden behind panels). | Chrome that floats over content must earn it. Our dock floats, but sidebars and inspectors are fixed. | Floating panels over working content. | [S59] |
| **Notion** | Calm: whitespace, understated type, near-black on white, 1 px borders at 10 % *(third-party reconstruction)*. | Hairline borders at low alpha instead of bevels. | — | [S66] |

### 1.5 The practice, in one line each

| Topic | Field consensus | Sources |
|---|---|---|
| Grid | A 4 pt base with an 8 pt rhythm (Polaris, Fluent, COSMIC, Atlassian at 8, Carbon at 8). | [S18] [S29] [S31] [S33] [S48] |
| Type | One sans with optical sizes, a short ramp, body 13 (Mac) or 14 (Windows), semibold for titles, tabular numerals for figures. | [S2] [S21] [S35] [S36] [S45] |
| Colour | Semantic tokens named by job, light and dark sets that are *not* inversions, accents generated in a perceptual space, 4.5:1 / 3:1 floors. | [S5] [S6] [S24] [S37] [S38] [S39] [S50] |
| Depth | Two-layer shadows (ambient plus key). In dark, elevation by *lighter* surfaces. A 1 px outline so edges survive any background. | [S19] [S20] [S23] [S49] |
| Translucency | Only on the functional layer. Live blur is costly. Mica samples the wallpaper once. Dual-Kawase is the cheapest good blur. | [S3] [S47] [S70] [S71] |
| Radii | By object class, concentric (inner = outer − inset), square when maximized or snapped. | [S11] [S17] [S22] [S44] |
| Motion | 100–300 ms for UI, decelerate on enter and accelerate on exit, never block input, a reduced-motion fallback that crossfades or cuts. | [S4] [S10] [S15] [S16] [S28] [S43] [S80] |
| States | A translucent overlay per state, not a new colour per state. A visible focus ring at ≥ 3:1 change and ≥ 2 px. | [S12] [S42] |
| Icons | A 24 grid with a 20 live area (or 20/16 dense), one stroke weight, filled for the selected state. | [S75] [S53] |
| Targets | 24×24 minimum, 44×44 for touch or enhanced, macOS 28 default and 20 minimum. | [S1] [S40] [S41] |
| Screenshots | At least one window open, a cohesive palette matched to the wallpaper, not "riced out", and a desktop you would use. | [S81] |

---

## 2. UNAOS'S OWN LANGUAGE — the decisions

### 2.1 The unit and the grid

* **pt** is the logical unit. A device pixel count is `pt × scale`, where `scale` is `video::dpi`'s half-pixel scale
  (`unaos/crates/kernel/src/video/dpi.rs`). `ui::base` (`unaos/crates/kernel/src/ui.rs:37`) is already in this unit
  ("96-ppi CSS px"). The language keeps that seam and changes the numbers in it.
* **DECISION — density target: about 110 pt per inch.** Today `dpi::s2_for` rounds `ppi / 96` to the half, so the
  221 ppi rMBP panel runs at **2.5**. That gives a logical desktop of 1152×720, and everything looks oversized,
  which is part of the "early X-windows vibe". The Mac draws that panel at 2.0 (1440×900 logical). **The language
  sets `scale = ppi / 110` rounded to the half: 2.0 on the rMBP.** Settings → Display offers larger and smaller
  steps (1.5 · 1.75 · 2.0 · 2.25 · 2.5). This is the single most visible change in this file.
* **Grid: 4 pt.** Every size, gap and inset is a multiple of 4 pt. **2 pt** is allowed *inside* a control (an icon
  nudge, a focus offset). At scale 2, 4 pt = 8 px and 2 pt = 4 px: always whole pixels.
* **Hairline = 1 device pixel** at any scale. It is used for separators, outlines and borders. It is *not* `1 pt`:
  a 2–3 px "hairline" is the bevel look we are leaving.

**Spacing scale** (token → pt), in three density modes, following COSMIC [S29]. Standard is the default for both looks:

| Token | Compact | **Standard** | Spacious | Typical use |
|---|---|---|---|---|
| `space.0` | 0 | 0 | 0 | — |
| `space.1` | 2 | **4** | 4 | icon to label in a tight control |
| `space.2` | 4 | **8** | 8 | between related controls; menu container padding |
| `space.3` | 8 | **12** | 16 | control padding; list-row horizontal inset |
| `space.4` | 12 | **16** | 20 | group padding; window content margin (min) |
| `space.5` | 16 | **20** | 24 | window content margin; between groups |
| `space.6` | 20 | **24** | 32 | dialog padding; between sections |
| `space.8` | 24 | **32** | 40 | page title to first group |
| `space.12` | 32 | **48** | 64 | hero spacing (login, About) |

Compact is ui_guidelines §2's "data" density (Activity, Quarry's metadata list, the console). Spacious is for the
Pi on a TV or a touch panel.

### 2.2 Typography

**DECISION — the UI face is Inter (variable, `wght` + `opsz`), the monospace face is JetBrains Mono, and Noto Sans
is the fallback for every script Inter lacks.** All three are SIL OFL 1.1 [S35]. Inter is "designed for computer
screens" with a tall x-height. It has tabular numerals and a slashed zero [S35]. Since 4.0 it carries an optical-size
axis (`opsz`), with the Display designs folded in [S36]. That is the same idea as SF's and Segoe UI Variable's
optical sizes [S2] [S45], with a licence we can ship. It replaces DejaVu Sans as `system.display.font`'s default
(`video/text.rs`). DejaVu stays as a fallback.

**The ramp.** Una's body is 13 pt, after the Mac [S2]. Softy's is 14 pt, after Windows 11 [S45]. The roles are the
same; only the base step differs. Line heights are multiples of 2 pt, and of 4 pt from 20 pt up.

| Role | Una size/line | Softy size/line | Weight | Tracking | Use |
|---|---|---|---|---|---|
| `type.caption` | 11/14 | 12/16 | 400 | +0.5 % | dock tooltips, badges, timestamps |
| `type.footnote` | 12/16 | 12/16 | 400 | +0.25 % | secondary row text, section headers (600) |
| `type.body` | **13/18** | **14/20** | 400 | 0 | every label, menu item, list row, field |
| `type.body-strong` | 13/18 | 14/20 | 600 | 0 | window titles, app name in the bar, default button |
| `type.headline` | 15/20 | 16/22 | 600 | −0.25 % | toast title, dialog title (Una), group headers |
| `type.title3` | 17/22 | 18/24 | 600 | −0.5 % | Settings pane title (compact), dialog title (Softy) |
| `type.title2` | 20/26 | 20/28 | 600 | −0.75 % | Settings pane title |
| `type.title1` | 26/32 | 28/36 | 700 | −1 % | About, onboarding |
| `type.display` | 40/48 | 40/52 | 700 | −1.5 % | login clock, hero numbers |
| `type.mono` | 12/18 | 13/20 | 400 | 0 | console, code, Activity figures |

Rules: sentence case everywhere [S45]. Semibold, not bold, for emphasis below 20 pt. Never go below 11 pt
(macOS's floor is 10 [S1]; ours is one step safer). Figures that change in place (clock, meters, progress, Activity)
use `tnum`.

**What our raster needs** (`font_core::ui`, which `video/text.rs` fulfils):
1. **Variable-font instancing** on `wght` and `opsz`, with `opsz` set automatically from the pt size, as Segoe UI
   Variable does by default [S45].
2. **OpenType features** `tnum`, `zero`, `ss01`/`cv*` on request, and `kern`/`liga`/`calt` on by default.
3. **No hinting at scale ≥ 2**, grayscale AA (the existing Skia-style A8 coverage), fractional x origins (already
   quarter-pixel). Inter is drawn to favour high-density rasterisation over snapping stems at small px sizes [S35],
   which matches our panels.
4. **Per-role cached atlases** (body, body-strong, footnote, mono at the panel's scale), so a frame never shapes the
   chrome text twice.
5. **Fallback chain**: Inter → Noto Sans (script by script) → DejaVu Sans → the `font8x8` atlas (pre-heap only).

### 2.3 Colour

**DECISION — semantic tokens, named by job, generated in OKLCH.** The current `palette_core` (`unaos/libs/sys/palette_core/src/lib.rs`)
names *materials*: `BevelLight`, `BevelShadow`, `GlossHighlight`, `TitleActiveTop/Bottom` (a gradient). Those names
draw an Aqua-era bevelled window, and that is the X-windows vibe. The new table replaces them. The craft arc maps
old token to new token in one commit. All values were computed in OKLCH and checked against WCAG 2.x relative
luminance [S38] (§10 gives the method). Neutrals carry a trace of the accent hue (OKLCH chroma 0.006 at h 265), so
grey reads as *ours*, not stock.

| Token | Light | Dark | Job |
|---|---|---|---|
| `surface.window` | `#F5F7FB` | `#1D1E21` | window body, toolbars, title bar |
| `surface.sidebar` | `#EBEDF1` | `#252629` | sidebars (Settings, Quarry, Aether side tabs) |
| `surface.content` | `#FFFFFF` | `#16171A` | documents, web pages, list views, fields |
| `surface.raised` | `#FFFFFF` | `#292A2D` | grouped-list cards, segmented thumb |
| `surface.overlay` | `#FAFCFF` | `#2F3033` | menus, popovers, toasts, dialogs |
| `surface.sunken` | `#EEF0F4` | `#121417` | segmented track, omnibox at rest, slider track |
| `line.subtle` | `#DCDEE2` | `#343539` | separators, card borders |
| `line.strong` | `#BCBEC2` | `#4B4D50` | window outline (light), inactive control discs |
| `line.control` | `#84868A` | `#7E8084` | checkbox, radio, field and toggle-off borders: **≥ 3:1** [S39] |
| `text.primary` | `#191B1D` | `#F0F2F6` | body text: ≥ 14.7:1 (light), ≥ 11.7:1 (dark) on every surface |
| `text.secondary` | `#595B5E` | `#AFB1B5` | subtitles, shortcuts: ≥ 5.8:1 / ≥ 6.1:1 |
| `text.tertiary` | `#67696C` | `#96989C` | placeholders, timestamps: ≥ 4.5:1 on every surface |
| `text.disabled` | `#A3A5A8` | `#5C5E61` | disabled labels (exempt from 1.4.3 [S38]) |
| `scrim` | `#000000` @ 30 % | `#000000` @ 50 % | behind system-modal dialogs |
| `rim.dark` | — | `#FFFFFF` @ 8 % | 1-device-px inner rim on dark windows and overlays (edges survive a dark wallpaper) |

**The accent rule.** The accent is a *hue*. Its four uses are derived from it, the way libadwaita clamps its accent
in OKLab [S24]:

* `accent.fill`: the hue at the highest OKLCH lightness where **white text on it is ≥ 4.6:1** (gamut-mapped by
  reducing chroma). Used for the primary button, selected sidebar row, toggle-on, checkbox-on and slider fill.
* `accent.hover` / `accent.pressed`: `accent.fill` at L − 0.04 and L − 0.08.
* `accent.ink` (accent as text or icon on a surface): light L = min(L_fill, 0.50); dark L = 0.80. This is the
  libadwaita clamp, made ours by fixing it to a contrast target, not a constant.
* `focus.ring`: `accent.fill` in light (≥ 4.0:1 against `surface.window` and `surface.sidebar`) and `accent.ink` in
  dark (≥ 8.8:1).

The eight accents keep their Principia names (`prefs_core::appearance::ACCENTS`). **DECISION — the default accent
is renamed from `crispy` to `una`**: a periwinkle blue-violet that is neither Apple's blue nor Microsoft's. All
eight share chroma 0.16 (slate 0.035, teal gamut-limited to 0.09), and their fill L is solved per hue for the 4.6:1
white-label rule, so switching accent never changes perceived weight (the point of OKLCH [S50]):

| Accent | OKLCH h | `accent.fill` | white-on-fill | `.hover` | `.pressed` | `.ink` light (on window) | `.ink` dark (on window) |
|---|---|---|---|---|---|---|---|
| **una** (default) | 268 | `#4E6DD3` | 4.72 | `#4361C5` | `#3955B8` | `#3D5ABD` (5.73) | `#A2BCFF` (8.77) |
| teal | 195 | `#138282` | 4.62 | `#0A7575` | `#016869` | `#017272` (5.33) | `#10D8D8` (9.29) |
| moss | 140 | `#32851B` | 4.66 | `#247905` | `#1D6C00` | `#227702` (5.25) | `#83D670` (9.27) |
| amber | 70 | `#A26708` | 4.69 | `#925D0B` | `#845300` | `#895709` (5.67) | `#FEA92F` (8.58) |
| clay | 40 | `#C44F21` | 4.68 | `#B6420F` | `#A63801` | `#A83903` (5.98) | `#FDA485` (8.50) |
| rose | 5 | `#C4486D` | 4.67 | `#B63C62` | `#A82E57` | `#A82E57` (6.09) | `#FF9CB3` (8.36) |
| violet | 300 | `#8A5FC9` | 4.60 | `#7E53BC` | `#7347AF` | `#7347AF` (5.99) | `#C9ADFD` (8.54) |
| slate | 250 | `#657689` | 4.66 | `#5A6B7D` | `#4F5F71` | `#546577` (5.55) | `#ADC0D4` (8.84) |

**Status colours** are fixed, not accent-derived. They say the same thing in every theme. Each fill takes a white
label (≥ 4.5:1):

| Token | Fill (light and dark) | Ink, dark |
|---|---|---|
| `status.positive` | `#32851B` | `#89CC7B` |
| `status.warning` | `#A26708` | `#E8AA4E` |
| `status.critical` | `#BC4945` | `#FF958D` |
| `status.info` | `#1577C8` | `#73BDFF` |

**Window-control discs** (Una): **close `#F0584B`, minimise `#F4BF3F`, zoom `#47C86E`**. These are OKLCH (0.66 0.19 28),
(0.83 0.15 85) and (0.74 0.17 150), our own values. They replace `CTRL_CLOSE/MIN/ZOOM`, which are today macOS's exact
hexes (`palette_core` lib.rs, "DERIVED … *same color as mac*"). The red-amber-green *convention* is free to follow.
Copying Apple's exact values gains nothing and is the kind of detail §7 asks us to make our own.

**Contrast targets** (gates for every craft arc): text ≥ 4.5:1 below 18 pt and ≥ 3:1 from 18 pt [S1] [S38]. UI
component boundaries and state indicators ≥ 3:1 [S39]. The focus indicator changes ≥ 3:1 between focused and
unfocused, over a ≥ 2-device-px perimeter [S42]. Thresholds are never rounded (4.499 fails) [S38]. The "Increase
contrast" switch in Settings → Accessibility selects the contrast set below.

**The contrast set** (CONTRAST, rmbp-ledger B664). With `system.accessibility.increase_contrast` on, the theme resolves
the same 27 tokens through `palette_core::contrast` (`contrast::rows(dark, on)`). The surfaces, discs, status colours,
inks, syntax and accents do not change. The text ramp and the three lines move toward the ends of OKLCH L, at the same
neutral trace (chroma 0.006, h 265). Each one takes the highest L (Light) or the lowest L (Dark), in 0.001 steps, that
reaches its target against the **lowest-contrast of the six surfaces**. The targets: `text.primary` at L 0.13 / 0.985;
`text.secondary` ≥ 9:1; `text.tertiary` ≥ 7:1, the enhanced floor of WCAG 1.4.6 [S38], so every text token clears 7:1
on every surface; `text.disabled` ≥ 4.5:1 (no longer exempt); `line.subtle` ≥ 3:1 [S39]; `line.strong` ≥ 4.5:1;
`line.control` ≥ 6:1. The method is §10's. The hex values and their ratios are in palette_core's `contrast` module, and
a const block there recomputes every ratio from the hex at build time, so a value below its floor fails the build.
Softy's Light table gets its own pairs, solved the same way against Softy's five grounds. Softy's Dark uses the
contrast set's Dark row. A toggle bumps the theme epoch, the same as an accent change, so chrome, menus and the
generated wallpaper redraw. `tests contrast` samples six text-on-surface pairs on each look.

**Dark mode** is not an inversion [S5]. Elevation in dark is *lighter* surfaces (window `#1D1E21` < raised `#292A2D`
< overlay `#2F3033`), because shadows are hard to see there [S49]. Pure `#000000` is reserved for the console and media
grounds (Pulse, Facet, the Player), where ui_guidelines §4's OLED argument applies.

### 2.4 Depth

**Elevation levels.** Four, after Atlassian's vocabulary [S49], with Fluent's two-layer recipe [S19] [S20] and
libadwaita's outline [S23]:

| Level | Used by | Light | Dark |
|---|---|---|---|
| `elev.0` flat | content, sidebars, cards | none (cards: 1 px `line.subtle`) | none (cards: `surface.raised` + 1 px `line.subtle`) |
| `elev.1` raised | segmented thumb, toggle knob, slider knob | 0 1 2 pt @ 14 % + 0 0 1 pt @ 10 % | 0 1 2 pt @ 40 % |
| `elev.2` overlay | menus, popovers, tooltips, toasts, omnibox suggestions | 0 0 2 pt @ 12 % + 0 8 24 pt @ 18 % | 0 0 2 pt @ 30 % + 0 8 24 pt @ 45 % + `rim.dark` |
| `elev.3` window (focused) | every window | 0 0 2 pt @ 12 % + 0 16 40 pt @ 26 % + outline `line.strong` @ 60 % | 0 0 2 pt @ 40 % + 0 16 40 pt @ 55 % + `rim.dark` |
| `elev.3i` window (unfocused) | background windows | 0 0 2 pt @ 8 % + 0 8 20 pt @ 14 % | 0 0 2 pt @ 30 % + 0 8 20 pt @ 35 % |
| `elev.4` modal | system dialogs, login form | `elev.3` + `scrim` behind | `elev.3` + `scrim` behind |

Shadow notation is `x y blur` in pt, black at the stated alpha. Focus changes shadow *size*, as macOS and libadwaita
do (libadwaita's backdrop transition is 200 ms [S22] [S23]). That gives a focused window a clear lift with no colour
change in its title.

**DECISION — shadows are pre-rendered, never computed per frame.** A window's shadow is a 9-slice (four corner tiles
plus four edge ramps) rendered once per (radius, level, scale, theme) into a cached alpha table and blitted with the
window. The cost is one alpha-blend pass over the shadow's band, about 40 pt × the perimeter: about 0.5 M pixels (about
9 % of the panel) for a 900×600 pt window at scale 2, and only where the window is recomposited. No blur is ever
computed at run time. Today there is **no window shadow at all** in `wm.rs`.
This is the second most visible change in this file.

**DECISION — translucency: the VEIL, sampled once, never live.** A live backdrop blur on a 2880×1800 panel
composited by a CPU means re-blurring up to 5.2 M pixels behind every translucent surface on every frame. Even
dual-Kawase, the cheapest good blur, is 8 samples per pixel per pass [S71] (*via search*). picom's own documentation
calls background blur "bad in performance" [S71]. Microsoft built Mica for the same reason: it blurs the wallpaper
*once* instead of re-sampling what lies beneath [S47]. **So UnaOS's material, the VEIL, is the wallpaper blurred once**
(at wallpaper-set time and on a resolution change), downsampled 1/8, stored, and sampled by screen position with a
tint. The menu bar, dock, Softy's taskbar, Settings' sidebar and the login screen draw it. The cost per frame is
the same as a solid fill. The tint is `surface.window` at 72 % (light) or `surface.window` at 78 % (dark). Under
**Reduce transparency** it is `surface.window` at 100 %. Windows themselves are opaque. Apple keeps its glass to the
"functional layer" and never the content layer [S3], and so do we. *Live* blur is not ruled out forever. If the
Kepler copy engine (`video/blitter.rs`, KBLIT) ever does it for free, it may come back for menus only.

### 2.5 Corner radii

Radii follow object class [S44] and are concentric: an inner radius is the outer radius minus the inset, which
libadwaita writes as code [S22]. Maximized, snapped and full-screen windows are square [S44] [S23].

| Token | Una | Softy | Objects |
|---|---|---|---|
| `radius.window` | **12** | **8** | windows (all four corners), the login card |
| `radius.overlay` | 10 | 8 | menus, popovers, omnibox suggestions |
| `radius.dialog` | 14 | 8 | alerts and dialogs |
| `radius.card` | 10 | 6 | grouped-list cards, notification cards |
| `radius.control` | 6 | 4 | buttons, fields, segmented, popup buttons, sidebar selection, tabs |
| `radius.item` | 5 | 4 | menu-item highlight (= overlay − inset 5) |
| `radius.small` | 4 | 2 | checkbox, tooltips, badges, keycaps |
| `radius.full` | 9999 | 9999 | toggles, sliders, the omnibox (Una), avatars, dock running dots |
| `radius.dock` | 18 | — | the dock's container (icon plate 12 + inset 6) |

### 2.6 Motion

Motion explains a change of place or state, and nothing else [S4]. It never holds input: a click during an animation
acts at once, and the animation jumps to its end [S4]. All curves are evaluated from a 64-entry Q16 lookup table
(integer only, like the rest of the compositor).

| Token | ms | Easing | Use |
|---|---|---|---|
| `motion.instant` | 0 | — | text, caret, selection |
| `motion.fast` | **100** | `ease.standard` | hover and press feedback, focus ring appear |
| `motion.base` | **160** | `ease.standard` | toggle, checkbox, segmented thumb, tab switch |
| `motion.enter` | **240** | `ease.decelerate` | menus, popovers, toasts, sheets, side-tab strip expand |
| `motion.exit` | **160** | `ease.accelerate` | the same surfaces leaving (exits ≈ 2/3 of entrances) |
| `motion.window` | **280** | `ease.emphasized` | window open (scale 0.96 → 1 + fade), minimise to dock, zoom |
| `motion.space` | **400** | `ease.emphasized` | workspace switch, look switch (Una ↔ Softy) |

| Easing | cubic-bezier | Source |
|---|---|---|
| `ease.standard` | (0.2, 0, 0, 1) | M3 standard [S10] |
| `ease.decelerate` | (0, 0, 0, 1) | M3 standard-decelerate, Fluent decelerate-mid [S10] [S16] |
| `ease.accelerate` | (0.3, 0, 1, 1) | M3 standard-accelerate [S10] |
| `ease.emphasized` | (0.05, 0.7, 0.1, 1) | M3 emphasized-decelerate [S10] |

The ladder sits between Kirigami's 50/100/200/400 [S28], Fluent's 50–500 [S15] and M3's 50–1000 [S10]. We stop at 400:
desktop motion longer than that reads as slow.

**No-motion fallback** (Settings → Accessibility → Reduce motion, on by default where the panel's frame time misses
16 ms, as measured by `vperf`): every *spatial* animation (slide, scale, minimise genie) becomes a **120 ms crossfade**
or a cut. Toggles and checkboxes change state in one frame. WCAG asks that interaction-triggered motion can be turned
off, because vestibular reactions are severe [S43]. web.dev recommends defaulting to less movement [S80].

### 2.7 States

A state is an **overlay layer** of the content colour, not a new colour (M3's state layers [S12], Radix's step 10 for
hover [S37]):

| State | Neutral control (overlay of `text.primary`) | Accent control | Text and icon |
|---|---|---|---|
| rest | — | `accent.fill` | `text.primary` |
| hover | + 6 % (light) / + 8 % (dark) | `accent.hover` | unchanged |
| pressed | + 12 % / + 14 % | `accent.pressed` | unchanged |
| selected (focused window) | `accent.fill`, label white | — | white |
| selected (unfocused window) | `line.subtle` fill, label `text.primary` | — | `text.primary` |
| focused (keyboard) | `focus.ring` 2 pt outside, 2 pt offset, `radius` + 2 | same | unchanged |
| disabled | whole control at 40 % opacity, no hover, no focus | same | `text.disabled` |
| dragging | + 16 % and `elev.2` | `elev.2` | unchanged |
| drop target | 2 pt inset `accent.fill` outline | — | — |

The focus ring shows only after keyboard navigation ("focus-visible"). A click does not draw it. The ring is ≥ 2
device px and ≥ 3:1 [S42], so every control is reachable and visible without a pointer: the Steam Deck case [S74],
the Pi on a TV.

### 2.8 Icons

* **UI icon grid**: **20 pt** (sidebars, toolbars, menus where present) with a 16 pt live area and 2 pt padding, and
  **16 pt** (dense: menu-bar extras, tab favicons, list rows) with a 12 pt live area. This is Material's 24/20 and
  20/16 structure scaled to a desktop [S75]. **24 pt** is for the Softy taskbar and touch.
* **Style**: outline, **1.5 pt stroke** at 20 pt and 1.25 pt at 16 pt, round caps and joins, interior corners square,
  exterior corners 1 pt radius. A **filled variant for the selected or active state** (selected sidebar row, active
  tab-strip button, toggled toolbar button). Outline icons are the field's current direction (Chrome Refresh [S53]).
  At scale 2, a 1.5 pt stroke is 3 px: crisp, on whole pixels.
* **App icons**: 16 / 32 / 64 / 128 / 256 / 512 pt. **DECISION — the plate is ours: the FACET.** It is a rounded square
  (radius 22 % of side) with its **top-right corner cut by a chamfer of 18 % of side**, one bevel facet. It echoes the
  crystal and is not Apple's squircle or Microsoft's flat tile. The glyph sits in the central 64 % of the plate.
* Icons are drawn by us, as code or our own vector files. **No icon from any other system is copied** (§7).

### 2.9 Hit targets and focus

| Context | Minimum | Default | Source |
|---|---|---|---|
| Pointer (both looks) | 24×24 pt (WCAG 2.5.8) | **28 pt tall** (Una) / **32 pt tall** (Softy) | [S1] [S40] [S45] |
| Touch or TV (Spacious density) | 44×44 pt (WCAG 2.5.5) | 44 pt | [S41] |
| Spacing around targets below 24 pt | 24 pt circle clear of neighbours | — | [S40] |
| Padding between bezelled controls | 12 pt | 12 pt | [S1] |

The window-control discs are 12 pt to the eye but answer **20 pt hit boxes** (the macOS minimum control size [S1]).
The resize edge is an invisible 6 pt band outside the window plus the 12 pt corner squares.

**AMENDMENT 2026-10-13 — A WINDOW HAS A VISIBLE EDGE** (WINEDGE, rmbp-ledger B659). Peter, flight 35, on the
frameless window with only the outside shadow (DESIGNAPPLY B639): *"new prefs panel looking very nice. new borders not
so much."* The shadow alone does not read as a window's edge. So every window carries a **1-device-px hairline round
the whole box at every scale** — `line.strong` on Light, `line.subtle` on Dark (§2.3's roles; `wm::desktop::edge_ink`,
never a literal) — opaque, INSIDE the box (its outermost column each side, its last row, and the rounded head's
stroke): the content is painted first and the edge over it, so the content cannot hide it. The shadow
(§2.4) still starts on the first pixel outside the box, and the resize band above stays outside it, invisible. This
replaces §3.1's "`line.strong` @ 60 % (light); `rim.dark` inside (dark)" outline for windows (overlays keep
`rim.dark`). Peter's eyes on the next staged screenshot decide whether this edge is the one he asked for.

---

## 3. THE COMPONENT CATALOGUE

Dimensions in pt, Standard density. "U / S" gives a value for Una / Softy when they differ. Every colour is a §2.3
token. Every state follows §2.7, and every motion follows §2.6.

### 3.1 Window chrome

| Part | Una | Softy |
|---|---|---|
| Title bar | 32 tall. Unified title + toolbar 52 | 32 tall [S46]. Toolbar below, 44 |
| Body | `surface.window`, radius 12, `elev.3` / `elev.3i` | `surface.window`, radius 8, `elev.3` / `elev.3i` |
| Outline | the EDGE (§2.9, amended 2026-10-13): 1 device px `line.strong` (light) / `line.subtle` (dark), opaque, round the whole box. Was `line.strong` @ 60 % / `rim.dark` | Softy's own 1 px border (`softy::chrome`) |
| Title text | `type.body-strong`, centred, `text.primary` (focused) / `text.tertiary` (unfocused), ellipsis | `type.body`, left at 12 after a 16 app icon at 12 from the edge (Windows' 16 px icon-to-title [S46]) |
| Controls | Three **discs** top-left, Ø 12, 8 apart edge to edge, first at 12 from the edge, centred in the bar; 20 hit boxes. Fills close/min/zoom (§2.3). Unfocused: all three `line.strong`. Hover on the group shows glyphs ×, −, and two diagonal arrows, at 1.25 stroke, black @ 50 % | Three **caption buttons** top-right, 46×32 each (*via search* [S46]): minimise, maximise/restore, close. Glyphs 10, 1 pt stroke. Hover + 6 %. Close hover `status.critical` fill, white glyph. The maximise button opens **snap layouts** on hover (Windows idiom [S80b]) |
| Maximized / snapped | square corners, no shadow (`winsnap.rs`) | same |
| Double-click title | zoom (fill the panel minus the dock) | maximise |
| Removed | the FRAME (5) and BEVEL (1) of `ui::base`, the gloss gradient, KNURL on the discs, CERAMIC on the head | same |

### 3.2 Menu bar (both looks: R21, R25)

* 28 tall, full width, drawn in the **VEIL** (§2.4). `type.body`, `text.primary`.
* Left: the **crystal**, 28×28 hit and a 16 glyph, flush left (R25). Then the app name in `type.body-strong`. Then
  menu titles, each padded 10 horizontally (a 28 tall hit). One-word titles [S7].
* Menus drop with `motion.enter`. Items keep their order and are disabled, never hidden [S7].
* Right (Una): status items, 16 icons in 28 hit boxes, 4 apart, then the clock in `type.body` with `tnum`.
  In **Softy** the right end is empty: status and clock live in the taskbar tray (§6). The bar is 24 tall in Softy.

### 3.3 Dock (Una) and taskbar (Softy)

| | Dock (Una) | Taskbar (Softy) |
|---|---|---|
| Placement | bottom centre, **floating 8 above the edge**; goes flush with the edge when a window is maximized (Plasma 6 [S69]) | full width at the bottom, 48 tall |
| Container | VEIL, radius 18, padding 6, `elev.2`, outline `line.subtle` | VEIL, flat, top hairline `line.subtle` |
| Items | icon plates 48 (FACET), cells 56, gap 4. Settings → Dock: 32–80 | crystal button + pinned + running, 40×40 hits in 44 cells, icons 24. **Centred** by default with a left option, as Windows 11 does [S80b] |
| Running | 4 dot, 3 below the plate, `text.primary` @ 60 % | 3×16 pill under the icon. Active window 3×24 `accent.fill` |
| Label | tooltip above on hover (`type.caption`, `radius.small`, `elev.2`). No permanent captions | tooltip above. Right end: tray icons 16, the clock as two lines (time / date) in `type.footnote` with `tnum` |
| Every window has a way back | yes (dock.rs ruling, R49) | yes |

### 3.4 Settings (R118: tabs on the left)

* Default window 760×520. **Sidebar 220 wide**, `surface.sidebar` (VEIL in Una), padding 8.
* Sidebar top: a search field, 28 tall, `radius.control`.
* Sidebar rows 28 tall, inset 8 from both edges, `radius.control`. A 16 icon at 8 from the left, then the label in
  `type.body` at 8 after it. **Selected**: `accent.fill` with white label and icon (focused window) or `line.subtle`
  fill (unfocused). Group headers are `type.footnote` 600 `text.secondary`, 16 above and 4 below.
* Content: `surface.window`. Pane title in `type.title2`, 20 from the top and 24 from the left. 24 from the title to
  the first group.
* **Grouped list** (the Settings idiom on both Mac and Windows): cards in `surface.raised` with `radius.card`
  (10 / 6) and a 1 px `line.subtle` border. Rows are **min 40 tall** (52 with a subtitle), inset 12. The label
  (`type.body`) is on the left. The control is on the right, at 12 from the edge. Separators are 1 px `line.subtle`,
  inset 12 from the left. Section headers are `type.footnote` 600 `text.secondary`, 20 above the card and 6 below.
  Footnotes below a card are `type.footnote` `text.secondary`, 6 below.
* One primary button per pane at most [S79].

### 3.5 Controls

| Control | Una | Softy | States and motion |
|---|---|---|---|
| **Button** | 28 tall, min-width 72, padding 12, `radius.control` 6, `type.body`. Primary: `accent.fill` + white. Secondary: `surface.raised` + 1 px `line.control` @ 50 % (light) / white 8 % fill (dark), `elev.1` | 32 tall, min-width 96, padding 12, radius 4, `type.body` (14). Same two styles | hover / pressed overlays, focus ring, disabled 40 %, `motion.fast` |
| **Toggle (switch)** | track 36×20, `radius.full`. Off: `line.control` @ 35 % fill. On: `accent.fill`. Knob 16 white, inset 2, `elev.1`. Only in list rows, never in a toolbar [S8] | track 40×20. Off: 1 px `line.control` outline, knob 12 `text.secondary`. On: `accent.fill`, knob 14 white | knob travels in `motion.base` / `ease.standard`; Reduce motion: one frame |
| **Checkbox** | 16 box, `radius.small` 4, 1 px `line.control`. On: `accent.fill` with a white check at 2 stroke. Mixed: a 8×2 dash | 20 box, radius 4, same | `motion.base` check draw-on (Reduce motion: none) |
| **Radio** | Ø 16, 1 px `line.control`. On: `accent.fill` with a white Ø 6 dot | Ø 20. On: `accent.fill` ring, white Ø 8 dot | same |
| **Slider** | track 4, `radius.full`, `surface.sunken` + 1 px `line.subtle`. Fill `accent.fill`. Knob Ø 20 white, `elev.1`. Hit 28 tall | track 4. Knob Ø 20 white with an inner Ø 12 `accent.fill` that grows to 14 on hover | knob follows the pointer with no easing; keyboard steps in `motion.fast` |
| **Text field** | 28 tall, `radius.control`, `surface.content`, 1 px `line.control` @ 60 %, padding 8, `type.body`. Caret 1 pt `accent.ink`. Placeholder `text.tertiary`. Focus: `focus.ring` | 32 tall, radius 4, same fill. Focus: a **2 bottom stroke `accent.fill`** (the Windows idiom) | caret blink 1000 ms cycle, steady under Reduce motion |
| **Segmented** | 28 tall, track `surface.sunken`, radius 7, inset 2. Thumb `surface.raised` radius 5, `elev.1`. Labels `type.body` | 32 tall, same structure, radius 4 / 3 | thumb slides `motion.base` (Reduce motion: crossfade) |
| **Popup / dropdown** | as the button, with a chevron 10 at 8 from the right | as the button, with a chevron 12 | opens a menu (§3.7) |
| **Scrollbar** | overlay 6 wide (10 on hover), `radius.full`, `text.primary` @ 35 %. Shows while scrolling, fades after 800 ms | 4 rest, 8 hover, with arrows on hover | fade `motion.exit` |
| **Tooltip** | `type.caption`, padding 4 / 8, `radius.small`, `surface.overlay`, `elev.2`; after 600 ms hover | same | fade `motion.fast` |

### 3.6 Toasts and notifications

* **Una**: top-right, 12 below the bar and 12 from the edge. **Softy**: bottom-right, 12 above the taskbar.
* Card 360 wide, `radius.card` + 4 (14) / 8, `surface.overlay`, `elev.2`, padding 12. A 32 FACET icon, then
  `type.headline` title and `type.body` `text.secondary` body (max 3 lines). Action buttons are secondary buttons,
  right-aligned.
* At most 3 stacked, 8 apart. Auto-dismiss after 5 s, paused while hovered. Enter is a slide in from the edge plus a
  fade (`motion.enter`). Exit is `motion.exit`. Under Reduce motion, both are a fade.

### 3.7 Menus (menu-bar menus, context menus, the crystal)

| | Una | Softy |
|---|---|---|
| Container | `surface.overlay`, `radius.overlay` 10, padding 5, `elev.2`, min-width 180 | radius 8, padding 4, min-width 200 |
| Item | 24 tall, inset 5, `radius.item` 5, `type.body`. Shortcut right-aligned `text.secondary` | 32 tall, radius 4, `type.body` (14). A leading 16 icon column is allowed |
| Hover / keyboard | `accent.fill`, white label and shortcut (Mac idiom) | + 6 % neutral overlay (Windows idiom) |
| Separator | 1 px `line.subtle`, 5 above and below, inset 10 | 1 px, 4 above and below, full width |
| Submenu | chevron 8 at the right. Opens after 200 ms or on Right | same |
| Motion | `motion.enter` fade + 4 pt slide from the anchor. Closing is instant | same |

### 3.8 Dialogs

* **Una (alert)**: 320 wide, centred over the parent window (window-modal, no scrim) or the panel (system-modal,
  `scrim`). `radius.dialog` 14, `surface.overlay`, `elev.3`, padding 20. A 48 icon at top, `type.headline` title,
  `type.body` `text.secondary` message, centred. Buttons stacked full-width (2 or more) or side by side. The default
  button is `accent.fill`.
* **Softy (content dialog)**: 400–540 wide, radius 8, `type.title3` title left, body left, padding 24. A **footer band**
  80 tall in `surface.window` with buttons right-aligned at equal width; the primary button is `accent.fill`.
* Both: Return = default, Escape = cancel, focus starts on the default button, one primary action [S79].

### 3.9 The login screen

* Background: the **VEIL** of the wallpaper (blurred, darkened 20 %). It is shown from the first frame, so nothing
  flashes (R93: nothing of the desktop is drawn before login).
* **Una**: the date (`type.headline`) and time (`type.display` 40, `tnum`) centred in the top third. The user tile
  is centred at 55 %: a 96 avatar (`radius.full`), the name in `type.title3` white, then a password field 240×32
  (`radius.full`, white @ 20 % fill, white text, an arrow button inside) and a 12 hint line. Power row: three 44
  circular buttons at the bottom centre, 24 apart, with `type.caption` labels.
* **Softy**: a **lock screen** first, with the time (`type.display` ×1.5) and date at bottom-left, 48 from the edges.
  Any key or click lifts it (`motion.window`, up and fade) to a centred tile with the same parts, a 32 tall field at
  radius 4, and power at the bottom-right.
* Multiple users: tiles in a row, 120 apart, with the picked tile in front. Arrow keys move between them, with a
  visible focus ring.

### 3.10 Browser chrome (Aether, ring 3)

| Part | Spec |
|---|---|
| **Top tab strip** | Tabs live in the title bar (Una: 40 tall, after the discs, with 80 left clearance; Softy: 40 tall, before the caption buttons). Tab 32 tall, top corners `radius.control` + 2 (8), width 240 down to 72, then pinned 40. Favicon 16 at 10 from the left; title `type.body` with a 12 fade-out instead of an ellipsis; close button 16 in a 20 hit, shown on hover and on the active tab. **Active**: `surface.content`, joined to the toolbar, `elev.1`. **Inactive**: transparent, hover a 6 % pill (Chrome Refresh [S53]), 1 px `line.subtle` dividers 16 tall that disappear beside hover or active tabs. New-tab button 28. Groups: a coloured 4 underline and a named chip in `type.footnote` 600 (stacks, after Vivaldi [S58]). Tabs never merge with the omnibox (the Safari 15 lesson [S55]). |
| **Side tabs (R118)** | A left column **240 wide** (resizable 180–400) in `surface.sidebar`, collapsible to a **48 favicon rail** (Zen compact mode [S56]; Chrome's native vertical tabs collapse too [S54]). Top: a filter field 28 tall. Pinned tabs as a grid of 44 tiles, 4 per row. Rows **32 tall**, inset 6, `radius.control`: favicon 16, title `type.body` single line (the full title shown in a tooltip), close button on hover, audio glyph when playing. **Selected**: `accent.fill` @ 16 % with `text.primary` (a quieter selection than Settings', because many tabs are scanned). Groups: a 24 header with a colour dot, name, count and disclosure chevron, collapsible. Built for dozens: smooth scroll, a sticky active tab, drag to reorder and into groups, and "close tabs below". The title bar stays (discs and window title), and the toolbar sits above the content only. |
| **Toolbar** | 44 tall, `surface.window`. Back, forward and reload are 28 icon buttons with 20 icons. The omnibox is flexible. Then extensions or downloads (28 buttons) and the app menu (Softy). 8 between groups. |
| **Omnibox** | 32 tall. **Una** `radius.full` / **Softy** radius 8. Rest: `surface.sunken`. Focused: `surface.content` + `focus.ring`. A site-controls chip at the left (24 tall, `radius.full`) replaces the lock, as in Chrome Refresh [S53]. URL in `type.body`: the host in `text.primary`, the rest in `text.secondary`, the scheme hidden at rest. Suggestions: `surface.overlay`, `radius.overlay`, `elev.2`, rows 36 (a 16 icon, then the title and the URL in `text.secondary`). The first row is selected and Return takes it. The URL-or-search decision is `handlers/aether/src/chrome/omnibox.rs`'s and stays there. |
| **Find bar, downloads** | Find: a 36 strip under the toolbar, field 28, "n of m" in `type.footnote` with `tnum`, chevrons and a close button. Downloads: a toolbar button with a progress ring; the popover lists rows 44 tall. |

### 3.11 Window switcher, launcher, Lumen

* **App switcher** (Cmd/Alt-Tab): a centred `surface.overlay` VEIL card, `radius.dialog`, with icons 64 in 88 cells.
  The selection is a `radius.card` `line.subtle` @ 60 % fill. The app name is below in `type.footnote`.
* **Launcher** (Cmd-Space / Softy: the crystal key): a 640 wide field, 52 tall, `type.title3`, `radius.dialog`, `elev.2`
  at 22 % from the top. Rows 40 tall, with chords shown as keycaps (`radius.small`, `surface.sunken`, `type.caption`),
  after Raycast [S65].
* **Lumen**: a sidebar 240 of conversations (the §3.4 row spec). The message column has a max line length of 72
  characters. Bubbles are not used: messages are aligned blocks with names in `type.body-strong`, after Linear's
  noise reduction [S51].

---

## 4. THE SCREENSHOT

The desktop an UnaOS screenshot shows by default, and how it is arranged to make people want it.

**What the field says.** r/UsabilityPorn requires at least one window open and rejects setups too "riced-out" for
long-term use. Screenshots are judged on a palette that agrees with the wallpaper [S81]. Apple shows its glass only
over "visually rich backgrounds" [S3]. A good screenshot shows a desktop you would *use*, at a density you believe.

**The wallpaper — DECISION: "Una Dawn", procedural and ours.** It is generated at boot at the panel's native resolution
by our own code (`video/wallpaper.rs` already takes a panel-sized image; `video/paper.rs` already does procedural
Q16 textures). No photograph or asset of anyone else's. It is a smooth OKLCH field from **deep indigo** `#222153` at
the top through **violet** `#543A8B` to a **warm dawn glow** `#EBA66D` low on the horizon left of centre. Over it lie
three or four very large, very soft **crystal facets**: translucent planes at 3–6 % lightness difference with
1-device-px light edges at 10 %, the brand crystal's geometry at landscape scale. A dark variant ("Una Night") keeps
`#0C0F27` at the top and drops the glow to a violet ember. It is the existing `DESKTOP_BG` indigo, grown into a scene.
It is rich enough for the VEIL to show colour, and calm enough that window edges and white text read on it.

**The arrangement (2880×1800 at scale 2 = 1440×900 pt):**

1. **Aether** front and centre-left, about 900×600: a real, well-designed web page (the UnaOS site or a long-form
   article), **side tabs open** with 8–12 real tabs in two named groups. This is the browser people will try.
2. **Settings** behind it to the right, about 760×520, overlapping by about 15 %: the **Appearance** pane showing
   the eight accent swatches, the Light/Dark/Auto segmented control and the **Una ↔ Softy** look switch. That one
   window tells the story of "one OS, your comfort".
3. **Lumen**, small (about 380×520), at the right edge and partly behind Settings: a short real conversation.
4. The **dock** floating at the bottom with 7–9 FACET icons, three with running dots. The **menu bar** shows the
   crystal, "Aether" in semibold and a clean status area.
5. **Whitespace**: 30–40 % of the wallpaper visible, upper-left and lower-right free, the dawn glow uncovered. No
   window touches another's title bar (ui_guidelines §8). Shadows lift each window clearly off the scene.
6. **Pairs**: every release ships the *same* arrangement in **Light/Una** and **Dark/Una**, plus one **Softy** shot
   (the same three windows, the taskbar, caption buttons) captioned "Coming from Windows? Switch to Softy."
7. **Density**: Standard. Text at body 13 must be legible at 50 % zoom on a phone, which scale 2.0 guarantees and
   2.5 overshoots.

`prtscr.rs` captures it. A `UNAOS_SHOTSTAGE` demo knob (owed, not in this arc) would stage exactly this arrangement
for release screenshots.

---

## 5. THE UNA LOOK — at home for people from the Mac

**What gives the comfort**:

| Area | Una does | Why it comforts |
|---|---|---|
| Layout | Global menu bar at the top with the crystal top-left (R21, R25). Dock floating bottom centre. Discs top-left | the Mac's muscle memory: menus up, apps down, close top-left |
| Chords | **Cmd** (the ⌘ / Super key) chords: Cmd-Q, Cmd-W, Cmd-Tab, Cmd-Space, Cmd-, for settings, Cmd-H. This is `theme::CRISPY_BINDINGS` (`video/theme.rs:562`) | every shortcut where the hand expects it |
| Files | Quarry is always running, like the Finder (R50) | the same "home base" |
| Type and density | Inter at body 13, 28 pt controls, 12 pt window radius | the Mac's scale and softness |
| Controls | the accent-filled selection in menus and sidebars, the toggle in list rows only [S8], the grouped-list Settings | identical idioms, our drawing |
| Behaviour | menu items disabled not hidden [S7], standard menus in standard order [S7], Return/Escape in dialogs | predictable |

**What we deliberately do differently**:
* The **crystal**, not an apple. The **FACET** plate, not a squircle. Our **disc hues**, our **accent** (una), our
  **wallpaper**, our **face** (Inter), our icons. Nothing a court would call copied (§7).
* **No live glass.** The VEIL is the honest material for a CPU compositor (§2.4).
* **Snap layouts in Una too**: window snapping (`winsnap.rs`, Cmd-Alt-arrows and panel-edge drags) is something Mac
  users install a utility to get.
* **Density modes** (Compact/Standard/Spacious), which the Mac does not offer.

## 6. THE SOFTY LOOK — at home for people from Windows

**What gives the comfort**:

| Area | Softy does | Why it comforts |
|---|---|---|
| Layout | **Taskbar** at the bottom: crystal button, pinned and running apps, tray and a two-line clock at the right, **centred** by default with a left option [S80b] | Windows 11's shape |
| Windows | **Caption buttons top-right**, 46×32. Title left with the app icon. Maximise hover offers **snap layouts**. Square when maximized [S44] | the hand goes top-right to close |
| Chords | **Ctrl** chords: Ctrl-C/V/X/Z/S/W, Alt-F4, Alt-Tab, Win-arrows to snap, Win-E for Quarry, Win-D for desktop. This is `theme::PC_BINDINGS` (`video/theme.rs:662`) | the PC keyboard |
| Type and density | Inter at body 14/20, 32 pt controls, 8 pt windows, 4 pt controls [S44] [S45] | Windows 11's scale |
| Controls | outline-off toggles, underline-focus fields, neutral menu hover, footer-band dialogs, the lock screen before login | Windows idioms, our drawing |
| Files | Quarry with a command bar and a navigation pane, like Explorer | the same mental model |

**What we deliberately do differently**:
* **Menus stay in the top bar** (R21 binds every look). Softy's bar is a slim 24 pt strip with the crystal and the
  focused app's menus; status moves to the tray. Windows apps already hide their menus behind command bars, so
  people from Windows meet a strip they can ignore, with everything findable.
* No Start *wordmark* or Windows logo: the taskbar button is the **crystal**. The word "Start" is not used.
* Our VEIL, not Mica or Acrylic. Inter, not Segoe. Our icons, not Fluent's.

**The switch.** Settings → Appearance → Look: **Una · Softy**, live, with `motion.space` (Reduce motion: a cut).
The login screen offers it at first boot ("Coming from a Mac / from Windows?"), the way Zorin and Deepin let a
person choose [S72] [S73]. The choice is Principia's (a preference key, never a dotfile, per HEAD rule 11). Both
looks read one token table. Only the **look table** differs: radii, body size, control heights, chrome placement,
menu hover style, binding table.

---

## 7. THE LEGAL LINE

*This is a design team's reading of the public record, written so every craft arc can check itself. It is not legal
advice. Where a question is close, ask before shipping.*

### 7.1 Free to follow

* **Functional layout and conventions**: a menu bar at the top, a dock or taskbar, close buttons at a corner,
  windows that overlap, a sidebar of settings, tabs, an address bar. In *Apple v. Microsoft* the Ninth Circuit filtered
  out every similarity that came from the licence or from "common ideas or their logical extensions". What was left
  could only be protected against "virtual identity" [L1].
* **Menu structure and command words**: Cut, Copy, Paste, File, Edit, View, and the order of a command hierarchy. *Lotus
  v. Borland* held a menu command hierarchy to be an uncopyrightable "method of operation" (17 U.S.C. § 102(b)), like
  the buttons on a VCR [L2].
* **Keyboard shortcuts** and their meaning: they are part of the method of operation in the *Lotus* sense [L2]. *Google
  v. Oracle* treated reimplementing an interface so that people "put their accrued talents to work" as fair use [L5].
  A Mac or Windows user's chords working on UnaOS is that talent.
* **Measurements, spacing, radii, durations, contrast targets, colours in general**: facts and functional choices.
  The red-amber-green control *convention* is functional signalling.
* **Typeface designs** as shapes: US law does not protect "typeface as typeface" (37 CFR 202.1(e)) [L9]. **Font
  software** is protected, though. We ship OFL fonts (Inter, JetBrains Mono, Noto, DejaVu) and never someone else's
  font files [L9].

### 7.2 Not free

* **Copied icons, images, wallpapers, sounds, cursors, fonts**: artwork and font software are copyrighted. Even
  *Apple v. Microsoft* left "a handful" of items standing, including the trash-can icon design (*via search*) [L1].
  SF Pro's licence forbids use outside Apple platforms and forbids embedding [L12]. Segoe Fluent Icons may not ship to
  other platforms [L13].
* **Logos, trademarks and product names**: the apple, the Windows flag, "Finder", "Start", "Mission Control",
  "Aqua", "Liquid Glass", "Mica", "Fluent". *Microsoft v. Lindows* ended with Microsoft paying $20 M and Lindows renaming itself Linspire and handing over its
  domains, after European courts had ruled against the name [L11]. A name close to a rival's mark costs more than it
  is worth. Ours are ours: Crystal, Quarry, Launcher, Aether, Lumen, Facet, Stria, Una, Softy.
* **Design patents on a GUI**: *Apple v. Samsung* upheld design patents D'677, D'087 and D'305, the last covering the
  home-screen GUI. That is roughly $548 M on appeal [L3]. The Supreme Court then let damages be limited to the
  infringing *component* [L4]. Patents on specific screens, icon grids and animations are real and enforceable.
* **Distinctive trade dress reproduced as a whole**: in the same case the iPhone's trade dress was reversed as
  *functional* [L3]. *Tetris v. Xio*, though, found infringement because Xio copied the game's whole "look and feel",
  not just its rules, and the colours and styles were "arbitrary flourishes" [L6]. Product-design trade dress needs
  secondary meaning (*Wal-Mart v. Samara* [L7]) and cannot cover functional features (*TrafFix* [L8]). The risk is the
  sum of copied *non-functional* details: exact hues, exact gloss, exact icon plate, exact animation.
* **History**: in 2000–2001 Apple had Aqua-styled themes removed from themes.org and Skinz.org and pressed Stardock
  over DesktopX skins, on the ground that they copied its artwork and code [L10]. UnaOS draws everything from its own
  code, so that ground is absent. But the lesson is that *look-alike as a whole* draws letters.

### 7.3 The checklist every craft arc answers in its commit body

1. **Pixels**: did any image, icon, glyph, cursor, wallpaper or sound come from another product? It must be **no**:
   draw it from code or our own vector.
2. **Fonts**: is every font file OFL or similarly licensed for embedding? It must be **yes** (Inter, JetBrains Mono,
   Noto, DejaVu).
3. **Names**: does any visible string use another company's product or feature name? It must be **no**. "Coming from
   a Mac" is fine as a *description*.
4. **Values**: is any colour, curve or radius a *verbatim* copy of a competitor's distinctive value where ours would
   do as well? Replace it with a §2 token. (The existing `CTRL_CLOSE/MIN/ZOOM` macOS hexes are the first instance.)
5. **Whole**: put the component beside its Mac or Windows counterpart. Would a reasonable person think it *is* that
   product? It should read "same idea, different hand". If it reads "same product", change the distinctive parts:
   plate shape, hue, glyph, proportion.
6. **Function first**: keep what people *operate* (positions, chords, menu order). Change what *identifies* (shapes,
   colours, marks, art).

---

## 8. WHAT MAKES IT OURS

* **ONE OS, two comforts.** No other desktop makes the Mac and Windows layouts first-class, live-switchable looks of
  one engine with one token table. Zorin and Deepin come close, as skins [S72] [S73]. Una and Softy are the product's
  promise in one switch.
* **The crystal.** Top-left in every look (R25). It is our system menu, our taskbar button, the FACET plate's chamfer
  and the wallpaper's planes. One geometric motif, everywhere.
* **The una accent**, a periwinkle at OKLCH h 268. The indigo-to-dawn wallpaper is our colour story and it is not
  borrowed: `DESKTOP_BG` was indigo before this file.
* **An honest material.** The VEIL is the wallpaper sampled once and tinted. It is built for a CPU compositor and is
  as cheap as a fill. We do not imitate glass we cannot render.
* **Integer craft.** Every curve is a Q16 table, every shadow a cached 9-slice, every length a 4 pt multiple on whole
  pixels. The "few excellent primitives" approach that makes Zed fast [S60] is how `wm.rs` already composes.
* **Density as a choice**, not a fight: Compact for the workbench (ui_guidelines §2), Standard for everyone, Spacious
  for the Pi on a TV.
* **Respect for the hand**: every control reachable by keyboard with a visible ring, motion that never blocks, and a
  reduced-motion mode that is a first-class look, not a degraded one.

---

## 9. APPENDIX — where each component is drawn today

Paths are from the repository root. Line numbers are at `bd3d399d`. "Today" says what the craft arc will find.

| Component | File(s) | Today |
|---|---|---|
| Metrics (all lengths) | `unaos/crates/kernel/src/ui.rs` (`base` :37, `Metrics` :90, `Metrics::at`) and `video/metrics.rs` | FRAME 5, BEVEL 1, TITLE_HEIGHT 34, CORNER 12 (top only), WIDGET 8, WELL 15, BUTTON 28, GAP 12, CONTROL_BOX 12, TEXT_PX 15 |
| Density scale | `unaos/crates/kernel/src/video/dpi.rs` (`s2_for` :74) | ppi/96 to the half: **2.5** on the rMBP (§2.1 sets ppi/110 → 2.0) |
| Colour tokens | `unaos/libs/sys/palette_core/src/lib.rs` (kit roles, `Tok`, `LIGHT`/`DARK`, `ACCENTS` :139); re-exported by `video/theme.rs`; the choice is `video/appearance.rs` + `prefs_core::appearance` | material-named tokens (bevel, gloss, gradient title); `CTRL_*` = macOS hexes; accent `crispy` `#4A73AA` |
| Chrome materials to retire | `video/ceramic.rs` (brushed aluminium head), `video/knurl.rs` (disc crosshatch), `video/paper.rs` (content paper texture; keep its procedural engine for the wallpaper), `theme.rs` gloss constants (:285–296) | skeuomorphic layers |
| Window chrome, shadow, compositor | `video/wm.rs`: `draw_window` :19640, `paint_window` :20343, `draw_title` :21759, `title_row_color` :14700, `title_ink` :14742, `control_disc` :15180, `chrome_hit` :15912, `title_bar_hit` :15942 | gradient title, no shadow, top corners only |
| Snapping | `video/winsnap.rs` (`paint` :343) | snap preview |
| Menu bar | `video/menubar.rs` (`compose` :1037, `compose_row` :1895); `video/strip.rs` (the strip primitive, `paint` :372); `video/status.rs`, `video/statusmenu.rs` (`paint_slider` :278) | solid strip |
| Crystal menu | `video/crystal.rs` (`compose` :854, `compose_row` :955) | — |
| Window menus | `video/winmenu.rs` (`draw_bar_row` :1656, `compose` :1714, `compose_row` :1841); `video/sysmenu.rs`; `video/appmenu.rs` | — |
| Dock | `video/dock.rs` (`compose` :826, `paint` :929, `compose_row` :952) | edge strip, captioned tiles 28 tall |
| Settings | `video/settings.rs` (`TAB_H` :60, `TAB_NAMES` :66, `btn`/`field`/`slider` :425–452, `paint` :455, panes :490–1758) | **nine tabs across the top** (R118 wants the left), flat rect buttons, a `_` caret, a square slider knob |
| Toasts, notifications | `video/toast.rs` (`post` :72), `video/notify.rs` (`paint_card` :328, `paint_center` :367), `video/notifypane.rs` | — |
| Dialogs | `video/dialog.rs` (`paint` :226) | — |
| Login | `video/login.rs` (`field` :444, `button` :464, `user_row` :479, `power_row` :2464), `video/loginwindow.rs` (`tile_avatar` :175), `video/loginrender.rs` | — |
| Launcher, switcher | `video/launcher.rs` (`paint` :462), `video/appswitch.rs` (`paint` :192) | — |
| Wallpaper, VEIL source | `video/wallpaper.rs` (`row` :59, `paint` :74), `video/paper.rs` | one flat `DESKTOP_BG` or an image |
| Text | `video/text.rs` (faces: Body, Chrome, Ui, Grid; `system.display.font` default DejaVu Sans), `video/font.rs` (bitmap fallback), `font_core::ui` (the engine) | DejaVu; chrome size derived from the bar |
| Bindings (Una / Softy) | `video/theme.rs` `CRISPY_ROWS`/`CRISPY_BINDINGS` :562/:649, `PC_ROWS`/`PC_BINDINGS` :662/:710; `video/shortcuts.rs` | both tables exist |
| Pointer | `video/cursor.rs`, `video/cursorshape.rs` | — |
| Volume/brightness bezel | `video/bezel.rs` (`paint` :210) | — |
| Aether chrome — model | `handlers/aether/src/chrome/{tabs.rs,omnibox.rs,history.rs,bookmarks.rs,find.rs,downloads.rs,zoom.rs,mod.rs}` | std-free model; faces only paint (R104) |
| Aether chrome — metal paint | `unaos/crates/user-aether/src/chrome.rs` (`TAB_H` :101 = 18, `BAR_H` :102 = 20, `FONT` :107 = 11.0, `BTN` :755, `paint_chrome` :1007), `user-aether/src/theme.rs` | one 38-px chrome band; no side tabs yet (R118) |
| Lumen | `unaos/crates/user-lumen/src/{main.rs (paint :1041),side.rs,theme.rs}` | — |

---

## 10. SOURCES

Method for every colour figure in §2.3: OKLCH → OKLab → linear sRGB (Björn Ottosson's matrices), gamut-mapped by
reducing chroma, gamma-encoded, then WCAG relative luminance and contrast `(L1+0.05)/(L2+0.05)` [S38]. The script is
a dozen lines and the seat can rerun it. All values above are its output.

**Design and practice**

* [S1] https://developer.apple.com/design/human-interface-guidelines/accessibility: macOS default text 13 pt, minimum 10 pt. Default control 28×28 pt, minimum 20×20 (iOS 44/28). Contrast 4.5:1 up to 17 pt, 3:1 at 18 pt+. About 12 pt padding around bezelled elements, about 24 pt around unbezelled ones. Enlarge text by at least 200 %.
* [S2] https://developer.apple.com/design/human-interface-guidelines/typography: macOS text styles (Large Title 26/32, Title1 22/26, Title2 17/22, Title3 15/20, Headline 13/16 bold, Body 13/16, Callout 12/15, Subheadline 11/14, Footnote 10/13). System fonts support dynamic optical sizes merging Text and Display. Tracking adjusts at every size.
* [S3] https://developer.apple.com/design/human-interface-guidelines/materials: Liquid Glass is a functional layer for controls and navigation. "Don't use Liquid Glass in the content layer". Use it sparingly. Clear glass only over visually rich backgrounds.
* [S4] https://developer.apple.com/design/human-interface-guidelines/motion: add motion purposefully. Make motion optional. Aim for brevity. Avoid motion on frequent interactions. Let people cancel motion.
* [S5] https://developer.apple.com/design/human-interface-guidelines/dark-mode: dark colours "aren't necessarily inversions". Contrast no lower than 4.5:1. Soften white backgrounds.
* [S6] https://developer.apple.com/design/human-interface-guidelines/color: avoid hard-coding system colour values. Dynamic colours are defined by purpose. Apply colour sparingly.
* [S7] https://developer.apple.com/design/human-interface-guidelines/the-menu-bar: menus in a fixed order. Always show the same items, disable instead of hiding. Support standard shortcuts. Prefer short one-word titles. The app name is in bold.
* [S8] https://developer.apple.com/design/human-interface-guidelines/toggles: use the switch only in a list row. Switches, checkboxes and radios go in the window body, not the frame or toolbar.
* [S9] https://developer.apple.com/design/human-interface-guidelines/sidebars: let people hide and customise the sidebar. Disclosure for hierarchy. Familiar symbols.
* [S10] https://raw.githubusercontent.com/material-components/material-web/main/tokens/versions/v0_192/_md-sys-motion.scss: durations short1–4 50/100/150/200, medium 250–400, long 450–600, extra-long 700–1000 ms. Easing standard (0.2,0,0,1), standard-decelerate (0,0,0,1), standard-accelerate (0.3,0,1,1), emphasized-decelerate (0.05,0.7,0.1,1), emphasized-accelerate (0.3,0,0.8,0.15).
* [S11] https://raw.githubusercontent.com/material-components/material-web/main/tokens/versions/v0_192/_md-sys-shape.scss: corner extra-small 4, medium 12, large 16, extra-large 28, full 9999 px.
* [S12] https://raw.githubusercontent.com/material-components/material-web/main/tokens/versions/v0_192/_md-sys-state.scss: state-layer opacities hover 0.08, focus 0.12, pressed 0.12, dragged 0.16.
* [S13] https://raw.githubusercontent.com/material-components/material-web/main/tokens/versions/v0_192/_md-sys-elevation.scss: levels 0/1/3/6/8/12.
* [S14] https://raw.githubusercontent.com/material-components/material-web/main/tokens/versions/v0_192/_md-sys-typescale.scss: body-medium 0.875 rem / 1.25 rem (14/20); display-large 3.5625 rem / 4 rem.
* [S15] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/global/durations.ts: 50/100/150/200/250/300/400/500 ms.
* [S16] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/global/curves.ts: decelerateMid (0,0,0,1), easyEase (0.33,0,0.67,1), and the others.
* [S17] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/global/borderRadius.ts: radii 2/4/6/8/12/16/24/32/40/circular.
* [S18] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/global/spacings.ts: 2/4/6/8/10/12/16/20/24/32.
* [S19] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/utils/shadows.ts: shadowN = "0 0 2px ambient, 0 (N/2)px Npx key" (N = 2…16); 28 and 64 use an 8 px ambient.
* [S20] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/alias/lightColor.ts: light shadow ambient rgba(0,0,0,0.12), key 0.14; darker pair 0.20/0.24; foreground1 #242424 on background1 #ffffff.
* [S21] https://raw.githubusercontent.com/microsoft/fluentui/master/packages/tokens/src/global/fonts.ts: font sizes 10/12/14/16/20/24 with line heights 14/16/20/22/28/32.
* [S22] https://raw.githubusercontent.com/GNOME/libadwaita/main/src/stylesheet/_common.scss: button radius 9, card 12, menu 9, popover = menu + 6, dialog and window = button + 6. Focus and button transitions 200 ms ease-out-quad.
* [S23] https://raw.githubusercontent.com/GNOME/libadwaita/main/src/stylesheet/widgets/_window.scss: CSD shadow "0 0 14px 5px 15%, 0 0 5px 2px 10%, 0 0 0 1px 5%" with a 1 px outline. Backdrop shrinks to "0 0 14px 2px 3%". Maximized and tiled windows have radius 0.
* [S24] https://raw.githubusercontent.com/GNOME/libadwaita/main/src/stylesheet/_colors.scss: accent_color = oklab(from accent_bg min(l, 0.5) a b) in light and max(l, 0.85) in dark. window_bg #fafafb / #222226. Dark card_bg is white at 8 %. Nine named accents.
* [S26] https://raw.githubusercontent.com/GNOME/libadwaita/main/src/stylesheet/widgets/_buttons.scss: button min-height 24. Circular buttons 34×34. Text buttons min-width 100 in dialogs.
* [S27] https://raw.githubusercontent.com/KDE/breeze/master/kstyle/breezemetrics.h: Layout_DefaultSpacing 6, Button_MinWidth 80, CheckBox_Size 20, MenuItem margins 4, ScrollBar_SliderWidth 8, Frame_FrameRadius 5.
* [S28] https://raw.githubusercontent.com/KDE/kirigami/master/src/platform/units.cpp: gridUnit 18, smallSpacing 4, mediumSpacing 6, largeSpacing 8. Durations 50/100/200/400.
* [S29] https://raw.githubusercontent.com/pop-os/libcosmic/master/cosmic-theme/src/model/spacing.rs: spacing 0/4/8/12/16/24/32/48/64/128. Density Compact / Standard / Spacious remap the same names.
* [S30] https://raw.githubusercontent.com/pop-os/libcosmic/master/cosmic-theme/src/model/corner.rs: radii 0/4/8/16/32/160 (round) and 0/2/8/8/8 (slightly round).
* [S31] https://raw.githubusercontent.com/carbon-design-system/carbon/main/packages/layout/src/index.ts: `miniUnit = 8`.
* [S32] https://raw.githubusercontent.com/carbon-design-system/carbon/main/packages/type/src/scale.ts: `Yn = Yn-1 + (floor((n-2)/4)+1)*2`, giving 12,14,16,18,20,24,28,32,36,42,48…
* [S33] https://raw.githubusercontent.com/Shopify/polaris/main/polaris-tokens/src/size.ts: '100' = 4px, '200' = 8px, '400' = 16px.
* [S34] https://raw.githubusercontent.com/Shopify/polaris/main/polaris-tokens/src/themes/base/motion.ts: durations 0–500 in 50 ms steps. motion-ease cubic-bezier(0.25, 0.1, 0.25, 1).
* [S35] https://raw.githubusercontent.com/rsms/inter/master/README.md: "carefully crafted & designed for computer screens". Tall x-height. Tabular numbers and slashed zero. SIL OFL 1.1. Below 12 px some stems are drawn with two semi-opaque pixels because it prioritises higher-density rasterisation.
* [S36] https://newreleases.io/project/github/rsms/inter/release/v4.0: Inter 4.0 has six Display designs assigned to an `opsz` axis, and two variable fonts with `wght` and `opsz` *(via search)*.
* [S37] https://www.radix-ui.com/colors/docs/palette-composition/understanding-the-scale: steps 1–2 backgrounds, 6–8 borders (7 also the focus ring), 9 purest solid, 10 its hover, 11–12 text *(via search)*.
* [S38] https://raw.githubusercontent.com/w3c/wcag/main/understanding/20/contrast-minimum.html: 4.5:1 and 3:1. Ratios are thresholds and are not rounded (4.499:1 fails).
* [S39] https://raw.githubusercontent.com/w3c/wcag/main/understanding/21/non-text-contrast.html: visual information needed to identify a control and its state needs 3:1 against adjacent colours.
* [S40] https://raw.githubusercontent.com/w3c/wcag/main/understanding/22/target-size-minimum.html: targets at least 24×24 CSS px, or spaced.
* [S41] https://raw.githubusercontent.com/w3c/wcag/main/understanding/21/target-size-enhanced.html: targets at least 44×44 CSS px.
* [S42] https://raw.githubusercontent.com/w3c/wcag/main/understanding/22/focus-appearance.html: the indicator's area is at least a 2 CSS px perimeter, with ≥ 3:1 change between focused and unfocused.
* [S43] https://raw.githubusercontent.com/w3c/wcag/main/understanding/21/animation-from-interactions.html: let users turn off non-essential interaction-triggered motion; vestibular reactions include nausea and migraine.
* [S44] https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/geometry: 8 px for top-level containers (windows, flyouts, dialogs), 4 px for in-page elements, tooltips 4, not rounded when snapped or maximized *(via search)*.
* [S45] https://learn.microsoft.com/windows/apps/design/signature-experiences/typography: ramp 12/16, 14/20, 18/24, 20/28, 28/36, 40/52, 68/92. Segoe UI Variable's optical axis is automatic. Sentence case. Minimums 14 semibold / 12 regular *(via search)*.
* [S46] https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design: standard title bar height 32 px, title 16 px from the icon *(via search)*. Caption buttons 46×32 are cited only by a third-party bug report *(via search, secondary)*.
* [S47] https://betanews.com/2021/08/13/microsoft-claims-windows-11s-visual-effects-do-not-reduce-performance: Mica is an "opaque, dynamic material that incorporates theme and desktop wallpaper"; designed for higher performance than Acrylic; blurs the wallpaper once instead of re-sampling *(via search; Digital Trends and Windows Latest concur)*.
* [S48] https://atlassian.design/foundations/spacing: base unit 8 px, space.200 = 16 px, scale to space.1000 = 80 px *(via search)*.
* [S49] https://atlassian.design/foundations/elevation: four levels (sunken, default, raised, overlay). Raised and overlay carry shadows. In dark, overlay is a lighter grey *(via search)*.
* [S50] https://evilmartians.com/chronicles/oklch-in-css-why-quit-rgb-hsl: OKLCH lightness is perceptual, so palettes can be generated by formula; it covers P3 *(via search)*.
* [S51] https://linear.app/now/how-we-redesigned-the-linear-ui and https://linear.app/changelog/2024-03-20-new-linear-ui: the redesign cut visual noise and raised density and hierarchy, with a new theme generator; LCH is per a secondary summary *(via search)*.
* [S52] https://www.thurrott.com/software/browsers/296096/arc-browser-for-windows-review: a collapsible left sidebar of tabs; Spaces with colours; today tabs archive after 12 h; "almost nothing in Arc works the way you expect". Development stopped in May 2025 per toolradar.com *(via search)*.
* [S53] https://9to5google.com/2023/11/10/google-chrome-material-you-redesign/: Chrome Refresh 2023 has taller tabs and omnibox, rounder corners, lighter dividers, a rounded hover highlight, a site-controls chip replacing the lock, and outline icons *(via search)*.
* [S54] https://macrumors.com/2026/04/07/chrome-vertical-tabs: native vertical tabs on desktop Chrome from April 7, 2026: a left sidebar, full titles, collapsible, groups kept *(via search)*.
* [S55] https://daringfireball.net/2021/10/the_tragedy_of_safari_15_quote_unquote_tabs: Safari 15's tabs "look like buttons", waste space and do not scale to many tabs; Apple added back a separate tab bar *(via search)*.
* [S56] https://maketecheasier.com/replacing-firefox-with-zen-browser/: Zen has vertical tabs, workspaces, and a compact mode that hides the sidebar until the edge *(via search)*.
* [S57] https://kagi.com/orion/: a WebKit browser with a Safari-like look plus vertical tree-style tabs and tab groups *(via search)*.
* [S58] https://vivaldi.com/press/releases/vivaldi-browser-tabs-two-level/: tab stacks and a two-level stack bar; tiling *(via search)*.
* [S59] https://www.figma.com/blog/our-approach-to-designing-ui3/: floating panels slowed heavy users (cramped canvas, designs hidden, rulers farther away); reverted to fixed resizable panels *(via search)*.
* [S60] https://zed.dev/blog/videogame: the UI is rendered "like a videogame" from a few primitives (rectangles, shadows, text, icons, images), each with a dedicated shader, at 120 fps *(via search)*.
* [S61] https://www.warp.dev/blog/why-is-the-terminal-input-so-weird: the command editor is built into the terminal; commands and output are grouped into blocks *(via search)*.
* [S62] https://www.macstories.net/reviews/things-3-beauty-and-delight-in-a-task-manager/: the Magic Plus button is dragged where the task belongs; design that adds "a sense of fun" *(via search)*.
* [S63] https://hunted.space/product/bear/launches/bear-2-3: Bear won a 2017 Apple Design Award; a minimal UI *(via search)*.
* [S64] https://www.businesswire.com/news/home/20211202005370/en/: Craft was Mac App of the Year 2021, for speed and simplicity, built natively *(via search)*.
* [S65] https://blakecrosley.com/guides/design/raycast: a ~50 ms latency budget, inline shortcut badges, a native feel *(via search, third-party study)*.
* [S66] https://raw.studio/blog/how-notion-ux-converts-100-million-users/: whitespace, simple typography, a calm editing experience; hairline borders *(via search, third-party)*.
* [S67] https://haiku-os.org/docs/userguide/en/gui.html: the yellow tab instead of a full-width title bar; Stack & Tile is built on it *(via search)*.
* [S68] https://raw.githubusercontent.com/SerenityOS/serenity/master/README.md: "a love letter to '90s user interfaces"; late-1990s productivity aesthetic *(via search; README mirror)*.
* [S69] https://www.debugpoint.com/kde-plasma-6-review/: Plasma 6 made the floating panel the default, with a shadow and smooth de-floating; reviewers want Breeze overhauled *(via search)*.
* [S70] https://www.linux.org/threads/omg-ubuntu-cosmic-desktop-%E2%80%98frosted-glass%E2%80%99-ui-effect-previewed.61779/post-293985: COSMIC's frosted glass uses a "more performant" dual-Kawase blur. Auto-tiling: https://www.phoronix.com/news/System76-COSMIC-Better-Tiling *(via search)*.
* [S71] https://phabricator.kde.org/R108:0179f741bbd4cc01da5866fdf7b6a302869a099b: KWin replaced its blur with dual-Kawase ("virtually infinite blur with very very little performance cost", measured on a GPU); dual-Kawase is about 8 samples per pass; picom's configuration calls background blur "bad in performance" *(via search)*.
* [S72] https://www.ubuntubuzz.com/2017/04/a-visual-review-to-deepin-154-gnulinux-beauty.html and https://linuxinsider.com/?p=86473: Deepin is called "strikingly beautiful"; its dock has fashion (Mac-like) and efficient (panel) modes *(via search)*.
* [S73] https://gamingonlinux.com/2021/08/zorin-os-16-pro-brings-a-little-windows-11-flavour-to-linux/page=1: Zorin's Appearance tool switches between layouts including macOS-style, Windows 11-style and Windows Classic *(via search)*.
* [S74] https://steamdeckhq.com/news/new-big-picture-now-in-beta-for-steam-based-on-deck-game-mode/: the controller-first Steam Deck UI became the desktop Big Picture mode *(via search)*.
* [S75] https://material.io/design/iconography/system-icons.html: a 24 dp grid with a 20 dp live area and 2 dp padding; dense 20 dp with a 16 dp live area; 2 dp stroke *(via search)*.
* [S76] https://docs.elementary.io/hig/design-philosophy/accessible-configuration: settings are more code, bugs, testing and documentation, and can be an easy way out of design decisions; prefer sane defaults *(via search)*.
* [S77] https://develop.kde.org/hig/layout/units: do not rely on the ratio between gridUnit and spacing, because it changes with the user's font *(via search)*.
* [S78] https://dev.to/mechcloud_academy/shadcnui-the-component-library-that-isnt-a-library-5b94: components are copied into the codebase and owned, with no upstream breaking changes *(via search)*.
* [S79] https://www.superdesign.dev/blog/uber-design-system: Base is minimal and composable, with one primary button per screen and a 4 px spacing scale *(via search, third-party)*.
* [S80] https://web.dev/learn/accessibility/motion: we cannot predict which movement harms; default to less animation *(via search)*. Crossfade substitution: https://telescope.pleo.io/foundations/motion/motion.accessibility *(via search)*.
* [S80b] https://en.wikipedia.org/wiki/Features_new_to_Windows_11: Start and the taskbar are centred with an option to move them left; Snap Layouts open from hovering the maximize button *(via search)*.
* [S81] https://reddit.sudovanilla.org/r/UsabilityPorn: founded because setups were "too riced-out and unappealing for long-term desktop use"; screenshots must have at least one window open. https://draco.is-a.dev/guides/ricing-guide.html: match wallpaper contrast and palette *(via search)*.

**Law**

* [L1] https://law.justia.com/cases/federal/appellate-courts/F3/35/1435/605245/: *Apple Computer, Inc. v. Microsoft Corp.*, 35 F.3d 1435 (9th Cir. 1994). Analytic dissection filters out licensed similarities and those from common ideas or their logical extensions; what remains is protected against "virtual identity" only *(via search)*.
* [L2] https://en.wikipedia.org/wiki/Lotus_Development_Corp._v._Borland_International,_Inc.: *Lotus v. Borland*, 49 F.3d 807 (1st Cir. 1995), aff'd by an equally divided Court, 516 U.S. 233 (1996). The menu command hierarchy is an uncopyrightable "method of operation" under § 102(b) *(via search)*.
* [L3] https://www.bitlaw.com/source/cases/patent/Apple_v_Samsung_Fed_Cir.html: *Apple v. Samsung*, 786 F.3d 983 (Fed. Cir. 2015). Design patents D'677, D'087 and D'305 (GUI) were affirmed; the unregistered and registered iPhone trade dress were reversed as functional *(via search)*.
* [L4] https://hunton.com/insights/legal/samsung-v-apple-the-supreme-court-limits-damages-in-design-patent-cases: *Samsung v. Apple* (U.S. Dec. 6, 2016). The "article of manufacture" for total-profit damages may be a component *(via search)*.
* [L5] https://www.jenner.com/en/news-insights/publications/client-alert-us-supreme-court-holds-that-copying-declaring-code-is-fair-use: *Google v. Oracle*, 593 U.S. (2021). Reimplementing an interface so programmers can use accrued talents is fair use; copyrightability is assumed, not decided *(via search)*.
* [L6] https://www.loeb.com/en/insights/publications/2012/06/tetris-holding-llc-v-xio-interactive-inc: *Tetris Holding v. Xio*, 863 F. Supp. 2d 394 (D.N.J. 2012). Copying the whole look and feel infringed; the colours and styles were "arbitrary flourishes", not functional *(via search)*.
* [L7] https://www.wipo.int/wipolex/ru/text/581663: *Wal-Mart v. Samara Bros.*, 529 U.S. 205 (2000). Product-design trade dress is protectable only on a showing of secondary meaning *(via search)*.
* [L8] https://corporate.findlaw.com/intellectual-property/supreme-court-cuts-back-on-trademark-rights-in-traffix.html: *TrafFix Devices v. Marketing Displays*, 532 U.S. 23 (2001). Functional features are not protectable trade dress *(via search)*.
* [L9] https://www.monotype.com/resources/update-application-copyright-law-typeface-design-and-font-software: 37 CFR 202.1(e) excludes "typeface as typeface"; font software is registrable as a program (1992 rule) *(via search)*.
* [L10] https://apple.slashdot.org/story/01/09/28/0241232/apple-still-says-no-to-aqua-like-themes and https://betanews.com/article/stardock-gets-heat-for-aqua-skins/: Apple pressed theme sites (themes.org, Skinz.org) and Stardock to remove Aqua-like themes and skins in 2000–2001, alleging copied artwork and code *(via search)*.
* [L11] https://en.wikipedia.org/wiki/Microsoft_Corp._v._Lindows.com,_Inc.: settled 2004. Microsoft paid $20 M; Lindows became Linspire and handed over its domains; the "windows is generic" defence had been gaining ground *(via search)*.
* [L12] https://developer.apple.com/forums/thread/727961 and https://developer.apple.com/fonts: the SF fonts licence limits use to mockups of software for Apple platforms; no embedding or redistribution *(via search)*.
* [L13] https://learn.microsoft.com/es-mx/windows/apps/design/style/segoe-fluent-icons-font: Segoe Fluent Icons may be downloaded for design and development but not shipped to another platform *(via search)*.

*Source count: 81 design and practice entries (S1–S81 without S25, plus S80b) and 13 law entries: 94 entries,
101 distinct URLs.*
