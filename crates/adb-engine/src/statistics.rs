//! Optimizer statistics owned by the engine: the per-entity documents written by `ANALYZE`
//! and the per-entity modification counters that tell how stale they are.
//!
//! Both are **derived data**: the canonical log stays the source of truth.
//!
//! * `stats/entity-<id>.stats` — one [`TableStatistics`] document per analyzed entity, in a
//!   checksummed envelope, replaced atomically. A missing or unreadable file means "no
//!   statistics"; `ANALYZE` rewrites it.
//! * `stats/modifications.meta` — the number of row mutations committed per entity since the
//!   database was created. It changes with every commit, so it is persisted together with the
//!   projections through the checkpoint journal and, after a crash, brought forward by log
//!   replay exactly like the projections. `Database::rebuild_projections` deletes it and
//!   recounts from the complete log.
//!
//! "Modifications since ANALYZE" is the counter minus the value recorded in the document when
//! it was analyzed.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use adb_journal::{envelope, replace_file, sync_dir, FileWrite};
use adb_stats::{TableStatistics, MAX_STATISTICS_JSON_BYTES};
use adb_storage::StorageError;
use parking_lot::{Mutex, RwLock};

use crate::{committed::CommittedTx, DbError};

/// Statistics directory inside the database directory.
pub const STATS_DIR: &str = "stats";
/// Modification counters file inside [`STATS_DIR`].
pub const MODIFICATIONS_FILE: &str = "modifications.meta";

/// Magic prefix of a statistics document envelope.
const STATS_MAGIC: &[u8; 8] = b"ADBSTA01";
/// Envelope version of a statistics document.
const STATS_VERSION: u16 = 1;
/// Magic prefix of the modification counters envelope.
const MODIFICATIONS_MAGIC: &[u8; 8] = b"ADBMOD01";
/// Envelope version of the modification counters.
const MODIFICATIONS_VERSION: u16 = 1;
/// Upper bound on the modification counters file size.
const MAX_MODIFICATIONS_BYTES: usize = 16 * 1024 * 1024;

/// Row mutations committed per entity since the database was created.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ModificationCounters {
    /// Mutation count per entity id.
    counts: BTreeMap<u64, u64>,
}

impl ModificationCounters {
    /// Loads the counters of the database in `dir`; a missing file means all zero.
    ///
    /// A damaged file is reported as storage corruption, like any other checkpointed file;
    /// `Database::rebuild_projections` recounts it from the log.
    pub fn load(dir: &Path) -> Result<Self, DbError> {
        let path = modifications_path(dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(&path)?;
        let payload = envelope::open(
            MODIFICATIONS_MAGIC,
            MODIFICATIONS_VERSION,
            &bytes,
            MAX_MODIFICATIONS_BYTES,
        )
        .map_err(|reason| corrupt(&path, reason))?;
        let counts =
            bincode::deserialize(payload).map_err(|error| corrupt(&path, error.to_string()))?;
        Ok(Self { counts })
    }

    /// Adds every mutation of a committed transaction to its entity's counter.
    pub fn record(&mut self, tx: &CommittedTx) {
        for (row_id, _) in &tx.mutations {
            let count = self.counts.entry(row_id.entity_id()).or_default();
            *count = count.saturating_add(1);
        }
    }

    /// Mutations committed to `entity_id` so far.
    pub fn get(&self, entity_id: u64) -> u64 {
        self.counts.get(&entity_id).copied().unwrap_or(0)
    }

    /// The journal write that persists the counters of the database in `dir`.
    pub fn journal_write(&self, dir: &Path) -> Result<FileWrite, DbError> {
        let payload = bincode::serialize(&self.counts)
            .map_err(|error| corrupt(&modifications_path(dir), error.to_string()))?;
        Ok(FileWrite::replace(
            modifications_path(dir),
            envelope::seal(MODIFICATIONS_MAGIC, MODIFICATIONS_VERSION, &payload),
        ))
    }
}

/// The statistics documents of one database, cached in memory.
///
/// Every published document gets a **generation** from a counter of this instance, so a client
/// that caches decoded documents can detect a newer one with a single cheap call. Generations
/// are not persisted; they only need to differ within one open database. 0 means "none".
pub(crate) struct StatisticsStore {
    /// The `stats` directory.
    dir: PathBuf,
    /// Every readable document and its generation, by entity id.
    documents: RwLock<BTreeMap<u64, (TableStatistics, u64)>>,
    /// Last generation handed out.
    generation: AtomicU64,
    /// Serializes publication, so concurrent `ANALYZE`s never interleave their file writes.
    publish: Mutex<()>,
}

impl StatisticsStore {
    /// Loads the documents of the database in `dir`, creating the directory if needed.
    ///
    /// Unreadable documents are skipped: statistics are derived data and the entity simply
    /// counts as never analyzed until the next `ANALYZE`.
    pub fn open(dir: &Path) -> Result<Self, DbError> {
        let stats_dir = dir.join(STATS_DIR);
        std::fs::create_dir_all(&stats_dir)?;
        let mut documents = BTreeMap::new();
        for entry in std::fs::read_dir(&stats_dir)? {
            let path = entry?.path();
            let Some(entity_id) = entity_of(&path) else {
                continue;
            };
            if let Some(statistics) = read_document(&path)
                .ok()
                .filter(|statistics| statistics.entity_id == entity_id)
            {
                let generation = documents.len() as u64 + 1;
                documents.insert(entity_id, (statistics, generation));
            }
        }
        let generation = AtomicU64::new(documents.len() as u64);
        Ok(Self {
            dir: stats_dir,
            documents: RwLock::new(documents),
            generation,
            publish: Mutex::new(()),
        })
    }

    /// The document of `entity_id`, if it was analyzed.
    pub fn get(&self, entity_id: u64) -> Option<TableStatistics> {
        self.documents
            .read()
            .get(&entity_id)
            .map(|(statistics, _)| statistics.clone())
    }

    /// Generation of the document of `entity_id`; 0 if there is none.
    pub fn generation(&self, entity_id: u64) -> u64 {
        self.documents
            .read()
            .get(&entity_id)
            .map_or(0, |(_, generation)| *generation)
    }

    /// Durably replaces the document of its entity, then publishes it.
    ///
    /// A document of an older snapshot than the published one is not written (two concurrent
    /// `ANALYZE`s of one entity: the later snapshot wins whichever finishes last).
    pub fn save(&self, statistics: TableStatistics) -> Result<(), DbError> {
        let _publish = self.publish.lock();
        if self
            .documents
            .read()
            .get(&statistics.entity_id)
            .is_some_and(|(current, _)| current.analyzed_at_ts > statistics.analyzed_at_ts)
        {
            return Ok(());
        }
        let path = self.dir.join(file_name(statistics.entity_id));
        let json = statistics.to_json()?;
        replace_file(&path, &envelope::seal(STATS_MAGIC, STATS_VERSION, &json))?;
        sync_dir(&self.dir)?;
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.documents
            .write()
            .insert(statistics.entity_id, (statistics, generation));
        Ok(())
    }
}

/// Path of the modification counters of the database in `dir`.
pub(crate) fn modifications_path(dir: &Path) -> PathBuf {
    dir.join(STATS_DIR).join(MODIFICATIONS_FILE)
}

/// File name of the document of `entity_id`.
fn file_name(entity_id: u64) -> String {
    format!("entity-{entity_id}.stats")
}

/// Entity id encoded in a document file name, if `path` is one.
fn entity_of(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_prefix("entity-")?.strip_suffix(".stats")?;
    let entity_id = id.parse().ok()?;
    (file_name(entity_id) == name).then_some(entity_id)
}

/// Reads and validates one document.
fn read_document(path: &Path) -> Result<TableStatistics, DbError> {
    let bytes = std::fs::read(path)?;
    let json = envelope::open(
        STATS_MAGIC,
        STATS_VERSION,
        &bytes,
        MAX_STATISTICS_JSON_BYTES + 64,
    )
    .map_err(|reason| corrupt(path, reason))?;
    Ok(TableStatistics::from_json(json)?)
}

/// Maps a malformed statistics file to a storage-corruption error.
fn corrupt(path: &Path, reason: String) -> DbError {
    DbError::Storage(StorageError::Invalid(format!(
        "statistics file {}: {reason}",
        path.display()
    )))
}

/// Unit tests of file naming.
#[cfg(test)]
mod tests {
    use super::*;

    /// Only canonical document names map to an entity.
    #[test]
    fn document_names_are_canonical() {
        assert_eq!(entity_of(Path::new("stats/entity-7.stats")), Some(7));
        assert_eq!(entity_of(Path::new("entity-07.stats")), None);
        assert_eq!(entity_of(Path::new("entity-7.stats.tmp")), None);
        assert_eq!(entity_of(Path::new("modifications.meta")), None);
    }
}
