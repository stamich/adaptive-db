//! Data model of the change log and its binary encoding.
//!
//! The encoding is `bincode` with fixed-width little-endian integers. [`ChangeEnvelope`] declares
//! `commit_seq` as its first field, so the first eight bytes of an encoded envelope are the
//! sequence number: a reader can order or skip frames without decoding the rest.
use adb_core::{CommitSeq, CommitTs, Row, RowId, SchemaEpoch, SchemaVersion, TxId};
use bincode::Options;
use serde::{Deserialize, Serialize};

use crate::error::ChangeLogError;

/// Upper bound of one encoded envelope (64 MiB). A transaction larger than this cannot be
/// represented and the commit that produced it is rejected before it is published.
pub const MAX_ENVELOPE_BYTES: u64 = 64 * 1024 * 1024;

/// The effect of one commit on one row, with both images.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowChange {
    /// Storage key of the row (`entity << 64 | primary key`).
    pub entity_id: u64,
    /// Row the change applies to.
    pub row_id: RowId,
    /// Schema version of `before`, `None` when the row did not exist.
    pub before_schema_version: Option<SchemaVersion>,
    /// Schema version of `after`, `None` when the row was deleted.
    pub after_schema_version: Option<SchemaVersion>,
    /// Image before the commit; `None` for an insert.
    pub before: Option<Row>,
    /// Image after the commit; `None` for a delete.
    pub after: Option<Row>,
}

/// A catalog change carried by a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaChangeMarker {
    /// A schema version of `entity_id` was published (`CREATE`/`ALTER TABLE`).
    Schema {
        /// Entity whose schema changed.
        entity_id: u64,
        /// Newly published version.
        schema_version: SchemaVersion,
    },
    /// `entity_id` was dropped.
    DropEntity {
        /// Dropped entity.
        entity_id: u64,
    },
    /// The catalog epoch moved without a schema change (index DDL).
    Epoch,
}

/// What a commit changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChangePayload {
    /// A data transaction.
    Data {
        /// Transaction that produced the commit.
        tx_id: TxId,
        /// Row changes in row order.
        rows: Vec<RowChange>,
    },
    /// A DDL transaction.
    Schema {
        /// Transaction that produced the commit.
        tx_id: TxId,
        /// Catalog changes in publication order.
        changes: Vec<SchemaChangeMarker>,
    },
}

/// One commit in the change log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeEnvelope {
    /// Global logical order of the commit (data and catalog commits share it). Declared first
    /// on purpose: see the module documentation.
    pub commit_seq: CommitSeq,
    /// MVCC commit timestamp.
    pub commit_ts: CommitTs,
    /// Catalog epoch in force after the commit.
    pub schema_epoch: SchemaEpoch,
    /// The changes.
    pub payload: ChangePayload,
}

impl ChangeEnvelope {
    /// Number of row changes the commit carries (0 for a DDL commit).
    pub fn change_count(&self) -> usize {
        match &self.payload {
            ChangePayload::Data { rows, .. } => rows.len(),
            ChangePayload::Schema { changes, .. } => changes.len(),
        }
    }
}

/// The bincode configuration shared by encoder and decoder: fixed-width little-endian integers,
/// trailing bytes rejected and an allocation limit so corrupt lengths cannot exhaust memory.
fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
        .with_limit(MAX_ENVELOPE_BYTES)
}

/// Encodes `envelope`; fails with [`ChangeLogError::TooLarge`] beyond [`MAX_ENVELOPE_BYTES`].
pub fn encode_envelope(envelope: &ChangeEnvelope) -> Result<Vec<u8>, ChangeLogError> {
    options().serialize(envelope).map_err(|error| match *error {
        bincode::ErrorKind::SizeLimit => ChangeLogError::TooLarge {
            size: MAX_ENVELOPE_BYTES + 1,
            limit: MAX_ENVELOPE_BYTES,
        },
        other => ChangeLogError::Malformed(other.to_string()),
    })
}

/// Decodes bytes produced by [`encode_envelope`]; any damage yields an error, never a panic.
pub fn decode_envelope(bytes: &[u8]) -> Result<ChangeEnvelope, ChangeLogError> {
    if bytes.len() as u64 > MAX_ENVELOPE_BYTES {
        return Err(ChangeLogError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_ENVELOPE_BYTES,
        });
    }
    options()
        .deserialize(bytes)
        .map_err(|error| ChangeLogError::Malformed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adb_core::Value;

    /// A data envelope with one update.
    fn sample() -> ChangeEnvelope {
        ChangeEnvelope {
            commit_seq: CommitSeq(7),
            commit_ts: CommitTs(7),
            schema_epoch: SchemaEpoch(2),
            payload: ChangePayload::Data {
                tx_id: TxId(9),
                rows: vec![RowChange {
                    entity_id: 1,
                    row_id: RowId(5),
                    before_schema_version: Some(SchemaVersion(1)),
                    after_schema_version: Some(SchemaVersion(1)),
                    before: Some(Row::new().with_field(1, Value::Int64(1))),
                    after: Some(Row::new().with_field(1, Value::Int64(2))),
                }],
            },
        }
    }

    /// Encoding then decoding returns the same envelope, for data and schema payloads.
    #[test]
    fn round_trip() {
        let data = sample();
        assert_eq!(
            decode_envelope(&encode_envelope(&data).unwrap()).unwrap(),
            data
        );
        let ddl = ChangeEnvelope {
            commit_seq: CommitSeq(8),
            commit_ts: CommitTs(8),
            schema_epoch: SchemaEpoch(3),
            payload: ChangePayload::Schema {
                tx_id: TxId(10),
                changes: vec![
                    SchemaChangeMarker::Schema {
                        entity_id: 1,
                        schema_version: SchemaVersion(2),
                    },
                    SchemaChangeMarker::DropEntity { entity_id: 2 },
                    SchemaChangeMarker::Epoch,
                ],
            },
        };
        assert_eq!(
            decode_envelope(&encode_envelope(&ddl).unwrap()).unwrap(),
            ddl
        );
        assert_eq!(ddl.change_count(), 3);
    }

    /// The first eight bytes are the little-endian sequence number.
    #[test]
    fn sequence_number_prefixes_the_encoding() {
        let bytes = encode_envelope(&sample()).unwrap();
        assert_eq!(&bytes[..8], &7u64.to_le_bytes());
    }

    /// Truncation, trailing bytes and bit flips are errors or decode to something else, never
    /// a panic; a hostile length prefix is refused instead of allocated.
    #[test]
    fn damaged_bytes_are_rejected_without_panic() {
        let bytes = encode_envelope(&sample()).unwrap();
        for cut in 0..bytes.len() {
            assert!(decode_envelope(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_envelope(&trailing).is_err());
        for at in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[at] ^= 0xff;
            let _ = decode_envelope(&damaged);
        }
        let mut hostile = bytes;
        // The payload variant tag is followed by tx_id and the row-vector length.
        let length_at = 8 + 8 + 8 + 4 + 8;
        hostile[length_at..length_at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode_envelope(&hostile).is_err());
    }
}
