// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PARTINSTALL — lay UnaOS INTO one partition of an EXISTING GPT, beside volumes it must never touch.
//
// WHAT THIS CHANGES, AND WHY IT IS NARROW. Everything the installer engine could do before this
// module wrote a WHOLE DISK: `gpt::write_gpt` lays a fresh table over LBA 0, and `blank_check`
// refuses unless the leading sectors are already zero. That is a perfectly good discipline for a
// blank stick and a useless one for the job Peter named — *"i do not want to run catalina, i want
// unaos on the internal hd"* (rmbp-ledger B89) — where the disk is partitioned by the operator in
// Disk Utility and a foreign volume lives on the very same medium. So this module adds ONE new
// capability: a target that is a single partition, never the disk.
//
// RULINGS R25 (Peter, 2026-09-08): *"another way to think of it is like on the macbook if UnaOS saw
// catalina and immediately formatted the disk as an alien enemy."* A non-UnaOS disk is a STRANGER.
// rmbp-ledger B91 states the gap this closes precisely: INSTALL-SELF protects HOME (the disk we
// booted from) and *"leaves every OTHER disk a legitimate candidate by construction"*. The guard
// here points the other way — at the disk we merely SEE.
//
// THE FOUR STRUCTURAL INVARIANTS, each mechanical rather than argued:
//
//   1. NO WRITE ESCAPES THE NAMED PARTITION. Not by review — by the type system. Every byte of the
//      install goes through [`PartitionTarget`], whose `write_sectors` adds the partition's base LBA
//      and refuses (`BadLba`) anything past its last sector. The formatter, the tree writer and the
//      verifier below are handed that target and cannot address the disk at all, so "did we clip
//      LBA 0" is not a question about this code's care; it is a question about a bounds check that
//      has a go-red fixture.
//   2. NOTHING WRITES TO LBA 0 OR THE PARTITION TABLES. `PartitionTarget`'s base is at minimum the
//      first usable LBA, so the protective MBR, both headers and both entry arrays are unreachable
//      through it. The ONE exception is [`gpt::set_entry_type_guid`], which is off by default
//      (`as_esp: false`), runs on the DISK target, and edits exactly one 16-byte field.
//   3. EVERY REFUSAL IS NAMED ON THE WIRE AND IS A FIXTURE CASE. A guard nobody can see fire is an
//      absent one (LAWS §5). Each variant of [`Refusal`] carries a stable `reason=` token, and
//      `scripts/make-gpt-fixture.py` builds a disk carrying one partition per reason.
//   4. SATA IS REFUSED, ON THE WIRE, UNTIL AHCIWRITE LANDS. `drivers/block.rs`'s `Ahci` handle is
//      read-only in every cfg (the image compiles no ATA write opcode), so a SATA target is refused
//      `transport-read-only` here too — one layer up, where the operator can read it — rather than
//      failing as an I/O error three layers down. The rMBP's internal SSD is Peter's live disk; the
//      transport half is its own rung with its own metal risk and is not in this arc.
//
// WHAT THIS ARC DOES NOT DO. It does not write to SATA (see 4). It does not bless anything: on the
// 2012 rMBP the firmware boot selection is Apple's picker plus `bless` from Recovery (RULINGS R3 —
// *"if you want to do unattended reboots you cannot because there would be nobody to hold down
// option"*; rmbp-ledger A4), so making the installed partition the startup volume is the OPERATOR'S
// step and is written down as such in `docs/dev/OS/10_INSTALL/partition-install.md`. And it does not
// touch the disk's existing ESP: the honest first cut formats the TARGET partition FAT32 and lays
// `EFI/BOOT/BOOTX64.EFI` inside it, so the firmware picker can find it there. Whether this
// machine's picker lists a FAT partition that is not ESP-TYPED is a FIRMWARE FACT WE HAVE NOT
// MEASURED — marked metal-unproven in the doc, and the reason `--as-esp` exists at all.

use super::gpt;
use super::{hash, InstallError, InstallTarget};
use crate::drivers::block;
use alloc::string::String;
use alloc::vec::Vec;

const SECTOR: usize = 512;

/// How much of a partition's head the content probe reads. Eight sectors covers every signature we
/// key on — a FAT BPB at +0, an APFS container superblock magic at +32, an HFS+ volume header at
/// +1024 — and is also the window the "is this empty" answer is made over.
const PROBE_BYTES: usize = 8 * SECTOR;

/// Free space a target must have BEYOND the tree, so an install does not land on a volume with no
/// room to ever be updated. One mebibyte; small, but the point is that the number is stated.
const SLACK_BYTES: usize = 1024 * 1024;

/// The FAT32 cluster-count floor (fatgen: below 65525 clusters the volume is FAT16, not FAT32).
/// Mirrored from `fat32::format_esp`'s own refusal so the size check can answer BEFORE a write
/// begins instead of discovering it half way through a format.
const FAT32_MIN_CLUSTERS: u64 = 65525;

// ---------------------------------------------------------------------------------------------
// The content probe — what is ACTUALLY on a partition, read off the medium.
// ---------------------------------------------------------------------------------------------

/// What the first sectors of a partition say it holds. Read-only, and it is the fact the refusal
/// ladder turns on: a TYPE GUID is what somebody once declared, and the content is what is there.
/// Both are printed, because when they disagree that disagreement is the interesting thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Every byte of the probe window is zero. The only content an install may overwrite.
    Empty,
    /// A FAT boot sector: the jump instruction, the 0x55AA signature, a sane bytes-per-sector and a
    /// sane FAT count. Deliberately four conditions and not just 0x55AA — half the sectors on a
    /// partitioned disk end in 0x55AA.
    Fat,
    /// A native UnaFS superblock. A FRIEND in R25's three-clause model (HOME / FRIEND / STRANGER),
    /// not a stranger — but still not empty, and so still not ours to overwrite unasked.
    UnaFs,
    /// An APFS container superblock — `NXSB` at +32 (Apple File System Reference,
    /// `nx_superblock_t.nx_magic`). On the bench rMBP this is Catalina. It is the exact volume R25
    /// is about.
    Apfs,
    /// An HFS+ / HFSX volume header — `H+` or `HX` at +1024 (Apple TN1150).
    HfsPlus,
    /// The partition's TYPE GUID is the EFI System Partition GUID. Reported in the content column
    /// (rather than only in the type column) because it is what decides the refusal; when the ESP
    /// also carries bytes, `Content::Esp` is what the census prints and `esp_bytes` says what the
    /// byte probe found underneath.
    Esp,
    /// Non-zero, and none of the signatures above. UNKNOWN IS NOT EMPTY: a probe that cannot name
    /// what is there is the strongest possible reason to leave it alone.
    Unknown,
}

impl Content {
    /// The token the census and refusal lines print.
    pub fn tag(self) -> &'static str {
        match self {
            Content::Empty => "empty",
            Content::Fat => "FAT",
            Content::UnaFs => "UNAFS",
            Content::Apfs => "APFS",
            Content::HfsPlus => "HFS+",
            Content::Esp => "ESP",
            Content::Unknown => "unknown",
        }
    }
    /// May an install overwrite this? Only `Empty`. Every other answer — including `Unknown` — is no.
    pub fn is_installable(self) -> bool {
        matches!(self, Content::Empty)
    }
    /// A STRANGER's volume in R25's sense: somebody else's filesystem. `UnaFs` is excluded on
    /// purpose (it is a FRIEND), and so is `Esp` (a platform structure, not a foreign OS), and both
    /// are counted separately by [`Census`] so the whole-disk refusal line can say which is which.
    pub fn is_foreign(self) -> bool {
        matches!(self, Content::Fat | Content::Apfs | Content::HfsPlus | Content::Unknown)
    }
}

/// Probe one partition's head. READ-ONLY, and it reads through the DISK target at the partition's
/// absolute LBA rather than through a `PartitionTarget`, because this runs BEFORE anything has
/// decided the partition is a legitimate destination — building a write-capable handle to a volume
/// we have not yet cleared would be the wrong order of operations even though the handle is unused.
fn probe_content<T: InstallTarget>(t: &T, e: &gpt::GptEntryView) -> Result<Content, InstallError> {
    let want = core::cmp::min(PROBE_BYTES as u64, e.sectors() * SECTOR as u64) as usize;
    let sectors = want / SECTOR;
    let mut buf = alloc::vec![0u8; sectors * SECTOR];
    t.read_sectors(e.first_lba, &mut buf)?;
    Ok(classify_bytes(&buf, e.is_esp()))
}

/// The probe's decision, isolated from any storage so a fixture can pin it over synthetic bytes.
/// Order is deliberate: the most specific magic first, `Empty` only after every signature has been
/// ruled out, and `Unknown` as the catch-all — so a new filesystem nobody here has heard of lands
/// on "leave it alone" rather than on "looks blank to me".
pub fn classify_bytes(buf: &[u8], type_is_esp: bool) -> Content {
    let at = |o: usize, n: usize| -> Option<&[u8]> { buf.get(o..o + n) };

    if at(0, 5) == Some(b"UNAFS") {
        return Content::UnaFs;
    }
    if at(32, 4) == Some(b"NXSB") {
        return Content::Apfs;
    }
    if matches!(at(1024, 2), Some(b"H+") | Some(b"HX")) {
        return Content::HfsPlus;
    }
    if is_fat_bpb(buf) {
        return Content::Fat;
    }
    let blank = buf.iter().all(|&b| b == 0);
    if type_is_esp && blank {
        // A BLANK ESP. The type GUID is the operator's declaration about what this slot is for, so
        // it is reported as `ESP` rather than as `empty`: an installer that formatted it as a data
        // volume because it happened to be empty would be doing the thing this module prevents.
        //
        // AND THE CONJUNCTION IS LOAD-BEARING. `Content::Esp` is the ONLY content `--as-esp`
        // excuses, so if this arm also caught an ESP carrying bytes we merely failed to recognise,
        // `--as-esp` would license overwriting a LIVE ESP — the shared platform volume whose loss
        // unboots every other OS on the medium. A non-blank unrecognised ESP falls through to
        // `Unknown` below and is refused like any other volume nobody can name.
        return Content::Esp;
    }
    if blank {
        return Content::Empty;
    }
    Content::Unknown
}

/// TYPE GUIDs that belong to somebody else's operating system. THE SECOND, INDEPENDENT WITNESS.
///
/// WHY THIS LIST EXISTS AT ALL — it was not in the arc's design, and a go-red put it there. Blinding
/// the APFS arm of [`classify_bytes`] (go-red (a)) made the fixture install ONTO the APFS partition
/// and the neighbours check still reported `untouched=4/4 -> PASS`, because the baseline was "every
/// partition except the one the installer chose" and the installer had chosen the APFS one. LAWS §5
/// names that shape exactly: *"A true check can answer a different question than the one asked … if
/// those are different sentences, the gap is the error."* The content probe was the ONLY thing
/// standing between "empty" and "Catalina", and nothing cross-checked it.
///
/// A TYPE GUID is independent evidence: it is what the operator's partitioning tool DECLARED the
/// slot is for, it lives in the GPT rather than in the volume, and it is wrong in different
/// circumstances than a content probe is. So a slot declared Apple/Linux/Windows-system is refused
/// **whatever the bytes say** — including when the bytes say empty, which is precisely the state an
/// erased-but-not-repartitioned stranger's volume is in.
///
/// WHAT IS DELIBERATELY *NOT* HERE: Microsoft Basic Data (`EBD0A0A2-…`). That is what Disk Utility
/// stamps on the FAT partition Peter is told to make in the operator procedure, so listing it would
/// refuse the one target this arc exists to accept. Basic Data is the "no declaration" case and is
/// judged on content alone.
const FOREIGN_TYPE_GUIDS: &[([u8; 16], &str)] = &[
    // Apple APFS 7C3457EF-0000-11AA-AA11-00306543ECAC — Catalina's container on this bench.
    ([0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC], "apple-apfs"),
    // Apple HFS+ 48465300-0000-11AA-AA11-00306543ECAC
    ([0x00, 0x53, 0x46, 0x48, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC], "apple-hfs+"),
    // Apple Boot (Recovery HD) 426F6F74-0000-11AA-AA11-00306543ECAC — the volume ⌘R starts.
    ([0x74, 0x6F, 0x6F, 0x42, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC], "apple-boot"),
    // Apple Core Storage 53746F72-6167-11AA-AA11-00306543ECAC — a FileVault/Fusion member.
    ([0x72, 0x6F, 0x74, 0x53, 0x67, 0x61, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC], "apple-corestorage"),
    // Linux filesystem data 0FC63DAF-8483-4772-8E79-3D69D8477DE4
    ([0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4], "linux-fs"),
    // Linux LVM E6D6D379-F507-44C2-A23C-238F2A3DF928
    ([0x79, 0xD3, 0xD6, 0xE6, 0x07, 0xF5, 0xC2, 0x44, 0xA2, 0x3C, 0x23, 0x8F, 0x2A, 0x3D, 0xF9, 0x28], "linux-lvm"),
    // Linux RAID A19D880F-05FC-4D3B-A006-743F0F84911E
    ([0x0F, 0x88, 0x9D, 0xA1, 0xFC, 0x05, 0x3B, 0x4D, 0xA0, 0x06, 0x74, 0x3F, 0x0F, 0x84, 0x91, 0x1E], "linux-raid"),
    // Linux swap 0657FD6D-A4AB-43C4-84E5-0933C84B4F4F
    ([0x6D, 0xFD, 0x57, 0x06, 0xAB, 0xA4, 0xC4, 0x43, 0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B, 0x4F, 0x4F], "linux-swap"),
    // Microsoft Reserved E3C9E316-0B5C-4DB8-817D-F92DF00215AE
    ([0x16, 0xE3, 0xC9, 0xE3, 0x5C, 0x0B, 0xB8, 0x4D, 0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02, 0x15, 0xAE], "ms-reserved"),
    // Windows Recovery DE94BBA4-06D1-4D40-A16A-BFD50179D6AC
    ([0xA4, 0xBB, 0x94, 0xDE, 0xD1, 0x06, 0x40, 0x4D, 0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xD6, 0xAC], "windows-recovery"),
];

/// Is this slot DECLARED as somebody else's? Returns the name for the wire, so the refusal says
/// which foreign OS's type it found rather than only that it found one.
pub fn foreign_type(type_guid: &[u8; 16]) -> Option<&'static str> {
    FOREIGN_TYPE_GUIDS.iter().find(|(g, _)| g == type_guid).map(|(_, n)| *n)
}

/// A FAT boot sector, by four independent facts. Any one of them alone is a coincidence on a disk
/// full of arbitrary bytes; together they are a BPB.
fn is_fat_bpb(buf: &[u8]) -> bool {
    if buf.len() < SECTOR {
        return false;
    }
    if buf[510] != 0x55 || buf[511] != 0xAA {
        return false;
    }
    if buf[0] != 0xEB && buf[0] != 0xE9 {
        return false; // the x86 short/near jump every BPB opens with
    }
    let bytes_per_sector = u16::from_le_bytes([buf[11], buf[12]]);
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        return false;
    }
    let num_fats = buf[16];
    (1..=2).contains(&num_fats)
}

// ---------------------------------------------------------------------------------------------
// The pre-flight census — read-only, printed before anything is written.
// ---------------------------------------------------------------------------------------------

/// One partition as the census saw it.
pub struct CensusRow {
    pub entry: gpt::GptEntryView,
    pub content: Content,
}

/// What the disk carries, in full, before any decision is made about it.
pub struct Census {
    pub rows: Vec<CensusRow>,
    pub foreign: usize,
    pub friend: usize,
    pub empty: usize,
}

impl Census {
    pub fn row(&self, index: u32) -> Option<&CensusRow> {
        self.rows.iter().find(|r| r.entry.index == index)
    }
}

/// Walk the existing GPT and probe every partition's content. NOTHING IS WRITTEN and nothing is
/// decided here — the census is the evidence the refusals are then read against, and it is printed
/// first so a log of a refused install still says what the disk looked like.
pub fn census<T: InstallTarget>(t: &T) -> Result<Census, InstallError> {
    let table = gpt::read_table(t)?;
    let mut rows = Vec::new();
    let (mut foreign, mut friend, mut empty) = (0usize, 0usize, 0usize);
    for e in &table.entries {
        let content = probe_content(t, e)?;
        if content.is_foreign() {
            foreign += 1;
        } else if content == Content::UnaFs {
            friend += 1;
        } else if content == Content::Empty {
            empty += 1;
        }
        rows.push(CensusRow { entry: *e, content });
    }
    Ok(Census { rows, foreign, friend, empty })
}

/// Print the census. One rollup line plus one line per partition.
///
/// The arc's brief asked for a single line carrying every partition. It is emitted per-partition
/// instead, and that is a deliberate change with a reason: five partitions on one line is ~300
/// characters, one `awk` hit for five independent facts, and unreadable in a wrapped serial window.
/// Per-row lines are individually addressable (`awk 'index($0,"PINSTALL: census part=")'`), which is
/// what a fixture that must assert about ONE partition's content actually needs.
pub fn print_census(disk: &str, c: &Census) {
    serial_println!(
        ":: PINSTALL: census disk={} parts={} foreign={} friend={} empty={} ::",
        disk,
        c.rows.len(),
        c.foreign,
        c.friend,
        c.empty
    );
    for r in &c.rows {
        serial_println!(
            ":: PINSTALL: census part={} type={:08x} lba={}..{} sectors={} content={} ::",
            r.entry.index,
            r.entry.type_short(),
            r.entry.first_lba,
            r.entry.last_lba,
            r.entry.sectors(),
            r.content.tag()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The refusals.
// ---------------------------------------------------------------------------------------------

/// Every way this installer says no, each with a stable token on the wire. The token is the API: a
/// fixture asserts on `reason=<token>`, an operator reads it, and the docs' refusal table is keyed
/// on it. Renaming one is a contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A WHOLE-DISK target on a disk that carries somebody else's volumes. This is R25 stated as a
    /// guard: the existing `write_gpt` flow would lay a fresh table over the lot.
    DiskHasForeignVolumes { foreign: usize, friend: usize },
    /// The named partition already holds something. Carries WHAT, because "not empty" is a decision
    /// and "not empty, it is APFS" is evidence.
    PartitionNotEmpty(Content),
    /// The named partition's TYPE GUID is declared as somebody else's operating system — Apple
    /// APFS, HFS+, Recovery, Core Storage, a Linux or Windows system type. THE SECOND, INDEPENDENT
    /// WITNESS to `PartitionNotEmpty`, and it fires even when the bytes read as blank: an erased-
    /// but-not-repartitioned stranger's volume is exactly that state. Added because go-red (a)
    /// proved the content probe was load-bearing and unchecked — see `FOREIGN_TYPE_GUIDS`.
    PartitionForeignType { kind: &'static str },
    /// The named partition's type GUID is the EFI System Partition GUID. Refused unless the caller
    /// explicitly asked to write an ESP (`as_esp`), because an ESP is shared platform property:
    /// reformatting the disk's ESP can unboot every other OS on the medium.
    PartitionIsEsp,
    /// No such in-use slot in this table.
    NoSuchPartition,
    /// The partition cannot hold a FAT32 volume plus the tree plus slack.
    PartitionTooSmall { have_bytes: u64, need_bytes: u64 },
    /// The bound device is the volume this kernel booted from — INSTALL-SELF, unchanged by this arc
    /// and asked FIRST, because "this is the disk you are running from" outranks every other reason.
    BootDevice,
    /// The transport cannot write. Today this is every SATA disk: `drivers/block.rs`'s `Ahci` handle
    /// refuses writes in every cfg and the image compiles no ATA write opcode. Named here, one layer
    /// above the transport, so the answer an operator reads is a policy and not an I/O error.
    TransportReadOnly { transport: &'static str },
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::DiskHasForeignVolumes { .. } => "disk-has-foreign-volumes",
            Refusal::PartitionNotEmpty(_) => "partition-not-empty",
            Refusal::PartitionForeignType { .. } => "partition-foreign-type",
            Refusal::PartitionIsEsp => "partition-is-esp",
            Refusal::NoSuchPartition => "no-such-partition",
            Refusal::PartitionTooSmall { .. } => "partition-too-small",
            Refusal::BootDevice => "boot-device",
            Refusal::TransportReadOnly { .. } => "transport-read-only",
        }
    }

    /// Emit the refusal. `who` names what was asked for — `disk`, or `part=<i>`.
    pub fn say(self, who: &str) {
        match self {
            Refusal::DiskHasForeignVolumes { foreign, friend } => serial_println!(
                ":: PINSTALL: refusal target={} reason={} foreign={} friend={} -> guard OK ::",
                who,
                self.reason(),
                foreign,
                friend
            ),
            Refusal::PartitionNotEmpty(c) => serial_println!(
                ":: PINSTALL: refusal target={} reason={} content={} -> guard OK ::",
                who,
                self.reason(),
                c.tag()
            ),
            Refusal::PartitionTooSmall { have_bytes, need_bytes } => serial_println!(
                ":: PINSTALL: refusal target={} reason={} have={}B need={}B -> guard OK ::",
                who,
                self.reason(),
                have_bytes,
                need_bytes
            ),
            Refusal::PartitionForeignType { kind } => serial_println!(
                ":: PINSTALL: refusal target={} reason={} type={} -> guard OK ::",
                who,
                self.reason(),
                kind
            ),
            Refusal::TransportReadOnly { transport } => serial_println!(
                ":: PINSTALL: refusal target={} reason={} transport={} -> guard OK ::",
                who,
                self.reason(),
                transport
            ),
            _ => serial_println!(
                ":: PINSTALL: refusal target={} reason={} -> guard OK ::",
                who,
                self.reason()
            ),
        }
    }
}

/// Can the installer WRITE through this handle at all? A total match over the handle kinds this
/// build carries, so a future transport is a compile error here rather than an accidental yes.
///
/// `Global`/`Usb` are the two the engine has always written through. Everything else is a NO, and
/// each NO has the same shape: the block layer refuses one layer down as well, so this is the
/// second of two refusals — the one that can say WHY in words the operator reads.
pub fn transport_writable(h: block::BlockHandle) -> bool {
    match h {
        block::BlockHandle::Global | block::BlockHandle::Usb => true,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => false,
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::TegraSd => false,
        #[cfg(all(target_arch = "x86_64", feature = "ahci"))]
        block::BlockHandle::Ahci { .. } => false,
    }
}

/// The transport's name for the wire.
pub fn transport_name(h: block::BlockHandle) -> &'static str {
    match h {
        block::BlockHandle::Global => "global",
        block::BlockHandle::Usb => "usb",
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        block::BlockHandle::Sdhc => "sdhc",
        #[cfg(all(target_arch = "aarch64", feature = "tegra", feature = "sdmmc"))]
        block::BlockHandle::TegraSd => "tegra-sd",
        #[cfg(all(target_arch = "x86_64", feature = "ahci"))]
        block::BlockHandle::Ahci { .. } => "ahci",
    }
}

/// Bytes a partition must have to hold `tree_bytes` as a FAT32 volume with slack. Returns the
/// requirement so the refusal can print BOTH numbers — a size refusal that does not say how much
/// was needed cannot be acted on.
fn size_requirement(tree_bytes: usize) -> u64 {
    // The FAT32 floor dominates every tree this installer writes today, but both terms are summed
    // rather than max'd so a future large tree is covered by the same expression.
    FAT32_MIN_CLUSTERS * SECTOR as u64 + tree_bytes as u64 + SLACK_BYTES as u64
}

/// The whole refusal ladder for ONE named partition, evaluated in order of what outranks what, with
/// NO writes performed by any of it. Returns `Ok(())` only if every guard passed.
///
/// ORDER IS THE DESIGN. Boot device first (a blank boot device is still a boot device — the engine's
/// own rule at `install/mod.rs:468-473`). Transport next, because a disk we cannot write to makes
/// every later question moot. Then existence, then type, then content, then size: each one is
/// cheaper and more specific than the last, and each answers with the reason an operator can act on.
#[allow(clippy::result_large_err)]
pub fn check_partition(
    census: &Census,
    id: block::BlockDeviceId,
    index: u32,
    tree_bytes: usize,
    as_esp: bool,
) -> Result<(), Refusal> {
    if super::selfguard::refuses(id) {
        return Err(Refusal::BootDevice);
    }
    if !transport_writable(id.handle) {
        return Err(Refusal::TransportReadOnly { transport: transport_name(id.handle) });
    }
    let row = census.row(index).ok_or(Refusal::NoSuchPartition)?;
    if row.entry.is_esp() && !as_esp {
        return Err(Refusal::PartitionIsEsp);
    }
    // CONTENT FIRST, then the DECLARATION. Both refuse; the order decides which sentence the
    // operator reads, and what is actually on the volume is more informative than what somebody
    // once declared the slot was for. The type check below is the BACKSTOP — the half that
    // survives a content probe which is wrong, absent, or looking at an erased-but-not-
    // repartitioned volume, the state where the bytes are blank and the disk is still not ours.
    // `--as-esp` reaches NEITHER of them as a general override. It excuses exactly one content:
    // a blank ESP-typed slot (`Content::Esp`, which by construction above means ESP-typed AND
    // all-zero). It does not excuse an ESP with a filesystem on it, it does not excuse an ESP
    // carrying bytes we could not name, and it does not excuse a foreign type GUID at all. A
    // switch about which type a partition ends up carrying must never widen into a licence to
    // overwrite a live volume — least of all the ESP, which every other OS on the disk boots via.
    let excused_by_as_esp = as_esp && row.entry.is_esp() && row.content == Content::Esp;
    if !row.content.is_installable() && !excused_by_as_esp {
        return Err(Refusal::PartitionNotEmpty(row.content));
    }
    if let Some(kind) = foreign_type(&row.entry.type_guid) {
        return Err(Refusal::PartitionForeignType { kind });
    }
    let have = row.entry.sectors() * SECTOR as u64;
    let need = size_requirement(tree_bytes);
    if have < need {
        return Err(Refusal::PartitionTooSmall { have_bytes: have, need_bytes: need });
    }
    Ok(())
}

/// The WHOLE-DISK question, asked of the same census. This is the arm that did not exist before: the
/// disk-level installer had `blank_check`, which answers "are the first 64 sectors zero" — true of
/// no partitioned disk on earth, and therefore an answer that never distinguished a stranger's disk
/// from a scratch one. A GPT that parses at all means partitions exist; whether any of them is
/// somebody's is what this asks.
#[allow(clippy::result_large_err)]
pub fn check_whole_disk(census: &Census) -> Result<(), Refusal> {
    if census.foreign > 0 || census.friend > 0 {
        return Err(Refusal::DiskHasForeignVolumes {
            foreign: census.foreign,
            friend: census.friend,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The partition target — the bounds check that makes invariant 1 mechanical.
// ---------------------------------------------------------------------------------------------

/// An [`InstallTarget`] that IS one partition. Its LBA 0 is the partition's first sector and its
/// capacity is the partition's length, so every existing engine primitive — `fat32::format_esp`,
/// `fat32::TreeWriter`, `clone::write_snapshot`, `verify_extents` — drives it unchanged and NONE of
/// them can name a sector outside it.
///
/// This is why the arc adds a TYPE rather than a check. A check ("assert lba >= first") is a
/// statement about the code that currently exists; a target whose address space is the partition is
/// a statement about every caller there will ever be, including the ones written after this comment.
pub struct PartitionTarget<'a, T: InstallTarget> {
    disk: &'a mut T,
    first_lba: u64,
    sectors: u64,
    index: u32,
    id: String,
}

impl<'a, T: InstallTarget> PartitionTarget<'a, T> {
    /// Bind to one entry of a census the caller has already read. Refuses an extent that escapes
    /// the disk — the GPT reader already bounds entries against `last_usable`, so this is the second
    /// of two checks and exists because the first one lives in a different file.
    pub fn new(disk: &'a mut T, e: &gpt::GptEntryView) -> Result<Self, InstallError> {
        let cap = disk.capacity_sectors();
        if e.first_lba == 0 || e.last_lba >= cap || e.last_lba < e.first_lba {
            return Err(InstallError::BadLba);
        }
        let id = alloc::format!("part{}@{}..{}", e.index, e.first_lba, e.last_lba);
        Ok(Self { disk, first_lba: e.first_lba, sectors: e.sectors(), index: e.index, id })
    }

    pub fn index(&self) -> u32 {
        self.index
    }
    /// The partition's base on the disk — for the one field (`BPB.hidden_sectors`) that must carry
    /// an ABSOLUTE LBA even though every write is relative.
    pub fn first_lba(&self) -> u64 {
        self.first_lba
    }

    /// Translate and bound. `BadLba` — never a clamp, never a wrap — for anything that would leave
    /// the partition, because a clamped write is still a write to a sector nobody named.
    fn map(&self, lba: u64, buf_len: usize) -> Result<u64, InstallError> {
        let n = (buf_len / SECTOR) as u64;
        let end = lba.checked_add(n).ok_or(InstallError::BadLba)?;
        if end > self.sectors {
            return Err(InstallError::BadLba);
        }
        self.first_lba.checked_add(lba).ok_or(InstallError::BadLba)
    }
}

impl<T: InstallTarget> InstallTarget for PartitionTarget<'_, T> {
    fn capacity_sectors(&self) -> u64 {
        self.sectors
    }
    fn id(&self) -> String {
        self.id.clone()
    }
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), InstallError> {
        let abs = self.map(lba, buf.len())?;
        self.disk.read_sectors(abs, buf)
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), InstallError> {
        let abs = self.map(lba, buf.len())?;
        self.disk.write_sectors(abs, buf)
    }
}

// ---------------------------------------------------------------------------------------------
// The tree that gets written.
// ---------------------------------------------------------------------------------------------

/// Build the boot tree to lay into the target partition, as a [`super::clone::SnapTree`] — the same
/// buffered-tree shape the Pi's self-clone mirrors from a mounted volume, so ONE writer
/// (`clone::write_snapshot`) serves both and the directory/extent machinery is not forked.
///
/// WHAT IS SYNTHETIC HERE AND WHY IT IS SAID OUT LOUD. On metal the source of this tree is the
/// running system's own boot volume, read through `clone::snapshot(&FatFs)`. In QEMU there is no
/// such source: the boot ESP is a separate `ide-hd` the kernel has no driver bound to (the same gap
/// `selfguard::live_media_leg` documents), and the ONE enumerated disk is the fixture. So the
/// fixture leg builds a tree of the right SHAPE — `EFI/BOOT/BOOTX64.EFI`, the kernel image, and the
/// source-along pair `SRC.TGZ`/`SRC.SHA` that SELFHOST-2 reads off a boot medium — with synthetic
/// deterministic contents. What that proves is the WRITE path: layout, directory chains, extents,
/// and content verification. What it does not prove is the read of a real source tree, which is
/// `clone::snapshot`'s half and is already exercised by the Pi flow.
fn demo_tree() -> super::clone::SnapTree {
    use super::clone::{SnapDir, SnapFile, SnapTree};

    /// Deterministic, non-degenerate filler — not all-zero and not all-one, so a verifier that
    /// compared against a constant would not pass by accident.
    fn body(tag: &str, len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        v.extend_from_slice(tag.as_bytes());
        let mut i: usize = 0;
        while v.len() < len {
            v.push(((i.wrapping_mul(131).wrapping_add(tag.len())) & 0xFF) as u8);
            i += 1;
        }
        v
    }
    fn file(name: &str, tag: &str, len: usize) -> SnapFile {
        SnapFile { name: String::from(name), data: body(tag, len) }
    }

    let boot = SnapDir {
        files: alloc::vec![file("BOOTX64.EFI", "UNAOS-PARTINSTALL-LOADER\n", 12_288)],
        subdirs: Vec::new(),
    };
    let efi = SnapDir { files: Vec::new(), subdirs: alloc::vec![(String::from("BOOT"), boot)] };
    let root = SnapDir {
        files: alloc::vec![
            file("KERNEL.ELF", "UNAOS-PARTINSTALL-KERNEL\n", 40_960),
            file("SRC.TGZ", "UNAOS-PARTINSTALL-SOURCE\n", 8_192),
            file("SRC.SHA", "UNAOS-PARTINSTALL-SOURCE-DIGEST\n", 512),
        ],
        subdirs: alloc::vec![(String::from("EFI"), efi)],
    };

    let (mut file_count, mut total_bytes) = (0usize, 0usize);
    fn tally(d: &SnapDir, n: &mut usize, b: &mut usize) {
        for f in &d.files {
            *n += 1;
            *b += f.data.len();
        }
        for (_, s) in &d.subdirs {
            tally(s, n, b);
        }
    }
    tally(&root, &mut file_count, &mut total_bytes);
    SnapTree { root, file_count, total_bytes }
}

// ---------------------------------------------------------------------------------------------
// The write.
// ---------------------------------------------------------------------------------------------

/// What an install actually did, for the caller's verdict line.
pub struct Written {
    pub index: u32,
    pub files: usize,
    pub bytes: usize,
    pub verified: usize,
}

/// Format the named partition FAT32 and mirror the boot tree into it, verifying every file by
/// content read-back. THE CALLER HAS ALREADY RUN [`check_partition`]; this function performs no
/// policy of its own beyond the bounds the target enforces. Splitting it that way is deliberate — a
/// writer that also decided whether to write could be reached by a future caller that forgot to ask.
fn write_partition<T: InstallTarget>(
    disk: &mut T,
    e: &gpt::GptEntryView,
    tree: &super::clone::SnapTree,
) -> Result<Written, InstallError> {
    let index = e.index;
    let mut pt = PartitionTarget::new(disk, e)?;
    let sectors = pt.capacity_sectors();
    let part_lba = pt.first_lba();

    // 1) Zero the FAT metadata region. `format_esp` documents a blank precondition: it writes the
    //    defining structures and assumes the rest of reserved+FAT is already zero, because a stale
    //    FAT entry left by whatever was here before would FORGE AN ALLOCATION — a file claiming
    //    clusters nothing wrote. The content probe proved the first 8 sectors are zero; it proved
    //    nothing about sector 1500. So zero exactly the region `blank_region_sectors` names: no more
    //    (the data area's free clusters may hold stale bytes harmlessly) and no less.
    let blank = super::fat32::blank_region_sectors(sectors)?;
    let zero = [0u8; SECTOR];
    for s in 0..blank {
        pt.write_sectors(s, &zero)?;
    }

    // 2) Format. `format_esp` takes the volume's base LBA; through a `PartitionTarget` that base is
    //    ZERO, which is the whole point — the formatter cannot address the disk.
    let geom = super::fat32::format_esp(&mut pt, 0, sectors)?;

    // 3) Fix up BPB.hidden_sectors. It is the ONE field in a FAT boot sector that must carry the
    //    partition's ABSOLUTE LBA (fatgen: "the number of hidden sectors preceding this partition"),
    //    and `format_esp` filled it with the base it was handed — 0 — because through this target
    //    that is genuinely where the volume starts. Patch both the boot sector and its backup at
    //    volume sector 6. Nothing in the in-tree reader consults this field (it gets the partition's
    //    start from the GPT), so this is for OTHER readers: it is what makes the volume a correct
    //    FAT32 partition rather than one that only our own code can make sense of.
    for vol_sector in [0u64, 6u64] {
        let mut bs = [0u8; SECTOR];
        pt.read_sectors(vol_sector, &mut bs)?;
        bs[28..32].copy_from_slice(&(part_lba as u32).to_le_bytes());
        pt.write_sectors(vol_sector, &bs)?;
    }

    // 4) Mirror the tree — `clone.rs`'s writer, unchanged, driven through the bounded target.
    let recs = super::clone::write_snapshot(&mut pt, geom, tree)?;

    // 5) VERIFY BY CONTENT, per file, by re-reading the exact extents the writer recorded and
    //    SHA-256ing them (clone.rs's rule). Not "the write returned Ok" — the bytes are read back
    //    off the medium and hashed.
    let mut verified = 0usize;
    let mut bytes = 0usize;
    for r in &recs {
        if super::verify_extents(&pt, &r.extents, &r.sha)? {
            verified += 1;
            bytes += r.size;
        } else {
            serial_println!(
                ":: PINSTALL: verify part={} file={} => MISMATCH ::",
                index,
                r.path.as_str()
            );
        }
    }
    Ok(Written { index, files: recs.len(), bytes, verified })
}

/// THE OPERATOR ENTRY POINT: install into the partition the operator NAMED, on the device the
/// operator SELECTED.
///
/// `as_esp` is the `--as-esp` switch and DEFAULTS OFF at every caller. When set, the target's type
/// GUID is changed to the EFI System Partition GUID after a successful write — the one partition-
/// table edit in this arc, on the disk target, through `gpt::set_entry_type_guid`, which rewrites
/// both headers with fresh CRCs and re-validates. It is off by default because on this machine we
/// have NOT measured whether the firmware picker needs it: the loader reads its own volume
/// (`bootloader/src` resolves files through its LoadedImage device handle, not by hunting the disk
/// for an ESP), and the picker is believed to list any FAT volume carrying `EFI/BOOT/BOOTX64.EFI` —
/// but that is a FIRMWARE FACT AND IT IS METAL-UNPROVEN. If the picker turns out to require the ESP
/// type, `--as-esp` is the answer and the operator asks for it knowingly.
pub fn install_into_partition(
    sel: block::BlockDeviceId,
    index: u32,
    as_esp: bool,
) -> Result<Written, InstallError> {
    let mut disk = super::BlockTarget::bind_id(sel)?;
    let c = census(&disk)?;
    print_census(&disk.id(), &c);
    let tree = demo_tree();
    if let Err(r) = check_partition(&c, sel, index, tree.total_bytes, as_esp) {
        r.say(&alloc::format!("part{}", index));
        return Err(refusal_error(r));
    }
    let entry = c.row(index).ok_or(InstallError::BadArg)?.entry;
    let w = write_partition(&mut disk, &entry, &tree)?;
    if as_esp {
        gpt::set_entry_type_guid(&mut disk, index, &gpt::ESP_TYPE_GUID)?;
        serial_println!(":: PINSTALL: part={} type GUID set to ESP (--as-esp) ::", index);
    }
    Ok(w)
}

/// Map a refusal onto the engine's error type. Every arm is a REFUSAL — nothing was written — and
/// the distinctions the engine already draws (`BootDevice`, `NotReady`) are preserved so a caller
/// that only reads the error still gets the honest kind.
fn refusal_error(r: Refusal) -> InstallError {
    match r {
        Refusal::BootDevice => InstallError::BootDevice,
        Refusal::TransportReadOnly { .. } => InstallError::NotReady,
        Refusal::PartitionTooSmall { .. } => InstallError::TooSmall,
        Refusal::NoSuchPartition => InstallError::BadArg,
        Refusal::PartitionForeignType { .. } => InstallError::NotBlank,
        _ => InstallError::NotBlank,
    }
}

// ---------------------------------------------------------------------------------------------
// The fixture leg.
// ---------------------------------------------------------------------------------------------

/// SHA-256 of a partition's first 4 KiB — the neighbour fingerprint, taken before and after the
/// install. This is the check that turns invariant 1 from a claim into a measurement: if any write
/// escaped the target, the disk's other partitions change and these digests differ.
fn head_sha<T: InstallTarget>(t: &T, e: &gpt::GptEntryView) -> Result<[u8; 32], InstallError> {
    let n = core::cmp::min(8u64, e.sectors()) as usize;
    let mut buf = alloc::vec![0u8; n * SECTOR];
    t.read_sectors(e.first_lba, &mut buf)?;
    Ok(hash::sha256(&buf))
}

/// One-shot fixture driver, `installdemo`-armed, run from [`super::install_probe_once`].
///
/// Returns `true` when it took the boot: the disk carried a VALID GPT, so this is a partition-install
/// fixture and the blank-disk engine demo (`run_demo`) would only print a `NotBlank` refusal about
/// it. Returns `false` on a disk with no readable GPT — the ordinary `UNAOS_INSTALLDEMO` blank
/// scratch — leaving that leg's behaviour exactly as it was.
///
/// THE SELECTION IS UNATTENDED AND THAT IS ONLY ACCEPTABLE HERE. R25 forbids the kernel writing on
/// its own initiative, and rmbp-ledger B91 flags the existing unattended witness path as "the exact
/// shape R25 forbids". This function is the same shape and rides the same arming: it exists ONLY
/// under `installdemo`, against a disk the build attached for it, and it is NOT the operator path —
/// that is [`install_into_partition`], which takes the index from its caller. The distinction is
/// written here rather than left implicit because the next reader will be deciding whether to widen
/// it, and the answer is no.
pub fn run_fixture() -> bool {
    let Ok(mut disk) = super::BlockTarget::bind() else { return false };
    let Ok(c) = census(&disk) else {
        // No parseable GPT. Not a partition-install disk; say so once and hand the leg back.
        return false;
    };
    let id = disk.id();
    let sel = disk.identity();
    serial_println!(":: PINSTALL: fixture start — disk carries a valid GPT ::");
    print_census(&id, &c);

    let tree = demo_tree();

    // --- REFUSAL 1: the whole disk. ---
    match check_whole_disk(&c) {
        Err(r) => r.say("disk"),
        Ok(()) => {
            serial_println!(
                ":: PINSTALL: whole-disk target ACCEPTED on a disk with no foreign volumes — the fixture expects a refusal here => FAIL ::"
            );
            return true;
        }
    }

    // --- REFUSALS 2..n: every partition that is not installable, each by its own reason. ---
    // Every row is asked, so the log carries a verdict per partition rather than a sample: a
    // refusal table with an untested row is an untested guard.
    let mut target: Option<gpt::GptEntryView> = None;
    for row in &c.rows {
        match check_partition(&c, sel, row.entry.index, tree.total_bytes, false) {
            Err(r) => r.say(&alloc::format!("part{}", row.entry.index)),
            Ok(()) => {
                if target.is_none() {
                    target = Some(row.entry);
                    serial_println!(
                        ":: PINSTALL: part={} passes every guard — fixture names it the target ::",
                        row.entry.index
                    );
                } else {
                    serial_println!(
                        ":: PINSTALL: part={} also eligible (not selected) ::",
                        row.entry.index
                    );
                }
            }
        }
    }

    // --- The SATA answer, on the wire, whether or not this build has the handle. ---
    transport_leg();

    let Some(entry) = target else {
        serial_println!(":: PINSTALL: no eligible partition on this disk => FAIL ::");
        return true;
    };

    // --- THE FIXTURE'S OWN EXPECTATION, and it is not derived from the code under test. ---
    //
    // THIS PARAGRAPH IS A GO-RED FINDING, kept as the record of why the check has this shape. The
    // first version took the neighbour set as "every partition except the one the installer chose",
    // and go-red (a) — blinding the APFS arm of `classify_bytes` — then installed straight onto the
    // APFS partition and the check reported `untouched=4/4 -> PASS`: the destroyed volume was not in
    // the baseline, because the thing under test had picked the baseline. LAWS §5's exact shape —
    // "say what the check measures and what the decision needs; if those are different sentences,
    // the gap is the error." It measured "did we write outside our own choice" and the decision
    // needed "did we write outside the one partition that was ours to write".
    //
    // So the fixture states the answer INDEPENDENTLY: `scripts/make-gpt-fixture.py` builds slot
    // FIXTURE_TARGET_INDEX as the empty 48 MiB UNAOS-TARGET and every other slot as something that
    // must be refused. The neighbour set is every OTHER slot, fixed before the selection runs, and a
    // selection that disagrees with the fixture is itself a failure. Both halves must hold.
    const FIXTURE_TARGET_INDEX: u32 = 2;
    if entry.index != FIXTURE_TARGET_INDEX {
        serial_println!(
            ":: PINSTALL: selection part={} but the fixture built part{} as the only writable slot => FAIL ::",
            entry.index, FIXTURE_TARGET_INDEX
        );
    }

    // --- Neighbour fingerprints BEFORE the write, over every slot the FIXTURE calls a neighbour. ---
    let mut before: Vec<(u32, [u8; 32])> = Vec::new();
    for row in &c.rows {
        if row.entry.index == FIXTURE_TARGET_INDEX {
            continue;
        }
        match head_sha(&disk, &row.entry) {
            Ok(s) => before.push((row.entry.index, s)),
            Err(e) => {
                serial_println!(":: PINSTALL: neighbour part={} pre-read failed ({:?}) => FAIL ::", row.entry.index, e);
                return true;
            }
        }
    }

    // --- THE WRITE. ---
    let w = match write_partition(&mut disk, &entry, &tree) {
        Ok(w) => w,
        Err(e) => {
            // `err={:?} -> FAIL ::`, not `=> FAIL ({:?}) ::`. mbench installs `-> FAIL` and `FAIL ::`
            // as DEFAULT_FORBIDs into every spec, and `FAIL (NotReady) ::` matches NEITHER — the
            // parenthesis breaks `FAIL ::` and the arrow is wrong for `-> FAIL`. A write error would
            // have printed and left the run GREEN. Found by auditing every failure line in this file
            // against the forbid set rather than by a run: this path needs an I/O fault to fire, so
            // no fixture here exercises it and it is UNWITNESSED — which is exactly why it had to be
            // read. (`install/mod.rs`'s own `engine => FAIL ({:?})` has the same shape; not mine.)
            serial_println!(":: PINSTALL: write part={} err={:?} -> FAIL ::", entry.index, e);
            return true;
        }
    };
    if w.verified != w.files || w.files == 0 {
        serial_println!(
            ":: PINSTALL: wrote part={} fat32 tree={} bytes={} verified={}/{} -> FAIL ::",
            w.index, w.files, w.bytes, w.verified, w.files
        );
        return true;
    }
    serial_println!(
        ":: PINSTALL: wrote part={} fat32 tree={} bytes={} verified={}/{} -> PASS ::",
        w.index, w.files, w.bytes, w.verified, w.files
    );

    // --- RE-CENSUS: the table must still validate, and every neighbour must read as it did. ---
    let Ok(after_census) = census(&disk) else {
        serial_println!(":: PINSTALL: post-write census — GPT no longer parses => FAIL ::");
        return true;
    };
    print_census(&id, &after_census);

    let mut untouched = 0usize;
    for (index, sha) in &before {
        let Some(row) = after_census.row(*index) else {
            serial_println!(":: PINSTALL: neighbour part={} vanished from the table => FAIL ::", index);
            return true;
        };
        match head_sha(&disk, &row.entry) {
            Ok(now) if &now == sha => untouched += 1,
            Ok(_) => serial_println!(
                ":: PINSTALL: neighbour part={} CHANGED across the install ::",
                index
            ),
            Err(e) => serial_println!(
                ":: PINSTALL: neighbour part={} post-read failed ({:?}) ::",
                index, e
            ),
        }
    }
    if untouched == before.len() {
        serial_println!(":: PINSTALL: neighbours untouched={}/{} -> PASS ::", untouched, before.len());
    } else {
        serial_println!(":: PINSTALL: neighbours untouched={}/{} -> FAIL ::", untouched, before.len());
    }
    true
}

/// The transport refusal, stated on every run. Read THREE-VALUED, deliberately: a build with the
/// `ahci` handle prints the refusal itself; a build without it says the arm is ABSENT rather than
/// letting silence read as "SATA is allowed". A guard you cannot see is indistinguishable from one
/// that is not there (LAWS §5), and so is a guard whose build does not contain it.
fn transport_leg() {
    #[cfg(all(target_arch = "x86_64", feature = "ahci"))]
    {
        let h = block::BlockHandle::Ahci { port: 0 };
        if transport_writable(h) {
            serial_println!(
                ":: PINSTALL: SATA transport reports WRITABLE — AHCIWRITE has not landed and this must be a refusal => FAIL ::"
            );
        } else {
            Refusal::TransportReadOnly { transport: transport_name(h) }.say("disk=sata");
        }
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
    serial_println!(
        ":: PINSTALL: SATA transport arm ABSENT in this build (no `ahci` feature) — refusal unmeasured here ::"
    );
}

/// The probe `install_probe_once` calls. One-shot, and it answers the only question the caller has:
/// did the partition fixture take this boot?
pub fn probe_once() -> bool {
    use core::sync::atomic::{AtomicU8, Ordering};
    // Three states, not two: UNRUN, RAN-AND-TOOK-IT, RAN-AND-DECLINED. A bare `bool` done-flag would
    // make the second call answer "already done" with no way to say which way it went.
    const UNRUN: u8 = 0;
    const TOOK: u8 = 1;
    const DECLINED: u8 = 2;
    static STATE: AtomicU8 = AtomicU8::new(UNRUN);
    match STATE.load(Ordering::Relaxed) {
        TOOK => return true,
        DECLINED => return false,
        _ => {}
    }
    let took = run_fixture();
    STATE.store(if took { TOOK } else { DECLINED }, Ordering::Relaxed);
    took
}
