// GATE-ATTRKEYS (B452): unafs links no una-abi, so nameindex's keys MIRROR `una_abi::attr_keys`; this host test is
// the pin.
#[test]
fn nameindex_keys_are_the_registry_literals() {
    assert_eq!(unafs::fs::nameindex::NAME_KEY, una_abi::attr_keys::FSNAME);
    assert_eq!(unafs::fs::nameindex::NAME_MARK_KEY, una_abi::attr_keys::FSNAME_INDEX);
}
