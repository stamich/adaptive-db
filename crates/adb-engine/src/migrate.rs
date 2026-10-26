//! Offline migration of a database directory from data format 1 to 2 (see [`crate::format`]).
//!
//! Format 1 stored primary keys as raw two's-complement bits; format 2 flips their sign bit so
//! key order is value order. The log is the source of truth, so the migration rewrites the log
//! and rebuilds every projection from it:
//!
//! 1. the source must be an unmarked format-1 directory (it is never modified);
//! 2. every log record is copied into the target with the sign bit of its row key flipped
//!    ([`adb_wal::rewrite_log`]); records keep their size, so every LSN stays valid and the
//!    consumer offsets of the change feed are copied unchanged;
//! 3. statistics documents are copied (they describe values, not keys); the modification
//!    counters are recounted from the log, like `Database::rebuild_projections` does;
//! 4. the target is stamped as format 2 and opened, which replays the whole log into fresh
//!    projections, then checkpointed;
//! 5. verification: the target log is read back and compared record by record with the
//!    mapped source log, the projections pass the temporal integrity check, and the live rows
//!    per entity are reported.
//!
//! [`migrate_in_place`] wraps this in copy, verify and swap: the original directory is kept
//! next to the migrated one until the user removes it.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use adb_core::{KeyRange, Lsn, RowId};
use adb_wal::{rewrite_log, WalCursor, WalRecord};

use crate::{
    format::{self, DATA_FORMAT_VERSION, LEGACY_DATA_FORMAT_VERSION},
    statistics::{MODIFICATIONS_FILE, STATS_DIR},
    Database, DbError,
};

/// Log directory inside a database directory.
const WAL_DIR: &str = "wal";
/// Change-feed consumer offsets directory inside a database directory.
const CDC_DIR: &str = "cdc";
/// Sign bit of the key bits of a row id.
const SIGN: u128 = 1 << 63;
/// Rows read per page while counting the live rows of the target.
const COUNT_PAGE: usize = 4096;

/// What a migration did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MigrationReport {
    /// Log records rewritten (and verified).
    pub records: u64,
    /// Log segments written.
    pub segments: u64,
    /// Bytes of an incomplete crash tail dropped from the end of the source log.
    pub dropped_tail_bytes: u64,
    /// Statistics documents copied.
    pub statistics_documents: u64,
    /// Live rows per entity in the migrated database.
    pub rows_per_entity: BTreeMap<u64, u64>,
}

/// Migrates the format-1 database in `source` into the new directory `target` (which must not
/// exist or be empty). `source` is only read.
pub fn migrate(source: &Path, target: &Path) -> Result<MigrationReport, DbError> {
    match format::detect(source, &source.join(WAL_DIR))? {
        Some(LEGACY_DATA_FORMAT_VERSION) => {}
        Some(found) => {
            return Err(DbError::InvalidArgument(format!(
                "{} is already in data format {found}; nothing to migrate",
                source.display()
            )))
        }
        None => {
            return Err(DbError::InvalidArgument(format!(
                "{} holds no database log",
                source.display()
            )))
        }
    }
    if target.exists() && fs::read_dir(target)?.next().is_some() {
        return Err(DbError::InvalidArgument(format!(
            "target {} exists and is not empty",
            target.display()
        )));
    }
    fs::create_dir_all(target)?;

    let mut report = MigrationReport::default();
    let rewrite = rewrite_log(&source.join(WAL_DIR), &target.join(WAL_DIR), flip_row_keys)?;
    report.records = rewrite.records;
    report.segments = rewrite.segments;
    report.dropped_tail_bytes = rewrite.dropped_tail_bytes;
    report.statistics_documents = copy_statistics(source, target)?;
    let offsets = source.join(CDC_DIR);
    if offsets.exists() {
        copy_dir(&offsets, &target.join(CDC_DIR))?;
    }
    verify_log(source, target)?;

    format::stamp(target, DATA_FORMAT_VERSION)?;
    let db = Database::open(target)?;
    let integrity = db.verify()?;
    if !integrity.is_ok() {
        return Err(DbError::InvalidArgument(format!(
            "migrated projections fail the integrity check: {integrity:?}"
        )));
    }
    report.rows_per_entity = live_rows(&db)?;
    db.close()?;
    Ok(report)
}

/// Migrates `dir` in place: migrates into `<dir>.migrating`, then renames `dir` to
/// `<dir>.format1` and the migrated copy to `dir`. Returns the report and the path of the kept
/// original.
pub fn migrate_in_place(dir: &Path) -> Result<(MigrationReport, PathBuf), DbError> {
    let staging = sibling(dir, "migrating")?;
    let backup = sibling(dir, "format1")?;
    if backup.exists() {
        return Err(DbError::InvalidArgument(format!(
            "{} exists; remove or move it first",
            backup.display()
        )));
    }
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let report = migrate(dir, &staging)?;
    fs::rename(dir, &backup)?;
    fs::rename(&staging, dir)?;
    Ok((report, backup))
}

/// The record with the sign bit of its row key flipped (format 1 -> 2); records without a row
/// key are unchanged. The mapping is its own inverse.
pub fn flip_row_keys(record: WalRecord) -> WalRecord {
    let flip = |row_id: RowId| RowId(row_id.0 ^ SIGN);
    match record {
        WalRecord::Version {
            tx_id,
            row_id,
            begin_ts,
            end_ts,
            value,
        } => WalRecord::Version {
            tx_id,
            row_id: flip(row_id),
            begin_ts,
            end_ts,
            value,
        },
        WalRecord::Put {
            tx_id,
            row_id,
            value,
        } => WalRecord::Put {
            tx_id,
            row_id: flip(row_id),
            value,
        },
        WalRecord::Delete { tx_id, row_id } => WalRecord::Delete {
            tx_id,
            row_id: flip(row_id),
        },
        other => other,
    }
}

/// Reads both logs back in lockstep and checks that every target record is the mapped source
/// record at the same position (the target frames were re-checksummed by the rewrite).
fn verify_log(source: &Path, target: &Path) -> Result<(), DbError> {
    let mut before = WalCursor::open(source.join(WAL_DIR), Lsn(0))?;
    let mut after = WalCursor::open(target.join(WAL_DIR), Lsn(0))?;
    loop {
        let (old, new) = match (
            before.next_before(Lsn(u64::MAX))?,
            after.next_before(Lsn(u64::MAX))?,
        ) {
            (None, None) => return Ok(()),
            (Some(old), Some(new)) => (old, new),
            (_, new) => {
                return Err(DbError::InvalidArgument(format!(
                    "migrated log ends differently from the source (at {:?})",
                    new.map(|entry| entry.lsn)
                )))
            }
        };
        if old.lsn != new.lsn || flip_row_keys(old.record) != new.record {
            return Err(DbError::InvalidArgument(format!(
                "migrated log differs from the source at {:?}",
                new.lsn
            )));
        }
    }
}

/// Copies the statistics documents (not the modification counters, which are recounted);
/// returns how many.
fn copy_statistics(source: &Path, target: &Path) -> Result<u64, DbError> {
    let from = source.join(STATS_DIR);
    if !from.exists() {
        return Ok(0);
    }
    let to = target.join(STATS_DIR);
    fs::create_dir_all(&to)?;
    let mut copied = 0;
    for entry in fs::read_dir(&from)? {
        let path = entry?.path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) != Some(MODIFICATIONS_FILE) {
            fs::copy(&path, to.join(path.file_name().expect("file has a name")))?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Copies the regular files of one directory.
fn copy_dir(from: &Path, to: &Path) -> Result<(), DbError> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let path = entry?.path();
        if path.is_file() {
            fs::copy(&path, to.join(path.file_name().expect("file has a name")))?;
        }
    }
    Ok(())
}

/// Live rows per entity at the latest snapshot.
fn live_rows(db: &Database) -> Result<BTreeMap<u64, u64>, DbError> {
    use adb_execution::DataSource;
    let snapshot = db.latest_committed_ts();
    let mut counts = BTreeMap::new();
    let mut after = None;
    loop {
        let page = db
            .scan_page(&KeyRange::all(), after, COUNT_PAGE, snapshot)
            .map_err(DbError::Execution)?;
        let Some((last, _)) = page.last() else {
            return Ok(counts);
        };
        after = Some(*last);
        for (row_id, _) in &page {
            *counts.entry(row_id.entity_id()).or_insert(0) += 1;
        }
    }
}

/// `<dir>.<suffix>` next to `dir`.
fn sibling(dir: &Path, suffix: &str) -> Result<PathBuf, DbError> {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| DbError::InvalidArgument(format!("{} has no name", dir.display())))?;
    Ok(dir.with_file_name(format!("{name}.{suffix}")))
}
