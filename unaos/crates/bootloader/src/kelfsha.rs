//! CHARTER: Principia — kernel-by-ruling (B709 KELFHASH, BP-bar-4/8: the KELF.SHA record and the loader's hash check; ONE file the UEFI loader and the builder compile)
//!
//! KELFHASH (rmbp-ledger B709). The builder writes `KELF.SHA` beside the kernel on the boot FAT, one ASCII row per
//! slot (slot A = `KERNEL.ELF`, slot B = `KERNEL.NEW`, ABIMAGE B683):
//!
//! `sha256=<64 hex> file=KERNEL.ELF slot=A kelf=0x<max vaddr>`
//!
//! and the x86 loader hashes the bytes it read and asks [`check`] BEFORE the jump. `kelf=` is the loader's own
//! `KELF max=` (BP-boot-13), computed by [`kelf_of`] on both sides: it binds a row to the image it describes, so a
//! row for another image reads as [`Check::Stale`] — `unverified`, never a lockout (BP-bar-1). What this guards
//! against is a torn write and a half-flashed stick, nothing else: no signing, no key (BP-security-16).
//!
//! KELFSTAGE (B720): the kernel's `install::abimage` keeps the record in step with the slots — the stage writes the
//! slot-B row ([`with_row`]), the promotion respells it as the slot-A row ([`promoted`]), the unstage drops it.
//!
//! STAGEWINDOW (B733): the promotion writes the next record as [`NEXT`] BEFORE its renames and renames it over
//! [`NAME`] after them; the loader takes [`NEXT`] as the record whenever it is present ([`record`]), so no power cut
//! between the renames and the record leaves a row that halts the boot. A row is KEYED ([`Key`]): `kelf=` plus the
//! file's `size=` and the sha256 of its first 4 KiB (`head=`), so two builds sharing one `kelf=` max never alias;
//! [`row`] is the ONE spelling the builder, the stage and the promotion write.
//!
//! Pure: no `alloc`, no `uefi`, no `crate::` path — the loader includes it by `#[path]` (`main.rs` tail), and so do
//! the builder (`builder/src/main.rs`) and the kernel (`install/abimage.rs`), so the writers and the reader cannot
//! disagree.

/// The record's file on the ESP (8.3; FAT names are case-blind).
pub const NAME: &str = "KELF.SHA";
/// STAGEWINDOW (B733): the promotion's next record (a VFAT long name), written before the kernel renames and renamed
/// over [`NAME`] after them. Present = the promotion got past its record write: the loader reads it as the record.
pub const NEXT: &str = "KELF.SHA.NEW";
/// The most of the record either side reads (two keyed rows are 430 bytes).
pub const MAX: usize = 512;
/// One row's buffer (`sha256=` 7 + 64 + ` file=` 6 + 12 + ` slot=` 6 + 1 + ` kelf=0x` 8 + 16 + ` size=` 6 + 20 +
/// ` head=` 6 + 64 + `\n` = 217).
pub const ROW_MAX: usize = 256;
/// STAGEWINDOW: the bytes `head=` hashes — the file's first 4 KiB (all of it when shorter).
pub const HEAD: usize = 4096;

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
    /// A record, but no row for this slot (a KERNEL.NEW staged before KELFSTAGE): `unverified`, jump.
    NoRow,
    /// The row describes another image (its key — `kelf=`, `size=`, `head=` — is not these bytes'): `unverified`, jump.
    Stale,
    /// Same key, different bytes — or bytes that are not an ELF64 at all: a torn write. Try the other slot.
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

/// STAGEWINDOW (B733): what binds a row to one image without hashing all of it — the loader's `KELF max=`, the file's
/// byte size and the sha256 of its first [`HEAD`] bytes. Built by [`key_of`] on every side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    pub kelf: u64,
    pub size: u64,
    pub head: [u8; 32],
}

/// One parsed row. `size`/`head` are `None` on a row written before STAGEWINDOW (keyed on `kelf=` alone).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Row {
    pub sha: [u8; 32],
    pub slot: u8,
    pub kelf: u64,
    pub size: Option<u64>,
    pub head: Option<[u8; 32]>,
}

impl Row {
    /// The row every writer spells: the bytes' sha256, the slot, the full key.
    pub fn new(sha: [u8; 32], slot: u8, key: Key) -> Row {
        Row { sha, slot, kelf: key.kelf, size: Some(key.size), head: Some(key.head) }
    }
    /// Does this row describe the image with key `k`? Every field the row carries must agree.
    pub fn keyed(&self, k: &Key) -> bool {
        self.kelf == k.kelf && self.size.map_or(true, |s| s == k.size) && self.head.map_or(true, |h| h == k.head)
    }
}

/// The slot's file (slot A = `KERNEL.ELF`, slot B = `KERNEL.NEW`).
pub fn file_of(slot: u8) -> &'static str {
    if slot == SLOT_B { "KERNEL.NEW" } else { "KERNEL.ELF" }
}

/// The bytes `head=` hashes: the first [`HEAD`] of `prefix` (a buffer that starts at the file's offset 0).
pub fn head(prefix: &[u8]) -> &[u8] {
    &prefix[..prefix.len().min(HEAD)]
}

/// The key of a file whose first bytes are `prefix` (at least its program headers and [`head`]), `size` bytes long,
/// with `head_sha` = sha256 of [`head`]`(prefix)`. `None` when the bytes are not an ELF64 ([`kelf_of`]).
pub fn key_of(prefix: &[u8], size: u64, head_sha: &[u8; 32]) -> Option<Key> {
    Some(Key { kelf: kelf_of(prefix)?, size, head: *head_sha })
}

/// STAGEWINDOW: THE RECORD THE LOADER CHECKS — `KELF.SHA.NEW` when present (the promotion got past its record write),
/// else `KELF.SHA`. The loader and `tests kelfstage` call this one fn.
pub fn record<'a>(cur: Option<&'a [u8]>, next: Option<&'a [u8]>) -> Option<&'a [u8]> {
    next.or(cur)
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

/// The ELF magic `kelf_of` requires (the fixtures that build an image take it from here, the one reader).
pub const ELF_MAGIC: &[u8; 4] = b"\x7fELF";

/// The loader's `KELF max=`: the highest `p_vaddr + p_memsz` over the PT_LOAD segments of a little-endian ELF64.
/// `None` when the bytes are not one (bad magic, class, a header table past the end, no PT_LOAD).
pub fn kelf_of(elf: &[u8]) -> Option<u64> {
    if elf.len() < 64 || &elf[..4] != ELF_MAGIC || elf[4] != 2 || elf[5] != 1 {
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

fn hex32(hex: &[u8]) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (nib(hex[2 * i])? << 4) | nib(hex[2 * i + 1])?;
    }
    Some(out)
}

fn parse_row(row: &[u8]) -> Option<Row> {
    let sha = hex32(field(row, b"sha256")?)?;
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
    // STAGEWINDOW: `size=` (decimal) and `head=` (64 hex) are optional — absent on an older row — but a field that is
    // present and malformed makes the row unparseable (a torn row is no row).
    let size = match field(row, b"size") {
        None => None,
        Some(v) if v.is_empty() || v.len() > 20 => return None,
        Some(v) => {
            let mut n = 0u64;
            for &c in v {
                if !c.is_ascii_digit() {
                    return None;
                }
                n = n.checked_mul(10)?.checked_add((c - b'0') as u64)?;
            }
            Some(n)
        }
    };
    let head = match field(row, b"head") {
        None => None,
        Some(v) => Some(hex32(v)?),
    };
    Some(Row { sha, slot, kelf, size, head })
}

/// The first well-formed row for `slot` in a record.
pub fn find(rec: &[u8], slot: u8) -> Option<Row> {
    rec.split(|&c| c == b'\n').filter_map(parse_row).find(|r| r.slot == slot)
}

/// THE ONE ROW SPELLING (STAGEWINDOW B733): the builder, the stage and the promotion all call it. `file=` follows the
/// slot ([`file_of`]); `size=`/`head=` are written when the row carries them. Returns the length (<= [`ROW_MAX`]).
pub fn row(r: &Row, out: &mut [u8; ROW_MAX]) -> usize {
    let (sha, file, slot, kelf) = (&r.sha, file_of(r.slot), r.slot, r.kelf);
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
    if let Some(size) = r.size {
        put(b" size=", &mut n);
        let mut dec = [0u8; 20];
        let (mut v, mut i) = (size, dec.len());
        loop {
            i -= 1;
            dec[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        put(&dec[i..], &mut n);
    }
    if let Some(h) = r.head {
        put(b" head=", &mut n);
        for b in &h {
            put(&[HEX[(b >> 4) as usize], HEX[(b & 15) as usize]], &mut n);
        }
    }
    put(b"\n", &mut n);
    n
}

/// KELFSTAGE (rmbp-ledger B720): `rec` with `slot`'s row replaced — every well-formed row of ANOTHER slot kept in order,
/// then `new_row` (a [`row`] spelling) appended; `None` for `new_row` drops the slot's row. Blank and malformed lines
/// are not carried. `None` when the result would not fit [`MAX`]. The kernel's stage (slot B) and unstage call it.
pub fn with_row(rec: &[u8], slot: u8, new_row: Option<&[u8]>, out: &mut [u8; MAX]) -> Option<usize> {
    let mut n = 0;
    let mut put = |s: &[u8], n: &mut usize| -> Option<()> {
        let end = n.checked_add(s.len())?;
        out.get_mut(*n..end)?.copy_from_slice(s);
        *n = end;
        Some(())
    };
    for line in rec.split(|&c| c == b'\n') {
        if parse_row(line).is_some_and(|r| r.slot != slot) {
            put(line, &mut n)?;
            put(b"\n", &mut n)?;
        }
    }
    if let Some(r) = new_row {
        let r = r.strip_suffix(b"\n").unwrap_or(r);
        put(r, &mut n)?;
        put(b"\n", &mut n)?;
    }
    Some(n)
}

/// KELFSTAGE (B720): the record after ABIMAGE's promotion (KERNEL.NEW renamed over KERNEL.ELF) — slot B's row
/// respelled as `file=KERNEL.ELF slot=A` (same sha256, same key), every other row dropped, so the old image's A
/// row never survives an update (`hash=ok` does). `None` when there is no slot-B row to promote. STAGEWINDOW: the
/// promotion writes this as [`NEXT`] before its renames.
pub fn promoted(rec: &[u8], out: &mut [u8; MAX]) -> Option<usize> {
    let b = find(rec, SLOT_B)?;
    let mut r = [0u8; ROW_MAX];
    let n = row(&Row { slot: SLOT_A, ..b }, &mut r);
    out.get_mut(..n)?.copy_from_slice(&r[..n]);
    Some(n)
}

/// THE LOADER'S CHECK of one slot's bytes — pure over the record ([`record`]; `None` = neither file), the slot, the
/// bytes' sha256 and their key ([`key_of`]; `None` = not an ELF64). Same sha = Ok; else a key the row does not carry
/// = Stale; a matching key (or no key) = Bad. See [`Check`].
pub fn check(rec: Option<&[u8]>, slot: u8, sha: &[u8; 32], key: Option<&Key>) -> Check {
    let Some(rec) = rec else { return Check::NoRecord };
    let Some(r) = find(rec, slot) else { return Check::NoRow };
    if &r.sha == sha {
        return Check::Ok;
    }
    match key {
        Some(k) if !r.keyed(k) => Check::Stale,
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
