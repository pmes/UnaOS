use std::path::PathBuf;
use directories::BaseDirs;

pub fn data_dir() -> PathBuf {
    BaseDirs::new()
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("unaos")
}
