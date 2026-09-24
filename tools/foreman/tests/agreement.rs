//! The shared-corpus agreement test (design §5.1).
//!
//! `mbench.py` remains the bench's tool and its spec semantics are the
//! reference. The `verdict` module must agree with it directive-for-directive —
//! including the default FORBID set and the binary-safe sanitization — and the
//! cheapest way to keep that true is a shared corpus: the same (log, spec) pairs
//! evaluated by both, asserted equal.
//!
//! Agreement is asserted at the STRONGEST available level: the exit code AND the
//! rendered verdict table, byte for byte. The corpus is
//!
//!   * every checked-in `unaos/scripts/specs/*.spec` crossed with every capture
//!     present in the tree (`unaos/target/serial*.log`, `~/*-serial.log`);
//!   * mbench's own `--self-test` canned lines, written to temp files, so the
//!     corpus never depends on a QEMU or bench log existing in this checkout.
//!
//! If `python3` or `mbench.py` is not reachable, the test SKIPS loudly rather
//! than passing quietly — a green run that proved nothing is the failure mode
//! this whole tool exists to avoid.

use std::path::{Path, PathBuf};
use std::process::Command;

use foreman::{capture, verdict};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root resolves")
}

fn mbench_py(root: &Path) -> PathBuf {
    root.join("unaos/scripts/mbench.py")
}

/// Run mbench in replay mode; returns (rc, stdout).
fn run_mbench(root: &Path, log: &Path, spec: &Path) -> Option<(i32, String)> {
    run_mbench_art(root, log, spec, None)
}

/// mbench replay with `--artifact` (FORBID-UNREACHABLE, B210) when one is given.
fn run_mbench_art(root: &Path, log: &Path, spec: &Path, art: Option<&Path>) -> Option<(i32, String)> {
    let mut cmd = Command::new("python3");
    cmd.arg(mbench_py(root))
        .arg("--replay")
        .arg(log)
        .arg("--spec")
        .arg(spec)
        .arg("--quiet");
    if let Some(a) = art {
        cmd.arg("--artifact").arg(a);
    }
    let out = cmd.current_dir(root).output().ok()?;
    let rc = out.status.code()?;
    Some((rc, String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Run foreman's `verdict` module over the same pair.
fn run_foreman(log: &Path, spec: &Path) -> (i32, String) {
    run_foreman_art(log, spec, None)
}

fn run_foreman_art(log: &Path, spec: &Path, art: Option<&Path>) -> (i32, String) {
    let cap = capture::read(log).expect("log readable");
    let ds = verdict::parse_spec(spec).expect("spec parses");
    let mut ev = verdict::evaluate(ds, &cap, spec);
    verdict::apply_reach(&mut ev, art);
    (ev.verdict().rc(), verdict::render_table(&ev))
}

fn specs(root: &Path) -> Vec<PathBuf> {
    let dir = root.join("unaos/scripts/specs");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("spec"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Captures present in this checkout. May be empty — the canned corpus below
/// carries the test when it is.
fn logs(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.join("unaos/target")) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with("serial") && name.ends_with(".log") && p.is_file() {
                v.push(p);
            }
        }
    }
    v.sort();
    v
}

// mbench's own canned self-test lines — control bytes, split UTF-8, an
// unterminated final line, and the truncation fixtures.
const CANNED: &[u8] = b"\x1b[2J\x1b[HUEFI firmware noise \x00\x01 garbage\r\n\
:: CAPSTONE Semaphore: PASS ::\r\n\
:: \x1b[32mU4: process model \xe2\x80\x94 reaped -> PASS\x1b[0m ::\r\n\
:: U5: capabilities -> PASS ::\r\n\
half a line, no newline yet";
const CANNED_TAIL: &[u8] =
    b" \xe2\x80\xa6 now finished\r\n:: CAPSTONE COMPLETE \xe2\x80\x94 all 6 verified ::\r\n";
const CANNED_BAD: &[u8] = b":: U9: write-back -> FAIL (sector mismatch) ::\r\n";

const SELFTEST_SPEC: &[u8] = b"# mbench self-test spec\n\
REQUIRE CAPSTONE COMPLETE\n\
COUNT 2 -> PASS\n\
OPTIONAL Semaphore: PASS\n\
PENDING NEVER-FLASHED-WITNESS\n";
const TRUNC_SPEC: &[u8] = b"# mbench self-test spec - end-of-run marker declared\n\
COMPLETE RUN-END marker\n\
REQUIRE FIRST-WITNESS PASS\n\
REQUIRE LAST-WITNESS PASS\n";
const TRUNC_HEAD: &[u8] = b":: FIRST-WITNESS PASS ::\r\n";

// FORBID-UNREACHABLE (B210) corpus: five spec FORBIDs against one "artifact" whose
// bytes carry SOME of their literals (`kind=ax88179` is the real x86-usbnet.spec
// row that motivated the gate: the artifact carries `kind=ecm`, not it). `-> FAIL` is the wire's default FORBID text
// and IS in the artifact (reachable, 0 hits → ✅ as before); `RENAMED-WITNESS` is
// NOT (→ ◦ UNREACHABLE); the third HITS the capture (a hit is a hit, reach or no
// reach → ❌); the fourth's longest literal is under REACH_MIN, so it is never
// checked (✅). The escaped-metachar and class shapes come from real specs.
const REACH_SPEC: &[u8] = b"# FORBID-UNREACHABLE agreement spec\n\
REQUIRE CAPSTONE COMPLETE\n\
FORBID :: U[0-9]+: .* -> FAIL \\(sector\n\
FORBID :: RENAMED-WITNESS: .* ::\n\
FORBID :: U5: capabilities\n\
FORBID U[45]:\n\
FORBID kind=ax88179\n";
const REACH_ART: &[u8] = b"\x7fELF\x02\x01\x01 :: U4: process model -> FAIL (sector mismatch) :: \
:: U5: capabilities -> PASS :: kind=ecm \x00 CAPSTONE COMPLETE \x00";
const TRUNC_TAIL: &[u8] = b":: LAST-WITNESS PASS ::\r\n:: RUN-END marker ::\r\n";

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).expect("fixture writable");
    p
}

#[test]
fn agrees_with_mbench_on_the_shared_corpus() {
    let root = repo_root();
    if !mbench_py(&root).is_file() {
        eprintln!("SKIP: {} not present", mbench_py(&root).display());
        return;
    }
    // Probe once: if python3 cannot run mbench at all, skip loudly.
    let probe = Command::new("python3")
        .arg(mbench_py(&root))
        .arg("--help")
        .output();
    if probe.map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("SKIP: python3 could not run mbench.py");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("foreman-agreement-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");

    // --- the canned corpus (always present) ---------------------------------
    let selftest_spec = write(&tmp, "selftest.spec", SELFTEST_SPEC);
    let trunc_spec = write(&tmp, "trunc.spec", TRUNC_SPEC);
    let mut pairs: Vec<(PathBuf, PathBuf)> = vec![
        (write(&tmp, "good.log", &cat(&[CANNED, CANNED_TAIL])), selftest_spec.clone()),
        (write(&tmp, "bad.log", &cat(&[CANNED, CANNED_TAIL, CANNED_BAD])), selftest_spec.clone()),
        (write(&tmp, "short.log", CANNED), selftest_spec.clone()),
        (write(&tmp, "t-good.log", &cat(&[TRUNC_HEAD, TRUNC_TAIL])), trunc_spec.clone()),
        (write(&tmp, "t-cut.log", TRUNC_HEAD), trunc_spec.clone()),
        (
            write(&tmp, "t-regress.log", &cat(&[TRUNC_HEAD, b":: RUN-END marker ::\r\n"])),
            trunc_spec.clone(),
        ),
        (
            write(&tmp, "t-midline.log", &cat(&[TRUNC_HEAD, TRUNC_TAIL, b":: half a li"])),
            trunc_spec.clone(),
        ),
        (
            write(&tmp, "t-panic.log", &cat(&[TRUNC_HEAD, b"PANIC: something exploded\r\n"])),
            trunc_spec.clone(),
        ),
        // The end-of-run marker and NOTHING else. The run finished, so both pinned
        // REQUIREs are a GENUINE shortfall — the only pair in this corpus that
        // reaches mbench's `(N further pinned line(s) also short)` companion to
        // FIRST-SHORTFALL. Added with the SPECRUN shapes: without it that branch is
        // a string no fixture can execute, which is the vacuum this test exists to
        // refuse.
        (
            write(&tmp, "t-both-short.log", b":: RUN-END marker ::\r\n"),
            trunc_spec.clone(),
        ),
    ];

    // --- every checked-in spec x every capture in the tree -------------------
    let real_logs = logs(&root);
    for spec in specs(&root) {
        for log in &real_logs {
            pairs.push((log.clone(), spec.clone()));
        }
    }

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for (log, spec) in &pairs {
        let Some((mrc, mtable)) = run_mbench(&root, log, spec) else {
            eprintln!("SKIP pair (mbench did not run): {} / {}", log.display(), spec.display());
            continue;
        };
        let (frc, ftable) = run_foreman(log, spec);
        checked += 1;
        if mrc != frc {
            mismatches.push(format!(
                "exit code: {} vs {}: mbench {mrc}, foreman {frc}",
                log.display(),
                spec.display()
            ));
        }
        if mtable.trim_end() != ftable.trim_end() {
            let first = mtable
                .lines()
                .zip(ftable.lines())
                .find(|(a, b)| a != b)
                .map(|(a, b)| format!("\n  mbench : {a}\n  foreman: {b}"))
                .unwrap_or_else(|| " (length differs)".to_string());
            mismatches.push(format!(
                "table: {} / {}:{first}",
                log.display(),
                spec.display()
            ));
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);

    assert!(checked > 0, "the agreement corpus was empty — nothing was proved");
    assert!(
        mismatches.is_empty(),
        "foreman disagrees with mbench on {}/{checked} pairs:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
    eprintln!("agreement: {checked} (log, spec) pairs — exit code and verdict table identical");
}

#[test]
fn agrees_with_mbench_on_forbid_reachability() {
    let root = repo_root();
    if !mbench_py(&root).is_file() {
        eprintln!("SKIP: {} not present", mbench_py(&root).display());
        return;
    }
    let tmp = std::env::temp_dir().join(format!("foreman-reach-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let spec = write(&tmp, "reach.spec", REACH_SPEC);
    let art = write(&tmp, "reach-kernel.elf", REACH_ART);
    let good = write(&tmp, "good.log", &cat(&[CANNED, CANNED_TAIL]));
    let bad = write(&tmp, "bad.log", &cat(&[CANNED, CANNED_TAIL, CANNED_BAD]));

    let mut mismatches: Vec<String> = Vec::new();
    let mut unreach_seen = false;
    for log in [&good, &bad] {
        let Some((mrc, mtable)) = run_mbench_art(&root, log, &spec, Some(&art)) else {
            eprintln!("SKIP: mbench did not run");
            return;
        };
        let (frc, ftable) = run_foreman_art(log, &spec, Some(&art));
        if mrc != frc || mtable.trim_end() != ftable.trim_end() {
            mismatches.push(format!("{}:\n--- mbench\n{mtable}\n--- foreman\n{ftable}", log.display()));
        }
        // The rule itself, on foreman's table (mbench's is byte-identical by now).
        if ftable.contains("UNREACHABLE: its literal \":: RENAMED-WITNESS: \"") {
            unreach_seen = true;
        }
        assert!(
            !ftable.contains("UNREACHABLE: its literal \"-> FAIL"),
            "a literal present in the artifact was called unreachable:\n{ftable}"
        );
        assert!(
            ftable.contains(", 2 unreachable FORBID(s)"),
            "summary must count the two unreachable FORBIDs (RENAMED-WITNESS, kind=ax88179):\n{ftable}"
        );
    }
    // Advisory: the verdict is the verdict without the artifact.
    let (rc_no, _) = run_foreman(&good, &spec);
    let (rc_art, _) = run_foreman_art(&good, &spec, Some(&art));
    assert_eq!(rc_no, rc_art, "reachability changed a verdict");
    let (rc_bad, table_bad) = run_foreman_art(&bad, &spec, Some(&art));
    assert_ne!(rc_bad, 0, "the FORBID hit must still fail:\n{table_bad}");

    let _ = std::fs::remove_dir_all(&tmp);
    assert!(unreach_seen, "the renamed token was not reported UNREACHABLE");
    assert!(
        mismatches.is_empty(),
        "foreman disagrees with mbench with --artifact:\n{}",
        mismatches.join("\n")
    );
    eprintln!("reach agreement: 2 pairs — tables identical, UNREACHABLE row and summary present");
}
