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
//! Wire: `[serialdoor] key=… -> shell(window|headless)` (syscall.rs `wc_route_event`), `[serialdoor] line verb=<v>
//! -> shell(headless) on=<task|render>`, then `[midden]`; `tests door` → `:: DOOR: headless=1 ran=1 window=0 -> PASS ::`.

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub use imp::{ensure_tests, key, note_key, route_word, service, take_door};

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod imp {
    use crate::console::Console;
    use crate::pal::TargetPal;
    use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    use core::sync::atomic::{AtomicBool, AtomicU32};

    /// The render pass's last word on the shell window (`shell_id != WIN_NONE`).
    static SHELL_WIN: AtomicBool = AtomicBool::new(false);
    /// The key `wc_route_event` just routed was the door's (`ftdirx::claim_origin`).
    static DOOR_KEY: AtomicBool = AtomicBool::new(false);
    /// The door's headless console (built on the first headless byte).
    static HEAD: crate::sync::Mutex<Option<Console>> = crate::sync::Mutex::new(None);
    /// Lines the headless door submitted; the last one went to the shell task.
    static LINES: AtomicU32 = AtomicU32::new(0);
    static LAST_TASK: AtomicBool = AtomicBool::new(false);

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
        if SHELL_WIN.load(Acquire) { "window" } else { "headless" }
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
}

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
