# Milestone 1.6.1 — Temporal Storage / Segmented WAL Hardening

This patch preserves Milestone 1.6 scope: persistent Current Store, persistent Version Store,
temporal B+Tree, segmented WAL and checkpoint v2.

Implemented hardening:
- bounded 16 MiB WAL payloads before allocation/append,
- incomplete newest-segment crash-tail truncation before new appends,
- corruption rejection for truncated non-tail segments,
- WAL segment-gap detection,
- checked packed-LSN range and segment-size validation,
- fsync of directory metadata after segment create/delete,
- logical WAL transaction-state validation during recovery,
- hardened page format marker + CRC and structural heap-slot validation,
- legacy 1.5/1.6 page compatibility with upgrade-on-write,
- partial page-file suffix normalization,
- checked page offsets and no poisoned `std::sync::Mutex<File>`,
- panic-free bounded Current and Version B+Tree codecs,
- key-order and node-shape validation,
- root-page range validation,
- traversal/depth guards against corrupted B+Tree cycles,
- CRC/version/length framed B+Tree root metadata with legacy bare-bincode read,
- CRC/version/length framed checkpoint v2 with legacy bare-bincode read,
- fsync-before-rename and directory fsync for root/checkpoint publication.

No 1.7 execution/FFI functionality is introduced here.
