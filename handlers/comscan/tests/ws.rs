// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VUGCOMSCANWS (SR499): a WebSocket grbl (FluidNC / ESP3D, RFC 6455) as the pump's Transport, exercised against std
// TcpListener fakes on 127.0.0.1:0 that speak through gneiss_pal's own codec on the server side. No real controller
// is touched.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bandy::{Origin, PumpEvent};
use comscan::pump::{Opener, PumpDesk, Verdict, by_scheme, digest_hex, with_ws};
use comscan::tcp;
use comscan::ws::{CONNECT_WAIT, Port, Refusal, opener, parse};
use gneiss_pal::api::http::ws::{
    FrameDecoder, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG, OP_TEXT, accept_key, close_payload, encode_frame, parse_close,
};
use make_core::gcode::Dialect;
use make_core::{Move, Program, Units, Vec3};
use pump_core::{LinkError, Pump, Transport};

const GRBL: Dialect = Dialect::Grbl { laser: false };

fn listener() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = l.local_addr().unwrap().port();
    (l, port)
}

/// Reads one request head off `s` (to the blank line).
fn read_head(s: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match s.read(&mut b) {
            Ok(1) => head.push(b[0]),
            _ => break,
        }
    }
    String::from_utf8(head).unwrap()
}

/// The request's `Sec-WebSocket-Key`.
fn key_of(req: &str) -> String {
    req.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case("sec-websocket-key").then(|| v.trim().to_string())
        })
        .expect("a Sec-WebSocket-Key")
}

/// Accepts one connection, checks the opening handshake (RFC 6455 §4.1) and answers 101 with the key's Accept.
fn accept_ws(l: &TcpListener, path: &str) -> TcpStream {
    let (mut s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let req = read_head(&mut s);
    assert!(req.starts_with(&format!("GET {path} HTTP/1.1\r\n")), "{req}");
    let lower = req.to_ascii_lowercase();
    assert!(lower.contains("\r\nupgrade: websocket\r\n"), "{req}");
    assert!(lower.contains("\r\nsec-websocket-version: 13\r\n"), "{req}");
    let key = key_of(&req);
    assert_eq!(key.len(), 24, "a 16-byte nonce, base64: {key}");
    assert!(key.ends_with("=="), "{key}");
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(&key)
    );
    s.write_all(head.as_bytes()).unwrap();
    s
}

/// The next whole frame the client sent (`None`: the socket ended).
fn next_frame(s: &mut TcpStream, dec: &mut FrameDecoder) -> Option<gneiss_pal::api::http::ws::Frame> {
    loop {
        if let Some(f) = dec.next_frame().expect("the client's frames are well formed") {
            return Some(f);
        }
        let mut b = [0u8; 512];
        match s.read(&mut b) {
            Ok(0) | Err(_) => return None,
            Ok(n) => dec.push(&b[..n]),
        }
    }
}

/// Reads the port until `done(bytes)` or the link ends (`true`) or 5 s pass.
fn drain(p: &mut Port, done: impl Fn(&[u8]) -> bool) -> (Vec<u8>, bool) {
    let t0 = Instant::now();
    let mut got = Vec::new();
    let mut buf = [0u8; 64];
    while t0.elapsed() < Duration::from_secs(5) {
        match p.read(&mut buf) {
            Ok(0) => thread::sleep(Duration::from_millis(2)),
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(LinkError::Closed) => return (got, true),
        }
        if done(&got) {
            break;
        }
    }
    (got, false)
}

fn url(port: u16) -> String {
    format!("ws://127.0.0.1:{port}/")
}

/// A fake FluidNC: the WebUI's `CURRENT_ID` in a text frame, the banner in a TEXT frame with no newline, then `ok`
/// per line in BINARY frames (its WSChannel's shape); every client frame must be masked. Returns the lines it ran.
fn fake_fluidnc(l: TcpListener) -> JoinHandle<Vec<String>> {
    thread::spawn(move || {
        let mut s = accept_ws(&l, "/");
        s.write_all(&encode_frame(true, OP_TEXT, b"CURRENT_ID:0", None)).unwrap();
        s.write_all(&encode_frame(true, OP_TEXT, b"Grbl 3.7 [FluidNC v3.7.8 (wifi) '$' for help]", None)).unwrap();
        let mut dec = FrameDecoder::new();
        let mut got = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        while let Some(f) = next_frame(&mut s, &mut dec) {
            assert!(f.mask.is_some(), "RFC 6455 §5.3: every client frame is masked");
            match f.opcode {
                OP_TEXT | OP_BINARY => {
                    for b in f.payload {
                        match b {
                            b'?' | b'!' | b'~' | 0x18 => {}
                            b'\n' => {
                                got.push(String::from_utf8_lossy(&cur).trim().to_string());
                                cur.clear();
                                let _ = s.write_all(&encode_frame(true, OP_BINARY, b"ok\r\n", None));
                            }
                            _ => cur.push(b),
                        }
                    }
                }
                OP_CLOSE => break,
                _ => {}
            }
        }
        got
    })
}

fn program() -> Program {
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) });
    for i in 0..20 {
        p.moves.push(Move::Linear { to: Vec3::new(i as f64, (i * 2) as f64, -1.0), feed: 300.0 });
    }
    p
}

fn refuse(what: &'static str) -> Box<Opener> {
    Box::new(move |port: &str| -> Result<Box<dyn Transport + Send>, String> { Err(format!("{what} asked for {port}")) })
}

#[test]
fn the_accept_value_is_rfc6455s_worked_example() {
    // RFC 6455 §1.3: the sample nonce and its Sec-WebSocket-Accept (the fakes answer with this function).
    assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

#[test]
fn an_armed_program_streams_to_done_over_a_websocket_through_the_ws_arm() {
    let (l, port) = listener();
    let fake = fake_fluidnc(l);
    let p = program();
    let mut desk = PumpDesk::new();
    let digest = desk.arm(&Verdict::Pass { digest: p.digest() }, &p, GRBL).expect("armed by a pass");
    let mut open = with_ws(opener(GRBL, CONNECT_WAIT), by_scheme(refuse("serial"), refuse("tcp")));
    let start = PumpEvent::PumpStart {
        principal: Origin::LocalUser("una".to_string()),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: url(port),
    };
    let out = desk.apply(&start, &mut *open);
    assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "started"), "{out:?}");

    let t0 = Instant::now();
    let mut last = None;
    while t0.elapsed() < Duration::from_secs(10) {
        for s in desk.tick(t0.elapsed().as_micros() as u64) {
            last = Some(s);
        }
        if matches!(&last, Some(PumpEvent::Squawk { event: Some(e), .. }) if e == "done" || e == "link lost") {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    match last {
        Some(PumpEvent::Squawk { sent, acked, total, event: Some(e), .. }) => {
            assert_eq!(e, "done");
            assert_eq!((sent, acked), (total, total));
        }
        other => panic!("no done squawk: {other:?}"),
    }
    drop(desk); // the link closes: the fake sees the socket end and returns
    let lines = fake.join().unwrap();
    let want = Pump::from_program(&p, GRBL).unwrap().lines().to_vec();
    assert_eq!(lines, want, "the controller ran make_core's text, line for line, over a WebSocket");
}

#[test]
fn the_ws_arm_routes_ws_and_wss_and_leaves_the_rest() {
    let ws = Box::new(|port: &str| -> Result<Box<dyn Transport + Send>, String> { Err(format!("ws:{port}")) });
    let mut open = with_ws(ws, by_scheme(refuse("serial"), refuse("tcp")));
    assert_eq!(open("ws://a:81/").err().unwrap(), "ws:ws://a:81/");
    assert_eq!(open("wss://a/").err().unwrap(), "ws:wss://a/");
    assert_eq!(open("tcp://a:23").err().unwrap(), "tcp asked for tcp://a:23");
    assert_eq!(open("/dev/ttyUSB0").err().unwrap(), "serial asked for /dev/ttyUSB0");
}

#[test]
fn a_server_ping_is_answered_with_a_masked_pong() {
    let (l, port) = listener();
    let srv = thread::spawn(move || {
        let mut s = accept_ws(&l, "/");
        s.write_all(&encode_frame(true, OP_PING, b"fluid", None)).unwrap();
        let mut dec = FrameDecoder::new();
        let pong = next_frame(&mut s, &mut dec).expect("a pong");
        s.write_all(&encode_frame(true, OP_BINARY, b"ok\r\n", None)).unwrap();
        pong
    });
    let mut p = Port::connect(&url(port), CONNECT_WAIT).expect("connect");
    let (got, closed) = drain(&mut p, |b| b.ends_with(b"ok\r\n"));
    assert_eq!((got.as_slice(), closed), (&b"ok\r\n"[..], false), "the ping is not stream data; the link lives on");
    let pong = srv.join().unwrap();
    assert_eq!(pong.opcode, OP_PONG);
    assert_eq!(pong.payload, b"fluid");
    assert!(pong.mask.is_some());
}

#[test]
fn a_close_frame_ends_the_link_as_closed_by_its_code_and_is_echoed() {
    let (l, port) = listener();
    let srv = thread::spawn(move || {
        let mut s = accept_ws(&l, "/");
        s.write_all(&encode_frame(true, OP_BINARY, b"ok\r\n", None)).unwrap();
        s.write_all(&encode_frame(true, OP_CLOSE, &close_payload(Some(1001), "bye"), None)).unwrap();
        let mut dec = FrameDecoder::new();
        next_frame(&mut s, &mut dec).expect("the echoed close")
    });
    let mut p = Port::connect(&url(port), CONNECT_WAIT).expect("connect");
    let peer = p.peer().to_string();
    let (got, closed) = drain(&mut p, |_| false);
    assert_eq!(got, b"ok\r\n", "what arrived before the close is still read");
    assert!(closed);
    assert_eq!(p.stopped(), Some(&Refusal::Closed { peer, code: Some(1001) }));
    assert_eq!(p.write(b"G0 X1\n"), Err(LinkError::Closed));
    let echo = srv.join().unwrap();
    assert_eq!(echo.opcode, OP_CLOSE);
    assert!(echo.mask.is_some());
    assert_eq!(parse_close(&echo.payload).unwrap().map(|(c, _)| c), Some(1001));
}

#[test]
fn rfc6455_section_5_7_vectors_round_trip_through_the_codec_and_the_port() {
    // The single-frame masked text "Hello" (§5.7): decoded, and encoded back with the same key, byte for byte.
    let masked = [0x81u8, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];
    let mut dec = FrameDecoder::new();
    dec.push(&masked);
    let f = dec.next_frame().unwrap().unwrap();
    assert_eq!((f.fin, f.opcode, f.payload.as_slice()), (true, OP_TEXT, &b"Hello"[..]));
    assert_eq!(f.mask, Some([0x37, 0xfa, 0x21, 0x3d]));
    assert_eq!(encode_frame(true, OP_TEXT, b"Hello", Some([0x37, 0xfa, 0x21, 0x3d])), masked);
    assert_eq!(encode_frame(true, OP_TEXT, b"Hello", None), [0x81u8, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);

    let (l, port) = listener();
    let srv = thread::spawn(move || {
        let mut s = accept_ws(&l, "/fluid");
        s.set_nodelay(true).unwrap();
        // §5.7's fragmented unmasked text "Hel" + "lo", cut mid-header across segments.
        for piece in [&[0x01u8][..], &[0x03u8, 0x48, 0x65, 0x6c, 0x80][..], &[0x02u8, 0x6c, 0x6f][..]] {
            s.write_all(piece).unwrap();
            thread::sleep(Duration::from_millis(20));
        }
        // §5.7's 256-byte unmasked binary message (16-bit length).
        let mut bin = vec![0x82, 0x7E, 0x01, 0x00];
        bin.extend((0..=255u8).collect::<Vec<u8>>());
        s.write_all(&bin).unwrap();
        let mut dec = FrameDecoder::new();
        next_frame(&mut s, &mut dec).expect("the client's Hello")
    });
    let mut p = Port::connect(&format!("ws://127.0.0.1:{port}/fluid"), CONNECT_WAIT).expect("connect");
    let (got, closed) = drain(&mut p, |b| b.len() >= 6 + 256);
    assert!(!closed);
    assert_eq!(&got[..6], b"Hello\n", "a fragmented text message is one line");
    assert_eq!(got[6..].to_vec(), (0..=255u8).collect::<Vec<u8>>(), "a binary message joins the stream as it is");
    assert_eq!(p.write(b"Hello"), Ok(5));
    let f = srv.join().unwrap();
    assert_eq!((f.fin, f.opcode, f.payload.as_slice()), (true, OP_TEXT, &b"Hello"[..]));
    assert!(f.mask.is_some());
}

#[test]
fn a_realtime_byte_past_ascii_goes_as_a_binary_frame() {
    let (l, port) = listener();
    let srv = thread::spawn(move || {
        let mut s = accept_ws(&l, "/");
        let mut dec = FrameDecoder::new();
        next_frame(&mut s, &mut dec).expect("the jog cancel")
    });
    let mut p = Port::connect(&url(port), CONNECT_WAIT).expect("connect");
    assert_eq!(p.write(&[0x85]), Ok(1));
    let f = srv.join().unwrap();
    assert_eq!((f.opcode, f.payload.as_slice()), (OP_BINARY, &[0x85u8][..]));
}

#[test]
fn an_oversized_or_masked_server_frame_is_refused_by_its_code() {
    for (frame, code) in [
        ({
            let mut h = vec![0x82, 0x7F];
            h.extend_from_slice(&(1u64 << 27).to_be_bytes());
            h
        }, 1009u16),
        (encode_frame(true, OP_BINARY, b"ok\r\n", Some([1, 2, 3, 4])), 1002u16),
    ] {
        let (l, port) = listener();
        let srv = thread::spawn(move || {
            let mut s = accept_ws(&l, "/");
            s.write_all(&frame).unwrap();
            let mut b = [0u8; 64];
            let _ = s.read(&mut b);
        });
        let mut p = Port::connect(&url(port), CONNECT_WAIT).expect("connect");
        let (got, closed) = drain(&mut p, |_| false);
        assert!(got.is_empty() && closed, "code {code}");
        match p.stopped() {
            Some(Refusal::Frame { code: c, .. }) => assert_eq!(*c, code),
            other => panic!("not a frame refusal for {code}: {other:?}"),
        }
        srv.join().unwrap();
    }
}

#[test]
fn a_refused_or_false_handshake_is_bad_handshake_by_status() {
    for (answer, status) in [
        ("HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n", 400u16),
        (
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
            101u16,
        ),
    ] {
        let (l, port) = listener();
        let srv = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _ = read_head(&mut s);
            s.write_all(answer.as_bytes()).unwrap();
            let mut b = [0u8; 64];
            let _ = s.read(&mut b);
        });
        match Port::connect(&url(port), CONNECT_WAIT) {
            Err(Refusal::BadHandshake { status: s, .. }) => assert_eq!(s, Some(status)),
            Err(other) => panic!("wrong refusal for {status}: {other:?}"),
            Ok(_) => panic!("{status} must not open"),
        }
        srv.join().unwrap();
    }
}

#[test]
fn bad_addresses_and_a_dead_port_are_refused_by_name() {
    assert!(matches!(parse("wss://fluidnc.local/"), Err(Refusal::BadAddress { why, .. }) if why.contains("VUG-COMSCAN-WSS")));
    assert!(matches!(parse("tcp://fluidnc.local:23"), Err(Refusal::BadAddress { .. })));
    assert!(matches!(parse("ws://:81/"), Err(Refusal::BadAddress { .. })));
    let (u, host, port) = parse("ws://fluidnc.local:81/").unwrap();
    assert_eq!((host.as_str(), port, u.request_target().as_str()), ("fluidnc.local", 81, "/"));
    assert_eq!(parse("ws://10.0.0.7").unwrap().2, 80, "RFC 6455 §3: ws defaults to 80");
    let (l, port) = listener();
    drop(l);
    assert!(matches!(Port::connect(&url(port), CONNECT_WAIT), Err(Refusal::Link(tcp::Refusal::Refused { .. }))));
}
