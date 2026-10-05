//! AETHERVIDEO M3 oracle (LEDGER SR39): Aether + Stria vs Chromium on `oracle/page.html`.
//!
//! Builds the fixture (utp1 WebM with demux_core's writer; the same frames VP9-encoded by
//! Chromium's WebCodecs at quantizer 0 and muxed by demux_core), screenshots the page in Chromium
//! at frame N, renders it in Aether with Stria's real media service on a bandy bus, and scores
//! the three video boxes. Skipped loudly when node or Chromium is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_aethervideo-check");

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle")
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("spawn");
    assert!(out.status.success() || out.status.code() == Some(1), "{cmd:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn field(json: &str, key: &str) -> f64 {
    let i = json.find(&format!("\"{key}\":")).unwrap_or_else(|| panic!("{key} in {json}")) + key.len() + 3;
    json[i..].split([',', '}']).next().unwrap().parse().unwrap_or(f64::NAN)
}

#[test]
fn aether_matches_chromium_at_frame_n() {
    let browsers = std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_else(|_| "/opt/pw-browsers".into());
    if Command::new("node").arg("--version").output().is_err() || !Path::new(&browsers).exists() {
        eprintln!("SKIP aether_matches_chromium_at_frame_n: node or Chromium ({browsers}) not present");
        return;
    }
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("aethervideo-oracle");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    run(Command::new(BIN).args(["make", work.to_str().unwrap(), "10", "10", "320", "240"]));
    let enc = run(Command::new("node").env("PLAYWRIGHT_BROWSERS_PATH", &browsers).arg(oracle_dir().join("encode.cjs")).arg(&work));
    assert!(enc.contains("\"chunks\":10"), "{enc}");
    run(Command::new(BIN).args(["mux", work.join("chunks.bin").to_str().unwrap(), work.join("pattern-vp9.webm").to_str().unwrap(), "320", "240", "10"]));
    std::fs::copy(oracle_dir().join("page.html"), work.join("page.html")).unwrap();
    for n in [0u32, 7, 9] {
        let chrome = work.join(format!("chromium-{n}.png"));
        let aether = work.join(format!("aether-{n}.png"));
        let shot = run(Command::new("node").env("PLAYWRIGHT_BROWSERS_PATH", &browsers).arg(oracle_dir().join("shot.cjs")).arg(work.join("page.html")).arg(n.to_string()).arg("10").arg(&chrome).args(["640", "500"]));
        assert_eq!(shot.lines().count(), 3, "{shot}");
        let rendered = run(Command::new(BIN).arg("render").arg(work.join("page.html")).arg(&aether).args(["--frame", &n.to_string(), "--fps", "10", "--width", "640", "--height", "500"]));
        let lines: Vec<&str> = rendered.lines().filter(|l| l.starts_with("{\"url\"")).collect();
        assert_eq!(lines.len(), 3, "{rendered}");
        assert!(lines[0].contains("pattern-utp.webm") && lines[0].contains("\"real_video\":true"), "Aether picks the utp1 source: {}", lines[0]);
        assert!(lines[2].contains("pattern-vp9.webm"), "{}", lines[2]);
        for (i, b) in ["0,0,320,240", "330,0,300,100", "0,250,320,240"].iter().enumerate() {
            let mut c = Command::new(BIN);
            c.arg("compare").arg(&aether).arg(&chrome).arg(b);
            if i != 1 {
                c.args(["--counter", &n.to_string()]);
            }
            let s = run(&mut c);
            eprintln!("frame {n} box {b}: {}", s.trim());
            if i != 1 {
                assert!(s.contains(&format!("\"counter_a\":{n},\"counter_b\":{n}")), "the counter reads {n} in both: {s}");
                // Residual = Chromium's VP9 4:2:0 round trip: chroma at three odd bar edges and
                // a few levels of YUV rounding inside some bars.
                assert!(field(&s, "within8_pct") >= 98.0 && field(&s, "psnr_db") >= 28.0, "{s}");
            } else {
                // Letterbox geometry (same box, same picture rect), different scaling filters.
                assert!(field(&s, "within8_pct") >= 95.0, "{s}");
            }
        }
    }
}
