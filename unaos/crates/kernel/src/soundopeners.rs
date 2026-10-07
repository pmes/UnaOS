// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — shared-core (audio_core::route, the one sound-routing predicate both rings link; B500 SOUNDOPENERS)
//!
//! SOUNDOPENERS (rmbp-ledger B500) — flight 27 (image 20): `tests play flac|mp3`, `tests playwav` and `tests hda`
//! answered `ran=0`. Nothing retired them (R90): their eight registrations are one line inside `u8x_launcher`
//! (`arch/x86_64/syscall.rs`), the tail of the U7x→U8x demo chain, and the metal boot never reached that chain
//! (no `:: U7x`/`:: U8x` line on the wire). They are registered here, behind the `tests` verb (R80: nothing at
//! boot); `tests::register` refuses a duplicate name, so the demo chain's line is harmless where it does run.
//!
//! `tests soundopeners` — the seven sound samples in system/test-f: each typed, its opener read (the Player since
//! VPLAYAUDIO B475), and OPENED by audio_core on a task with the decoder's own stack (`hda::play::OPEN_STACK`),
//! never the shell's. The route is `audio_core::route` (`audiocore`: the file's own reader; `demux`: MP4/Matroska).
//!
//! `[soundopeners] TEST.<EXT> type=<t> opener=<id> handler=<h> via=<audiocore|demux> dec=<codec> rate=<hz> ch=<n> -> opened`
//! `:: SOUNDOPENERS: formats=wav,flac,opus,ogg,mp3,aac,m4a opened=<n>/7 via=<list> -> PASS|FAIL ::`

/// Register the HDA/play fixtures and `tests soundopeners` once.
pub fn ensure() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
    {
        use crate::drivers::hda as h;
        crate::tests::register("hda", h::hda_tone_test_default);
        crate::tests::register("hdaboth", h::hda_tone_test_both);
        crate::tests::register("hda220", h::hda_tone_test_220);
        crate::tests::register("hda880", h::hda_tone_test_880);
        crate::tests::register("hda1", h::hda_tone_test_m0);
        crate::tests::register("hda2", h::hda_tone_test_m1);
        crate::tests::register("playwav", h::play::selftest);
        crate::tests::register("play", h::play::selftest_codecs);
        crate::tests::register("soundopeners", selftest);
    }
}

#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
const FMTS: [(&str, &str); 7] = [("wav", "WAV"), ("flac", "FLAC"), ("opus", "OPUS"), ("ogg", "OGG"), ("mp3", "MP3"), ("aac", "AAC"), ("m4a", "M4A")];

#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
static RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// `tests soundopeners`: spawn the opener task and return (the verdict is the task's line).
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn selftest() {
    use core::sync::atomic::Ordering;
    if RUNNING.swap(true, Ordering::AcqRel) {
        serial_println!(":: SOUNDOPENERS: already running -> REFUSED ::");
        return;
    }
    let cpu = crate::arch::smp::worker_cpu(0).unwrap_or(crate::arch::sched::CPU_AUTO);
    crate::arch::sched::spawn_stack("sndopen", open_task, 0, cpu, crate::arch::sched::PRIO_NORMAL, crate::drivers::hda::play::OPEN_STACK);
}

#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn open_task(_: usize) {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::sync::atomic::Ordering;
    let mt = crate::shell::vfs_mount_table();
    let (mut opened, mut vias) = (0usize, Vec::new());
    for (_fmt, ext) in FMTS {
        let leaf = alloc::format!("TEST.{}", ext);
        let Some(path) = crate::fs::volumes::testf_find(&mt, &leaf) else {
            serial_println!("[soundopeners] {} -> MISSING (not in system/test-f)", leaf);
            vias.push(String::from("-"));
            continue;
        };
        let (m, _) = crate::fs::filetype::type_of(&path);
        let (op, _) = crate::fs::assoc::opener_for_in(&mt, &path, &m);
        #[cfg(all(feature = "quarry", feature = "wc"))]
        let handler = crate::video::quarry::live::openers::effective(&op, &path);
        #[cfg(not(all(feature = "quarry", feature = "wc")))]
        let handler = op.clone();
        let sound_handler = handler == "player" || handler == "play";
        let head = mt.read(&path, 0, 64).unwrap_or_default();
        let via = audio_core::route(&head).map_or("demux", audio_core::Route::via); // the one predicate (M1)
        match crate::drivers::hda::play::open_facts(&path) {
            Ok(info) => {
                let ok = sound_handler;
                opened += ok as usize;
                vias.push(String::from(via));
                serial_println!(
                    "[soundopeners] {} type={} opener={} handler={} via={} dec={} rate={} ch={} -> {}",
                    leaf, m, op, handler, via, alloc::format!("{:?}", info.codec).to_ascii_lowercase(), info.rate, info.channels,
                    if ok { "opened" } else { "OPENED-BUT-NO-SOUND-HANDLER" }
                );
            }
            Err(e) => {
                vias.push(String::from("refused"));
                serial_println!("[soundopeners] {} type={} opener={} handler={} -> REFUSED why={}", leaf, m, op, handler, e);
            }
        }
    }
    let names: Vec<&str> = FMTS.iter().map(|f| f.0).collect();
    serial_println!(
        ":: SOUNDOPENERS: formats={} opened={}/{} via={} -> {} ::",
        names.join(","), opened, FMTS.len(), vias.join(","), if opened == FMTS.len() { "PASS" } else { "FAIL" }
    );
    RUNNING.store(false, Ordering::Release);
}
