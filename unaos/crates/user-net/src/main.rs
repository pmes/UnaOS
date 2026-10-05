#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NETRING3 M3 (rmbp-ledger B306): NET.ELF — a ring-3 network client on the rMBP, the first rung of Vein on
// the metal reaching `https://api.anthropic.com`. VEINTLS (LEDGER SR36): it now runs on the SAME transport
// Lumen does — `vein_ring3` (DNS, blocking TCP, and TLS 1.3 with UnaOS's own tls_core, every server verified
// against /system/trust/roots.pem at SYS_TIME). The embedded-tls compile spike (../tls-spike) is retired.
//
//   1. entropy  — SYS_GETRANDOM(32): the kernel DRBG answers 32 bytes.
//   2. resolve  — SYS_RESOLVE("api.anthropic.com"): the kernel resolver (DHCP-leased DNS).
//   3. connect  — TCP :80 through vein_ring3::net::Tcp (polled connect, bounded blocking I/O).
//   4. http     — `GET / HTTP/1.0` in clear; prints the status line.
//   5. tls      — TCP :443, then a VERIFIED TLS 1.3 handshake (vein_ring3::tls::with_session: trust store,
//                 clock, RFC 6125 name check) and `GET /` inside it; prints the status line and the issuer
//                 the chain verified to. When this boot cannot verify (no crypto provider in the build, no
//                 trust store on the volume, no wall clock) the leg is SKIPPED and says which.
//
// One witness line: `:: NETRING3: rand=<n> resolve=<ip> connect=<0|err> http=<status> tls=<status|skip|fail>
// [verified=<issuer CN>|reason=<why>] -> PASS|SKIP|FAIL ::`. SKIP (never FAIL) when the resolve finds no link
// or no answer: a dark NIC is not a broken program. A TLS verification failure is a FAIL with its reason.
// RING3ABI2 (B333): the host is argv[1] when given (`net example.com`), else api.anthropic.com; the line prints `host= argc=`.

extern crate alloc;

use una_abi::{ENODEV, ENOENT, SYS_EXIT};
use vein_core::client::Transport;
use vein_core::prefs::Verify;
use vein_ring3::net::{resolve, Tcp};
use vein_ring3::sys::{getrandom, sys, write};

/// tls_core allocates; the size-class heap over SYS_SBRK (NET.ELF links at the ELF window).
#[global_allocator]
static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();

const HOST: &str = "api.anthropic.com";
/// The longest host name accepted from argv (DNS's 253, rounded). RING3ABI2 (B333).
const HOST_MAX: usize = 255;

fn exit(code: u64) -> ! {
    sys(SYS_EXIT, code, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// A tiny line builder — no `core::fmt`.
struct Line {
    buf: [u8; 240],
    len: usize,
}
impl Line {
    fn new() -> Self {
        Line { buf: [0; 240], len: 0 }
    }
    fn s(&mut self, b: &[u8]) -> &mut Self {
        for &c in b {
            if self.len < self.buf.len() {
                self.buf[self.len] = c;
                self.len += 1;
            }
        }
        self
    }
    fn u(&mut self, mut v: u64) -> &mut Self {
        let mut t = [0u8; 20];
        let mut i = t.len();
        loop {
            i -= 1;
            t[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&t[i..])
    }
    fn i(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.s(b"-").u(v.unsigned_abs())
        } else {
            self.u(v as u64)
        }
    }
    fn ip(&mut self, ip: &[u8]) -> &mut Self {
        self.u(ip[0] as u64).s(b".").u(ip[1] as u64).s(b".").u(ip[2] as u64).s(b".").u(ip[3] as u64)
    }
    fn print(&mut self) {
        self.s(b"\n");
        write(&self.buf[..self.len]);
    }
}

fn verdict(l: &mut Line, tail: &[u8]) -> ! {
    l.s(b" -> ").s(tail).s(b" ::").print();
    exit(0)
}

/// RING3ABI2 (B333): `<head><host>\r\nUser-Agent: UnaOS-NET/1\r\nConnection: close\r\n\r\n` into `buf`; the slice written.
fn build_req<'a>(buf: &'a mut [u8], head: &[u8], host: &str) -> &'a [u8] {
    let mut n = 0;
    for part in [head, host.as_bytes(), &b"\r\nUser-Agent: UnaOS-NET/1\r\nConnection: close\r\n\r\n"[..]] {
        buf[n..n + part.len()].copy_from_slice(part);
        n += part.len();
    }
    &buf[..n]
}

/// `GET / HTTP/1.x` over `t`; the status code digits, or `None` for no parseable status line.
fn get_status<T: Transport + ?Sized>(t: &mut T, req: &[u8], head: &mut [u8; 256]) -> Result<Option<[u8; 3]>, i64> {
    t.send_all(req)?;
    let mut n = 0usize;
    while n < head.len() && !head[..n].contains(&b'\n') {
        match t.recv(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) => return Err(e),
        }
    }
    let end = head[..n].iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(n);
    let st = &head[..end];
    Line::new().s(b"NET: ").s(st).print();
    if st.len() >= 12 && st.starts_with(b"HTTP/1.") && st[9..12].iter().all(|b| b.is_ascii_digit()) {
        Ok(Some([st[9], st[10], st[11]]))
    } else {
        Ok(None)
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    // 0. argv — RING3ABI2 M2 (B333): the host is argv[1] when given (`net example.com`); merge12 fold onto VEINTLS.
    let args = una_abi::args();
    let argc = args.map(|a| a.argc()).unwrap_or(0);
    let host: &str = match args.and_then(|a| a.get(1)).and_then(|h| core::str::from_utf8(h).ok()) {
        Some(h) if !h.is_empty() && h.len() <= HOST_MAX => h,
        _ => HOST,
    };
    // 1. entropy
    let mut seed = [0u8; 32];
    let mut l = Line::new();
    let got = getrandom(&mut seed);
    l.s(b":: NETRING3: host=").s(host.as_bytes()).s(b" argc=").u(argc as u64).s(b" rand=").i(if got.is_ok() { 32 } else { got.unwrap_err() });
    if got.is_err() || seed == [0u8; 32] {
        l.s(b" resolve=skip connect=skip http=skip tls=skip");
        verdict(&mut l, b"FAIL reason=getrandom");
    }

    // 2. resolve
    l.s(b" resolve=");
    let ip = match resolve(host) {
        Ok(ip) => ip,
        Err(r) => {
            l.s(b"err").i(r).s(b" connect=skip http=skip tls=skip");
            if r == ENODEV {
                verdict(&mut l, b"SKIP reason=no-link");
            }
            if r == ENOENT {
                verdict(&mut l, b"SKIP reason=no-answer");
            }
            verdict(&mut l, b"FAIL reason=resolve");
        }
    };
    l.ip(&ip);

    // 3. connect :80 + 4. http in clear
    let mut head = [0u8; 256];
    let mut reqbuf = [0u8; 64 + HOST_MAX + 64];
    let req: &[u8] = build_req(&mut reqbuf, b"GET / HTTP/1.0\r\nHost: ", host); // RING3ABI2: the argv host rides the request
    match Tcp::connect(ip, 80) {
        Err(c) => {
            l.s(b" connect=").i(c).s(b" http=skip tls=skip");
            verdict(&mut l, b"FAIL reason=connect");
        }
        Ok(mut tcp) => {
            l.s(b" connect=0 http=");
            match get_status(&mut tcp, req, &mut head) {
                Ok(Some(code)) => l.s(&code),
                Ok(None) => {
                    l.s(b"bad tls=skip");
                    verdict(&mut l, b"FAIL reason=status-line");
                }
                Err(e) => {
                    l.s(b"err").i(e).s(b" tls=skip");
                    verdict(&mut l, b"FAIL reason=recv");
                }
            };
        }
    }

    // 5. verified TLS :443
    let setup = vein_ring3::TlsSetup::load();
    let why = match setup.verify() {
        Verify::Ready => None,
        Verify::NoProvider => Some(&b"no-provider"[..]),
        Verify::NoTrustStore => Some(&b"no-trust-store"[..]),
        Verify::NoClock => Some(&b"clock-unset"[..]),
    };
    if let Some(w) = why {
        l.s(b" tls=skip reason=").s(w);
        verdict(&mut l, b"PASS");
    }
    let Some(ctx) = setup.context() else { verdict(l.s(b" tls=skip reason=no-context"), b"PASS") };
    let mut tcp = match Tcp::connect(ip, 443) {
        Ok(t) => t,
        Err(c) => {
            l.s(b" tls=connect").i(c);
            verdict(&mut l, b"FAIL reason=tls-connect");
        }
    };
    let mut treqbuf = [0u8; 64 + HOST_MAX + 64];
    let treq: &[u8] = build_req(&mut treqbuf, b"GET / HTTP/1.1\r\nHost: ", host);
    match vein_ring3::tls::with_session(&mut tcp, host, &ctx, |s, v| (get_status(s, treq, &mut head), v)) {
        Ok(((Ok(Some(code)), v), _)) => {
            l.s(b" tls=").s(&code).s(b" verified=").s(v.issuer().as_bytes());
            verdict(&mut l, b"PASS")
        }
        Ok(((_, v), fail)) => {
            l.s(b" tls=fail verified=").s(v.issuer().as_bytes()).s(b" reason=").s(fail.map_or("status-line", |f| f.why).as_bytes());
            verdict(&mut l, b"FAIL reason=tls-exchange")
        }
        Err(f) => {
            l.s(b" tls=fail reason=").s(f.why.as_bytes());
            verdict(&mut l, b"FAIL reason=tls-verify")
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: NETRING3: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): this program's launch declaration — a console program (no SYS_WIN_CREATE): a bare `net` runs in the foreground like `run`.
/// Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(0);
