//! UNAFSMAP (B354): the refcount-map bench — mount time and peak heap on a
//! SPARSE image file, blocks written by a 1-block-write commit, and fsck
//! time. The `#[ignore]`d `bench_*` tests print the table rows the arc's doc
//! carries (`cargo test -p unafs --release --test map_bench -- --ignored
//! --nocapture --test-threads 1`); the un-ignored tests are the proof bounds.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use unafs::{FileDevice, UnaFS};

struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = CUR.fetch_add(l.size(), Ordering::SeqCst) + l.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CUR.fetch_sub(l.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Reset the peak to the current live heap; returns that baseline.
fn peak_reset() -> usize {
    let c = CUR.load(Ordering::SeqCst);
    PEAK.store(c, Ordering::SeqCst);
    c
}
fn peak_since(base: usize) -> usize {
    PEAK.load(Ordering::SeqCst).saturating_sub(base)
}

struct Img(std::path::PathBuf);
impl Drop for Img {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A sparse image file of `mib` MiB.
fn sparse(name: &str, mib: u64) -> Img {
    let p = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&p);
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(mib * 1024 * 1024).unwrap();
    Img(p)
}

/// One row: format a sparse `mib` image, write one file, remount (timed, heap
/// measured), a 1-block overwrite commit (blocks counted), fsck (timed).
pub fn row(mib: u64) -> (f64, usize, u64, f64, u64) {
    let img = sparse(&format!("map-bench-{mib}.img"), mib);
    {
        let mut fs = UnaFS::format(FileDevice::open(&img.0).unwrap(), mib).unwrap();
        let root = fs.superblock.root_inode;
        let id = fs.create_file(root, "f".into()).unwrap();
        fs.write_data(id, 0, &[7u8; 4096]).unwrap();
    }
    let base = peak_reset();
    let t = Instant::now();
    let mut fs = UnaFS::mount(FileDevice::open(&img.0).unwrap()).unwrap();
    let mount_s = t.elapsed().as_secs_f64();
    let mount_peak = peak_since(base);
    let id = fs.resolve_path("/f").unwrap();
    fs.write_data(id, 0, &[9u8; 4096]).unwrap();
    let commit_blocks = fs.commit_stats().last_commit_blocks;
    let t = Instant::now();
    let rep = fs.fsck(false).unwrap();
    let fsck_s = t.elapsed().as_secs_f64();
    assert!(rep.is_clean());
    (mount_s, mount_peak, commit_blocks, fsck_s, fs.superblock.block_count)
}

#[test]
#[ignore]
fn bench_rows() {
    let sizes: Vec<u64> = std::env::var("UNAFS_BENCH_MIB")
        .unwrap_or_else(|_| "1024,4096,16384,65536".into())
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    println!("| volume | blocks | mount s | mount peak heap | 1-block write commit blocks | fsck s |");
    for mib in sizes {
        let (m, h, c, f, b) = row(mib);
        println!(
            "| {} MiB | {} | {:.3} | {} KiB | {} | {:.3} |",
            mib,
            b,
            m,
            h / 1024,
            c,
            f
        );
    }
}

/// UNAFSMAP M3: the kernel's shape — a 500 GB SSD root (the rMBP's p2) under
/// the default 4 MiB leaf budget the kernel mount uses, against the x86
/// kernel's 256 MiB heap. Mount, a 64 MiB file written in 1 MiB calls, 2,000
/// small files, a full fsck: the peak heap the crate takes stays small.
#[test]
fn kernel_budget_heap_on_a_500_gb_root() {
    const MIB: u64 = 500 * 1000 * 1000 * 1000 / (1024 * 1024);
    let img = sparse("map-bench-500g.img", MIB);
    drop(UnaFS::format(FileDevice::open(&img.0).unwrap(), MIB).unwrap());
    let base = peak_reset();
    let mut fs = UnaFS::mount(FileDevice::open(&img.0).unwrap()).unwrap();
    let mount_peak = peak_since(base);
    let root = fs.superblock.root_inode;
    let big = fs.create_file(root, "big".into()).unwrap();
    let chunk = vec![0x5au8; 1 << 20];
    for i in 0..64u64 {
        fs.write_data(big, i << 20, &chunk).unwrap();
    }
    let d = fs.mkdir(root, "many".into()).unwrap();
    let batch: Vec<unafs::BatchFile> = (0..2000)
        .map(|i| unafs::BatchFile {
            name: format!("f{i}"),
            data: vec![i as u8; 100],
            attributes: Default::default(),
        })
        .collect();
    fs.create_files_batch(d, batch).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    let peak = peak_since(base);
    let s = fs.map_stats();
    println!(
        "[map-bench] 500 GB root: blocks={} mount_peak={} KiB work_peak={} KiB pages_peak={} page_reads={} leaves={} leaf_blocks={}",
        fs.superblock.block_count,
        mount_peak / 1024,
        peak / 1024,
        s.pages_peak,
        s.page_reads,
        s.leaves,
        s.leaf_blocks
    );
    assert!(mount_peak < 8 << 20, "mount peak {mount_peak}");
    assert!(peak < 32 << 20, "work peak {peak} (kernel heap is 256 MiB)");
}

/// The proof bounds of the ledger row on a 1 TiB sparse image.
#[test]
fn one_tib_mount_under_one_second_and_eight_mib() {
    let (mount_s, peak, commit_blocks, _fsck_s, blocks) = row(1024 * 1024);
    assert_eq!(blocks, 1 << 28);
    assert!(mount_s < 1.0, "mount {mount_s} s");
    assert!(peak < 8 << 20, "mount peak {peak}");
    assert!(commit_blocks < 16, "commit {commit_blocks}");
}
