//! CARDREGISTRY (rmbp-ledger B833): the stamp KAT — the value the kernel prints (`[filetypes] … stamp=`) and the card
//! builder writes, pinned at 159f16f6. A change to the facts, the extension table or a built-in's resource block moves
//! it; re-pin with the new value AND bump nothing else.
use type_registry_core::*;

#[test]
fn stamp_kat() {
    assert_eq!(stamp(), "v1 h=87bb828989426a43 n=32");
    assert_eq!(builtin_type_count(), 32);
    assert_eq!(TYPE_FACTS.len(), 31);
    assert_eq!(BUILTIN.len(), 13);
}

#[test]
fn fresh_objects_are_the_compiled_in_types() {
    let objs = fresh_objects();
    assert_eq!(objs.len(), builtin_type_count());
    let mut leaves: Vec<&str> = objs.iter().map(|(l, _)| l.as_str()).collect();
    leaves.sort();
    leaves.dedup();
    assert_eq!(leaves.len(), objs.len());
    assert!(objs.iter().all(|(l, _)| !l.contains('/')));
}

fn get<'a>(kv: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
    kv.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str())
}

#[test]
fn object_keys_samples() {
    use una_abi::attr_keys::{DESCRIPTION, EXTENSIONS, ICON, PREFERRED};
    let png = object_keys("image/png");
    assert_eq!(get(&png, DESCRIPTION), Some("PNG image"));
    assert_eq!(get(&png, ICON), Some("image"));
    assert_eq!(get(&png, EXTENSIONS), Some("png"));
    assert_eq!(get(&png, PREFERRED), Some("org.unaos.facet"));
    assert_eq!(leaf("image/png"), "image-png");

    let dir = object_keys("inode/directory");
    assert_eq!(get(&dir, EXTENSIONS), None);
    assert_eq!(get(&dir, PREFERRED), Some("org.unaos.quarry"));

    let opus = object_keys("audio/opus");
    assert_eq!(get(&opus, DESCRIPTION), Some("audio/opus"));
    assert_eq!(get(&opus, ICON), Some("file"));
    assert_eq!(get(&opus, EXTENSIONS), None);
    assert_eq!(get(&opus, PREFERRED), Some("org.unaos.player"));

    let oct = object_keys("application/octet-stream");
    assert_eq!(get(&oct, EXTENSIONS), None);
    assert_eq!(get(&oct, PREFERRED), None);

    assert_eq!(get(&object_keys("text/markdown"), EXTENSIONS), Some("md, markdown"));
    assert_eq!(get(&object_keys("text/plain"), PREFERRED), Some("org.unaos.textedit"));
}
