//! AETHERTESTROOTS (LEDGER SR696): an integration binary re-rooted by `aether::storage::init_root` keeps every
//! file Aether writes under its process root — the storage buckets, the HTTP cache, the font index
//! (`fonts::db_cache_dir`, registered by `init_root` before the first engine) and the bookmarks file
//! (`chrome::host::bookmarks_path`) — and none under the platform data directory.
//!
//! CHARTER: Aether — the engine half's integration test of the one storage root (`storage`, `fonts`, `chrome::host`).

use aether::AetherEngine;

#[test]
fn every_path_sits_under_the_process_root() {
    let root = aether::storage::init_root(env!("CARGO_TARGET_TMPDIR"), "testroot");
    let mut engine = AetherEngine::new();
    engine.load_html_styled("https://testroot.kat/", "<p style=\"font-family: sans-serif\">root</p>", &[], true);

    let platform = aether::storage::platform_data_dir();
    assert_eq!(aether::storage::root_dir(), root);
    assert_eq!(aether::storage::init_root(env!("CARGO_TARGET_TMPDIR"), "other"), root, "one root per process");
    let fonts = aether::fonts::db_cache_dir();
    let bookmarks = aether::chrome::host::bookmarks_path().expect("a bookmarks path");
    let http = aether::storage::http_cache_dir();
    for p in [&fonts, &bookmarks, &http] {
        assert!(p.starts_with(&root), "{p:?} under {root:?}");
        assert!(!p.starts_with(&platform), "{p:?} never under the platform data directory");
    }
    let db = text_host::db::db();
    if !db.faces.is_empty() && db.discover_cache() == "miss" {
        assert!(fonts.join(text_host::db_cache::INDEX_FILE).is_file(), "the font index was written under the root");
    }
}
