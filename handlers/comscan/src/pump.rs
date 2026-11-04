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
pub type Opener = dyn FnMut(&str) -> Result<Box<dyn Transport + Send>, String> + Send;

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
}

fn refused(why: impl Into<String>) -> Vec<PumpEvent> {
    vec![PumpEvent::PumpRefused { why: why.into() }]
}

impl PumpDesk {
    /// A desk for `machine`: every pass it arms must name this machine's identity (VUG-HELM-BIND-MACHINE).
    pub fn new(machine: Machine) -> PumpDesk {
        PumpDesk { machine, armed: Vec::new(), running: None, identify: None, hello: None }
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
        open: &mut dyn FnMut(&str) -> Result<Box<dyn Transport + Send>, String>,
    ) -> Vec<PumpEvent> {
        match ev {
            PumpEvent::PumpStart { program, dialect, port, .. } => {
                if self.busy() {
                    return refused("busy: a program is streaming (PumpStop first)");
                }
                let Some(d) = parse_dialect(dialect) else {
                    return refused(format!("unknown dialect {dialect:?} (grbl, grbl-laser, marlin)"));
                };
                let Ok(digest) = u64::from_str_radix(program, 16) else {
                    return refused(format!("program {program:?} is not a digest (16 hex digits)"));
                };
                let Some(i) = self.armed.iter().position(|a| a.digest == digest) else {
                    return refused(format!(
                        "program {} is not armed (only Helm's Verdict::Pass arms a program, in process)",
                        digest_hex(digest)
                    ));
                };
                if self.armed[i].dialect != d {
                    return refused(format!(
                        "dialect mismatch: {} was armed for {}, asked {}",
                        digest_hex(digest),
                        dialect_word(self.armed[i].dialect),
                        dialect_word(d)
                    ));
                }
                let mut link = match open(port) {
                    Ok(l) => l,
                    Err(e) => return refused(format!("port {port:?}: {e}")),
                };
                // One arming, one run: a FirstRun approval is per run, so a finished program must pass Helm again.
                let a = self.armed.remove(i);
                let identity = self.identify.as_ref().and_then(|f| f(port.as_str())); // VUGCOMSCANTTYUSBID (SR529)
                let hello = self.hello_spec_for(a.dialect).and_then(|(d, w)| hello(&mut *link, d, w)); // VUGCOMSCANHELLOID (SR546)
                self.running =
                    Some(Running { pump: a.pump, link: Some(link), port: port.clone(), redial: None, identity, hello: hello.clone() });
                let mut out = vec![self.squawk(Some("started".to_string()))];
                if let Some(h) = hello {
                    out.push(self.squawk(Some(format!("hello {h}")))); // printed once, at the first open
                }
                out
            }
            PumpEvent::PumpHold { .. } | PumpEvent::PumpResume { .. } | PumpEvent::PumpStop { .. } => {
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
            PumpEvent::Squawk { .. } | PumpEvent::PumpRefused { .. } => Vec::new(),
        }
    }

    /// Drive the running stream one turn; a Squawk for each named event, else one for any traffic.
    pub fn tick(&mut self, now_us: u64) -> Vec<PumpEvent> {
        let evs = match self.running.as_mut() {
            Some(Running { pump, link: Some(link), .. }) => pump.tick(&mut **link, now_us),
            _ => return Vec::new(),
        };
        if evs.is_empty() {
            return Vec::new();
        }
        if evs.contains(&Event::LinkLost) {
            self.link_lost(now_us);
        }
        let named: Vec<String> =
            evs.iter().filter_map(event_word).map(|w| if w == "link lost" { self.lost_word() } else { w }).collect();
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
    let t0 = tokio::time::Instant::now();
    let mut iv = tokio::time::interval(every);
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(SMessage::Pump(ev)) if ev.is_request() => {
                    let out = match desk.lock() {
                        Ok(mut d) => d.apply(&ev, &mut *open),
                        Err(_) => refused("the pump desk is poisoned"),
                    };
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
                redial(&synapse, &desk, &mut *open, t0); // VUGCOMSCANRECONNECT (SR521): the reopen, outside the lock
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
    Box::new(move |port: &str| -> Result<Box<dyn Transport + Send>, String> {
        if port.starts_with(TCP_SCHEME) { net(port) } else { serial(port) }
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
    let key = match dialect {
        Dialect::Grbl { .. } => "Grbl ",
        Dialect::Marlin => "start",
        Dialect::LinuxCnc => return Ok(None),
    };
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
    Box::new(move |port: &str| -> Result<Box<dyn Transport + Send>, String> {
        if port.starts_with(WS_SCHEME) || port.starts_with(WSS_SCHEME) { ws(port) } else { rest(port) }
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
    /// run is still lost and link-less; otherwise the new link is dropped and nothing is said.
    pub fn relinked(&mut self, now_us: u64, res: Result<Box<dyn Transport + Send>, String>) -> Vec<PumpEvent> {
        let spec = self.hello_spec(); // VUGCOMSCANHELLOID (SR546): the reopened link says who it is first
        self.relinked_said(now_us, with_hello(res, spec))
    }

    /// [`PumpDesk::relinked`] with the reopened link's identity answer beside it (VUGCOMSCANHELLOID, SR546): an answer
    /// that differs from the one the first open kept refuses the relink by name and the redial goes on.
    pub fn relinked_said(
        &mut self,
        now_us: u64,
        res: Result<(Box<dyn Transport + Send>, Option<String>), String>,
    ) -> Vec<PumpEvent> {
        let Some(r) = self.running.as_mut() else { return Vec::new() };
        let PumpState::LinkLost { at_line, .. } = r.pump.state() else { return Vec::new() };
        let Some(mut d) = r.redial else { return Vec::new() };
        if r.link.is_some() {
            return Vec::new();
        }
        let word = match res {
            Ok((link, now)) if r.hello.is_some() && now != r.hello => {
                drop(link); // another board: its link (a tty's exclusive fd) goes at once
                let k = d.attempt();
                d.failed(now_us);
                r.redial = Some(d);
                let was = r.hello.clone().unwrap_or_default();
                format!("reconnect attempt {k} refused: {:?}; attempt {} in {} ms", Refused::AnotherBoard { was, now }, d.attempt(), d.wait_us() / 1000)
            }
            Ok((link, _)) => {
                r.link = Some(link);
                r.redial = None;
                format!("relinked on attempt {}; re-arm to resume from line {at_line}", d.attempt())
            }
            Err(why) => {
                let k = d.attempt();
                d.failed(now_us);
                r.redial = Some(d);
                format!("reconnect attempt {k} failed: {why}; attempt {} in {} ms", d.attempt(), d.wait_us() / 1000)
            }
        };
        vec![self.squawk(Some(word))]
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
fn redial(synapse: &Synapse, desk: &Mutex<PumpDesk>, open: &mut Opener, t0: tokio::time::Instant) {
    let now = t0.elapsed().as_micros() as u64;
    let (port, spec) = match desk.lock() {
        Ok(d) => (d.redial_due(now), d.hello_spec()),
        Err(_) => (None, None),
    };
    let Some(port) = port else { return };
    let res = with_hello(open(&port), spec); // VUGCOMSCANHELLOID (SR546): the hello too, with the lock released
    let now = t0.elapsed().as_micros() as u64;
    let out = match desk.lock() {
        Ok(mut d) => d.relinked_said(now, res),
        Err(_) => Vec::new(),
    };
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
    let mut bytes = verb.as_bytes().to_vec();
    bytes.push(b'\n');
    let end = std::time::Instant::now() + wait;
    let mut off = 0;
    while off < bytes.len() {
        match link.write(&bytes[off..]) {
            Ok(n) if n > 0 => off += n,
            Ok(_) if std::time::Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            Ok(_) | Err(_) => return None,
        }
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
        let mut open = move |port: &str| -> Result<Box<dyn Transport + Send>, String> {
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
        let mut open = move |_: &str| -> Result<Box<dyn Transport + Send>, String> { Ok(Box::new(m2.clone())) };
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
        let open: Box<Opener> = Box::new(|_: &str| -> Result<Box<dyn Transport + Send>, String> { Err("none".to_string()) });
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
}
