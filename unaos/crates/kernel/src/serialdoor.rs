//! CHARTER: Kernel — kernel-by-ruling (B487 DOORHEADLESS: the serial door's own console on SHELLTASK's job task — R88 the bare desktop, B458 the seam)
//!
//! DOORHEADLESS (rmbp-ledger B487). Flight 26 boot 2: the bare desktop (R88) had no shell window, and every byte the
//! serial door claimed (`[serialdoor] key=… -> shell`) fell off `x86_render_service`'s key arm, which hands a key to
//! the shell WINDOW or drops it — zero `[midden]`. Design: docs/dev/evidence/rmbp-1005/doorheadless.md.
//!
//! The door owns ONE headless [`Console`] — not a second shell: its lines go through `shelltask::submit` exactly as the
//! window's Enter does (a routed verb on the `shell-job` task, the rest inline on the render task), and its output
//! sink is the wire, so the transcript the job writes (`shelltask::out` → [`service`] → `place_from_task`) and every
//! inline `println` land on the serial console the door is. A shell window, when one exists, keeps the door's lines.
//!
//! Wire: `[serialdoor] key=… -> shell(window|headless|backdrop-console)` (syscall.rs `wc_route_event`), `[serialdoor] line verb=<v>
//! -> shell(headless) on=<task|render>`, then `[midden]`; `tests door` → `:: DOOR: headless=1 ran=1 window=0 -> PASS ::`.
//!
//! DOORSPLIT (rmbp-ledger B634). A door LINE has one consumer. The route is latched on the line's first byte
//! ([`to_window`], from the render key arm's live shell window) and cleared on CR/LF/^C; a window-held line whose
//! window is gone moves its partial input to the headless console ([`adopt`]) and completes there; a headless-held
//! line stays headless when a window opens. Wire: `[serialdoor] line route=window|headless at=first-byte win=<id>:<gen>
//! -> held` once per line, `[serialdoor] adopt from=window bytes=N -> headless`; `tests doorsplit` →
//! `:: DOORSPLIT: held=ok adopt=ok reverse=ok -> PASS ::`.

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub use imp::{adopt, ensure_tests, key, note_key, route_word, service, take_door, to_window, window_saw};

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod imp {
    use crate::console::Console;
    use crate::pal::TargetPal;
    use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};
    use alloc::string::String;

    /// The render pass's last word on the shell window (`shell_id != WIN_NONE`).
    static SHELL_WIN: AtomicBool = AtomicBool::new(false);
    /// The key `wc_route_event` just routed was the door's (`ftdirx::claim_origin`).
    static DOOR_KEY: AtomicBool = AtomicBool::new(false);
    /// The door's headless console (built on the first headless byte).
    static HEAD: crate::sync::Mutex<Option<Console>> = crate::sync::Mutex::new(None);
    /// Lines the headless door submitted; the last one went to the shell task.
    static LINES: AtomicU32 = AtomicU32::new(0);
    static LAST_TASK: AtomicBool = AtomicBool::new(false);
    /// DOORSPLIT: the open door line's route, latched on its first byte (0 idle, 1 window, 2 headless).
    static LINE_AT: AtomicU8 = AtomicU8::new(0);
    /// DOORSPLIT: the (id << 32 | gen) of the window a window-held line was latched on.
    static LINE_WIN: AtomicU64 = AtomicU64::new(0);
    /// DOORSPLIT: the held window's input line after its last key (`window_saw`) — what `adopt` moves.
    static MIRROR: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new());

    /// `wc_route_event`, every key: was it the door's?
    pub fn note_key(door: bool) {
        DOOR_KEY.store(door, Relaxed);
    }
    /// `x86_render_service`'s key arm, no shell window: the key is the door's (consumed once).
    pub fn take_door() -> bool {
        DOOR_KEY.swap(false, Relaxed)
    }
    /// The `[serialdoor] key=` witness's route word.
    pub fn route_word() -> &'static str {
        if !crate::video::desktop_uefi::is_active() { "backdrop-console" } else if LINE_AT.load(Acquire) == 1 { "window" } else if LINE_AT.load(Acquire) == 2 { "headless" } else if SHELL_WIN.load(Acquire) { "window" } else { "headless" } // BOOTVERDICTS2 (rmbp-ledger B616, B567's wording item): with no takeover the render key arm (main.rs `desktop_owns_backdrop()` false) hands the byte to the BACKDROP console's `handle_key`, never `key` below — the same latch `desktop_owns_backdrop` reads
    }

    /// DOORSPLIT — the render key arm (`main.rs`), every key on the crispy desktop: does this key go to the shell
    /// WINDOW? `id`/`wgen` name the live shell window (`id == 0`: none); `partial` is that window's input line. A
    /// keyboard key answers as before (the window if one exists). A door key goes where its line is held: a line
    /// held for a window that is gone (closed, or replaced by a new generation) is adopted into the headless
    /// console first, and the key then follows it there (`take_door` + `key`).
    pub fn to_window(c: u8, id: u32, wgen: u32, partial: &mut String) -> bool {
        steer(c, DOOR_KEY.load(Relaxed), id, wgen, partial)
    }

    /// `to_window` with the door bit passed in: the one entry `tests doorsplit` drives.
    fn steer(c: u8, door: bool, id: u32, wgen: u32, partial: &mut String) -> bool {
        let win = if id != 0 { ((id as u64) << 32) | wgen as u64 } else { 0 };
        if !door {
            return win != 0;
        }
        let mut at = LINE_AT.load(Acquire);
        if at == 0 {
            at = if win != 0 { 1 } else { 2 };
            LINE_WIN.store(win, Release);
            LINE_AT.store(at, Release);
            if at == 1 {
                let mut m = MIRROR.lock();
                m.clear();
                m.push_str(partial);
            }
            serial_println!("[serialdoor] line route={} at=first-byte win={}:{} -> held", if at == 1 { "window" } else { "headless" }, id, wgen);
        } else if at == 1 && (win == 0 || win != LINE_WIN.load(Acquire)) {
            let moved = core::mem::take(&mut *MIRROR.lock());
            if win == 0 {
                partial.clear(); // the gone window's console (torn down already, or about to be)
            }
            adopt(&moved);
            at = 2;
            LINE_AT.store(2, Release);
        }
        if matches!(c, b'\r' | b'\n' | 3) {
            LINE_AT.store(0, Release);
            LINE_WIN.store(0, Release);
            MIRROR.lock().clear();
        }
        at == 1
    }

    /// DOORSPLIT — the render key arm, after the window's `handle_key`: the window's input line as it now stands
    /// (only while a door line is held for that window; one clone of a short line per key).
    pub fn window_saw(line: &str) {
        if LINE_AT.load(Acquire) == 1 {
            let mut m = MIRROR.lock();
            m.clear();
            m.push_str(line);
        }
    }

    /// DOORSPLIT — a door line begun in a shell window that is gone: its partial input becomes the headless
    /// console's line, so the rest of the line (and its Enter) completes in ONE place.
    pub fn adopt(partial: &str) {
        serial_println!("[serialdoor] adopt from=window bytes={} -> headless", partial.len());
        let mut g = HEAD.lock();
        let con = g.get_or_insert_with(console);
        con.current_input.clear();
        con.current_input.push_str(partial);
        crate::pwwire::note_line(&con.current_input);
    }

    /// The headless console's sink: the wire.
    fn wire(text: &str) {
        serial_println!("{}", text);
    }

    fn console() -> Console {
        let mut c = Console::new();
        c.mark_in_window(); // the shell task takes lines only from a windowed console; this one is the door's window
        c.set_output_sink(wire);
        c
    }

    /// A throwaway 16x16 pal (the `shell-job` task's shape): the headless shell has no glass.
    fn scratch<R>(f: impl FnOnce(&mut TargetPal) -> R) -> R {
        let info = unaos_boot_info::FrameBufferInfo {
            width: 16, height: 16, stride: 16, bytes_per_pixel: 4,
            pixel_format: unaos_boot_info::PixelFormat::Bgr,
        };
        let mut store = alloc::vec![0u8; 16 * 16 * 4];
        let mut fb = crate::video::FrameBuffer::new();
        fb.init(store.as_mut_ptr() as usize, store.len(), info);
        let mut screen = crate::video::Screen::direct(fb);
        let r = {
            let mut pal = TargetPal { surface: &mut screen };
            f(&mut pal)
        };
        drop(store);
        r
    }

    /// A door byte with no shell window (render task): the line editor `main.rs` `handle_key` is, minus the paint.
    pub fn key(c: u8) {
        let mut g = HEAD.lock();
        let con = g.get_or_insert_with(console);
        #[cfg(feature = "login")]
        if crate::fs::users::prompt_key(c, con) != 0 {
            return; // a password prompt took the byte
        }
        match c {
            b'\r' | b'\n' => {
                let line = core::mem::take(&mut con.current_input);
                crate::pwwire::note_line("");
                let v = alloc::string::String::from(line.split_whitespace().next().unwrap_or(""));
                if v.is_empty() {
                    return;
                }
                LINES.fetch_add(1, Relaxed);
                let task = crate::shelltask::routed(&line);
                LAST_TASK.store(task, Release);
                serial_println!("[serialdoor] line verb={} -> shell(headless) on={}", v, if task { "task" } else { "render" });
                if crate::shelltask::submit(&line, con) {
                    return;
                }
                scratch(|pal| {
                    let _ = crate::origin::with(crate::origin::Origin::Door, || crate::shell::dispatch_command(&line, con, pal));
                });
                crate::pwwire::refresh();
                con.drain_output();
            }
            3 => {
                if !crate::shelltask::interrupt(con) {
                    con.current_input.clear();
                    crate::pwwire::note_line("");
                }
            }
            8 | 0x7F => {
                con.current_input.pop();
                crate::pwwire::note_line(&con.current_input);
            }
            32..=126 => {
                con.current_input.push(c as char);
                crate::pwwire::note_line(&con.current_input);
            }
            _ => {}
        }
    }

    /// Every render pass (`x86_render_service`, beside the window's `shelltask::service`): note the shell window;
    /// with none, the shell task's transcript and queue are the door's — drained onto the wire.
    pub fn service(shell_window: bool) {
        SHELL_WIN.store(shell_window, Release);
        if shell_window || !crate::shelltask::pending() {
            return;
        }
        let Some(mut g) = HEAD.try_lock() else { return };
        let con = g.get_or_insert_with(console);
        scratch(|pal| {
            let _ = crate::shelltask::service(con, pal);
        });
    }

    pub fn ensure_tests() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, AcqRel) {
            crate::tests::register("door", selftest);
            crate::tests::register("doorsplit", doorsplit_selftest);
        }
    }

    /// `tests door` (R80: on demand), typed on the wire with no shell window: the line came through the headless
    /// door (`headless=1`), it is running on the shell task (`ran=1`), and no shell window exists (`window=0`).
    fn selftest() {
        let window = SHELL_WIN.load(Acquire);
        let headless = !window && LINES.load(Relaxed) > 0 && LAST_TASK.load(Acquire);
        let tid = crate::sync::here_tid();
        let ran = crate::shelltask::job_tid() == tid && tid != 0;
        if window {
            serial_println!(":: DOOR: headless=0 ran={} window=1 lines={} -> SKIP reason=shell-window (type `tests door` on the wire with no shell window) ::", ran as u8, LINES.load(Relaxed));
            return;
        }
        let ok = headless && ran;
        serial_println!(":: DOOR: headless={} ran={} window=0 lines={} -> {} ::", headless as u8, ran as u8, LINES.load(Relaxed), if ok { "PASS" } else { "FAIL" });
    }

    /// `tests doorsplit` (DOORSPLIT, B634; no serial needed): the line `tests una` + Enter is fed through the door's
    /// own entry (`steer`, the door bit set) with a simulated shell window — a local line standing for the window
    /// console's `current_input`, updated as `handle_key` would and reported through `window_saw` — and the real
    /// headless console (`key`, printable bytes only: the Enter is steered but never submitted). Three cases:
    /// held (window open throughout: the window saw the whole line, headless nothing), adopt (the window closes
    /// after `tes`: the headless console completes `tests una`, the window saw only `tes` and nothing after),
    /// reverse (begun headless, a window opens after `tes`: headless saw it all, the window nothing). The door's
    /// latch, mirror and headless line are saved first and restored after.
    fn doorsplit_selftest() {
        const LINE: &[u8] = b"tests una";
        const SPLIT: usize = 3;
        let saved_at = LINE_AT.swap(0, AcqRel);
        let saved_win = LINE_WIN.swap(0, AcqRel);
        let saved_mirror = core::mem::take(&mut *MIRROR.lock());
        let saved_head = { let mut g = HEAD.lock(); core::mem::take(&mut g.get_or_insert_with(console).current_input) };
        // One case: `open_at(i)` is the simulated window's (id, gen) at byte i (id 0: no window).
        let run = |open_at: &dyn Fn(usize) -> (u32, u32)| -> (String, String, bool) {
            let mut win = String::new();
            let mut win_after_split = false;
            for (i, &c) in LINE.iter().chain(b"\r".iter()).enumerate() {
                let (id, wgen) = open_at(i);
                if steer(c, true, id, wgen, &mut win) {
                    if c != b'\r' { win.push(c as char); }
                    if i >= SPLIT { win_after_split = true; }
                    window_saw(&win);
                } else if c != b'\r' {
                    key(c);
                }
            }
            let head = { let mut g = HEAD.lock(); core::mem::take(&mut g.get_or_insert_with(console).current_input) };
            (win, head, win_after_split)
        };
        let whole = core::str::from_utf8(LINE).unwrap_or("");
        let (w, h, _) = run(&|_| (0x7e, 1));
        let held = w == whole && h.is_empty() && LINE_AT.load(Acquire) == 0;
        let (w2, h2, after) = run(&|i| if i < SPLIT { (0x7e, 1) } else { (0, 0) });
        let adopt = h2 == whole && !after && w2.is_empty() && LINE_AT.load(Acquire) == 0;
        let (w3, h3, _) = run(&|i| if i < SPLIT { (0, 0) } else { (0x7e, 2) });
        let reverse = w3.is_empty() && h3 == whole && LINE_AT.load(Acquire) == 0;
        LINE_AT.store(saved_at, Release);
        LINE_WIN.store(saved_win, Release);
        *MIRROR.lock() = saved_mirror;
        { let mut g = HEAD.lock(); let con = g.get_or_insert_with(console); con.current_input = saved_head; crate::pwwire::note_line(&con.current_input); }
        let w = |ok: bool| if ok { "ok" } else { "FAIL" };
        serial_println!(":: DOORSPLIT: held={} adopt={} reverse={} -> {} ::", w(held), w(adopt), w(reverse), if held && adopt && reverse { "PASS" } else { "FAIL" });
    }
}

/// Every other build: no door line to hold.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn to_window(_c: u8, id: u32, _wgen: u32, _partial: &mut alloc::string::String) -> bool {
    id != 0
}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn window_saw(_line: &str) {}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn adopt(_partial: &str) {}

/// Every other build: no shell window to lack; the door's byte goes where it always went.
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn note_key(_door: bool) {}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn take_door() -> bool {
    false
}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn route_word() -> &'static str {
    "window"
}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn key(_c: u8) {}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn service(_shell_window: bool) {}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn ensure_tests() {}
