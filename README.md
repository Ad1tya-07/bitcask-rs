# bitcask-rs

A crash-safe, append-only key-value storage engine in Rust, built on the
[Bitcask](https://riak.com/assets/bitcask-intro.pdf) model: an append-only data
file on disk plus an in-memory index (the *keydir*) mapping each key to the
offset of its most recent value.

Writes are sequential appends. Reads are a single hash-map lookup followed by
one seek-and-read. The engine recovers cleanly from crashes that tear a write
mid-record, and reclaims space from stale and deleted records via compaction.

## Usage

```rust
use bitcask_rs::{BitCask, SyncMode};

let mut db = BitCask::open("data/store.db", SyncMode::EveryWrite)?;

db.put("user_1", "Alice")?;
db.put("user_1", "Bob")?;        // overwrites; older version becomes garbage
assert_eq!(db.get("user_1"), "Bob");

db.delete("user_1")?;
assert_eq!(db.get("user_1"), "");

db.merge()?;                     // reclaim space from stale/deleted records
```

`open` creates the file (and parent directories) if absent and runs recovery
automatically, so a returned `BitCask` is always a consistent handle.

## On-disk format

Each record is a flat, length-prefixed frame:

```
[ crc: u32 ][ key_len: u32 ][ key ][ val_len: u32 ][ val ]
             └──────────── CRC covers these bytes ───────────┘
```

A **deletion** is a record whose `val_len` is a tombstone marker (`u32::MAX`)
and carries no value. All integers are little-endian.

The keydir stores, per live key, the byte offset of its `val_len` field — so a
read seeks straight to the value length and reads the value.

## Crash safety

Two failure modes are handled, and they are distinct:

**Torn writes (process/power death mid-append).** Because every write only ever
appends, a crash can damage only the *last* record — everything before it
already completed. On `open`, recovery scans the log from the start and, for
every record, recomputes the CRC over the bytes on disk and compares it to the
stored CRC. Recovery stops at the first record that is either:

- **short** — fewer bytes on disk than the record's length fields require (a
  write cut off mid-record), or
- **corrupt** — a complete record whose recomputed CRC doesn't match.

In both cases the file is **truncated back to the end of the last valid
record** and the scan ends. A half-written trailing record — even a
half-written CRC — is therefore dropped rather than mistaken for real data, and
the file is left ending on a clean record boundary. A clean end-of-log
truncates to its own length (a no-op).

The CRC is validated *before* any field is trusted, so corrupt bytes can never
be indexed or decoded as a value.

**Compaction (`merge`).** Append-only means the file only grows: every
overwrite leaves a dead older version, every delete adds a tombstone. `merge`
rewrites only the live records into a fresh sibling file and swaps it in with a
single atomic `rename`. The original file is never mutated in place, so a crash
at any point during a merge leaves either the untouched original or the
complete new file — never a partial state.

## The durability tradeoff (`SyncMode`)

The one knob that matters. After each write, does the engine force the data to
physical disk (`fsync`), or leave it in the OS page cache for the OS to flush on
its own schedule?

- `SyncMode::EveryWrite` — `fsync` after every write. No acknowledged write is
  ever lost, even on power failure. Maximum durability.
- `SyncMode::Never` — rely on the OS to flush. A process kill is still safe (the
  OS flushes the cache afterward), but a **power loss** can lose the last writes
  that were acknowledged but not yet flushed. Maximum throughput.

Measured with `criterion` (single-write latency, 100 samples each):

| Mode                   | Time per write | Throughput        |
|------------------------|----------------|-------------------|
| `SyncMode::EveryWrite` | ~3.0 ms        | ~330 writes/sec   |
| `SyncMode::Never`      | ~9.6 µs        | ~104,000 writes/sec |

**`fsync`-per-write is ~310× slower on this hardware.** That cost buys a hard
durability guarantee: the physical disk seek/flush on every write is the price
of never losing an acknowledged write to power loss. `Never` is ~310× faster
because writes only reach RAM before returning.

Neither is "correct" — it's a workload decision. A ledger or anything where a
lost acknowledged write is unacceptable wants `EveryWrite`. A cache, a metrics
sink, or a bulk load that can tolerate losing the last few writes on a hard
crash wants `Never` (or, in a fuller engine, group commit — batching one
`fsync` across many writes to recover most of the throughput while bounding the
loss window).

*(Numbers are from one machine; the absolute times and the ratio depend heavily
on the underlying disk — an SSD narrows the gap, a spinning disk widens it.
Re-run `cargo bench` to measure your own.)*

## Building and testing

```bash
cargo build
cargo test          # unit + integration tests, including crash-recovery tests
cargo bench         # the SyncMode throughput comparison
```

The integration tests under `tests/` cover the recovery paths directly:
put/get roundtrip, restart recovery, last-write-wins, torn-tail truncation,
half-written-CRC truncation, mid-record corruption detection, and merge
(file shrinks, newest value wins, tombstoned keys vanish — all surviving a
reopen).

## Scope and non-goals

Deliberately kept to a single-node core. Not implemented, by design:

- **Single active data file.** No multi-file segments or hint files.
- **Manual compaction.** `merge` is called explicitly — no background thread or
  automatic size/garbage-ratio trigger.
- **No directory `fsync` after the merge `rename`.** The record-level durability
  above is honored; making the *swap itself* durable across power loss would
  additionally require syncing the parent directory.
- **No length sanity-check on recovery.** A corrupt length field is caught by
  the CRC, but is read into a buffer of the claimed size first.

These are the natural next steps toward a production engine, not gaps in the
crash-safety model, which is complete for the single-file case.