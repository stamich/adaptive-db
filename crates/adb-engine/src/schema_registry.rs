//! Persistence of the schema registry (`schema/registry.meta`).
//!
//! The registry is a projection of the catalog records in the log (see
//! [`adb_schema::SchemaRegistry`]): a checkpoint publishes a snapshot of it in the same journal
//! commit as the other projections, recovery loads that snapshot and replays the catalog records
//! logged after it, and `rebuild_projections` deletes it and replays the whole log.
//!
//! File: an envelope (magic, version, CRC32) around the bincode encoding of
//! [`RegistryState`]; a damaged file, or a history that fails validation, is corruption.

use std::path::{Path, PathBuf};

use adb_journal::{envelope, replace_file, sync_dir, FileWrite};
use adb_schema::{RegistryState, SchemaRegistry};

use crate::DbError;

/// Directory of the registry inside a database directory.
pub const SCHEMA_DIR: &str = "schema";
/// Registry file inside [`SCHEMA_DIR`].
pub const REGISTRY_FILE: &str = "registry.meta";
/// Magic prefix of the registry envelope.
const MAGIC: &[u8; 8] = b"ADBSCH01";
/// Envelope version of the registry.
const VERSION: u16 = 1;
/// Upper bound on the registry file size.
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Path of the registry of the database in `dir`.
pub fn registry_path(dir: &Path) -> PathBuf {
    dir.join(SCHEMA_DIR).join(REGISTRY_FILE)
}

/// Loads the registry of the database in `dir`; a missing file is an empty registry.
pub fn load(dir: &Path) -> Result<SchemaRegistry, DbError> {
    let path = registry_path(dir);
    if !path.exists() {
        return Ok(SchemaRegistry::new());
    }
    let bytes = std::fs::read(&path)?;
    let payload = envelope::open(MAGIC, VERSION, &bytes, MAX_FILE_BYTES)
        .map_err(|reason| DbError::CorruptSchemaRegistry(reason.to_string()))?;
    let state: RegistryState = bincode::deserialize(payload)
        .map_err(|error| DbError::CorruptSchemaRegistry(error.to_string()))?;
    SchemaRegistry::from_state(state)
        .map_err(|error| DbError::CorruptSchemaRegistry(error.to_string()))
}

/// The journal write that persists `registry` for the database in `dir`.
pub fn journal_write(dir: &Path, registry: &SchemaRegistry) -> Result<FileWrite, DbError> {
    Ok(FileWrite::replace(registry_path(dir), encode(registry)?))
}

/// Writes the registry directly (temp file, fsync, rename): for tools and tests on a closed
/// database; the engine publishes it through the checkpoint journal.
pub fn save(dir: &Path, registry: &SchemaRegistry) -> Result<(), DbError> {
    let root = dir.join(SCHEMA_DIR);
    std::fs::create_dir_all(&root)?;
    replace_file(&registry_path(dir), &encode(registry)?)?;
    sync_dir(&root)?;
    Ok(())
}

/// The envelope-framed encoding of a registry.
fn encode(registry: &SchemaRegistry) -> Result<Vec<u8>, DbError> {
    let payload = bincode::serialize(registry.state())
        .map_err(|error| DbError::CorruptSchemaRegistry(error.to_string()))?;
    Ok(envelope::seal(MAGIC, VERSION, &payload))
}
