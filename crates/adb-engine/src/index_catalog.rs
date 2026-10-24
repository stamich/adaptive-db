//! The engine-owned catalog of secondary indexes (`indexes/catalog.meta`).
//!
//! The engine maintains every index inside the commit, so it must know the definitions without
//! asking the JVM (the same reasoning that made statistics native in 2.2.3). The catalog is small
//! and changes only with `CREATE INDEX` and `DROP INDEX`; it is checkpointed state: the commit
//! of the checkpoint journal that publishes an index's pages also publishes the catalog that
//! names it, so a definition and its index are always on disk together.
//!
//! File: an envelope (magic, version, CRC32) around the bincode encoding of
//! [`IndexCatalog`]; a damaged file is reported as corruption.

use std::path::{Path, PathBuf};

use adb_core::{CommitTs, IndexId};
use adb_index::{IndexDefinition, IndexSpec, IndexState};
use adb_journal::{envelope, FileWrite};
use serde::{Deserialize, Serialize};

use crate::DbError;

/// Directory of the index projections and the catalog inside a database directory.
pub const INDEXES_DIR: &str = "indexes";
/// Catalog file inside [`INDEXES_DIR`].
pub const CATALOG_FILE: &str = "catalog.meta";
/// Most indexes of one database.
pub const MAX_INDEXES: usize = 256;
/// Most indexes of one entity.
pub const MAX_INDEXES_PER_ENTITY: usize = 32;
/// Magic prefix of the catalog envelope.
const MAGIC: &[u8; 8] = b"ADBIDX01";
/// Envelope version of the catalog.
const VERSION: u16 = 1;
/// Upper bound on the catalog file size.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

/// Every index definition plus the id counter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IndexCatalog {
    /// Next id to hand out (ids are never reused, so stale files never match a new index).
    next_id: u64,
    /// Definitions in id order.
    definitions: Vec<IndexDefinition>,
}

impl IndexCatalog {
    /// Loads the catalog of the database in `dir`; a missing file is an empty catalog.
    pub fn load(dir: &Path) -> Result<Self, DbError> {
        let path = catalog_path(dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(&path)?;
        let payload = envelope::open(MAGIC, VERSION, &bytes, MAX_FILE_BYTES)
            .map_err(|reason| DbError::CorruptIndexCatalog(reason.to_string()))?;
        let catalog: Self = bincode::deserialize(payload)
            .map_err(|error| DbError::CorruptIndexCatalog(error.to_string()))?;
        catalog.check()?;
        Ok(catalog)
    }

    /// The journal write that persists the catalog of the database in `dir`.
    pub fn journal_write(&self, dir: &Path) -> Result<FileWrite, DbError> {
        let payload = bincode::serialize(self)
            .map_err(|error| DbError::CorruptIndexCatalog(error.to_string()))?;
        Ok(FileWrite::replace(
            catalog_path(dir),
            envelope::seal(MAGIC, VERSION, &payload),
        ))
    }

    /// Every definition, in id order.
    pub fn definitions(&self) -> &[IndexDefinition] {
        &self.definitions
    }

    /// The definition named `name` (case-insensitive).
    pub fn by_name(&self, name: &str) -> Option<&IndexDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name.eq_ignore_ascii_case(name))
    }

    /// The definition with id `id`.
    pub fn by_id(&self, id: IndexId) -> Option<&IndexDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
    }

    /// Validates `spec` against the catalog and adds it in state `state`; returns the new
    /// definition. Fails on an invalid spec, a taken name or a full catalog.
    pub fn add(
        &mut self,
        spec: IndexSpec,
        state: IndexState,
        created_at_ts: CommitTs,
    ) -> Result<IndexDefinition, DbError> {
        spec.validate().map_err(DbError::Index)?;
        if self.by_name(&spec.name).is_some() {
            return Err(DbError::InvalidArgument(format!(
                "an index named {} already exists",
                spec.name
            )));
        }
        if self.definitions.len() >= MAX_INDEXES {
            return Err(DbError::InvalidArgument(format!(
                "a database holds at most {MAX_INDEXES} indexes"
            )));
        }
        let on_entity = self
            .definitions
            .iter()
            .filter(|definition| definition.entity_id == spec.entity_id)
            .count();
        if on_entity >= MAX_INDEXES_PER_ENTITY {
            return Err(DbError::InvalidArgument(format!(
                "an entity has at most {MAX_INDEXES_PER_ENTITY} indexes"
            )));
        }
        let definition = IndexDefinition::new(IndexId(self.next_id), spec, state, created_at_ts);
        self.next_id += 1;
        self.definitions.push(definition.clone());
        Ok(definition)
    }

    /// Changes the state of index `id`; returns whether it exists.
    pub fn set_state(&mut self, id: IndexId, state: IndexState) -> bool {
        match self
            .definitions
            .iter_mut()
            .find(|definition| definition.id == id)
        {
            Some(definition) => {
                definition.state = state;
                true
            }
            None => false,
        }
    }

    /// Removes index `id` and returns its definition.
    pub fn remove(&mut self, id: IndexId) -> Option<IndexDefinition> {
        let at = self
            .definitions
            .iter()
            .position(|definition| definition.id == id)?;
        Some(self.definitions.remove(at))
    }

    /// Consistency of a loaded catalog: valid specs, unique ids below the counter, unique names.
    fn check(&self) -> Result<(), DbError> {
        let fail = |message: String| Err(DbError::CorruptIndexCatalog(message));
        if self.definitions.len() > MAX_INDEXES {
            return fail(format!("more than {MAX_INDEXES} indexes"));
        }
        for (at, definition) in self.definitions.iter().enumerate() {
            let spec = IndexSpec {
                name: definition.name.clone(),
                entity_id: definition.entity_id,
                columns: definition.columns.clone(),
                include: definition.include.clone(),
            };
            if let Err(error) = spec.validate() {
                return fail(error.to_string());
            }
            if definition.id.0 >= self.next_id {
                return fail(format!(
                    "index id {} is not below the counter",
                    definition.id.0
                ));
            }
            if self.definitions[..at].iter().any(|other| {
                other.id == definition.id || other.name.eq_ignore_ascii_case(&definition.name)
            }) {
                return fail(format!("index {} appears twice", definition.name));
            }
        }
        Ok(())
    }
}

/// Path of the catalog of the database in `dir`.
pub fn catalog_path(dir: &Path) -> PathBuf {
    dir.join(INDEXES_DIR).join(CATALOG_FILE)
}

/// Directory of the projection of index `id` of the database in `dir`.
pub fn index_dir(dir: &Path, id: IndexId) -> PathBuf {
    dir.join(INDEXES_DIR).join(format!("{:016x}", id.0))
}

/// Removes projection directories that no catalog entry names (left by a crash between the
/// checkpoint that dropped an index and the removal of its files, or by an interrupted build);
/// returns how many were removed.
pub fn remove_orphans(dir: &Path, catalog: &IndexCatalog) -> Result<u64, DbError> {
    let root = dir.join(INDEXES_DIR);
    if !root.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(&root)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let known = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| u64::from_str_radix(name, 16).ok())
            .is_some_and(|id| catalog.by_id(IndexId(id)).is_some());
        if !known {
            std::fs::remove_dir_all(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Unit tests of the catalog rules and its file format.
#[cfg(test)]
mod tests {
    use adb_core::Direction;
    use adb_index::IndexColumn;
    use adb_journal::Journal;

    use super::*;

    /// A one-column index named `name` on entity `entity`.
    fn spec(name: &str, entity: u64) -> IndexSpec {
        IndexSpec {
            name: name.into(),
            entity_id: entity,
            columns: vec![IndexColumn {
                field_id: 2,
                direction: Direction::Asc,
            }],
            include: vec![],
        }
    }

    /// Ids are never reused, names are unique case-insensitively, limits hold.
    #[test]
    fn ids_names_and_limits() {
        let mut catalog = IndexCatalog::default();
        let first = catalog
            .add(spec("a", 1), IndexState::Active, CommitTs(1))
            .unwrap();
        assert!(catalog
            .add(spec("A", 1), IndexState::Active, CommitTs(1))
            .is_err());
        assert!(catalog
            .add(spec("bad name", 1), IndexState::Active, CommitTs(1))
            .is_err());
        catalog.remove(first.id).unwrap();
        let second = catalog
            .add(spec("a", 1), IndexState::Active, CommitTs(2))
            .unwrap();
        assert_ne!(first.id, second.id);
        for i in 1..MAX_INDEXES_PER_ENTITY {
            catalog
                .add(spec(&format!("e{i}"), 1), IndexState::Active, CommitTs(2))
                .unwrap();
        }
        assert!(catalog
            .add(spec("full", 1), IndexState::Active, CommitTs(2))
            .is_err());
        assert!(catalog.set_state(second.id, IndexState::Building));
        assert_eq!(catalog.by_name("A").unwrap().state, IndexState::Building);
    }

    /// The catalog round-trips through the journal; a damaged file is corruption; orphan
    /// projection directories are removed.
    #[test]
    fn persistence_and_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = IndexCatalog::default();
        let kept = catalog
            .add(spec("kept", 1), IndexState::Active, CommitTs(3))
            .unwrap();
        std::fs::create_dir_all(dir.path().join(INDEXES_DIR)).unwrap();
        Journal::new(dir.path())
            .commit(&[catalog.journal_write(dir.path()).unwrap()])
            .unwrap();
        assert_eq!(IndexCatalog::load(dir.path()).unwrap(), catalog);

        std::fs::create_dir_all(index_dir(dir.path(), kept.id)).unwrap();
        std::fs::create_dir_all(index_dir(dir.path(), IndexId(77))).unwrap();
        assert_eq!(remove_orphans(dir.path(), &catalog).unwrap(), 1);
        assert!(index_dir(dir.path(), kept.id).exists());

        std::fs::write(catalog_path(dir.path()), b"damaged").unwrap();
        assert!(IndexCatalog::load(dir.path()).unwrap_err().is_corruption());
    }
}
