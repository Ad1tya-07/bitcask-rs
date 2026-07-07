// benches/sync_modes.rs
use storage_engine::{BitCask, SyncMode};
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_sync_modes(c: &mut Criterion) {
    // Benchmark 1: fsync on every write.
    c.bench_function("put_every_write", |b| {
        let path = std::env::temp_dir().join("bench_every.db");
        let _ = std::fs::remove_file(&path);
        let mut db = BitCask::open(&path, SyncMode::EveryWrite).unwrap();
        let mut i = 0u64;
        b.iter(|| {                                  // criterion runs THIS many times, timed
            db.put(&format!("key_{i}"), "value").unwrap();
            i += 1;
        });
    });

    // Benchmark 2: let the OS flush whenever.
    c.bench_function("put_never", |b| {
        let path = std::env::temp_dir().join("bench_never.db");
        let _ = std::fs::remove_file(&path);
        let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
        let mut i = 0u64;
        b.iter(|| {
            db.put(&format!("key_{i}"), "value").unwrap();
            i += 1;
        });
    });
}

criterion_group!(benches, bench_sync_modes);   // register the benchmark fn
criterion_main!(benches);                        // generate the runner's main()