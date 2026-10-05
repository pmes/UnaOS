//! CHARTER: Midden — shared-core (the redaction table is `midden_core::secret_from` / `midden_core::redact`, both rings; R65)
//!
//! CONSOLEFIX M3 (rmbp-ledger B365, R65) — **A PASSWORD NEVER REACHES THE WIRE.**
//!
//! Flight 22 put Peter's keyring password on the serial wire three ways:
//!
//! * the command tracer: `:: [midden] cmd="holocron init qwerty peter" -> Exec holocron.elf ::` (and the
//!   `holocron put … sk-ant-…` line the same way) — `shell.rs` printed `cmd_line` raw;
//! * `USB-DEBUG: KEY 0x71 'q'` — the raw-key witness, withheld only while the LOGIN screen is up;
//! * `EHCI-HID: KEY: 'q'` — the internal keyboard's edge line, withheld NEVER (not even under the login
//!   screen: lines 881/884/888 of `f22-boots.log` are a login password).
//!
//! The answer to "where does the secret start on this line" is `midden_core::secret_from` (one table, both
//! rings). This file is the kernel's use of it:
//!
//! * [`trace`] — the ONE `:: [midden] cmd="…" -> … ::` printer, redacting through `midden_core::redact`;
//! * [`note_line`] — the console's line editor publishes, after every edit, whether the line typed so far
//!   has reached its secret (`holocron init ` → yes); [`withhold`] is what every raw-key echo asks (lock-free;
//!   LOGIN13's screen/prompt state is cached by [`refresh`] on the main loop);
//! * [`wire_note`] — a tap on every serial line (`serial_line::line_note`): it counts lines and remembers the
//!   line number of the last one that carried the `tests pwwire` fixture password;
//! * [`pwwire_fixture`] — `tests pwwire`: the KAT, the tracer on a fixture line, the key-echo rule over the
//!   same line typed byte by byte, then the wire's last 2000 lines must not hold the fixture password.
use alloc::borrow::Cow;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// The console line typed so far has reached its secret (published by [`note_line`]).
static SECRET_LINE: AtomicBool = AtomicBool::new(false);

/// The line editor's hook: called after every edit of the console's input line (and with `""` when the line
/// is submitted or cleared). One table lookup on a short string.
pub fn note_line(line: &str) {
    SECRET_LINE.store(midden_core::secret_from(line).is_some(), Relaxed);
}

/// The console line being typed is in its secret part: the next typed byte must not be echoed raw.
pub fn secret_line() -> bool {
    SECRET_LINE.load(Relaxed)
}

/// LOGIN13's `users::secret_input()` (the login screen, an `adduser` password prompt), as last read on the main
/// loop by [`refresh`]. Cached because the drivers' key-edge lines (EHCI, xHCI) run on the USB service path,
/// where taking the login form's lock could spin against a holder on the same CPU.
static SCREEN_SECRET: AtomicBool = AtomicBool::new(false);

/// Re-read LOGIN13's secret state into the cache. Main-loop callers only (it takes the users/login locks):
/// the storage passes (beside `users::service`), the line editor after every dispatch, and the main loop's
/// own raw-key witness ([`withhold_fresh`]).
pub fn refresh() {
    #[cfg(feature = "login")]
    SCREEN_SECRET.store(crate::fs::users::secret_input(), Relaxed);
}

/// What every raw-key echo asks, LOCK-FREE: a console line in its secret part, or the login screen / an
/// `adduser` prompt as last refreshed. The drivers' key-edge lines call this.
pub fn withhold() -> bool {
    secret_line() || SCREEN_SECRET.load(Relaxed)
}

/// [`withhold`] after a [`refresh`] — for the main-loop echoes that already read `secret_input()` directly.
pub fn withhold_fresh() -> bool {
    refresh();
    withhold()
}

/// The per-key traces' vocabulary (CONSOLEFIX M3, GLASSLAG's finding: `[quarry] key_route key=0x..` printed
/// 40+ raw codes on flight 22 — under a password field that IS the password). A per-key line names the CLASS
/// of the key, never its value or scancode: EHCI/xHCI `KEY:`/`KEYUP`, `[hidkeys]`, `USB-DEBUG`, `[serialdoor]`,
/// `[quarry] key_route`, `[keystat]`, `KEYREPEAT-X86`, the tegra `JD2`/`JB2b` markers.
pub const KEY_CLASSES: &[&str] = &["printable", "enter", "backspace", "tab", "esc", "control", "nav"];

/// The class of a key byte (see [`KEY_CLASSES`]).
pub fn key_class(c: u8) -> &'static str {
    match c {
        b'\r' | b'\n' => "enter",
        8 | 0x7f => "backspace",
        b'\t' => "tab",
        0x1b => "esc",
        0x20..=0x7e => "printable",
        0x80..=0xff => "nav",
        _ => "control",
    }
}

/// `line` as the wire may carry it (trimmed; `holocron init ***` when secret-shaped).
pub fn shown(line: &str) -> Cow<'_, str> {
    let t = line.trim();
    match midden_core::redact(t) {
        Some(r) => Cow::Owned(r),
        None => Cow::Borrowed(t),
    }
}

/// THE command tracer: `:: [midden] cmd="<line, redacted>" -> <tail> ::`. Every `[midden] cmd=` arm in
/// `shell.rs` calls this, and `tests pwwire` calls it on its fixture line, so the fixture proves the printer.
pub fn trace(line: &str, tail: fmt::Arguments) {
    serial_println!(":: [midden] cmd=\"{}\" -> {} ::", shown(line), tail);
}

// ---- the wire tap -------------------------------------------------------------------------------------------

/// The `tests pwwire` fixture password. It is typed into the tracer and the key-echo rule and must never be
/// printed; the tap below remembers any wire line that carries it.
const FIXTURE_PW: &[u8] = b"pwwire-Fx7q-k3y";
/// The window `tests pwwire` reads back: the last this-many wire lines.
const WINDOW: u64 = 2000;
static WIRE_LINES: AtomicU64 = AtomicU64::new(0);
/// `WIRE_LINES` at the last line that carried [`FIXTURE_PW`] (`0` = never).
static LAST_HIT: AtomicU64 = AtomicU64::new(0);

/// Called for every serial emit (`serial_line::line_note`): counts lines, and scans for the fixture password.
/// A first-byte prefilter over a line of at most `LINE_MAX` bytes; lock-free, never allocates.
pub fn wire_note(b: &[u8]) {
    if b.last() == Some(&b'\n') {
        WIRE_LINES.fetch_add(1, Relaxed);
    }
    let n = FIXTURE_PW.len();
    if b.len() < n {
        return;
    }
    let first = FIXTURE_PW[0];
    for i in 0..=b.len() - n {
        if b[i] == first && &b[i..i + n] == FIXTURE_PW {
            LAST_HIT.store(WIRE_LINES.load(Relaxed).max(1), Relaxed);
            return;
        }
    }
}

/// Register `tests pwwire` and (where the screen is built) `tests notice`. Called once from `tests::ensure_*`.
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    crate::tests::register("pwwire", pwwire_fixture);
    #[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    crate::tests::register("notice", crate::video::crystal::login::notice_typing_fixture);
}

/// `tests pwwire` (CONSOLEFIX M3): (1) the shared KAT (`midden_core::redact_kat`); (2) the tracer prints the
/// fixture line `holocron init <FIXTURE_PW> peter` — it must come out `holocron init ***`; (3) the same line typed
/// byte by byte through the line editor's hook: every byte after `holocron init ` must be withheld from the raw
/// key echoes, and none before it; (4) the wire's last [`WINDOW`] lines must not carry the fixture password.
pub fn pwwire_fixture() {
    let (kat_ok, kat_n) = midden_core::redact_kat();
    let pw = core::str::from_utf8(FIXTURE_PW).unwrap_or("?");
    let line = alloc::format!("holocron init {} peter", pw);
    let shown_ok = shown(&line) == "holocron init ***";
    trace(&line, format_args!("Exec holocron.elf (pwwire fixture, not run)"));
    // Type it: the hook runs after each byte lands in the line, and the echo for byte k is asked BEFORE byte k
    // lands (the drivers print a key before the console sees it), so byte k is withheld iff line[..k] is secret.
    let was = secret_line();
    let secret_at = midden_core::secret_from(&line).unwrap_or(line.len());
    let (mut withheld, mut leaked, mut early) = (0usize, 0usize, 0usize);
    note_line("");
    for k in 0..line.len() {
        let w = withhold();
        if k >= secret_at {
            if w { withheld += 1 } else { leaked += 1 }
        } else if secret_line() {
            early += 1;
        }
        note_line(&line[..k + 1]);
    }
    note_line("");
    let after_submit = !secret_line();
    SECRET_LINE.store(was, Relaxed);
    let secret_n = line.len() - secret_at;
    let lines = WIRE_LINES.load(Relaxed);
    let hit = LAST_HIT.load(Relaxed);
    let hits = (hit != 0 && lines.saturating_sub(hit) < WINDOW) as u32;
    // The per-key sweep: every byte of the fixture line, through the per-key traces' one formatter, is a class
    // word (no value, no glyph); `[quarry] key_route` and the edge lines print nothing while `withhold()` (above).
    let classes_ok = line.bytes().all(|b| KEY_CLASSES.contains(&key_class(b))) && key_class(b'q') == "printable" && key_class(b'\r') == "enter";
    let ok = classes_ok && kat_ok == kat_n && shown_ok && leaked == 0 && early == 0 && withheld == secret_n && after_submit && hits == 0;
    serial_println!(
        ":: PWWIRE: kat={}/{} trace={} keytrace={} keys_withheld={}/{} lines={} window={} hits={} -> {} ::",
        kat_ok, kat_n, if shown_ok { "redacted" } else { "LEAKED" }, if classes_ok { "class" } else { "VALUE" }, withheld, secret_n, lines, WINDOW, hits, if ok { "PASS" } else { "FAIL" }
    );
}
