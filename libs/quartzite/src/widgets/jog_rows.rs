//! CHARTER: Comscan (CODEX §2 hardware I/O, §5 Squawk) — the Console's jog pad as pure rows, read from comscan's PumpCard.
//!
//! VUGCOMSCANJOGFACEGTK (SR616): the pad's state lives in comscan's `PumpCard` (LogView's shape: the core folds the bus,
//! [`comscan::PumpCard::hear`], and hands a [`CardSnapshot`]); what a face derives from it is written ONCE here,
//! platform-neutral (std + bandy + comscan, no toolkit): the rows ([`jog_rows`]) and the verb a key fires ([`key_verb`],
//! the card's own `press`, so nothing new rides the wire and the desk stays the one validator). The pad is greyed, with
//! the reason said, while no link is up. `platforms/gtk/console_view.rs` paints them; the AppKit face is owed
//! (VUG-COMSCAN-JOG-FACE-MACOS).

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

    /// A linked card with travel 200 / 200 / 100 declared and read: the link, the travel, the last word, the picker and
    /// the keys enabled, no reason row.
    #[test]
    fn jog_rows_linked_kat() {
        let mut c = card();
        c.hear(&squawk("done", Some("started")));
        c.set_travel([Some(200.0), Some(200.0), Some(100.0)], [Some(200.0), Some(200.0), Some(100.0)]);
        c.set_step(10.0);
        assert_eq!(
            jog_rows(&c.snapshot()),
            vec![
                JogRow::Link("link up · done".into()),
                JogRow::Travel("travel  X 200 (read 200)  Y 200 (read 200)  Z 100 (read 100)".into()),
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
}
