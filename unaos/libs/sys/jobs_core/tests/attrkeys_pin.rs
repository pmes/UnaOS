// GATE-ATTRKEYS (B452): jobs_core is dependency-free, so its K_* keys MIRROR `una_abi::attr_keys`; this host test is
// the pin that keeps the two spellings one.
use jobs_core::*;
use una_abi::attr_keys as k;

#[test]
fn job_keys_are_the_registry_literals() {
    assert_eq!(K_KIND, k::JOB_KIND);
    assert_eq!(K_ID, k::JOB_ID);
    assert_eq!(K_SEQ, k::JOB_SEQ);
    assert_eq!(K_STATUS, k::JOB_STATUS);
    assert_eq!(K_FLIGHT, k::JOB_FLIGHT);
    assert_eq!(K_LINE, k::JOB_LINE);
    assert_eq!(K_SET_BY, k::JOB_SET_BY);
    assert_eq!(K_REFS, k::JOB_REFS);
    assert_eq!(K_OWNER, k::JOB_OWNER);
    assert_eq!(K_ARC, k::JOB_ARC);
    assert_eq!(K_TRACK, k::JOB_TRACK);
    assert_eq!(TYPE_KEY, k::TYPE);
    assert_eq!(VIEW_KEY, k::VIEW);
}
