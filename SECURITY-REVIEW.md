# Milestone 1.5.1 — Security / Durability Review

## Inherited WAL hardening

1.5.1 includes the complete 1.0.1 WAL hardening:
- incomplete crash-tail truncation before append,
- 16 MiB per-record payload bound,
- checked frame length conversion,
- framed corruption remains a hard error.

## Hardened page/storage issues

### HIGH — corrupted B+Tree counts could panic the process
The original codec trusted persisted leaf/internal counts and sliced payloads with `unwrap`.
1.5.1 validates:
- maximum entry/key counts,
- required payload byte size,
- leaf key/value shape,
- internal `children = keys + 1`,
- strict key ordering,
- every primitive read/write range.

Corruption now produces `BTreeError::Corrupt` rather than a data-dependent slice panic.

### HIGH — pages had no checksum
New/rewritten pages use page format v1 and CRC32 over the complete 16 KiB page.

Legacy Milestone 1.5 pages (format marker 0) remain readable after structural validation so the
hardening patch can open existing data. They are automatically upgraded to checksummed v1 on write.
This means a never-rewritten legacy page retains the original 1.5 corruption-detection limitation.

### HIGH — common page header/slot metadata was insufficiently validated
1.5.1 validates:
- page magic/kind/version,
- `PAGE_HEADER_SIZE <= free_start <= free_end <= PAGE_SIZE`,
- heap slot-directory size,
- every slot tuple range,
- overflow-safe range arithmetic.

### HIGH — partial page-file crash suffix
A page file whose length is not a multiple of 16 KiB is normalized by truncating only the incomplete
last page before allocation/read use. Checked offset arithmetic prevents wraparound.

### MEDIUM — poisoned `std::sync::Mutex<File>` panic path
The file page store now uses `parking_lot::Mutex`, eliminating `.lock().unwrap()` poisoning failures.

### MEDIUM — panic could leak manual BufferPool pins
Read/write callbacks execute while the BufferPool state mutex is held, so manual `pins += 1/-=1`
was unnecessary and panic-sensitive. 1.5.1 removes that counter from the synchronous pool.

### HIGH — root/checkpoint metadata replacement lacked durability framing
B+Tree root metadata and checkpoint metadata now use:
- magic,
- version,
- payload length bound,
- CRC32,
- temporary file,
- `sync_all`,
- atomic rename,
- parent-directory `sync_all`.

Original 1.5 bare-bincode root/checkpoint files remain readable and are upgraded on the next save.

## Residual risks intentionally not solved in 1.5.1

- Root/checkpoint metadata has one active file, not the later dual-slot fallback design.
  A corrupt latest metadata file therefore fails open instead of automatically selecting a previous generation.
- Current heap is append-oriented and does not reclaim obsolete records; update-heavy workloads cause file growth.
- There is no page free list or general compaction.
- Historical Version Store is not yet independently persistent; Milestone 1.6 addresses that layer.
- WAL remains a single file and whole-WAL recovery remains memory proportional to WAL size.
- B+Tree uses a coarse tree mutex; this is correctness-first, not high-concurrency production design.
- CRC32 detects accidental corruption but is not a cryptographic integrity/MAC mechanism.

The patch deliberately does not import Milestone 1.6+ features into this historical milestone.
