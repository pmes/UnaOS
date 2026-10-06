//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! FILETYPE M3 (B307, audit B293) — THE ONE OPENER DISPATCH. Quarry is the Finder by ruling (R50);
//! routing a file to the thing that opens it is the Finder's job, and it is done here and nowhere
//! else.
//!
//! An opener id comes from `fs::assoc::opener_for` (the file's `una:preferred`, the type
//! registry's `una:preferred`, or the first registrant — FILETYPES B423). [`open`] maps it to the request the tree already has:
//!
//! | id | what runs |
//! | :--- | :--- |
//! | `facet` | `video::facet::request_open` (image viewer; `UNAOS_FACET`) |
//! | `fileview` | `video::fileview::request_open` (read-only text viewer) |
//! | `textedit` | `video::textedit::request_open` when the user may edit the file, else the viewer |
//! | `markdown` `json` | `video::fileview::request_open_styled` — the viewer with that renderer (QUARRY2) |
//! | `play` | `drivers::hda::play::request_open` (x86 `UNAOS_HDA` + `UNAOS_HDATONE`) |
//! | `player` | `video::player::request_open` — a video in the Player's picture window (x86 `UNAOS_VIDEO`, VIDEOPLAYER B434) |
//! | `launch` | Quarry's `launch` — `arch::syscall::spawn_user_image_bg`, the seam `bg` takes |
//! | `linux` | refused from the desktop: the Linux ABI runs a foreground session; the line names `linux <path>` |
//! | `/a/PROGRAM.BIN` | launch that program, then hand it the file (below) |
//!
//! A RING-3 PROGRAM CANNOT BE GIVEN AN ARGV: `quarry.md` §7 item 8 — there is no `SYS_EXEC` with an
//! argv. So the file path is delivered as one `BUS_VERB_NOTICE` frame in the spawned program's
//! mailbox (x86: `arch::syscall::bus_notice_to`, the same queue its `SYS_MRECV` drains), and the wire
//! says so. aarch64 launches the program without the path and says that too.
use alloc::string::String;

use super::{Act, JOBS, MAX_JOBS};

/// Is `id` an opener THIS BUILD can run. A type whose opener is not compiled in reads as "no opener
/// for <type> in this build", not as a press that silently does nothing.
pub fn available(id: &str) -> bool {
    match id {
        "launch" | "fileview" | "textedit" => true,
        "markdown" | "json" => true, // QUARRY2 (B336): the text viewer, rendered (`fileview::request_open_styled`)
        "facet" => cfg!(feature = "facet"),
        "play" => cfg!(all(target_arch = "x86_64", feature = "hda-tone")),
        "player" => cfg!(all(target_arch = "x86_64", feature = "videoplayer")), // VIDEOPLAYER (B434): video in the Player (UNAOS_VIDEO)
        "linux" => cfg!(all(target_arch = "x86_64", feature = "linuxabi")),
        p if p.starts_with('/') => true,
        _ => false,
    }
}

/// The handler that will ACTUALLY run for `id` on `path` — `textedit` on a file the user may not
/// edit is the viewer, and the witness names the one that opens. Pure apart from `may_edit`.
pub fn effective(id: &str, path: &str) -> String {
    if !available(id) {
        return String::from("none");
    }
    if id == "textedit" && !crate::video::textedit::may_edit(path) {
        return String::from("fileview");
    }
    String::from(id)
}

/// The routing decision as an [`Act`] (taken inside the model lock; performed by [`open`] outside it).
pub(super) fn act(opener: String, path: String, mime: String) -> Act {
    if opener == "none" || opener.is_empty() {
        Act::NoOpener { path, mime, why: "" }
    } else if !available(&opener) {
        Act::NoOpener { path, mime, why: " in this build" }
    } else {
        Act::Open { opener, path, mime }
    }
}

/// THE dispatch: run opener `id` on `path` (of type `mime`). Returns the path-bar line.
pub fn open(id: &str, path: &str, mime: &str) -> String {
    let leaf = super::leaf(path);
    super::attrcols::queue_open(path); // ATTRCOLUMNS (B402): an opened file's facts are refreshed on the next service pass
    match id {
        "launch" => {
            // Reap first, then test the ceiling (rmbp-7 QUARRY) — moved here unchanged from run_act.
            super::reap_jobs();
            if JOBS.lock().len() >= MAX_JOBS {
                let s = alloc::format!("{} live jobs — kill one first", MAX_JOBS);
                serial_println!("[quarry] launch REFUSED path={} reason=job-table-full ({})", path, s);
                s
            } else {
                let r = super::launch(path);
                super::reap_jobs();
                r
            }
        }
        "textedit" | "fileview" => {
            // TEXTEDIT (R75): a file the user owns opens the EDITOR, any other the read-only viewer.
            if id == "textedit" && crate::video::textedit::may_edit(path) {
                crate::video::textedit::request_open(path);
                serial_println!("[quarry] open TEXT path={} type={} -> textedit (latched for the render pass)", path, mime);
            } else {
                crate::video::fileview::request_open(path);
                serial_println!("[quarry] open TEXT path={} type={} -> fileview (latched for the render pass)", path, mime);
            }
            alloc::format!("opening {}", leaf)
        }
        // QUARRY2 (B336): Markdown and JSON open in the read-only viewer with their renderer
        // (`fileview::richtext`): headings bold and lists indented / pretty-printed and tinted.
        "markdown" | "json" => {
            crate::video::fileview::request_open_styled(path, id);
            serial_println!("[quarry] open TEXT path={} type={} -> fileview render={} (latched for the render pass)", path, mime, id);
            alloc::format!("opening {}", leaf)
        }
        "play" => {
            #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
            {
                crate::video::player::request_open(path); // PLAYER (B419): the window owns the play (was the headless `hda::play::request_open`)
                serial_println!("[quarry] open PLAY path={} type={} -> player (latched for the render pass)", path, mime);
            }
            #[cfg(not(all(target_arch = "x86_64", feature = "hda-tone")))]
            serial_println!("[quarry] open PLAY path={} -> no audio in this build (UNAOS_HDA+UNAOS_HDATONE arm it)", path);
            alloc::format!("playing {}", leaf)
        }
        // VIDEOPLAYER (B434): video opens in the Player's picture window — latched like `play` (the job's I/O runs off the router).
        "player" => {
            #[cfg(all(target_arch = "x86_64", feature = "videoplayer"))]
            {
                crate::video::player::request_open(path);
                serial_println!("[quarry] open VIDEO path={} type={} -> player (latched for the render pass)", path, mime);
                alloc::format!("playing {}", leaf)
            }
            #[cfg(not(all(target_arch = "x86_64", feature = "videoplayer")))]
            {
                serial_println!("[quarry] open VIDEO path={} -> no video player in this build (UNAOS_VIDEO arms it)", path);
                String::from("no video player in this build")
            }
        }
        // FACET — LATCHED, not opened (click-router depth; see `facet::request_open`).
        "facet" => {
            #[cfg(feature = "facet")]
            {
                #[cfg(not(feature = "svg"))]
                if mime == crate::fs::filetype::IMAGE_SVG {
                    serial_println!("[quarry] open VIEW path={} type={} kind=svg handler=none -> no svg renderer in this build (UNAOS_SVG arms it)", path, mime);
                    return String::from("no svg renderer in this build");
                }
                crate::video::facet::request_open(path);
                serial_println!("[quarry] open VIEW path={} type={} kind={} handler=facet -> facet (latched for the render pass)", path, mime, super::kind_token(mime)); // SMALLFIX2 (B391): kind + handler named
                alloc::format!("opening {}", leaf)
            }
            #[cfg(not(feature = "facet"))]
            {
                serial_println!("[quarry] open VIEW path={} -> no image viewer in this build (UNAOS_FACET arms it)", path);
                String::from("no image viewer in this build")
            }
        }
        "linux" => {
            serial_println!(
                "[quarry] open LINUX path={} type={} -> REFUSED reason=foreground-only (the Linux ABI runs one foreground session at the shell: type `linux {}`)",
                path, mime, path
            );
            alloc::format!("Linux programs run at the shell: linux {}", path)
        }
        prog if prog.starts_with('/') => {
            let r = open("launch", prog, crate::fs::filetype::UNAOS_ELF);
            let slot = JOBS.lock().iter().rev().find(|j| j.name == prog).map(|j| j.asid);
            match slot {
                Some(_s) => {
                    #[cfg(target_arch = "x86_64")]
                    {
                        let rc = crate::arch::syscall::bus_notice_to(_s as usize, path.as_bytes());
                        serial_println!(
                            "[quarry] open PROGRAM prog={} file={} type={} -> launched; file delivered as BUS_VERB_NOTICE to slot {} rc={} (no SYS_EXEC argv yet, quarry.md 7 item 8)",
                            prog, path, mime, _s, rc
                        );
                    }
                    #[cfg(not(target_arch = "x86_64"))]
                    serial_println!(
                        "[quarry] open PROGRAM prog={} file={} type={} -> launched WITHOUT the file (no SYS_EXEC argv and no kernel->mailbox notice on this arch yet)",
                        prog, path, mime
                    );
                }
                None => serial_println!("[quarry] open PROGRAM prog={} file={} -> launch refused ({})", prog, path, r),
            }
            r
        }
        other => {
            serial_println!("[quarry] open UNHANDLED path={} type={} opener={} — not an opener this tree knows", path, mime, other);
            alloc::format!("unknown opener {}", other)
        }
    }
}
