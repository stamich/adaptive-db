//! Data format marker of a database directory (`format.meta`).
//!
//! The data format names how logical values are laid out in the log and the projections:
//!
//! | Version | Milestone | Change |
//! |---|---|---|
//! | 1 | ≤ 2.2.3 | primary keys stored as raw two's-complement bits (negative keys sort last); no marker file |
//! | 2 | 2.3.0 | primary keys stored sign-flipped ([`adb_core::RowId::from_primary_key`]), so key order is value order |
//! | 3 | 2.4.0 | rows carry the schema version they were written under ([`adb_core::Row`]); the log holds catalog records |
//! | 4 | 2.5.0 | commits carry a global sequence number and a catalog epoch ([`adb_wal::WalRecord::CommitV2`]); the checkpoint records the highest sequence number |
//!
//! Format 4 reads formats 2 and 3 unchanged: a 2.3.0 row decodes as an unversioned row, a 2.3.0
//! log holds no catalog records, and a commit record of a log before 2.5.0 is numbered by its
//! timestamp (see [`crate::committed::TxAssembler`]). A format-2 or format-3 directory is
//! therefore upgraded in place on open by stamping its marker (an older engine then refuses
//! it); nothing is rewritten. A newer format than this engine knows is refused.
//!
//! A new directory is stamped with [`DATA_FORMAT_VERSION`] when it is first opened. A directory
//! without a marker whose log holds records was written by an older engine (format 1) and is
//! refused with [`DbError::IncompatibleFormat`]; `adb-migrate` ([`mod@crate::migrate`])
//! rewrites it. A directory without a marker and without log records holds no data and is
//! stamped.

use std::path::Path;

use adb_core::Lsn;
use adb_journal::{envelope, replace_file, sync_dir};
use adb_wal::WalCursor;

use crate::DbError;

/// Data format written by this engine.
pub const DATA_FORMAT_VERSION: u32 = 4;
/// Oldest marked format this engine upgrades in place (see the module docs); every format from
/// here up to [`DATA_FORMAT_VERSION`] is upgraded the same way.
pub const UPGRADABLE_DATA_FORMAT_VERSION: u32 = 2;
/// Data format of directories written before the marker existed.
pub const LEGACY_DATA_FORMAT_VERSION: u32 = 1;
/// Marker file inside the database directory.
pub const FORMAT_FILE: &str = "format.meta";
/// Magic prefix of the marker envelope.
const MAGIC: &[u8; 8] = b"ADBFMT01";
/// Envelope version of the marker.
const ENVELOPE_VERSION: u16 = 1;
/// Upper bound on the marker file size.
const MAX_FILE_BYTES: usize = 64;

/// Data format of the database in `dir` (with its log in `wal_dir`): the marker's version, or
/// [`LEGACY_DATA_FORMAT_VERSION`] for an unmarked directory with log records, or `None` for an
/// unmarked directory without data.
pub fn detect(dir: &Path, wal_dir: &Path) -> Result<Option<u32>, DbError> {
    let path = dir.join(FORMAT_FILE);
    if path.exists() {
        let bytes = std::fs::read(&path)?;
        let payload = envelope::open(MAGIC, ENVELOPE_VERSION, &bytes, MAX_FILE_BYTES)
            .map_err(|error| DbError::CorruptFormatMarker(error.to_string()))?;
        let version: [u8; 4] = payload
            .try_into()
            .map_err(|_| DbError::CorruptFormatMarker("marker payload is not 4 bytes".into()))?;
        return Ok(Some(u32::from_le_bytes(version)));
    }
    if !wal_dir.exists() {
        return Ok(None);
    }
    let mut cursor = WalCursor::open(wal_dir, Lsn(0))?;
    Ok(cursor
        .next_before(Lsn(u64::MAX))?
        .map(|_| LEGACY_DATA_FORMAT_VERSION))
}

/// Checks that the database in `dir` uses [`DATA_FORMAT_VERSION`], stamping a directory that
/// holds no data yet and upgrading a directory of format 2 or 3 (which format 4 reads unchanged).
pub fn check_or_stamp(dir: &Path, wal_dir: &Path) -> Result<(), DbError> {
    match detect(dir, wal_dir)? {
        Some(DATA_FORMAT_VERSION) => Ok(()),
        Some(found) if (UPGRADABLE_DATA_FORMAT_VERSION..DATA_FORMAT_VERSION).contains(&found) => {
            stamp(dir, DATA_FORMAT_VERSION)
        }
        Some(found) => Err(DbError::IncompatibleFormat {
            found,
            expected: DATA_FORMAT_VERSION,
        }),
        None => stamp(dir, DATA_FORMAT_VERSION),
    }
}

/// Writes the marker for `version` atomically (temp file, fsync, rename, directory fsync).
pub fn stamp(dir: &Path, version: u32) -> Result<(), DbError> {
    let bytes = envelope::seal(MAGIC, ENVELOPE_VERSION, &version.to_le_bytes());
    replace_file(&dir.join(FORMAT_FILE), &bytes)?;
    sync_dir(dir)?;
    Ok(())
}
