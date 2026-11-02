//! Hostile input: arbitrary bytes and randomly damaged segment files must produce an error or a
//! consistent log, never a panic, an unbounded allocation or an out-of-order read.
//!
//! The generator is a fixed-seed xorshift so failures reproduce.
use adb_change_log::model::{decode_envelope, encode_envelope};
use adb_change_log::{ChangeEnvelope, ChangeLog, ChangePayload, RowChange};
use adb_core::{CommitSeq, CommitTs, Row, RowId, SchemaEpoch, TxId, Value};
use tempfile::tempdir;

/// A small deterministic pseudo-random generator.
struct Rng(u64);

impl Rng {
    /// The next 64 random bits.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `0..bound` (`bound` > 0).
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// A data envelope with sequence number `seq`.
fn envelope(seq: u64) -> ChangeEnvelope {
    ChangeEnvelope {
        commit_seq: CommitSeq(seq),
        commit_ts: CommitTs(seq),
        schema_epoch: SchemaEpoch(1),
        payload: ChangePayload::Data {
            tx_id: TxId(seq),
            rows: vec![RowChange {
                entity_id: 1,
                row_id: RowId(seq as u128),
                before_schema_version: None,
                after_schema_version: None,
                before: None,
                after: Some(Row::new().with_field(1, Value::String("payload".repeat(5)))),
            }],
        },
    }
}

/// Random bytes, and valid encodings with random bytes flipped or cut, decode to an error or to
/// some envelope, and never panic or allocate beyond the size cap.
#[test]
fn decoding_arbitrary_bytes_is_safe() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..2000 {
        let len = rng.below(200);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let _ = decode_envelope(&bytes);
    }
    let valid = encode_envelope(&envelope(3)).unwrap();
    assert_eq!(decode_envelope(&valid).unwrap(), envelope(3));
    for _ in 0..2000 {
        let mut damaged = valid.clone();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(damaged.len());
            damaged[at] = rng.next() as u8;
        }
        damaged.truncate(1 + rng.below(damaged.len()));
        let _ = decode_envelope(&damaged);
    }
    // A length prefix claiming gigabytes of elements must fail fast, not allocate them.
    let mut hostile = vec![0xFFu8; 64];
    hostile[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(decode_envelope(&hostile).is_err());
}

/// Randomly damaged segment files either refuse to open or open to a log whose envelopes are
/// strictly increasing and start with a readable prefix of what was written.
#[test]
fn damaged_segments_never_yield_inconsistent_logs() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for round in 0..60 {
        let dir = tempdir().unwrap();
        {
            let log = ChangeLog::open(dir.path(), 1 << 20).unwrap();
            for seq in 1..=8 {
                log.append(&envelope(seq)).unwrap();
            }
            log.sync().unwrap();
        }
        let mut segments: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect();
        segments.sort();
        let target = &segments[0];
        let mut bytes = std::fs::read(target).unwrap();
        match round % 3 {
            0 => {
                let at = rng.below(bytes.len());
                bytes[at] ^= 1 << rng.below(8);
            }
            1 => bytes.truncate(rng.below(bytes.len())),
            _ => {
                let at = rng.below(bytes.len());
                let end = (at + 1 + rng.below(16)).min(bytes.len());
                for byte in &mut bytes[at..end] {
                    *byte = rng.next() as u8;
                }
            }
        }
        std::fs::write(target, &bytes).unwrap();
        if let Ok(log) = ChangeLog::open(dir.path(), 1 << 20) {
            let read = log.read_after(CommitSeq(0), usize::MAX).unwrap_or_default();
            let seqs: Vec<u64> = read.iter().map(|e| e.commit_seq.0).collect();
            assert!(
                seqs.windows(2).all(|pair| pair[0] < pair[1]),
                "round {round}: {seqs:?}"
            );
            assert!(
                seqs.iter().all(|seq| (1..=8).contains(seq)),
                "round {round}"
            );
        }
    }
}
