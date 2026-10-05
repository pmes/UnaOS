//! UNAFSMAP (B354): the paged map tree — sparse 1 TiB volumes, dirty-only
//! commits, the bounded leaf cache, checksummed leaves, grow over holes, and
//! the v6 → v7 migration (incl. the interrupted one and a pre-migration
//! snapshot's legacy inode map).

use unafs::maptree::Shape;
use unafs::fs::FileSystemError;
use unafs::{BLOCK_SIZE, BlockDevice, FileDevice, MemDevice, UnaFS};

const BS: usize = BLOCK_SIZE as usize;

struct Img(std::path::PathBuf);
impl Drop for Img {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn sparse(name: &str, mib: u64) -> Img {
    let p = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&p);
    std::fs::File::create(&p).unwrap().set_len(mib * 1024 * 1024).unwrap();
    Img(p)
}
fn bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len).map(|k| seed.wrapping_add((k * 7) as u8)).collect()
}

#[test]
fn one_tib_sparse_volume_is_a_handful_of_map_blocks() {
    let img = sparse("map-tree-1tib.img", 1024 * 1024);
    let mut fs = UnaFS::format(FileDevice::open(&img.0).unwrap(), 1024 * 1024).unwrap();
    assert_eq!(fs.superblock.version, 7);
    assert_eq!(fs.map_shape(), Shape::Paged);
    assert_eq!(fs.superblock.block_count, 1 << 28);
    // 262,144 refmap leaves, three levels — but only the touched ones exist.
    assert!(fs.map_block_total() < 16, "map blocks {}", fs.map_block_total());
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "a".into()).unwrap();
    fs.write_data(id, 0, &bytes(1, 3 * BS)).unwrap();
    fs.write_data(id, BS as u64, &bytes(2, BS)).unwrap();
    let c = fs.commit_stats().last_commit_blocks;
    assert!(c < 16, "a 1-block write commits {c} blocks");
    drop(fs);
    let mut fs = UnaFS::mount(FileDevice::open(&img.0).unwrap()).unwrap();
    let mut want = bytes(1, 3 * BS);
    want[BS..2 * BS].copy_from_slice(&bytes(2, BS));
    assert_eq!(fs.read_data(id, 0, 3 * BS as u64).unwrap(), want);
    assert!(fs.fsck(false).unwrap().is_clean());
    assert!(fs.map_stats().page_reads < 8);
}

#[test]
fn the_leaf_cache_stays_inside_its_budget() {
    let img = sparse("map-tree-cache.img", 256);
    let mut fs = UnaFS::format(FileDevice::open(&img.0).unwrap(), 256).unwrap();
    fs.set_map_cache_budget(16 * BS);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "big".into()).unwrap();
    // 128 MiB spans 32 refmap leaves — more than the 16-page budget.
    let data = bytes(5, 128 * 1024 * 1024);
    fs.write_data(id, 0, &data).unwrap();
    let s = fs.map_stats();
    assert!(s.pages_resident <= 16, "{s:?}");
    assert!(s.leaf_blocks >= 32, "{s:?}");
    drop(fs);
    let mut fs = UnaFS::mount(FileDevice::open(&img.0).unwrap()).unwrap();
    fs.set_map_cache_budget(16 * BS);
    assert!(fs.fsck(false).unwrap().is_clean());
    let s = fs.map_stats();
    assert!(s.pages_peak <= 17, "{s:?}");
    assert!(s.page_reads >= 32, "fsck walked every used leaf: {s:?}");
    // A write after the walk still allocates right (cache cold for most leaves).
    fs.write_data(id, 0, &bytes(6, BS)).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    let rep = fs.fsck(true).unwrap();
    assert_eq!(rep.reclaimed_blocks, 0);
    assert_eq!(fs.read_data(id, BS as u64, BS as u64).unwrap(), data[BS..2 * BS]);
}

#[test]
fn a_corrupt_refmap_leaf_is_refused_not_trusted() {
    let mut fs = UnaFS::format(MemDevice::new(), 64).unwrap();
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "x".into()).unwrap();
    fs.write_data(id, 0, b"hello").unwrap();
    let mut dev = fs.device.clone();
    drop(fs);
    let (rr, _) = unafs::root::read_active(&mut dev).unwrap().unwrap();
    // The top node's entry 0 is leaf 0 (one level at 64 MiB).
    let mut top = vec![0u8; BS];
    dev.read_block(rr.refmap_block, &mut top).unwrap();
    let leaf0 = u64::from_le_bytes(top[0..8].try_into().unwrap());
    let mut leaf = vec![0u8; BS];
    dev.read_block(leaf0, &mut leaf).unwrap();
    leaf[100] ^= 0x40;
    dev.write_block(leaf0, &leaf).unwrap();
    let mut fs = UnaFS::mount(dev).expect("mount reads index nodes only");
    assert!(matches!(fs.fsck(false), Err(FileSystemError::CorruptVolume(_))));
    assert!(fs.write_data(id, 0, b"again").is_err(), "no commit over an unreadable leaf");
}

#[test]
fn grow_to_a_tebibyte_appends_holes() {
    let img = sparse("map-tree-grow.img", 64);
    let mut fs = UnaFS::format(FileDevice::open(&img.0).unwrap(), 64).unwrap();
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "keep".into()).unwrap();
    fs.write_data(id, 0, b"kept across the grow").unwrap();
    let before = fs.map_block_total();
    drop(fs);
    std::fs::OpenOptions::new().write(true).open(&img.0).unwrap().set_len(1 << 40).unwrap();
    let (mut fs, rep) = unafs::grow(FileDevice::open(&img.0).unwrap(), 1 << 28).unwrap();
    assert_eq!(rep.to, 1 << 28);
    assert!(rep.free_after + 16 >= rep.free_before + (1 << 28) - 16384, "{rep:?}");
    let _ = before;
    assert!(fs.map_block_total() < 16);
    assert!(fs.fsck(false).unwrap().is_clean());
    drop(fs);
    let mut fs = UnaFS::mount(FileDevice::open(&img.0).unwrap()).unwrap();
    assert_eq!(fs.read_data(id, 0, 20).unwrap(), b"kept across the grow");
}

fn v6_volume_with_snapshot() -> (MemDevice, u64, u64, u64) {
    let mut fs = UnaFS::format_with_version(MemDevice::new(), 64, 6).unwrap();
    assert_eq!(fs.map_shape(), Shape::Legacy);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "doc".into()).unwrap();
    fs.write_data(id, 0, b"version one").unwrap();
    let snap = fs.snapshot_create("pre".into(), "kernel".into(), 1).unwrap();
    fs.write_data(id, 0, b"version two").unwrap();
    let big = fs.create_file(root, "big".into()).unwrap();
    fs.write_data(big, 0, &bytes(9, 40 * BS)).unwrap();
    (fs.device.clone(), id, big, snap)
}

fn check_migrated(mut fs: UnaFS<MemDevice>, id: u64, big: u64, snap: u64) {
    assert_eq!(fs.superblock.version, 7);
    assert_eq!(fs.map_shape(), Shape::Paged);
    assert_eq!(fs.read_data(id, 0, 11).unwrap(), b"version two");
    assert_eq!(fs.read_data(big, 0, 40 * BS as u64).unwrap(), bytes(9, 40 * BS));
    let mut view = fs.open_snapshot(snap).unwrap();
    assert_eq!(view.read_data(id, 0, 11).unwrap(), b"version one", "legacy snapshot map");
    drop(view);
    assert!(fs.fsck(false).unwrap().is_clean());
    fs.snapshot_drop(snap).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean(), "drop leaks nothing");
    let dev = fs.device.clone();
    drop(fs);
    let mut again = UnaFS::mount(dev).unwrap();
    assert_eq!(again.superblock.version, 7);
    assert_eq!(again.read_data(id, 0, 11).unwrap(), b"version two");
    assert!(again.fsck(false).unwrap().is_clean());
}

#[test]
fn a_v6_volume_migrates_on_its_first_commit() {
    let (dev, id, big, snap) = v6_volume_with_snapshot();
    let mut fs = UnaFS::mount(dev).unwrap();
    assert_eq!(fs.superblock.version, 6, "mount alone writes nothing");
    assert_eq!(fs.map_shape(), Shape::Legacy);
    assert!(fs.fsck(false).unwrap().is_clean());
    let free = fs.free_blocks();
    let root = fs.superblock.root_inode;
    let n = fs.create_file(root, "after".into()).unwrap();
    fs.write_data(n, 0, b"post").unwrap();
    assert!(fs.free_blocks() + 8 > free, "the migration keeps every leaf block");
    check_migrated(fs, id, big, snap);
}

#[test]
fn an_interrupted_migration_finishes_on_the_next_commit() {
    let (dev, id, big, snap) = v6_volume_with_snapshot();
    let mut fs = UnaFS::mount(dev).unwrap();
    fs.migrate_interrupted_before_superblock().unwrap();
    let dev = fs.device.clone();
    drop(fs);
    let mut fs = UnaFS::mount(dev).unwrap();
    assert_eq!(fs.superblock.version, 6, "block 0 not yet rewritten");
    assert_eq!(fs.map_shape(), Shape::Paged, "the MIGRATE root is read paged");
    assert_eq!(fs.read_data(id, 0, 11).unwrap(), b"version two");
    assert!(fs.fsck(false).unwrap().is_clean());
    fs.commit().unwrap();
    check_migrated(fs, id, big, snap);
}

#[test]
fn a_v5_volume_stays_legacy_and_commits_incrementally() {
    let mut fs = UnaFS::format_with_version(MemDevice::new(), 3072, 5).unwrap();
    assert_eq!(fs.map_shape(), Shape::Legacy);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "old".into()).unwrap();
    fs.write_data(id, 0, &bytes(3, BS)).unwrap();
    fs.write_data(id, 0, &bytes(4, BS)).unwrap();
    // 768 legacy leaves (two levels) — a commit writes a few, not 770.
    let c = fs.commit_stats().last_commit_blocks;
    assert!(c < 16, "legacy commit wrote {c}");
    let dev = fs.device.clone();
    drop(fs);
    let mut fs = UnaFS::mount(dev).unwrap();
    assert_eq!(fs.superblock.version, 5);
    assert_eq!(fs.read_data(id, 0, BS as u64).unwrap(), bytes(4, BS));
    assert!(fs.fsck(false).unwrap().is_clean());
}
