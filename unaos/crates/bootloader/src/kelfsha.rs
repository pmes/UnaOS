//! CHARTER: Principia — kernel-by-ruling (B709 KELFHASH, BP-bar-4/8: the KELF.SHA record and the loader's hash check; ONE file the UEFI loader and the builder compile)
//!
//! KELFHASH (rmbp-ledger B709). The builder writes `KELF.SHA` beside the kernel on the boot FAT, one ASCII row per
//! slot (slot A = `KERNEL.ELF`, slot B = `KERNEL.NEW`, ABIMAGE B683):
//!
//! `sha256=<64 hex> file=KERNEL.ELF slot=A kelf=0x<max vaddr>`
//!
//! and the x86 loader hashes the bytes it read and asks [`check`] BEFORE the jump. `kelf=` is the loader's own
//! `KELF max=` (BP-boot-13), computed by [`kelf_of`] on both sides: it binds a row to the image it describes, so a
//! row left behind by ABIMAGE's promotion (KERNEL.NEW renamed over KERNEL.ELF, the record untouched) reads as
//! [`Check::Stale`] — `unverified`, never a lockout (BP-bar-1). What this guards against is a torn write and a
//! half-flashed stick, nothing else: no signing, no key (BP-security-16).
//!
//! Pure: no `alloc`, no `uefi`, no `crate::` path — the loader includes it by `#[path]` (`main.rs` tail) and so does
//! the builder (`builder/src/main.rs`), so the writer and the reader cannot disagree.

/// The record's file on the ESP (8.3; FAT names are case-blind).
pub const NAME: &str = "KELF.SHA";
/// The most of the record either side reads.
pub const MAX: usize = 512;
/// One row's buffer (`sha256=` 7 + 64 + ` file=` 6 + 12 + ` slot=` 6 + 1 + ` kelf=0x` 8 + 16 + `\n`).
pub const ROW_MAX: usize = 128;

/// The slot letter for a file (slot A = `KERNEL.ELF`, slot B = `KERNEL.NEW`).
pub const SLOT_A: u8 = b'A';
pub const SLOT_B: u8 = b'B';

/// What the loader makes of one slot's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    /// The row's sha256 is the bytes' sha256: `hash=ok`.
    Ok,
    /// No `KELF.SHA` on the volume (an image older than KELFHASH): `hash=unverified`, jump.
    NoRecord,
    /// A record, but no row for this slot (KERNEL.NEW is staged by the kernel, not the builder): `unverified`, jump.
    NoRow,
    /// The row describes another image (its `kelf=` is not these bytes'): `unverified`, jump.
    Stale,
    /// Same image, different bytes — or bytes that are not an ELF64 at all: a torn write. Try the other slot.
    Bad,
}

impl Check {
    /// `hash=` on the wire.
    pub fn hash_word(self) -> &'static str {
        if self == Check::Ok { "ok" } else { "unverified" }
    }
    /// `why=` on the loader's line, and the boot-info `why` code (`unaos_boot_info::KelfVerdict`).
    pub fn why_word(self) -> &'static str {
        match self {
            Check::Ok => "ok",
            Check::NoRecord => "no-record",
            Check::NoRow => "no-row",
            Check::Stale => "stale",
            Check::Bad => "mismatch",
        }
    }
    pub fn why_code(self) -> u8 {
        match self {
            Check::Ok => 0,
            Check::NoRecord => 1,
            Check::NoRow => 2,
            Check::Stale => 3,
            Check::Bad => 4,
        }
    }
}

/// One parsed row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Row {
    pub sha: [u8; 32],
    pub slot: u8,
    pub kelf: u64,
}

fn rd16(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]) as u64)
}
fn rd32(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o + 8)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

/// The loader's `KELF max=`: the highest `p_vaddr + p_memsz` over the PT_LOAD segments of a little-endian ELF64.
/// `None` when the bytes are not one (bad magic, class, a header table past the end, no PT_LOAD).
pub fn kelf_of(elf: &[u8]) -> Option<u64> {
    if elf.len() < 64 || &elf[..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return None;
    }
    let phoff = rd64(elf, 0x20)? as usize;
    let phentsize = rd16(elf, 0x36)? as usize;
    let phnum = rd16(elf, 0x38)? as usize;
    if phentsize < 56 {
        return None;
    }
    let mut max = None;
    for i in 0..phnum {
        let p = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if rd32(elf, p)? != 1 {
            continue;
        }
        let end = rd64(elf, p + 0x10)?.checked_add(rd64(elf, p + 0x28)?)?;
        if max.map_or(true, |m| end > m) {
            max = Some(end);
        }
    }
    max
}

fn nib(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The value of `key=` in one row (`key` at the start or after a space), up to the next space.
fn field<'a>(row: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i + key.len() < row.len() {
        if (i == 0 || row[i - 1] == b' ') && &row[i..i + key.len()] == key && row[i + key.len()] == b'=' {
            let v = &row[i + key.len() + 1..];
            let end = v.iter().position(|&c| c == b' ' || c == b'\r').unwrap_or(v.len());
            return Some(&v[..end]);
        }
        i += 1;
    }
    None
}

fn parse_row(row: &[u8]) -> Option<Row> {
    let hex = field(row, b"sha256")?;
    if hex.len() != 64 {
        return None;
    }
    let mut sha = [0u8; 32];
    for i in 0..32 {
        sha[i] = (nib(hex[2 * i])? << 4) | nib(hex[2 * i + 1])?;
    }
    let slot = match field(row, b"slot")? {
        [s] => *s,
        _ => return None,
    };
    let k = field(row, b"kelf")?;
    let k = if k.len() > 2 && (&k[..2] == b"0x" || &k[..2] == b"0X") { &k[2..] } else { k };
    if k.is_empty() || k.len() > 16 {
        return None;
    }
    let mut kelf = 0u64;
    for &c in k {
        kelf = (kelf << 4) | nib(c)? as u64;
    }
    Some(Row { sha, slot, kelf })
}

/// The first well-formed row for `slot` in a record.
pub fn find(rec: &[u8], slot: u8) -> Option<Row> {
    rec.split(|&c| c == b'\n').filter_map(parse_row).find(|r| r.slot == slot)
}

/// Spell one row into `out`; returns its length (always <= [`ROW_MAX`]; `file` is cut at 12 bytes).
pub fn row(sha: &[u8; 32], file: &str, slot: u8, kelf: u64, out: &mut [u8; ROW_MAX]) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut n = 0;
    let mut put = |s: &[u8], n: &mut usize| {
        for &c in s {
            out[*n] = c;
            *n += 1;
        }
    };
    put(b"sha256=", &mut n);
    for b in sha {
        put(&[HEX[(b >> 4) as usize], HEX[(b & 15) as usize]], &mut n);
    }
    put(b" file=", &mut n);
    let f = file.as_bytes();
    put(&f[..f.len().min(12)], &mut n);
    put(b" slot=", &mut n);
    put(&[slot], &mut n);
    put(b" kelf=0x", &mut n);
    let mut started = false;
    for i in (0..16).rev() {
        let d = ((kelf >> (i * 4)) & 15) as usize;
        if d != 0 || started || i == 0 {
            started = true;
            put(&[HEX[d]], &mut n);
        }
    }
    put(b"\n", &mut n);
    n
}

/// THE LOADER'S CHECK of one slot's bytes — pure over the record (`None` = no `KELF.SHA`), the slot, the bytes'
/// sha256 and the bytes. See [`Check`].
pub fn check(rec: Option<&[u8]>, slot: u8, sha: &[u8; 32], elf: &[u8]) -> Check {
    let Some(rec) = rec else { return Check::NoRecord };
    let Some(r) = find(rec, slot) else { return Check::NoRow };
    if &r.sha == sha {
        return Check::Ok;
    }
    match kelf_of(elf) {
        Some(k) if k != r.kelf => Check::Stale,
        _ => Check::Bad,
    }
}

/// The first 8 hex digits of a digest (the `sha=` field on the loader's and the kernel's lines).
pub fn short(sha: &[u8; 32]) -> [u8; 8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut o = [0u8; 8];
    for i in 0..4 {
        o[2 * i] = HEX[(sha[i] >> 4) as usize];
        o[2 * i + 1] = HEX[(sha[i] & 15) as usize];
    }
    o
}
