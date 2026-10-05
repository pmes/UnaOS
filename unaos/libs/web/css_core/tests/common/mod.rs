//! Shared test plumbing: pinned vector fetch (curl + crypto_core SHA-256) and the JSON reader.
#![allow(dead_code)]
pub mod dom;
pub mod style;
pub mod json;

use std::path::PathBuf;

const VECTORS_TXT: &str = include_str!("../vectors.txt");

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn cache_dir() -> PathBuf {
    match std::env::var_os("CSS_VECTORS_DIR") {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir().join("unaos-css-vectors"),
    }
}

/// A FETCH file's text: from the cache when its sha256 matches, else downloaded and verified.
/// `Err(reason)` when offline / `CSS_OFFLINE=1` / the download does not match its pin.
pub fn fetch(name: &str) -> Result<String, String> {
    let line = VECTORS_TXT
        .lines()
        .find(|l| {
            let mut it = l.split_whitespace();
            it.next() == Some("FETCH") && it.next() == Some(name)
        })
        .ok_or_else(|| format!("{name}: not in vectors.txt"))?;
    let f: Vec<&str> = line.split_whitespace().collect();
    let (want, url) = (f[2], f[3]);
    let dir = cache_dir();
    let path = dir.join(name);
    if let Ok(b) = std::fs::read(&path) {
        if hex(&crypto_core::sha2::sha256(&b)) == want {
            return Ok(String::from_utf8_lossy(&b).into_owned());
        }
    }
    if std::env::var_os("CSS_OFFLINE").is_some() {
        return Err(format!("{name}: CSS_OFFLINE set"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{name}: {e}"))?;
    let tmp = dir.join(format!("{name}.part{}", std::process::id()));
    let st = std::process::Command::new("curl")
        .args(["-sSfL", "--max-time", "120", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| format!("{name}: curl: {e}"))?;
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{name}: offline (curl {st})"));
    }
    let b = std::fs::read(&tmp).map_err(|e| format!("{name}: {e}"))?;
    let got = hex(&crypto_core::sha2::sha256(&b));
    if got != want {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{name}: sha256 {got} != pinned {want}"));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("{name}: {e}"))?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}
