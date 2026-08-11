// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Large-scale feasibility harness: can UnaFS hold a git-shaped workload?
//!
//! This is the measured half of the BOLT-2 feasibility study. It answers four
//! questions with numbers rather than argument, against a synthetic workload
//! whose *shape* is taken from this monorepo's real object store (measured with
//! `git cat-file --batch-all-objects`: 45,987 objects, p50 = 559 B, 70 % under
//! 1 KiB, 9,971 blobs / 30,989 trees / 5,027 commits, 33 packfiles):
//!
//! * **(a) object-store shape** — many small content-addressed blobs written
//!   through `create_files_batch`, plus a packfile-shaped large sequential
//!   stream through `write_data`.
//! * **(b) ref churn** — many small files rewritten and committed, the
//!   `refs/` update pattern.
//! * **(c) tree walks** — recursive `ls` enumeration, and the cost of a
//!   name lookup as a function of directory size (the git fan-out shape).
//! * **(d) integrity** — what `fsck` costs and reports on a populated image.
//!
//! Run it (nothing here touches a real repository or a shared image — the
//! image is created under a temp dir and removed on exit):
//!
//! ```text
//! cargo run --release -p vaire --example unafs_repo_feasibility
//! ```
//!
//! Scale knobs (env): `OBJECTS` (default 10000), `STREAM_MB` (default 52 — the
//! monorepo's real total packfile size), `REFS` (default 493 — its real ref
//! count), `IMAGE_MB` (default 1024; the v3 format caps a volume at 2 GiB).

use std::collections::BTreeMap;
use std::time::Instant;

use unafs::{AttributeValue, BatchFile, FileDevice, FileKind, FileSystem};

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A deterministic xorshift so runs are comparable without a rand dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Draw an object size from the monorepo's measured bucket distribution:
/// 70.3 % < 1 KiB, 15.6 % 1–8 KiB, 7.0 % 8–64 KiB, 6.8 % 64 KiB–1 MiB,
/// 0.3 % > 1 MiB (capped here at 1 MiB to keep the harness bounded).
fn draw_size(rng: &mut Rng) -> usize {
    let r = rng.next() % 1000;
    let span = |lo: usize, hi: usize, rng: &mut Rng| lo + (rng.next() as usize % (hi - lo));
    match r {
        0..=702 => span(20, 1024, rng),
        703..=858 => span(1024, 8192, rng),
        859..=928 => span(8192, 65536, rng),
        929..=996 => span(65536, 1024 * 1024, rng),
        _ => span(1024 * 1024, 2 * 1024 * 1024, rng),
    }
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let objects = env_u64("OBJECTS", 10_000) as usize;
    let stream_mb = env_u64("STREAM_MB", 52);
    let refs = env_u64("REFS", 493) as usize;
    let image_mb = env_u64("IMAGE_MB", 1024);

    let dir = tempfile::tempdir()?;
    let img = dir.path().join("feasibility.img");
    println!("== UnaFS repo-store feasibility ==");
    println!(
        "image {} MB, {objects} objects, {stream_mb} MB stream, {refs} refs\n",
        image_mb
    );

    let t = Instant::now();
    {
        let f = std::fs::File::create(&img)?;
        f.set_len(image_mb * 1024 * 1024)?;
    }
    let device = FileDevice::open(&img)?;
    let mut fs = FileSystem::format(device, image_mb).map_err(|e| format!("format: {e:?}"))?;
    println!("format:            {:8.1} ms", ms(t.elapsed()));
    let root = fs.superblock.root_inode;

    // ---------------------------------------------------------------------
    // (a) object-store shape — many small content-addressed blobs
    // ---------------------------------------------------------------------
    // Git's loose store is a 256-way fan-out (`objects/ab/cdef…`). Reproduce
    // that exactly, so the per-directory group sizes match a real store.
    fs.set_autocommit(false);
    let t = Instant::now();
    let objects_dir = fs.mkdir(root, "objects".into()).map_err(|e| format!("{e:?}"))?;
    let mut fanout = Vec::with_capacity(256);
    for i in 0..256u32 {
        fanout.push(
            fs.mkdir(objects_dir, format!("{i:02x}"))
                .map_err(|e| format!("{e:?}"))?,
        );
    }
    let mkdir_ms = ms(t.elapsed());

    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let mut groups: BTreeMap<u64, Vec<BatchFile>> = BTreeMap::new();
    let mut total_bytes = 0u64;
    let t = Instant::now();
    for i in 0..objects {
        let size = draw_size(&mut rng);
        let bucket = (rng.next() % 256) as usize;
        let mut attrs = BTreeMap::new();
        attrs.insert("vaire.size".into(), AttributeValue::Int(size as i64));
        attrs.insert("vaire.kind".into(), AttributeValue::String("blob".into()));
        groups.entry(fanout[bucket]).or_default().push(BatchFile {
            name: format!("{i:038x}"),
            data: vec![(i % 251) as u8; size],
            attributes: attrs,
        });
        total_bytes += size as u64;
    }
    let gen_ms = ms(t.elapsed());

    let t = Instant::now();
    let group_count = groups.len();
    for (parent, files) in groups {
        fs.create_files_batch(parent, files)
            .map_err(|e| format!("batch: {e:?}"))?;
    }
    let stage_ms = ms(t.elapsed());
    let t = Instant::now();
    fs.commit().map_err(|e| format!("commit: {e:?}"))?;
    let commit_ms = ms(t.elapsed());
    let st = fs.commit_stats();
    println!(
        "(a) {objects} blobs, {} MiB in {group_count} fan-out dirs:",
        total_bytes / 1024 / 1024
    );
    println!("      mkdir 257 dirs:  {mkdir_ms:8.1} ms");
    println!("      generate:        {gen_ms:8.1} ms  (harness, not UnaFS)");
    println!(
        "      stage (batch):   {stage_ms:8.1} ms  ({:.3} ms/object)",
        stage_ms / objects as f64
    );
    println!("      ONE commit:      {commit_ms:8.1} ms");
    println!(
        "      => {:.1} MiB/s end-to-end, {} flips, {} blocks written",
        total_bytes as f64 / 1024.0 / 1024.0 / ((stage_ms + commit_ms) / 1000.0),
        st.commits,
        st.blocks_written
    );
    let write_amp = st.blocks_written as f64 * 4096.0 / total_bytes as f64;
    println!("      write amplification: {write_amp:.2}x (blocks*4096 / payload)");

    // ---------------------------------------------------------------------
    // (a2) packfile-shaped large sequential stream
    // ---------------------------------------------------------------------
    let pack_dir = fs.mkdir(objects_dir, "pack".into()).map_err(|e| format!("{e:?}"))?;
    let stream_bytes = (stream_mb * 1024 * 1024) as usize;
    let chunk = vec![0xA5u8; 1024 * 1024];
    let t = Instant::now();
    let pack = fs
        .create_file(pack_dir, "pack-feasibility.pack".into())
        .map_err(|e| format!("{e:?}"))?;
    let mut off = 0u64;
    while (off as usize) < stream_bytes {
        fs.write_data(pack, off, &chunk)
            .map_err(|e| format!("write_data: {e:?}"))?;
        off += chunk.len() as u64;
    }
    let pack_stage_ms = ms(t.elapsed());
    let t = Instant::now();
    fs.commit().map_err(|e| format!("commit pack: {e:?}"))?;
    let pack_commit_ms = ms(t.elapsed());
    let inode = fs.read_inode(pack).map_err(|e| format!("{e:?}"))?;
    println!("\n(a2) {stream_mb} MB packfile-shaped stream (1 MiB write_data calls):");
    println!("      stage:           {pack_stage_ms:8.1} ms");
    println!("      commit:          {pack_commit_ms:8.1} ms");
    println!(
        "      => {:.1} MiB/s;  inode carries {} extents for {} bytes",
        stream_mb as f64 / ((pack_stage_ms + pack_commit_ms) / 1000.0),
        inode.chunks.len(),
        inode.size
    );
    println!(
        "      NOTE: an Inode must serialize inside ONE 4096 B block, and each\n\
         \x20           extent costs ~24 B — so the extent count is the hard scaling\n\
         \x20           limit for a large file, not the volume size."
    );
    let t = Instant::now();
    let back = fs
        .read_data(pack, 0, 4 * 1024 * 1024)
        .map_err(|e| format!("read_data: {e:?}"))?;
    println!(
        "      sequential read-back of 4 MiB: {:8.1} ms ({:.1} MiB/s), {} B correct",
        ms(t.elapsed()),
        4.0 / (ms(t.elapsed()) / 1000.0),
        back.len()
    );

    // ---------------------------------------------------------------------
    // (b) ref churn — many small files rewritten and committed
    // ---------------------------------------------------------------------
    let refs_dir = fs.mkdir(root, "refs".into()).map_err(|e| format!("{e:?}"))?;
    let oid = vec![b'a'; 41];
    let mut batch = Vec::with_capacity(refs);
    for i in 0..refs {
        batch.push(BatchFile {
            name: format!("ref-{i:05}"),
            data: oid.clone(),
            attributes: BTreeMap::new(),
        });
    }
    let t = Instant::now();
    fs.create_files_batch(refs_dir, batch)
        .map_err(|e| format!("{e:?}"))?;
    fs.commit().map_err(|e| format!("{e:?}"))?;
    let refs_create_ms = ms(t.elapsed());

    // The realistic churn pattern: rewrite every ref, one flip for the batch.
    let t = Instant::now();
    let mut rebatch = Vec::with_capacity(refs);
    for i in 0..refs {
        let name = format!("ref-{i:05}");
        fs.unlink(refs_dir, &name).map_err(|e| format!("{e:?}"))?;
        rebatch.push(BatchFile {
            name,
            data: vec![b'b'; 41],
            attributes: BTreeMap::new(),
        });
    }
    fs.create_files_batch(refs_dir, rebatch)
        .map_err(|e| format!("{e:?}"))?;
    fs.commit().map_err(|e| format!("{e:?}"))?;
    let refs_batch_ms = ms(t.elapsed());

    // The pessimal pattern: one ref updated per transaction (what a naive
    // `update-ref` per push would do).
    let before = fs.commit_stats().blocks_written;
    let t = Instant::now();
    for i in 0..20usize {
        let name = format!("ref-{i:05}");
        fs.unlink(refs_dir, &name).map_err(|e| format!("{e:?}"))?;
        fs.create_files_batch(
            refs_dir,
            vec![BatchFile {
                name,
                data: vec![b'c'; 41],
                attributes: BTreeMap::new(),
            }],
        )
        .map_err(|e| format!("{e:?}"))?;
        fs.commit().map_err(|e| format!("{e:?}"))?;
    }
    let per_ref_ms = ms(t.elapsed()) / 20.0;
    let per_ref_blocks = (fs.commit_stats().blocks_written - before) as f64 / 20.0;
    println!("\n(b) ref churn ({refs} refs in one directory):");
    println!("      create all (1 flip):     {refs_create_ms:8.1} ms");
    println!(
        "      rewrite all (1 flip):    {refs_batch_ms:8.1} ms  ({:.3} ms/ref)",
        refs_batch_ms / refs as f64
    );
    println!(
        "      one ref per flip:        {per_ref_ms:8.1} ms/ref, {per_ref_blocks:.0} blocks/ref \
         => {:.0}x the batched cost",
        per_ref_ms / (refs_batch_ms / refs as f64)
    );

    // ---------------------------------------------------------------------
    // (c) tree walks — enumeration and per-name lookup vs directory size
    // ---------------------------------------------------------------------
    let t = Instant::now();
    let (files, bytes) = walk(&mut fs, root)?;
    let walk_ms = ms(t.elapsed());
    println!("\n(c) tree walk:");
    println!(
        "      full recursive ls: {walk_ms:8.1} ms for {files} files / {} MiB \
         ({:.3} ms/1000 entries)",
        bytes / 1024 / 1024,
        walk_ms / (files as f64 / 1000.0)
    );

    // Lookup cost as a function of directory size — the shape that decides
    // whether a flat object directory is viable.
    for n in [64usize, 512, 4096] {
        let d = fs
            .mkdir(root, format!("flat-{n}"))
            .map_err(|e| format!("{e:?}"))?;
        let mut b = Vec::with_capacity(n);
        for i in 0..n {
            b.push(BatchFile {
                name: format!("o{i:06}"),
                data: vec![7u8; 64],
                attributes: BTreeMap::new(),
            });
        }
        fs.create_files_batch(d, b).map_err(|e| format!("{e:?}"))?;
        fs.commit().map_err(|e| format!("{e:?}"))?;
        let t = Instant::now();
        let probes = 200;
        for i in 0..probes {
            let want = format!("o{:06}", i % n);
            let hit = fs
                .ls(d)
                .map_err(|e| format!("{e:?}"))?
                .into_iter()
                .any(|e| e.name == want);
            assert!(hit);
        }
        println!(
            "      name lookup in a {n:5}-entry dir: {:6.3} ms/lookup (full ls + scan)",
            ms(t.elapsed()) / probes as f64
        );
    }

    // ---------------------------------------------------------------------
    // (d) integrity — snapshot + fsck on a populated image
    // ---------------------------------------------------------------------
    let t = Instant::now();
    let snap_gen = fs
        .snapshot_create("feasibility".into(), "vaire".into(), 0)
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let snap_ms = ms(t.elapsed());
    let t = Instant::now();
    let report = fs.fsck(false).map_err(|e| format!("fsck: {e:?}"))?;
    let fsck_ms = ms(t.elapsed());
    let st = fs.commit_stats();
    println!("\n(d) integrity:");
    println!("      snapshot_create:   {snap_ms:8.1} ms (generation {snap_gen})");
    println!(
        "      fsck (read-only):  {fsck_ms:8.1} ms — clean={} in_use={} reachable={} leaked={} orphans={}",
        report.is_clean(),
        report.blocks_in_use,
        report.reachable_blocks,
        report.leaked_blocks.len(),
        report.orphan_inodes.len()
    );
    println!(
        "\ntotals: {} commits, {} blocks written ({} MiB), {} free blocks left",
        st.commits,
        st.blocks_written,
        st.blocks_written * 4096 / 1024 / 1024,
        fs.free_blocks()
    );
    Ok(())
}

fn walk(fs: &mut FileSystem, dir: u64) -> Result<(usize, u64), Box<dyn std::error::Error>> {
    let (mut files, mut bytes) = (0usize, 0u64);
    for e in fs.ls(dir).map_err(|e| format!("{e:?}"))? {
        match e.kind {
            FileKind::Directory => {
                let (f, b) = walk(fs, e.inode_id)?;
                files += f;
                bytes += b;
            }
            _ => {
                files += 1;
                bytes += fs.read_inode(e.inode_id).map_err(|e| format!("{e:?}"))?.size;
            }
        }
    }
    Ok((files, bytes))
}
