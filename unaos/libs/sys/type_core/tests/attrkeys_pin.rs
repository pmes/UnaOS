// GATE-ATTRKEYS (B452): type_core links no una-abi, so EXTENSIONS_KEY MIRRORS `una_abi::attr_keys::EXTENSIONS`;
// this host test is the pin.
#[test]
fn extensions_key_is_the_registry_literal() {
    assert_eq!(type_core::EXTENSIONS_KEY, una_abi::attr_keys::EXTENSIONS);
}
