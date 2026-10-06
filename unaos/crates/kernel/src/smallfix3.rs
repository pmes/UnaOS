//! CHARTER: Kernel — kernel-by-ruling
//!
//! SMALLFIX3 (rmbp-ledger B416) — the merge17 fold's hygiene, the parts that live in code:
//!
//! * **Action codes unique by construction.** `video::clipboard::action_code` is the ring-3 wire value of
//!   every [`Action`]; [`ACTIONS`] lists every variant (the exhaustive [`listed`] match fails the build
//!   when a variant is added and not listed here) and a const assertion refuses two variants on one code —
//!   ATTRCOLUMNS' Get Info and APPMENU2's Quit both took 42 on their own branches.
//! * **The fixture registry with its arcs.** `tests list` (and `help tests`, which runs it) prints every
//!   registered fixture beside the arc that wrote it ([`arc_of`]): a fixture named after its arc needs no
//!   row; [`ARCS`] carries the ones that are not.
//! * **The witness.** `tests smallfix3` →
//!   `:: SMALLFIX3: boot_writers=<n|unarmed> event_codes=unique bus_verbs=unique action_codes=unique theme_rows=derived -> PASS ::`.
//!   Read-only (R80): nothing runs at boot; the fixture registers behind the `tests` verb.
use crate::video::keymap::Action;

/// Every [`Action`] variant, once.
pub const ACTIONS: &[Action] = &[
    Action::Screenshot, Action::ScreenshotRegion, Action::ScreenshotWindow, Action::Copy, Action::Cut, Action::Paste,
    Action::SelectAll, Action::LogOut, Action::SelectLeft, Action::SelectRight, Action::SelectLineStart,
    Action::SelectLineEnd, Action::Deselect, Action::CursorLeft, Action::CursorRight, Action::CursorLineStart,
    Action::CursorLineEnd, Action::BrightnessDown, Action::BrightnessUp, Action::CycleWindow, Action::LockScreen,
    Action::ShowShortcuts, Action::ScrollPageUp, Action::ScrollPageDown, Action::ScrollTop, Action::ScrollBottom,
    Action::SnapLeft, Action::SnapRight, Action::SnapZoom, Action::SnapRestore, Action::WinNudgeLeft,
    Action::WinNudgeRight, Action::WinNudgeUp, Action::WinNudgeDown, Action::WinSizeLeft, Action::WinSizeRight,
    Action::WinSizeUp, Action::WinSizeDown, Action::Minimize, Action::CycleApp, Action::QuitApp, Action::CloseWindow,
    Action::HideApp, Action::OpenSettings, Action::ForceQuit, Action::ClearView,
];

/// Exhaustive on purpose (no `_` arm): a new variant does not compile until it is named here — and the
/// reviewer adding it here adds it to [`ACTIONS`] three lines up.
pub const fn listed(a: Action) -> bool {
    match a {
        Action::Screenshot | Action::ScreenshotRegion | Action::ScreenshotWindow | Action::Copy | Action::Cut
        | Action::Paste | Action::SelectAll | Action::LogOut | Action::SelectLeft | Action::SelectRight
        | Action::SelectLineStart | Action::SelectLineEnd | Action::Deselect | Action::CursorLeft
        | Action::CursorRight | Action::CursorLineStart | Action::CursorLineEnd | Action::BrightnessDown
        | Action::BrightnessUp | Action::CycleWindow | Action::LockScreen | Action::ShowShortcuts
        | Action::ScrollPageUp | Action::ScrollPageDown | Action::ScrollTop | Action::ScrollBottom | Action::SnapLeft
        | Action::SnapRight | Action::SnapZoom | Action::SnapRestore | Action::WinNudgeLeft | Action::WinNudgeRight
        | Action::WinNudgeUp | Action::WinNudgeDown | Action::WinSizeLeft | Action::WinSizeRight | Action::WinSizeUp
        | Action::WinSizeDown | Action::Minimize | Action::CycleApp | Action::QuitApp | Action::CloseWindow
        | Action::HideApp | Action::OpenSettings | Action::ForceQuit | Action::ClearView => true,
    }
}

/// No two listed actions share a wire code, no code is zero, every entry is a listed variant.
pub const fn action_codes_unique() -> bool {
    let mut i = 0;
    while i < ACTIONS.len() {
        let c = crate::video::clipboard::action_code(ACTIONS[i]);
        if c == 0 || !listed(ACTIONS[i]) {
            return false;
        }
        let mut j = i + 1;
        while j < ACTIONS.len() {
            if crate::video::clipboard::action_code(ACTIONS[j]) == c {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}
const _: () = assert!(action_codes_unique(), "SMALLFIX3: two Actions share an INPUT_EV_ACTION wire code");

/// The theme tables are slices: their counts are `.len()`, never a typed literal (`[Binding; N]` made every
/// arc that added a row edit the same line — the fold's conflict). Read off the live tables.
pub fn theme_rows() -> (usize, usize) {
    (crate::video::theme::CRISPY_ROWS.len(), crate::video::theme::PC_ROWS.len())
}

/// Writes refused on the boot FAT (R99) since boot, by any fixture. `None` = the gate is not armed on this
/// tree/boot (no `fs::bootfat`, or a FAT-root boot where the FAT is `/`). ROOTDISK2's fold replaces the body
/// with `bootfat`'s non-probe refusal count (smallfix3.md, patch P1).
pub fn boot_writers() -> Option<u32> {
    None
}

/// `(fixture, arc)` for the fixtures NOT named after their arc. Every other fixture's arc is its name,
/// upper-cased (`windowlist` → WINDOWLIST).
pub const ARCS: &[(&str, &str)] = &[
    ("activity", "KVBLANK6"), ("ahciw", "AHCIROOT"), ("appmenu", "APPMENU2"), ("attr", "ATTRSURF"),
    ("bandy3", "BANDY3"), ("blitter", "KCOMP"), ("boot2-login", "LOGINFLOW2"), ("botpark", "BOT-PARK"),
    ("brightstep", "BRIGHTSLIDER"), ("bt", "BTHID"), ("canonguard", "CLOCK-X1"), ("ce", "KBLIT"),
    ("clickband", "MENUDROP"), ("clickroute", "TERMSEL2"), ("clock", "CLOCKCORE"), ("crystal", "MENU-OCC"),
    ("ctrldecline", "ARMROUTER"), ("dmgovlp", "DMGOVLP"), ("dns", "DNS-X86"), ("dock", "DOCK"),
    ("dragperf", "ARMROUTER"), ("dragwedge", "ARMROUTER"), ("ehci", "EHCI-HID"), ("ehciisr", "EHCI-HID"),
    ("elfbss", "STORMFAULT"), ("exec", "EXECNAME"), ("fatverb", "FATVERB"), ("fileopen", "FILEOPEN"),
    ("font", "KERNELFONT"), ("hda", "PULSE"), ("hda1", "PULSE"), ("hda2", "PULSE"), ("hda220", "PULSE"),
    ("hda880", "PULSE"), ("hdaboth", "PULSE"), ("helpdoc", "HELPDOC"), ("hittest", "ARMROUTER"),
    ("imgview", "IMGVIEW"), ("install", "SELFINSTALL2"), ("instgui", "INSTALL3"), ("keplerlog", "KEPLER"),
    ("kvblank", "KVBLANK"), ("linuxabi", "LINUXABI"), ("linuxabi2", "LINUXABI2"), ("linuxabi3", "LINUXABI3"),
    ("login-chain", "LOGINST"), ("logout", "LOGINFLOW2"), ("lumen", "LUMENAPP"), ("metrics", "UIMETRICS"),
    ("movevacate", "WINMOVE"), ("name", "EXECNAME"), ("net", "NETRING3"), ("passperiod", "EHCI-HID"),
    ("play", "PLAYCODEC"), ("playwav", "PULSE"), ("power", "POWERMENU"), ("prof", "PROFILE"),
    ("prof2", "PROFILE2"), ("prtscr", "PRTSCR"), ("ptrdead", "LOCKFIX"), ("quarryops", "QUARRYOPS"),
    ("ring3abi", "RING3ABI2"), ("selfbuild", "SELFBUILD1"), ("shot", "GLASSEYES"), ("smallfix3", "SMALLFIX3"),
    ("sock", "INPUTSTALL2"), ("spawnstorm", "WINDOWCAP3"), ("stackroom", "STACKGUARD2"), ("tste", "TSTE"),
    ("typematic", "UVUG-6"), ("u20c", "CLOCK-X1"), ("u3", "U3"), ("unafs", "UNAFSX86"), ("unafsroot", "HOMESOIL"),
    ("usbnet7", "NETFRAME"), ("uvc", "UVC"), ("vugres", "DMGOVLP"), ("wcdskip", "WCDLATCH"), ("wifi", "WIFI1"),
    ("winx", "WINX"), ("winx-pulse", "WINX"), ("winx-stat", "WINX"), ("winx-threads", "WINX-7"),
    ("winx-vug", "WINX"), ("winx3", "WINX"), ("wmdirect", "WMDIRECT"), ("wxaudit", "WXAUDIT"),
    ("x86bind", "X86BIND"),
];

/// The arc a registered fixture belongs to (upper-cased into `out` when it is the fixture's own name).
pub fn arc_of(name: &str) -> alloc::string::String {
    match ARCS.iter().find(|r| r.0 == name) {
        Some(r) => alloc::string::String::from(r.1),
        None => name.to_ascii_uppercase(),
    }
}

/// Register `tests smallfix3` once (every build: the fixture reads tables, no hardware).
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("smallfix3", selftest);
    }
}

/// `tests smallfix3` — the fold's codes and counts, read from the live tables.
pub fn selftest() {
    let bw = boot_writers();
    let ev = una_abi::codes_unique_u64(una_abi::INPUT_EV_ALL);
    let bv = una_abi::codes_unique_u8(una_abi::BUS_VERB_ALL);
    let ac = action_codes_unique();
    let (crispy, pc) = theme_rows();
    let rows = crispy > 0 && pc > 0;
    let pass = bw.unwrap_or(0) == 0 && ev && bv && ac && rows;
    let u = |b: bool| if b { "unique" } else { "CLASH" };
    let bws = match bw { Some(n) => alloc::format!("{}", n), None => alloc::string::String::from("unarmed") };
    serial_println!(
        ":: SMALLFIX3: boot_writers={} event_codes={} bus_verbs={} action_codes={} theme_rows={} -> {} :: events={} verbs={} actions={} crispy_rows={} pc_rows={} ::",
        bws, u(ev), u(bv), u(ac), if rows { "derived" } else { "EMPTY" }, if pass { "PASS" } else { "FAIL" },
        una_abi::INPUT_EV_ALL.len(), una_abi::BUS_VERB_ALL.len(), ACTIONS.len(), crispy, pc
    );
}
