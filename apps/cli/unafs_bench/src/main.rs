// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::{Context, Result};
use rand::Rng;
use std::fs;
use std::time::Instant;
use unafs::{AttributeValue, BLOCK_SIZE, FileDevice, FileSystem, cosine_similarity};

/// The `swapped_345` golden from `libs/unafs/tests/query_kats.rs`:
/// cosine([3,4], [4,3]) = 24/25 = 0.96, bit pattern 0x3f75_c28f.
/// A frozen contract — do not edit to make the bench pass.
const GOLDEN_345_BITS: u32 = 0x3f75_c28f;

fn main() -> Result<()> {
    println!("================================================================================");
    println!(":: UNAFS CAN-AM BENCHMARK (STRESS TEST) ::");
    println!("================================================================================");

    let vault_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("unaos");
    fs::create_dir_all(&vault_dir)?;
    let disk_path = vault_dir.join("bench_vault.img");

    // Clean start
    if disk_path.exists() {
        fs::remove_file(&disk_path)?;
    }

    // Pre-allocate the disk file
    let block_count = 100_000;
    let file = fs::File::create(&disk_path)?;
    file.set_len(block_count * BLOCK_SIZE)?;
    drop(file);

    println!("-> Initializing raw UnaFS instance at {:?}", disk_path);
    let device = FileDevice::open(&disk_path).context("Failed to create FileDevice")?;
    let mut fs = FileSystem::format(device, 0).context("Failed to format filesystem")?;

    let root_id = fs.superblock.root_inode;
    let num_inodes = 10_000;

    println!(
        "-> Firing loop to rapidly create {} blank Inodes...",
        num_inodes
    );

    let mut rng = rand::thread_rng();
    let types = ["engram", "directive", "noise"];

    let start_time = Instant::now();
    for i in 0..num_inodes {
        let filename = format!("file_{}.txt", i);
        let inode_id = fs
            .create_file(root_id, filename)
            .context("Failed to create file")?;

        let mut vec_data = Vec::with_capacity(384);
        for _ in 0..384 {
            vec_data.push(rng.gen_range(-1.0..1.0));
        }

        let type_str = types[i % 3].to_string();

        fs.set_attribute(
            inode_id,
            "embedding".to_string(),
            AttributeValue::Vector(vec_data),
        )
        .context("Failed to set embedding")?;
        fs.set_attribute(
            inode_id,
            "type".to_string(),
            AttributeValue::String(type_str),
        )
        .context("Failed to set type")?;

        if i > 0 && i % 1000 == 0 {
            println!("   ... created {} inodes", i);
        }
    }

    let write_latency = start_time.elapsed();
    println!("-> High-RPM Inode Generation complete.");

    // Plant a golden sentinel with a KNOWN embedding among the 10k random
    // inodes: its score against the [4, 3] target is pinned bit-for-bit by
    // the query-KAT contract, so the correctness assertions below can
    // genuinely fail (unlike the old `> -1.0` threshold, which nothing
    // could ever miss).
    let sentinel_id = fs
        .create_file(root_id, "golden_345.vec".to_string())
        .context("Failed to create golden sentinel")?;
    fs.set_attribute(
        sentinel_id,
        "embedding".to_string(),
        AttributeValue::Vector(vec![3.0, 4.0]),
    )
    .context("Failed to set sentinel embedding")?;
    fs.set_attribute(
        sentinel_id,
        "type".to_string(),
        AttributeValue::String("engram".to_string()),
    )
    .context("Failed to set sentinel type")?;

    // Action 2: The Persistence Drop
    println!("-> Synchronizing metadata and simulating cold boot...");
    fs.sync_metadata()?;

    // Safely drop filesystem instance
    drop(fs);

    let boot_time = Instant::now();
    let device = FileDevice::open(&disk_path).context("Failed to open FileDevice on reboot")?;
    let mut fs = FileSystem::mount(device).context("Failed to mount filesystem on reboot")?;
    let recovery_latency = boot_time.elapsed();

    // Verify
    // A quick way to verify inode count is to list the root directory
    let root_entries = fs.ls(root_id)?;
    assert_eq!(
        root_entries.len(),
        num_inodes + 1, // the 10k random inodes + the golden sentinel
        "Cold-Boot failed! Expected {} inodes, found {}",
        num_inodes + 1,
        root_entries.len()
    );
    println!(
        "-> Cold-Boot verification passed! Recovered {} inodes.",
        root_entries.len()
    );

    // Action 3: The Vector Gravity Slalom
    println!("-> Executing heavy compound query...");

    let mut target_vec = Vec::with_capacity(384);
    for _ in 0..384 {
        target_vec.push(rng.gen_range(-1.0..1.0));
    }
    let vec_str = format!("{:?}", target_vec);

    // A threshold real scores can actually miss: random 384-dim vectors
    // score in (-1, 1) against the target, so `> 0.0` genuinely filters
    // (~half of the engram third survives). The old `> -1.0` threshold
    // could not exclude anything — the assertion was unfailable.
    let query_str = format!(
        "similarity(embedding, {}) > 0.0 AND type == \"engram\"",
        vec_str
    );

    let query_start = Instant::now();
    let results = fs.query(&query_str)?;
    let query_latency = query_start.elapsed();

    println!("-> Query executed, analyzing {} results...", results.len());

    let mut valid_count = 0;
    for (inode, score) in results {
        if let Some(AttributeValue::String(t)) = inode.attributes.get("type") {
            assert_eq!(
                t, "engram",
                "Query corruption! Found type {} instead of engram",
                t
            );
        } else {
            panic!(
                "Query corruption! Inode {} missing 'type' attribute",
                inode.id
            );
        }
        // Strict `>` threshold semantics, per the query-KAT contract.
        assert!(
            score > 0.0,
            "Threshold breach! Inode {} returned with score {} <= 0.0",
            inode.id,
            score
        );
        // The engine's score must bit-match a host-side recompute from the
        // stored vector — 384-dim vectors SPILL to extent-backed
        // large_attributes, so fetch via get_attribute (which resolves the
        // spill); proves the vector round-trips through extents and the one
        // libm scoring path answers both.
        if let Some(AttributeValue::Vector(v)) = fs
            .get_attribute(inode.id, "embedding")
            .context("Failed to re-fetch embedding")?
        {
            let recomputed = cosine_similarity(&v, &target_vec);
            assert_eq!(
                score.to_bits(),
                recomputed.to_bits(),
                "Score divergence! Inode {}: engine {:?} vs recompute {:?}",
                inode.id,
                score,
                recomputed
            );
        } else {
            panic!(
                "Query corruption! Inode {} missing 'embedding' attribute",
                inode.id
            );
        }
        valid_count += 1;
    }

    assert!(
        valid_count > 0,
        "Expected at least 1 engram above the 0.0 similarity threshold."
    );

    // Golden-KAT correctness gate: the sentinel's score against [4, 3] is
    // pinned bit-for-bit (0.96 = 0x3f75c28f, the `swapped_345` golden in
    // libs/unafs/tests/query_kats.rs). The 384-dim random inodes mismatch
    // the 2-dim target (score 0.0), so exactly the sentinel survives.
    println!("-> Executing golden-KAT correctness query...");
    let golden = fs.query("similarity(embedding, [4.0, 3.0]) > 0.5 AND type == \"engram\"")?;
    assert_eq!(
        golden.len(),
        1,
        "Golden query: expected exactly the sentinel, got {} results",
        golden.len()
    );
    assert_eq!(
        golden[0].0.id, sentinel_id,
        "Golden query returned the wrong inode"
    );
    assert_eq!(
        golden[0].1.to_bits(),
        GOLDEN_345_BITS,
        "Golden score diverged from the KAT contract: got {:?} (0x{:08x}), golden 0x{:08x}",
        golden[0].1,
        golden[0].1.to_bits(),
        GOLDEN_345_BITS
    );

    // Strict `>`: a threshold exactly equal to the score must exclude it.
    let strict = fs.query("similarity(embedding, [4.0, 3.0]) > 0.96")?;
    assert!(
        strict.iter().all(|(inode, _)| inode.id != sentinel_id),
        "Strict-threshold breach! Score 0.96 cleared threshold 0.96"
    );
    println!("-> Golden-KAT gate passed (score 0x{:08x}, strict-> exclusion held).", GOLDEN_345_BITS);

    // Action 4: Telemetry Output
    println!("\n================================================================================");
    println!(":: TELEMETRY REPORT ::");
    println!("================================================================================");
    println!("Write Latency (10k Inodes): {:?}", write_latency);
    println!("Cold-Boot Recovery Time:    {:?}", recovery_latency);
    println!("Compound Query Speed:       {:?}", query_latency);
    println!("Valid Inodes Matched:       {}", valid_count);
    println!("================================================================================");

    // Clean up
    fs::remove_file(&disk_path)?;

    Ok(())
}
