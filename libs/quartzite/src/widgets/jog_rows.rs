//! CHARTER: Comscan (CODEX §2 hardware I/O, §5 Squawk) — the Console's jog pad as pure rows, read from comscan's PumpCard.
//!
//! VUGCOMSCANJOGFACEGTK (SR616): the pad's state lives in comscan's `PumpCard` (LogView's shape: the core folds the bus,
//! [`comscan::PumpCard::hear`], and hands a [`CardSnapshot`]); what a face derives from it is written ONCE here,
//! platform-neutral (std + bandy + comscan, no toolkit): the rows ([`jog_rows`]) and the verb a key fires ([`key_verb`],
//! the card's own `press`, so nothing new rides the wire and the desk stays the one validator). The pad is greyed, with
//! the reason said, while no link is up. `platforms/gtk/console_view.rs` paints them, and (VUGCOMSCANJOGFACEMACOS,
//! SR622) so does the AppKit Console pane, `platforms/macos/console_view.rs` `jog_pad`. VUGCOMSCANDECLARECONSOLEFACE (SR633):
//! each pad's declare entry under the squawk row fires [`declare_verb`] (the card's own `declare`, the one parser).

use bandy::SMessage;
use comscan::{CardSnapshot, Pad, PumpCard};

/// The step picker's choices, mm.
pub const STEPS_MM: [f64; 3] = [1.0, 10.0, 100.0];

/// The six jog keys in the order a face lays them out, with their labels.
pub const KEYS: [(Pad, &str); 6] =
    [(Pad::XMinus, "X−"), (Pad::XPlus, "X+"), (Pad::YMinus, "Y−"), (Pad::YPlus, "Y+"), (Pad::ZMinus, "Z−"), (Pad::ZPlus, "Z+")];

/// What a pad greyed for want of a link says.
pub const NO_LINK_WHY: &str = "no link: home and jog ride the link a PumpStart opens";

/// One row of the jog pad, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub enum JogRow {
    /// The link: `link up · <state>`, `link lost` or `no link`.
    Link(String),
    /// The travel per axis: `travel  X 200 (read 200)  Y …`, `X —` for an axis the card does not know.
    Travel(String),
    /// The last position, only when the card has one.
    Position(String),
    /// The last thing the pump said, only when it said something.
    Squawk(String),
    /// The step picker: its choices and the card's step.
    Steps { choices: [f64; 3], chosen: f64, enabled: bool },
    /// The six jog keys.
    Pad { keys: [(Pad, &'static str); 6], enabled: bool },
    /// Why the pad is greyed, only while it is.
    Why(String),
    /// VUGCOMSCANMDNSDIAL (SR638): a controller the last discover found on the link (telnet entries only), in the card's
    /// order; a press links it ([`dial_verb`]). `addr` is the address as discovery said it, the label is [`found_label`].
    Found { name: String, addr: String, port: u16 },
}

fn axis_text(c: char, declared: Option<f64>, read: Option<f64>) -> String {
    let d = declared.map_or("—".to_string(), |v| format!("{v}"));
    match read {
        Some(r) => format!("{c} {d} (read {r})"),
        None => format!("{c} {d}"),
    }
}

/// The pad's rows from a card snapshot: the link, the travel, the position and the last word when there are any, the
/// step picker, the keys, and the reason while no link is up.
pub fn jog_rows(s: &CardSnapshot) -> Vec<JogRow> {
    let mut out = Vec::with_capacity(7);
    out.push(JogRow::Link(if s.linked {
        format!("link up · {}", s.state)
    } else if s.state == "link-lost" {
        "link lost".to_string()
    } else {
        "no link".to_string()
    }));
    let axes: Vec<String> = ['X', 'Y', 'Z'].iter().enumerate().map(|(i, c)| axis_text(*c, s.declared[i], s.read[i])).collect();
    out.push(JogRow::Travel(format!("travel  {}", axes.join("  "))));
    if let Some([x, y, z]) = s.position {
        out.push(JogRow::Position(format!("at X {x}  Y {y}  Z {z}")));
    }
    if let Some(w) = &s.squawk {
        out.push(JogRow::Squawk(w.clone()));
    }
    out.extend(s.found.iter().map(|f| JogRow::Found { name: f.name.clone(), addr: f.addr.to_string(), port: f.port })); // VUGCOMSCANMDNSDIAL (SR638): the found rows under the squawk row
    out.push(JogRow::Steps { choices: STEPS_MM, chosen: s.step, enabled: s.linked });
    out.push(JogRow::Pad { keys: KEYS, enabled: s.linked });
    if !s.linked {
        out.push(JogRow::Why(NO_LINK_WHY.to_string()));
    }
    out
}

/// The verb a press of `pad` fires: the card's own request ([`PumpCard::press`]: PumpJog by the card's step at its feed).
pub fn key_verb(card: &PumpCard, pad: Pad) -> SMessage {
    card.press(pad)
}

// --- VUGCOMSCANDECLARECONSOLEFACE (LEDGER SR633): the pad's declare entry, written once for both faces (named tail hunk) ---

/// The placeholder of the pad's declare entry, under the squawk row.
pub const DECLARE_PLACEHOLDER: &str = "machine declare <name> x=.. y=.. z=..";

/// The verb Enter in the pad's declare entry fires: the card's own request ([`PumpCard::declare`]: the ONE parser with
/// the card's principal; the desk stays the ONE validator and answers through [`PumpCard::hear`]). `Some`: the face fires
/// it and clears the entry. `None`: the parser refused the line, the card already says `refused: <why>` in the squawk
/// row, and the face keeps the text so the person can mend it. Either way the face repaints the rows; it never greys
/// the entry (a declare with no link is typed by the desk's next tick, SR623) and never checks the line itself.
pub fn declare_verb(card: &mut PumpCard, line: &str) -> Option<SMessage> {
    card.declare(line)
}

// --- VUGCOMSCANMDNSDIAL (LEDGER SR638): the found rows' press and the discover button, written once for both faces (named tail hunk) ---

/// The label of the button beside the declare entry that asks the link who is there (the face runs
/// `comscan::discovery::discover` off its main thread and hands the answers to the card's `say_found` on it).
pub const DISCOVER_LABEL: &str = "discover";

/// What a found row's button says: `<name> <addr>:<port>`, an IPv6 address bracketed (discovery's own words, less `found:`).
pub fn found_label(name: &str, addr: &str, port: u16) -> String {
    if addr.contains(':') { format!("{name} [{addr}]:{port}") } else { format!("{name} {addr}:{port}") }
}

/// The verb a press on found row `i` fires: the card's own request ([`PumpCard::dial`]: `PumpLink` with the card's
/// principal for the row's `tcp://` name). The desk answers `linked <port>` or `refused: <why>` through the card's `hear`.
/// `None`: no such row.
pub fn dial_verb(card: &PumpCard, i: usize) -> Option<SMessage> {
    card.dial(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bandy::{Origin, PumpEvent};

    fn card() -> PumpCard {
        PumpCard::new(Origin::LocalUser("una".into()))
    }

    fn squawk(state: &str, event: Option<&str>) -> PumpEvent {
        PumpEvent::Squawk { sent: 0, acked: 0, total: 0, state: state.into(), status: None, event: event.map(Into::into) }
    }

    /// A linked card that heard the desk's PumpEnvelope (VUGCOMSCANJOGTRAVELWIRE SR618: travel 350 / 350 / 110 declared,
    /// 200 / 350 / 110 read, settled at X 150 Y 0 Z -2.5): the link, the travel, the position, the last word, the picker
    /// and the keys enabled, no reason row.
    #[test]
    fn jog_rows_linked_kat() {
        let mut c = card();
        c.hear(&squawk("done", Some("started")));
        c.hear(&PumpEvent::PumpEnvelope {
            declared: [Some(350.0), Some(350.0), Some(110.0)],
            read: [Some(200.0), Some(350.0), Some(110.0)],
            position: Some([150.0, 0.0, -2.5]),
        });
        c.set_step(10.0);
        assert_eq!(
            jog_rows(&c.snapshot()),
            vec![
                JogRow::Link("link up · done".into()),
                JogRow::Travel("travel  X 350 (read 200)  Y 350 (read 350)  Z 110 (read 110)".into()),
                JogRow::Position("at X 150  Y 0  Z -2.5".into()),
                JogRow::Squawk("started".into()),
                JogRow::Steps { choices: [1.0, 10.0, 100.0], chosen: 10.0, enabled: true },
                JogRow::Pad { keys: KEYS, enabled: true },
            ]
        );
    }

    /// A fresh card: no link, no travel known, nothing said, the picker and the keys greyed and the reason said.
    #[test]
    fn jog_rows_unlinked_kat() {
        assert_eq!(
            jog_rows(&card().snapshot()),
            vec![
                JogRow::Link("no link".into()),
                JogRow::Travel("travel  X —  Y —  Z —".into()),
                JogRow::Steps { choices: [1.0, 10.0, 100.0], chosen: 1.0, enabled: false },
                JogRow::Pad { keys: KEYS, enabled: false },
                JogRow::Why("no link: home and jog ride the link a PumpStart opens".into()),
            ]
        );
        assert_eq!(KEYS.map(|(_, l)| l), ["X−", "X+", "Y−", "Y+", "Z−", "Z+"]);
    }

    /// The keys fire the card's PumpJog (X+ at step 10, Z− at step 100, at the card's feed), and the link rule over a
    /// loss, a relink (state still `link-lost`), a later word on the relinked link, a refusal and an idle desk.
    #[test]
    fn jog_rows_verbs_kat() {
        let mut c = card();
        c.set_step(10.0);
        match key_verb(&c, Pad::XPlus) {
            SMessage::Pump(PumpEvent::PumpJog { dx, dy, dz, feed, .. }) => assert_eq!((dx, dy, dz, feed), (10.0, 0.0, 0.0, 600.0)),
            other => panic!("expected PumpJog, got {other:?}"),
        }
        c.set_step(100.0);
        match key_verb(&c, Pad::ZMinus) {
            SMessage::Pump(PumpEvent::PumpJog { dx, dy, dz, feed, .. }) => assert_eq!((dx, dy, dz, feed), (0.0, 0.0, -100.0, 600.0)),
            other => panic!("expected PumpJog, got {other:?}"),
        }

        let link = |c: &PumpCard| jog_rows(&c.snapshot())[0].clone();
        c.hear(&squawk("halted", Some("alarm:1 hard limit")));
        assert_eq!(link(&c), JogRow::Link("link up · halted".into()));
        c.hear(&squawk("link-lost", Some("link lost at line 4; reconnect attempt 1 in 500 ms")));
        assert_eq!(link(&c), JogRow::Link("link lost".into()));
        assert!(jog_rows(&c.snapshot()).contains(&JogRow::Why(NO_LINK_WHY.into())));
        c.hear(&squawk("link-lost", Some("relinked on attempt 1; re-arm to resume from line 4")));
        assert_eq!(link(&c), JogRow::Link("link up · link-lost".into()));
        c.hear(&squawk("link-lost", Some("jog sent: $J=G91 X10 F600")));
        assert_eq!(link(&c), JogRow::Link("link up · link-lost".into()));
        c.hear(&PumpEvent::PumpRefused { why: "jog: no axis moves".into() });
        assert_eq!(c.snapshot().squawk.as_deref(), Some("refused: jog: no axis moves"));
        c.hear(&squawk("idle", None));
        assert_eq!(link(&c), JogRow::Link("no link".into()));
        assert_eq!(c.snapshot().squawk.as_deref(), Some("refused: jog: no axis moves"));
    }

    /// VUGCOMSCANDECLARECONSOLEFACE (SR633): the entry's verb is the card's declare. A whole line posts the card's
    /// PumpDeclare with its principal and leaves the squawk row alone (the desk answers through `hear`); a line the parser
    /// refuses posts nothing and the pad's squawk row says the parser's refusal verbatim.
    #[test]
    fn jog_rows_declare_kat() {
        let mut c = card();
        match declare_verb(&mut c, "machine declare bench x=120 y=200 z=80") {
            Some(SMessage::Pump(PumpEvent::PumpDeclare { principal, name, x, y, z, dialect })) => {
                assert_eq!(principal, Origin::LocalUser("una".into()));
                assert_eq!((name.as_str(), x, y, z, dialect), ("bench", 120.0, 200.0, 80.0, None));
            }
            other => panic!("expected PumpDeclare, got {other:?}"),
        }
        assert!(!jog_rows(&c.snapshot()).iter().any(|r| matches!(r, JogRow::Squawk(_))));
        assert!(declare_verb(&mut c, "machine declare bench x=120 y=200").is_none());
        assert!(jog_rows(&c.snapshot()).contains(&JogRow::Squawk("refused: declare: x, y and z are each required".into())));
        assert_eq!(DECLARE_PLACEHOLDER, "machine declare <name> x=.. y=.. z=..");
    }

    /// VUGCOMSCANMDNSDIAL (SR638): a discover's telnet entries are rows under the squawk row (the http hint is said, not
    /// listed); a press is the card's PumpLink for the row's tcp:// name with its principal; the next discover replaces
    /// the rows; a row that is not there fires nothing.
    #[test]
    fn jog_rows_found_kat() {
        use comscan::discovery::{Found, Service};
        use std::net::{IpAddr, Ipv4Addr};
        let at = |n: u8| IpAddr::V4(Ipv4Addr::new(192, 168, 1, n));
        let mut c = card();
        c.say_found(&[
            Found { name: "fluidnc".into(), addr: at(40), port: 23, scope: 0, service: Service::Telnet, dialect_hint: None },
            Found { name: "fluidnc".into(), addr: at(40), port: 80, scope: 0, service: Service::Http, dialect_hint: None },
            Found { name: "router".into(), addr: at(41), port: 23, scope: 0, service: Service::Telnet, dialect_hint: None },
        ]);
        let rows = jog_rows(&c.snapshot());
        assert_eq!(
            rows[2..5],
            [
                JogRow::Squawk("found: router 192.168.1.41:23".into()),
                JogRow::Found { name: "fluidnc".into(), addr: "192.168.1.40".into(), port: 23 },
                JogRow::Found { name: "router".into(), addr: "192.168.1.41".into(), port: 23 },
            ]
        );
        assert_eq!(rows.iter().filter(|r| matches!(r, JogRow::Found { .. })).count(), 2, "the http hint is not a row");
        assert_eq!(found_label("fluidnc", "192.168.1.40", 23), "fluidnc 192.168.1.40:23");
        assert_eq!(found_label("cnc", "fe80::1", 23), "cnc [fe80::1]:23");
        match dial_verb(&c, 0) {
            Some(SMessage::Pump(PumpEvent::PumpLink { principal, port, dialect })) => {
                assert_eq!(principal, Origin::LocalUser("una".into()));
                assert_eq!((port.as_str(), dialect.as_str()), ("tcp://192.168.1.40:23", "grbl"));
            }
            other => panic!("expected PumpLink, got {other:?}"),
        }
        match dial_verb(&c, 1) {
            Some(SMessage::Pump(PumpEvent::PumpLink { port, dialect, .. })) => {
                assert_eq!((port.as_str(), dialect.as_str()), ("tcp://192.168.1.41:23", "grbl"), "no hint: grbl's raw stream (quartzite names no make_core Dialect)");
            }
            other => panic!("expected PumpLink, got {other:?}"),
        }
        assert!(dial_verb(&c, 2).is_none());
        c.say_found(&[]);
        assert!(!jog_rows(&c.snapshot()).iter().any(|r| matches!(r, JogRow::Found { .. })), "the next discover replaces the rows");
    }
}
