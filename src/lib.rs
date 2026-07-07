use std::io::{Read, Write, Seek, SeekFrom, ErrorKind};
use std::fs;
use std::fs::{OpenOptions, File};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const TOMBSTONE_MARKER: u32 = u32::MAX;

// Durability policy. EveryWrite = fsync after each write (your current behavior,
// safest, slowest). Never = let the OS flush whenever (fastest, least safe).
#[derive(Clone, Copy)]
pub enum SyncMode {
    EveryWrite,
    Never,
}

// The database handle. Holds the three things that always travel together:
// the data file, the in-memory index, and the durability policy.
pub struct BitCask {
    file: File,
    keydir: HashMap<String, u64>,
    sync_mode: SyncMode,
    path: PathBuf,  
}

impl BitCask {
    // Open (or create) a database at `path`, running crash recovery automatically.
    pub fn open(path: impl AsRef<Path>, sync_mode: SyncMode) -> std::io::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().read(true).write(true).create(true).open(path)?;

        // Rebuild the index from disk. This is recovery — it also truncates any
        // torn/corrupt tail. Runs on every open.
        let mut keydir = HashMap::new();
        fill_map(&mut file, &mut keydir)?;

        Ok(BitCask { file, keydir, sync_mode, path: path.to_path_buf() })
    }

    pub fn put(&mut self, key: &str, val: &str) -> std::io::Result<()> {
    self.file.seek(SeekFrom::End(0))?;
    let val_len_pos = write_record(&mut self.file, key, val)?;
    if let SyncMode::EveryWrite = self.sync_mode {
        self.file.sync_all()?;
    }
    self.keydir.insert(key.to_string(), val_len_pos);
    Ok(())
    }

    pub fn get(&mut self, key: &str) -> String {
        // Copy the offset out so we stop borrowing keydir before touching file.
        let offset = match self.keydir.get(key) {
            Some(offset) => *offset,
            None => return "".to_string(),
        };
        if self.file.seek(SeekFrom::Start(offset)).is_err() {
            return "".to_string();
        }
        let mut u32_buf = [0u8; 4];
        match self.file.read_exact(&mut u32_buf) {
            Ok(()) => {
                let val_len = u32::from_le_bytes(u32_buf) as usize;
                let mut val_buf = vec![0u8; val_len];
                match self.file.read_exact(&mut val_buf) {
                    Ok(()) => String::from_utf8(val_buf).unwrap(),
                    Err(_) => "".to_string(),
                }
            }
            Err(_) => "".to_string(),
        }
    }

    pub fn delete(&mut self, key: &str) -> std::io::Result<()> {
        self.file.seek(SeekFrom::End(0))?;
        let key_len = key.len() as u32;

        let mut body = Vec::with_capacity(8);
        body.extend_from_slice(&key_len.to_le_bytes());
        body.extend_from_slice(key.as_bytes());
        body.extend_from_slice(&TOMBSTONE_MARKER.to_le_bytes());

        let crc = crc32fast::hash(&body);
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&key_len.to_le_bytes())?;
        self.file.write_all(key.as_bytes())?;
        self.file.write_all(&TOMBSTONE_MARKER.to_le_bytes())?;

        if let SyncMode::EveryWrite = self.sync_mode {
            self.file.sync_all()?;
        }
        self.keydir.remove(key);
        Ok(())
    }
    pub fn merge(&mut self) -> std::io::Result<()> {
    // A sibling temp file in the SAME directory (same filesystem -> atomic rename).
    let merge_path = self.path.with_extension("merge");

    // Fresh, empty merge file. truncate(true) clears any leftover from a
    // previous crashed merge.
    let mut new_file = OpenOptions::new()
        .read(true).write(true).create(true).truncate(true)
        .open(&merge_path)?;

    // Snapshot the live keys, then write ONLY their newest values into the new
    // file. Dead versions and tombstones simply never get copied over.
    let keys: Vec<String> = self.keydir.keys().cloned().collect();
    let mut new_keydir = HashMap::new();
    for key in keys {
        let val = self.get(&key);                          // newest value, read from OLD file
        let val_len_pos = write_record(&mut new_file, &key, &val)?;
        new_keydir.insert(key, val_len_pos);
    }

    new_file.sync_all()?;                                  // merged data durable BEFORE the swap

    // The atomic swap: after this line, self.path points at the NEW file.
    // A crash before it leaves the original untouched; a crash after it leaves
    // the new one complete. Never a half-state.
    fs::rename(&merge_path, &self.path)?;

    // Point the struct at the new file + new index. Dropping the old File here
    // closes the now-unlinked original.
    new_file.seek(SeekFrom::End(0))?;
    self.file = new_file;
    self.keydir = new_keydir;
    Ok(())
}
}

// ---- private free functions: recovery internals, unchanged from your version ----

// Reads exactly buf.len() bytes. Ok(false) = read fine; Ok(true) = short read
// (torn tail), file truncated to record_start, caller should stop; Err = real I/O error.
fn read_or_truncate(file: &mut File, buf: &mut [u8], record_start: u64) -> std::io::Result<bool> {
    match file.read_exact(buf) {
        Ok(()) => Ok(false),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
            file.set_len(record_start)?;   // <-- RESTORED (was commented out)
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

fn fill_map(file: &mut File, map: &mut HashMap<String, u64>) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;

    loop {
        let record_start = file.stream_position()?;
        let mut body: Vec<u8> = Vec::new();
        let mut u32_buf = [0u8; 4];

        // CRC (short read here = clean end OR half-written CRC; both truncate + stop)
        if read_or_truncate(file, &mut u32_buf, record_start)? { break; }
        let crc_stored = u32::from_le_bytes(u32_buf);

        // key_len
        if read_or_truncate(file, &mut u32_buf, record_start)? { break; }
        body.extend_from_slice(&u32_buf);
        let key_len = u32::from_le_bytes(u32_buf) as usize;

        // key
        let mut key_buffer = vec![0u8; key_len];
        if read_or_truncate(file, &mut key_buffer, record_start)? { break; }
        body.extend_from_slice(&key_buffer);

        let seek_offset = file.stream_position()?;

        // val_len
        if read_or_truncate(file, &mut u32_buf, record_start)? { break; }
        body.extend_from_slice(&u32_buf);
        let val_len = u32::from_le_bytes(u32_buf);

        // value (skipped for tombstone)
        let is_tombstone = val_len == TOMBSTONE_MARKER;
        if !is_tombstone {
            let mut val_buffer = vec![0u8; val_len as usize];
            if read_or_truncate(file, &mut val_buffer, record_start)? { break; }
            body.extend_from_slice(&val_buffer);
        }

        // integrity gate
        if crc32fast::hash(&body) != crc_stored {
            file.set_len(record_start)?;
            break;
        }

        let key = String::from_utf8(key_buffer).unwrap();
        if is_tombstone {
            map.remove(&key);
        } else {
            map.insert(key, seek_offset);
        }
    }

    file.seek(SeekFrom::End(0))?;
    Ok(())
}
// Writes one record at the file's current position. Returns the offset of the
// val_len field (what the keydir stores, what get() seeks to). Assumes the
// cursor is already where you want to write.
fn write_record(file: &mut File, key: &str, val: &str) -> std::io::Result<u64> {
    let key_len = key.len() as u32;
    let val_len = val.len() as u32;

    let mut body = Vec::with_capacity(8 + key.len() + val.len());
    body.extend_from_slice(&key_len.to_le_bytes());
    body.extend_from_slice(key.as_bytes());
    body.extend_from_slice(&val_len.to_le_bytes());
    body.extend_from_slice(val.as_bytes());
    let crc = crc32fast::hash(&body);

    file.write_all(&crc.to_le_bytes())?;
    file.write_all(&key_len.to_le_bytes())?;
    file.write_all(key.as_bytes())?;
    let val_len_pos = file.stream_position()?;
    file.write_all(&val_len.to_le_bytes())?;
    file.write_all(val.as_bytes())?;
    Ok(val_len_pos)
}