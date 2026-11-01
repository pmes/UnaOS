// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! grbl 1.1's messages: the `error:N` and `ALARM:N` tables and the `<...>` status report.
//!
//! The tables are gnea/grbl `doc/csv/error_codes_en_US.csv` and `doc/csv/alarm_codes_en_US.csv` (the short names,
//! verbatim; codes 18 and 19 are unassigned). The status grammar is `doc/markdown/interface.md` "Real-time Status
//! Reports": `<State|MPos:x,y,z|...>` (or `WPos:`), fields separated by `|`, optional fields in any order.

use alloc::string::String;

/// `error:N` — the line was NOT executed.
pub const ERRORS: &[(u16, &str)] = &[
    (1, "Expected command letter"),
    (2, "Bad number format"),
    (3, "Invalid statement"),
    (4, "Value < 0"),
    (5, "Setting disabled"),
    (6, "Value < 3 usec"),
    (7, "EEPROM read fail. Using defaults"),
    (8, "Not idle"),
    (9, "G-code lock"),
    (10, "Homing not enabled"),
    (11, "Line overflow"),
    (12, "Step rate > 30kHz"),
    (13, "Check Door"),
    (14, "Line length exceeded"),
    (15, "Travel exceeded"),
    (16, "Invalid jog command"),
    (17, "Setting disabled"),
    (20, "Unsupported command"),
    (21, "Modal group violation"),
    (22, "Undefined feed rate"),
    (23, "Invalid gcode ID:23"),
    (24, "Invalid gcode ID:24"),
    (25, "Invalid gcode ID:25"),
    (26, "Invalid gcode ID:26"),
    (27, "Invalid gcode ID:27"),
    (28, "Invalid gcode ID:28"),
    (29, "Invalid gcode ID:29"),
    (30, "Invalid gcode ID:30"),
    (31, "Invalid gcode ID:31"),
    (32, "Invalid gcode ID:32"),
    (33, "Invalid gcode ID:33"),
    (34, "Invalid gcode ID:34"),
    (35, "Invalid gcode ID:35"),
    (36, "Invalid gcode ID:36"),
    (37, "Invalid gcode ID:37"),
    (38, "Invalid gcode ID:38"),
];

/// `ALARM:N` — the controller locked out (alarm 3, a reset during motion, means the position is lost).
pub const ALARMS: &[(u16, &str)] = &[
    (1, "Hard limit"),
    (2, "Soft limit"),
    (3, "Abort during cycle"),
    (4, "Probe fail"),
    (5, "Probe fail"),
    (6, "Homing fail"),
    (7, "Homing fail"),
    (8, "Homing fail"),
    (9, "Homing fail"),
    (10, "Homing fail"),
];

fn lookup(t: &[(u16, &'static str)], code: u16) -> &'static str {
    t.iter().find(|(c, _)| *c == code).map(|(_, n)| *n).unwrap_or("unknown")
}

/// The csv's short name for `error:code`, or "unknown".
pub fn error_name(code: u16) -> &'static str {
    lookup(ERRORS, code)
}

/// The csv's short name for `ALARM:code`, or "unknown".
pub fn alarm_name(code: u16) -> &'static str {
    lookup(ALARMS, code)
}

/// The machine state, with grbl's sub-state where it has one (`Hold:0` complete, `Hold:1` in progress; `Door:0..3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Run,
    Hold(u8),
    Jog,
    Alarm,
    Door(u8),
    Check,
    Home,
    Sleep,
}

/// One `<...>` report. Absent optional fields are `None` / empty.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusReport {
    pub state: State,
    pub mpos: Option<[f64; 3]>,
    pub wpos: Option<[f64; 3]>,
    pub wco: Option<[f64; 3]>,
    /// `Bf:blocks,bytes` — free planner blocks and free RX bytes.
    pub buf: Option<(u16, u16)>,
    /// `Ln:n`.
    pub line: Option<u32>,
    /// `F:f` or the first of `FS:f,s`.
    pub feed: Option<f64>,
    /// The second of `FS:f,s`.
    pub spindle: Option<f64>,
    /// `Ov:feed,rapid,spindle` percentages.
    pub ov: Option<[u16; 3]>,
    /// `Pn:` letters (XYZPDHRS).
    pub pins: String,
    /// `A:` letters (S C F M).
    pub accessories: String,
}

/// A report the parser will not take, by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusRefusal {
    NotChevroned,
    UnknownState(String),
    /// A field this parser knows, malformed.
    BadField(&'static str),
}

fn triple(v: &str, what: &'static str) -> Result<[f64; 3], StatusRefusal> {
    let mut it = v.split(',');
    let mut o = [0.0; 3];
    for slot in o.iter_mut() {
        *slot = it.next().and_then(|s| s.trim().parse::<f64>().ok()).ok_or(StatusRefusal::BadField(what))?;
    }
    // grbl sends three axes on a stock build; a fourth (an A axis build) is not ours to drop silently.
    if it.next().is_some() {
        return Err(StatusRefusal::BadField(what));
    }
    Ok(o)
}

fn state(s: &str) -> Result<State, StatusRefusal> {
    let (word, sub) = match s.split_once(':') {
        Some((w, n)) => (w, Some(n)),
        None => (s, None),
    };
    let subn = |what: &'static str| -> Result<u8, StatusRefusal> {
        match sub {
            None => Ok(0),
            Some(n) => n.parse::<u8>().map_err(|_| StatusRefusal::BadField(what)),
        }
    };
    Ok(match word {
        "Idle" => State::Idle,
        "Run" => State::Run,
        "Hold" => State::Hold(subn("Hold")?),
        "Jog" => State::Jog,
        "Alarm" => State::Alarm,
        "Door" => State::Door(subn("Door")?),
        "Check" => State::Check,
        "Home" => State::Home,
        "Sleep" => State::Sleep,
        _ => return Err(StatusRefusal::UnknownState(String::from(s))),
    })
}

/// Parse one status report line (`<Idle|MPos:0.000,0.000,0.000|FS:0,0>`). Unknown field names are skipped (a newer
/// grbl may add one); a known field that does not parse is refused by name.
pub fn parse_status(line: &str) -> Result<StatusReport, StatusRefusal> {
    let body = line
        .trim()
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .ok_or(StatusRefusal::NotChevroned)?;
    let mut fields = body.split('|');
    let st = state(fields.next().unwrap_or(""))?;
    let mut r = StatusReport {
        state: st,
        mpos: None,
        wpos: None,
        wco: None,
        buf: None,
        line: None,
        feed: None,
        spindle: None,
        ov: None,
        pins: String::new(),
        accessories: String::new(),
    };
    for f in fields {
        let Some((k, v)) = f.split_once(':') else { continue };
        match k {
            "MPos" => r.mpos = Some(triple(v, "MPos")?),
            "WPos" => r.wpos = Some(triple(v, "WPos")?),
            "WCO" => r.wco = Some(triple(v, "WCO")?),
            "Bf" => {
                let (a, b) = v.split_once(',').ok_or(StatusRefusal::BadField("Bf"))?;
                let a = a.parse::<u16>().map_err(|_| StatusRefusal::BadField("Bf"))?;
                let b = b.parse::<u16>().map_err(|_| StatusRefusal::BadField("Bf"))?;
                r.buf = Some((a, b));
            }
            "Ln" => r.line = Some(v.parse::<u32>().map_err(|_| StatusRefusal::BadField("Ln"))?),
            "F" => r.feed = Some(v.parse::<f64>().map_err(|_| StatusRefusal::BadField("F"))?),
            "FS" => {
                let (a, b) = v.split_once(',').ok_or(StatusRefusal::BadField("FS"))?;
                r.feed = Some(a.parse::<f64>().map_err(|_| StatusRefusal::BadField("FS"))?);
                r.spindle = Some(b.parse::<f64>().map_err(|_| StatusRefusal::BadField("FS"))?);
            }
            "Ov" => {
                let mut it = v.split(',').map(|s| s.parse::<u16>());
                let mut o = [0u16; 3];
                for slot in o.iter_mut() {
                    *slot = match it.next() {
                        Some(Ok(n)) => n,
                        _ => return Err(StatusRefusal::BadField("Ov")),
                    };
                }
                r.ov = Some(o);
            }
            "Pn" => r.pins = String::from(v),
            "A" => r.accessories = String::from(v),
            _ => {}
        }
    }
    Ok(r)
}
