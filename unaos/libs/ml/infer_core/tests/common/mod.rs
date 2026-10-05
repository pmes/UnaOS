// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Test-time vectors: the archives in `vectors.txt`, fetched with `curl`, verified by sha256
//! (CRYPTOCORE) BEFORE unpacking with `tar`, cached under `target/infer_core-vectors/`. Offline →
//! `None` and the caller prints a SKIP line.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&crypto_core::sha2::sha256(data))
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn vectors_dir() -> PathBuf {
    std::env::var_os("INFER_CORE_VECTORS").map(PathBuf::from).unwrap_or_else(|| manifest().join("../../../../target/infer_core-vectors"))
}

/// The `vectors.txt` entry `name` → (url, sha256).
fn entry(name: &str) -> (String, String) {
    let txt = std::fs::read_to_string(manifest().join("vectors.txt")).unwrap();
    for line in txt.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split(" | ").collect();
        if f[0].trim() == name {
            return (f[1].trim().to_string(), f[2].trim().to_string());
        }
    }
    panic!("vectors.txt has no {name}");
}

/// Fetch + verify + unpack archive `name`; answers the unpack directory.
pub fn archive(name: &str) -> Option<PathBuf> {
    let (url, sha) = entry(name);
    let dir = vectors_dir().join(name);
    let done = dir.join(".ok");
    if done.is_file() {
        return Some(dir);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let tgz = dir.join("archive.tgz");
    let ok = Command::new("curl").args(["-sSfL", "--max-time", "300", "-o"]).arg(&tgz).arg(&url).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIP: cannot fetch {url} (offline?)");
        let _ = std::fs::remove_file(&tgz);
        return None;
    }
    let got = sha256_hex(&std::fs::read(&tgz).ok()?);
    assert_eq!(got, sha, "sha256 of {url}");
    let ok = Command::new("tar").arg("xzf").arg(&tgz).arg("-C").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "tar xzf {}", tgz.display());
    let _ = std::fs::remove_file(&tgz);
    std::fs::write(&done, b"").ok()?;
    Some(dir)
}

/// The all-MiniLM-L6-v2 ONNX directory (model.onnx, tokenizer.json, vocab.txt, config.json).
pub fn minilm() -> Option<PathBuf> {
    archive("minilm-onnx").map(|d| d.join("onnx"))
}

/// The F16 safetensors directory (model.fp16.safetensors, tokenizer.json, config.json).
pub fn minilm_f16() -> Option<PathBuf> {
    archive("minilm-f16").map(|d| d.join("package/model"))
}

pub fn kat(name: &str) -> Vec<u8> {
    std::fs::read(manifest().join("kat").join(name)).unwrap()
}

/// A pinned KAT file: its bytes, after the sha256 check.
pub fn pinned(name: &str, sha: &str) -> Vec<u8> {
    let b = kat(name);
    assert_eq!(sha256_hex(&b), sha, "kat/{name} changed — re-record it with kat/record.py and re-pin");
    b
}

pub fn read(dir: &Path, f: &str) -> Vec<u8> {
    std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("{}: {e}", dir.join(f).display()))
}
