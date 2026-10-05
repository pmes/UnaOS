//! `js_core-bench`: the Octane-style microbenchmarks in bench/*.js, timed on js_core and (optionally) on another
//! engine's command-line runner, reporting per-kernel times, the output check and the time ratio.
//!
//! cargo run --release -p js_core --example js_core-bench -- [--other <runner-binary>] [--runs 3]
//! Both engines run each kernel as a fresh process (`js_core-run` next to this binary); the best of `--runs`
//! wall-clock times is reported, minus each engine's measured startup time (an empty script).

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

fn time_run(bin: &PathBuf, file: &PathBuf, runs: usize) -> (f64, String) {
    let mut best = f64::INFINITY;
    let mut out = String::new();
    for _ in 0..runs {
        let t = Instant::now();
        let o = Command::new(bin).arg(file).output().expect("run");
        let dt = t.elapsed().as_secs_f64();
        if dt < best {
            best = dt;
        }
        out = String::from_utf8_lossy(&o.stdout).trim().lines().last().unwrap_or("").to_string();
    }
    (best, out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut other: Option<PathBuf> = None;
    let mut runs = 3;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--other" => {
                other = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--runs" => {
                runs = args[i + 1].parse().unwrap();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    let ours = std::env::current_exe().unwrap().with_file_name("js_core-run");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bench");
    let empty = std::env::temp_dir().join("js_core_bench_empty.js");
    std::fs::write(&empty, "").unwrap();
    let (s_ours, _) = time_run(&ours, &empty, 5);
    let s_other = other.as_ref().map(|b| time_run(b, &empty, 5).0).unwrap_or(0.0);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x == "js").unwrap_or(false)).collect();
    files.sort();
    println!("{:<12} {:>10} {:>10} {:>8}  output", "kernel", "js_core s", "other s", "ratio");
    let mut log_sum = 0.0;
    let mut n = 0;
    for f in &files {
        let (t1, o1) = time_run(&ours, f, runs);
        let t1 = (t1 - s_ours).max(1e-4);
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        if let Some(b) = &other {
            let (t2, o2) = time_run(b, f, runs);
            let t2 = (t2 - s_other).max(1e-4);
            let same = if o1 == o2 { "same" } else { "DIFFERENT" };
            println!("{:<12} {:>10.3} {:>10.3} {:>8.2}  {} ({})", name, t1, t2, t1 / t2, o1, same);
            log_sum += (t1 / t2).ln();
            n += 1;
        } else {
            println!("{:<12} {:>10.3} {:>10} {:>8}  {}", name, t1, "-", "-", o1);
        }
    }
    if n > 0 {
        println!("geometric mean time ratio js_core/other: {:.2} (startup js_core {:.3}s, other {:.3}s)", (log_sum / n as f64).exp(), s_ours, s_other);
    }
}
