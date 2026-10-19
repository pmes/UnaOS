//! CHARTER: Principia — kernel-by-ruling (ABIMAGE B683, BP-bar-8: the A/B kernel slots' records and the loader's choice; ONE file both the kernel and the UEFI loader compile)
//!
//! ABIMAGE (rmbp-ledger B683). The image is the unit of change and no restart is forced. Two kernel slots on the
//! ESP — `KERNEL.ELF` (current) and `KERNEL.NEW` (staged) — and two one-line ASCII records beside them:
//!
//! * `STAGE.REC` — `stage=1 kelf=<max_vaddr> bytes=<n> crc=<crc32>`, written LAST by `install::abimage::stage_image`
//!   (so its presence means `KERNEL.NEW` is whole and re-read), removed by the promotion and by the loader's fallback.
//! * `SHUTDOWN.REC` — `why=<staged|user|trial|fallback|promoted>`: `staged` by the stage (a stale `user` never arms a
//!   new stage), `user` by `power::prelude` (the user's own restart or shut down), `trial` by the LOADER when it picks
//!   `KERNEL.NEW`, `fallback` by the loader when the trial never reached DESKTOPBUILT, `promoted` by the promotion.
//!
//! [`decide`] is the loader's whole decision, a pure function over the two records (four cases, `tests abimage`):
//!
//! | STAGE.REC | SHUTDOWN.REC | slot | why          | the loader writes                               |
//! |-----------|--------------|------|--------------|-------------------------------------------------|
//! | absent    | any          | ELF  | normal       | nothing                                         |
//! | staged    | `why=user`   | NEW  | user-restart | `why=trial` (BEFORE the jump: a NEW that dies is a trial on the next boot) |
//! | staged    | `why=trial`  | ELF  | fallback     | `why=fallback`, and STAGE.REC removed (the stage is spent) |
//! | staged    | anything else| ELF  | normal       | nothing (a crash or firmware restart never switches)       |
//!
//! No `alloc`, no `crate::` path: the UEFI loader includes this file by `#[path]` (`crates/bootloader/src/main.rs`
//! tail), so the kernel's fixture asserts the very function the loader runs.

/// The current slot's file on the ESP (FAT names are case-blind; the loader has always opened `kernel.elf`).
pub const ELF: &str = "KERNEL.ELF";
/// The staged slot's file on the ESP.
pub const NEW: &str = "KERNEL.NEW";
/// The promotion's moment-long holder of the outgoing image (rename ELF->OLD, NEW->ELF, unlink OLD).
pub const OLD: &str = "KERNEL.OLD";
/// The stage record.
pub const STAGE: &str = "STAGE.REC";
/// The shutdown record.
pub const SHUTDOWN: &str = "SHUTDOWN.REC";

/// The records' payloads, one place.
pub const REC_STAGED: &[u8] = b"why=staged\n";
pub const REC_USER: &[u8] = b"why=user\n";
pub const REC_TRIAL: &[u8] = b"why=trial\n";
pub const REC_FALLBACK: &[u8] = b"why=fallback\n";
pub const REC_PROMOTED: &[u8] = b"why=promoted\n";

/// The most a record may hold (both sides read at most this many bytes).
pub const REC_MAX: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Elf,
    New,
}

impl Slot {
    /// The wire word (`[loader] slot=<ELF|NEW>`).
    pub fn word(self) -> &'static str {
        match self {
            Slot::Elf => "ELF",
            Slot::New => "NEW",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Why {
    Normal,
    UserRestart,
    Fallback,
}

impl Why {
    /// The wire word (`[loader] … why=<user-restart|fallback|normal>`).
    pub fn word(self) -> &'static str {
        match self {
            Why::Normal => "normal",
            Why::UserRestart => "user-restart",
            Why::Fallback => "fallback",
        }
    }
}

/// What the loader writes back before it jumps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Write {
    /// Nothing.
    None,
    /// `SHUTDOWN.REC` := [`REC_TRIAL`].
    Trial,
    /// `SHUTDOWN.REC` := [`REC_FALLBACK`], and `STAGE.REC` removed.
    Fallback,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Decision {
    pub slot: Slot,
    pub why: Why,
    pub write: Write,
}

/// The value of `key=` in a one-line record (`key` at the start or after a space), up to the next space or newline.
pub fn field<'a>(rec: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i + key.len() < rec.len() {
        let at_word = i == 0 || rec[i - 1] == b' ' || rec[i - 1] == b'\n';
        if at_word && &rec[i..i + key.len()] == key && rec[i + key.len()] == b'=' {
            let v = &rec[i + key.len() + 1..];
            let end = v.iter().position(|&c| c == b' ' || c == b'\n' || c == b'\r').unwrap_or(v.len());
            return Some(&v[..end]);
        }
        i += 1;
    }
    None
}

/// Is a stage present? A `STAGE.REC` that does not say `stage=1` is no stage (a torn or foreign file never switches).
pub fn staged(stage: Option<&[u8]>) -> bool {
    matches!(stage.and_then(|r| field(r, b"stage")), Some(b"1"))
}

/// THE LOADER'S DECISION — pure over the two records (`None` = the file is absent). See the module table.
pub fn decide(stage: Option<&[u8]>, shutdown: Option<&[u8]>) -> Decision {
    if !staged(stage) {
        return Decision { slot: Slot::Elf, why: Why::Normal, write: Write::None };
    }
    match shutdown.and_then(|r| field(r, b"why")) {
        Some(b"user") => Decision { slot: Slot::New, why: Why::UserRestart, write: Write::Trial },
        Some(b"trial") => Decision { slot: Slot::Elf, why: Why::Fallback, write: Write::Fallback },
        _ => Decision { slot: Slot::Elf, why: Why::Normal, write: Write::None },
    }
}

/// Spell `STAGE.REC` into `out` (no allocator on the loader side): `stage=1 kelf=0x<hex> bytes=<dec> crc=0x<hex8>\n`.
/// Returns the length written (always < [`REC_MAX`]).
pub fn stage_record(kelf: u64, bytes: u64, crc: u32, out: &mut [u8; REC_MAX]) -> usize {
    let mut n = 0;
    let mut put = |s: &[u8], n: &mut usize| {
        for &c in s {
            out[*n] = c;
            *n += 1;
        }
    };
    put(b"stage=1 kelf=0x", &mut n);
    put(hex(kelf, 1).as_slice(), &mut n);
    put(b" bytes=", &mut n);
    put(dec(bytes).as_slice(), &mut n);
    put(b" crc=0x", &mut n);
    put(hex(crc as u64, 8).as_slice(), &mut n);
    put(b"\n", &mut n);
    n
}

/// Read a `0x`-prefixed or bare hex field back (`kelf=`, `crc=`).
pub fn parse_hex(v: &[u8]) -> Option<u64> {
    let v = if v.len() > 2 && (&v[..2] == b"0x" || &v[..2] == b"0X") { &v[2..] } else { v };
    if v.is_empty() || v.len() > 16 {
        return None;
    }
    let mut x = 0u64;
    for &c in v {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        x = (x << 4) | d as u64;
    }
    Some(x)
}

/// A small fixed buffer for the spellers below.
pub struct Digits {
    buf: [u8; 20],
    len: usize,
}

impl Digits {
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Lower-case hex, at least `min` digits.
fn hex(mut x: u64, min: usize) -> Digits {
    let mut t = [0u8; 16];
    let mut k = 0;
    while x != 0 || k < min.max(1) {
        let d = (x & 0xF) as u8;
        t[k] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        x >>= 4;
        k += 1;
        if k == 16 {
            break;
        }
    }
    let mut out = Digits { buf: [0; 20], len: k };
    for i in 0..k {
        out.buf[i] = t[k - 1 - i];
    }
    out
}

/// Decimal.
fn dec(mut x: u64) -> Digits {
    let mut t = [0u8; 20];
    let mut k = 0;
    loop {
        t[k] = b'0' + (x % 10) as u8;
        x /= 10;
        k += 1;
        if x == 0 || k == 20 {
            break;
        }
    }
    let mut out = Digits { buf: [0; 20], len: k };
    for i in 0..k {
        out.buf[i] = t[k - 1 - i];
    }
    out
}
