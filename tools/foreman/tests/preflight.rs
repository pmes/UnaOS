//! The spec preflight, end to end through the binary.
//!
//! foreman evaluates with the Rust `regex` crate, which refuses look-around BY
//! DESIGN, while `mbench.py` (Python `re`) accepts it. On such a spec the run
//! must stop BEFORE evaluation with a report that names the offending spec
//! lines — and print no verdict table at all, so the mid-run hard stop that
//! named nothing can never come back.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmpdir(case: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("foreman-preflight-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).expect("fixture written");
    p
}

const LOG: &str = ":: FIRST-WITNESS PASS ::\n:: RUN-END marker ::\n";

fn run(case: &str, spec_body: &str) -> (i32, String, String) {
    let dir = tmpdir(case);
    let log = write(&dir, "preflight.log", LOG);
    let spec = write(&dir, "preflight.spec", spec_body);
    let out = Command::new(env!("CARGO_BIN_EXE_foreman"))
        .arg("--log")
        .arg(&log)
        .arg("--spec")
        .arg(&spec)
        .output()
        .expect("foreman runs");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn look_ahead_spec_stops_before_evaluation() {
    let (rc, stdout, stderr) = run(
        "lookahead",
        "# a spec mbench accepts and foreman cannot\n\
         REQUIRE FIRST-WITNESS PASS\n\
         REQUIRE ^(?=.*RUN-END).*marker\n",
    );
    assert_eq!(rc, foreman::verdict::RC_ERROR, "stdout={stdout} stderr={stderr}");
    assert!(stdout.is_empty(), "no evaluation output expected, got: {stdout}");
    assert!(stderr.contains("preflight.spec:3: REQUIRE ^(?=.*RUN-END).*marker — "), "{stderr}");
    assert!(stderr.contains("look-around"), "{stderr}");
    assert!(stderr.contains("mbench.py"), "{stderr}");
    assert!(!stderr.contains("MBENCH VERDICT"), "{stderr}");
}

#[test]
fn valid_spec_is_untouched_by_the_preflight() {
    let (rc, stdout, stderr) = run("valid", "REQUIRE FIRST-WITNESS PASS\nCOMPLETE RUN-END marker\n");
    assert_eq!(rc, foreman::verdict::RC_PASS, "stdout={stdout} stderr={stderr}");
    assert!(stdout.starts_with("════════════ MBENCH VERDICT"), "{stdout}");
    assert!(!stdout.contains("preflight FAILED"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}
