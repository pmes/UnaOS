// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VUGCOMSCANMDNS (LEDGER SR632): the mDNS discover road — the one question's bytes, the answer walk on a responder's
// message (compression pointers followed, hostile bytes refused without a panic), the socket road against a fake
// responder on the loopback (legacy unicast: the same road the real 224.0.0.251 question takes), silence said
// `found: none`, and the card's squawk row. Nothing here dials or posts to the bus.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use bandy::Origin;
use comscan::PumpCard;
use comscan::discovery::{Found, Service, discover_at, query, walk, words};
use make_core::gcode::Dialect;

fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// The question: id 0, no flags, QDCOUNT 2, `_telnet._tcp.local` then `_http._tcp.local`, each PTR class IN with QU.
const QUERY_HEX: &str = "000000000002000000000000\
    075f74656c6e6574045f746370056c6f63616c00000c8001\
    055f68747470045f746370056c6f63616c00000c8001";

/// A response in the shape an ESP-IDF mdns responder gives FluidNC's two `mdns_service_add(NULL, ..)` services (no
/// instance name, so the hostname `fluidnc`; no TXT): two PTR answers, then SRV x2 and one A in the additional section,
/// every repeated name a compression pointer (`_tcp.local` at 20, `local` at 25, the instances at 42 / 70, the target
/// `fluidnc.local` at 98). CONSTRUCTED from RFC 1035 / 2782 / 6762 and FluidNC's source, not captured: no FluidNC
/// board answers on this seat. 144 bytes.
const FLUIDNC_HEX: &str = "\
    000084000000000200000003075f74656c6e6574045f746370056c6f63616c00\
    000c000100001194000a07666c7569646e63c00c055f68747470c014000c0001\
    00001194000a07666c7569646e63c034c02a0021800100000078001000000000\
    001707666c7569646e63c019c04600218001000000780008000000000050c062\
    c06200018001000000780004c0a80128";

fn fluidnc_found() -> Vec<Found> {
    let addr = Ipv4Addr::new(192, 168, 1, 40);
    let grbl = Some(Dialect::Grbl { laser: false });
    vec![
        Found { name: "fluidnc".into(), addr, port: 23, service: Service::Telnet, dialect_hint: grbl },
        Found { name: "fluidnc".into(), addr, port: 80, service: Service::Http, dialect_hint: grbl },
    ]
}

#[test]
fn the_question_is_one_ptr_per_service() {
    assert_eq!(query(), unhex(QUERY_HEX));
    assert_eq!(query().len(), 58);
}

#[test]
fn the_walk_follows_ptr_srv_a_through_compression_pointers() {
    let msg = unhex(FLUIDNC_HEX);
    assert_eq!(msg.len(), 144);
    assert_eq!(walk(&msg), fluidnc_found());
    assert_eq!(words(&walk(&msg)), vec!["found: fluidnc 192.168.1.40:23", "found: fluidnc 192.168.1.40:80 (http)"]);
}

#[test]
fn hostile_or_partial_messages_yield_nothing_and_never_panic() {
    let msg = unhex(FLUIDNC_HEX);
    // Our own question is not a response (QR clear).
    assert!(walk(&query()).is_empty(), "a question is not an answer");
    // A name whose pointer points at itself: the hop cap ends it.
    let looped = unhex("000084000000000100000000c00c000c00010000119400020000");
    assert!(walk(&looped).is_empty(), "a pointer loop is refused");
    // Every truncation of the response: the SRV / A had not arrived, so nothing is found (and nothing panics).
    for n in 0..msg.len() {
        assert!(walk(&msg[..n]).is_empty(), "truncated at {n}");
    }
    // The PTRs alone (ARCOUNT 0): an instance whose SRV and A did not come with it is not found; nothing is resolved further.
    let mut ptr_only = msg.clone();
    ptr_only[10] = 0;
    ptr_only[11] = 0;
    assert!(walk(&ptr_only).is_empty(), "no SRV / A in the message, no entry");
}

#[test]
fn the_socket_road_collects_a_fake_responders_unicast_answer() {
    let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
    responder.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let dest: SocketAddr = responder.local_addr().unwrap();
    let answer = std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        let (n, from) = responder.recv_from(&mut buf).expect("the question arrives");
        let asked = buf[..n].to_vec();
        // A legacy unicast question is answered to its source port; twice, as a responder repeating itself may.
        responder.send_to(&unhex(FLUIDNC_HEX), from).unwrap();
        responder.send_to(&unhex(FLUIDNC_HEX), from).unwrap();
        asked
    });
    let found = discover_at(dest, Duration::from_millis(800));
    assert_eq!(answer.join().unwrap(), query(), "the one question, as built");
    assert_eq!(found, fluidnc_found(), "both services, once each, telnet first");
}

#[test]
fn silence_is_found_none_and_the_card_says_it_in_the_squawk_row() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let found = discover_at(silent.local_addr().unwrap(), Duration::from_millis(300));
    assert!(found.is_empty(), "nothing answered");
    assert_eq!(words(&found), vec!["found: none"]);

    let mut card = PumpCard::new(Origin::LocalUser("una".to_string()));
    assert_eq!(card.say_found(&found), vec!["found: none"]);
    assert_eq!(card.snapshot().squawk.as_deref(), Some("found: none"));
    let said = card.say_found(&fluidnc_found());
    assert_eq!(said, vec!["found: fluidnc 192.168.1.40:23", "found: fluidnc 192.168.1.40:80 (http)"]);
    assert_eq!(card.snapshot().squawk.as_deref(), Some("found: fluidnc 192.168.1.40:80 (http)"), "the last word");
}
