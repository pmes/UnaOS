# DESKTOPBUILT (rmbp-ledger B387; R92, R93) — design

**Finding (read from the code and flight 24's wire, not the row's premise).** The menubar was painted at
`login ok` (the R92 enable + composite in `users::stage_publish`) but WITHOUT the battery: the status item is
fed by `status::poll`, which the services gate holds until the session opens, so its first sweep answers
~1 s AFTER that composite (`[status] poll n=1 answered=1 src=smc` at 12:11:40 vs `login ok` 12:11:39).
`status::store` dirties nothing, the bare desktop (R88) mints no window, so no composite runs again until a
press that composites — the crystal menu. `[strip] paint` only prints AT-RISK passes, so its absence on
card 3 is not "no paint"; Peter's glass ("battery only after the crystal menu") is the fact. Card 1's empty
glass is the latch race (`bar_release` while the stage still read CreateUser). Both are the owed/held shape.

**The seam.** Kernel — kernel-by-ruling (R93): one new file `video/desktopbuild.rs`. The desktop is an
object that does not exist before a session: `strip::compose_all` paints furniture only when it is BUILT
(and vacates bar and dock pixels when it is not). `build(why)` is called at `login ok`
(`login::close_into_session`) and at a store-less Desktop stage; on x86 the build runs on the next
device-service pass (the task that already polls the SMC; never the click/key path, never masked): it
waits (bounded 500 ms) for the battery source to resolve, turns the bar on, re-arms the wallpaper,
composites ONCE and reads back that the bar and dock own pixels. While built, a change of the battery
item composites (the bar's damage follows its model). Logout calls `teardown()`: the bar goes off,
the object is unbuilt, one composite vacates the strips; the next login builds fresh.

**Deleted:** `users::BAR_HELD`, `bar_owed`, `bar_release`, `stage_publish`'s bar-off + `installer_sweep`
arm and R92's unconditional enable; `login::installer_sweep`, `installer_release`, `furniture_owed`;
`desktop_uefi::activate`'s `bar_owed` (the bare activation constructs nothing).

**Milestones.** M1 the design. M2 `desktopbuild.rs` (build/teardown/service, the gate in `compose_all`,
the battery-change composite) wired at login/stage/logout; the latch and sweep deleted.

**Witness (every build, then once at the first):**
`[desktop] built at=<login ok|stage> bar_ms=<n> battery=<painted|absent|unpainted> dock=<painted|unpainted> pins=<n>`
`:: DESKTOPBUILT: prebuilt=<0|1> built_at=<login|stage> bar_first_paint_ms=<n> battery=<0|1> dock=<0|1> swept=<n> -> PASS|FAIL ::`
(PASS: prebuilt=0, swept=0, bar_first_paint_ms ≤ 500, battery=1 on a board with a battery source, dock=1).
Logout: `[desktop] torn down why=logout bar_off=1`.

**Owed:** the clock's minute tick still composites only with other damage (same class; not in this arc);
a metal flight reads the lines above.
