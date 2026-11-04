// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! # `pump` — Comscan pumps the approved stream (VUGCOMSCAN1, LEDGER SR476)
//!
//! CODEX §5: "Helm holds the wheel — the stream passes its gate ... **Comscan** takes the approved stream and pumps it
//! directly to the hardware via USB/Serial. Telemetry returns through its **Squawk** capability."
//!
//! - [`start`] is the ONLY way Comscan makes a [`Pump`]: it takes Helm's [`Verdict`] and refuses anything but
//!   `Verdict::Pass` by name (BP-vug-3: the design holds the wheel). The pump streams make_core's own dialect text
//!   (`pump_core::Pump::from_program`, which calls `make_core::gcode::write`).
//! - A bus message cannot carry a verdict (any sender could forge a pass), so [`PumpDesk::arm`] arms a program IN
//!   PROCESS through [`start`], and the bus verb `PumpStart` names an armed program by its digest
//!   (`make_core::Program::digest`, 16 hex digits). An unarmed digest, a dialect other than the armed one, a busy
//!   desk or a port the opener cannot open is answered `PumpRefused` by name.
//! - The link is the host's: the caller supplies the opener (port name -> [`Transport`]): `crate::tty` (termios,
//!   SR484) and `crate::tcp` (SR487), routed by [`by_scheme`]; tests open a `pump_core::mock::MockGrbl`.
//! - [`PumpDesk`] is the pure synchronous core (the [`crate::LogView`] shape); [`serve_pump`] is the only bus code.
//!   The desk sits behind a `std::sync::Mutex` that the caller's arming code and [`serve_pump`] share; the lock is
//!   taken only inside the synchronous calls and never held across an await.
//!
//! VUG-HELM-BIND (LEDGER SR489): Helm's `Verdict::Pass { digest }` names the program it passed, and [`start`] checks
//! it against `Program::digest()`, refusing [`Refused::Unbound`] by both digests: a pass issued for one program never
//! pumps another.
//!
//! VUG-HELM-BIND-MACHINE (LEDGER SR498): the pass names the machine too (`Verdict::Pass { digest, machine }`, the
//! reviewed `make_core::Machine::digest`). A [`PumpDesk`] is built for the machine the operator declares behind its
//! ports ([`PumpDesk::new`]), and [`start`] refuses a pass naming another machine as [`Refused::WrongMachine`] by both
//! identities: a pass planned for a 300 mm envelope never pumps to a 100 mm router.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bandy::{PumpEvent, SMessage, Synapse};
use make_core::{Machine, Program};
use make_core::gcode::Dialect;
use pump_core::{Event, Pump, PumpRefusal, PumpState, StatusReport, Transport};
use pump_core::pump::Redial; // VUGCOMSCANRECONNECT (SR521): the redial schedule
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

/// VUGHELM (SR475) folded first: the Verdict is helm's own (fold of SR476 collapsed the shim).
pub use helm::machine::Verdict;

/// Why Comscan will not pump, by name.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// Helm still asks a person `n` things.
    Asked(usize),
    /// Helm refused: make_core's refusals and the asks the policy holds at Never.
    RefusedByHelm { refusals: usize, never: usize },
    /// The pump would not take the program.
    Pump(PumpRefusal),
    /// VUG-HELM-BIND (SR489): the pass was issued for the program `passed` (or names none: Helm's `Unbound`), not
    /// for `program`, the one asked to start.
    Unbound { passed: Option<u64>, program: u64 },
    /// VUG-HELM-BIND-MACHINE (SR498): the pass was issued for the machine `passed` (its `Machine::digest`), not for
    /// `desk`, the machine this pump would drive.
    WrongMachine { passed: u64, desk: u64 },
    /// VUGCOMSCANRECONNECT (SR521): a re-arm with no run whose link was lost.
    NotLinkLost,
    /// VUGCOMSCANRECONNECT (SR521): a re-arm before the link came back (redial `attempt` is next).
    LinkDown { attempt: u32 },
    /// VUGCOMSCANRECONNECT (SR521): a re-arm for `program`, not the `lost` program the run was streaming.
    NotTheLostProgram { lost: u64, program: u64 },
    /// VUGCOMSCANRECONNECT (SR521): a re-arm in `asked`, not the `lost` run's dialect.
    DialectChanged { lost: Dialect, asked: Dialect },
    /// VUGCOMSCANHELLOID (SR546): the reopened link answered the identity verb `now` (`None`: silent or refused), not
    /// `was`, the answer the first open kept: another board took the port; the link is dropped and the redial goes on.
    AnotherBoard { was: String, now: Option<String> },
}

/// The only way Comscan makes a pump: a `Verdict::Pass` for `p` (VUG-HELM-BIND: by digest) on `m` (VUG-HELM-BIND-MACHINE:
/// by identity), then make_core's text for `p` in `d`.
pub fn start(v: &Verdict, p: &Program, d: Dialect, m: &Machine) -> Result<Pump, Refused> {
    match v {
        Verdict::Pass { digest, .. } if *digest != p.digest() => {
            Err(Refused::Unbound { passed: Some(*digest), program: p.digest() })
        }
        Verdict::Pass { machine, .. } if *machine != m.digest() => {
            Err(Refused::WrongMachine { passed: *machine, desk: m.digest() })
        }
        Verdict::Pass { .. } => Pump::from_program(p, d).map_err(Refused::Pump),
        Verdict::Unbound => Err(Refused::Unbound { passed: None, program: p.digest() }),
        Verdict::Ask(a) => Err(Refused::Asked(a.len())),
        Verdict::Refuse { refusals, never } => {
            Err(Refused::RefusedByHelm { refusals: refusals.len(), never: never.len() })
        }
    }
}

/// The host's opener: a port name to a live link.
pub type Opener = dyn FnMut(&str, Dialect) -> Result<Box<dyn Transport + Send>, String> + Send; // VUGCOMSCANHOSTDIALECT (SR566): the dialect at call time

/// A digest as the bus names it.
pub fn digest_hex(d: u64) -> String {
    format!("{:016x}", d)
}

/// The bus' dialect word: `grbl`, `grbl-laser`, `marlin` (LinuxCNC is not streamed).
pub fn dialect_word(d: Dialect) -> &'static str {
    match d {
        Dialect::Grbl { laser: false } => "grbl",
        Dialect::Grbl { laser: true } => "grbl-laser",
        Dialect::Marlin => "marlin",
        Dialect::LinuxCnc => "linuxcnc",
    }
}

fn parse_dialect(s: &str) -> Option<Dialect> {
    match s {
        "grbl" => Some(Dialect::Grbl { laser: false }),
        "grbl-laser" => Some(Dialect::Grbl { laser: true }),
        "marlin" => Some(Dialect::Marlin),
        _ => None,
    }
}

fn state_word(s: PumpState) -> &'static str {
    match s {
        PumpState::Streaming => "streaming",
        PumpState::Halted(_) => "halted",
        PumpState::Stopping => "stopping",
        PumpState::Stopped => "stopped",
        PumpState::Done => "done",
        PumpState::LinkLost { .. } => "link-lost",
        PumpState::Rearmed { .. } => "rearmed",
    }
}

fn axes(o: &mut String, k: &str, v: Option<[f64; 3]>) {
    if let Some([x, y, z]) = v {
        o.push_str(&format!("|{}:{:.3},{:.3},{:.3}", k, x, y, z));
    }
}

/// A status report back in grbl's own grammar (the fields the report carried).
pub fn status_text(r: &StatusReport) -> String {
    use pump_core::State;
    let mut o = String::from("<");
    o.push_str(&match r.state {
        State::Idle => "Idle".to_string(),
        State::Run => "Run".to_string(),
        State::Hold(n) => format!("Hold:{}", n),
        State::Jog => "Jog".to_string(),
        State::Alarm => "Alarm".to_string(),
        State::Door(n) => format!("Door:{}", n),
        State::Check => "Check".to_string(),
        State::Home => "Home".to_string(),
        State::Sleep => "Sleep".to_string(),
    });
    axes(&mut o, "MPos", r.mpos);
    axes(&mut o, "WPos", r.wpos);
    if let Some((b, c)) = r.buf {
        o.push_str(&format!("|Bf:{},{}", b, c));
    }
    if let Some(n) = r.line {
        o.push_str(&format!("|Ln:{}", n));
    }
    match (r.feed, r.spindle) {
        (Some(f), Some(s)) => o.push_str(&format!("|FS:{},{}", f, s)),
        (Some(f), None) => o.push_str(&format!("|F:{}", f)),
        _ => {}
    }
    axes(&mut o, "WCO", r.wco);
    if !r.pins.is_empty() {
        o.push_str(&format!("|Pn:{}", r.pins));
    }
    if let Some([f, ra, s]) = r.ov {
        o.push_str(&format!("|Ov:{},{},{}", f, ra, s));
    }
    if !r.accessories.is_empty() {
        o.push_str(&format!("|A:{}", r.accessories));
    }
    o.push('>');
    o
}

/// The named event a Squawk carries, or `None` for routine traffic (an `ok`, a report, a message).
fn event_word(e: &Event) -> Option<String> {
    Some(match e {
        Event::Error { line, code, name } => format!("error:{} {} at line {}", code, name, line),
        Event::Alarm { code, name } => format!("alarm:{} {}", code, name),
        Event::Reset => "controller reset".to_string(),
        Event::ResendAsked { line } => format!("resend asked at line {} (VUG-COMSCAN-MARLIN-RESEND owed)", line),
        Event::Refused { line, text } => format!("refused at line {}: {}", line, text),
        Event::Timeout => "timeout".to_string(),
        Event::Stopped => "stopped".to_string(),
        Event::Done => "done".to_string(),
        Event::LinkLost => "link lost".to_string(),
        Event::Ok(_) | Event::Status(_) | Event::Message(_) => return None,
    })
}

struct Armed {
    digest: u64,
    dialect: Dialect,
    pump: Pump,
}

struct Running {
    pump: Pump,
    /// VUGCOMSCANRECONNECT (SR521): `None` while the link is lost (the dead one is dropped at the loss).
    link: Option<Box<dyn Transport + Send>>,
    /// The port name PumpStart opened: the redial opens it again.
    port: String,
    /// The redial schedule while the link is lost.
    redial: Option<Redial>,
    /// VUGCOMSCANTTYUSBID (SR529): the device the port was at the first open (`tty:usb:vvvv:pppp[:serial]`, from the
    /// desk's identify hook); when known the redial opens it by identity, wherever it came back.
    identity: Option<String>,
    /// VUGCOMSCANHELLOID (SR546): the controller's answer to its dialect's identity verb at the first open (`None`: the
    /// hello is off, the dialect has no verb, or the controller said nothing it keeps); a reopen must answer the same.
    hello: Option<String>,
    /// VUGCOMSCANJOG (SR573): the home / jog lines sent on this link and not yet answered, oldest first.
    motion: std::collections::VecDeque<Motion>,
    /// VUGCOMSCANJOGFACE (SR579): a jog was sent on this link and no home or jog cancel since (grbl answers `$J=` when
    /// it is PLANNED, so `motion` empties while the axis still moves; Hold / Stop cancel on this, not on `motion`).
    jogged: bool,
    /// VUGCOMSCANALARMRESET (SR585): the soft reset after an alarm went out and its banner is not yet seen; the banner
    /// sends the dialect's unlock line ([`pump_core::pump::unlock_line`]).
    unlock: bool,
}

/// Comscan's pump desk: the machine it drives, the armed programs and the one running stream.
pub struct PumpDesk {
    /// VUG-HELM-BIND-MACHINE (SR498): the machine the operator declared behind this desk's ports, set once at `new`.
    machine: Machine,
    armed: Vec<Armed>,
    running: Option<Running>,
    /// VUGCOMSCANTTYUSBID (SR529): a port name to its device identity (`crate::tty::identify`); none: redial by name.
    identify: Option<Box<Identify>>,
    /// VUGCOMSCANHELLOID (SR546): the wait for the identity answer when the hello is on (`hello_within`); none: off.
    hello: Option<Duration>,
    /// VUGCOMSCANALARMIDLE (SR591): the controller's travel is read at a first open (`envelope_read`); VUGCOMSCANENVELOPEREADON
    /// (SR597): on by default, riding the hello (read only where the hello is on).
    envelope: bool,
    /// VUGCOMSCANENVELOPEBOUND (SR602): the controller's travel the first open read, kept for the jog's [`bound`]; `None`:
    /// the hello or the read is off or the controller did not answer. VUGCOMSCANRELINKENVELOPE (SR607): an accepted relink
    /// reads it again (the start's spec, [`PumpDesk::envelope_spec_for`]) and keeps the reopen's answer.
    read: Option<Envelope>,
    /// VUGCOMSCANMACHINEDECLARE (SR613): the name and dialect of the machine a PumpDeclare declared (its travel is
    /// `machine`'s axes, rewritten in place); `None`: nothing declared on the bus this session.
    declared: Option<Declared>,
    /// VUGCOMSCANJOGPOSITION (SR612): the controller's last settled (Idle) machine position, polled outside a job, the poll's time and the once-per-link fallback word ([`Position`]).
    pos: Position,
}

fn refused(why: impl Into<String>) -> Vec<PumpEvent> {
    vec![PumpEvent::PumpRefused { why: why.into() }]
}

impl PumpDesk {
    /// A desk for `machine`: every pass it arms must name this machine's identity (VUG-HELM-BIND-MACHINE).
    pub fn new(machine: Machine) -> PumpDesk {
        PumpDesk { machine, armed: Vec::new(), running: None, identify: None, hello: None, envelope: true, read: None, declared: None, pos: Position::default() } // VUGCOMSCANMACHINEDECLARE (SR613): + declared
    }

    /// The machine this desk drives.
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// Arm `p` for `d` on the desk's machine through [`start`] (the Verdict's only entry); the digest `PumpStart`
    /// names. Re-arming the same digest replaces the earlier arming.
    pub fn arm(&mut self, v: &Verdict, p: &Program, d: Dialect) -> Result<u64, Refused> {
        let pump = start(v, p, d, &self.machine)?;
        let digest = p.digest();
        self.armed.retain(|a| a.digest != digest);
        self.armed.push(Armed { digest, dialect: d, pump });
        Ok(digest)
    }

    /// The digests armed and not yet started.
    pub fn armed(&self) -> Vec<u64> {
        self.armed.iter().map(|a| a.digest).collect()
    }

    fn squawk(&self, event: Option<String>) -> PumpEvent {
        match &self.running {
            Some(r) => {
                let p = r.pump.progress();
                PumpEvent::Squawk {
                    sent: p.sent as u32,
                    acked: p.acked as u32,
                    total: p.total as u32,
                    state: state_word(p.state).to_string(),
                    status: p.last.as_ref().map(status_text),
                    event,
                }
            }
            None => PumpEvent::Squawk { sent: 0, acked: 0, total: 0, state: "idle".to_string(), status: None, event },
        }
    }

    fn busy(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| {
                matches!(
                    r.pump.state(),
                    PumpState::Streaming | PumpState::Stopping | PumpState::LinkLost { .. } | PumpState::Rearmed { .. }
                )
            })
    }

    /// Apply one bus request; the answers to publish. Comscan's own answers are inert (no answer).
    pub fn apply(
        &mut self,
        ev: &PumpEvent,
        open: &mut dyn FnMut(&str, Dialect) -> Result<Box<dyn Transport + Send>, String>, // VUGCOMSCANHOSTDIALECT (SR566)
    ) -> Vec<PumpEvent> {
        match ev {
            // VUGCOMSCANHELLOON (SR549): the checks, the open + hello, the handback; the bus runs the middle with the desk
            // lock released (`request` over `open_road`, the redial's road).
            PumpEvent::PumpStart { .. } => match self.start_due(ev) {
                Ok(u) => {
                    let res = with_envelope(with_hello_named(open(&u.port, u.dialect), u.hello, u.name), u.envelope); // VUGCOMSCANHOSTDESK (SR561); VUGCOMSCANHOSTDIALECT (SR566); VUGCOMSCANALARMIDLE (SR591)
                    self.started(ev, res)
                }
                Err(out) => out,
            },
            PumpEvent::PumpHold { .. } | PumpEvent::PumpResume { .. } | PumpEvent::PumpStop { .. } => {
                if let Some(out) = self.alarm_reset_due(ev) { return out; } // VUGCOMSCANALARMRESET (SR585): Stop on a run halted by an alarm is the soft reset at once
                if let Some(out) = self.jog_cancel_due(ev) { return out; } // VUGCOMSCANJOGFACE (SR579): Hold / Stop while a grbl jog runs is grbl's jog cancel
                let Some(r) = self.running.as_mut() else {
                    return refused("nothing is streaming");
                };
                let mut down = Down;
                let link: &mut dyn Transport = match r.link.as_mut() {
                    Some(l) => &mut **l,
                    None => &mut down,
                };
                let (res, word) = match ev {
                    PumpEvent::PumpHold { .. } => (r.pump.hold(link), "hold"),
                    PumpEvent::PumpResume { .. } => (r.pump.resume(link), "resume"),
                    _ => (r.pump.stop(link), "stop"),
                };
                if r.pump.state() == PumpState::Stopped {
                    r.redial = None; // a stopped run is not redialled
                }
                match res {
                    Ok(()) => vec![self.squawk(Some(word.to_string()))],
                    Err(e) => refused(format!("{word}: {e:?}")),
                }
            }
            PumpEvent::PumpHome { .. } | PumpEvent::PumpJog { .. } => self.motion(ev), // VUGCOMSCANJOG (SR573)
            PumpEvent::PumpDeclare { .. } => self.declare(ev), // VUGCOMSCANMACHINEDECLARE (SR613)
            PumpEvent::Squawk { .. } | PumpEvent::PumpRefused { .. } | PumpEvent::Commissioned { .. } => Vec::new(),
        }
    }

    /// Drive the running stream one turn; a Squawk for each named event, else one for any traffic.
    pub fn tick(&mut self, now_us: u64) -> Vec<PumpEvent> {
        let evs = match self.running.as_mut() {
            Some(Running { pump, link: Some(link), .. }) => pump.tick(&mut **link, now_us),
            _ => return Vec::new(),
        };
        self.position_seen(&evs, now_us); // VUGCOMSCANJOGPOSITION (SR612): the reports' settled position; the idle `?` on a quiet tick too
        if evs.is_empty() {
            return Vec::new();
        }
        let (evs, moved) = self.motion_replies(evs); // VUGCOMSCANJOG (SR573): the home / jog answers by name
        let moved: Vec<String> = moved.into_iter().chain(self.unlock_after_banner(&evs)).collect(); // VUGCOMSCANALARMRESET (SR585): the unlock after the reset's banner
        if evs.contains(&Event::LinkLost) {
            self.link_lost(now_us);
        }
        let named: Vec<String> =
            evs.iter().filter_map(event_word).map(|w| if w == "link lost" { self.lost_word() } else { w }).collect();
        let named: Vec<String> = moved.into_iter().chain(named).collect(); // VUGCOMSCANJOG (SR573)
        if named.is_empty() {
            return vec![self.squawk(None)];
        }
        named.into_iter().map(|w| self.squawk(Some(w))).collect()
    }
}

/// Comscan's pump loop on the bus: requests under [`SMessage::Pump`] go to the desk, the desk ticks every `every`, and
/// every answer is published as [`SMessage::Pump`]. Runs until the bus closes or a [`SMessage::Kill`] names
/// `"comscan"`. Subscribe first and hand the receiver in (the [`crate::serve`] seam).
pub async fn serve_pump(
    synapse: Synapse,
    mut rx: Receiver<SMessage>,
    desk: Arc<Mutex<PumpDesk>>,
    mut open: Box<Opener>,
    every: Duration,
) {
    hello_on(&desk); machine_from_prefs(&synapse, &desk).await; // VUGCOMSCANHELLOON (SR549): the bus turns the hello on (an explicit hello_within kept); VUGCOMSCANMACHINEDECLAREPREFS (SR619): the declared machine read once from Principia's store
    let t0 = tokio::time::Instant::now();
    let mut iv = tokio::time::interval(every);
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(SMessage::Pump(ev)) if ev.is_request() => {
                    let out = request(&desk, &ev, &mut open).await; declare_kept(&synapse, &desk, &ev, &out); // VUGCOMSCANMACHINEDECLAREPREFS (SR619): an accepted declare written to Principia's store; VUGCOMSCANHELLOON (SR549): PumpStart's open with the lock released; VUGCOMSCANHOSTDIALECT (SR566): off the worker
                    for o in out {
                        synapse.fire(SMessage::Pump(o));
                    }
                }
                Ok(SMessage::Kill(who)) if who == "comscan" => break,
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            _ = iv.tick() => {
                let now = t0.elapsed().as_micros() as u64;
                let out = match desk.lock() {
                    Ok(mut d) => d.tick(now),
                    Err(_) => Vec::new(),
                };
                for o in out {
                    synapse.fire(SMessage::Pump(o));
                }
                redial(&synapse, &desk, &mut open, t0).await; // VUGCOMSCANRECONNECT (SR521): the reopen, outside the lock; VUGCOMSCANHOSTDIALECT (SR566)
            }
        }
    }
}

// --- VUGCOMSCANTCP (LEDGER SR487): the port string by scheme and the transport-neutral banner wait (named tail hunk) ---

/// The scheme that names a network link on the bus: `tcp://host:port` (grblHAL / FluidNC telnet-style raw TCP).
pub const TCP_SCHEME: &str = "tcp://";

/// The one opener [`serve_pump`] takes, routed by the port string: a name starting [`TCP_SCHEME`] goes to `net` whole
/// (`crate::tcp::opener`), any other name (a `/dev` path) to `serial` (`crate::tty::opener`).
pub fn by_scheme(mut serial: Box<Opener>, mut net: Box<Opener>) -> Box<Opener> {
    Box::new(move |port: &str, d: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        if port.starts_with(TCP_SCHEME) { net(port, d) } else { serial(port, d) } // VUGCOMSCANHOSTDIALECT (SR566)
    })
}

/// Why a link did not come ready; each transport names its link in its own refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unready {
    /// grbl said no banner, even after a soft reset (`waited_ms` over both waits).
    NoBanner { waited_ms: u64 },
    /// The link would not take the one soft-reset byte within the wait.
    Stalled,
    /// The link went away while waiting.
    Closed,
}

/// Wait for the controller's banner before any pump sees the link: `Grbl ` (grbl) or `start` (Marlin). A silent grbl
/// is sent make_core's soft reset once (position-safe at idle) and waited for again, then `NoBanner`; a silent Marlin
/// proceeds (`Ok(None)`: native USB does not reset on open); LinuxCNC is not streamed (`Ok(None)`). Lifted from tty.rs
/// `Port::ready` / `banner` (SR484) so every transport shares it.
pub fn ready(link: &mut dyn Transport, dialect: Dialect, wait: Duration) -> Result<Option<String>, Unready> {
    let Some(key) = pump_core::pump::banner_key(dialect) else { return Ok(None) }; // VUGCOMSCANALARMRESET (SR585): the one banner key (LinuxCNC: none)
    if let Some(b) = banner(link, key, wait)? {
        return Ok(Some(b));
    }
    if let Dialect::Grbl { .. } = dialect {
        let reset = [make_core::gcode::Realtime::SoftReset.byte()];
        let end = std::time::Instant::now() + wait;
        loop {
            match link.write(&reset) {
                Ok(1) => break,
                Ok(_) if std::time::Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
                Ok(_) => return Err(Unready::Stalled),
                Err(_) => return Err(Unready::Closed),
            }
        }
        if let Some(b) = banner(link, key, wait)? {
            return Ok(Some(b));
        }
        return Err(Unready::NoBanner { waited_ms: 2 * wait.as_millis() as u64 });
    }
    Ok(None)
}

/// Read lines until one starts with `key` (`Some`) or `wait` passes (`None`).
fn banner(link: &mut dyn Transport, key: &str, wait: Duration) -> Result<Option<String>, Unready> {
    lines_until(link, wait, |s| s.starts_with(key))
}

/// The one line reader (the banner wait and the identity hello): read trimmed lines until `f` takes one (`Some`) or
/// `wait` passes (`None`).
fn lines_until(link: &mut dyn Transport, wait: Duration, mut f: impl FnMut(&str) -> bool) -> Result<Option<String>, Unready> {
    let end = std::time::Instant::now() + wait;
    let mut line: Vec<u8> = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        match link.read(&mut buf) {
            Ok(0) => {
                if std::time::Instant::now() >= end {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\n' {
                        let s = String::from_utf8_lossy(&line).trim().to_string();
                        line.clear();
                        if f(&s) {
                            return Ok(Some(s));
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
            Err(_) => return Err(Unready::Closed),
        }
    }
}

// --- VUGCOMSCANWS (LEDGER SR499): the WebSocket port strings and their opener arm (named hunk) ---

/// The scheme that names a WebSocket link on the bus: `ws://host:port/path` (FluidNC on port 81, ESP3D).
pub const WS_SCHEME: &str = "ws://";

/// The TLS WebSocket scheme: routed to the ws opener, which refuses it by name (owed VUG-COMSCAN-WSS).
pub const WSS_SCHEME: &str = "wss://";

/// An opener that sends a name starting [`WS_SCHEME`] or [`WSS_SCHEME`] whole to `ws` (`crate::ws::opener`) and any
/// other name to `rest` (the desk composes `with_ws(ws::opener(..), by_scheme(tty::opener(..), tcp::opener(..)))`;
/// [`by_scheme`], `PumpDesk::apply` and [`serve_pump`] unchanged).
pub fn with_ws(mut ws: Box<Opener>, mut rest: Box<Opener>) -> Box<Opener> {
    Box::new(move |port: &str, d: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        if port.starts_with(WS_SCHEME) || port.starts_with(WSS_SCHEME) { ws(port, d) } else { rest(port, d) } // VUGCOMSCANHOSTDIALECT (SR566)
    })
}

// --- VUGCOMSCANRECONNECT (LEDGER SR521): a lost link redialled with backoff, the run re-armed by a person (named hunk) ---

/// The link a link-less run answers Hold / Resume / Stop with: every call is `Closed`, so only a request that needs
/// no byte (Resume refused by name, Stop of a lost run) gets through.
struct Down;

impl Transport for Down {
    fn write(&mut self, _: &[u8]) -> Result<usize, pump_core::LinkError> {
        Err(pump_core::LinkError::Closed)
    }
    fn read(&mut self, _: &mut [u8]) -> Result<usize, pump_core::LinkError> {
        Err(pump_core::LinkError::Closed)
    }
}

impl PumpDesk {
    /// The running pump's state (`None`: nothing has started).
    pub fn state(&self) -> Option<PumpState> {
        self.running.as_ref().map(|r| r.pump.state())
    }

    /// The pump said `LinkLost`: drop the dead link (a tty's exclusive fd is released before the reopen) and, when
    /// the run is lost mid-stream, start the redial schedule.
    fn link_lost(&mut self, now_us: u64) {
        if let Some(r) = self.running.as_mut() {
            r.link = None;
            r.redial = match r.pump.state() {
                PumpState::LinkLost { .. } => Some(Redial::new(now_us)),
                _ => None,
            };
        }
    }

    /// The Squawk word for a loss: the line it stopped at and the first redial.
    fn lost_word(&self) -> String {
        match self.running.as_ref().map(|r| (r.pump.state(), r.redial)) {
            Some((PumpState::LinkLost { at_line, .. }, Some(d))) => format!(
                "link lost at line {at_line}; reconnect attempt {} in {} ms",
                d.attempt(),
                d.wait_us() / 1000
            ),
            _ => "link lost".to_string(),
        }
    }

    /// The port to reopen when a lost run's next redial is due at `now_us`: the device's identity when the first open
    /// gave one (VUGCOMSCANTTYUSBID, SR529: never the old name, which another board may have taken), else the name.
    pub fn redial_due(&self, now_us: u64) -> Option<String> {
        let r = self.running.as_ref()?;
        let lost = matches!(r.pump.state(), PumpState::LinkLost { .. });
        match r.redial {
            Some(d) if lost && r.link.is_none() && d.due(now_us) => {
                Some(r.identity.clone().unwrap_or_else(|| r.port.clone()))
            }
            _ => None,
        }
    }

    /// The opener's answer for a redial (the opener's own handshake and banner wait included). Taken only while the
    /// run is still lost and link-less; otherwise the new link is dropped and nothing is said. VUGCOMSCANRELINKENVELOPE
    /// (SR607): the travel is read after the hello, as at the start ([`with_envelope`]).
    pub fn relinked(&mut self, now_us: u64, res: Result<Box<dyn Transport + Send>, String>) -> Vec<PumpEvent> {
        let spec = self.hello_spec(); // VUGCOMSCANHELLOID (SR546): the reopened link says who it is first
        let read = self.running_dialect().and_then(|d| self.envelope_spec_for(d)); // VUGCOMSCANRELINKENVELOPE (SR607): the start's travel read, the same spec
        self.relinked_said(now_us, with_envelope(with_hello(res, spec).map(|(l, s)| (l, s, None, None)), read)) // VUGCOMSCANJOG (SR573): a reopen never commissions; VUGCOMSCANRELINKENVELOPE (SR607): it reads the travel again
    }

    /// [`PumpDesk::relinked`] with the reopened link's identity answer beside it (VUGCOMSCANHELLOID, SR546): an answer
    /// that differs from the one the first open kept refuses the relink by name and the redial goes on.
    /// VUGCOMSCANRELINKENVELOPE (SR607): an accepted relink keeps the reopen's travel read (the fourth slot) as the jog's
    /// bound and says the differing axes through the one sayer ([`PumpDesk::envelope_words`]) only when the read changed
    /// from the kept one; a refused or failed reopen leaves the kept read.
    pub fn relinked_said(
        &mut self,
        now_us: u64,
        res: Result<Opened, String>, // VUGCOMSCANJOG (SR573): the third slot (a commission) is never set on a reopen
    ) -> Vec<PumpEvent> {
        let Some(r) = self.running.as_mut() else { return Vec::new() };
        let PumpState::LinkLost { at_line, .. } = r.pump.state() else { return Vec::new() };
        let Some(mut d) = r.redial else { return Vec::new() };
        if r.link.is_some() {
            return Vec::new();
        }
        let mut fresh: Option<Envelope> = None; // VUGCOMSCANRELINKENVELOPE (SR607): the reopen's read when it changed
        let word = match res {
            Ok((link, now, _, _)) if r.hello.is_some() && now != r.hello => {
                drop(link); // another board: its link (a tty's exclusive fd) goes at once
                let k = d.attempt();
                d.failed(now_us);
                r.redial = Some(d);
                let was = r.hello.clone().unwrap_or_default();
                format!("reconnect attempt {k} refused: {:?}; attempt {} in {} ms", Refused::AnotherBoard { was, now }, d.attempt(), d.wait_us() / 1000)
            }
            Ok((link, _, _, env)) => {
                r.link = Some(link);
                r.redial = None;
                fresh = if env != self.read { env } else { None }; // VUGCOMSCANRELINKENVELOPE (SR607): said again only when it changed
                self.read = env; // VUGCOMSCANENVELOPEBOUND (SR602); VUGCOMSCANRELINKENVELOPE (SR607): the reopen's read (None: off or unanswered, the declared bound)
                format!("relinked on attempt {}; re-arm to resume from line {at_line}", d.attempt())
            }
            Err(why) => {
                let k = d.attempt();
                d.failed(now_us);
                r.redial = Some(d);
                format!("reconnect attempt {k} failed: {why}; attempt {} in {} ms", d.attempt(), d.wait_us() / 1000)
            }
        };
        let mut out = vec![self.squawk(Some(word))];
        for w in self.envelope_words(fresh) {
            out.push(self.squawk(Some(w))); // VUGCOMSCANRELINKENVELOPE (SR607): the one sayer, after the relinked word
        }
        out
    }

    /// Re-arm a lost run once its link is back: a fresh Helm `Verdict::Pass` for the SAME program on the desk's
    /// machine (through [`start`]: both digests), in the same dialect; the run then waits at the line it lost for a
    /// Resume. The line Resume starts from.
    pub fn rearm(&mut self, v: &Verdict, p: &Program, d: Dialect) -> Result<usize, Refused> {
        let Some(r) = self.running.as_ref() else { return Err(Refused::NotLinkLost) };
        let PumpState::LinkLost { at_line, program_digest } = r.pump.state() else {
            return Err(Refused::NotLinkLost);
        };
        if r.link.is_none() {
            return Err(Refused::LinkDown { attempt: r.redial.map(|x| x.attempt()).unwrap_or(0) });
        }
        if p.digest() != program_digest {
            return Err(Refused::NotTheLostProgram { lost: program_digest, program: p.digest() });
        }
        if d != r.pump.dialect() {
            return Err(Refused::DialectChanged { lost: r.pump.dialect(), asked: d });
        }
        let fresh = start(v, p, d, &self.machine)?.resume_at(at_line).map_err(Refused::Pump)?;
        if let Some(r) = self.running.as_mut() {
            r.pump = fresh;
        }
        Ok(at_line)
    }
}

/// serve_pump's reopen: the due port is taken under the desk lock, the opener (connect, handshake, banner wait) runs
/// with the lock RELEASED, and its answer is handed back under the lock to [`PumpDesk::relinked`].
async fn redial(synapse: &Synapse, desk: &Mutex<PumpDesk>, open: &mut Box<Opener>, t0: tokio::time::Instant) { // VUGCOMSCANHOSTDIALECT (SR566): async, the open off the worker
    let now = t0.elapsed().as_micros() as u64;
    // VUGCOMSCANHELLOON (SR549): the one open road (the hello too, with the lock released, SR546)
    let out = open_road(
        desk,
        open,
        |d| d.redial_due(now).zip(d.running_dialect()).map(|(port, dl)| (port, dl, d.hello_spec(), None, d.envelope_spec_for(dl))).ok_or_else(Vec::new), // VUGCOMSCANHOSTDESK (SR561): a reopen never commissions; VUGCOMSCANRELINKENVELOPE (SR607): it reads the travel, the start's spec; VUGCOMSCANHOSTDIALECT (SR566): the run's dialect
        |d, res| d.relinked_said(t0.elapsed().as_micros() as u64, res),
    )
    .await
    .unwrap_or_default();
    for o in out {
        synapse.fire(SMessage::Pump(o));
    }
}

// --- VUGCOMSCANTTYUSBID (LEDGER SR529): the desk's identity hook, so a redial reopens the same device (named tail hunk) ---

/// A port name to the identity the redial opens instead (`crate::tty::identify`: `tty:usb:vvvv:pppp[:serial]`), or
/// `None` (a pty, tcp, ws: the name is redialled). Called once per PumpStart under the desk lock, after the open.
pub type Identify = dyn Fn(&str) -> Option<String> + Send;

impl PumpDesk {
    /// Set the identity hook every later PumpStart records beside its port name.
    pub fn identify_by(&mut self, f: Box<Identify>) {
        self.identify = Some(f);
    }
}

// --- VUGCOMSCANHELLOID (LEDGER SR546): the controller's own identity, kept at the first open, checked at a reopen (named tail hunk) ---

/// Ask the controller who it is: its dialect's identity verb (`pump_core::pump::identity_verb`: grbl `$I`, Marlin
/// `M115`) sent once, the answer read through the one line reader and classified by `pump_core::pump::hello_line`; the
/// kept lines joined by one space. `None`: no verb, the link would not take the verb, the controller refused it,
/// kept nothing, went silent for `wait` or closed (the pump's next tick names a closed link).
pub fn hello(link: &mut dyn Transport, dialect: Dialect, wait: Duration) -> Option<String> {
    use pump_core::pump::{HelloLine, hello_line, identity_verb};
    let verb = identity_verb(dialect)?;
    if !send_line(link, verb, wait) {
        return None; // VUGCOMSCANHOSTDESK (SR561): the write loop lifted into send_line, shared with the commission
    }
    let mut kept: Vec<String> = Vec::new();
    let mut refused = false;
    let ended = lines_until(link, wait, |s| match hello_line(dialect, s) {
        HelloLine::Keep => {
            kept.push(s.to_string());
            false
        }
        HelloLine::End => true,
        HelloLine::Refused => {
            refused = true;
            true
        }
        HelloLine::Other => false,
    });
    match ended {
        Ok(Some(_)) if !refused && !kept.is_empty() => Some(kept.join(" ")),
        _ => None,
    }
}

/// An opener's answer with the hello run on its link when `spec` (dialect, wait) is set; the answer `None` otherwise.
pub fn with_hello(
    res: Result<Box<dyn Transport + Send>, String>,
    spec: Option<(Dialect, Duration)>,
) -> Result<(Box<dyn Transport + Send>, Option<String>), String> {
    res.map(|mut l| {
        let said = spec.and_then(|(d, w)| hello(&mut *l, d, w));
        (l, said)
    })
}

impl PumpDesk {
    /// Turn the hello on: every later PumpStart asks the controller its identity (waiting up to `wait`) and every
    /// reopen must answer the same.
    pub fn hello_within(&mut self, wait: Duration) {
        self.hello = Some(wait);
    }

    /// The identity the running stream's first open kept.
    pub fn hello(&self) -> Option<&str> {
        self.running.as_ref().and_then(|r| r.hello.as_deref())
    }

    /// The hello a reopen runs: the running pump's dialect and the wait, when the hello is on.
    pub fn hello_spec(&self) -> Option<(Dialect, Duration)> {
        let r = self.running.as_ref()?;
        self.hello_spec_for(r.pump.dialect())
    }

    /// The hello for a start in `d` (PumpStart: nothing is running yet).
    fn hello_spec_for(&self, d: Dialect) -> Option<(Dialect, Duration)> {
        self.hello.map(|w| (d, w))
    }
}

// --- VUGCOMSCANHELLOON (LEDGER SR549): the first open on the redial road, the lock released; the bus turns the hello on (named tail hunk) ---

/// The identity wait the bus turns on (VUG-COMSCAN-HELLO-ON): the network transports' banner wait, `crate::tcp::READY_WAIT`
/// (the hello runs after the banner, so a serial board's reset-on-open boot is already over).
pub const HELLO_WAIT: Duration = crate::tcp::READY_WAIT;

/// What a PumpStart's checks found: the arming's index, the port to open and the hello to run on it.
struct StartDue {
    i: usize,
    port: String,
    hello: Option<(Dialect, Duration)>,
    /// VUGCOMSCANHOSTDESK (SR561): the name a first bind commissions on an unnamed board (none: no hello, no verb).
    name: Option<String>,
    /// VUGCOMSCANHOSTDIALECT (SR566): the PumpStart's dialect, handed to the opener (its banner key).
    dialect: Dialect,
    /// VUGCOMSCANALARMIDLE (SR591): the travel read to run after the hello (none: the read is off).
    envelope: Option<(Dialect, Duration)>,
}

impl PumpDesk {
    /// PumpStart's checks under the desk lock, nothing opened: the port and the hello, or the refusal by name.
    fn start_due(&self, ev: &PumpEvent) -> Result<StartDue, Vec<PumpEvent>> {
        let PumpEvent::PumpStart { program, dialect, port, .. } = ev else { return Err(Vec::new()) };
        if self.busy() {
            return Err(refused("busy: a program is streaming (PumpStop first)"));
        }
        let Some(d) = parse_dialect(dialect) else {
            return Err(refused(format!("unknown dialect {dialect:?} (grbl, grbl-laser, marlin)")));
        };
        if let Some(why) = self.declared_dialect_refusal(d) { return Err(refused(why)); } // VUGCOMSCANMACHINEDECLARE (SR613): the declared record first
        let Ok(digest) = u64::from_str_radix(program, 16) else {
            return Err(refused(format!("program {program:?} is not a digest (16 hex digits)")));
        };
        let Some(i) = self.armed.iter().position(|a| a.digest == digest) else {
            return Err(refused(format!(
                "program {} is not armed (only Helm's Verdict::Pass arms a program, in process)",
                digest_hex(digest)
            )));
        };
        if self.armed[i].dialect != d {
            return Err(refused(format!(
                "dialect mismatch: {} was armed for {}, asked {}",
                digest_hex(digest),
                dialect_word(self.armed[i].dialect),
                dialect_word(d)
            )));
        }
        let hello = self.hello_spec_for(d);
        let name = match hello {
            Some(_) if pump_core::pump::name_verb(d, "").is_some() => Some(self.name_for(port)), // VUGCOMSCANHOSTDESK (SR561)
            _ => None,
        };
        Ok(StartDue { i, port: port.clone(), hello, name, dialect: d, envelope: self.envelope_spec_for(d) }) // VUGCOMSCANHOSTDIALECT (SR566); VUGCOMSCANALARMIDLE (SR591)
    }

    /// PumpStart's handback under the desk lock: the checks again (an arming may have moved while the lock was
    /// released; a refusal drops the new link), then the open's answer: a refused open leaves the arming, a live link
    /// starts the run and its hello is printed once.
    fn started(
        &mut self,
        ev: &PumpEvent,
        res: Result<Opened, String>, // VUGCOMSCANJOG (SR573): + the answer before a commission
    ) -> Vec<PumpEvent> {
        let u = match self.start_due(ev) {
            Ok(u) => u,
            Err(out) => return out, // a live link in `res` is dropped here (a tty's exclusive fd released)
        };
        let port = u.port;
        let (link, said, before, env) = match res { // VUGCOMSCANALARMIDLE (SR591): + the travel read
            Ok(x) => x,
            Err(e) => return refused(format!("port {port:?}: {e}")),
        };
        // One arming, one run: a FirstRun approval is per run, so a finished program must pass Helm again.
        let a = self.armed.remove(u.i);
        let identity = self.identify.as_ref().and_then(|f| f(port.as_str())); // VUGCOMSCANTTYUSBID (SR529)
        let commissioned = commissioned_name(a.dialect, before.as_deref(), said.as_deref()); // VUGCOMSCANJOG (SR573)
        self.running = Some(Running { pump: a.pump, link: Some(link), port: port.clone(), redial: None, identity, hello: said.clone(), motion: Default::default(), jogged: false, unlock: false });
        let mut out = vec![self.squawk(Some("started".to_string()))];
        if let Some(h) = before.or(said) {
            out.push(self.squawk(Some(format!("hello {h}")))); // printed once, at the first open (VUGCOMSCANJOG SR573: as the controller first said it)
        }
        self.read = env; // VUGCOMSCANENVELOPEBOUND (SR602): kept for the jog's bound (a hello-off start keeps None)
        self.pos = Position::default(); // VUGCOMSCANJOGPOSITION (SR612): a new link knows no position yet
        for w in self.envelope_words(env) {
            out.push(self.squawk(Some(w))); // VUGCOMSCANALARMIDLE (SR591): a travel that differs from the declared machine, said once
        }
        if let Some(name) = commissioned {
            out.push(PumpEvent::Commissioned { port, name }); // VUGCOMSCANJOG (SR573): the EEPROM write, its own word, once
        }
        out
    }
}

/// serve_pump's request: PumpStart takes the one open road (the checks under the desk lock, the opener and the hello
/// with it RELEASED, the handback under it); every other request is applied under the lock.
async fn request(desk: &Mutex<PumpDesk>, ev: &PumpEvent, open: &mut Box<Opener>) -> Vec<PumpEvent> { // VUGCOMSCANHOSTDIALECT (SR566): async
    if !matches!(ev, PumpEvent::PumpStart { .. }) {
        return match desk.lock() {
            Ok(mut d) => d.apply(ev, &mut **open),
            Err(_) => refused("the pump desk is poisoned"),
        };
    }
    open_road(desk, open, |d| d.start_due(ev).map(|u| (u.port, u.dialect, u.hello, u.name, u.envelope)), |d, res| d.started(ev, res)) // VUGCOMSCANHOSTDESK (SR561); VUGCOMSCANHOSTDIALECT (SR566)
        .await
        .unwrap_or_else(|| refused("the pump desk is poisoned"))
}

/// The one open road (PumpStart and the redial): `due` names the port and the hello under the desk lock (its `Err` the
/// answers when nothing is to open), the opener (connect, handshake, banner wait) and the hello run with the lock
/// RELEASED, and `back` takes the answer under the lock. `None`: the desk is poisoned. VUGCOMSCANHOSTDIALECT (SR566): `due`
/// names the dialect the opener is handed, and the released section runs on tokio's blocking pool ([`open_blocking`]):
/// the lock is not held across the await, and the handback re-takes it on the runtime thread.
async fn open_road(
    desk: &Mutex<PumpDesk>,
    open: &mut Box<Opener>,
    due: impl FnOnce(&PumpDesk) -> Result<(String, Dialect, Option<(Dialect, Duration)>, Option<String>, Option<(Dialect, Duration)>), Vec<PumpEvent>>, // VUGCOMSCANHOSTDESK (SR561): + the name; VUGCOMSCANHOSTDIALECT (SR566): + the dialect; VUGCOMSCANALARMIDLE (SR591): + the travel read
    back: impl FnOnce(&mut PumpDesk, Result<Opened, String>) -> Vec<PumpEvent>, // VUGCOMSCANJOG (SR573): Opened
) -> Option<Vec<PumpEvent>> {
    let (port, dialect, spec, name, envelope) = {
        let d = desk.lock().ok()?;
        match due(&*d) {
            Ok(x) => x,
            Err(out) => return Some(out),
        }
    };
    let res = open_blocking(open, port, dialect, spec, name, envelope).await; // the desk lock is released here (VUGCOMSCANHOSTDESK SR561: the commission too; VUGCOMSCANHOSTDIALECT (SR566): on the blocking pool)
    let mut d = desk.lock().ok()?;
    Some(back(&mut *d, res))
}

/// The bus turns the hello on (VUG-COMSCAN-HELLO-ON): a desk whose hello is unset waits [`HELLO_WAIT`]; an explicit
/// [`PumpDesk::hello_within`] is kept.
fn hello_on(desk: &Mutex<PumpDesk>) {
    if let Ok(mut d) = desk.lock() {
        if d.hello.is_none() {
            d.hello_within(HELLO_WAIT);
        }
    }
}

// --- VUGCOMSCANHOSTDESK (LEDGER SR561): the one host composition and the commissioned name at a first bind (named tail hunk) ---

/// Write `line` and its newline to the link within `wait` (the hello's and the commission's one writer); false when
/// the link would not take it or closed.
fn send_line(link: &mut dyn Transport, line: &str, wait: Duration) -> bool {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    let end = std::time::Instant::now() + wait;
    let mut off = 0;
    while off < bytes.len() {
        match link.write(&bytes[off..]) {
            Ok(n) if n > 0 => off += n,
            Ok(_) if std::time::Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            Ok(_) | Err(_) => return false,
        }
    }
    true
}

/// Commission the controller's own name: its dialect's name verb (`pump_core::pump::name_verb`: grbl `$I=<name>`)
/// sent once, its `ok` read through the one line reader, then the hello asked again; the readback (what the
/// controller stored: grbl upper-cases) or `None` (no verb, refused `error:N`, silent for `wait`, closed).
pub fn commission(link: &mut dyn Transport, dialect: Dialect, name: &str, wait: Duration) -> Option<String> {
    use pump_core::pump::{HelloLine, hello_line, name_verb};
    let verb = name_verb(dialect, name)?;
    if !send_line(link, &verb, wait) {
        return None;
    }
    let mut refused = false;
    let ended = lines_until(link, wait, |s| match hello_line(dialect, s) {
        HelloLine::End => true,
        HelloLine::Refused => {
            refused = true;
            true
        }
        HelloLine::Keep | HelloLine::Other => false,
    });
    match ended {
        Ok(Some(_)) if !refused => hello(link, dialect, wait),
        _ => None,
    }
}

/// [`with_hello`], then at a first bind (`name` set) on a board whose answer carries no name
/// (`pump_core::pump::hello_name` is `Some("")`) the commission, whose readback replaces the answer; a refused
/// commission keeps the unnamed answer (the reopen then compares by the build, VUGCOMSCANHELLOID's behaviour).
/// VUGCOMSCANJOG (SR573): the third slot is the unnamed answer, `Some` only when the commission acked and was read back.
pub fn with_hello_named(
    res: Result<Box<dyn Transport + Send>, String>,
    spec: Option<(Dialect, Duration)>,
    name: Option<String>,
) -> Result<Opened, String> {
    let (mut l, said) = with_hello(res, spec)?;
    let (said, before) = match (spec, name, said) {
        (Some((d, w)), Some(n), Some(s)) if pump_core::pump::hello_name(d, &s) == Some("") => {
            match commission(&mut *l, d, &n, w) {
                Some(back) => (Some(back), Some(s)),
                None => (Some(s), None),
            }
        }
        (_, _, s) => (s, None),
    };
    Ok((l, said, before, None)) // VUGCOMSCANALARMIDLE (SR591): the travel slot is filled by with_envelope
}

/// The name a first bind commissions: `UNA-` and the last 8 letters / digits of the USB serial (upper-cased) when the
/// device identity (`tty:usb:vvvv:pppp:serial`) carries at least 4, else 8 upper hex digits hashed from the port and
/// `nonce` (a CH340 board, tcp and ws carry no serial; the nonce keeps two unnamed boards bound at one port apart).
pub fn commission_name(identity: Option<&str>, port: &str, nonce: u64) -> String {
    let serial: String = identity
        .and_then(|id| id.splitn(5, ':').nth(4))
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let id = if serial.len() >= 4 {
        serial[serial.len().saturating_sub(8)..].to_string()
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        port.hash(&mut h);
        nonce.hash(&mut h);
        format!("{:08X}", h.finish() as u32)
    };
    format!("UNA-{id}")
}

/// A nonce per commission: the wall clock's nanoseconds and a process counter.
fn name_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ COUNT.fetch_add(1, Ordering::Relaxed).rotate_left(32)
}

impl PumpDesk {
    /// The name a first bind at `port` would commission (the identity hook's device, when set).
    fn name_for(&self, port: &str) -> String {
        let identity = self.identify.as_ref().and_then(|f| f(port));
        commission_name(identity.as_deref(), port, name_nonce())
    }
}

/// The host desk's tick (the tests' serve_pump tick).
pub const PUMP_TICK: Duration = Duration::from_millis(5);

/// THE composition of a desk and its opener: `machine` behind the ports, the identity hook, the hello on at
/// [`HELLO_WAIT`] (so a first bind commissions), and the opener routed ws / tcp / the rest
/// (`with_ws(ws, by_scheme(serial, tcp))`); no dialect is bound: each open is handed its PumpStart's (VUGCOMSCANHOSTDIALECT, SR566).
pub fn compose(
    machine: Machine,
    identify: Box<Identify>,
    serial: Box<Opener>,
    tcp: Box<Opener>,
    ws: Box<Opener>,
) -> (PumpDesk, Box<Opener>) {
    let mut desk = PumpDesk::new(machine);
    desk.identify_by(identify);
    desk.hello_within(HELLO_WAIT);
    (desk, with_ws(ws, by_scheme(serial, tcp)))
}

/// [`compose`] over the host's own links: `crate::tty::identify`, the serial port at grbl 1.1's default 115200 with
/// no flow control, tcp and ws within their connect waits, every opener made ready for the dialect each open is handed
/// (VUGCOMSCANHOSTDIALECT, SR566).
pub fn host_parts(machine: Machine) -> (PumpDesk, Box<Opener>) {
    compose(
        machine,
        host_identify(),
        host_serial(),
        crate::tcp::opener(crate::tcp::CONNECT_WAIT), // VUGCOMSCANHOSTDIALECT (SR566)
        crate::ws::opener(crate::ws::CONNECT_WAIT), // VUGCOMSCANHOSTDIALECT (SR566)
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn host_identify() -> Box<Identify> {
    crate::tty::identify()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_identify() -> Box<Identify> {
    Box::new(|_: &str| -> Option<String> { None })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn host_serial() -> Box<Opener> {
    crate::tty::opener(crate::tty::Baud::B115200, crate::tty::Flow::None) // VUGCOMSCANHOSTDIALECT (SR566)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn host_serial() -> Box<Opener> {
    Box::new(|port: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> { Err(format!("no serial ports on this host: {port}")) })
}

/// The machine a vessel's desk declares until one is declared (a `PumpDeclare`, VUGCOMSCANMACHINEDECLARE SR613): no axes,
/// no tools, a zero envelope. Nothing arms it in a vessel (arming is in process only), and a Helm pass for any real machine is
/// refused `Refused::WrongMachine` by both identities.
pub fn undeclared_machine() -> Machine {
    use make_core::{Aabb3, Kinematics, MachineKind, Power, PowerKind, Units, Vec3};
    Machine {
        kind: MachineKind::Mill3,
        kinematics: Kinematics::Cartesian,
        axes: Vec::new(),
        power: Power { kind: PowerKind::Spindle, max: 0.0, ask_above: 0.0 },
        envelope: Aabb3 { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(0.0, 0.0, 0.0) },
        safe_z: 0.0,
        units: Units::Mm,
        tools: Vec::new(),
        stock: None,
        fixtures: Vec::new(),
        loaded: None,
    }
}

// --- VUGCOMSCANHOSTDIALECT (LEDGER SR566): the open road's released section on tokio's blocking pool (named tail hunk) ---

/// The open road's released section off the runtime worker: the opener is MOVED into `tokio::task::spawn_blocking` (a
/// placeholder that refuses by name stands in), called with the port and the dialect it is handed, the hello and the
/// commission run on its link there, and the opener is moved back from the join. A panicked opener leaves the
/// placeholder: every later open is refused by name, the arming kept (the handback's refusal).
async fn open_blocking(
    open: &mut Box<Opener>,
    port: String,
    dialect: Dialect,
    spec: Option<(Dialect, Duration)>,
    name: Option<String>,
    envelope: Option<(Dialect, Duration)>, // VUGCOMSCANALARMIDLE (SR591)
) -> Result<Opened, String> { // VUGCOMSCANJOG (SR573): Opened
    let mut op = std::mem::replace(open, opener_lost());
    let job = tokio::task::spawn_blocking(move || {
        let res = with_envelope(with_hello_named(op(&port, dialect), spec, name), envelope); // VUGCOMSCANALARMIDLE (SR591)
        (op, res)
    });
    match job.await {
        Ok((op, res)) => {
            *open = op;
            res
        }
        Err(e) => Err(format!("the opener panicked: {e}")),
    }
}

/// The opener that stands in while the real one is on the blocking pool, and for good after it panicked.
fn opener_lost() -> Box<Opener> {
    Box::new(|port: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> {
        Err(format!("the opener panicked; {port} not opened"))
    })
}

impl PumpDesk {
    /// The running pump's dialect (the redial hands it to the opener).
    fn running_dialect(&self) -> Option<Dialect> {
        self.running.as_ref().map(|r| r.pump.dialect())
    }
}

// --- VUGCOMSCANJOG (LEDGER SR573): home and jog on the run's link, answered by name; the commission said as its own word (named tail hunk) ---

/// An opener's answer after the hello ([`with_hello_named`]): the link, the identity the run keeps (a commission's
/// readback, else the answer) and the answer as the controller first said it, `Some` only when a commission acked and was
/// read back. VUGCOMSCANALARMIDLE (SR591): the fourth slot is the controller's travel ([`with_envelope`]), `Some` only when
/// the read is on and answered.
pub type Opened = (Box<dyn Transport + Send>, Option<String>, Option<String>, Option<Envelope>);

/// The wait for the link to take one home / jog line (a few bytes; the answer is read by [`PumpDesk::tick`]).
pub const MOTION_WAIT: Duration = Duration::from_millis(100);

/// One home / jog line sent and not yet answered.
struct Motion {
    /// `home` or `jog`.
    word: &'static str,
    /// The verb's last line: its `ok` names the verb done.
    last: bool,
    /// An earlier line of the verb was refused: this line's answer is claimed silently.
    failed: bool,
}

/// What [`PumpDesk::motion_replies`] does with one event.
enum Claim {
    Ok,
    Refused(String),
    Drop,
    Pass,
}

/// The name a first bind commissioned: the readback's name when the commission acked (`before` set) and it is not empty.
fn commissioned_name(d: Dialect, before: Option<&str>, said: Option<&str>) -> Option<String> {
    before?;
    pump_core::pump::hello_name(d, said?).filter(|n| !n.is_empty()).map(str::to_string)
}

impl PumpDesk {
    /// PumpHome / PumpJog: the dialect's lines (`pump_core::pump::home_lines` / `jog_lines`) on the running link through
    /// [`send_line`], each answered through [`PumpDesk::tick`]. Allowed with the link up and nothing of a job in flight
    /// (halted, stopped, done, lost and relinked); refused by name when nothing is linked, busy on a job, the jog's numbers
    /// cannot be said, the dialect has no verb, or the link is down.
    fn motion(&mut self, ev: &PumpEvent) -> Vec<PumpEvent> {
        let Some(r) = self.running.as_mut() else {
            return refused("nothing is linked: home and jog ride the link a PumpStart opened");
        };
        let st = r.pump.state();
        if matches!(st, PumpState::Streaming | PumpState::Stopping | PumpState::Rearmed { .. }) || r.pump.in_flight() > 0 {
            return refused(format!("busy: {} (home and jog wait for the job: PumpStop first)", state_word(st)));
        }
        let d = r.pump.dialect();
        let (word, lines) = match ev {
            PumpEvent::PumpHome { .. } => ("home", pump_core::pump::home_lines(d)),
            PumpEvent::PumpJog { dx, dy, dz, feed, .. } => {
                if ![*dx, *dy, *dz, *feed].iter().all(|v| v.is_finite()) {
                    return refused(format!("jog: not a number in X{dx} Y{dy} Z{dz} F{feed}"));
                }
                if *feed <= 0.0 {
                    return refused(format!("jog: feed {feed} is not above zero"));
                }
                if *dx == 0.0 && *dy == 0.0 && *dz == 0.0 {
                    return refused("jog: no axis moves");
                }
                if let Some(why) = jog_past_travel(&self.machine, self.read, self.pos.at, [*dx, *dy, *dz]) { return refused(why); } // VUGCOMSCANALARMRESET (SR585): VUG-COMSCAN-JOG-ENVELOPE; VUGCOMSCANENVELOPEBOUND (SR602): + the kept read
                ("jog", pump_core::pump::jog_lines(d, [*dx, *dy, *dz], *feed))
            }
            _ => return Vec::new(),
        };
        let Some(lines) = lines else {
            return refused(format!("{word}: no {word} verb in {}", dialect_word(d)));
        };
        if matches!(d, Dialect::Grbl { .. }) && lines.iter().any(|l| l.len() + 1 > pump_core::pump::GRBL_RX) {
            return refused(format!("{word}: a line over grbl's {} bytes", pump_core::pump::GRBL_RX));
        }
        let attempt = r.redial.map(|x| x.attempt()).unwrap_or(0);
        let Some(link) = r.link.as_mut() else {
            return refused(format!("{word}: the link is down (reconnect attempt {attempt})"));
        };
        for (k, l) in lines.iter().enumerate() {
            if !send_line(&mut **link, l, MOTION_WAIT) {
                return refused(format!("{word}: the link would not take {l:?}"));
            }
            r.motion.push_back(Motion { word, last: k + 1 == lines.len(), failed: false });
        }
        r.jogged = word == "jog"; // VUGCOMSCANJOGFACE (SR579): set on a jog, cleared on a home
        let first = self.position_moved(word == "jog", matches!(d, Dialect::Grbl { .. })); // VUGCOMSCANJOGPOSITION (SR612): the fallback word once per link; the axis moves, so the position is asked again
        first.into_iter().chain(std::iter::once(format!("{word} sent: {}", lines.join(" / ")))).map(|w| self.squawk(Some(w))).collect()
    }

    /// [`PumpDesk::tick`]'s events with the home / jog answers claimed, oldest line first: an `ok` (grbl `ok`, Marlin
    /// `ok...`, which the pump reads as a message with nothing of the job in flight) answers one line and the verb's last
    /// names it `<verb> ok`; a grbl `error:N` or a Marlin `Error:` names it `<verb> error:N <name>` / `<verb> refused: ..`
    /// and the verb's later lines are claimed silently; an alarm, a reset or a lost link drops what is pending. The
    /// unclaimed events go on to the Squawk words unchanged.
    fn motion_replies(&mut self, evs: Vec<Event>) -> (Vec<Event>, Vec<String>) {
        let Some(r) = self.running.as_mut() else { return (evs, Vec::new()) };
        if r.motion.is_empty() {
            return (evs, Vec::new());
        }
        let mut keep = Vec::new();
        let mut words = Vec::new();
        for e in evs {
            let pending = !r.motion.is_empty();
            let claim = match &e {
                Event::Message(s) if pending && s.starts_with("ok") => Claim::Ok,
                Event::Error { code, name, .. } if pending && *code != 9 => Claim::Refused(format!("error:{code} {name}")), // VUGCOMSCANJOGALARM (SR579): error:9 answers only a G-code line (a stream line's late answer after an alarm), never `$H` / `$J=`
                Event::Refused { text, .. } if pending => Claim::Refused(format!("refused: {text}")),
                Event::Alarm { .. } | Event::Reset | Event::LinkLost => Claim::Drop,
                _ => Claim::Pass,
            };
            match claim {
                Claim::Ok => {
                    if let Some(m) = r.motion.pop_front() {
                        if m.last && !m.failed {
                            words.push(if m.word == "unlock" { "unlocked".to_string() } else { format!("{} ok", m.word) }); // VUGCOMSCANALARMRESET (SR585)
                        }
                    }
                }
                Claim::Refused(why) => {
                    if let Some(m) = r.motion.pop_front() {
                        if !m.failed {
                            words.push(format!("{} {why}", m.word));
                        }
                        if !m.last {
                            for later in r.motion.iter_mut() {
                                later.failed = true;
                                if later.last {
                                    break;
                                }
                            }
                        }
                    }
                }
                Claim::Drop => {
                    r.motion.clear();
                    keep.push(e);
                }
                Claim::Pass => keep.push(e),
            }
        }
        (keep, words)
    }
}

// --- VUGCOMSCANJOGFACE (LEDGER SR579): Hold / Stop while a grbl jog runs is grbl's jog cancel (named tail hunk) ---

impl PumpDesk {
    /// PumpHold / PumpStop on a grbl run that `jogged` with nothing of a job in flight (the motion guard's own test):
    /// 0x85 through [`pump_core::Pump::jog_cancel`] (the realtime road), the pending home / jog lines dropped, `jogged`
    /// cleared, answered `jog cancelled`; no feed hold and no stop follow (a jog runs only with no job in flight, so
    /// there is nothing else to hold; grbl ignores 0x85 with no jog running). `None`: the request is not a cancel
    /// (Resume, Marlin, no jog sent, a job in flight) and the Hold / Resume / Stop arm runs as before.
    fn jog_cancel_due(&mut self, ev: &PumpEvent) -> Option<Vec<PumpEvent>> {
        if !matches!(ev, PumpEvent::PumpHold { .. } | PumpEvent::PumpStop { .. }) {
            return None;
        }
        let r = self.running.as_mut()?;
        let st = r.pump.state();
        if !r.jogged
            || !matches!(r.pump.dialect(), Dialect::Grbl { .. })
            || matches!(st, PumpState::Streaming | PumpState::Stopping | PumpState::Rearmed { .. })
            || r.pump.in_flight() > 0
        {
            return None;
        }
        let Some(link) = r.link.as_mut() else {
            return Some(refused("jog cancel: the link is down"));
        };
        if let Err(e) = r.pump.jog_cancel(&mut **link) {
            return Some(refused(format!("jog cancel: {e:?}")));
        }
        r.jogged = false;
        r.motion.clear();
        Some(vec![self.squawk(Some("jog cancelled".to_string()))])
    }
}

// --- VUGCOMSCANALARMRESET (LEDGER SR585): Stop on an alarm is the soft reset at once, the unlock after its banner; a jog bounded by the declared machine (named tail hunk) ---

impl PumpDesk {
    /// PumpStop on a run halted by an alarm (grbl reports `Alarm`, never Idle / Hold:0, and hears only the soft reset):
    /// [`pump_core::Pump::stop`] takes the core's reset road (0x18 at once), the unlock is due at the controller's
    /// banner when the dialect has an unlock line, the pending home / jog lines, `jogged` and the redial cleared, answered
    /// `reset sent`. `None`: not a Stop, not an alarm halt, or the link is down (the Stop arm's Down road as before).
    fn alarm_reset_due(&mut self, ev: &PumpEvent) -> Option<Vec<PumpEvent>> {
        if !matches!(ev, PumpEvent::PumpStop { .. }) {
            return None;
        }
        let r = self.running.as_mut()?;
        if !matches!(r.pump.state(), PumpState::Halted(pump_core::Halt::Alarm { .. })) {
            return None;
        }
        let link = r.link.as_mut()?;
        if let Err(e) = r.pump.stop(&mut **link) {
            return Some(refused(format!("reset: {e:?}")));
        }
        let grbl = matches!(r.pump.dialect(), Dialect::Grbl { .. }); // VUGCOMSCANALARMIDLE (SR591): only grbl's reset says a banner to wait on
        r.unlock = grbl && pump_core::pump::unlock_line(r.pump.dialect()).is_some();
        r.jogged = false;
        r.motion.clear();
        r.redial = None;
        let word = if grbl { Some("reset sent".to_string()) } else { send_unlock(r) }; // VUGCOMSCANALARMIDLE (SR591): Marlin has no reset: its unlock at once
        Some(vec![self.squawk(Some(word.unwrap_or_else(|| "stop".to_string())))])
    }

    /// [`PumpDesk::tick`]'s events after the alarm reset: the controller's banner ([`pump_core::pump::banner_key`] in a
    /// message) sends the dialect's unlock line through [`send_line`], queued as the `unlock` verb so its `ok` is claimed
    /// by [`PumpDesk::motion_replies`] (said `unlocked`); said `unlock sent: <line>`. `None`: no unlock due, no banner
    /// in these events, or the link went away in them.
    fn unlock_after_banner(&mut self, evs: &[Event]) -> Option<String> {
        let r = self.running.as_mut()?;
        if !r.unlock || evs.contains(&Event::LinkLost) {
            return None;
        }
        let d = r.pump.dialect();
        let key = pump_core::pump::banner_key(d)?;
        if !evs.iter().any(|e| matches!(e, Event::Message(s) if s.starts_with(key))) {
            return None;
        }
        r.unlock = false;
        send_unlock(r) // VUGCOMSCANALARMIDLE (SR591): the one unlock writer
    }
}

/// VUG-COMSCAN-JOG-ENVELOPE: a jog's delta against the declared machine's axes (`Machine::axes`, the SR498 machine
/// behind the desk): an axis that moves and is not declared, or a jog past its travel (`max - min`), refused by name
/// before the wire. VUGCOMSCANENVELOPEBOUND (SR602): the bound is [`bound`], the tighter of the declared travel and the
/// controller's `read`, and the refusal names which (`declared` / `controller`). VUGCOMSCANJOGPOSITION (SR612): with the
/// controller's settled position `pos` (polled outside a job, [`PumpDesk::position`]) the jog's END is bounded: every jog
/// is G91 (`pump_core::pump::jog_lines`), so the end is `pos + delta`, refused when `|end|` passes the travel (both grbl
/// MPos frames: 0..-travel after a default homing, 0..+travel with the origin forced); with no position yet the delta is
/// bounded as before.
fn jog_past_travel(m: &Machine, read: Option<Envelope>, pos: Option<[f64; 3]>, delta: [f64; 3]) -> Option<String> {
    use make_core::AxisId;
    for (i, (id, c, v)) in [(AxisId::X, 'X', delta[0]), (AxisId::Y, 'Y', delta[1]), (AxisId::Z, 'Z', delta[2])].into_iter().enumerate() {
        if v == 0.0 {
            continue;
        }
        let Some((travel, which)) = bound(m, read, id, i) else { // VUGCOMSCANALARMIDLE (SR591): the one travel; VUGCOMSCANENVELOPEBOUND (SR602): through bound
            return Some(format!("jog: {c}{v} on an axis the declared machine does not have"));
        };
        if let Some(p) = pos.map(|p| p[i]) { // VUGCOMSCANJOGPOSITION (SR612): the end from the reported position
            if (p + v).abs() > travel {
                return Some(format!("jog: {c}{v} from {p} ends past the {which} {c} travel {travel}"));
            }
        } else if v.abs() > travel {
            return Some(format!(
                "jog: {c}{v} is past the {which} {c} travel {travel} (no position report yet: the jog's delta is bounded)"
            ));
        }
    }
    None
}

// --- VUGCOMSCANALARMIDLE (LEDGER SR591): the controller's travel read once at a first open and checked against the declared machine; the one unlock writer (named tail hunk) ---

/// The controller's own travel, X / Y / Z in mm (grbl `$130`-`$132`); `None` for an axis the answer did not carry.
pub type Envelope = [Option<f64>; 3];

/// Ask the controller its travel: its dialect's verb (`pump_core::pump::envelope_verb`: grbl `$$`, Marlin `M211`) sent once
/// through [`send_line`], the answer read through the one line reader to its end (`pump_core::pump::hello_line`: `ok` / a
/// refusal) and every axis a line carries kept (`pump_core::pump::envelope_setting`: grbl `$130`-`$132`, Marlin's soft
/// endstops `Max - Min`). `None`: no verb, the link would not take it,
/// the controller refused it, went silent for `wait` or closed.
pub fn envelope(link: &mut dyn Transport, dialect: Dialect, wait: Duration) -> Option<Envelope> {
    use pump_core::pump::{HelloLine, envelope_setting, envelope_verb, hello_line};
    let verb = envelope_verb(dialect)?;
    if !send_line(link, verb, wait) {
        return None;
    }
    let mut got: Envelope = [None; 3];
    let mut refused = false;
    let ended = lines_until(link, wait, |s| {
        for (g, v) in got.iter_mut().zip(envelope_setting(dialect, s)) {
            if v.is_some() {
                *g = v; // VUGCOMSCANENVELOPEREADON (SR597): a line may carry every axis (Marlin's M211)
            }
        }
        match hello_line(dialect, s) {
            HelloLine::End => true,
            HelloLine::Refused => {
                refused = true;
                true
            }
            HelloLine::Keep | HelloLine::Other => false,
        }
    });
    match ended {
        Ok(Some(_)) if !refused => Some(got),
        _ => None,
    }
}

/// An opener's answer after the hello with the travel read run on its link when `read` (dialect, wait) is set (the
/// fourth [`Opened`] slot); the slot `None` otherwise.
pub fn with_envelope(res: Result<Opened, String>, read: Option<(Dialect, Duration)>) -> Result<Opened, String> {
    res.map(|(mut l, said, before, _)| {
        let env = read.and_then(|(d, w)| envelope(&mut *l, d, w));
        (l, said, before, env)
    })
}

impl PumpDesk {
    /// Turn the travel read on (or off): every later PumpStart (and accepted relink, SR607) asks the controller its travel after the hello and says a
    /// travel that differs from the declared machine's once (`envelope differs axis=<a> declared=<d> controller=<c>`); the
    /// tighter of the declared and the read travel is the jog's bound ([`bound`], VUGCOMSCANENVELOPEBOUND SR602). VUGCOMSCANENVELOPEREADON (SR597): on by default wherever the hello is on (the
    /// bus, [`compose`]); `envelope_read(false)` is the off switch.
    pub fn envelope_read(&mut self, on: bool) {
        self.envelope = on;
    }

    /// The travel read for a start in `d` at the hello's wait; `None` while the read or the hello is off
    /// (VUGCOMSCANENVELOPEREADON SR597: the read rides the hello, so a hello-off desk sends nothing new).
    fn envelope_spec_for(&self, d: Dialect) -> Option<(Dialect, Duration)> {
        self.hello.filter(|_| self.envelope).map(|w| (d, w))
    }

    /// One word per axis both declared and read whose travel differs at grbl's printed resolution (three decimals), the
    /// numbers in make_core's one grammar (`make_core::gcode::num`).
    fn envelope_words(&self, env: Option<Envelope>) -> Vec<String> {
        use make_core::AxisId;
        let Some(env) = env else { return Vec::new() };
        let mut out = Vec::new();
        for (i, (id, c)) in [(AxisId::X, 'X'), (AxisId::Y, 'Y'), (AxisId::Z, 'Z')].into_iter().enumerate() {
            let (Some(declared), Some(controller)) = (axis_travel(&self.machine, id), env[i]) else { continue };
            if format!("{declared:.3}") == format!("{controller:.3}") {
                continue;
            }
            let (mut d, mut k) = (String::new(), String::new());
            make_core::gcode::num(&mut d, declared);
            make_core::gcode::num(&mut k, controller);
            out.push(format!("envelope differs axis={c} declared={d} controller={k}"));
        }
        out
    }
}

/// The one unlock writer: the dialect's unlock line ([`pump_core::pump::unlock_line`]) through [`send_line`], queued as the
/// `unlock` verb so its `ok` is claimed by [`PumpDesk::motion_replies`] (said `unlocked`); said `unlock sent: <line>`.
/// Called at grbl's banner after the reset and at once for Marlin (no reset). `None`: no unlock line or the link is down.
fn send_unlock(r: &mut Running) -> Option<String> {
    let line = pump_core::pump::unlock_line(r.pump.dialect())?;
    let link = r.link.as_mut()?;
    if !send_line(&mut **link, line, MOTION_WAIT) {
        return Some(format!("unlock: the link would not take {line:?}"));
    }
    r.motion.push_back(Motion { word: "unlock", last: true, failed: false });
    Some(format!("unlock sent: {line}"))
}

/// The declared machine's travel on axis `id` (`max - min`, the SR498 machine behind the desk); `None`: not declared.
fn axis_travel(m: &Machine, id: make_core::AxisId) -> Option<f64> {
    m.axes.iter().find(|a| a.id == id).map(|a| a.max - a.min)
}

/// VUGCOMSCANENVELOPEBOUND (SR602): the jog's bound on axis `id` (slot `i` of the controller's `read`): `None` when the
/// declared machine does not have the axis; the controller's travel (`"controller"`) when it was read, is finite, above
/// zero and strictly tighter than the declared one; else the declared travel (`"declared"`, a tie included).
fn bound(m: &Machine, read: Option<Envelope>, id: make_core::AxisId, i: usize) -> Option<(f64, &'static str)> {
    let declared = axis_travel(m, id)?;
    match read.and_then(|r| r[i]) {
        Some(c) if c.is_finite() && c > 0.0 && c < declared => Some((c, "controller")),
        _ => Some((declared, "declared")),
    }
}

// --- VUGCOMSCANMACHINEDECLARE (LEDGER SR613): the machine declared behind the desk's ports by name, on the bus (named tail hunk) ---

/// What a declare carries that make_core's `Machine` has no field for: the operator's name for it and the dialect it
/// speaks (`None`: any). The travel is the desk's `machine` itself ([`declared_machine`]), the one record every reader reads.
struct Declared {
    name: String,
    dialect: Option<Dialect>,
    /// VUGCOMSCANMACHINEDECLAREPREFS (SR619): the declare's own text `<name> x=.. y=.. z=..[ dialect=..]` (the said word's
    /// tail), the value kept in Principia's `comscan.machine` and parsed back through [`declare_request`].
    record: String,
}

/// `base` with its X / Y / Z travel declared (mm): an axis `base` has keeps its `min` and its feed / acceleration / rapid
/// and takes `max = min + travel`; an axis it lacks is added `0..travel` with the first axis' limits (else zero: a
/// declare says travel only); the envelope spans each axis. The digest moves with the travel (SR498: a pass must name it).
fn declared_machine(base: &Machine, travel: [f64; 3]) -> Machine {
    use make_core::{Aabb3, Axis, AxisId, Vec3};
    let mut m = base.clone();
    let (max_feed, max_accel, rapid) = base.axes.first().map(|a| (a.max_feed, a.max_accel, a.rapid)).unwrap_or((0.0, 0.0, 0.0));
    let ids = [AxisId::X, AxisId::Y, AxisId::Z];
    for (id, t) in ids.into_iter().zip(travel) {
        match m.axes.iter_mut().find(|a| a.id == id) {
            Some(a) => a.max = a.min + t,
            None => m.axes.push(Axis { id, min: 0.0, max: t, max_feed, max_accel, rapid }),
        }
    }
    let span = |id: AxisId| m.axes.iter().find(|a| a.id == id).map(|a| (a.min, a.max)).unwrap_or((0.0, 0.0));
    let ((x0, x1), (y0, y1), (z0, z1)) = (span(AxisId::X), span(AxisId::Y), span(AxisId::Z));
    m.envelope = Aabb3 { min: Vec3::new(x0, y0, z0), max: Vec3::new(x1, y1, z1) };
    m
}

impl PumpDesk {
    /// PumpDeclare: the machine behind the ports declared by name. Refused by name while a job is in flight ([`PumpDesk::busy`]:
    /// a lost run's re-arm is bound to the machine its pass named), for an empty name or one holding a space, a travel
    /// not finite or not above zero, or an unknown dialect. Else `machine` is rewritten ([`declared_machine`]: the next
    /// jog's bound, no relink; the controller's kept `read` still the tighter, SR602), the armings are dropped (their
    /// passes named the machine before, SR498) and said when there were any, and the declare is said back
    /// `machine declared <name> x=<x> y=<y> z=<z>[ dialect=<d>]`, then a kept read that differs, through [`PumpDesk::envelope_words`].
    /// VUGCOMSCANMACHINEDECLAREPREFS (SR619): kept past the pump in Principia's `comscan.machine` ([`declare_kept`]) and
    /// read back once at [`serve_pump`]'s start ([`machine_from_prefs`]); a name holding `=` is refused (it would not parse
    /// back through [`declare_request`]).
    fn declare(&mut self, ev: &PumpEvent) -> Vec<PumpEvent> {
        let PumpEvent::PumpDeclare { name, x, y, z, dialect, .. } = ev else { return Vec::new() };
        if self.busy() {
            return refused("declare: busy: a job is in flight (PumpStop first)");
        }
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return refused(format!("declare: the machine name {name:?} is empty or holds a space"));
        }
        if name.contains('=') {
            return refused(format!("declare: the machine name {name:?} holds '=' (it would not parse back)")); // VUGCOMSCANMACHINEDECLAREPREFS (SR619)
        }
        let travel = [*x, *y, *z];
        for (c, v) in ['x', 'y', 'z'].into_iter().zip(travel) {
            if !v.is_finite() || v <= 0.0 {
                return refused(format!("declare: {c}={v} is not a travel above zero"));
            }
        }
        let dialect = match dialect.as_deref() {
            None => None,
            Some(s) => match parse_dialect(s) {
                Some(d) => Some(d),
                None => return refused(format!("declare: unknown dialect {s:?} (grbl, grbl-laser, marlin)")),
            },
        };
        self.machine = declared_machine(&self.machine, travel);
        let dropped = self.armed.len();
        self.armed.clear();
        let mut record = name.clone(); // VUGCOMSCANMACHINEDECLAREPREFS (SR619): the record first; the word is `machine declared <record>`
        for (c, v) in ['x', 'y', 'z'].into_iter().zip(travel) {
            record.push_str(&format!(" {c}="));
            make_core::gcode::num(&mut record, v);
        }
        if let Some(d) = dialect {
            record.push_str(&format!(" dialect={}", dialect_word(d)));
        }
        let word = format!("machine declared {record}");
        self.declared = Some(Declared { name: name.clone(), dialect, record });
        let mut out = vec![self.squawk(Some(word))];
        if dropped > 0 {
            out.push(self.squawk(Some(format!("armed dropped: {dropped} (passed for the machine before)"))));
        }
        for w in self.envelope_words(self.read) {
            out.push(self.squawk(Some(w)));
        }
        out
    }

    /// The declared machine's name and dialect (`None`: nothing declared this session).
    pub fn declared(&self) -> Option<(&str, Option<Dialect>)> {
        self.declared.as_ref().map(|d| (d.name.as_str(), d.dialect))
    }

    /// A PumpStart in `d` against the declared dialect: the refusal by name when the declared machine speaks another.
    fn declared_dialect_refusal(&self, d: Dialect) -> Option<String> {
        let decl = self.declared.as_ref()?;
        let speaks = decl.dialect?;
        (speaks != d).then(|| {
            format!("dialect mismatch: the declared machine {} speaks {}, asked {}", decl.name, dialect_word(speaks), dialect_word(d))
        })
    }
}

/// The text form of the declare, `machine declare <name> x=<mm> y=<mm> z=<mm> [dialect=grbl|grbl-laser|marlin]`, to the
/// one bus request ([`PumpEvent::PumpDeclare`]): each key once, x / y / z required; a line that is not a declare, an
/// unknown key, a repeated key or a number that does not parse refused by name. The travel's sense (finite, above zero)
/// and the dialect word are the desk's to judge ([`PumpDesk::declare`]), so there is one validator.
pub fn declare_request(line: &str, principal: bandy::Origin) -> Result<PumpEvent, String> {
    let mut w = line.split_whitespace();
    if (w.next(), w.next()) != (Some("machine"), Some("declare")) {
        return Err(format!("not a declare: {line:?} (machine declare <name> x=<mm> y=<mm> z=<mm> [dialect=..])"));
    }
    let Some(name) = w.next().filter(|n| !n.contains('=')) else {
        return Err("declare: no machine name before the keys".to_string());
    };
    let mut xyz: [Option<f64>; 3] = [None; 3];
    let mut dialect: Option<String> = None;
    for kv in w {
        let Some((k, v)) = kv.split_once('=') else {
            return Err(format!("declare: {kv:?} is not key=value"));
        };
        let i = match k {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            "dialect" => {
                if dialect.replace(v.to_string()).is_some() {
                    return Err("declare: dialect given twice".to_string());
                }
                continue;
            }
            _ => return Err(format!("declare: unknown key {k:?} (x, y, z, dialect)")),
        };
        if xyz[i].is_some() {
            return Err(format!("declare: {k} given twice"));
        }
        let Ok(n) = v.parse::<f64>() else {
            return Err(format!("declare: {k}={v:?} is not a number"));
        };
        xyz[i] = Some(n);
    }
    let [Some(x), Some(y), Some(z)] = xyz else {
        return Err("declare: x, y and z are each required".to_string());
    };
    Ok(PumpEvent::PumpDeclare { principal, name: name.to_string(), x, y, z, dialect })
}

// --- VUGCOMSCANJOGPOSITION (LEDGER SR612): the position polled outside a job, the jog's END bounded (named tail hunk) ---

/// The desk's view of where the machine stands (one field, [`PumpDesk`]'s `pos`, under the desk lock).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Position {
    /// The last settled machine position per axis (X / Y / Z, mm) from an `Idle` report ([`idle_mpos`]).
    at: Option<[f64; 3]>,
    /// When the desk last asked `?` outside a job (`now_us`); the next ask waits `pump_core::pump::STATUS_EVERY_US`.
    asked_us: Option<u64>,
    /// The fallback word (`jog bound=distance: no position report yet`) was said on this link.
    said: bool,
}

/// A report's machine position when the machine is settled: `Idle` only (grbl answers `$J=` when planned, so a Jog /
/// Run / Home / Alarm report is a moving or untrusted axis); `MPos` (grbl `$10=1`, the default), else `WPos + WCO` from
/// the same report (`$10=0`); else `None`.
fn idle_mpos(r: &StatusReport) -> Option<[f64; 3]> {
    if !matches!(r.state, pump_core::State::Idle) {
        return None;
    }
    r.mpos.or_else(|| {
        let (w, o) = (r.wpos?, r.wco?);
        Some([w[0] + o[0], w[1] + o[1], w[2] + o[2]])
    })
}

impl PumpDesk {
    /// [`PumpDesk::tick`]'s events and turn: a status report sets the settled position ([`idle_mpos`]: a moving or
    /// alarmed report forgets it), an alarm or reset forgets it, a lost link forgets it all (the fallback word is said
    /// again on the next link); then on a grbl link that is up with no job live (a live job's pump polls itself) and no
    /// settled position, `?` through the one writer ([`pump_core::Pump::status`]) at most every
    /// `pump_core::pump::STATUS_EVERY_US`. A dead link is the next tick's `LinkLost`.
    fn position_seen(&mut self, evs: &[Event], now_us: u64) {
        for e in evs {
            match e {
                Event::Status(r) => self.pos.at = idle_mpos(r),
                Event::Alarm { .. } | Event::Reset => self.pos.at = None,
                Event::LinkLost => self.pos = Position::default(),
                _ => {}
            }
        }
        if self.pos.at.is_some() || self.pos.asked_us.is_some_and(|t| now_us.saturating_sub(t) < pump_core::pump::STATUS_EVERY_US) {
            return;
        }
        let Some(r) = self.running.as_mut() else { return };
        if matches!(r.pump.state(), PumpState::Streaming | PumpState::Stopping) || !matches!(r.pump.dialect(), Dialect::Grbl { .. }) {
            return;
        }
        let Some(link) = r.link.as_mut() else { return };
        self.pos.asked_us = Some(now_us);
        let _ = r.pump.status(&mut **link);
    }

    /// A home or jog was sent: the fallback word when it is a grbl jog bounded by its distance (no settled position)
    /// and not yet said on this link; the axis moves, so the position is forgotten and asked again at the next tick.
    fn position_moved(&mut self, jog: bool, grbl: bool) -> Option<String> {
        let word = (jog && grbl && self.pos.at.is_none() && !self.pos.said).then(|| "jog bound=distance: no position report yet".to_string());
        self.pos.said |= word.is_some();
        self.pos.at = None;
        self.pos.asked_us = None;
        word
    }

    /// The controller's last settled machine position (X / Y / Z, mm), polled outside a job; `None` until an `Idle`
    /// report on this link, and after a home or jog until the next one.
    pub fn position(&self) -> Option<[f64; 3]> {
        self.pos.at
    }
}

// --- VUGCOMSCANMACHINEDECLAREPREFS (LEDGER SR619): the declared machine kept in Principia's store across a pump restart (named tail hunk) ---

/// How long [`serve_pump`]'s start waits for Principia's answer to the one `comscan.machine` PrefGet; no answer in time
/// (no Principia on the bus) is silence: the desk stays undeclared, as before SR619.
pub const PREFS_WAIT: Duration = Duration::from_millis(250);

impl PumpDesk {
    /// The declared machine's record, the declare's own text `<name> x=.. y=.. z=..[ dialect=..]` (`None`: nothing
    /// declared): the value [`declare_kept`] writes to Principia's `comscan.machine`.
    pub fn declared_record(&self) -> Option<&str> {
        self.declared.as_ref().map(|d| d.record.as_str())
    }

    /// The machine Principia's store kept (`record`, the declare's own text) declared again through the ONE parser
    /// ([`declare_request`]) and the ONE validator ([`PumpDesk::declare`]); the first word or the refusal is said with
    /// ` (from prefs)`. Nothing is written back.
    pub fn declare_from_prefs(&mut self, record: &str) -> Vec<PumpEvent> {
        let ev = match declare_request(&format!("machine declare {record}"), bandy::Origin::System("principia".to_string())) {
            Ok(ev) => ev,
            Err(why) => return refused(format!("{why} (from prefs)")),
        };
        let mut out = self.declare(&ev);
        if let Some(PumpEvent::Squawk { event: Some(w), .. } | PumpEvent::PumpRefused { why: w }) = out.first_mut() {
            w.push_str(" (from prefs)");
        }
        out
    }
}

/// [`serve_pump`]'s start: ask Principia ONCE for `comscan.machine` on its own subscriber (so the loop's receiver keeps
/// every bus request, served after this read), wait at most [`PREFS_WAIT`] for the answer, and declare a kept record
/// ([`PumpDesk::declare_from_prefs`]), its words on the bus. Unset, not a string or unanswered: silent, undeclared.
/// Never re-read after (PrefChanged is not heard).
async fn machine_from_prefs(synapse: &Synapse, desk: &Arc<Mutex<PumpDesk>>) {
    let mut rx = synapse.subscribe();
    synapse.fire(SMessage::Principia(bandy::PrincipiaCommand::PrefGet { ns: "comscan".to_string(), key: "machine".to_string() }));
    let answer = tokio::time::timeout(PREFS_WAIT, async {
        loop {
            match rx.recv().await {
                Ok(SMessage::Principia(bandy::PrincipiaCommand::PrefValueIs { ns, key, value })) if ns == "comscan" && key == "machine" => {
                    return value;
                }
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    })
    .await;
    let Ok(Some(bandy::PrefValue::Str(record))) = answer else { return };
    let out = match desk.lock() {
        Ok(mut d) => d.declare_from_prefs(&record),
        Err(_) => return,
    };
    for o in out {
        synapse.fire(SMessage::Pump(o));
    }
}

/// [`serve_pump`]'s request arm: a PumpDeclare the desk accepted (no PumpRefused in its answers) is written to
/// Principia's store as `comscan.machine` = the declared record, so the next pump start reads it back.
fn declare_kept(synapse: &Synapse, desk: &Mutex<PumpDesk>, ev: &PumpEvent, out: &[PumpEvent]) {
    if !matches!(ev, PumpEvent::PumpDeclare { .. }) || out.iter().any(|o| matches!(o, PumpEvent::PumpRefused { .. })) {
        return;
    }
    let Some(record) = desk.lock().ok().and_then(|d| d.declared_record().map(str::to_string)) else { return };
    synapse.fire(SMessage::Principia(bandy::PrincipiaCommand::PrefSet { ns: "comscan".to_string(), key: "machine".to_string(), value: bandy::PrefValue::Str(record) }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bandy::Origin;
    use make_core::{
        Aabb3, Ask, Axis, AxisId, Kinematics, MachineKind, Move, Power, PowerKind, Tool, ToolKind, Units, Vec3,
    };
    use pump_core::LinkError;
    use pump_core::mock::MockGrbl;

    const GRBL: Dialect = Dialect::Grbl { laser: false };

    /// The mock behind a lock, so the test reads what reached the wire after the desk took the link.
    #[derive(Clone)]
    struct Shared(Arc<Mutex<MockGrbl>>);

    impl Transport for Shared {
        fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
            self.0.lock().unwrap().write(b)
        }
        fn read(&mut self, b: &mut [u8]) -> Result<usize, LinkError> {
            self.0.lock().unwrap().read(b)
        }
    }

    fn program() -> Program {
        let mut p = Program::new(Units::Mm);
        p.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) });
        for i in 0..20 {
            p.moves.push(Move::Linear { to: Vec3::new(i as f64, (i * 2) as f64, -1.0), feed: 300.0 });
        }
        p
    }

    fn axis(id: AxisId, min: f64, max: f64) -> Axis {
        Axis { id, min, max, max_feed: 3000.0, max_accel: 100.0, rapid: 6000.0 }
    }

    /// The desk's machine: a mill whose envelope holds `program()` (VUG-HELM-BIND-MACHINE, SR498).
    fn machine() -> Machine {
        Machine {
            kind: MachineKind::Mill3,
            kinematics: Kinematics::Cartesian,
            axes: vec![axis(AxisId::X, -50.0, 300.0), axis(AxisId::Y, -50.0, 300.0), axis(AxisId::Z, -50.0, 60.0)],
            power: Power { kind: PowerKind::Spindle, max: 24000.0, ask_above: 18000.0 },
            envelope: Aabb3 { min: Vec3::new(-20.0, -20.0, -20.0), max: Vec3::new(300.0, 300.0, 50.0) },
            safe_z: 5.0,
            units: Units::Mm,
            tools: vec![Tool { number: 1, kind: ToolKind::FlatEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 3, sig: 0.0, ramp_deg: None, centre_cutting: None }],
            stock: None,
            fixtures: Vec::new(),
            loaded: Some(1),
        }
    }

    /// A pass for `p` on the desk's machine.
    fn pass(p: &Program) -> Verdict {
        Verdict::Pass { digest: p.digest(), machine: machine().digest() }
    }

    fn who() -> Origin {
        Origin::LocalUser("una".to_string())
    }

    fn start_msg(digest: u64, dialect: &str) -> PumpEvent {
        PumpEvent::PumpStart { principal: who(), program: digest_hex(digest), dialect: dialect.to_string(), port: "mock".to_string() }
    }

    fn why(out: &[PumpEvent]) -> String {
        match out {
            [PumpEvent::PumpRefused { why }] => why.clone(),
            other => panic!("expected one PumpRefused, got {other:?}"),
        }
    }

    #[test]
    fn only_a_pass_makes_a_pump() {
        let p = program();
        assert_eq!(start(&Verdict::Ask(vec![Ask::FirstRun { digest: p.digest() }]), &p, GRBL, &machine()).err(), Some(Refused::Asked(1)));
        let never = Verdict::Refuse { refusals: vec![], never: vec![Ask::ToolChange { at: 0, tool: 2 }] };
        assert_eq!(start(&never, &p, GRBL, &machine()).err(), Some(Refused::RefusedByHelm { refusals: 0, never: 1 }));
        assert!(start(&pass(&p), &p, GRBL, &machine()).is_ok());
        assert_eq!(
            start(&pass(&p), &p, Dialect::LinuxCnc, &machine()).err(),
            Some(Refused::Pump(PumpRefusal::DialectNotStreamed { dialect: Dialect::LinuxCnc }))
        );
        let mut desk = PumpDesk::new(machine());
        assert_eq!(desk.arm(&never, &p, GRBL), Err(Refused::RefusedByHelm { refusals: 0, never: 1 }));
        assert!(desk.armed().is_empty());
    }

    // VUG-HELM-BIND (SR489): a pass names its program; it never starts another.
    #[test]
    fn a_pass_for_one_program_refuses_another_by_name() {
        let a = program();
        let mut b = program();
        b.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 10.0) });
        assert_ne!(a.digest(), b.digest());
        let pass_a = pass(&a);
        let unbound = Refused::Unbound { passed: Some(a.digest()), program: b.digest() };
        assert_eq!(start(&pass_a, &b, GRBL, &machine()).err(), Some(unbound.clone()));
        let mut desk = PumpDesk::new(machine());
        assert_eq!(desk.arm(&pass_a, &b, GRBL), Err(unbound));
        assert!(desk.armed().is_empty(), "a pass for another program arms nothing");
        assert_eq!(
            start(&Verdict::Unbound, &a, GRBL, &machine()).err(),
            Some(Refused::Unbound { passed: None, program: a.digest() })
        );
        assert!(start(&pass_a, &a, GRBL, &machine()).is_ok(), "the pass still starts the program it names");
        assert_eq!(desk.arm(&pass_a, &a, GRBL), Ok(a.digest()));
    }

    // VUG-HELM-BIND-MACHINE (SR498): a desk armed for one machine refuses a pass for another, by both identities.
    #[test]
    fn a_desk_armed_for_one_machine_refuses_a_pass_for_another_by_name() {
        let p = program();
        let a = machine();
        let mut b = machine();
        b.envelope.max = Vec3::new(100.0, 100.0, 50.0);
        assert_ne!(a.digest(), b.digest());
        let pass_b = Verdict::Pass { digest: p.digest(), machine: b.digest() };
        let wrong = Refused::WrongMachine { passed: b.digest(), desk: a.digest() };
        assert_eq!(start(&pass_b, &p, GRBL, &a).err(), Some(wrong.clone()));
        let mut desk = PumpDesk::new(a.clone());
        assert_eq!(desk.machine(), &a);
        assert_eq!(desk.arm(&pass_b, &p, GRBL), Err(wrong));
        assert!(desk.armed().is_empty(), "a pass for another machine arms nothing");
        // The program check comes first: a pass for another program on another machine is Unbound.
        let mut q = program();
        q.moves.push(Move::Rapid { to: Vec3::new(0.0, 0.0, 10.0) });
        assert_eq!(
            start(&pass_b, &q, GRBL, &a).err(),
            Some(Refused::Unbound { passed: Some(p.digest()), program: q.digest() })
        );
        // The pass naming the desk's machine still arms, and the same pass on B's own desk does too.
        assert_eq!(desk.arm(&pass(&p), &p, GRBL), Ok(p.digest()));
        assert_eq!(PumpDesk::new(b).arm(&pass_b, &p, GRBL), Ok(p.digest()));
    }

    #[test]
    fn the_bus_starts_only_an_armed_program_and_streams_it_to_done() {
        let p = program();
        let mock = Shared(Arc::new(Mutex::new(MockGrbl::new(2))));
        let m2 = mock.clone();
        let mut open = move |port: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> {
            if port == "mock" { Ok(Box::new(m2.clone())) } else { Err("no such port".to_string()) }
        };
        let mut desk = PumpDesk::new(machine());

        // Nothing armed: refused by name, whatever the sender claims.
        assert!(why(&desk.apply(&start_msg(p.digest(), "grbl"), &mut open)).contains("is not armed"));

        let digest = desk.arm(&pass(&p), &p, GRBL).unwrap();
        assert_eq!(digest, p.digest());
        assert!(why(&desk.apply(&start_msg(digest, "marlin"), &mut open)).contains("dialect mismatch"));
        assert!(why(&desk.apply(&start_msg(digest, "fanuc"), &mut open)).contains("unknown dialect"));
        let mut bad_port = start_msg(digest, "grbl");
        if let PumpEvent::PumpStart { port, .. } = &mut bad_port {
            *port = "ttyX".to_string();
        }
        assert!(why(&desk.apply(&bad_port, &mut open)).contains("no such port"));
        assert_eq!(desk.armed(), vec![digest], "a refused start leaves the arming");

        let out = desk.apply(&start_msg(digest, "grbl"), &mut open);
        assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), total, .. }] if e == "started" && *total > 20));
        assert!(desk.armed().is_empty(), "one arming, one run");
        assert!(why(&desk.apply(&start_msg(digest, "grbl"), &mut open)).contains("busy"));

        let mut last = None;
        for k in 1..5_000u64 {
            for s in desk.tick(k * 1000) {
                last = Some(s);
            }
            if matches!(&last, Some(PumpEvent::Squawk { event: Some(e), .. }) if e == "done") {
                break;
            }
        }
        match last {
            Some(PumpEvent::Squawk { sent, acked, total, state, event: Some(e), .. }) => {
                assert_eq!((e.as_str(), state.as_str()), ("done", "done"));
                assert_eq!((sent, acked), (total, total));
            }
            other => panic!("no done squawk: {other:?}"),
        }
        let m = mock.0.lock().unwrap();
        assert!(!m.overflowed);
        let want = Pump::from_program(&p, GRBL).unwrap().lines().to_vec();
        assert_eq!(m.executed, want, "the controller ran make_core's text, line for line");

        // Done: a second run needs a second pass.
        drop(m);
        assert!(why(&desk.apply(&start_msg(digest, "grbl"), &mut open)).contains("is not armed"));
    }

    #[test]
    fn hold_puts_the_feed_hold_byte_on_the_wire_and_stop_resets_after_it() {
        let p = program();
        let mock = Shared(Arc::new(Mutex::new(MockGrbl::new(5))));
        let m2 = mock.clone();
        let mut open = move |_: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> { Ok(Box::new(m2.clone())) };
        let mut desk = PumpDesk::new(machine());
        assert_eq!(why(&desk.apply(&PumpEvent::PumpHold { principal: who() }, &mut open)), "nothing is streaming");
        let d = desk.arm(&pass(&p), &p, GRBL).unwrap();
        desk.apply(&start_msg(d, "grbl"), &mut open);
        desk.tick(1000);
        let out = desk.apply(&PumpEvent::PumpHold { principal: who() }, &mut open);
        assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }] if e == "hold"));
        assert!(mock.0.lock().unwrap().realtime.contains(&b'!'));
        desk.apply(&PumpEvent::PumpResume { principal: who() }, &mut open);
        assert!(mock.0.lock().unwrap().realtime.contains(&b'~'));
        desk.apply(&PumpEvent::PumpStop { principal: who() }, &mut open);
        let mut stopped = false;
        for k in 2..500u64 {
            if desk.tick(k * 1000).iter().any(|s| matches!(s, PumpEvent::Squawk { event: Some(e), .. } if e == "stopped")) {
                stopped = true;
                break;
            }
        }
        assert!(stopped);
        assert!(mock.0.lock().unwrap().realtime.contains(&0x18));
        // Comscan's own answers are inert as input.
        assert!(desk.apply(&PumpEvent::PumpRefused { why: "x".to_string() }, &mut open).is_empty());
    }

    #[test]
    fn status_text_is_grbls_grammar() {
        let r = pump_core::parse_status("<Hold:0|MPos:1.000,2.000,3.000|FS:500,8000|Ov:100,100,100>").unwrap();
        assert_eq!(status_text(&r), "<Hold:0|MPos:1.000,2.000,3.000|FS:500,8000|Ov:100,100,100>");
    }

    #[tokio::test]
    async fn serve_pump_answers_on_the_bus() {
        let syn = Synapse::new();
        let rx = syn.subscribe();
        let mut watch = syn.subscribe();
        let desk = Arc::new(Mutex::new(PumpDesk::new(machine())));
        let open: Box<Opener> = Box::new(|_: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> { Err("none".to_string()) });
        let task = tokio::spawn(serve_pump(syn.clone(), rx, desk, open, Duration::from_millis(5)));
        syn.fire(SMessage::Pump(PumpEvent::PumpStop { principal: who() }));
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(SMessage::Pump(PumpEvent::PumpRefused { why })) = watch.recv().await {
                    return why;
                }
            }
        })
        .await
        .expect("an answer");
        assert_eq!(got, "nothing is streaming");
        syn.fire(SMessage::Kill("comscan".to_string()));
        tokio::time::timeout(Duration::from_secs(5), task).await.expect("stops on Kill").unwrap();
    }

    // --- VUGCOMSCANHELLOON (SR549): the bus's PumpStart opens and says hello with the desk lock released ---

    /// Whether the desk's lock is free right now (`false`: held, or the desk is gone).
    fn free_now(desk: &std::sync::Weak<Mutex<PumpDesk>>) -> bool {
        match desk.upgrade() {
            Some(d) => {
                let free = d.try_lock().is_ok();
                free
            }
            None => false,
        }
    }

    /// The Shared mock, recording on every write whether the desk lock was free.
    struct Probe {
        link: Shared,
        desk: std::sync::Weak<Mutex<PumpDesk>>,
        seen: Arc<Mutex<Vec<bool>>>,
    }

    impl Transport for Probe {
        fn write(&mut self, b: &[u8]) -> Result<usize, LinkError> {
            let free = free_now(&self.desk);
            self.seen.lock().unwrap().push(free);
            self.link.write(b)
        }
        fn read(&mut self, b: &mut [u8]) -> Result<usize, LinkError> {
            self.link.read(b)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)] // VUGCOMSCANHOSTDIALECT (SR566): request is async
    async fn the_bus_start_opens_and_says_hello_with_the_desk_lock_released() {
        let p = program();
        let desk = Arc::new(Mutex::new(PumpDesk::new(machine())));
        let digest = {
            let mut d = desk.lock().unwrap();
            d.hello_within(Duration::from_millis(50));
            d.arm(&pass(&p), &p, GRBL).unwrap()
        };
        let seen: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
        let mock = Shared(Arc::new(Mutex::new(MockGrbl::new(2))));
        let (weak, probe_seen) = (Arc::downgrade(&desk), seen.clone());
        let mut open: Box<Opener> = Box::new(move |_: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> {
            probe_seen.lock().unwrap().push(free_now(&weak));
            Ok(Box::new(Probe { link: mock.clone(), desk: weak.clone(), seen: probe_seen.clone() }))
        });
        let out = request(&desk, &start_msg(digest, "grbl"), &mut open).await; // the open on the blocking pool
        assert!(matches!(&out[..], [PumpEvent::Squawk { event: Some(e), .. }, ..] if e == "started"), "{out:?}");
        let probed = seen.lock().unwrap().clone();
        assert!(probed.len() >= 2, "the open and the hello's $I write were probed: {probed:?}");
        assert!(probed.iter().all(|f| *f), "the desk lock was free across the open and the hello: {probed:?}");
        assert!(desk.lock().unwrap().armed().is_empty(), "the handback took the arming under the lock");

        // Control: the probe sees a held lock.
        {
            let _held = desk.lock().unwrap();
            let _ = open("mock", GRBL);
        }
        assert_eq!(seen.lock().unwrap().last(), Some(&false), "the probe reads a held lock as held");
    }

    // --- VUGCOMSCANHOSTDESK (SR561): host_parts is compose over the host's links; the commissioned name's shape ---

    #[test]
    fn host_parts_is_the_one_composition() {
        let why = |r: Result<Box<dyn Transport + Send>, String>| r.err().unwrap_or_default();
        let (desk, mut open) = host_parts(machine());
        assert!(desk.identify.is_some(), "the identity hook is set");
        assert_eq!(desk.hello_spec_for(GRBL), Some((GRBL, HELLO_WAIT)), "the hello is on, so a first bind commissions");
        assert_eq!(desk.machine(), &machine());
        assert!(why(open("wss://127.0.0.1:1/", GRBL)).contains("VUG-COMSCAN-WSS"), "wss:// goes to the ws arm");
        if cfg!(target_os = "linux") {
            let e = why(open("/dev/comscan-hostdesk-no-such-port", GRBL));
            assert!(e.starts_with("no such port"), "a /dev path goes to the tty arm: {e}");
        }

        let arm = |name: &'static str| -> Box<Opener> {
            Box::new(move |port: &str, _: Dialect| -> Result<Box<dyn Transport + Send>, String> { Err(format!("{name} {port}")) })
        };
        let (d, mut open) = compose(machine(), Box::new(|_: &str| -> Option<String> { None }), arm("serial"), arm("tcp"), arm("ws"));
        assert_eq!(d.hello_spec_for(GRBL), desk.hello_spec_for(GRBL));
        assert_eq!(why(open("ws://h:81/", GRBL)), "ws ws://h:81/");
        assert_eq!(why(open("wss://h:443/", GRBL)), "ws wss://h:443/");
        assert_eq!(why(open("tcp://h:23", GRBL)), "tcp tcp://h:23");
        assert_eq!(why(open("/dev/ttyUSB0", GRBL)), "serial /dev/ttyUSB0");

        assert_eq!(commission_name(Some("tty:usb:0403:6001:a50285bi"), "/dev/ttyUSB0", 1), "UNA-A50285BI");
        assert_eq!(commission_name(Some("tty:usb:2341:0043:7573530303035161D0A1"), "/dev/ttyACM0", 1), "UNA-5161D0A1");
        let a = commission_name(Some("tty:usb:1a86:7523"), "/dev/ttyUSB0", 1);
        let b = commission_name(Some("tty:usb:1a86:7523"), "/dev/ttyUSB0", 2);
        assert!(a.len() == 12 && a.starts_with("UNA-") && a != b, "{a} {b}");
        assert!(a[4..].chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()), "{a}");
        assert_ne!(undeclared_machine().digest(), machine().digest(), "the undeclared machine is no real machine");

        // VUGCOMSCANENVELOPEREADON (SR597): the travel read rides the hello, on by default; envelope_read(false) the off switch.
        let (mut d, _) = compose(machine(), Box::new(|_: &str| -> Option<String> { None }), arm("serial"), arm("tcp"), arm("ws"));
        assert_eq!(d.envelope_spec_for(GRBL), Some((GRBL, HELLO_WAIT)), "the composed desk reads the travel");
        d.envelope_read(false);
        assert_eq!(d.envelope_spec_for(GRBL), None, "the off switch");
        assert_eq!(PumpDesk::new(machine()).envelope_spec_for(GRBL), None, "a hello-off desk sends nothing new");
    }
}
