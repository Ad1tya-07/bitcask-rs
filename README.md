## Background(about why and what this project is)
Reading through the book 'Designing Data Intensive Applications' chapter 3, I came across the Bitcask storage engine and how it stores value. I know the real design has a lot to it - compaction, creash safety, handling concurrent access, but...we all start somewhere, especially if you are just starting to learn Rust. So, this is a simple implementation of Bitcask style storage engine which basically appends the logs as key-value pairs.

## How it works
- **Append-only log:** every write is appended to a single data file as a
  length-prefixed binary record: `[key_len: u32][key][val_len: u32][val]`.
- **In-memory index:** on startup, the log is scanned front-to-back to rebuild an in-memory map of keys to their latest offsets. Later records for the same key overwrite earlier ones, so the index always reflects the most recent write.
- **Crash recovery:** because state is rebuilt by replaying the log, the store survives restarts — reopening the file and re-scanning reconstructs everything.

## Usage
Currently, just gotta go to the project directory and run it as cargo run. The functions that I am calling are hardcoded in the main function.

## Known limitations /  next steps
- **Compaction:** the log grows forever as keys are updated; dead records need
  periodic merging.
- **Buffered I/O:** reads and writes are currently unbuffered.
- **Error handling:** minimal; UTF-8 and I/O edge cases are not all handled gracefully.