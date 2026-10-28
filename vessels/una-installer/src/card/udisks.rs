//! CHARTER: Amber Bytes — the card face's udisks2 elevation route (vessel module, child of card; seam shared-core amber_core + amber_bytes)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALLERUDISKS (LEDGER SR427): the second Linux arm of the card face's ONE elevation (`card::elevated_open`), taken
// only when pkexec is not installed. udisks2's `org.freedesktop.UDisks2.Block.OpenDevice (s mode, a{sv} options) -> h`
// over the system D-Bus: udisksd opens the node as root (mode "rw", option `flags` = O_EXCL) under polkit's
// `org.freedesktop.udisks2.open-device` (a removable / USB / SDIO disk has HintSystem false; a fixed disk would ask
// `open-device-system`, and Amber refuses it before any elevation) and answers the fd as SCM_RIGHTS on the reply,
// received by the card's ONE `recv_chunk`. The workspace locks no D-Bus crate (R83), so this is the minimal wire, by
// the D-Bus specification: SASL EXTERNAL with fd negotiation, `Hello`, the call, and a reader for the one reply.
//
// Object path (udisks2 `util_compute_object_path`): `/org/freedesktop/UDisks2/block_devices/<kernel name>`, every
// byte outside [A-Za-z0-9] escaped `_<2 lower hex>` (digits kept: `dm-0` -> `dm_2d0`, `mmcblk0` unchanged).
// Verdicts: an ERROR reply named `org.freedesktop.DBus.Error.ServiceUnknown` / `UnknownMethod` / `UnknownObject` /
// `UnknownInterface` (udisks2 absent or older than 2.7.3) and no bus = `elevation-unavailable`; any other error
// (NotAuthorized*, NotAuthorizedDismissed, Failed), end of stream, a malformed message or a return without its fd =
// `elevation-denied`.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::recv_chunk;

/// The D-Bus specification's default system bus address (`unix:path=` of it).
pub const DEFAULT_SYSTEM_BUS: &str = "/var/run/dbus/system_bus_socket";
const UNAVAILABLE: &str = "elevation-unavailable";
const DENIED: &str = "elevation-denied";
/// How long the reply may take: the polkit dialog is in front of the person.
const REPLY_WAIT: Duration = Duration::from_secs(300);
/// A message longer than this is malformed for our two calls.
const MAX_MESSAGE: usize = 1 << 20;

// Message types and header flags (D-Bus specification, "Message Format").
pub const METHOD_CALL: u8 = 1;
pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
#[cfg_attr(not(test), allow(dead_code))]
pub const SIGNAL: u8 = 4;
pub const ALLOW_INTERACTIVE_AUTHORIZATION: u8 = 0x4;

// Header field codes.
pub const PATH: u8 = 1;
pub const INTERFACE: u8 = 2;
pub const MEMBER: u8 = 3;
pub const ERROR_NAME: u8 = 4;
pub const REPLY_SERIAL: u8 = 5;
pub const DESTINATION: u8 = 6;
pub const SIGNATURE: u8 = 8;
pub const UNIX_FDS: u8 = 9;

/// The serial of the OpenDevice call (Hello is 1).
const OPEN_SERIAL: u32 = 2;

/// udisks2's object path for the block device `node` (its basename, escaped).
pub fn object_path(node: &Path) -> String {
    let name = node.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut s = String::from("/org/freedesktop/UDisks2/block_devices/");
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() {
            s.push(b as char);
        } else {
            s.push_str(&format!("_{b:02x}"));
        }
    }
    s
}

/// The system bus socket from a `DBUS_SYSTEM_BUS_ADDRESS` value: its first `unix:path=` entry; no value = the
/// specification's default; an address with no unix path (abstract, tcp) = `None`.
pub fn bus_from_address(addr: Option<&str>) -> Option<PathBuf> {
    let Some(addr) = addr else { return Some(PathBuf::from(DEFAULT_SYSTEM_BUS)) };
    addr.split(';').find_map(|entry| entry.strip_prefix("unix:")?.split(',').find_map(|kv| kv.strip_prefix("path=")).map(PathBuf::from))
}

/// This host's system bus socket.
pub fn system_bus() -> Option<PathBuf> {
    bus_from_address(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").ok().as_deref())
}

/// Ask udisks2 on the bus at `bus` to open `node` read-write and exclusively; the fd it returns.
pub fn open_device(bus: Option<&Path>, node: &Path) -> Result<File, &'static str> {
    let bus = bus.ok_or(UNAVAILABLE)?;
    let mut s = UnixStream::connect(bus).map_err(|_| UNAVAILABLE)?;
    let _ = s.set_read_timeout(Some(REPLY_WAIT));
    auth(&mut s)?;
    let hello = message(
        METHOD_CALL,
        0,
        1,
        &[
            (PATH, Val::O(String::from("/org/freedesktop/DBus"))),
            (INTERFACE, Val::S(String::from("org.freedesktop.DBus"))),
            (MEMBER, Val::S(String::from("Hello"))),
            (DESTINATION, Val::S(String::from("org.freedesktop.DBus"))),
        ],
        &[],
    );
    let call = message(
        METHOD_CALL,
        ALLOW_INTERACTIVE_AUTHORIZATION,
        OPEN_SERIAL,
        &[
            (PATH, Val::O(object_path(node))),
            (INTERFACE, Val::S(String::from("org.freedesktop.UDisks2.Block"))),
            (MEMBER, Val::S(String::from("OpenDevice"))),
            (DESTINATION, Val::S(String::from("org.freedesktop.UDisks2"))),
            (SIGNATURE, Val::G(String::from("sa{sv}"))),
        ],
        &open_device_body("rw", libc::O_EXCL),
    );
    s.write_all(&hello).and_then(|_| s.write_all(&call)).map_err(|_| DENIED)?;
    let (reply, fds) = read_reply(&s, OPEN_SERIAL)?;
    verdict(&reply, fds)
}

/// SASL EXTERNAL as this uid, then unix fd negotiation, then BEGIN. A bus that refuses either is no route.
fn auth(s: &mut UnixStream) -> Result<(), &'static str> {
    s.write_all(format!("\0AUTH EXTERNAL {}\r\n", uid_hex()).as_bytes()).map_err(|_| UNAVAILABLE)?;
    if !read_line(s)?.starts_with("OK ") {
        return Err(UNAVAILABLE);
    }
    s.write_all(b"NEGOTIATE_UNIX_FD\r\n").map_err(|_| UNAVAILABLE)?;
    if read_line(s)? != "AGREE_UNIX_FD" {
        return Err(UNAVAILABLE);
    }
    s.write_all(b"BEGIN\r\n").map_err(|_| UNAVAILABLE)
}

/// EXTERNAL's identity: the hex of this process's uid written in ASCII decimal (uid 1000 -> `31303030`).
fn uid_hex() -> String {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    uid.to_string().bytes().map(|b| format!("{b:02x}")).collect()
}

/// One `\r\n`-terminated auth line, without the terminator.
fn read_line(s: &mut UnixStream) -> Result<String, &'static str> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while !line.ends_with(b"\r\n") {
        if line.len() > 512 || s.read(&mut b).map_err(|_| UNAVAILABLE)? == 0 {
            return Err(UNAVAILABLE);
        }
        line.push(b[0]);
    }
    line.truncate(line.len() - 2);
    String::from_utf8(line).map_err(|_| UNAVAILABLE)
}

/// Read messages until the reply to `serial` (signals such as NameAcquired and the Hello return are skipped). The
/// fds arrive in order with the bytes; each message takes its UNIX_FDS count off the front of the queue.
fn read_reply(s: &UnixStream, serial: u32) -> Result<(Message, Vec<File>), &'static str> {
    let mut buf: Vec<u8> = Vec::new();
    let mut fds: VecDeque<File> = VecDeque::new();
    let mut chunk = vec![0u8; 4096];
    loop {
        if let Some((m, used)) = parse(&buf).map_err(|_| DENIED)? {
            buf.drain(..used);
            let mine: Vec<File> = (0..m.unix_fds).filter_map(|_| fds.pop_front()).collect();
            if m.reply_serial == Some(serial) && (m.ty == METHOD_RETURN || m.ty == ERROR) {
                return Ok((m, mine));
            }
            continue;
        }
        let (n, fd) = recv_chunk(s, &mut chunk).ok_or(DENIED)?;
        if n == 0 {
            return Err(DENIED);
        }
        buf.extend_from_slice(&chunk[..n]);
        fds.extend(fd);
    }
}

/// The reply decides: a return of signature `h` hands over the fd it indexes; an error is judged by its name.
fn verdict(m: &Message, mut fds: Vec<File>) -> Result<File, &'static str> {
    match m.ty {
        METHOD_RETURN if m.signature == "h" => {
            let i = m.body_u32().ok_or(DENIED)? as usize;
            if i < fds.len() { Ok(fds.swap_remove(i)) } else { Err(DENIED) }
        }
        ERROR => Err(why_of(m.error.as_deref().unwrap_or(""))),
        _ => Err(DENIED),
    }
}

/// An error name's verdict: the bus says udisks2 (or the method) is not there = no route; anything else = denied.
pub fn why_of(error: &str) -> &'static str {
    match error.strip_prefix("org.freedesktop.DBus.Error.") {
        Some("ServiceUnknown" | "UnknownMethod" | "UnknownObject" | "UnknownInterface") => UNAVAILABLE,
        _ => DENIED,
    }
}

// ── the wire: ONE marshaller (the client's calls and the tests' fake bus replies), ONE reader ─────────────────────

/// A header field's value: string, object path, signature, uint32.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum Val {
    S(String),
    O(String),
    G(String),
    U(u32),
}

fn pad(v: &mut Vec<u8>, align: usize) {
    while v.len() % align != 0 {
        v.push(0);
    }
}

fn put_u32(v: &mut Vec<u8>, x: u32) {
    pad(v, 4);
    v.extend(x.to_le_bytes());
}

fn put_str(v: &mut Vec<u8>, s: &str) {
    put_u32(v, s.len() as u32);
    v.extend(s.as_bytes());
    v.push(0);
}

fn put_sig(v: &mut Vec<u8>, s: &str) {
    v.push(s.len() as u8);
    v.extend(s.as_bytes());
    v.push(0);
}

/// One little-endian message: the fixed header, the a(yv) header fields, padding to 8, the body. `body` is already
/// marshalled from an 8-aligned start (the body always starts 8-aligned).
pub fn message(ty: u8, flags: u8, serial: u32, fields: &[(u8, Val)], body: &[u8]) -> Vec<u8> {
    let mut v = vec![b'l', ty, flags, 1];
    v.extend((body.len() as u32).to_le_bytes());
    v.extend(serial.to_le_bytes());
    v.extend([0u8; 4]); // the fields array's length, patched below; the elements start at 16 (8-aligned)
    for (code, val) in fields {
        pad(&mut v, 8);
        v.push(*code);
        match val {
            Val::S(s) => {
                put_sig(&mut v, "s");
                put_str(&mut v, s);
            }
            Val::O(s) => {
                put_sig(&mut v, "o");
                put_str(&mut v, s);
            }
            Val::G(s) => {
                put_sig(&mut v, "g");
                put_sig(&mut v, s);
            }
            Val::U(x) => {
                put_sig(&mut v, "u");
                put_u32(&mut v, *x);
            }
        }
    }
    let flen = (v.len() - 16) as u32;
    v[12..16].copy_from_slice(&flen.to_le_bytes());
    pad(&mut v, 8);
    v.extend(body);
    v
}

/// OpenDevice's body, signature `sa{sv}`: the mode, then the options dict `{"flags": <int32 flags>}`.
pub fn open_device_body(mode: &str, flags: i32) -> Vec<u8> {
    let mut b = Vec::new();
    put_str(&mut b, mode);
    pad(&mut b, 4);
    let len_at = b.len();
    b.extend([0u8; 4]);
    pad(&mut b, 8); // dict entries align to 8; the array length counts from here
    let start = b.len();
    put_str(&mut b, "flags");
    put_sig(&mut b, "i");
    pad(&mut b, 4);
    b.extend(flags.to_le_bytes());
    let alen = (b.len() - start) as u32;
    b[len_at..len_at + 4].copy_from_slice(&alen.to_le_bytes());
    b
}

/// One parsed message: the fields this route reads, and the body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Message {
    pub ty: u8,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub signature: String,
    pub unix_fds: u32,
    pub body: Vec<u8>,
    /// Little-endian ('l'); else big-endian ('B').
    pub le: bool,
}

impl Message {
    /// The body's first uint32 (the `h` index of a return).
    fn body_u32(&self) -> Option<u32> {
        let b: [u8; 4] = self.body.get(..4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }
}

fn up(n: usize, align: usize) -> usize {
    (n + align - 1) & !(align - 1)
}

/// The first whole message in `buf` and its length; `Ok(None)` = more bytes needed; `Err(())` = malformed.
pub fn parse(buf: &[u8]) -> Result<Option<(Message, usize)>, ()> {
    if buf.len() < 16 {
        return Ok(None);
    }
    let le = match buf[0] {
        b'l' => true,
        b'B' => false,
        _ => return Err(()),
    };
    if buf[3] != 1 {
        return Err(());
    }
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = buf.get(at..at + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let body_len = u32_at(4).ok_or(())? as usize;
    let fend = 16 + u32_at(12).ok_or(())? as usize;
    let body_at = up(fend, 8);
    let total = body_at + body_len;
    if total > MAX_MESSAGE {
        return Err(());
    }
    if buf.len() < total {
        return Ok(None);
    }
    let mut m = Message { ty: buf[1], flags: buf[2], serial: u32_at(8).ok_or(())?, le, ..Default::default() };
    let mut at = 16;
    while at < fend {
        at = up(at, 8);
        let code = *buf.get(at).ok_or(())?;
        let sl = *buf.get(at + 1).ok_or(())? as usize;
        let sig = buf.get(at + 2..at + 2 + sl).ok_or(())?;
        at += 3 + sl;
        let text = |from: usize, n: usize| -> Result<String, ()> { String::from_utf8(buf.get(from..from + n).ok_or(())?.to_vec()).map_err(|_| ()) };
        let val = match sig {
            b"s" | b"o" => {
                at = up(at, 4);
                let n = u32_at(at).ok_or(())? as usize;
                let s = text(at + 4, n)?;
                at += 4 + n + 1;
                Val::S(s)
            }
            b"g" => {
                let n = *buf.get(at).ok_or(())? as usize;
                let s = text(at + 1, n)?;
                at += 1 + n + 1;
                Val::S(s)
            }
            b"u" => {
                at = up(at, 4);
                let x = u32_at(at).ok_or(())?;
                at += 4;
                Val::U(x)
            }
            _ => return Err(()),
        };
        match (code, val) {
            (PATH, Val::S(s)) => m.path = Some(s),
            (INTERFACE, Val::S(s)) => m.interface = Some(s),
            (MEMBER, Val::S(s)) => m.member = Some(s),
            (ERROR_NAME, Val::S(s)) => m.error = Some(s),
            (REPLY_SERIAL, Val::U(x)) => m.reply_serial = Some(x),
            (DESTINATION, Val::S(s)) => m.destination = Some(s),
            (SIGNATURE, Val::S(s)) => m.signature = s,
            (UNIX_FDS, Val::U(x)) => m.unix_fds = x,
            _ => {} // SENDER and any other field: not read by this route
        }
    }
    if at > fend {
        return Err(());
    }
    m.body = buf[body_at..total].to_vec();
    Ok(Some((m, total)))
}

// ── tests: a fake system bus the test spawns (this seat has no bus, no udisks2, no pkexec) ───────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{SECTOR, send_chunk, verify, write_to};
    use amber_core::block::Block;
    use amber_core::file_block::FileBlock;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    /// OpenDevice("rw", {"flags": <int32 O_EXCL 128>}), hand-marshalled from the specification: the KAT.
    const OPEN_BODY: [u8; 36] = [
        2, 0, 0, 0, b'r', b'w', 0, 0, // s "rw", pad to 4
        20, 0, 0, 0, 0, 0, 0, 0, // a{sv} length 20, pad to 8
        5, 0, 0, 0, b'f', b'l', b'a', b'g', b's', 0, // the key "flags"
        1, b'i', 0, 0, 0, 0, // the variant's signature "i", pad to 4
        128, 0, 0, 0, // int32 O_EXCL
    ];

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("udisks-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(x: &str) -> Val {
        Val::S(x.to_string())
    }

    /// The fake system bus on `sock`: accept one client, check its EXTERNAL uid and fd negotiation, read Hello and the
    /// call, answer Hello and a NameAcquired signal, then `answer(stream, call)`. Joins to the call it read.
    fn fake_bus(sock: &Path, answer: impl FnOnce(&UnixStream, &Message) + Send + 'static) -> std::thread::JoinHandle<Message> {
        let listener = UnixListener::bind(sock).unwrap();
        std::thread::spawn(move || {
            let (mut st, _) = listener.accept().unwrap();
            let mut nul = [9u8; 1];
            st.read_exact(&mut nul).unwrap();
            assert_eq!(nul, [0], "the credentials byte");
            assert_eq!(read_line(&mut st).unwrap(), format!("AUTH EXTERNAL {}", uid_hex()));
            st.write_all(b"OK 0123456789abcdef0123456789abcdef\r\n").unwrap();
            assert_eq!(read_line(&mut st).unwrap(), "NEGOTIATE_UNIX_FD");
            st.write_all(b"AGREE_UNIX_FD\r\n").unwrap();
            assert_eq!(read_line(&mut st).unwrap(), "BEGIN");
            let (mut buf, mut msgs) = (Vec::new(), Vec::new());
            while msgs.len() < 2 {
                if let Some((m, used)) = parse(&buf).unwrap() {
                    buf.drain(..used);
                    msgs.push(m);
                    continue;
                }
                let mut c = [0u8; 1024];
                let n = st.read(&mut c).unwrap();
                assert!(n > 0, "the client hung up before its call");
                buf.extend_from_slice(&c[..n]);
            }
            let call = msgs.pop().unwrap();
            let hello = msgs.pop().unwrap();
            assert_eq!((hello.ty, hello.serial), (METHOD_CALL, 1));
            assert_eq!(hello.member.as_deref(), Some("Hello"));
            assert_eq!(hello.destination.as_deref(), Some("org.freedesktop.DBus"));
            let mut name = Vec::new();
            put_str(&mut name, ":1.42");
            st.write_all(&message(METHOD_RETURN, 0, 1, &[(REPLY_SERIAL, Val::U(1)), (DESTINATION, s(":1.42")), (SIGNATURE, Val::G("s".into()))], &name)).unwrap();
            let acquired = [(PATH, Val::O("/org/freedesktop/DBus".into())), (INTERFACE, s("org.freedesktop.DBus")), (MEMBER, s("NameAcquired")), (DESTINATION, s(":1.42")), (SIGNATURE, Val::G("s".into()))];
            st.write_all(&message(SIGNAL, 0, 2, &acquired, &name)).unwrap();
            answer(&st, &call);
            call
        })
    }

    #[test]
    fn object_path_rule() {
        let p = |n: &str| object_path(Path::new(n));
        assert_eq!(p("/dev/sdb"), "/org/freedesktop/UDisks2/block_devices/sdb");
        assert_eq!(p("/dev/mmcblk0"), "/org/freedesktop/UDisks2/block_devices/mmcblk0", "digits are kept");
        assert_eq!(p("/dev/nvme0n1"), "/org/freedesktop/UDisks2/block_devices/nvme0n1");
        assert_eq!(p("/dev/dm-0"), "/org/freedesktop/UDisks2/block_devices/dm_2d0");
        assert_eq!(p("/dev/my_disk"), "/org/freedesktop/UDisks2/block_devices/my_5fdisk");
    }

    #[test]
    fn bus_address_and_wire_kat() {
        assert_eq!(bus_from_address(None), Some(PathBuf::from("/var/run/dbus/system_bus_socket")));
        assert_eq!(bus_from_address(Some("unix:path=/run/dbus/system_bus_socket")), Some(PathBuf::from("/run/dbus/system_bus_socket")));
        assert_eq!(bus_from_address(Some("unix:abstract=/tmp/x;unix:guid=1,path=/run/b")), Some(PathBuf::from("/run/b")));
        assert_eq!(bus_from_address(Some("tcp:host=localhost,port=1")), None);
        assert_eq!(open_device_body("rw", libc::O_EXCL), OPEN_BODY);
        // The marshaller and the reader agree, field by field.
        let call = message(METHOD_CALL, ALLOW_INTERACTIVE_AUTHORIZATION, 2, &[(PATH, Val::O("/a/b".into())), (MEMBER, s("OpenDevice")), (SIGNATURE, Val::G("sa{sv}".into())), (UNIX_FDS, Val::U(0))], &OPEN_BODY);
        assert_eq!(call.len() % 8, 4, "36-byte body after an 8-aligned header");
        let (m, used) = parse(&call).unwrap().unwrap();
        assert_eq!(used, call.len());
        assert_eq!((m.ty, m.flags, m.serial, m.le), (METHOD_CALL, ALLOW_INTERACTIVE_AUTHORIZATION, 2, true));
        assert_eq!((m.path.as_deref(), m.member.as_deref(), m.signature.as_str()), (Some("/a/b"), Some("OpenDevice"), "sa{sv}"));
        assert_eq!(m.body, OPEN_BODY);
        assert_eq!(parse(&call[..call.len() - 1]), Ok(None), "a partial message waits");
        let mut bad = call.clone();
        bad[0] = b'x';
        assert_eq!(parse(&bad), Err(()), "no endian mark");
        assert_eq!(why_of("org.freedesktop.UDisks2.Error.NotAuthorizedDismissed"), DENIED);
        assert_eq!(why_of("org.freedesktop.UDisks2.Error.Failed"), DENIED);
        assert_eq!(why_of("org.freedesktop.DBus.Error.UnknownMethod"), UNAVAILABLE);
    }

    // The fd udisksd would open arrives over the fake bus into FileBlock::from_file and the ONE writer runs on it.
    #[test]
    fn fake_bus_hands_the_fd_to_the_writer() {
        let dir = scratch("fd");
        let sectors = (2 << 20) / SECTOR + 5;
        let img: Vec<u8> = (0..sectors * SECTOR).map(|i| (i.wrapping_mul(2654435761) >> 9) as u8).collect();
        let (ip, dp, sock) = (dir.join("card.img"), dir.join("device.bin"), dir.join("bus"));
        fs::write(&ip, &img).unwrap();
        fs::File::create(&dp).unwrap().set_len(8 << 20).unwrap();
        let dev_path = dp.clone();
        let bus = fake_bus(&sock, move |st, call| {
            assert_eq!((call.ty, call.serial), (METHOD_CALL, 2));
            assert_ne!(call.flags & ALLOW_INTERACTIVE_AUTHORIZATION, 0, "polkit may ask");
            assert_eq!(call.path.as_deref(), Some("/org/freedesktop/UDisks2/block_devices/sdb"));
            assert_eq!(call.interface.as_deref(), Some("org.freedesktop.UDisks2.Block"));
            assert_eq!(call.member.as_deref(), Some("OpenDevice"));
            assert_eq!(call.destination.as_deref(), Some("org.freedesktop.UDisks2"));
            assert_eq!(call.signature, "sa{sv}");
            assert_eq!(call.body, OPEN_BODY, "the canned OpenDevice body");
            let dev = fs::OpenOptions::new().read(true).write(true).open(&dev_path).unwrap();
            let mut idx = Vec::new();
            put_u32(&mut idx, 0);
            let reply = message(METHOD_RETURN, 0, 3, &[(REPLY_SERIAL, Val::U(call.serial)), (DESTINATION, s(":1.42")), (SIGNATURE, Val::G("h".into())), (UNIX_FDS, Val::U(1))], &idx);
            assert_eq!(send_chunk(st, &reply, Some(&dev)).unwrap(), reply.len());
        });
        let got = open_device(Some(&sock), Path::new("/dev/sdb")).expect("the fd over the fake bus");
        bus.join().unwrap();
        let mut dst = FileBlock::from_file(got, None).unwrap();
        assert_eq!(dst.sectors(), (8 << 20) / SECTOR);
        let mut src = FileBlock::open(&ip).unwrap();
        write_to(&mut src, &mut dst, sectors, &mut |_, _| {}).unwrap();
        assert!(verify(&mut src, &mut dst, sectors));
        drop(dst);
        assert_eq!(&fs::read(&dp).unwrap()[..img.len()], &img[..]);
        let _ = fs::remove_dir_all(&dir);
    }

    // A bus error is judged by its name; a return without its fd is denied; no bus is no route.
    #[test]
    fn fake_bus_refusals() {
        let dir = scratch("refuse");
        let cases: [(&'static str, &'static str); 3] = [
            ("org.freedesktop.UDisks2.Error.NotAuthorizedDismissed", DENIED),
            ("org.freedesktop.DBus.Error.ServiceUnknown", UNAVAILABLE),
            ("", DENIED), // a return of signature h whose fd never came
        ];
        for (i, (name, why)) in cases.into_iter().enumerate() {
            let sock = dir.join(format!("bus{i}"));
            let bus = fake_bus(&sock, move |st, call| {
                let reply = if name.is_empty() {
                    let mut idx = Vec::new();
                    put_u32(&mut idx, 0);
                    message(METHOD_RETURN, 0, 3, &[(REPLY_SERIAL, Val::U(call.serial)), (SIGNATURE, Val::G("h".into())), (UNIX_FDS, Val::U(1))], &idx)
                } else {
                    let mut text = Vec::new();
                    put_str(&mut text, "refused");
                    message(ERROR, 0, 3, &[(REPLY_SERIAL, Val::U(call.serial)), (ERROR_NAME, s(name)), (SIGNATURE, Val::G("s".into()))], &text)
                };
                assert_eq!(send_chunk(st, &reply, None).unwrap(), reply.len());
            });
            assert_eq!(open_device(Some(&sock), Path::new("/dev/mmcblk0")).unwrap_err(), why, "{name}");
            bus.join().unwrap();
        }
        assert_eq!(open_device(Some(&dir.join("absent")), Path::new("/dev/sdb")).unwrap_err(), UNAVAILABLE, "no socket");
        assert_eq!(open_device(None, Path::new("/dev/sdb")).unwrap_err(), UNAVAILABLE, "no unix bus address");
        let _ = fs::remove_dir_all(&dir);
    }
}
