// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFBUILD6 (B360): the host proofs `cargo test -p ldso_core` runs.
// * `tramp_bytes_match_source`: `src/tramp_bytes.rs` is exactly what `src/tramp.S` assembles to (and has no relocations).
// * `dyn_probe`: builds fixtures/dyn (a musl PIE + libdyn.so + libplug.so, libc.so relinked from the Rust musl target's
//   self-contained libc.a) and runs it under `ldrun`: constructors, the seven relocation types, static TLS in the main
//   thread and a new one, dl_iterate_phdr, dlopen/dlsym/dlerror/dladdr. Prints `dyn=ok`.
// Each SKIPS (passes, saying so) when clang / ld.lld / the musl target is not on this host.
use std::path::{Path, PathBuf};
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn musl_self_contained() -> Option<PathBuf> {
    let out = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
    let root = String::from_utf8(out.stdout).ok()?;
    let p = Path::new(root.trim()).join("lib/rustlib/x86_64-unknown-linux-musl/lib/self-contained");
    p.join("libc.a").exists().then_some(p)
}

#[test]
fn tramp_bytes_match_source() {
    if !have("clang") || !have("llvm-objcopy") {
        eprintln!("SKIP: clang/llvm-objcopy absent");
        return;
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let st = Command::new("python3").arg(format!("{}/tools/gen_tramp.py", root)).arg("--check").status().expect("python3");
    assert!(st.success(), "src/tramp_bytes.rs is stale: run python3 tools/gen_tramp.py");
}

#[test]
fn dyn_probe() {
    let Some(sc) = musl_self_contained() else {
        eprintln!("SKIP: rustup target x86_64-unknown-linux-musl absent");
        return;
    };
    if !have("clang") || !have("ld.lld") || !have("gcc") {
        eprintln!("SKIP: clang/ld.lld/gcc absent");
        return;
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dyn");
    let st = Command::new("sh").arg(format!("{}/fixtures/dyn/build.sh", root)).arg(&sc).arg(&out).status().expect("sh");
    assert!(st.success(), "fixtures/dyn/build.sh failed");
    let r = Command::new(env!("CARGO_BIN_EXE_ldrun")).arg(out.join("hello")).arg("x").output().expect("ldrun");
    let so = String::from_utf8_lossy(&r.stdout);
    eprintln!("{}{}", so, String::from_utf8_lossy(&r.stderr));
    assert!(so.starts_with("dyn=ok objects=3 after_dlopen=4"), "dyn probe: {}", so);
    assert!(so.contains("dyn: destructor ran"), "fini list did not run");
    assert_eq!(r.status.code(), Some(0));
}
