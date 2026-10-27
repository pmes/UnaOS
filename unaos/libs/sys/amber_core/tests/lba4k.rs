// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NVME4KN-FIXTURE (zenbook-ledger Z40): the 4Kn GPT KAT. The golden card plan (`kat::golden_card_plan`, the
// table `tools/una-card` lays) at 4096-byte LBAs, every offset and field checked BY HAND, then read back
// through THE reader (`gpt::read_table_with`) the way the kernel's `nvme_lba_spans` reads a 4Kn namespace.
// The disk is 49 664 LBAs (203 MiB); only the head (LBAs 0..6) and the tail (the last 5) carry the table,
// so the KAT lays those two windows instead of the whole disk.

use amber_core::gpt::{self, GptError, Header};
use amber_core::kat::golden_card_plan;

const LBA: usize = 4096;
const TOTAL: u64 = 397_312 / 8; // 49 664
const HEAD_LBAS: u64 = 6; // MBR, header, 4-LBA array
const TAIL_LBAS: u64 = 5; // 4-LBA backup array, backup header

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Lay the table's five writes into the head and tail windows, at `lba * 4096`.
fn lay() -> (Vec<u8>, Vec<u8>) {
    let img = golden_card_plan().gpt_lba(LBA).expect("4Kn table builds");
    assert_eq!(img.total_sectors, TOTAL);
    assert_eq!(img.lba_bytes, LBA);
    let mut head = vec![0u8; HEAD_LBAS as usize * LBA];
    let mut tail = vec![0u8; TAIL_LBAS as usize * LBA];
    let tail0 = TOTAL - TAIL_LBAS;
    for (lba, bytes) in img.writes() {
        if lba < HEAD_LBAS {
            let o = lba as usize * LBA;
            head[o..o + bytes.len()].copy_from_slice(bytes);
        } else {
            assert!(lba >= tail0, "write at lba {lba} outside the two windows");
            let o = (lba - tail0) as usize * LBA;
            tail[o..o + bytes.len()].copy_from_slice(bytes);
        }
    }
    (head, tail)
}

#[test]
fn gpt_4kn_offsets_by_hand() {
    let (head, tail) = lay();

    // LBA 0: the protective MBR over the whole disk in 4 KiB LBAs.
    assert_eq!(&head[510..512], &[0x55, 0xAA]);
    assert_eq!(head[446 + 4], 0xEE);
    assert_eq!(u32le(&head, 446 + 8), 1);
    assert_eq!(u32le(&head, 446 + 12), (TOTAL - 1) as u32); // 49 663
    // No 512 B GPT: byte 512..4096 is zero (a 512-byte reader finds no header at its LBA 1).
    assert!(head[512..4096].iter().all(|&b| b == 0));

    // LBA 1 = byte 4096: the primary header.
    let h = &head[4096..4096 + 512];
    assert_eq!(&h[0..8], b"EFI PART");
    assert_eq!(u32le(h, 12), 92);
    assert_eq!(u64le(h, 24), 1); // current
    assert_eq!(u64le(h, 32), TOTAL - 1); // backup = 49 663
    assert_eq!(u64le(h, 40), 6); // first usable = 2 + 4
    assert_eq!(u64le(h, 48), TOTAL - 6); // last usable = 49 658
    assert_eq!(u64le(h, 72), 2); // entries at LBA 2 = byte 8192
    assert_eq!(u32le(h, 80), 128);
    assert_eq!(u32le(h, 84), 128);
    assert!(head[4096 + 512..8192].iter().all(|&b| b == 0), "the header's LBA tail is zero");

    // LBA 2 = byte 8192: the array, 16 KiB = 4 LBAs; its CRC is the header's.
    let array = &head[8192..8192 + 16384];
    assert_eq!(amber_core::crc32(array), u32le(h, 88));
    let e0 = &array[0..128];
    assert_eq!(&e0[0..16], &gpt::ESP_TYPE);
    assert_eq!(u64le(e0, 32), 256); // 2048 / 8: 1 MiB
    assert_eq!(u64le(e0, 40), 256 + 32_768 - 1); // 128 MiB of 4 KiB = 33 023
    let e1 = &array[128..256];
    assert_eq!(&e1[0..16], &gpt::UNAFS_TYPE);
    assert_eq!(u64le(e1, 32), 33_024); // 264 192 / 8
    assert_eq!(u64le(e1, 40), 33_024 + 16_384 - 1); // 64 MiB of 4 KiB = 49 407
    assert!(array[256..].iter().all(|&b| b == 0));

    // The tail: the backup array at TOTAL-5 (49 659), the backup header at TOTAL-1 (49 663).
    assert_eq!(&tail[0..16384], array, "backup array == primary array");
    let b = &tail[4 * LBA..4 * LBA + 512];
    assert_eq!(&b[0..8], b"EFI PART");
    assert_eq!(u64le(b, 24), TOTAL - 1);
    assert_eq!(u64le(b, 32), 1);
    assert_eq!(u64le(b, 72), TOTAL - 5);
    assert_eq!((u64le(b, 40), u64le(b, 48)), (6, TOTAL - 6));
    assert_eq!(u32le(b, 88), u32le(h, 88), "both headers carry the array CRC");
    // The backup's header CRC by hand (92 bytes, CRC field zeroed). `Header::decode` is NOT asked here: it bounds
    // the array as `entries_lba + array_sectors()` in 512 B units (32), so a 4Kn BACKUP header (entries at 49 659)
    // reads as BadGeometry — the primary (entries at 2) passes, and it is the only one `read_table_with` reads
    // (NVME4KN-DECODE owed, zenbook-ledger Z40).
    let mut z = b[..92].to_vec();
    z[16..20].fill(0);
    assert_eq!(amber_core::crc32(&z), u32le(b, 16));
    assert!(Header::decode(h, TOTAL).is_ok(), "the primary header validates");
    assert_eq!(Header::decode(b, TOTAL).err(), Some(GptError::BadGeometry), "NVME4KN-DECODE: pinned until owed is built");
}

#[test]
fn gpt_4kn_reads_back_through_the_one_reader() {
    let (head, _tail) = lay();
    // The kernel's shape (fs/unafs.rs `nvme_lba_spans`): read(lba, buf) fills buf from byte lba * 4096.
    let t = gpt::read_table_with(TOTAL, |lba, buf: &mut [u8]| {
        let o = lba as usize * LBA;
        match head.get(o..o + buf.len()) {
            Some(src) => {
                buf.copy_from_slice(src);
                true
            }
            None => false,
        }
    })
    .expect("the 4Kn table reads back");
    assert_eq!(t.entries.len(), 2);
    assert_eq!((t.entries[0].1.first_lba, t.entries[0].1.last_lba), (256, 33_023));
    assert_eq!((t.entries[1].1.first_lba, t.entries[1].1.last_lba), (33_024, 49_407));
    assert_eq!(t.entries[1].1.name_string(), "UNAOS-UNAFS");
    // The 512-byte image parser finds no header at byte 512: a 4Kn table is not a 512 table.
    assert_eq!(gpt::parse_image(&head).err(), Some(GptError::BadSignature));
}

#[test]
fn gpt_lba_default_and_refusals() {
    let plan = golden_card_plan();
    let a = plan.gpt().unwrap();
    let b = plan.gpt_lba(512).unwrap();
    assert_eq!(a.lba_bytes, 512);
    assert_eq!((a.total_sectors, a.backup_array_lba(), a.backup_header_lba()), (397_312, 397_312 - 33, 397_311));
    assert_eq!(a.writes().map(|(l, d)| (l, d.to_vec())), b.writes().map(|(l, d)| (l, d.to_vec())), "512 is the default");
    for bad in [0usize, 256, 4095, 8192] {
        assert_eq!(plan.gpt_lba(bad).err(), Some(GptError::BadGeometry), "lba {bad}");
    }
    assert!(gpt::lba_ratio(1024).is_ok() && gpt::lba_ratio(2048).is_ok());
}
