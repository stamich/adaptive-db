//! Deterministic fuzzing of stored index entries: a damaged page can hand the decoder any
//! bytes, which must produce a corruption error, never a panic.
use adb_core::{CommitTs, Direction, IndexId, RowId, Value};
use adb_index::{
    entry::{decode_entry, encode_included, encode_index_key, tree_key, tree_value},
    IndexColumn, IndexDefinition, IndexEntry, IndexSpec, IndexState, OPEN,
};

/// Deterministic pseudo-random words (xorshift64).
struct Words(u64);

impl Words {
    /// Next word.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Up to `max` random bytes.
    fn bytes(&mut self, max: u64) -> Vec<u8> {
        let len = (self.next() % (max + 1)) as usize;
        (0..len).map(|_| self.next() as u8).collect()
    }
}

/// `(region DESC, owner)` INCLUDE `(balance)`.
fn definition() -> IndexDefinition {
    IndexDefinition::new(
        IndexId(1),
        IndexSpec {
            name: "idx".into(),
            entity_id: 3,
            columns: vec![
                IndexColumn {
                    field_id: 1,
                    direction: Direction::Desc,
                },
                IndexColumn {
                    field_id: 2,
                    direction: Direction::Asc,
                },
            ],
            include: vec![3],
        },
        IndexState::Active,
        CommitTs(1),
    )
}

/// Random keys and values, and every truncation and bit flip of a valid entry, are rejected
/// or decoded without panicking; the valid entry itself round-trips.
#[test]
fn damaged_entries_never_panic() {
    let definition = definition();
    let entry = IndexEntry {
        key: vec![Value::Int64(-7), Value::String("ow\u{0}ner".into())],
        row_id: RowId::from_primary_key(3, 11),
        begin_ts: CommitTs(5),
        end_ts: OPEN,
        included: vec![Value::Float64(2.5)],
    };
    let key = tree_key(
        &encode_index_key(&definition, &entry.key).unwrap(),
        entry.row_id,
        entry.begin_ts,
    );
    let value = tree_value(
        entry.end_ts,
        &encode_included(&definition, &entry.included).unwrap(),
    );
    assert_eq!(decode_entry(&definition, &key, &value).unwrap(), entry);
    for cut in 0..key.len() {
        let _ = decode_entry(&definition, &key[..cut], &value);
    }
    for cut in 0..value.len() {
        assert!(decode_entry(&definition, &key, &value[..cut]).is_err());
    }
    let mut words = Words(0xDEAD_BEEF);
    for _ in 0..20_000 {
        let mut k = key.clone();
        let mut v = value.clone();
        let at = (words.next() as usize) % k.len();
        k[at] ^= 1 << (words.next() % 8);
        let at = (words.next() as usize) % v.len();
        v[at] ^= 1 << (words.next() % 8);
        let _ = decode_entry(&definition, &k, &value);
        let _ = decode_entry(&definition, &key, &v);
        let _ = decode_entry(&definition, &words.bytes(64), &words.bytes(32));
    }
}
