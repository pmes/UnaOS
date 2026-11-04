// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Falsifiable proofs for the Console log viewer (comscan's first capability):
// the bounded scrollback, live tail, pause/resume scroll-lock, the text +
// source filters, and the bus round-trip through `serve`. Each test fails on
// its own if the property it names regresses.

use bandy::state::{LogLine, LogSource};
use bandy::{LogEvent, SMessage, Synapse};
use comscan::LogView;

use std::time::Duration;
use tokio::time::timeout;

// ── Pure-core props (no bus, no runtime) ────────────────────────────────────

/// The ring NEVER grows past its cap; the OLDEST record is the one evicted; and
/// every eviction is counted. Offer cap+16 and assert exactly 16 dropped, the
/// survivors are the newest `cap`, and the head is the record at seq 16.
#[test]
fn scrollback_is_bounded_and_drops_oldest() {
    let cap = 8;
    let mut v = LogView::new(cap);
    let offered = cap + 16;
    for i in 0..offered {
        v.ingest("info", "test", format!("line-{i}"));
    }
    assert_eq!(v.len(), cap, "ring grew past its cap");
    assert_eq!(v.dropped(), 16, "eviction count wrong");

    let snap = v.snapshot();
    assert_eq!(snap.len(), cap);
    // Drop-OLDEST: the survivors are seqs 16..24, in order, present-preserving.
    assert_eq!(snap.first().unwrap().seq, 16, "did not drop the OLDEST");
    assert_eq!(snap.first().unwrap().content, "line-16");
    assert_eq!(snap.last().unwrap().content, format!("line-{}", offered - 1));
}

/// A zero cap is refused (clamped to 1): a scrollback that could not hold the
/// line it just took would loop dropping it.
#[test]
fn zero_cap_is_clamped() {
    let mut v = LogView::new(0);
    assert_eq!(v.cap(), 1);
    v.ingest("info", "test", "a");
    v.ingest("info", "test", "b");
    assert_eq!(v.len(), 1);
    assert_eq!(v.snapshot()[0].content, "b");
    assert_eq!(v.dropped(), 1);
}

/// Live tail: a fresh record appears at the end of the snapshot, and `on_log`
/// hands back a `LogTail` carrying it while not paused.
#[test]
fn live_tail_appends() {
    let mut v = LogView::new(16);
    v.ingest("info", "kernel", "boot");
    let ev = v.on_log("warn", "kernel", "late").expect("unpaused ingest emits a tail");
    match ev {
        LogEvent::LogTail { lines, paused, .. } => {
            assert!(!paused);
            assert_eq!(lines.len(), 2);
            assert_eq!(lines.last().unwrap().content, "late");
        }
        other => panic!("expected LogTail, got {other:?}"),
    }
}

/// Pause is scroll-lock: paused ingest still GROWS the ring but emits NOTHING;
/// resume re-emits and the frozen records are all present.
#[test]
fn pause_freezes_tail_but_ring_keeps_filling() {
    let mut v = LogView::new(16);
    v.set_paused(true);
    assert!(v.on_log("info", "a", "one").is_none(), "paused tail must not emit");
    assert!(v.on_log("info", "a", "two").is_none());
    assert_eq!(v.len(), 2, "paused ingest still fills the ring");

    // Resume via the command path — it must answer immediately with everything.
    let ev = v.apply(&LogEvent::LogPause(false)).expect("resume emits a tail");
    match ev {
        LogEvent::LogTail { lines, paused, .. } => {
            assert!(!paused);
            assert_eq!(lines.len(), 2);
        }
        other => panic!("expected LogTail, got {other:?}"),
    }
    // And the next live record now flows again.
    assert!(v.on_log("info", "a", "three").is_some());
}

/// Text filter: a match shows ONLY matches, a no-match shows EMPTY, clearing
/// restores all. Case-insensitive, and it spans content/source/level.
#[test]
fn text_filter_matches_nomatch_clear() {
    let mut v = LogView::new(32);
    v.ingest("info", "net", "link up");
    v.ingest("error", "net", "link DOWN");
    v.ingest("info", "gpu", "frame drawn");

    v.set_filter("link");
    assert_eq!(v.snapshot().len(), 2, "filter should keep both 'link' lines");

    v.set_filter("DoWn"); // case-insensitive
    let s = v.snapshot();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].content, "link DOWN");

    v.set_filter("nonexistent-zzz");
    assert!(v.snapshot().is_empty(), "a no-match filter must show empty");

    // Filter also spans the source and level fields.
    v.set_filter("gpu");
    assert_eq!(v.snapshot().len(), 1);
    v.set_filter("error");
    assert_eq!(v.snapshot().len(), 1);

    v.set_filter("");
    assert_eq!(v.snapshot().len(), 3, "clearing the filter restores every line");
}

/// Source facet: selecting a subsystem shows only that source; `All` restores.
#[test]
fn source_select_facets_by_subsystem() {
    let mut v = LogView::new(32);
    v.ingest("info", "kernel", "a");
    v.ingest("info", "net", "b");
    v.ingest("info", "kernel", "c");

    v.set_source(LogSource::Subsystem("kernel".into()));
    let s = v.snapshot();
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|l| l.source == "kernel"));

    // Source AND text filter compose.
    v.set_filter("c");
    assert_eq!(v.snapshot().len(), 1);

    v.set_filter("");
    v.set_source(LogSource::All);
    assert_eq!(v.snapshot().len(), 3);
}

/// `view_state` is the Console-pane snapshot the vessel wraps in
/// `ViewEntity::Console`. It must agree with the live tail: the SAME filtered
/// lines, and the same eviction/pause/filter/source state — so a freshly
/// summoned pane and a live subscriber never disagree about what the log says.
#[test]
fn view_state_snapshot_agrees_with_the_live_tail() {
    let cap = 4;
    let mut v = LogView::new(cap);
    for i in 0..(cap + 3) {
        v.ingest("info", if i % 2 == 0 { "kernel" } else { "net" }, format!("line-{i}"));
    }
    v.set_source(LogSource::Subsystem("kernel".into()));
    v.set_paused(true);

    let vs = v.view_state();
    // The lines match the filtered snapshot exactly.
    assert_eq!(vs.lines, v.snapshot());
    assert!(vs.lines.iter().all(|l| l.source == "kernel"));
    // Bounded-ring honesty and the active query ride along.
    assert_eq!(vs.dropped, v.dropped());
    assert!(vs.paused);
    assert_eq!(vs.source, LogSource::Subsystem("kernel".into()));
    assert_eq!(vs.filter, "");
}

// ── Bus round-trip through `serve` ──────────────────────────────────────────

/// Pull LogTails off `rx` until one arrives whose lines satisfy `want`, or time
/// out. Ignores non-`Logs` traffic and command echoes.
async fn next_tail_where(
    rx: &mut tokio::sync::broadcast::Receiver<SMessage>,
    want: impl Fn(&[LogLine]) -> bool,
) -> Vec<LogLine> {
    let deadline = Duration::from_secs(2);
    timeout(deadline, async {
        loop {
            if let Ok(SMessage::Logs(LogEvent::LogTail { lines, .. })) = rx.recv().await {
                if want(&lines) {
                    return lines;
                }
            }
        }
    })
    .await
    .expect("timed out waiting for the expected LogTail")
}

/// The whole contract over a real Synapse: fire `Log` producer messages, then
/// fire a `LogFilter` command, and assert a filtered `LogTail` comes back —
/// only the matching line, published by the handler onto the same bus.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bus_round_trip_filter_yields_filtered_tail() {
    let synapse = Synapse::new();

    // Subscribe the observer BEFORE anything is fired, and hand `serve` its own
    // receiver so there is no gap where traffic could be lost.
    let mut observer = synapse.subscribe();
    let handler_rx = synapse.subscribe();
    let bus = synapse.clone();
    tokio::spawn(async move {
        comscan::serve(bus, handler_rx, 128).await;
    });

    // Three producer records land in the ring.
    synapse.fire(SMessage::Log {
        level: "info".into(),
        source: "net".into(),
        content: "link up".into(),
    });
    synapse.fire(SMessage::Log {
        level: "error".into(),
        source: "gpu".into(),
        content: "reset needed".into(),
    });
    synapse.fire(SMessage::Log {
        level: "info".into(),
        source: "net".into(),
        content: "link down".into(),
    });

    // The unfiltered tail eventually shows all three.
    let all = next_tail_where(&mut observer, |l| l.len() == 3).await;
    assert_eq!(all.len(), 3);

    // Now a filter COMMAND on the bus; the handler must answer with a tail that
    // holds ONLY the "gpu" record.
    synapse.fire(SMessage::Logs(LogEvent::LogFilter("gpu".into())));
    let filtered = next_tail_where(&mut observer, |l| {
        l.len() == 1 && l[0].source == "gpu"
    })
    .await;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].content, "reset needed");
}

/// The handler must never react to its OWN `LogTail` output — otherwise the
/// published tail would echo on the broadcast bus and re-trigger the handler
/// forever. `apply` answers `None` for a `LogTail`, which is exactly what the
/// `serve` loop relies on (plus its `!matches!(.., LogTail)` guard). Proven
/// deterministically at the core, not by waiting on the absence of a message.
#[test]
fn handler_does_not_react_to_its_own_tail() {
    let mut v = LogView::new(16);
    v.ingest("info", "a", "one");
    let echo = LogEvent::LogTail {
        lines: vec![LogLine { seq: 99, level: "x".into(), source: "x".into(), content: "echo".into() }],
        dropped: 0,
        paused: false,
    };
    assert!(v.apply(&echo).is_none(), "a LogTail must produce no reaction");
    // And the ring is untouched by the echo.
    assert_eq!(v.len(), 1);
}

// --- VUGCOMSCANJOGFACE (SR579): the pump card's pad posts the desk's verbs; the Console says a commission once ---

/// A press of the card's pad is the bus request the desk already serves (PumpJog at the card's step and feed, PumpHome),
/// carrying the card's principal; and `serve` says `commissioned <name>` in the Console's feed once, however many times
/// the same Commissioned arrives (a Log fired after both is the fence: by then both were folded).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_jog_pad_press_posts_the_verb_and_commissioned_shows_once() {
    use bandy::{Origin, PumpEvent};
    use comscan::{Pad, PumpCard};

    let synapse = Synapse::new();
    let mut observer = synapse.subscribe();
    let handler_rx = synapse.subscribe();
    let bus = synapse.clone();
    tokio::spawn(async move {
        comscan::serve(bus, handler_rx, 128).await;
    });

    let who = Origin::LocalUser("una".to_string());
    let mut card = PumpCard::new(who.clone());
    assert_eq!((card.step(), card.feed()), (PumpCard::STEP_MM, PumpCard::FEED_MM_MIN));
    card.set_step(5.0);
    card.set_feed(1200.0);
    synapse.fire(card.press(Pad::XMinus));
    synapse.fire(card.press(Pad::Home));

    let got = timeout(Duration::from_secs(2), async {
        let mut got = Vec::new();
        while got.len() < 2 {
            if let Ok(SMessage::Pump(ev)) = observer.recv().await {
                got.push(ev);
            }
        }
        got
    })
    .await
    .expect("timed out waiting for the pad's requests");
    match &got[0] {
        PumpEvent::PumpJog { principal, dx, dy, dz, feed } => {
            assert_eq!(principal, &who);
            assert_eq!((*dx, *dy, *dz, *feed), (-5.0, 0.0, 0.0, 1200.0), "X minus at the card's step and feed");
        }
        other => panic!("expected PumpJog, got {other:?}"),
    }
    assert!(got[0].is_request());
    assert!(matches!(&got[1], PumpEvent::PumpHome { principal } if principal == &who), "{:?}", got[1]);

    let commissioned = PumpEvent::Commissioned { port: "/dev/ttyACM0".to_string(), name: "UNA-1a2b".to_string() };
    synapse.fire(SMessage::Pump(commissioned.clone()));
    synapse.fire(SMessage::Pump(commissioned));
    synapse.fire(SMessage::Log { level: "info".into(), source: "test".into(), content: "fence".into() });
    let lines = next_tail_where(&mut observer, |l| l.iter().any(|x| x.content == "fence")).await;
    let said: Vec<&LogLine> = lines.iter().filter(|x| x.content == "commissioned UNA-1a2b").collect();
    assert_eq!(said.len(), 1, "said once: {lines:?}");
    assert_eq!((said[0].level.as_str(), said[0].source.as_str()), ("info", "comscan"));
}

// --- VUGCOMSCANDECLARECONSOLE (SR624): a typed `machine declare ..` line from the Console's card, answered as every other ---

/// Hear every pump answer on `rx` into `card` until one says what `want` takes (a Squawk's event, or `refused: <why>` for a
/// PumpRefused), within 10 s; the words heard are in the panic.
async fn card_hears_until(
    rx: &mut tokio::sync::broadcast::Receiver<SMessage>,
    card: &mut comscan::PumpCard,
    mut want: impl FnMut(&str) -> bool,
) -> String {
    let mut heard: Vec<String> = Vec::new();
    let got = timeout(Duration::from_secs(10), async {
        loop {
            let Ok(SMessage::Pump(ev)) = rx.recv().await else { continue };
            card.hear(&ev);
            let word = match &ev {
                bandy::PumpEvent::Squawk { event: Some(e), .. } => e.clone(),
                bandy::PumpEvent::PumpRefused { why } => format!("refused: {why}"),
                _ => continue,
            };
            if want(&word) {
                return word;
            }
            heard.push(word);
        }
    })
    .await;
    match got {
        Ok(w) => w,
        Err(_) => panic!("timed out; the card heard {heard:?}"),
    }
}

/// A typed declare on the Console's card goes through the one parser with the card's principal; the desk answers the
/// declare word, which the card says in its squawk row; the desk's own road keeps it in Principia's store; and the next jog
/// past the typed X travel is refused by name while one inside it goes out (a desk over the vessel's undeclared machine,
/// on pump_core's mock grbl).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_typed_declare_answers_the_declare_word_and_bounds_the_next_jog() {
    use bandy::{Origin, PrefValue, PrincipiaCommand, PumpEvent};
    use comscan::pump::{Opener, PumpDesk, Verdict, digest_hex, serve_pump, undeclared_machine};
    use comscan::{Pad, PumpCard};
    use make_core::gcode::Dialect;
    use make_core::{Move, Program, Units, Vec3};
    use pump_core::Transport;
    use std::sync::{Arc, Mutex};

    let synapse = Synapse::new();
    let rx = synapse.subscribe();
    let mut watch = synapse.subscribe();
    let mut prefs = synapse.subscribe();
    let desk = Arc::new(Mutex::new(PumpDesk::new(undeclared_machine())));
    desk.lock().unwrap().hello_within(Duration::from_millis(50));
    let open: Box<Opener> = Box::new(|_: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        Ok(Box::new(pump_core::mock::MockGrbl::new(2)))
    });
    let task = tokio::spawn(serve_pump(synapse.clone(), rx, desk.clone(), open, Duration::from_millis(5)));

    let who = Origin::LocalUser("una".to_string());
    let mut card = PumpCard::new(who.clone());
    let typed = card.declare("machine declare bench x=120 y=200 z=80").expect("a declare the parser takes posts its request");
    match &typed {
        SMessage::Pump(PumpEvent::PumpDeclare { principal, name, x, y, z, dialect }) => {
            assert_eq!(principal, &who, "the card's own principal");
            assert_eq!((name.as_str(), *x, *y, *z, dialect), ("bench", 120.0, 200.0, 80.0, &None));
        }
        other => panic!("expected PumpDeclare, got {other:?}"),
    }
    synapse.fire(typed);
    let said = card_hears_until(&mut watch, &mut card, |w| w.starts_with("machine declared") || w.starts_with("refused")).await;
    assert_eq!(said, "machine declared bench x=120 y=200 z=80");
    assert_eq!(card.snapshot().squawk.as_deref(), Some("machine declared bench x=120 y=200 z=80"), "said in the squawk row");

    // The desk's own road (declare_kept) keeps the console's declare in Principia's store; nothing of the console's.
    let kept = timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(SMessage::Principia(PrincipiaCommand::PrefSet { ns, key, value })) = prefs.recv().await {
                return (ns, key, value);
            }
        }
    })
    .await
    .expect("the accepted declare is kept");
    assert_eq!(kept, ("comscan".to_string(), "machine".to_string(), PrefValue::Str("bench x=120 y=200 z=80".to_string())));

    // A run on the declared machine (the pass names it), started on the bus, to done: the link the jog rides.
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) });
    p.moves.push(Move::Linear { to: Vec3::new(10.0, 10.0, 5.0), feed: 300.0 });
    let digest = {
        let mut d = desk.lock().unwrap();
        let pass = Verdict::Pass { digest: p.digest(), machine: d.machine().digest() };
        d.arm(&pass, &p, Dialect::Grbl { laser: false }).expect("armed by a pass naming the declared machine")
    };
    synapse.fire(SMessage::Pump(PumpEvent::PumpStart {
        principal: who.clone(),
        program: digest_hex(digest),
        dialect: "grbl".to_string(),
        port: "mock".to_string(),
    }));
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w == "done" || w.starts_with("refused")).await, "done");

    // The typed X travel is 120: X150 refused by name before the wire, X100 goes out.
    card.set_step(150.0);
    synapse.fire(card.press(Pad::XPlus));
    let past = card_hears_until(&mut watch, &mut card, |w| w.starts_with("refused") || w.starts_with("jog sent")).await;
    assert!(past.starts_with("refused: jog: X150") && past.contains("the declared X travel 120"), "{past}");
    assert_eq!(card.snapshot().squawk.as_deref(), Some(past.as_str()));
    card.set_step(100.0);
    synapse.fire(card.press(Pad::XPlus));
    let inside = card_hears_until(&mut watch, &mut card, |w| w.starts_with("refused") || w.starts_with("jog sent")).await;
    assert_eq!(inside, "jog sent: $J=G91 X100 Y0 Z0 F600");

    synapse.fire(SMessage::Kill("comscan".to_string()));
    timeout(Duration::from_secs(5), task).await.expect("stops on Kill").unwrap();
}

/// A typed line the parser refuses posts nothing and is said in the card's squawk row in the parser's own words (no second
/// parser: the words are `declare_request`'s, verbatim); a line that is not a declare likewise.
#[test]
fn a_typed_bad_declare_is_refused_in_the_parsers_words() {
    use bandy::{Origin, PumpEvent};
    use comscan::PumpCard;
    use comscan::pump::declare_request;

    let who = Origin::LocalUser("una".to_string());
    let mut card = PumpCard::new(who.clone());
    for line in ["machine declare bench x=120 y=200", "machine home", "machine declare bench x=120 y=200 z=80 w=1"] {
        let why = declare_request(line, who.clone()).expect_err("the parser refuses it");
        assert!(card.declare(line).is_none(), "a refused line posts nothing: {line}");
        assert_eq!(card.snapshot().squawk, Some(format!("refused: {why}")), "the parser's words: {line}");
    }
    assert_eq!(
        card.snapshot().squawk.as_deref(),
        Some("refused: declare: unknown key \"w\" (x, y, z, dialect)"),
        "the last refusal, verbatim"
    );
    let missing = declare_request("machine declare bench x=120 y=200", who.clone()).unwrap_err();
    assert_eq!(missing, "declare: x, y and z are each required");

    // A line the parser takes is the request, carrying the card's principal; posting it says nothing of its own.
    let before = card.snapshot().squawk;
    match card.declare("machine declare bench x=120 y=200 z=80 dialect=grbl") {
        Some(SMessage::Pump(PumpEvent::PumpDeclare { principal, name, x, y, z, dialect })) => {
            assert_eq!(principal, who);
            assert_eq!((name.as_str(), x, y, z, dialect.as_deref()), ("bench", 120.0, 200.0, 80.0, Some("grbl")));
        }
        other => panic!("expected PumpDeclare, got {other:?}"),
    }
    assert_eq!(card.snapshot().squawk, before, "the desk's answer, not the post, is what the row says next");
}

// --- VUGCOMSCANMDNSDIAL (SR638): a found controller linked by one press of the card's row, then jogged ---

/// A discover's telnet entry pressed on the Console's card posts the card's PumpLink; the desk opens that `tcp://` name in
/// grbl through its one open road (the opener asked exactly it), says `linked <port>`, streams the empty program's modal
/// lines to `done`, and the next jog rides the link; a second press while linked is refused by name (a desk over the
/// vessel's undeclared machine, the travel typed first, on pump_core's mock grbl).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_found_controller_is_linked_by_one_press_and_jogs() {
    use bandy::{Origin, PumpEvent};
    use comscan::discovery::{Found, Service};
    use comscan::pump::{Opener, PumpDesk, serve_pump, undeclared_machine};
    use comscan::{Pad, PumpCard};
    use make_core::gcode::Dialect;
    use pump_core::Transport;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::{Arc, Mutex};

    let synapse = Synapse::new();
    let rx = synapse.subscribe();
    let mut watch = synapse.subscribe();
    let desk = Arc::new(Mutex::new(PumpDesk::new(undeclared_machine())));
    desk.lock().unwrap().hello_within(Duration::from_millis(50));
    let asked: Arc<Mutex<Vec<(String, Dialect)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    let open: Box<Opener> = Box::new(move |port: &str, d: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        seen.lock().unwrap().push((port.to_string(), d));
        Ok(Box::new(pump_core::mock::MockGrbl::new(2)))
    });
    let task = tokio::spawn(serve_pump(synapse.clone(), rx, desk.clone(), open, Duration::from_millis(5)));

    let mut card = PumpCard::new(Origin::LocalUser("una".to_string()));
    synapse.fire(card.declare("machine declare bench x=120 y=200 z=80").expect("a declare the parser takes"));
    assert_eq!(
        card_hears_until(&mut watch, &mut card, |w| w.starts_with("machine declared") || w.starts_with("refused")).await,
        "machine declared bench x=120 y=200 z=80"
    );

    let addr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40));
    let grbl = Some(Dialect::Grbl { laser: false });
    card.say_found(&[
        Found { name: "fluidnc".into(), addr, port: 23, scope: 0, service: Service::Telnet, dialect_hint: grbl },
        Found { name: "fluidnc".into(), addr, port: 80, scope: 0, service: Service::Http, dialect_hint: grbl },
    ]);
    synapse.fire(card.dial(0).expect("the telnet row"));
    let said = card_hears_until(&mut watch, &mut card, |w| w.starts_with("linked") || w.starts_with("refused")).await;
    assert_eq!(said, "linked tcp://192.168.1.40:23");
    assert_eq!(*asked.lock().unwrap(), vec![("tcp://192.168.1.40:23".to_string(), Dialect::Grbl { laser: false })], "the one opener, asked the row's name");
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w == "done" || w.starts_with("refused")).await, "done");
    assert!(card.snapshot().linked, "the pad's link is up");

    card.set_step(10.0);
    synapse.fire(card.press(Pad::XPlus));
    let jog = card_hears_until(&mut watch, &mut card, |w| w.starts_with("refused") || w.starts_with("jog sent")).await;
    assert_eq!(jog, "jog sent: $J=G91 X10 Y0 Z0 F600", "the jog rides the dialed link");

    synapse.fire(card.dial(0).expect("the telnet row"));
    let again = card_hears_until(&mut watch, &mut card, |w| w.starts_with("refused") || w.starts_with("linked")).await;
    assert_eq!(again, "refused: linked: tcp://192.168.1.40:23 (one link at a time; PumpStart replaces it)");
    assert!(matches!(card.dial(0), Some(SMessage::Pump(PumpEvent::PumpLink { .. }))));

    synapse.fire(SMessage::Kill("comscan".to_string()));
    timeout(Duration::from_secs(5), task).await.expect("stops on Kill").unwrap();
}

// --- VUGCOMSCANLINKREUSE (SR641): a PumpStart on the linked port streams on the live link; another port reopens ---

/// A controller linked by one press (SR638's road) then started on the SAME `tcp://` name streams the armed program on the
/// live link: said `started (on the live link)`, run to `done`, the opener asked ONCE; a start naming another port opens it
/// and replaces the link as before (said `started`, the opener asked a second time, that name) (a desk over the vessel's
/// undeclared machine, the travel typed first, on pump_core's mock grbl).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_start_on_the_linked_port_streams_on_the_link_and_another_port_reopens() {
    use bandy::{Origin, PumpEvent};
    use comscan::PumpCard;
    use comscan::discovery::{Found, Service};
    use comscan::pump::{Opener, PumpDesk, Verdict, digest_hex, serve_pump, undeclared_machine};
    use make_core::gcode::Dialect;
    use make_core::{Move, Program, Units, Vec3};
    use pump_core::Transport;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::{Arc, Mutex};

    let synapse = Synapse::new();
    let rx = synapse.subscribe();
    let mut watch = synapse.subscribe();
    let desk = Arc::new(Mutex::new(PumpDesk::new(undeclared_machine())));
    desk.lock().unwrap().hello_within(Duration::from_millis(50));
    let asked: Arc<Mutex<Vec<(String, Dialect)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    let open: Box<Opener> = Box::new(move |port: &str, d: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        seen.lock().unwrap().push((port.to_string(), d));
        Ok(Box::new(pump_core::mock::MockGrbl::new(2)))
    });
    let task = tokio::spawn(serve_pump(synapse.clone(), rx, desk.clone(), open, Duration::from_millis(5)));

    let who = Origin::LocalUser("una".to_string());
    let mut card = PumpCard::new(who.clone());
    synapse.fire(card.declare("machine declare bench x=120 y=200 z=80").expect("a declare the parser takes"));
    assert_eq!(
        card_hears_until(&mut watch, &mut card, |w| w.starts_with("machine declared") || w.starts_with("refused")).await,
        "machine declared bench x=120 y=200 z=80"
    );

    let addr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40));
    card.say_found(&[Found { name: "fluidnc".into(), addr, port: 23, scope: 0, service: Service::Telnet, dialect_hint: Some(Dialect::Grbl { laser: false }) }]);
    synapse.fire(card.dial(0).expect("the telnet row"));
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w.starts_with("linked") || w.starts_with("refused")).await, "linked tcp://192.168.1.40:23");
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w == "done" || w.starts_with("refused")).await, "done");
    assert_eq!(asked.lock().unwrap().len(), 1, "the link's one open");

    // A run on the declared machine, armed by a pass naming it (one arming, one run: armed again for the second start).
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) });
    p.moves.push(Move::Linear { to: Vec3::new(10.0, 10.0, 5.0), feed: 300.0 });
    let arm = |desk: &Arc<Mutex<PumpDesk>>| {
        let mut d = desk.lock().unwrap();
        let pass = Verdict::Pass { digest: p.digest(), machine: d.machine().digest() };
        d.arm(&pass, &p, Dialect::Grbl { laser: false }).expect("armed by a pass naming the declared machine")
    };
    let start = |digest: u64, port: &str| {
        SMessage::Pump(PumpEvent::PumpStart { principal: who.clone(), program: digest_hex(digest), dialect: "grbl".to_string(), port: port.to_string() })
    };

    synapse.fire(start(arm(&desk), "tcp://192.168.1.40:23"));
    assert_eq!(
        card_hears_until(&mut watch, &mut card, |w| w.starts_with("started") || w.starts_with("refused")).await,
        "started (on the live link)"
    );
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w == "done" || w.starts_with("refused")).await, "done");
    assert_eq!(asked.lock().unwrap().len(), 1, "the start streamed on the live link: the opener asked once");

    synapse.fire(start(arm(&desk), "tcp://192.168.1.40:24"));
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w.starts_with("started") || w.starts_with("refused")).await, "started");
    assert_eq!(card_hears_until(&mut watch, &mut card, |w| w == "done" || w.starts_with("refused")).await, "done");
    let names: Vec<String> = asked.lock().unwrap().iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(names, vec!["tcp://192.168.1.40:23".to_string(), "tcp://192.168.1.40:24".to_string()], "another port is opened");

    synapse.fire(SMessage::Kill("comscan".to_string()));
    timeout(Duration::from_secs(5), task).await.expect("stops on Kill").unwrap();
}
