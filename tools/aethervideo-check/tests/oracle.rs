//! AETHERVIDEO M3 oracle (LEDGER SR39): Aether + Stria vs Chromium on `oracle/page.html`.
//!
//! Builds the fixture (utp1 WebM with demux_core's writer; the same frames VP9-encoded by
//! Chromium's WebCodecs at quantizer 0 and muxed by demux_core), screenshots the page in Chromium
//! at frame N, renders it in Aether with Stria's real media service on a bandy bus, and scores
//! the three video boxes. Skipped loudly when node or Chromium is absent.
//!
//! AUDIOTRACK (LEDGER SR45) adds two cases to the page: (4) `<audio controls src="tone.wav">` —
//! the box geometry equals Chromium's, Aether's duration (Stria's audio-only `MediaOpened`)
//! equals Chromium's `audio.duration` to the millisecond, and the control's pixels score against
//! Chromium's default audio control; (5) the VP9 `<video>` served over http by a local
//! python `http.server` the test starts (`oracle/serve.py`: byte ranges) — Aether fetches it into its media cache, Stria
//! plays the file, and the counter reads N in both.

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
    // the http case: python's http.server (with byte ranges, `oracle/serve.py`, so Chromium can
    // seek and Aether's cache takes its ranged path) over the work dir on a free port
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut server = Command::new("python3").arg(oracle_dir().join("serve.py")).arg(port.to_string()).arg(&work).current_dir(&work).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().expect("python3 http.server");
    struct Kill(u32);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = Command::new("kill").arg(self.0.to_string()).status();
        }
    }
    let _kill = Kill(server.id());
    let t0 = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(t0.elapsed().as_secs() < 10, "http.server did not start");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let page = std::fs::read_to_string(oracle_dir().join("page.html")).unwrap().replace("{{PORT}}", &port.to_string());
    std::fs::write(work.join("page.html"), page).unwrap();
    let cache = work.join("media-cache");
    for n in [0u32, 7, 9] {
        let chrome = work.join(format!("chromium-{n}.png"));
        let aether = work.join(format!("aether-{n}.png"));
        let shot = run(Command::new("node").env("PLAYWRIGHT_BROWSERS_PATH", &browsers).arg(oracle_dir().join("shot.cjs")).arg(work.join("page.html")).arg(n.to_string()).arg("10").arg(&chrome).args(["660", "500"]));
        let chrome_lines: Vec<&str> = shot.lines().collect();
        assert_eq!(chrome_lines.len(), 5, "{shot}");
        let rendered = run(Command::new(BIN).arg("render").arg(work.join("page.html")).arg(&aether).args(["--frame", &n.to_string(), "--fps", "10", "--width", "660", "--height", "500", "--cache", cache.to_str().unwrap()]));
        let lines: Vec<&str> = rendered.lines().filter(|l| l.starts_with("{\"url\"")).collect();
        assert_eq!(lines.len(), 5, "{rendered}");
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
        // (4) <audio controls>: same box, same duration, Chromium's control look
        assert!(chrome_lines[3].contains("\"tag\":\"audio\"") && chrome_lines[3].contains("\"box\":[330,110,300,54]"), "{}", chrome_lines[3]);
        assert!(lines[3].contains("\"kind\":\"Audio\"") && lines[3].contains("\"box\":[330,110,300,54]"), "{}", lines[3]);
        let (da, dc) = (field(lines[3], "duration_ns") / 1e9, field(chrome_lines[3], "duration"));
        assert!((da - dc).abs() < 0.001, "duration: Aether {da} s, Chromium {dc} s");
        let s = run(Command::new(BIN).arg("compare").arg(&aether).arg(&chrome).arg("330,110,300,54"));
        eprintln!("frame {n} audio box: {}", s.trim());
        assert!(field(&s, "within8_pct") >= 90.0, "{s}");
        // (5) the http <video>: fetched into the cache, handed to Stria as a file, frame N
        assert!(lines[4].contains("\"src\":\"http://127.0.0.1:") && lines[4].contains(&format!("\"url\":\"file://{}", cache.display())), "{}", lines[4]);
        assert!(chrome_lines[4].contains("\"box\":[330,250,320,240]"), "{}", chrome_lines[4]);
        let s = run(Command::new(BIN).arg("compare").arg(&aether).arg(&chrome).args(["330,250,320,240", "--counter", &n.to_string()]));
        eprintln!("frame {n} http box: {}", s.trim());
        assert!(s.contains(&format!("\"counter_a\":{n},\"counter_b\":{n}")), "{s}");
        assert!(field(&s, "within8_pct") >= 98.0 && field(&s, "psnr_db") >= 28.0, "{s}");
    }
    let _ = server.kill();
}
