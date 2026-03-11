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
use unafs::{AttributeValue, BLOCK_SIZE, FileDevice, FileSystem};

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

    // Action 2: The Persistence Drop
    println!("-> Synchronizing metadata and simulating cold boot...");
    fs.sync_metadata()?;

    let expected_inode_count = fs.superblock.root_inode; // roughly

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
        num_inodes,
        "Cold-Boot failed! Expected {} inodes, found {}",
        num_inodes,
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

    // Using a lower threshold for benchmarking so it actually returns some results out of random vectors
    let query_str = format!(
        "similarity(embedding, {}) > -1.0 AND type == \"engram\"",
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
            valid_count += 1;
        } else {
            panic!(
                "Query corruption! Inode {} missing 'type' attribute",
                inode.id
            );
        }
    }

    assert!(
        valid_count > 0,
        "Expected at least 1 result from -1.0 similarity threshold."
    );

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
