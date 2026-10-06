// SPDX-License-Identifier: LGPL-3.0-or-later
//! EXFAT (B392) host KATs on a volume the HOST's tools wrote (`tests/mkfixture.sh`). The image path is
//! `EXFAT_FIXTURE`; without it every test here says so and returns (the unit KATs in `src/lib.rs` still run).

use exfat_core::{locate, Error, SectorRead, Volume, SECTOR};

struct Img(Vec<u8>);
impl SectorRead for Img {
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), Error> {
        assert!(buf.len() % SECTOR == 0 && !buf.is_empty());
        let a = lba as usize * SECTOR;
        let s = self.0.get(a..a + buf.len()).ok_or(Error::Io)?;
        buf.copy_from_slice(s);
        Ok(())
    }
}

fn fixture() -> Option<Img> {
    match std::env::var("EXFAT_FIXTURE") {
        Ok(p) => Some(Img(std::fs::read(p).expect("EXFAT_FIXTURE unreadable"))),
        Err(_) => {
            eprintln!("EXFAT_FIXTURE unset: image KATs skipped (make one with tests/mkfixture.sh)");
            None
        }
    }
}

fn pat(n: usize, a: usize, b: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * a + b) & 0xff) as u8).collect()
}

#[test]
fn mount_label_geometry() {
    let Some(d) = fixture() else { return };
    let v = Volume::mount(&d, 0, (d.0.len() / SECTOR) as u64).expect("mount");
    assert_eq!(v.label(), "UNAEXFAT");
    assert_eq!(v.cluster_bytes(), 4096);
    assert!(!v.backup_boot);
    assert!(v.upcase().mappings() > 500, "the recommended up-case table maps ~1.5k units");
    let a = v.audit(&d);
    assert_eq!(a.passed(), a.total(), "audit: {:?}", a.checks);
    assert_eq!(a.total(), 8);
    assert_eq!(a.files, 7);
}

#[test]
fn readdir_and_names() {
    let Some(d) = fixture() else { return };
    let v = Volume::mount(&d, 0, 0).unwrap();
    let l = v.read_dir(&d, &v.root()).unwrap();
    assert_eq!((l.bad_sets, l.bad_hash), (0, 0));
    let mut names: Vec<_> = l.nodes.iter().map(|n| n.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        ["ReadMe.txt", "Sub", "empty.bin", "frag_a.bin", "frag_b.bin", "pattern.bin", "Ünïcødé — 日本語 😀 a long name past fifteen units.txt"]
    );
    let sub = v.lookup(&d, "/Sub").unwrap();
    assert!(sub.is_dir());
    let s = v.read_dir(&d, &sub).unwrap();
    assert_eq!(s.nodes.len(), 151, "150 files + Deeper; the directory spans several 4 KiB clusters");
    assert!(sub.data_len > 4096);
    assert_eq!(v.read(&d, &v.lookup(&d, "/Sub/f149.txt").unwrap(), 0, 64).unwrap(), b"file 149\n");
    assert_eq!(v.read(&d, &v.lookup(&d, "Sub/Deeper/leaf.txt").unwrap(), 0, 64).unwrap(), b"leaf\n");
}

#[test]
fn lookup_is_case_blind_through_the_upcase_table() {
    let Some(d) = fixture() else { return };
    let v = Volume::mount(&d, 0, 0).unwrap();
    assert_eq!(v.read(&d, &v.lookup(&d, "/readme.TXT").unwrap(), 0, 100).unwrap(), b"Hello, exFAT.\n");
    // Ü/ü, Ï/ï, Ø/ø are mapped by the volume's table, not by ASCII rules.
    let n = v.lookup(&d, "/üNÏCØDÉ — 日本語 😀 A LONG NAME PAST FIFTEEN UNITS.TXT").unwrap();
    assert_eq!(v.read(&d, &n, 0, 100).unwrap(), b"unicode\n");
    assert_eq!(v.lookup(&d, "/nope").unwrap_err(), Error::NotFound);
    assert_eq!(v.lookup(&d, "/ReadMe.txt/x").unwrap_err(), Error::NotADirectory);
}

#[test]
fn contiguous_and_chained_reads() {
    let Some(d) = fixture() else { return };
    let v = Volume::mount(&d, 0, 0).unwrap();
    let p = v.lookup(&d, "/pattern.bin").unwrap();
    assert_eq!(p.data_len, 300000);
    assert_eq!(v.read(&d, &p, 0, 1 << 20).unwrap(), pat(300000, 31, 7));
    // An unaligned window across cluster boundaries.
    assert_eq!(v.read(&d, &p, 4095, 9000).unwrap(), pat(300000, 31, 7)[4095..13095].to_vec());
    let a = v.lookup(&d, "/frag_a.bin").unwrap();
    let b = v.lookup(&d, "/frag_b.bin").unwrap();
    assert!(!a.contiguous || !b.contiguous, "the interleaved writes leave at least one file FAT-chained");
    assert_eq!(v.read(&d, &a, 0, 1 << 20).unwrap(), pat(100000, 7, 1));
    assert_eq!(v.read(&d, &b, 0, 1 << 20).unwrap(), pat(100000, 13, 3));
    assert_eq!(v.read(&d, &b, 50001, 7).unwrap(), pat(100000, 13, 3)[50001..50008].to_vec());
    assert_eq!(v.read(&d, &a, 100000, 10).unwrap(), Vec::<u8>::new());
    let e = v.lookup(&d, "/empty.bin").unwrap();
    assert_eq!((e.data_len, v.read(&d, &e, 0, 10).unwrap().len()), (0, 0));
    eprintln!("pattern contiguous={} frag_a contiguous={} frag_b contiguous={}", p.contiguous, a.contiguous, b.contiguous);
}

#[test]
fn locate_through_mbr_and_backup_boot_region() {
    let Some(d) = fixture() else { return };
    // The same volume as MBR partition 1 at LBA 2048 (the SD card shape, part_type=7).
    let mut disk = vec![0u8; 2048 * SECTOR];
    let blocks = (d.0.len() / SECTOR) as u32;
    disk[446 + 4] = 0x07;
    disk[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
    disk[446 + 12..446 + 16].copy_from_slice(&blocks.to_le_bytes());
    disk[510] = 0x55;
    disk[511] = 0xAA;
    disk.extend_from_slice(&d.0);
    let disk = Img(disk);
    let (lba, n, slot) = locate(&disk, (disk.0.len() / SECTOR) as u64).unwrap();
    assert_eq!((lba, n, slot), (2048, blocks as u64, 1));
    let v = Volume::mount(&disk, lba, n).unwrap();
    assert_eq!(v.label(), "UNAEXFAT");
    assert_eq!(v.read(&disk, &v.lookup(&disk, "/Sub/f007.txt").unwrap(), 0, 64).unwrap(), b"file 7\n");
    // A damaged main boot region: the backup (sector 12) mounts, and the audit says so.
    let mut bad = d.0.clone();
    bad[200] ^= 0xFF;
    let bad = Img(bad);
    let v = Volume::mount(&bad, 0, 0).unwrap();
    assert!(v.backup_boot);
    assert_eq!(v.audit(&bad).first_fail(), Some("boot_csum"));
    // Both regions damaged: refused.
    let mut worse = bad.0.clone();
    worse[12 * SECTOR + 200] ^= 0xFF;
    assert_eq!(Volume::mount(&Img(worse), 0, 0).err(), Some(Error::BootChecksum));
}

#[test]
fn a_damaged_entry_set_is_skipped_and_counted() {
    let Some(d) = fixture() else { return };
    let v = Volume::mount(&d, 0, 0).unwrap();
    // Find ReadMe.txt's file entry in the root cluster and flip a byte of its name.
    let root_lba = {
        let l = v.read_dir(&d, &v.root()).unwrap();
        assert!(l.nodes.iter().any(|n| n.name == "ReadMe.txt"));
        // cluster heap offset * 1 (512-byte sectors) + (root - 2) * 8
        let g = &v.geo;
        g.heap_offset as usize + ((g.root_cluster - 2) as usize) * 8
    };
    let mut img = d.0.clone();
    let base = root_lba * SECTOR;
    let needle: Vec<u8> = "ReadMe".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let at = img[base..base + 4096].windows(needle.len()).position(|w| w == needle.as_slice()).unwrap();
    img[base + at] = b'X';
    let img = Img(img);
    let l = v.read_dir(&img, &v.root()).unwrap();
    assert_eq!(l.bad_sets, 1);
    assert!(!l.nodes.iter().any(|n| n.name.ends_with("eadMe.txt")));
    assert_eq!(v.audit(&img).first_fail(), Some("set_csum"));
}
