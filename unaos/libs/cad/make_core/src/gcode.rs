// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The dialect text: ONE writer and ONE reader over the three controller families, in the research's order —
//! grbl 1.1 (the installed base, a subset of RS274NGC, real-time bytes out of band), Marlin 2 (the printers: G28,
//! M104 / M109 / M140 / M190, the E axis, G92 E), LinuxCNC's RS274NGC. STEP-NC (AP238) is a reader owed
//! (VUG-MAKE-STEPNC), not written.
//!
//! A move a dialect cannot say is refused by name (`Unsayable`), never commented out. Numbers are written on the
//! [`GRID`] (0.0001): positions, feeds and values on that grid round-trip exactly; an arc's centre is said relative to
//! its start (I/J) and snapped back to the grid on read, so it round-trips exactly when it is on the grid too.

use alloc::{format, string::String, string::ToString, vec::Vec};
use core::fmt::Write;
use libm::{fabs, round, sqrt};

use crate::geom::{Length, Vec2, Vec3, Xy};
use crate::machine::Units;
use crate::program::{Coolant, Heater, Move, Program, Spin};
use crate::validate::Refusal;

/// The writer's resolution (four decimals, the program's unit).
pub const GRID: f64 = 1e-4;

/// The controller family. `Grbl { laser }` is grbl's `$32` laser mode, in which M3 / M4 S is laser power (S = pct x
/// 10 at grbl's default `$30=1000`) and a spindle speed cannot be said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Grbl { laser: bool },
    Marlin,
    LinuxCnc,
}

/// grbl 1.1's real-time commands: single bytes the pump sends OUT OF BAND (never in the G-code text), acted on at
/// once by the controller. Comscan owns sending them (VUG-COMSCAN); they carry no safety authority (the E-stop is
/// hardwired).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Realtime {
    SoftReset = 0x18,
    StatusQuery = b'?',
    CycleStart = b'~',
    FeedHold = b'!',
    SafetyDoor = 0x84,
    JogCancel = 0x85,
    FeedOvrReset = 0x90,
    FeedOvrPlus10 = 0x91,
    FeedOvrMinus10 = 0x92,
    FeedOvrPlus1 = 0x93,
    FeedOvrMinus1 = 0x94,
    RapidOvrReset = 0x95,
    RapidOvr50 = 0x96,
    RapidOvr25 = 0x97,
    SpindleOvrReset = 0x99,
    SpindleOvrPlus10 = 0x9A,
    SpindleOvrMinus10 = 0x9B,
    SpindleOvrPlus1 = 0x9C,
    SpindleOvrMinus1 = 0x9D,
    SpindleStop = 0x9E,
    FloodToggle = 0xA0,
    MistToggle = 0xA1,
}

impl Realtime {
    pub fn byte(self) -> u8 {
        self as u8
    }
}

/// A number in the one dialect grammar: four decimals, trailing zeros and point trimmed, `-0` said `0` (VUGCOMSCANJOG
/// SR573: public, so pump_core's jog lines say numbers the same way).
pub fn num(o: &mut String, v: f64) {
    let s = format!("{:.4}", v);
    let t = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s.as_str() };
    if t == "-0" { o.push('0') } else { o.push_str(t) }
}

fn word(o: &mut String, c: char, v: f64) {
    o.push(' ');
    o.push(c);
    num(o, v);
}

fn xyz(o: &mut String, p: Vec3) {
    word(o, 'X', p.x);
    word(o, 'Y', p.y);
    word(o, 'Z', p.z);
}

/// To the grid as `k / 10000` (a correctly rounded quotient: equal to the parse of the same decimal).
fn snap(v: f64) -> f64 {
    round(v * 10000.0) / 10000.0
}

/// The program as `d`'s text, or the first move `d` cannot say.
pub fn write(p: &Program, d: Dialect) -> Result<String, Refusal> {
    let marlin = d == Dialect::Marlin;
    let laser = matches!(d, Dialect::Grbl { laser: true });
    let mut o = String::new();
    o.push_str(match p.units {
        Units::Mm => "G21",
        Units::Inch => "G20",
    });
    // Marlin takes one command per line.
    o.push_str(if marlin { "\nG90\nM82\n" } else { " G90 G17 G94\n" });
    let mut pos: Option<Vec3> = None;
    for (i, mv) in p.moves.iter().enumerate() {
        let no = |what: &'static str| Refusal::Unsayable { dialect: d, at: i, what };
        match mv {
            Move::Rapid { to } => {
                o.push_str("G0");
                xyz(&mut o, *to);
                pos = Some(*to);
            }
            Move::Linear { to, feed } => {
                o.push_str("G1");
                xyz(&mut o, *to);
                word(&mut o, 'F', *feed);
                pos = Some(*to);
            }
            Move::Arc { to, centre, cw, feed } => {
                let s = pos.ok_or(no("arc with no start position"))?;
                o.push_str(if *cw { "G2" } else { "G3" });
                xyz(&mut o, *to);
                word(&mut o, 'I', centre.x - s.x);
                word(&mut o, 'J', centre.y - s.y);
                word(&mut o, 'F', *feed);
                pos = Some(*to);
            }
            Move::ToolChange { tool } => match d {
                Dialect::LinuxCnc => {
                    let _ = write!(o, "T{} M6", tool);
                }
                Dialect::Marlin => {
                    let _ = write!(o, "T{}", tool);
                }
                Dialect::Grbl { .. } => return Err(no("tool change (grbl 1.1 rejects M6)")),
            },
            Move::Spindle(s) => {
                if laser {
                    return Err(no("spindle speed in grbl laser mode ($32=1)"));
                }
                match s {
                    Spin::Cw(r) => {
                        o.push_str("M3");
                        word(&mut o, 'S', *r);
                    }
                    Spin::Ccw(r) => {
                        o.push_str("M4");
                        word(&mut o, 'S', *r);
                    }
                    Spin::Off => o.push_str("M5"),
                }
            }
            Move::Power { pct } => {
                if !laser {
                    return Err(no("laser power outside grbl laser mode"));
                }
                if *pct == 0.0 {
                    o.push_str("M5");
                } else {
                    o.push_str("M4");
                    word(&mut o, 'S', *pct * 10.0);
                }
            }
            Move::Dwell { s } => {
                o.push_str("G4");
                // Marlin's P is milliseconds; grbl's and LinuxCNC's are seconds.
                word(&mut o, 'P', if marlin { *s * 1000.0 } else { *s });
            }
            Move::Coolant(c) => o.push_str(match c {
                Coolant::Mist => "M7",
                Coolant::Flood => "M8",
                Coolant::Off => "M9",
            }),
            Move::Comment(t) => {
                if t.contains('\n') || t.contains('\r') {
                    return Err(no("comment with a line break"));
                }
                if marlin {
                    o.push_str("; ");
                    o.push_str(t);
                } else {
                    if t.contains('(') || t.contains(')') {
                        return Err(no("comment with parentheses"));
                    }
                    o.push('(');
                    o.push_str(t);
                    o.push(')');
                }
            }
            Move::Home => {
                if !marlin {
                    return Err(no("homing (a controller action outside Marlin's G28)"));
                }
                o.push_str("G28");
                pos = None;
            }
            Move::Heat { heater, celsius, wait } => {
                if !marlin {
                    return Err(no("heater"));
                }
                o.push_str(match (*heater, *wait) {
                    (Heater::Hotend, false) => "M104",
                    (Heater::Hotend, true) => "M109",
                    (Heater::Bed, false) => "M140",
                    (Heater::Bed, true) => "M190",
                });
                word(&mut o, 'S', *celsius);
            }
            Move::Extrude { to, e, feed } => {
                if !marlin {
                    return Err(no("extrusion (E axis)"));
                }
                o.push_str("G1");
                xyz(&mut o, *to);
                word(&mut o, 'E', *e);
                word(&mut o, 'F', *feed);
                pos = Some(*to);
            }
            Move::SetE { e } => {
                if !marlin {
                    return Err(no("G92 E"));
                }
                o.push_str("G92");
                word(&mut o, 'E', *e);
            }
        }
        o.push('\n');
    }
    if !marlin {
        o.push_str("M2\n");
    }
    Ok(o)
}

/// The centre of an R-form arc (R < 0: the long way round).
fn centre_from_r(s: Vec2, e: Vec2, r: f64, cw: bool, line: usize) -> Result<Vec2, Refusal> {
    let ch = e - s;
    let d = ch.length();
    if d <= 0.0 {
        return Err(Refusal::Parse { line, what: "R arc with equal ends" });
    }
    let rr = fabs(r);
    let h2 = rr * rr - d * d * 0.25;
    if h2 < 0.0 && sqrt(-h2) > GRID {
        return Err(Refusal::Parse { line, what: "R smaller than half the chord" });
    }
    let h = sqrt(h2.max(0.0));
    let left = Vec2::new(-ch.y / d, ch.x / d);
    let sign = if cw != (r < 0.0) { -1.0 } else { 1.0 };
    Ok(s + ch * 0.5 + left * (h * sign))
}

fn mprio(m: i32) -> u8 {
    match m {
        6 => 0,
        3..=5 => 1,
        7..=9 => 2,
        2 | 30 => 9,
        _ => 3,
    }
}

/// `d`'s text as a Program: a modal reader (group 1 motion sticky, G90 / G91 folded to absolute, the units, F and S
/// modal). Anything it does not take is `Unsupported` by name; a malformed number is `Parse`.
pub fn read(text: &str, d: Dialect) -> Result<Program, Refusal> {
    let marlin = d == Dialect::Marlin;
    let laser = matches!(d, Dialect::Grbl { laser: true });
    let grbl = matches!(d, Dialect::Grbl { .. });
    let mut moves: Vec<Move> = Vec::new();
    let mut units: Option<Units> = None;
    let mut abs = true;
    let mut motion: Option<i32> = None;
    let mut feed: Option<f64> = None;
    let mut pos: [Option<f64>; 3] = [None; 3];
    let mut tsel: Option<u32> = None;
    let mut spin: Option<bool> = None;
    let mut s_val: Option<f64> = None;

    for (ln0, raw) in text.lines().enumerate() {
        let line = ln0 + 1;
        let unsup = |w: String| Refusal::Unsupported { line, word: w };
        let parse = |what: &'static str| Refusal::Parse { line, what };

        // Comments out, in order; the command text kept.
        let mut cmd = String::new();
        let mut comments: Vec<String> = Vec::new();
        let mut paren: Option<String> = None;
        for (k, c) in raw.char_indices() {
            if let Some(cur) = paren.as_mut() {
                if c == ')' {
                    comments.push(paren.take().unwrap_or_default());
                } else {
                    cur.push(c);
                }
                continue;
            }
            if c == ';' {
                let rest = &raw[k + 1..];
                comments.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                break;
            }
            if c == '(' && !marlin {
                paren = Some(String::new());
                continue;
            }
            cmd.push(c);
        }
        if paren.is_some() {
            return Err(parse("unclosed comment"));
        }
        if marlin {
            if let Some(k) = cmd.find('*') {
                cmd.truncate(k); // the line checksum
            }
        }
        for c in comments {
            moves.push(Move::Comment(c));
        }
        let cmd = cmd.trim();
        if cmd.is_empty() || cmd == "%" {
            continue;
        }

        // Words.
        let b = cmd.as_bytes();
        let mut k = 0;
        let mut gs: Vec<(f64, &str)> = Vec::new();
        let mut ms: Vec<(f64, &str)> = Vec::new();
        let mut w: [Option<f64>; 26] = [None; 26];
        while k < b.len() {
            let c = b[k];
            if c.is_ascii_whitespace() {
                k += 1;
                continue;
            }
            if !c.is_ascii_alphabetic() {
                return Err(parse("stray character"));
            }
            let letter = c.to_ascii_uppercase();
            k += 1;
            while k < b.len() && b[k] == b' ' {
                k += 1;
            }
            let st = k;
            while k < b.len() && (b[k].is_ascii_digit() || b[k] == b'.' || b[k] == b'-' || b[k] == b'+') {
                k += 1;
            }
            let s = &cmd[st..k];
            let v: f64 = s.parse().map_err(|_| parse("number"))?;
            match letter {
                b'G' => gs.push((v, s)),
                b'M' => ms.push((v, s)),
                b'N' => {}
                b'X' | b'Y' | b'Z' | b'I' | b'J' | b'K' | b'R' | b'F' | b'S' | b'P' | b'T' => {
                    w[(letter - b'A') as usize] = Some(v)
                }
                b'E' if marlin => w[(b'E' - b'A') as usize] = Some(v),
                _ => return Err(unsup(format!("{}{}", letter as char, s))),
            }
        }
        let wd = |c: u8| w[(c - b'A') as usize];
        let int = |v: f64| if v == round(v) { v as i32 } else { -1 };

        // F and S are modal values.
        if let Some(f) = wd(b'F') {
            feed = Some(f);
        }
        if let Some(s) = wd(b'S') {
            s_val = Some(s);
        }

        // G words.
        let (mut dwell, mut home, mut g92) = (false, false, false);
        for (v, s) in &gs {
            match int(*v) {
                g @ 0..=3 => motion = Some(g),
                17 | 94 | 54 => {}
                20 | 21 => {
                    let u = if int(*v) == 20 { Units::Inch } else { Units::Mm };
                    if units.is_some_and(|u0| u0 != u) {
                        return Err(unsup(format!("G{} (a units switch mid-program)", s)));
                    }
                    units = Some(u);
                }
                90 => abs = true,
                91 => abs = false,
                4 => dwell = true,
                28 if marlin => home = true,
                92 if marlin => g92 = true,
                _ => return Err(unsup(format!("G{}", s))),
            }
        }

        // T: Marlin changes on T alone; LinuxCNC selects for M6; grbl tracks nothing.
        if let Some(t) = wd(b'T') {
            if marlin {
                moves.push(Move::ToolChange { tool: t as u32 });
            } else if grbl {
                return Err(unsup(format!("T{}", t as u32)));
            } else {
                tsel = Some(t as u32);
            }
        }

        // M words in RS274NGC's order: M6, spindle, coolant, the rest, the end.
        let mut mi: Vec<(i32, &str)> = ms.iter().map(|(v, s)| (int(*v), *s)).collect();
        mi.sort_by_key(|(m, _)| mprio(*m));
        let mut end = false;
        let mut spindle_word = false;
        for (m, s) in &mi {
            match *m {
                6 if d == Dialect::LinuxCnc => {
                    let t = tsel.ok_or(parse("M6 without T"))?;
                    moves.push(Move::ToolChange { tool: t });
                }
                3 | 4 => {
                    let cw = *m == 3;
                    let sv = s_val.unwrap_or(0.0);
                    moves.push(if laser {
                        Move::Power { pct: sv / 10.0 }
                    } else {
                        Move::Spindle(if cw { Spin::Cw(sv) } else { Spin::Ccw(sv) })
                    });
                    spin = Some(cw);
                    spindle_word = true;
                }
                5 => {
                    moves.push(if laser { Move::Power { pct: 0.0 } } else { Move::Spindle(Spin::Off) });
                    spin = None;
                    spindle_word = true;
                }
                7 => moves.push(Move::Coolant(Coolant::Mist)),
                8 => moves.push(Move::Coolant(Coolant::Flood)),
                9 => moves.push(Move::Coolant(Coolant::Off)),
                2 | 30 => end = true,
                104 | 109 | 140 | 190 if marlin => {
                    let celsius = wd(b'S').ok_or(parse("heater without S"))?;
                    let heater = if *m == 104 || *m == 109 { Heater::Hotend } else { Heater::Bed };
                    moves.push(Move::Heat { heater, celsius, wait: *m == 109 || *m == 190 });
                    spindle_word = true;
                }
                82 if marlin => {}
                _ => return Err(unsup(format!("M{}", s))),
            }
        }
        // An S alone while the spindle (or beam) runs changes its speed (power).
        if let (Some(sv), Some(cw), false, false) = (wd(b'S'), spin, spindle_word, dwell) {
            moves.push(if laser {
                Move::Power { pct: sv / 10.0 }
            } else {
                Move::Spindle(if cw { Spin::Cw(sv) } else { Spin::Ccw(sv) })
            });
        }

        if dwell {
            let s = if marlin {
                match (wd(b'P'), wd(b'S')) {
                    (Some(p), _) => p / 1000.0,
                    (None, Some(s)) => s,
                    _ => return Err(parse("G4 without P or S")),
                }
            } else {
                wd(b'P').ok_or(parse("G4 without P"))?
            };
            moves.push(Move::Dwell { s });
        }
        let axes = [wd(b'X'), wd(b'Y'), wd(b'Z')];
        let any_axis = axes.iter().any(|a| a.is_some());
        if home {
            if any_axis {
                return Err(unsup("G28 with axes".to_string()));
            }
            moves.push(Move::Home);
            pos = [None; 3];
        }
        if g92 {
            if any_axis {
                return Err(unsup("G92 X/Y/Z (work offsets are not modelled)".to_string()));
            }
            let e = wd(b'E').ok_or(parse("G92 without E"))?;
            moves.push(Move::SetE { e });
        }

        // Motion.
        let e_word = if marlin { wd(b'E') } else { None };
        if (any_axis || e_word.is_some()) && !g92 && !home && !dwell {
            let mode = motion.ok_or(parse("axis words with no motion mode"))?;
            if marlin && !abs && e_word.is_some() {
                return Err(unsup("G91 E (relative extrusion)".to_string()));
            }
            let start = pos;
            let mut tgt = [0.0f64; 3];
            for a in 0..3 {
                tgt[a] = match (axes[a], start[a]) {
                    (Some(v), _) if abs => v,
                    (Some(v), Some(c)) => snap(c + v),
                    (None, Some(c)) => c,
                    _ => return Err(parse("position unknown")),
                };
            }
            let to = Vec3::new(tgt[0], tgt[1], tgt[2]);
            let mv = match mode {
                0 | 1 if e_word.is_some() => {
                    let f = feed.ok_or(parse("feed unset"))?;
                    Move::Extrude { to, e: e_word.unwrap_or(0.0), feed: f }
                }
                0 => Move::Rapid { to },
                1 => Move::Linear { to, feed: feed.ok_or(parse("feed unset"))? },
                _ => {
                    let cw = mode == 2;
                    let (sx, sy) = match (start[0], start[1]) {
                        (Some(x), Some(y)) => (x, y),
                        _ => return Err(parse("arc with no start")),
                    };
                    if wd(b'K').is_some_and(|k| k != 0.0) {
                        return Err(unsup("K (arcs outside the XY plane)".to_string()));
                    }
                    let s2 = Vec2::new(sx, sy);
                    let centre = match (wd(b'I'), wd(b'J'), wd(b'R')) {
                        (None, None, Some(r)) => centre_from_r(s2, to.xy(), r, cw, line)?,
                        (None, None, None) => return Err(parse("arc without I/J or R")),
                        (i, j, _) => Vec2::new(snap(sx + i.unwrap_or(0.0)), snap(sy + j.unwrap_or(0.0))),
                    };
                    Move::Arc { to, centre, cw, feed: feed.ok_or(parse("feed unset"))? }
                }
            };
            moves.push(mv);
            pos = [Some(to.x), Some(to.y), Some(to.z)];
        }
        if end {
            break;
        }
    }
    Ok(Program { units: units.unwrap_or(Units::Mm), moves })
}
