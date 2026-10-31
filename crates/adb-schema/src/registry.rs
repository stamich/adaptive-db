//! The schema registry: every entity's version history and the catalog epoch, as a projection
//! of the catalog records in the log.
//!
//! The engine applies each [`CatalogEvent`] in log order ([`SchemaRegistry::apply`]); recovery
//! replays them after loading the last checkpointed registry, and `rebuild_projections` replays
//! the whole log into an empty one. Versions are never deleted: a running query may still read
//! through an older one, and history is what makes the adapter possible.
//!
//! The registry also caches compiled [`RowAdapter`]s, one per `(entity, from, to)`.
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use adb_core::{Row, RowId, SchemaEpoch, SchemaVersion};
use serde::{Deserialize, Serialize};

use crate::{
    limits::MAX_ENTITIES,
    model::same_name,
    validate::{check_row, validate_chain, validate_schema, validate_successor},
    EntitySchema, RowAdapter, SchemaError,
};

/// A catalog change, as the log records it ([`adb_wal`'s `WalRecord::Catalog`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CatalogEvent {
    /// A new version of an entity's schema: version 1 for `CREATE TABLE`, the next version for
    /// `ALTER TABLE`.
    Schema(EntitySchema),
    /// `DROP TABLE`: the entity leaves the catalog; its id is never reused and its history is
    /// kept.
    DropEntity {
        /// The dropped entity.
        entity_id: u64,
    },
    /// The epoch moved without a schema change (`CREATE INDEX`, `DROP INDEX`): plans made before
    /// must be planned again.
    Epoch,
}

/// One entity's versions, oldest first, and when it was dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityHistory {
    /// Every version, oldest first (`versions[i].version == i + 1`).
    pub versions: Vec<Arc<EntitySchema>>,
    /// Epoch of the `DROP TABLE`, if the entity was dropped.
    pub dropped_at: Option<SchemaEpoch>,
}

impl EntityHistory {
    /// The newest version.
    pub fn latest(&self) -> &Arc<EntitySchema> {
        self.versions
            .last()
            .expect("a history holds at least one version")
    }

    /// The newest version published at or before `epoch`, if any.
    pub fn at_epoch(&self, epoch: SchemaEpoch) -> Option<&Arc<EntitySchema>> {
        self.versions
            .iter()
            .rev()
            .find(|version| version.epoch <= epoch)
    }

    /// Whether the entity exists at `epoch`: created by then and not yet dropped.
    pub fn is_live_at(&self, epoch: SchemaEpoch) -> bool {
        self.versions[0].epoch <= epoch && self.dropped_at.is_none_or(|dropped| dropped > epoch)
    }
}

/// Persistent form of a registry (the adapter cache is not persisted).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RegistryState {
    /// Current epoch.
    pub epoch: SchemaEpoch,
    /// Next entity id to hand out.
    pub next_entity_id: u64,
    /// Every entity ever created, by id.
    pub entities: BTreeMap<u64, EntityHistory>,
}

/// Every entity's schema history plus the catalog epoch.
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    /// The persistent state.
    state: RegistryState,
    /// Compiled adapters by `(entity, from, to)`.
    adapters: Mutex<HashMap<(u64, SchemaVersion, SchemaVersion), Arc<RowAdapter>>>,
}

impl SchemaRegistry {
    /// An empty registry at epoch 0 whose first entity gets id 1.
    pub fn new() -> Self {
        Self::from_state(RegistryState {
            next_entity_id: 1,
            ..RegistryState::default()
        })
        .expect("an empty registry is valid")
    }

    /// A registry from persisted state, validating every history (a damaged state is
    /// [`SchemaError::Corrupt`]).
    pub fn from_state(state: RegistryState) -> Result<Self, SchemaError> {
        let corrupt = |message: String| Err(SchemaError::Corrupt(message));
        if state.entities.len() > MAX_ENTITIES {
            return corrupt(format!("more than {MAX_ENTITIES} entities"));
        }
        for (id, history) in &state.entities {
            let versions: Vec<EntitySchema> =
                history.versions.iter().map(|v| (**v).clone()).collect();
            validate_chain(&versions)?;
            if versions[0].entity_id != *id || *id >= state.next_entity_id || *id == 0 {
                return corrupt(format!("entity {id} is misfiled"));
            }
            let last_epoch = history.latest().epoch;
            if last_epoch > state.epoch
                || history
                    .dropped_at
                    .is_some_and(|d| d <= last_epoch || d > state.epoch)
            {
                return corrupt(format!("entity {id} has epochs beyond the registry"));
            }
        }
        Ok(Self {
            state,
            adapters: Mutex::new(HashMap::new()),
        })
    }

    /// The persistent state (for checkpoints).
    pub fn state(&self) -> &RegistryState {
        &self.state
    }

    /// The current epoch.
    pub fn epoch(&self) -> SchemaEpoch {
        self.state.epoch
    }

    /// The id the next created entity gets.
    pub fn next_entity_id(&self) -> u64 {
        self.state.next_entity_id
    }

    /// The live (not dropped) entities' latest versions, in id order.
    pub fn entities(&self) -> impl Iterator<Item = &Arc<EntitySchema>> {
        self.state
            .entities
            .values()
            .filter(|history| history.dropped_at.is_none())
            .map(EntityHistory::latest)
    }

    /// The history of entity `id`, live or dropped.
    pub fn history(&self, id: u64) -> Option<&EntityHistory> {
        self.state.entities.get(&id)
    }

    /// The latest version of the live entity `id`.
    pub fn latest(&self, id: u64) -> Option<&Arc<EntitySchema>> {
        self.state
            .entities
            .get(&id)
            .filter(|history| history.dropped_at.is_none())
            .map(EntityHistory::latest)
    }

    /// The live entity named `name` (case-insensitive).
    pub fn by_name(&self, name: &str) -> Option<&Arc<EntitySchema>> {
        self.entities().find(|schema| same_name(&schema.name, name))
    }

    /// The version of entity `id` a reader pinned at `epoch` sees (`None` if the entity did not
    /// exist then or has no registered schema).
    pub fn at_epoch(&self, id: u64, epoch: SchemaEpoch) -> Option<&Arc<EntitySchema>> {
        self.state
            .entities
            .get(&id)
            .filter(|history| history.is_live_at(epoch))
            .and_then(|history| history.at_epoch(epoch))
    }

    /// Validates and applies one catalog event at `epoch` (which must be later than the current
    /// one). Nothing changes when it fails.
    pub fn apply(&mut self, epoch: SchemaEpoch, event: &CatalogEvent) -> Result<(), SchemaError> {
        let invalid = |message: String| Err(SchemaError::Invalid(message));
        if epoch <= self.state.epoch {
            return invalid(format!(
                "catalog epoch {} does not follow {}",
                epoch.0, self.state.epoch.0
            ));
        }
        match event {
            CatalogEvent::Schema(schema) => {
                if schema.epoch != epoch {
                    return invalid("a schema version carries the epoch it is published at".into());
                }
                validate_schema(schema)?;
                match self.state.entities.get(&schema.entity_id) {
                    None => {
                        if schema.version != SchemaVersion::FIRST {
                            return invalid(format!(
                                "entity {} starts at version {}",
                                schema.entity_id, schema.version.0
                            ));
                        }
                        // Every id ever used stays in `entities`, so an id not found there is free
                        // (an imported catalog may leave gaps below `next_entity_id`).
                        if schema.entity_id == 0 {
                            return invalid("entity id 0 is reserved".into());
                        }
                        if self.by_name(&schema.name).is_some() {
                            return invalid(format!(
                                "an entity named {} already exists",
                                schema.name
                            ));
                        }
                        if self.state.entities.len() >= MAX_ENTITIES {
                            return Err(SchemaError::Limit(format!(
                                "a database holds at most {MAX_ENTITIES} entities"
                            )));
                        }
                        self.state.entities.insert(
                            schema.entity_id,
                            EntityHistory {
                                versions: vec![Arc::new(schema.clone())],
                                dropped_at: None,
                            },
                        );
                        self.state.next_entity_id =
                            self.state.next_entity_id.max(schema.entity_id + 1);
                    }
                    Some(history) => {
                        if history.dropped_at.is_some() {
                            return invalid(format!("entity {} was dropped", schema.entity_id));
                        }
                        validate_successor(history.latest(), schema)?;
                        if let Some(other) = self.by_name(&schema.name) {
                            if other.entity_id != schema.entity_id {
                                return invalid(format!(
                                    "an entity named {} already exists",
                                    schema.name
                                ));
                            }
                        }
                        let history = self
                            .state
                            .entities
                            .get_mut(&schema.entity_id)
                            .expect("found");
                        history.versions.push(Arc::new(schema.clone()));
                    }
                }
            }
            CatalogEvent::DropEntity { entity_id } => {
                let Some(history) = self.state.entities.get_mut(entity_id) else {
                    return invalid(format!("entity {entity_id} does not exist"));
                };
                if history.dropped_at.is_some() {
                    return invalid(format!("entity {entity_id} was already dropped"));
                }
                history.dropped_at = Some(epoch);
            }
            CatalogEvent::Epoch => {}
        }
        self.state.epoch = epoch;
        Ok(())
    }

    /// The adapter from rows stamped `from` to version `to` of entity `id` (compiled once).
    ///
    /// # Panics
    ///
    /// If the entity or the target version does not exist.
    pub fn adapter(&self, id: u64, from: SchemaVersion, to: SchemaVersion) -> Arc<RowAdapter> {
        let from = from.effective();
        let mut cache = self.adapters.lock().expect("adapter cache lock");
        cache
            .entry((id, from, to))
            .or_insert_with(|| {
                let history = &self.state.entities[&id];
                Arc::new(RowAdapter::compile(history.latest(), from, to))
            })
            .clone()
    }

    /// `row`, about to be written under `row_id`, as the engine stores it: at the latest version
    /// of its entity, validated ([`check_row`]). Entities without a registered schema store rows
    /// as written.
    ///
    /// * An unversioned row is a new row written against the latest version: fields it omits
    ///   that have a default get the default (so a stored row never depends on a default).
    /// * A row of an older version (read, changed, written back) is adapted first: defaults it
    ///   predates are materialized and dropped fields are left out. This is how rows migrate
    ///   lazily: only rows that change are rewritten, and they reclaim dropped columns.
    /// * A row stamped with a version newer than the latest is refused.
    pub fn prepare_write(&self, row_id: RowId, row: Row) -> Result<Row, SchemaError> {
        let entity = row_id.entity_id();
        let Some(schema) = self.latest(entity) else {
            return Ok(row);
        };
        let row = if row.schema_version.is_unversioned() {
            let mut row = row.with_schema_version(schema.version);
            for field in schema.active_fields() {
                if let Some(default) = &field.default {
                    row.fields
                        .entry(field.id)
                        .or_insert_with(|| default.clone());
                }
            }
            row
        } else if row.schema_version <= schema.version {
            self.adapter(entity, row.schema_version, schema.version)
                .adapt(row)
        } else {
            return Err(SchemaError::Row {
                entity: schema.name.clone(),
                reason: format!(
                    "row is stamped version {}, newer than the latest version {}",
                    row.schema_version.0, schema.version.0
                ),
            });
        };
        check_row(schema, row_id, &row)?;
        Ok(row)
    }

    /// `row` of entity `id` as seen by a reader pinned at `epoch` (the latest version when
    /// `None`). Rows of entities without a registered schema are returned unchanged.
    pub fn adapt(&self, id: u64, row: Row, epoch: Option<SchemaEpoch>) -> Row {
        let target = match epoch {
            Some(epoch) => self.at_epoch(id, epoch),
            None => self.latest(id),
        };
        match target {
            Some(schema) => self
                .adapter(id, row.schema_version, schema.version)
                .adapt(row),
            None => row,
        }
    }
}
