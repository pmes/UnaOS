# Research C: interface, hardware and power, accessibility and i18n, delight
Key: **built** = in the tree at `1719fe54` (BP-video); **DL** = a DESIGN-LANGUAGE decision, not yet built; **new** = proposed here. Each wire line marked new is proposed, not an existing gate.

## 8. The interface
| what they do wrong | evidence (source) | what UnaOS does instead | the measurable bar |
|---|---|---|---|
| One OS, several design eras: Control Panel still sits beside Settings after 13 years, and XP/8/10/11 dialogs are mixed | [Pureinfotech][1], [PC Gamer][2], [mjtsai][3] | Every painter reads `theme::tok(Tok)` from the single `palette_core` table, and a colour literal anywhere else fails the gate (built, BP-video-1/2) | `appearance` gate `AUDIT_LITERALS_OUTSIDE = 0`; a screenshot of every kernel app in both looks with no un-tokened chrome |
| Settings sprawl: the same switch lives in two apps, and Settings hands you off to legacy panes | [Pureinfotech][1], [Betanews][4] | Every preference is one Principia key with one Settings row (R118: tabs on the left, a search field at the top, grouped-list cards). There are no dotfiles and no build knobs (built, BP-video-3) | new `:: SETTINGSMAP: keys=N rows=N orphans=0 ::`; a search for any key's label reaches its row in ≤ 2 keystrokes plus Return |
| "Early X-windows vibe": bevels, oversized UI at the wrong dpi, no window edge or shadow | Peter's verbatim words in DESIGN-LANGUAGE header; SerenityOS/Haiku survey rows | `scale = ppi/110` (rMBP 2.0 → 1440×900 logical), a 4 pt grid, Inter on a fixed ramp, a pre-rendered 9-slice `elev.3` shadow and a 1-device-px window edge (DL §2.1-2.4, WINEDGE) | `[wm]` logical panel 1440x900 on the rMBP; a staged screenshot signed off by Peter (R-id) |
| Modal dialogs that block the whole system, and several competing primary buttons | [NN/g heuristics][5] (user control, error prevention), [Tog][6] | Window-modal by default; system-modal only with `scrim` (BP-video-5, one `set_modal_top` row); one primary button per view; Return accepts and Escape cancels (DL §3.8) | `:: LOGINZ: … PASS ::`; a new fixture counts `accent.fill` buttons per dialog and fails above 1 |
| Notification overload and ads in Start and Explorer | [Register][7], [Pureinfotech ads][8]; interruptions cost about 23 min to resume ([Gallup/Mark][9]) | Toasts: at most 3 on screen, 5 s auto-dismiss, paused on hover (DL §3.6). The shell holds no ad or promotion surface: furniture is kernel code with no remote content feed | new `:: TOAST: shown= dropped= stack_max=3 ::`; `grep` of furniture finds no net call (fixture) |
| Lost direct manipulation: windows you cannot grab or snap, and drag replaced by menus | Shneiderman ([direct manipulation][10]); Things 3 / Haiku tab in DL §1 | Hit boxes: 20 pt control discs, a 6 pt resize band, 12 pt corners, snap layouts in both looks (DL §2.9); motion never holds input (DL §2.6) | input latency from a pointer event to the moved frame ≤ 16.7 ms (`DESKTOPCRAFT … -> UNDER`) |

## 9. Hardware and power
| what they do wrong | evidence (source) | what UnaOS does instead | the measurable bar |
|---|---|---|---|
| **Expiration dates the user is not privileged to** (Peter: "when the company decides to axe a thing that's it, consumer be damned"): the Windows 11 TPM/CPU cutoff strands 240-400 M working PCs (about 1.6 B lb of e-waste), macOS drops 2012-2015 Macs, and Chromebooks carry an AUE "death date" | [Canalys/TechRadar][11], [PIRG][12], [OCLP][13], [PIRG Chromebook Churn][27] | The 2012 rMBP, the Pi 4 and the Jetson are first-class targets (`hw-rmbp`, `hw-pi4`, `hw-jetson`). Limits come from the hardware present (`wincap`, BP-video-7), never from a list, and no feature of the OS needs a server to keep working | new `:: OLDEST: board= year= image=<line> -> BOOTS ::` printed by every image line (`image-line.env`, `-pi`, `-orin`); the fixture boots with the network unplugged to `[desktop] built` |
| Vendors stop shipping drivers, so old Macs need community patchers that disable cores and features | [OCLP via AppleInsider][13], [OCLP 2.0 notes][14] | Drivers live in-tree with the kernel and are gated per chip (`docs/dev/OS/08_VIDEO/PARITY.md`); firmware is pinned by digest (`unaos/firmware/kepler.pins`, R127) | Each chip has a PARITY row that is green on the bench; a firmware digest mismatch refuses to boot |
| Sleep that is not sleep: Modern Standby drains 100 % → 18 % overnight and cooks laptops in bags | [Dell forum][15], [msftnext][16] | Sleep is a measured state with a named waker, not a hope (new; power/SMC has no sheet yet, per INDEX) | new `:: SLEEP: drain_pct_per_h ≤ 1 wakes=0 unexplained ::` over 8 h on the rMBP |
| Fan noise from background churn (indexers, telemetry, updaters) | Forum boot and idle anecdotes ([ElevenForum][17]) | The panel is idle when nothing changes: per frame only the sparse STARFIELD shimmer runs through the erase drain, and the picture is generated once per panel size (built, BP-video-11) | `shimmer_us=` in the DESKTOPCRAFT cost line; new idle wire `cpu_idle_pct ≥ 98` with SMC fan RPM at its floor |
| Boot to a usable desktop takes 20-60 s on stock PCs | [ElevenForum][17], [Framework][18] | One path: loader, then kernel_main, then the splash hold, then login, then `phase=desktop`, with each step stamped by `bootpace` (BP-boot) | new budget: power-on to `[desktop] built` ≤ 5 s on the rMBP from the stamped boot log |

## 10. Accessibility and internationalisation
| what they do wrong | evidence (source) | what UnaOS does instead | the measurable bar |
|---|---|---|---|
| Contrast decided by taste: 79 % of the top million home pages have low-contrast text | [WebAIM Million 2025][19] | Every text token is solved in OKLCH against WCAG 2.x. Accents are solved for white labels at ≥ 4.6:1, and a threshold is never rounded (DL §2.3) | new `tests contrast`: every `text.*` and `accent.*` pair on every `surface.*`, light and dark, ≥ 4.5:1 (≥ 3:1 for `line.control`); 4.499 fails |
| A screen reader is a bolt-on that breaks when the compositor changes (Orca under Wayland: no Qt apps, no synthesized clicks) | [GNOME wiki][20], [orca-list][21] | The kernel `wm` owns the window table and the input router, so a role/name tree can be published from the same rows the compositor draws (new: an a11y bus verb in `una-abi`) | new `:: A11YTREE: windows=N unnamed=0 ::`; every Settings row is reachable by Tab and announced |
| Keyboard focus that is invisible, or drawn on every click | WCAG 2.4.11/2.4.13; DL [S42] | `focus.ring` 2 pt outside with a 2 pt offset, focus-visible only, ≥ 3:1 change (DL §2.7) | a screenshot fixture: Tab through Settings, and every stop shows a ring of ≥ 2 device px |
| Motion and transparency you cannot turn off | WCAG 2.3.3; DL §2.6 | Reduce motion makes every spatial animation a 120 ms crossfade, and it is ON by default where `vperf` misses 16 ms. Reduce transparency makes the VEIL 100 % `surface.window` | The Principia keys exist and have rows; new `[look] motion=reduced` appears on a slow panel |
| i18n as an afterthought: tofu glyphs, hard-coded LTR, one face | DL §2.2 | Fallback chain: Inter, then Noto Sans by script, then DejaVu, then `font8x8`. Every label is a key in a string table, and layout mirrors under RTL (new) | new fixture: render one line each of Arabic, Devanagari, CJK and Thai with `notdef=0`; a pseudo-locale (+40 % length) shows no clipped label |

## 11. Fun and delight
| what they do wrong | evidence (source) | what UnaOS does instead | the measurable bar |
|---|---|---|---|
| The joy is gone. The Amiga's Boing Ball (1984), the Mac saying "hello" and the iPhone's slide-to-unlock (2007) were each an instant, physical surprise | [Boing Ball][22], [Macintosh 1984][23], [iPhone keynote][24] | The start scene (STARTSCENE, R128) is one scene that runs from the splash hold through login to the desktop, with no cut to a black screen or a console | the capture series shows no `fbcon` frame between splash and `[desktop] built` |
| Loading spinners: responses over 400 ms break flow | [Doherty threshold][25] | Every repaint is budgeted at 16 667 µs and measured on the running machine (built, BP-video-12); the paint path does no disk or network I/O | `:: DESKTOPCRAFT: … budget_us=16667 -> UNDER ::` on every bench machine; Launcher result ≤ 50 ms |
| Bloat: a desktop that idles at gigabytes, and an app cap that is a guess | Wirth's law; DL §1.4 (Zed, Craft) | Windows and processes are limited by free heap (`wincap`), and the desktop costs only what it draws | `[wm] limit windows= procs= from=`; new memory line at the desktop ≤ 64 MB kernel+furniture |
| Ads, upsells and "recommended" tiles turn the shell into a storefront | [Register][7], [TechRadar ads][26] | Furniture has no remote content feed (§8 row 5), so the only thing on the desktop is the user's | Fixture: zero furniture surfaces fed by the network |
| Prettiness that costs frames (live blur), or no beauty at all | picom docs, Mica (DL §2.4) | The VEIL blurs the wallpaper once per set or resize, and the STARFIELD shimmer is sparse, so beauty is free per frame (built: shimmer; DL: VEIL) | `shimmer_us=` ≤ 1 % of the budget; the VEIL frame cost equals a solid fill (±5 %) |

**Three decisions that matter most.** (1) **Measure on the wire.** Every interaction must land inside the 16 667 µs frame budget (BP-video-12) on the slowest machine we claim, and the 2012 rMBP is that machine. One gate then covers speed, delight ("instant everything") and the Lazarus promise, because none of them can be faked by adding hardware. (2) **One token table and one preference store.** `palette_core` plus Principia `system.look` (BP-video-1/3) cures what makes Windows and Linux feel patched: mixed design eras, two settings apps, contrast left to taste. A contrast gate and a settings-map gate will catch regressions as they happen. (3) **Accessibility belongs in the window table, not beside it.** `wm` already owns every window row and the input router, so the role/name tree, focus-visible ring, reduce-motion and RTL mirroring should come from the same rows the compositor paints. This is the one thing a clean-sheet OS can do that Wayland-era Linux structurally cannot.

[1]: https://pureinfotech.com/windows-11-ui-inconsistencies/
[2]: https://www.pcgamer.com/uk/i-cant-believe-windows-11-still-hasnt-gotten-rid-of-these-old-ass-menus/
[3]: https://mjtsai.com/blog/2023/01/03/layers-of-ui-inconsistencies-in-windows-11
[4]: https://betanews.com/2025/08/22/microsoft-is-still-slowly-chipping-away-at-the-control-panel-in-windows-11/
[5]: https://www.nngroup.com/articles/ten-usability-heuristics/
[6]: https://asktog.com/atc/principles-of-interaction-design/
[7]: https://www.theregister.com/2024/04/24/adverts_windows_11/
[8]: https://pureinfotech.com/file-explorer-ads-windows-11/
[9]: https://news.gallup.com/businessjournal/23146/Too-Many-Interruptions-Work.aspx
[10]: https://en.wikipedia.org/wiki/Direct_manipulation_interface
[11]: https://www.techradar.com/computing/windows/windows-10-support-ending-could-be-an-environmental-disaster-that-puts-240-million-pcs-on-the-scrapheap
[12]: https://pirg.org/articles/r-i-p-windows-10-and-hundreds-of-millions-of-pcs-that-depended-on-it/
[13]: https://appleinsider.com/inside/macos-sequoia/tips/how-to-install-macos-sequoia-on-unsupported-macs/amp
[14]: https://www.callapple.org/?p=74764
[15]: https://www.dell.com/community/XPS/Power-drain-and-overheating-with-modern-standby/m-p/8041619
[16]: https://www.msftnext.com/?p=39452
[17]: https://www.elevenforum.com/t/slow-windows-11-boot-time.11281/latest
[18]: https://community.frame.work/t/framework-boot-speed/13066
[19]: https://webaim.org/projects/million/2025
[20]: https://wiki.gnome.org/Accessibility/Wayland
[21]: https://mail.gnome.org/archives/orca-list/2015-October/msg00036.html
[22]: https://en.wikipedia.org/wiki/Boing_Ball
[23]: https://en.wikipedia.org/wiki/Macintosh_128K
[24]: https://en.wikipedia.org/wiki/IPhone_(1st_generation)
[25]: https://lawsofux.com/doherty-threshold/
[26]: https://www.techradar.com/news/windows-11-start-menu-ads-are-gathering-momentum-just-stop-it-microsoft
[27]: https://pirg.org/media-center/new-chromebook-churn-report-highlights-problems-of-short-lived-laptops-in-schools/
