//! A committed transaction and its representation in the canonical log.
//!
//! The commit path turns a [`CommittedTx`] into log records ([`CommittedTx::to_records`]);
//! recovery and change-data capture turn log records back into [`CommittedTx`] values
//! ([`TxAssembler`]). Both directions live here so the log format has exactly one owner.

use adb_core::{CommitSeq, CommitTs, Lsn, RowId, SchemaEpoch, TxId};
use adb_storage::HistoricalVersion;
use adb_tx::Mutation;
use adb_wal::{CatalogEvent, LogEntry, WalError, WalRecord};

/// Everything a commit changes, in log order.
#[derive(Debug, Clone, PartialEq)]
pub struct CommittedTx {
    /// Transaction id.
    pub tx_id: TxId,
    /// Snapshot the transaction read at.
    pub snapshot_ts: CommitTs,
    /// Commit timestamp.
    pub commit_ts: CommitTs,
    /// Position in the global commit order (logs written before sequence numbers existed are
    /// numbered by commit timestamp, see [`TxAssembler`]).
    pub commit_seq: CommitSeq,
    /// Catalog epoch in force after the commit (the epoch of its last catalog change for a
    /// DDL transaction, the current epoch for a data transaction).
    pub schema_epoch: SchemaEpoch,
    /// Before-images of overwritten rows (closed at `commit_ts`).
    pub before_images: Vec<(RowId, HistoricalVersion)>,
    /// New row states in row order.
    pub mutations: Vec<(RowId, Mutation)>,
    /// Catalog changes, each with the epoch it moves the catalog to (DDL transactions only;
    /// data transactions carry none). Applied before the row changes of the same transaction.
    pub catalog: Vec<(SchemaEpoch, CatalogEvent)>,
}

impl CommittedTx {
    /// The contiguous run of log records describing this commit.
    pub fn to_records(&self) -> Vec<WalRecord> {
        let tx_id = self.tx_id;
        let mut records = Vec::with_capacity(
            self.catalog.len() + self.before_images.len() + self.mutations.len() + 2,
        );
        records.push(WalRecord::Begin {
            tx_id,
            snapshot_ts: self.snapshot_ts,
        });
        for (epoch, event) in &self.catalog {
            records.push(WalRecord::Catalog {
                tx_id,
                epoch: *epoch,
                event: event.clone(),
            });
        }
        for (row_id, version) in &self.before_images {
            records.push(WalRecord::Version {
                tx_id,
                row_id: *row_id,
                begin_ts: version.begin_ts,
                end_ts: version.end_ts,
                value: version.value.clone(),
            });
        }
        for (row_id, mutation) in &self.mutations {
            records.push(match mutation {
                Mutation::Put(row) => WalRecord::Put {
                    tx_id,
                    row_id: *row_id,
                    value: row.clone(),
                },
                Mutation::Delete => WalRecord::Delete {
                    tx_id,
                    row_id: *row_id,
                },
            });
        }
        records.push(WalRecord::CommitV2 {
            tx_id,
            commit_ts: self.commit_ts,
            commit_seq: self.commit_seq,
            schema_epoch: self.schema_epoch,
        });
        records
    }
}

/// A transaction reassembled from the log, with its log positions.
#[derive(Debug, Clone)]
pub struct LoggedTx {
    /// The transaction.
    pub tx: CommittedTx,
    /// Position of its `Commit` record.
    pub commit_lsn: Lsn,
    /// Position right after its `Commit` record (the next transaction boundary).
    pub end: Lsn,
}

/// Reassembles committed transactions from consecutive log entries.
///
/// The engine appends each transaction as one contiguous run, so at most one transaction is
/// open at a time. A run without `Commit` that is followed by a new `Begin` belongs to a
/// process that died mid-append and is discarded, exactly like an incomplete tail.
#[derive(Debug, Default)]
pub struct TxAssembler {
    /// Transaction whose records are being collected.
    open: Option<CommittedTx>,
    /// Records of transactions whose `Begin` precedes the readable log are skipped up to here
    /// (only for databases migrated from Milestone 2.0.2, whose logs may have been pruned).
    tolerate_orphans_through: Option<Lsn>,
}

impl TxAssembler {
    /// An assembler that requires every transaction to start with `Begin`.
    pub fn strict() -> Self {
        Self::default()
    }

    /// An assembler that silently skips orphan records at or before `through`.
    pub fn tolerating_orphans_through(through: Option<Lsn>) -> Self {
        Self {
            open: None,
            tolerate_orphans_through: through,
        }
    }

    /// Whether a transaction is open (the log ended mid-transaction).
    pub fn is_mid_transaction(&self) -> bool {
        self.open.is_some()
    }

    /// Consumes one entry; returns a transaction when `entry` is its `Commit`.
    pub fn push(&mut self, entry: LogEntry) -> Result<Option<LoggedTx>, WalError> {
        let LogEntry { lsn, next, record } = entry;
        match record {
            WalRecord::Begin { tx_id, snapshot_ts } => {
                self.open = Some(CommittedTx {
                    tx_id,
                    snapshot_ts,
                    commit_ts: CommitTs(0),
                    commit_seq: CommitSeq(0),
                    schema_epoch: SchemaEpoch(0),
                    before_images: Vec::new(),
                    mutations: Vec::new(),
                    catalog: Vec::new(),
                });
                Ok(None)
            }
            WalRecord::Catalog {
                tx_id,
                epoch,
                event,
            } => {
                if let Some(open) = self.open_for(tx_id, lsn)? {
                    open.catalog.push((epoch, event));
                }
                Ok(None)
            }
            WalRecord::Version {
                tx_id,
                row_id,
                begin_ts,
                end_ts,
                value,
            } => {
                if begin_ts >= end_ts {
                    return Err(WalError::Corrupt(format!(
                        "empty version interval in tx {}",
                        tx_id.0
                    )));
                }
                if let Some(open) = self.open_for(tx_id, lsn)? {
                    open.before_images.push((
                        row_id,
                        HistoricalVersion {
                            begin_ts,
                            end_ts,
                            value,
                        },
                    ));
                }
                Ok(None)
            }
            WalRecord::Put {
                tx_id,
                row_id,
                value,
            } => {
                if let Some(open) = self.open_for(tx_id, lsn)? {
                    open.mutations.push((row_id, Mutation::Put(value)));
                }
                Ok(None)
            }
            WalRecord::Delete { tx_id, row_id } => {
                if let Some(open) = self.open_for(tx_id, lsn)? {
                    open.mutations.push((row_id, Mutation::Delete));
                }
                Ok(None)
            }
            WalRecord::Abort { tx_id } => {
                self.open_for(tx_id, lsn)?;
                self.open = None;
                Ok(None)
            }
            WalRecord::Commit { tx_id, commit_ts } => {
                // Logs of Milestones before 2.5.0 carry no sequence numbers; their commits are
                // numbered by timestamp, which is unique and increasing. New commits continue
                // above the highest of them. The epoch is the one of the last catalog change
                // seen so far in the transaction (0 when it changed none).
                self.finish(tx_id, lsn, next, commit_ts, CommitSeq(commit_ts.0), None)
            }
            WalRecord::CommitV2 {
                tx_id,
                commit_ts,
                commit_seq,
                schema_epoch,
            } => self.finish(tx_id, lsn, next, commit_ts, commit_seq, Some(schema_epoch)),
        }
    }

    /// Closes the open transaction at its commit record and hands it out.
    fn finish(
        &mut self,
        tx_id: TxId,
        lsn: Lsn,
        next: Lsn,
        commit_ts: CommitTs,
        commit_seq: CommitSeq,
        schema_epoch: Option<SchemaEpoch>,
    ) -> Result<Option<LoggedTx>, WalError> {
        if self.open_for(tx_id, lsn)?.is_none() {
            return Ok(None);
        }
        let mut tx = self.open.take().expect("checked by open_for");
        tx.commit_ts = commit_ts;
        tx.commit_seq = commit_seq;
        if let Some(epoch) = schema_epoch {
            tx.schema_epoch = epoch;
        } else if let Some((epoch, _)) = tx.catalog.last() {
            tx.schema_epoch = *epoch;
        }
        Ok(Some(LoggedTx {
            tx,
            commit_lsn: lsn,
            end: next,
        }))
    }

    /// The open transaction if `tx_id` matches it; `None` for a tolerated orphan.
    fn open_for(&mut self, tx_id: TxId, lsn: Lsn) -> Result<Option<&mut CommittedTx>, WalError> {
        let open_id = self.open.as_ref().map(|open| open.tx_id);
        if open_id == Some(tx_id) {
            return Ok(self.open.as_mut());
        }
        if self
            .tolerate_orphans_through
            .is_some_and(|through| lsn <= through)
        {
            return Ok(None);
        }
        Err(WalError::Corrupt(match open_id {
            Some(open) => format!(
                "record of tx {} interleaved with open tx {} at {lsn:?}",
                tx_id.0, open.0
            ),
            None => format!("record of tx {} without BEGIN at {lsn:?}", tx_id.0),
        }))
    }
}

/// Unit tests of log encoding and reassembly.
#[cfg(test)]
mod tests {
    use super::*;
    use adb_core::Row;

    /// Wraps records as consecutive log entries starting at position `start`.
    fn entries(records: Vec<WalRecord>, start: u64) -> Vec<LogEntry> {
        records
            .into_iter()
            .enumerate()
            .map(|(i, record)| LogEntry {
                lsn: Lsn(start + i as u64),
                next: Lsn(start + i as u64 + 1),
                record,
            })
            .collect()
    }

    /// A transaction with one before-image, one delete and one put.
    fn sample(id: u64) -> CommittedTx {
        CommittedTx {
            tx_id: TxId(id),
            snapshot_ts: CommitTs(1),
            commit_ts: CommitTs(2),
            commit_seq: CommitSeq(2),
            schema_epoch: SchemaEpoch(1),
            before_images: vec![(
                RowId(1),
                HistoricalVersion {
                    begin_ts: CommitTs(1),
                    end_ts: CommitTs(2),
                    value: Some(Row::new()),
                },
            )],
            mutations: vec![
                (RowId(1), Mutation::Delete),
                (RowId(2), Mutation::Put(Row::new())),
            ],
            catalog: Vec::new(),
        }
    }

    /// Encoding a transaction and reassembling it yields the same transaction and end position.
    #[test]
    fn records_round_trip_through_the_assembler() {
        let tx = sample(7);
        let mut assembler = TxAssembler::strict();
        let mut out = Vec::new();
        for entry in entries(tx.to_records(), 0) {
            out.extend(assembler.push(entry).unwrap());
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tx, tx);
        assert_eq!(out[0].end, Lsn(5));
    }

    /// Catalog records travel with their transaction and come back in order, with their epochs.
    #[test]
    fn catalog_records_round_trip() {
        let mut tx = sample(9);
        tx.catalog = vec![
            (SchemaEpoch(3), CatalogEvent::Epoch),
            (SchemaEpoch(4), CatalogEvent::DropEntity { entity_id: 2 }),
        ];
        let mut assembler = TxAssembler::strict();
        let logged: Vec<_> = entries(tx.to_records(), 0)
            .into_iter()
            .filter_map(|entry| assembler.push(entry).unwrap())
            .collect();
        assert_eq!(logged[0].tx, tx);
        assert_eq!(logged[0].end, Lsn(7));
    }

    /// A run cut off by a crash is followed by the next process's transactions.
    #[test]
    fn an_unfinished_run_followed_by_begin_is_discarded() {
        let mut records = sample(1).to_records();
        records.pop(); // no COMMIT: the process died here
        records.extend(sample(2).to_records());
        let mut assembler = TxAssembler::strict();
        let committed: Vec<_> = entries(records, 0)
            .into_iter()
            .filter_map(|entry| assembler.push(entry).unwrap())
            .collect();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].tx.tx_id, TxId(2));
    }

    /// Records without `Begin` are corruption, except in the tolerated legacy prefix.
    #[test]
    fn orphans_are_corruption_unless_tolerated() {
        let records = sample(1).to_records()[1..].to_vec();
        let mut strict = TxAssembler::strict();
        assert!(entries(records.clone(), 0)
            .into_iter()
            .any(|entry| strict.push(entry).is_err()));

        let mut tolerant = TxAssembler::tolerating_orphans_through(Some(Lsn(100)));
        for entry in entries(records, 0) {
            assert!(tolerant.push(entry).unwrap().is_none());
        }
    }

    /// A commit record of a log written before sequence numbers existed is numbered by its
    /// timestamp and takes the epoch of its last catalog change.
    #[test]
    fn legacy_commits_are_numbered_by_timestamp() {
        let mut tx = sample(4);
        tx.catalog = vec![(SchemaEpoch(6), CatalogEvent::Epoch)];
        let mut records = tx.to_records();
        let last = records.len() - 1;
        records[last] = WalRecord::Commit {
            tx_id: tx.tx_id,
            commit_ts: tx.commit_ts,
        };
        let mut assembler = TxAssembler::strict();
        let mut out = Vec::new();
        for entry in entries(records, 0) {
            out.extend(assembler.push(entry).unwrap());
        }
        assert_eq!(out[0].tx.commit_seq, CommitSeq(tx.commit_ts.0));
        assert_eq!(out[0].tx.schema_epoch, SchemaEpoch(6));
    }
}
