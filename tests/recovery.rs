// tests/recovery.rs
//
// Integration tests: this is a SEPARATE crate that imports bitcask_rs from
// outside, so it can only touch the public API (BitCask, SyncMode, and their
// pub methods). Corruption is injected by reopening the file's PATH as a raw
// File and writing bytes directly -- exactly how real corruption happens:
// from outside the database process.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::PathBuf;
use storage_engine::{BitCask, SyncMode};   // <-- match your Cargo.toml package name (underscores)

// Clean, unique path per test so parallel tests never collide.
fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("bitcask_test_{name}.db"));
    let _ = fs::remove_file(&path);   // wipe any leftover from a previous run
    path
}

// ---------- behavioral tests: pure public API ----------

#[test]
fn put_get_roundtrip() {
    let path = temp_path("roundtrip");
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();

    db.put("a", "apple").unwrap();
    db.put("b", "banana").unwrap();

    assert_eq!(db.get("a"), "apple");
    assert_eq!(db.get("b"), "banana");
    assert_eq!(db.get("missing"), "");   // absent key -> empty string
}

#[test]
fn recovery_after_restart() {
    let path = temp_path("restart");

    // First "session": write some data, then close by dropping the handle.
    {
        let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
        db.put("a", "apple").unwrap();
        db.put("b", "banana").unwrap();
        db.delete("a").unwrap();
    } // db dropped here = clean shutdown

    // Second "session": reopening rebuilds the index from disk (that's recovery).
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
    assert_eq!(db.get("b"), "banana");   // survived restart
    assert_eq!(db.get("a"), "");          // delete survived too
}

#[test]
fn last_write_wins() {
    let path = temp_path("overwrite");
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();

    db.put("k", "first").unwrap();
    db.put("k", "second").unwrap();
    assert_eq!(db.get("k"), "second");   // newer append shadows the older one

    // ...and the newer value must win after recovery too (higher offset = newer).
    drop(db);
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
    assert_eq!(db.get("k"), "second");
}

// ---------- corruption tests: API write -> raw byte surgery -> API recovery ----------

#[test]
fn torn_tail_is_truncated() {
    let path = temp_path("torn");

    // Phase 1: good records through the API, then close.
    {
        let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
        db.put("a", "apple").unwrap();
        db.put("b", "banana").unwrap();
    }
    let good_len = fs::metadata(&path).unwrap().len();   // file length with only good records

    // Phase 2: raw handle on the same path -> append a TORN record.
    // We write a CRC + a key_len claiming 5 bytes, but only 2 bytes of key.
    // That's a crash mid-key: read_exact for the key will come up short.
    {
        let mut raw = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        raw.seek(SeekFrom::End(0)).unwrap();
        raw.write_all(&123u32.to_le_bytes()).unwrap();   // some CRC value
        raw.write_all(&5u32.to_le_bytes()).unwrap();     // key_len = 5
        raw.write_all(b"ke").unwrap();                    // ...but only 2 bytes follow
    }

    // Phase 3: reopen -> recovery must truncate the torn tail and keep good records.
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), good_len);  // torn tail chopped off
    assert_eq!(db.get("a"), "apple");
    assert_eq!(db.get("b"), "banana");
}

#[test]
fn half_written_crc_is_truncated() {
    let path = temp_path("half_crc");

    {
        let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
        db.put("a", "apple").unwrap();
    }
    let good_len = fs::metadata(&path).unwrap().len();

    // Append only 2 of the 4 CRC bytes: a crash mid-CRC. This is the case that
    // must NOT be mistaken for a clean end-of-log -- it has to be chopped.
    {
        let mut raw = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        raw.seek(SeekFrom::End(0)).unwrap();
        raw.write_all(&[0xAB, 0xCD]).unwrap();
    }

    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), good_len);   // stray 2 bytes gone
    assert_eq!(db.get("a"), "apple");
}

#[test]
fn corrupt_record_is_dropped() {
    let path = temp_path("corrupt");

    // Record "a" first, then note where "b" begins, then write "b".
    let boundary;
    {
        let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
        db.put("a", "apple").unwrap();
        boundary = fs::metadata(&path).unwrap().len();   // offset where record "b" starts
        db.put("b", "banana").unwrap();
    }

    // Flip a byte INSIDE record b's value, leaving all lengths valid. Every
    // read_exact still succeeds, so only the CRC check can catch this. Record
    // layout: [crc 4][key_len 4][key 1 = "b"][val_len 4][val...]. The first
    // value byte sits at boundary + 4 + 4 + 1 + 4 = boundary + 13.
    {
        let mut raw = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        raw.seek(SeekFrom::Start(boundary + 13)).unwrap();
        let mut one = [0u8; 1];
        raw.read_exact(&mut one).unwrap();
        one[0] ^= 0xFF;                                   // flip all bits of that byte
        raw.seek(SeekFrom::Start(boundary + 13)).unwrap();
        raw.write_all(&one).unwrap();
    }

    // Reopen: CRC mismatch on "b" -> it's dropped and the file truncated back
    // to the boundary. "a" (written before the corruption) survives.
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
    assert_eq!(db.get("a"), "apple");                          // intact
    assert_eq!(db.get("b"), "");                                // corrupt record gone
    assert_eq!(fs::metadata(&path).unwrap().len(), boundary);  // truncated to last good boundary
}

#[test]
fn merge_shrinks_and_keeps_newest() {
    let path = temp_path("merge");
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();

    for i in 0..100 {                              // 100 versions -> 99 dead records
        db.put("k", &format!("value_{i}")).unwrap();
    }
    db.put("gone", "temp").unwrap();
    db.delete("gone").unwrap();                    // a key that should vanish entirely

    let before = fs::metadata(&path).unwrap().len();
    db.merge().unwrap();
    let after = fs::metadata(&path).unwrap().len();

    assert!(after < before, "file should shrink: {after} !< {before}");
    assert_eq!(db.get("k"), "value_99");           // newest wins
    assert_eq!(db.get("gone"), "");                 // tombstoned key gone

    drop(db);                                       // and all of that survives reopening
    let mut db = BitCask::open(&path, SyncMode::Never).unwrap();
    assert_eq!(db.get("k"), "value_99");
    assert_eq!(db.get("gone"), "");
}