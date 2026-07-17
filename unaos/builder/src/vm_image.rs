// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// VMIMAGE-1 — package the SAME build products the esp-x86 path proves into ONE
// self-contained, distributable disk image: a GPT-partitioned disk carrying a single
// FAT32 EFI System Partition with the boot tree (EFI/BOOT/BOOTX64.EFI + kernel.elf +
// the small companion files) and a README-VM.txt. Boots in stock QEMU+OVMF, UTM, and
// VirtualBox/VMware with EFI enabled — zero UnaOS tooling on the consumer's machine.
//
// Design notes:
//   * The FAT32 filesystem is built with the pure-Rust `fatfs` crate (no host mkfs.vfat /
//     hdiutil), so the image builds identically on any host with a Rust toolchain.
//   * The GPT + protective MBR are written by hand here (not via a crate) for two reasons:
//     total control of the layout, and DETERMINISTIC GUIDs derived from the git hash — a
//     given source tree always packages a byte-identical partition table, which makes the
//     image reproducible and easy to diff. See `derive_guid`.
//   * The image is DISTRIBUTION surface: only the boot tree + README-VM.txt land in it.
//     Nothing about the bench, no host paths, no build internals.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

// Layout constants (512-byte logical sectors throughout).
const SECTOR: u64 = 512;
const ESP_LBA_START: u64 = 2048; // 1 MiB alignment for the ESP.
const GPT_ENTRIES: u32 = 128;
const GPT_ENTRY_SIZE: u32 = 128;
const GPT_ARRAY_SECTORS: u64 = (GPT_ENTRIES as u64 * GPT_ENTRY_SIZE as u64) / SECTOR; // 32
// ESP size: comfortably fits the ~0.7 MiB boot tree while staying well above the FAT32
// floor (a smaller volume would format as FAT16). 64 MiB.
const ESP_BYTES: u64 = 64 * 1024 * 1024;

// EFI System Partition type GUID: C12A7328-F81F-11D2-BA4B-00A0C93EC93B, stored in the
// GPT mixed-endian layout (data1/2/3 little-endian, data4 big-endian).
const EFI_SYSTEM_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

/// Package the boot tree at `esp_dir` into `target/vm/unaos-x86-<git7>.img` and return the
/// image path. `git7` is the short git hash (identity + deterministic-GUID seed).
pub fn build(target_dir: &Path, esp_dir: &Path, git7: &str) -> std::path::PathBuf {
    let esp_sectors = ESP_BYTES / SECTOR;
    let esp_lba_end = ESP_LBA_START + esp_sectors - 1; // inclusive last LBA of the ESP
    // Disk = primary GPT (LBA0 MBR, LBA1 header, LBA2..33 array) + ESP + backup array + backup header.
    let total_sectors = esp_lba_end + 1 + GPT_ARRAY_SECTORS + 1;
    let total_bytes = total_sectors * SECTOR;

    // 1) Build the FAT32 partition image in memory, then splice it into the disk.
    let mut fat = vec![0u8; ESP_BYTES as usize];
    format_and_fill_fat(&mut fat, esp_dir, git7);

    // 2) Assemble the whole disk image.
    let out_dir = target_dir.join("vm");
    fs::create_dir_all(&out_dir).unwrap();
    let out_path = out_dir.join(format!("unaos-x86-{git7}.img"));
    let mut img = fs::File::create(&out_path).unwrap();
    img.set_len(total_bytes).unwrap();

    let disk_guid = derive_guid(b"UNAOS-VM", git7);
    let part_guid = derive_guid(b"UNAOS-ESP", git7);

    // Protective MBR (LBA0).
    let mut mbr = [0u8; SECTOR as usize];
    write_protective_mbr(&mut mbr, total_sectors);
    write_at(&mut img, 0, &mbr);

    // Partition entry array (used by both primary and backup headers, identical bytes).
    let mut entries = vec![0u8; (GPT_ARRAY_SECTORS * SECTOR) as usize];
    write_esp_entry(&mut entries[..GPT_ENTRY_SIZE as usize], ESP_LBA_START, esp_lba_end, &part_guid);
    let entries_crc = crc32fast::hash(&entries);

    let first_usable = 2 + GPT_ARRAY_SECTORS; // LBA 34
    let last_usable = total_sectors - GPT_ARRAY_SECTORS - 2; // last LBA before the backup array
    let backup_header_lba = total_sectors - 1;
    let backup_array_lba = total_sectors - 1 - GPT_ARRAY_SECTORS;

    // Primary GPT header (LBA1) + array (LBA2).
    let primary_header = build_gpt_header(
        1, backup_header_lba, first_usable, last_usable, &disk_guid, 2, entries_crc,
    );
    write_at(&mut img, 1 * SECTOR, &primary_header);
    write_at(&mut img, 2 * SECTOR, &entries);

    // The FAT32 ESP.
    write_at(&mut img, ESP_LBA_START * SECTOR, &fat);

    // Backup array + backup header (LBA last).
    write_at(&mut img, backup_array_lba * SECTOR, &entries);
    let backup_header = build_gpt_header(
        backup_header_lba, 1, first_usable, last_usable, &disk_guid, backup_array_lba, entries_crc,
    );
    write_at(&mut img, backup_header_lba * SECTOR, &backup_header);

    img.flush().unwrap();
    out_path
}

// --- FAT32 filesystem ---

fn format_and_fill_fat(buf: &mut [u8], esp_dir: &Path, git7: &str) {
    {
        let mut cursor = std::io::Cursor::new(&mut buf[..]);
        fatfs::format_volume(
            &mut cursor,
            fatfs::FormatVolumeOptions::new()
                .fat_type(fatfs::FatType::Fat32)
                .volume_label(*b"UNAOS      "),
        )
        .expect("FAT32 format failed");
    }
    let cursor = std::io::Cursor::new(&mut buf[..]);
    let fs = fatfs::FileSystem::new(cursor, fatfs::FsOptions::new()).expect("FAT mount failed");
    {
        let root = fs.root_dir();
        copy_tree(esp_dir, &root);
        // README-VM.txt lives at the FAT root — the consumer's companion doc.
        let mut readme = root.create_file("README-VM.txt").unwrap();
        readme.truncate().unwrap();
        readme.write_all(readme_text(git7).as_bytes()).unwrap();
    }
    fs.unmount().expect("FAT unmount failed");
}

fn copy_tree<'a, T: fatfs::ReadWriteSeek>(src: &Path, dst: &fatfs::Dir<'a, T>) {
    let mut entries: Vec<_> = fs::read_dir(src).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name()); // stable order => reproducible image
    for entry in entries {
        let name = entry.file_name().into_string().unwrap();
        // Skip macOS metadata so the distributed tree is clean.
        if name.starts_with("._") || name == ".DS_Store" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            let sub = dst.create_dir(&name).unwrap();
            copy_tree(&entry.path(), &sub);
        } else {
            let data = fs::read(entry.path()).unwrap();
            let mut f = dst.create_file(&name).unwrap();
            f.truncate().unwrap();
            f.write_all(&data).unwrap();
        }
    }
}

// --- GPT / MBR ---

fn write_protective_mbr(mbr: &mut [u8], total_sectors: u64) {
    // Single 0xEE partition spanning the disk (minus the MBR itself), per UEFI spec.
    let e = 446;
    mbr[e] = 0x00; // boot indicator
    mbr[e + 1] = 0x00; // CHS first (start head)
    mbr[e + 2] = 0x02;
    mbr[e + 3] = 0x00;
    mbr[e + 4] = 0xEE; // type: GPT protective
    mbr[e + 5] = 0xFF; // CHS last
    mbr[e + 6] = 0xFF;
    mbr[e + 7] = 0xFF;
    mbr[e + 8..e + 12].copy_from_slice(&1u32.to_le_bytes()); // first LBA
    let count = core::cmp::min(total_sectors - 1, 0xFFFF_FFFF) as u32;
    mbr[e + 12..e + 16].copy_from_slice(&count.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
}

fn write_esp_entry(entry: &mut [u8], first_lba: u64, last_lba: u64, part_guid: &[u8; 16]) {
    entry[0..16].copy_from_slice(&EFI_SYSTEM_TYPE_GUID);
    entry[16..32].copy_from_slice(part_guid);
    entry[32..40].copy_from_slice(&first_lba.to_le_bytes());
    entry[40..48].copy_from_slice(&last_lba.to_le_bytes());
    // attributes [48..56] = 0
    // Partition name "UNAOS" as UTF-16LE at [56..128].
    for (i, ch) in "UNAOS".encode_utf16().enumerate() {
        entry[56 + i * 2..58 + i * 2].copy_from_slice(&ch.to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
fn build_gpt_header(
    current_lba: u64,
    backup_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: &[u8; 16],
    entries_start_lba: u64,
    entries_crc: u32,
) -> [u8; SECTOR as usize] {
    let mut h = [0u8; SECTOR as usize];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // revision 1.0
    h[12..16].copy_from_slice(&92u32.to_le_bytes()); // header size
    // [16..20] header CRC — filled after zeroing.
    // [20..24] reserved = 0
    h[24..32].copy_from_slice(&current_lba.to_le_bytes());
    h[32..40].copy_from_slice(&backup_lba.to_le_bytes());
    h[40..48].copy_from_slice(&first_usable.to_le_bytes());
    h[48..56].copy_from_slice(&last_usable.to_le_bytes());
    h[56..72].copy_from_slice(disk_guid);
    h[72..80].copy_from_slice(&entries_start_lba.to_le_bytes());
    h[80..84].copy_from_slice(&GPT_ENTRIES.to_le_bytes());
    h[84..88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    // Header CRC-32 over the first 92 bytes with the CRC field zeroed.
    let crc = crc32fast::hash(&h[0..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

/// Deterministic 16-byte GUID from a label + the git hash. Not a random v4 GUID — the point
/// is reproducibility: the same source tree always yields the same disk/partition GUIDs.
/// The RFC-4122 variant/version nibbles are stamped so the value is a well-formed GUID.
fn derive_guid(label: &[u8], git7: &str) -> [u8; 16] {
    let mut g = [0u8; 16];
    let src: Vec<u8> = label.iter().chain(git7.as_bytes()).copied().collect();
    for (i, b) in g.iter_mut().enumerate() {
        *b = src.get(i).copied().unwrap_or((0x11 * (i as u8 + 1)) ^ 0x5A);
    }
    g[7] = (g[7] & 0x0F) | 0x40; // version 4
    g[8] = (g[8] & 0x3F) | 0x80; // variant RFC 4122
    g
}

// --- helpers ---

fn write_at(f: &mut fs::File, off: u64, data: &[u8]) {
    f.seek(SeekFrom::Start(off)).unwrap();
    f.write_all(data).unwrap();
}

fn readme_text(git7: &str) -> String {
    format!(
        "UnaOS — try it in a virtual machine\n\
=====================================\n\
Build: {git7}   Image: unaos-x86-{git7}.img\n\
\n\
This is a self-contained bootable disk image: a GPT disk with a FAT32 EFI System\n\
Partition holding the UnaOS boot tree (EFI/BOOT/BOOTX64.EFI + kernel.elf, plus small\n\
demo files hello.txt/HELLO.BIN). It needs a\n\
UEFI/EFI virtual machine — enable EFI, attach this .img as the disk, and boot. No UnaOS\n\
tooling is required on your machine.\n\
\n\
QEMU (with OVMF UEFI firmware) — one command:\n\
  qemu-system-x86_64 -machine q35 -m 1G \\\n\
    -drive if=pflash,format=raw,readonly=on,file=/path/to/OVMF_CODE.fd \\\n\
    -drive format=raw,file=unaos-x86-{git7}.img \\\n\
    -serial stdio\n\
  (OVMF ships with QEMU. On Homebrew it is usually\n\
   /usr/local/share/qemu/edk2-x86_64-code.fd or the /opt/homebrew equivalent; on Linux\n\
   /usr/share/OVMF/OVMF_CODE.fd. The boot log appears on the serial console; the shell\n\
   appears in the QEMU graphical window.)\n\
\n\
UTM (macOS):\n\
  1. New VM -> Emulate -> Other. Architecture x86_64, and turn ON \"UEFI Boot\".\n\
  2. Remove the default drive; Import Drive and select unaos-x86-{git7}.img (raw).\n\
  3. Save, then run. UnaOS boots to its shell.\n\
\n\
VirtualBox / VMware:\n\
  1. Convert if the tool wants its own container, e.g.:\n\
       VBoxManage convertfromraw unaos-x86-{git7}.img unaos.vdi --format VDI\n\
  2. New VM, type \"Other/Unknown (64-bit)\". In Settings -> System, ENABLE EFI\n\
     (\"Enable EFI (special OSes only)\" in VirtualBox; \"firmware = efi\" in VMware).\n\
  3. Attach the disk (the .vdi, or the raw .img in VMware) and boot.\n\
\n\
What you should see: the UEFI firmware loads EFI/BOOT/BOOTX64.EFI, which loads the\n\
kernel; UnaOS prints its boot log and drops to an interactive shell.\n\
\n\
Report results back to whoever gave you this image — include your VM (QEMU/UTM/\n\
VirtualBox/VMware), its version, and the last boot-log lines you saw.\n"
    )
}
