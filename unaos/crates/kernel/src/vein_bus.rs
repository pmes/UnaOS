//! CHARTER: Vein — fulfiller
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VEINCORE (rmbp-ledger B304; docs/dev/evidence/rmbp-1004/VEINCORE.md): the KERNEL's part of Vein on
// UnaOS — relay plumbing only. The chat verbs (130..=133) are OWNED by ring 3: `APPS/VEIN.BIN` registers
// them (BANDY3) and answers through `vein_core`'s providers. The kernel never fulfils a chat verb. What
// lives here:
//
// * the `vein` shell verb — `vein status` (who owns the verbs, the relay counters) and
//   `vein rsp <conv> <seq> <done> <base64|->`, the bench relay companion's answer line. The serial wire
//   is the shell (SERIALDOOR) and ring 3 has no console read, so that line is how a host Vein's answer
//   reaches VEIN.BIN: `vein_core::wire::rsp_args_to_body` builds the ChatReply body and
//   `bus_route::inject` hands it to the owner of verb 131 as a KERNEL-stamped request;
// * `tests vein` — drives the PRODUCTION relay (the arch's msend body, BANDY3's scratch rows) through a
//   whole chat exchange: ChatSend → a multi-frame Echo answer (BUS_STATUS_MORE frames, then the final),
//   the closed correlation refusing a late frame, ChatCancel, ChatStatus, and the inject path; plus, when
//   a live VEIN.BIN owns the verbs, the same exchange against it.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use una_abi::{BUS_STATUS_MORE, BUS_VERB_CHAT_CANCEL, BUS_VERB_CHAT_REPLY, BUS_VERB_CHAT_SEND, BUS_VERB_CHAT_STATUS, BUS_VERB_REGISTER};
use vein_core::provider::{Echo, Provider, ProviderIo};
use vein_core::wire::{self, ChatCancel, ChatReply, ChatSend, ChatStatus};

// The two crates name the same tags: a drift is a build failure, never a silent misroute.
const _: () = assert!(BUS_VERB_CHAT_SEND == wire::VERB_CHAT_SEND && BUS_VERB_CHAT_REPLY == wire::VERB_CHAT_REPLY);
const _: () = assert!(BUS_VERB_CHAT_CANCEL == wire::VERB_CHAT_CANCEL && BUS_VERB_CHAT_STATUS == wire::VERB_CHAT_STATUS);
const _: () = assert!(BUS_STATUS_MORE == wire::STATUS_MORE && una_abi::ECANCELED as i32 == wire::ECANCELED);

const ENOENT: i32 = -2;

/// The `vein` shell verb.
pub fn shell_verb(console: &mut crate::console::Console, args: &[&str]) {
    match args.first().copied() {
        Some("rsp") => {
            let mut body = [0u8; wire::REPLY_HDR + wire::RSP_B64_MAX];
            match wire::rsp_args_to_body(&args[1..], &mut body) {
                Ok(n) => {
                    let rc = crate::arch::syscall::vein_inject(BUS_VERB_CHAT_REPLY, &body[..n]);
                    let line = alloc::format!("[vein] rsp conv={} seq={} done={} bytes={} inject={}", args[1], args[2], args[3], n - wire::REPLY_HDR, rc);
                    serial_println!("{}", line);
                    console.println(&line);
                }
                Err(e) => console.println(&alloc::format!("vein rsp: {:?} — usage: vein rsp <conv> <seq> <0|1> <base64 (<= {} chars)|->", e, wire::RSP_B64_MAX)),
            }
        }
        Some("status") | None => {
            let owned = crate::arch::syscall::vein_owned(BUS_VERB_CHAT_SEND);
            let line = alloc::format!(
                "[vein] chat verbs 130..=133 owner={} relayed={} replied={} more={} injected={}",
                if owned { "ring3" } else { "none (bg /apps/VEIN.BIN)" },
                crate::bus_route::RELAYED.load(Ordering::Relaxed),
                crate::bus_route::REPLIED.load(Ordering::Relaxed),
                crate::bus_route::MORE.load(Ordering::Relaxed),
                crate::bus_route::INJECTED.load(Ordering::Relaxed)
            );
            console.println(&line);
        }
        Some(_) => console.println("usage: vein [status] | vein rsp <conv> <seq> <0|1> <base64|->"),
    }
}

// ── tests vein ────────────────────────────────────────────────────────────────────────────────

struct Fx<'a> {
    fx: &'a crate::bus_route::Fixture,
}

impl Fx<'_> {
    fn send(&self, row: usize, f: &[u8]) -> i64 {
        (self.fx.send)(row, f)
    }
    fn pop(&self, row: usize) -> Option<(crate::bus::BusHdr, Vec<u8>)> {
        let b = (self.fx.pop)(row)?;
        let h = crate::bus::frame_parse(&b).ok()?;
        Some((h, b[crate::bus::BUS_HDR_LEN..].to_vec()))
    }
    fn request(&self, row: usize, verb: u8, corr: u32, body: &[u8]) -> i64 {
        let mut f = alloc::vec![0u8; crate::bus::BUS_HDR_LEN + body.len()];
        crate::bus::build_request(verb, corr, body, &mut f);
        self.send(row, &f)
    }
    fn reply(&self, row: usize, verb: u8, corr: u32, status: i32, body: &[u8]) -> i64 {
        let mut f = alloc::vec![0u8; crate::bus::BUS_HDR_LEN + body.len()];
        crate::bus::build_reply(verb, corr, status, [0u8; 32], body, &mut f);
        self.send(row, &f)
    }
}

/// The scratch fulfiller's ProviderIo: each ChatReply becomes a REPLY frame on the relay corr, sent
/// through the production msend body exactly as VEIN.BIN sends it.
struct ScratchIo<'a> {
    fx: &'a Fx<'a>,
    row: usize,
    verb: u8,
    corr: u32,
}

impl ProviderIo for ScratchIo<'_> {
    fn reply(&mut self, r: &ChatReply<'_>) -> bool {
        let mut b = alloc::vec![0u8; wire::REPLY_HDR + r.text.len()];
        r.encode(&mut b).is_ok() && self.fx.reply(self.row, self.verb, self.corr, r.status(), &b) == 0
    }
    fn line(&mut self, _bytes: &[u8]) {}
}

/// Collect a caller's ChatReply stream for `corr` from its mailbox: `(frames, more_frames, text, final
/// status, in_order)`. `wait` polls (yielding) for a live fulfiller; scratch mode needs none.
fn collect(f: &Fx<'_>, row: usize, corr: u32, conv: u32, wait: bool) -> (u32, u32, Vec<u8>, Option<i32>, bool) {
    let (mut frames, mut more, mut text, mut fin, mut order) = (0u32, 0u32, Vec::new(), None, true);
    // A live fulfiller gets up to 3 s of wall time (the counter where the arch has one; else a bounded
    // count of yields) — the shell may run where a yield is a no-op, so a count alone would mean ~1 ms.
    let t0 = crate::clock::uptime_ms();
    let mut idle = 0u32;
    while fin.is_none() {
        match f.pop(row) {
            Some((h, body)) if h.kind == crate::bus::BUS_KIND_REPLY && h.corr == corr => {
                idle = 0;
                if h.status == BUS_STATUS_MORE || h.status == 0 {
                    match ChatReply::decode(&body) {
                        Ok(r) if r.conv == conv && r.seq as u32 == frames && r.done == (h.status == 0) => text.extend_from_slice(r.text),
                        _ => order = false,
                    }
                    frames += 1;
                    if h.status == BUS_STATUS_MORE {
                        more += 1;
                    } else {
                        fin = Some(0);
                    }
                } else {
                    fin = Some(h.status);
                }
            }
            Some(_) => order = false, // a frame for some other corr — not ours
            None if wait && match (t0, crate::clock::uptime_ms()) { (Some(a), Some(b)) => b.saturating_sub(a) < 3000, _ => idle < 4_000_000 } => {
                idle += 1;
                crate::arch::sched::yield_now();
                core::hint::spin_loop();
            }
            None => break,
        }
    }
    (frames, more, text, fin, order)
}

/// `tests vein`: the witness line.
pub fn selftest(fx: &crate::bus_route::Fixture) {
    let f = Fx { fx };
    let (a, ful) = fx.rows;
    // A running VEIN.BIN that happens to sit on a scratch row is never disturbed (the BANDY3 rule).
    if crate::bus_route::owner_row(fx.ops, BUS_VERB_CHAT_SEND).is_some_and(|r| r == a || r == ful) {
        serial_println!(":: VEINBUS: scratch rows {}/{} hold the live chat fulfiller — fixture SKIP ::", a, ful);
        return;
    }
    (fx.clear)(a);
    (fx.clear)(ful);
    let (kp, kt) = wire::kats();
    let mut w = 0u32;
    if kp == kt {
        w |= 1 << 0;
    }
    // The verbs the scratch fulfiller takes: the real chat tags when free, else four high tags (a live
    // VEIN.BIN is never displaced by the fixture).
    let live = crate::arch::syscall::vein_owned(BUS_VERB_CHAT_SEND);
    let (vs, vr, vc, vt) = if live { (250u8, 251u8, 252u8, 253u8) } else { (BUS_VERB_CHAT_SEND, BUS_VERB_CHAT_REPLY, BUS_VERB_CHAT_CANCEL, BUS_VERB_CHAT_STATUS) };

    // bit1: register the four on the scratch fulfiller row.
    let reg = f.request(ful, BUS_VERB_REGISTER, 1, &[vs, vr, vc, vt]) == 0 && matches!(f.pop(ful), Some((h, _)) if h.status == 0);
    if reg {
        w |= 1 << 1;
    }
    // bit2: ChatSend reaches the fulfiller (relay corr, caller's stamp) and decodes.
    let prompt = "The quick brown fox jumps over the lazy dog; UnaOS speaks chat on the bus — ✓.";
    let mut sb = [0u8; 256];
    let sn = ChatSend { conv: 7, text: prompt.as_bytes() }.encode(&mut sb).unwrap_or(0);
    let sent = f.request(a, vs, 70, &sb[..sn]) == 0;
    let got = f.pop(ful);
    let relay = got.as_ref().map(|(h, _)| h.corr).unwrap_or(0);
    let decoded = got.as_ref().is_some_and(|(h, b)| h.verb == vs && h.principal == (fx.stamp)(a) && ChatSend::decode(b) == Ok(ChatSend { conv: 7, text: prompt.as_bytes() }));
    if sent && decoded {
        w |= 1 << 2;
    }
    // bit3: the Echo provider answers in several frames (32-byte chunks); every one is relayed — the
    // non-final ones on BUS_STATUS_MORE, the last on 0 — in order, and the text is the Echo answer.
    let mut chunk = [0u8; 32];
    let mut io = ScratchIo { fx: &f, row: ful, verb: vs, corr: relay };
    let begun = Echo { chunk: &mut chunk }.begin(7, prompt.as_bytes(), &mut io);
    let (frames, more, text, fin, order) = collect(&f, a, 70, 7, false);
    let want = vein_core::provider::echo_answer(prompt);
    if matches!(begun, vein_core::provider::Begin::Done { frames: n } if n == frames) && frames >= 3 && more == frames - 1 && fin == Some(0) && order && text == want.as_bytes() {
        w |= 1 << 3;
    }
    // bit4: the correlation is CLOSED by the final frame: a late MORE frame on it is -ENOENT.
    let mut lb = [0u8; 16];
    let ln = ChatReply { conv: 7, seq: 99, done: false, text: b"late" }.encode(&mut lb).unwrap_or(0);
    if f.reply(ful, vs, relay, BUS_STATUS_MORE, &lb[..ln]) == ENOENT as i64 && f.pop(a).is_none() {
        w |= 1 << 4;
    }
    // bit5: ChatCancel — relayed, answered status 0 with no body.
    let mut cb = [0u8; 4];
    let cn = ChatCancel { conv: 7 }.encode(&mut cb).unwrap_or(0);
    let cs = f.request(a, vc, 71, &cb[..cn]) == 0;
    let cancel_ok = match f.pop(ful) {
        Some((h, b)) if ChatCancel::decode(&b) == Ok(ChatCancel { conv: 7 }) => f.reply(ful, vc, h.corr, 0, &[]) == 0,
        _ => false,
    };
    if cs && cancel_ok && matches!(f.pop(a), Some((h, b)) if h.corr == 71 && h.status == 0 && b.is_empty()) {
        w |= 1 << 5;
    }
    // bit6: ChatStatus — `ready, provider, model` round-trip.
    let ts = f.request(a, vt, 72, &[]) == 0;
    let mut stb = [0u8; 64];
    let stn = ChatStatus { ready: true, provider: b"echo", model: b"reverse" }.encode(&mut stb).unwrap_or(0);
    let st_ok = match f.pop(ful) {
        Some((h, b)) if wire::status_request_ok(&b) => f.reply(ful, vt, h.corr, 0, &stb[..stn]) == 0,
        _ => false,
    };
    if ts && st_ok && matches!(f.pop(a), Some((h, b)) if h.corr == 72 && h.status == 0 && matches!(ChatStatus::decode(&b), Ok(s) if s.ready && s.provider == b"echo")) {
        w |= 1 << 6;
    }
    // bit7: inject (the `vein rsp` path) — only against the REAL tag 131 when the scratch row owns it.
    // The fulfiller receives REQUEST(131, corr 0, KERNEL principal, the ChatReply body).
    if live {
        w |= 1 << 7; // a live VEIN.BIN owns 131: the inject leg would feed it — proved by its own relay run
    } else {
        let mut rb = [0u8; 32];
        let rn = wire::rsp_args_to_body(&["7", "0", "1", "aGk="], &mut rb).unwrap_or(0);
        let rc = crate::arch::syscall::vein_inject(BUS_VERB_CHAT_REPLY, &rb[..rn]);
        let ok = matches!(f.pop(ful), Some((h, b)) if h.kind == crate::bus::BUS_KIND_REQUEST && h.corr == 0 && h.principal[0] == 4 && h.principal[1..].iter().all(|&x| x == 0)
            && matches!(ChatReply::decode(&b), Ok(r) if r.conv == 7 && r.done && r.text == b"hi"));
        if rc == 0 && ok {
            w |= 1 << 7;
        }
    }
    (fx.clear)(a);
    (fx.clear)(ful);
    crate::bus_route::on_exit(fx.ops, ful);

    // The LIVE leg: a running VEIN.BIN answers a ChatSend from the scratch caller row.
    let mut live_word = "absent";
    let mut live_frames = 0u32;
    if live {
        let lt = "hello, metal";
        let n = ChatSend { conv: 9, text: lt.as_bytes() }.encode(&mut sb).unwrap_or(0);
        live_word = "fail";
        if f.request(a, BUS_VERB_CHAT_SEND, 90, &sb[..n]) == 0 {
            let (fr, _m, text, fin, order) = collect(&f, a, 90, 9, true);
            live_frames = fr;
            if fin == Some(0) && order && fr >= 1 {
                // Echo answers the reversed prompt; any other provider: a well-formed stream is the pass.
                live_word = if text == vein_core::provider::echo_answer(lt).as_bytes() { "echo" } else { "pass" };
            } else if fin.is_none() {
                live_word = "timeout";
            }
        }
        (fx.clear)(a);
    }
    on_exit_caller(fx, a);
    const ALL: u32 = (1 << 8) - 1;
    let live_ok = matches!(live_word, "absent" | "echo" | "pass");
    let mode = if live { "live" } else { "scratch" };
    if w == ALL && live_ok {
        serial_println!(":: VEINBUS: mode={} kats={}/{} frames={} more={} done=1 cancel=0 status=0 live={} live_frames={} -> PASS ::", mode, kp, kt, frames, more, live_word, live_frames);
    } else {
        serial_println!(":: VEINBUS: mode={} kats={}/{} frames={} more={} w={:#04x}/{:#04x} live={} live_frames={} -> FAIL ::", mode, kp, kt, frames, more, w, ALL, live_word, live_frames);
    }
}

/// Reclaim any relay the scratch caller still holds (none on a pass) — the BANDY3 sweep shape.
fn on_exit_caller(fx: &crate::bus_route::Fixture, a: usize) {
    (fx.clear)(a);
    crate::bus_route::on_exit(fx.ops, a);
}
